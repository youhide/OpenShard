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

use std::path::{Path, PathBuf};

use super::tests::{connection, enter_as, packets_for, world};
use super::*;
use openshard_map::chunk::{Chunk, ChunkCoord, assemble, chunks_of};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::chunks::{
    Changes, ChangesReply, ChunkAt, ChunkData, PublishNotice, Refusal, WorldNotice, WorldRevision, join,
};
use openshard_state::WorldHome;
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
///
/// **No home**, so it is a facet read out of an install as far as everything
/// below is concerned: nothing can be committed to it and it has no identity to
/// send. [`world_of_ours`] is the other one.
fn world_with_ground() -> World {
    World::new((32, 32)).with_map(MapSnapshot::new(Facet(0), ground()))
}

/// The same facet, written to a base set in the temp dir and loaded back — a
/// world of *ours*, which is the only kind that can say what moved.
///
/// The tag keeps two tests in one binary off each other's files and the pid
/// keeps two runs off each other's, which is `tests/mapedit.rs`'s rule and
/// `openshard-basemap`'s before it. `base` is the revision the file is written
/// at: a base set at something other than the first revision is what makes
/// "older than this world" a state a client can be in.
fn world_of_ours(tag: &str, base: MapRevision) -> (World, PathBuf) {
    let path = std::env::temp_dir().join(format!("openshard-changes-{tag}-{}.osbase", std::process::id()));
    std::fs::remove_file(&path).ok();
    std::fs::remove_file(openshard_basemap::patches::log_path(&path)).ok();
    let written = MapSnapshot::restored(Facet(0), base, ground());
    openshard_basemap::write(&path, &written).expect("a writable temp dir");

    let loaded = openshard_basemap::load(&path).expect("the base set just written");
    let home = WorldHome {
        base_set: path.clone(),
        base: loaded.base,
        identity: openshard_basemap::identity_of(&path).expect("the base set just written"),
    };
    let world = World::new((32, 32)).with_facet(
        Facet(0),
        loaded.snapshot,
        None,
        openshard_state::facet_rules::FacetRules::classic(Facet(0)),
        Some(home),
    );
    (world, path)
}

/// Everything `world_of_ours` left in the temp dir.
fn clean(base_set: &Path) {
    std::fs::remove_file(openshard_basemap::patches::log_path(base_set)).ok();
    std::fs::remove_file(base_set).ok();
}

/// Move one tile of ground and write it down, the way an operator's `.setland`
/// does — through [`mapedit::commit`], so the log on disk is the one a real
/// shard would have written.
fn commit_a_tile(world: &mut World, at: (u16, u16)) -> MapRevision {
    let parent = world
        .state
        .facet_state(Facet(0))
        .ground()
        .snapshot()
        .expect("the fixture has ground")
        .revision();
    let map = world
        .state
        .facet_state(Facet(0))
        .ground()
        .snapshot()
        .expect("the fixture has ground")
        .map();
    let op = PatchOp::set_land(
        map,
        at.0,
        at.1,
        LandCell {
            tile: LandTileId(0x3FF),
            z: 7,
        },
    )
    .expect("a tile of this facet");
    let patch = Patch::new(
        Facet(0),
        parent,
        PatchAuthor("a test".to_owned()),
        PatchTime(0),
        vec![op],
    );
    crate::mapedit::commit(&mut world.state, Facet(0), &patch).expect("a world of ours takes a patch")
}

/// The one world notice a connection was sent on the way in, if it was sent one.
///
/// It drains the entry packets, so a caller that wants them for something else
/// has to take them first.
fn notice_for(world: &mut World, connection: ConnectionId) -> Option<WorldNotice> {
    let notices: Vec<WorldNotice> = packets_for(world, connection)
        .iter()
        .filter_map(|bytes| ServerPacket::decode(bytes, ClientVersion::TOL).expect("our own bytes"))
        .filter_map(|packet| match packet {
            ServerPacket::WorldNotice(notice) => Some(notice),
            _ => None,
        })
        .collect();
    assert!(notices.len() <= 1, "a notice is sent once, on world entry");
    notices.into_iter().next()
}

