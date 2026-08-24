//! The ground over the wire: what a client asks for, and what comes back.
//!
//! A child module rather than more of `tests.rs`, for `region_tests`' reason:
//! these read private world state and want a facet of their own, and they need
//! not pile into the same file.
//!
//! # The oracle is the shard's own snapshot, and then the whole facet
//!
//! Two assertions, and the second is the one that matters. A fragment joined and
//! decoded is compared against [`Chunk::of`] over the map the shard is holding —
//! which catches a byte lost in the framing — and then *every* chunk is put back
//! through [`assemble`], the same call `openshard_basemap::read` uses, and the
//! facet that comes out is compared tile by tile with the one that went in. A
//! wire that transposed a chunk, or dropped the statics of one block, passes the
//! first and fails the second.

use super::tests::{connection, enter_as, packets_for, world};
use super::*;
use openshard_map::chunk::{Chunk, ChunkCoord, assemble, chunks_of};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::snapshot::MapSnapshot;
use openshard_protocol::chunks::{ChunkAt, ChunkData, Refusal, WorldRevision, join};
use openshard_tiles::LandTileId;

/// Nine blocks square — 72 tiles — which is **not** a whole number of chunks on
/// either axis, so three of the four chunks are edge chunks: eight by eight,
/// eight by one, one by eight and one by one.
///
/// `openshard_map`'s own fixture is this shape and for this reason: Tokuno is
/// 181 blocks square, and a fixture that divided evenly would let a wire that
/// assumed a whole chunk pass.
const BLOCKS: u32 = 9;
/// The fixture's side in tiles.
const TILES: u32 = BLOCKS * 8;

/// The land of the fixture: every tile names its own coordinates, so a chunk
/// read transposed comes back holding a cell that says where it should have
/// been.
fn cell(x: u16, y: u16) -> LandCell {
    LandCell {
        tile: LandTileId(u16::try_from(u32::from(x) * TILES + u32::from(y)).unwrap()),
        z: (i32::from(x) - i32::from(y)) as i8,
    }
}

/// A facet with land that names itself and statics on both sides of both chunk
/// seams, in the far corner, and stacked three deep on one tile.
fn ground() -> WorldMap {
    let mut map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        cell,
    );
    let last = u16::try_from(TILES).unwrap() - 1;
    for (n, (x, y)) in [(0, 0), (63, 30), (64, 30), (30, 63), (30, 64), (last, last)]
        .into_iter()
        .enumerate()
    {
        map.place_static(StaticItem {
            tile: Graphic(0x100 + u16::try_from(n).unwrap()),
            x,
            y,
            z: i8::try_from(n).unwrap(),
            hue: Hue(0),
        });
    }
    // Three on one tile: the order they come back in is the order the client
    // draws them in, and the top one is the last.
    for n in 0..3u16 {
        map.place_static(StaticItem {
            tile: Graphic(0x200 + n),
            x: 20,
            y: 21,
            z: 5,
            hue: Hue(n),
        });
    }
    map
}

/// A world with that facet under it, spawning inside it.
fn world_with_ground() -> World {
    World::new((32, 32)).with_map(MapSnapshot::new(Facet(0), ground()))
}

/// Every chunk the fixture has, as the wire names them.
fn every_chunk() -> Vec<ChunkAt> {
    chunks_of(BlockExtent {
        wide: BLOCKS,
        down: BLOCKS,
    })
    .map(|at| ChunkAt {
        x: u16::try_from(at.x).unwrap(),
        y: u16::try_from(at.y).unwrap(),
    })
    .collect()
}

/// Ask for `wanted` and decode everything that comes back, in the order it was
/// sent.
///
/// Through the queue and a tick, not by calling the handler: what is under test
/// includes that a `0xBF 0xE002` becomes a command and that the command is
/// answered out of a tick like every other reply.
fn ask(world: &mut World, connection: ConnectionId, wanted: Vec<ChunkAt>) -> Vec<ServerPacket> {
    ask_on(world, connection, Facet(0), wanted)
}

/// The same, for a facet the caller names — including one the shard has never
/// heard of, which is a thing a client is free to ask about.
fn ask_on(
    world: &mut World,
    connection: ConnectionId,
    facet: Facet,
    wanted: Vec<ChunkAt>,
) -> Vec<ServerPacket> {
    world.queue(Command::RequestChunks {
        connection,
        facet,
        chunks: wanted,
    });
    world.tick(Instant::now());
    packets_for(world, connection)
        .iter()
        .filter_map(|bytes| {
            ServerPacket::decode(bytes, ClientVersion::TOL).expect("the shard's own bytes decode")
        })
        .collect()
}

