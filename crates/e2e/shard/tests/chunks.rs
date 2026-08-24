//! A client asks the shard for the ground, and the ground arrives.
//!
//! Every layer has a test where it lives: the packets round-trip in
//! `openshard-protocol`, the fragmenting and joining are a pair of functions
//! there with one round trip over them, and `openshard-world`'s `chunks_tests`
//! puts a whole fixture facet through the command path and reassembles it. What
//! none of them can catch is the seam this file is about — that a `0xBF 0xE002`
//! written by a client of ours **over a real socket** reaches the tick at all,
//! that the answer survives the shard's own per-write compression, and that the
//! bytes at the far end are the bytes the base set on disk holds.
//!
//! # Why it is `#[ignore]`
//!
//! `map_edit`'s reason exactly: it reads the install's `tiledata.mul` and bakes
//! a navigation graph, which is seconds rather than milliseconds, and a shard
//! cannot boot on a base set without both.
//!
//! ```sh
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
//!     cargo test -p openshard-e2e-shard --test chunks -- --ignored --nocapture
//! ```
//!
//! # The world it fetches is its own
//!
//! A small base set in a temp directory, written and baked by the test —
//! `map_edit`'s argument again, and here there is a second one: the oracle is
//! that file, read back through `openshard_basemap::read`, so the test needs a
//! world it knows every byte of.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::transport::{Socket, enter_world};
use openshard_config::{Config, FacetKey};
use openshard_map::chunk::{Chunk, ChunkCoord, assemble, chunks_of};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::overlay::{Doors, Overlay};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::spans::SpanIndex;
use openshard_movement::{Footing, MapTerrain, NavigationGraph, bake};
use openshard_protocol::chunks::{ChunkAt, ChunkData, ChunkRequest, Refusal, join};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;
use tokio::net::TcpStream;

use openshard_e2e_shard::{plan, spawn, stock_config, version};

const FACET: Facet = Facet(0);
/// 32 blocks square — 256×256 tiles, which is sixteen chunks and bakes in well
/// under a second.
const BLOCKS: u32 = 32;
/// Chunks along each side: `BLOCKS / 8`.
const CHUNKS: u16 = 4;
const START: (u16, u16) = (128, 128);
/// The ground the fixture is made of: grass, flat, at zero.
///
/// Uniform on purpose. What makes the oracle sharp is not the land but the
/// comparison — every record is checked byte for byte against what `Chunk::of`
/// cuts out of the file — and a fixture whose heights wandered would be a
/// fixture whose navigation bake could refuse to build for reasons that have
/// nothing to do with this test.
const GROUND: LandCell = LandCell {
    tile: LandTileId(3),
    z: 0,
};

/// The install, from the environment. `None` skips the test rather than failing
/// it — `map_edit`'s argument.
fn install() -> Option<PathBuf> {
    std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from)
}

/// The fixture's statics: one on each side of both chunk seams, one in the far
/// corner, and two stacked on one tile.
///
/// These are what a transposed or truncated chunk gets wrong. Their graphics are
/// nothing `tiledata.mul` describes, which is deliberate — an unknown graphic
/// stands in nobody's way, so the navigation bake is unaffected by them.
fn statics() -> Vec<StaticItem> {
    let last = u16::try_from(BLOCKS * 8).unwrap() - 1;
    let mut items: Vec<StaticItem> = [(0, 0), (63, 30), (64, 30), (30, 63), (30, 64), (last, last)]
        .into_iter()
        .enumerate()
        .map(|(n, (x, y))| StaticItem {
            tile: Graphic(0x4000 + u16::try_from(n).unwrap()),
            x,
            y,
            z: i8::try_from(n).unwrap(),
            hue: Hue(0),
        })
        .collect();
    for n in 0..2u16 {
        items.push(StaticItem {
            tile: Graphic(0x4100 + n),
            x: 20,
            y: 21,
            z: 5,
            hue: Hue(n),
        });
    }
    items
}

/// Write a small world of ours into `dir`, bake its graph beside it, and hand
/// back the base set's path.
///
/// `openshard-map-import` and `openshard-navigation-bake` in miniature, through
/// the same functions — `map_edit`'s reasoning, and the same code shape.
fn world_of_ours(dir: &Path, client: &Path) -> PathBuf {
    let base_set = dir.join("fixture.osbase");
    let mut map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| GROUND,
    );
    for item in statics() {
        map.place_static(item);
    }
    let snapshot = MapSnapshot::new(FACET, map);
    openshard_basemap::write(&base_set, &snapshot).expect("a writable temp directory");

    let tiledata = client.join("tiledata.mul");
    let tiles = openshard_uofiles::tiledata::load(&tiledata)
        .expect("the install has a tile table")
        .tiles;
    let spans = SpanIndex::build(snapshot.map(), &tiles);
    let nothing_placed = Overlay::default();
    let footing = Footing::new(
        Some(MapTerrain::new(snapshot.map(), &tiles, &spans)),
        &nothing_placed,
        Doors::AsTheyStand,
    );
    let graph = NavigationGraph::build(&footing, snapshot.map().width(), snapshot.map().height())
        .expect("a facet this size has a graph");
    let stamp = bake::stamp_of_base_set(&base_set, None, &tiledata, FACET, snapshot.revision())
        .expect("the two inputs exist");
    bake::save(
        &bake::artifact_path(bake::beside(&base_set), FACET),
        &graph,
        &stamp,
    )
    .expect("a writable temp directory");

    base_set
}

/// The stock config, pointed at a world of ours.
fn config_over(base_set: PathBuf, client: PathBuf) -> impl FnOnce(SocketAddr) -> Config + Send {
    move |address| {
        let mut config = stock_config(address);
        config.world.client_files = client.display().to_string();
        config.world.facets = vec![FACET.0];
        config.world.base_sets.insert(FacetKey(FACET), base_set);
        config.world.start.x = START.0;
        config.world.start.y = START.1;
        config
    }
}