/// Ask what has moved since `held`, through the queue and a tick.
fn ask_changes(world: &mut World, connection: ConnectionId, held: WorldRevision) -> ChangesReply {
    world.queue(Command::RequestChanges {
        connection,
        facet: Facet(0),
        revision: held,
    });
    world.tick(Instant::now());
    let replies: Vec<ChangesReply> = packets_for(world, connection)
        .iter()
        .filter_map(|bytes| ServerPacket::decode(bytes, ClientVersion::TOL).expect("the shard's own bytes"))
        .filter_map(|packet| match packet {
            ServerPacket::ChangesReply(reply) => Some(reply),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 1, "one question, one answer");
    replies.into_iter().next().expect("just counted")
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

    let notice = notice_for(&mut world, connection).expect("sent once, on world entry");
    assert_eq!(notice.facet, Facet(0));
    assert_eq!(notice.blocks.wide, BLOCKS);
    assert_eq!(notice.blocks.down, BLOCKS);
    assert_eq!(notice.revision.0, 1, "a facet nobody has patched");

    // And the extent is the one `assemble` wants: the chunks of a facet this
    // size are exactly the chunks the shard will answer for.
    assert_eq!(every_chunk().len(), 4);
}

/// A world of ours is named on the way in, and one out of an install is not.
///
/// The identity is what a client files its cache under, so the `None` half is
/// the load-bearing one: a facet the shard cannot name is a facet a client must
/// not keep, because nothing afterwards could tell that copy from another
/// install's Felucca.
#[test]
fn a_world_of_ours_is_named_and_one_out_of_an_install_is_not() {
    {
        let (mut world, base_set) = world_of_ours("named", MapRevision::INITIAL);
        let entered = enter_as(&mut world, connection(), Instant::now());
        let named = notice_for(&mut world, entered).expect("a facet with ground sends one");
        assert_eq!(
            named.world,
            Some(openshard_basemap::identity_of(&base_set).expect("the base set")),
            "the notice names the world the shard read"
        );
        clean(&base_set);
    }

    let mut world = world_with_ground();
    let entered = enter_as(&mut world, connection(), Instant::now());
    let unnamed = notice_for(&mut world, entered).expect("a facet with ground sends one");
    assert_eq!(unnamed.world, None, "a facet with no base set has no identity");
}

/// A client already holding the world is told that nothing moved — which is
/// knowledge and not a refusal, and is what makes a second run ask for no chunks
/// at all.
#[test]
fn a_client_that_is_up_to_date_is_told_that_nothing_moved() {
    let (mut world, base_set) = world_of_ours("current", MapRevision::INITIAL);
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let reply = ask_changes(&mut world, connection, WorldRevision(MapRevision::INITIAL.get()));
    assert_eq!(reply.facet, Facet(0));
    assert_eq!(reply.revision.0, MapRevision::INITIAL.get());
    assert_eq!(reply.changes, Changes::These(Vec::new()));
    clean(&base_set);
}

/// E3's "done", on the shard's side: what a stale client is told is exactly the
/// chunks the patches since its revision touched — no more, and never the facet.
///
/// Two patches, in two different chunks, and the answer is asked for from three
/// vantage points: before both, between them, and after both.
#[test]
fn what_moved_is_the_chunks_the_patches_touched() {
    let (mut world, base_set) = world_of_ours("moved", MapRevision::INITIAL);
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    let first = WorldRevision(MapRevision::INITIAL.get());
    // A tile in chunk (0, 0), then one in chunk (1, 1): the fixture is 72 tiles
    // square, so tile 70 is in the second chunk on both axes.
    let second = WorldRevision(commit_a_tile(&mut world, (3, 4)).get());
    let third = WorldRevision(commit_a_tile(&mut world, (70, 71)).get());
    assert_eq!(second.0, first.0 + 1);
    assert_eq!(third.0, second.0 + 1);

    let reply = ask_changes(&mut world, connection, first);
    assert_eq!(reply.revision, third, "the revision it will be at once applied");
    assert_eq!(
        reply.changes,
        Changes::These(vec![ChunkAt { x: 0, y: 0 }, ChunkAt { x: 1, y: 1 }]),
        "both patches, each chunk named once"
    );

    let reply = ask_changes(&mut world, connection, second);
    assert_eq!(
        reply.changes,
        Changes::These(vec![ChunkAt { x: 1, y: 1 }]),
        "only the patch it has not seen"
    );

    let reply = ask_changes(&mut world, connection, third);
    assert_eq!(reply.changes, Changes::These(Vec::new()));
    clean(&base_set);
}

/// Every revision this shard cannot describe a difference to, and there are
/// three of them: one it has never published, one from before this base set
/// existed, and any at all on a facet it does not own.
///
/// All three are one answer — take the facet again — because the client does the
/// same thing with each. What separates them is a line in the shard's log.
#[test]
fn a_revision_this_shard_cannot_place_is_answered_with_the_whole_facet() {
    {
        let (mut world, base_set) = world_of_ours("ahead", MapRevision::INITIAL);
        let entered = enter_as(&mut world, connection(), Instant::now());
        let _entry = packets_for(&mut world, entered);
        let reply = ask_changes(&mut world, entered, WorldRevision(99));
        assert_eq!(reply.changes, Changes::Everything, "a client from the future");
        assert_eq!(reply.revision.0, 1, "and it is told where the world actually is");
        clean(&base_set);
    }
    {
        // A base set written at a later revision: a client claiming an earlier
        // one holds a world this log has no record of reaching.
        let (mut world, base_set) = world_of_ours("older", MapRevision::decoded(5));
        let entered = enter_as(&mut world, connection(), Instant::now());
        let _entry = packets_for(&mut world, entered);
        let reply = ask_changes(&mut world, entered, WorldRevision(3));
        assert_eq!(reply.changes, Changes::Everything);
        assert_eq!(reply.revision.0, 5);
        clean(&base_set);
    }

    // And a facet with ground but no base set: there is no log to read, so
    // nothing here knows what moved. Such a facet is sent with no identity, so a
    // client should never have a copy of it to ask about — which is why this is
    // an answer rather than a way to be wrong.
    let mut world = world_with_ground();
    let entered = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, entered);
    let reply = ask_changes(&mut world, entered, WorldRevision(1));
    assert_eq!(reply.changes, Changes::Everything);
}