/// The fragments of each chunk, joined back into its record, keyed by position.
///
/// Refusals are counted separately by the caller; this is only the ground that
/// actually arrived.
fn records(answers: &[ServerPacket]) -> Vec<(ChunkAt, Vec<u8>)> {
    let mut order: Vec<ChunkAt> = Vec::new();
    let mut pieces: Vec<(ChunkAt, Vec<ChunkData>)> = Vec::new();
    for packet in answers {
        let ServerPacket::ChunkData(data) = packet else {
            continue;
        };
        match pieces.iter_mut().find(|(at, _)| *at == data.at) {
            Some((_, held)) => held.push(data.clone()),
            None => {
                order.push(data.at);
                pieces.push((data.at, vec![data.clone()]));
            }
        }
    }
    order
        .into_iter()
        .map(|at| {
            let held = pieces
                .iter()
                .find(|(had, _)| *had == at)
                .map(|(_, held)| held.clone())
                .expect("a chunk that was listed was collected");
            (at, join(&held).expect("the fragments the shard sent join"))
        })
        .collect()
}

/// The whole of E1's "done": a chunk asked for over the command path comes back
/// equal to what `Chunk::of` cuts out of the shard's own snapshot, and the
/// facet reassembled from all of them is the facet the shard is holding.
#[test]
fn the_chunks_that_arrive_are_the_ground_the_shard_is_standing_on() {
    let mut world = world_with_ground();
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let wanted = every_chunk();
    let answers = ask(&mut world, connection, wanted.clone());
    assert!(
        answers
            .iter()
            .all(|packet| matches!(packet, ServerPacket::ChunkData(_))),
        "every chunk of the fixture exists, so nothing is refused"
    );

    let arrived = records(&answers);
    assert_eq!(arrived.len(), wanted.len(), "one record per chunk asked for");

    let snapshot = world
        .state
        .facet_state(Facet(0))
        .ground()
        .snapshot()
        .expect("the fixture has ground");
    let revision = WorldRevision(snapshot.revision().get());

    let mut decoded = Vec::new();
    for (at, record) in &arrived {
        let coord = ChunkCoord {
            x: u32::from(at.x),
            y: u32::from(at.y),
        };
        let cut = Chunk::of(snapshot, coord).expect("a chunk of this facet");
        assert_eq!(
            record,
            &codec::encode(&cut),
            "chunk ({}, {}) arrived as different bytes than the shard would cut",
            at.x,
            at.y
        );
        decoded.push(codec::decode(record).expect("a record we just compared"));
    }

    // Every fragment claimed the world's revision, which is what a cache is
    // compared against.
    for packet in &answers {
        let ServerPacket::ChunkData(data) = packet else {
            continue;
        };
        assert_eq!(data.revision, revision);
        assert_eq!(data.facet, Facet(0));
    }

    // And the strong half: the same `assemble` a base set is read through.
    let extent = BlockExtent {
        wide: BLOCKS,
        down: BLOCKS,
    };
    let rebuilt = assemble(Facet(0), extent, &decoded).expect("a complete set");
    let original = snapshot.map();
    assert_eq!(
        (rebuilt.width(), rebuilt.height()),
        (original.width(), original.height())
    );
    assert_eq!(rebuilt.static_count(), original.static_count());
    for y in 0..u16::try_from(TILES).unwrap() {
        for x in 0..u16::try_from(TILES).unwrap() {
            assert_eq!(
                rebuilt.land(x, y),
                original.land(x, y),
                "the ground at ({x}, {y})"
            );
            let was: Vec<_> = original.statics_at(x, y).collect();
            let is: Vec<_> = rebuilt.statics_at(x, y).collect();
            assert_eq!(was, is, "the statics at ({x}, {y})");
        }
    }
}

/// A chunk the facet does not reach is refused, and refused *by name*: the
/// alternative — silence — is a client waiting on a packet that is never coming,
/// and there is nothing else in this conversation to end the wait.
#[test]
fn a_chunk_past_the_edge_is_refused_rather_than_answered_with_something() {
    let mut world = world_with_ground();
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let past = ChunkAt { x: 2, y: 0 };
    let answers = ask(&mut world, connection, vec![past]);
    match answers.as_slice() {
        [ServerPacket::ChunkRefused(refused)] => {
            assert_eq!(refused.at, past);
            assert_eq!(refused.facet, Facet(0));
            assert_eq!(refused.reason, Refusal::PastTheEdge);
        }
        other => panic!("a chunk off the facet was answered with {other:?}"),
    }
}