/// Read packets until `done` is satisfied, or give up.
///
/// A predicate and not a count: how many packets one chunk arrives in is the
/// fragmenting's business, and a test that pinned a number here would fail the
/// day a chunk crossed the cap for an honest reason. The timeout is a bound
/// rather than a wait — a conversation that has gone wrong here hangs, it does
/// not error.
async fn hear(
    socket: &mut Socket<TcpStream>,
    mut done: impl FnMut(&[ServerPacket]) -> bool,
) -> Vec<ServerPacket> {
    let mut heard: Vec<ServerPacket> = Vec::new();
    let collecting = async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            if let Event::Packet(packet) = event {
                heard.push(packet);
                if done(&heard) {
                    return;
                }
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(20), collecting).await;
    heard
}

/// Every chunk of the fixture, as the wire names them.
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

#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_client_asks_for_the_ground_and_gets_the_shards_own_bytes() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("openshard-chunks-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a writable temp directory");
    let base_set = world_of_ours(&dir, &client);

    // Held for the length of the test: dropping the handle stops the shard.
    let (address, shard) = spawn(config_over(base_set.clone(), client));
    let entered = tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, view) = entered;

    // First: the notice, which is what a client needs before it can ask for
    // anything. It arrived during the login conversation, unasked, and the view
    // is where it landed.
    let notice = view.world.expect("the shard told us what world we are in");
    assert_eq!(notice.facet, FACET);
    assert_eq!(notice.blocks.wide, BLOCKS);
    assert_eq!(notice.blocks.down, BLOCKS);
    assert_eq!(notice.revision.0, 1, "a fresh base set nobody has patched");

    // Then the ground. Sixteen chunks, which is inside one request's cap.
    let wanted = every_chunk();
    assert_eq!(wanted.len(), usize::from(CHUNKS) * usize::from(CHUNKS));
    socket
        .send(
            &ChunkRequest {
                facet: FACET,
                chunks: wanted.clone(),
            }
            .encode(),
        )
        .await
        .expect("the shard is listening");

    let heard = hear(&mut socket, |so_far| {
        so_far
            .iter()
            .filter(|packet| matches!(packet, ServerPacket::ChunkData(data) if data.fragment.is_last()))
            .count()
            == wanted.len()
    })
    .await;

    // Group the fragments by chunk and join each — the client's half of the
    // pair, and the one E2 will do for a whole facet.
    let mut records: Vec<(ChunkAt, Vec<u8>)> = Vec::new();
    for at in &wanted {
        let pieces: Vec<ChunkData> = heard
            .iter()
            .filter_map(|packet| match packet {
                ServerPacket::ChunkData(data) if data.at == *at => Some(data.clone()),
                _ => None,
            })
            .collect();
        assert!(!pieces.is_empty(), "chunk ({}, {}) never arrived", at.x, at.y);
        records.push((*at, join(&pieces).expect("the fragments the shard sent join")));
    }

    // The oracle: the file on disk, read back through the reader a shard boots
    // with. Byte for byte, because the chunk encoding is canonical — one world
    // has exactly one byte string per chunk.
    let on_disk = openshard_basemap::read(&base_set).expect("the base set reads back");
    let mut decoded = Vec::new();
    for (at, record) in &records {
        let coord = ChunkCoord {
            x: u32::from(at.x),
            y: u32::from(at.y),
        };
        let cut = Chunk::of(&on_disk, coord).expect("a chunk of this facet");
        assert_eq!(
            record,
            &codec::encode(&cut),
            "chunk ({}, {}) came off the wire as different bytes than the file holds",
            at.x,
            at.y
        );
        decoded.push(codec::decode(record).expect("a record we just compared"));
    }

    // And the whole facet, through the same `assemble` a base set is read
    // through: a wire that transposed a chunk passes the comparison above only
    // if it also transposed the file, and this is what says it did not.
    let extent = BlockExtent {
        wide: BLOCKS,
        down: BLOCKS,
    };
    let rebuilt = assemble(FACET, extent, &decoded).expect("a complete set");
    assert_eq!(rebuilt.static_count(), on_disk.map().static_count());
    for item in statics() {
        let there: Vec<StaticItem> = rebuilt.statics_at(item.x, item.y).copied().collect();
        assert!(
            there.contains(&item),
            "the static at ({}, {}) did not survive the wire: {there:?}",
            item.x,
            item.y
        );
    }

    // The other half of E1's "done": a chunk the facet does not reach is
    // refused by name rather than answered with something, or with nothing.
    let past = ChunkAt { x: CHUNKS, y: 0 };
    socket
        .send(
            &ChunkRequest {
                facet: FACET,
                chunks: vec![past],
            }
            .encode(),
        )
        .await
        .expect("the shard is listening");
    let refused = hear(&mut socket, |so_far| {
        so_far
            .iter()
            .any(|packet| matches!(packet, ServerPacket::ChunkRefused(_)))
    })
    .await;
    let refusal = refused
        .iter()
        .find_map(|packet| match packet {
            ServerPacket::ChunkRefused(refused) => Some(*refused),
            _ => None,
        })
        .expect("a chunk off the facet is refused rather than left unanswered");
    assert_eq!(refusal.at, past);
    assert_eq!(refusal.facet, FACET);
    assert_eq!(refusal.reason, Refusal::PastTheEdge);
    assert!(
        !refused
            .iter()
            .any(|packet| matches!(packet, ServerPacket::ChunkData(_))),
        "and nothing was sent for it"
    );

    shard.stop();
    std::fs::remove_dir_all(&dir).ok();
}