/// E4's "done", on the shard's side: a commit tells everyone standing on the
/// facet which revision it moved to and which chunk moved.
///
/// Two connections, because the audience is the facet and not the operator: the
/// second character never typed anything and is told the same thing.
#[test]
fn a_commit_tells_everyone_on_the_facet_what_moved() {
    let (mut world, base_set) = world_of_ours("published", MapRevision::INITIAL);
    let operator = enter_as(&mut world, connection(), Instant::now());
    let bystander = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, operator);
    let _entry = packets_for(&mut world, bystander);

    // A tile in chunk (1, 1): the fixture is 72 tiles square and cut into
    // sixty-four-tile squares, so tile 70 is in the second chunk on both axes.
    let revision = commit_a_tile(&mut world, (70, 71));

    for connection in [operator, bystander] {
        let notices = publish_notices(&mut world, connection);
        assert_eq!(notices.len(), 1, "one commit, one notice");
        assert_eq!(notices[0].facet, Facet(0));
        assert_eq!(notices[0].revision.0, revision.get());
        assert_eq!(
            notices[0].changes,
            Changes::These(vec![ChunkAt { x: 1, y: 1 }]),
            "the chunk the patch touched, and not the facet"
        );
    }
    clean(&base_set);
}

/// A commit that the log refuses tells nobody anything.
///
/// The world moved and was put back, so the revision the notice would have named
/// is one that never existed — and a client that fetched chunks of it would be
/// told they are at a revision it has never heard of.
#[test]
fn a_commit_the_log_refuses_is_not_announced() {
    let (mut world, base_set) = world_of_ours("unlogged", MapRevision::INITIAL);
    let connection = enter_as(&mut world, connection(), Instant::now());
    let _entry = packets_for(&mut world, connection);

    // A directory where the log file should be: appending to it cannot succeed,
    // and nothing else about the world is different. `tests/mapedit.rs` breaks
    // the log the same way and for the same reason.
    let log = openshard_basemap::patches::log_path(&base_set);
    std::fs::remove_file(&log).ok();
    std::fs::create_dir_all(&log).expect("a writable temp dir");

    let parent = world
        .state
        .facet_state(Facet(0))
        .ground()
        .snapshot()
        .expect("the fixture has ground")
        .revision();
    let map = world
        .state
        .facet_state(Facet(0))
        .ground()
        .snapshot()
        .expect("the fixture has ground")
        .map();
    let op = PatchOp::set_land(
        map,
        3,
        4,
        LandCell {
            tile: LandTileId(0x3FF),
            z: 7,
        },
    )
    .expect("a tile of this facet");
    let patch = Patch::new(
        Facet(0),
        parent,
        PatchAuthor("a test".to_owned()),
        PatchTime(0),
        vec![op],
    );
    assert!(
        crate::mapedit::commit(&mut world.state, Facet(0), &patch).is_err(),
        "a log that is a directory takes nothing"
    );
    assert!(
        publish_notices(&mut world, connection).is_empty(),
        "the world was put back, so there is nothing to announce"
    );

    std::fs::remove_dir_all(&log).ok();
    clean(&base_set);
}

/// Every publish notice a connection has been sent since the last drain.
fn publish_notices(world: &mut World, connection: ConnectionId) -> Vec<PublishNotice> {
    packets_for(world, connection)
        .iter()
        .filter_map(|bytes| ServerPacket::decode(bytes, ClientVersion::TOL).expect("our own bytes"))
        .filter_map(|packet| match packet {
            ServerPacket::PublishNotice(notice) => Some(notice),
            _ => None,
        })
        .collect()
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