/// A shard with no ground for the facet says so, once per chunk, rather than
/// going quiet. This is the ordinary mode of a shard with no client files.
#[test]
fn a_facet_with_no_ground_refuses_every_chunk_asked_for() {
    let mut world = world();
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let wanted = vec![ChunkAt { x: 0, y: 0 }, ChunkAt { x: 5, y: 5 }];
    let answers = ask(&mut world, connection, wanted.clone());
    let refused: Vec<_> = answers
        .iter()
        .filter_map(|packet| match packet {
            ServerPacket::ChunkRefused(refused) => Some((refused.at, refused.reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        refused,
        wanted
            .into_iter()
            .map(|at| (at, Refusal::NoWorld))
            .collect::<Vec<_>>()
    );
    assert_eq!(refused.len(), answers.len(), "and nothing else was sent");
}

/// A facet byte naming a facet this shard never loaded is refused, not indexed.
///
/// The client picks that byte, so it is an input and not an invariant — which is
/// why the reader goes through
/// [`facet_state_if_loaded`](openshard_state::WorldState::facet_state_if_loaded)
/// and not through the accessor that panics for a facet an *entity* carries. The
/// difference between the two is a shard that says no and a shard that drops the
/// connection.
#[test]
fn a_facet_the_shard_never_loaded_is_refused_rather_than_panicked_on() {
    let mut world = world_with_ground();
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let elsewhere = Facet(3);
    assert!(
        world.state.facet_state_if_loaded(elsewhere).is_none(),
        "the fixture loads facet 0 alone"
    );
    let wanted = vec![ChunkAt { x: 0, y: 0 }, ChunkAt { x: 1, y: 0 }];
    let answers = ask_on(&mut world, connection, elsewhere, wanted.clone());
    let refused: Vec<_> = answers
        .iter()
        .filter_map(|packet| match packet {
            ServerPacket::ChunkRefused(refused) => Some((refused.facet, refused.at, refused.reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        refused,
        wanted
            .into_iter()
            .map(|at| (elsewhere, at, Refusal::NoWorld))
            .collect::<Vec<_>>()
    );
    assert_eq!(refused.len(), answers.len(), "and no ground came back");
}

/// The rule the whole conversation rests on, over a request that mixes both
/// answers: every chunk named is spoken for exactly once, in the order asked.
#[test]
fn every_chunk_named_is_answered_exactly_once() {
    let mut world = world_with_ground();
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let wanted = vec![
        ChunkAt { x: 1, y: 1 },
        ChunkAt { x: 9, y: 9 },
        ChunkAt { x: 0, y: 0 },
        ChunkAt { x: 0, y: 9 },
    ];
    let answers = ask(&mut world, connection, wanted.clone());

    // A chunk is one refusal or one or more fragments, and the first fragment of
    // each is where it takes its turn.
    let mut spoken: Vec<ChunkAt> = Vec::new();
    for packet in &answers {
        match packet {
            ServerPacket::ChunkData(data) if data.fragment.index() == 0 => spoken.push(data.at),
            ServerPacket::ChunkData(_) => {}
            ServerPacket::ChunkRefused(refused) => spoken.push(refused.at),
            other => panic!("a chunk request was answered with {other:?}"),
        }
    }
    assert_eq!(spoken, wanted);
}

/// A request naming nothing is answered with nothing — which is not the same as
/// a chunk going unanswered, and is what a client with an up-to-date cache sends.
#[test]
fn a_request_for_no_chunks_is_answered_with_no_chunks() {
    let mut world = world_with_ground();
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    assert!(ask(&mut world, connection, Vec::new()).is_empty());
}

/// A client of ours learns what world it is standing in on the way in, because
/// it cannot ask for a chunk of a facet whose size it does not know.
#[test]
fn a_client_entering_is_told_what_world_it_is_in() {
    let mut world = world_with_ground();
    let connection = enter_as(&mut world, connection(), Instant::now());

    let notices: Vec<_> = packets_for(&mut world, connection)
        .iter()
        .filter_map(|bytes| ServerPacket::decode(bytes, ClientVersion::TOL).expect("our own bytes"))
        .filter_map(|packet| match packet {
            ServerPacket::WorldNotice(notice) => Some(notice),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "sent once, on world entry");
    assert_eq!(notices[0].facet, Facet(0));
    assert_eq!(notices[0].blocks.wide, BLOCKS);
    assert_eq!(notices[0].blocks.down, BLOCKS);
    assert_eq!(notices[0].revision.0, 1, "a facet nobody has patched");

    // And the extent is the one `assemble` wants: the chunks of a facet this
    // size are exactly the chunks the shard will answer for.
    assert_eq!(every_chunk().len(), 4);
}

/// A facet with no map sends none: a notice of nought blocks by nought would be
/// a world a client could ask for chunks of, described as though it could.
#[test]
fn a_shard_with_no_ground_says_nothing_about_a_world_it_has_not_got() {
    let mut world = world();
    let connection = enter_as(&mut world, connection(), Instant::now());

    assert!(
        packets_for(&mut world, connection)
            .iter()
            .filter_map(|bytes| ServerPacket::decode(bytes, ClientVersion::TOL).expect("our own bytes"))
            .all(|packet| !matches!(packet, ServerPacket::WorldNotice(_))),
    );
}
