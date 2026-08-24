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
//!
//! # Two tests, and they are two phases
//!
//! The first is E1's and is about the **wire**: sixteen chunks asked for by
//! hand, and every record compared byte for byte against what `Chunk::of` cuts
//! out of the file. The second is E2's and is about the **client**: it drives
//! `openshard_client_net::chunks::Fetch` — the thing a window runs — over
//! eighty-one chunks, which is more than one request may name, and asks whether
//! the facet it ends up holding is the shard's facet tile for tile.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openshard_client_net::chunks::Fetch;
use openshard_client_net::connection::Event;
use openshard_client_net::talk;
use openshard_client_net::transport::{Socket, enter_world};
use openshard_config::{Config, FacetKey, RawAccessLevel};
use openshard_map::chunk::{Chunk, ChunkCoord, assemble, chunks_of};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::overlay::{Doors, Overlay};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::spans::SpanIndex;
use openshard_movement::{Footing, MapTerrain, NavigationGraph, bake};
use openshard_protocol::chunks::{
    Changes, ChangesRequest, ChunkAt, ChunkData, ChunkRequest, Refusal, WorldRevision, join,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::TalkMode;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;
use tokio::net::TcpStream;

use openshard_e2e_shard::{ACCOUNT, plan, spawn, stock_config, version};

const FACET: Facet = Facet(0);
/// 32 blocks square — 256×256 tiles, which is sixteen chunks and bakes in well
/// under a second.
const BLOCKS: u32 = 32;
/// Chunks along each side: `BLOCKS / 8`.
const CHUNKS: u16 = 4;
/// 72 blocks square: **81 chunks, which is more than one request may name.**
///
/// The second test's fixture, and the size is the whole reason it is a second
/// one. Sixteen chunks fit in a single `0xE002` and prove nothing about a client
/// that has to pace itself; eighty-one are two requests, so the send-then-read
/// loop is exercised over a socket rather than only against a fixture.
const WIDE_BLOCKS: u32 = 72;
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
fn statics(blocks: u32) -> Vec<StaticItem> {
    let last = u16::try_from(blocks * 8).unwrap() - 1;
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
fn world_of_ours(dir: &Path, client: &Path, blocks: u32) -> PathBuf {
    let base_set = dir.join("fixture.osbase");
    let mut map = WorldMap::from_blocks(
        BlockExtent {
            wide: blocks,
            down: blocks,
        },
        |_, _| GROUND,
    );
    for item in statics(blocks) {
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
///
/// The development account is promoted, so that a `.`-command is a command
/// rather than speech — `map_edit`'s rule, and the third test below is why this
/// one wants it too: it moves the ground the way an operator does.
fn config_over(base_set: PathBuf, client: PathBuf) -> impl FnOnce(SocketAddr) -> Config + Send {
    move |address| {
        let mut config = stock_config(address);
        config.world.client_files = client.display().to_string();
        config.world.facets = vec![FACET.0];
        config.world.base_sets.insert(FacetKey(FACET), base_set);
        config.world.start.x = START.0;
        config.world.start.y = START.1;
        for account in &mut config.accounts {
            if account.name == ACCOUNT {
                account.access = RawAccessLevel("administrator".to_owned());
            }
        }
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

/// Every chunk of a fixture that size, as the wire names them.
fn every_chunk(blocks: u32) -> Vec<ChunkAt> {
    chunks_of(BlockExtent {
        wide: blocks,
        down: blocks,
    })
    .map(|at| ChunkAt {
        x: u16::try_from(at.x).unwrap(),
        y: u16::try_from(at.y).unwrap(),
    })
    .collect()
}

/// A temp directory of this test's own, removed when it passes.
///
/// Named for the test as well as the process: two of these run in one binary and
/// in parallel, and a shared directory would be one bake racing another.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openshard-{name}-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a writable temp directory");
    dir
}

/// Run a fetch to completion against a live shard, and say how many chunks it
/// asked for.
///
/// The loop `link.rs` runs — ask until the window is full, then read until
/// something completes — written out rather than called, for the reason the test
/// below gives: the loop is private to a crate that cannot see a shard. The
/// count is the assertion E3 is about, so it is the return value rather than a
/// thing to count afterwards.
async fn drive(socket: &mut Socket<TcpStream>, fetch: &mut Fetch) -> Asked {
    let mut asked = Asked {
        requests: 0,
        chunks: 0,
    };
    let fetching = async {
        loop {
            while let Some(request) = fetch.next_request() {
                asked.requests += 1;
                asked.chunks += request.chunks.len();
                socket
                    .send(&request.encode())
                    .await
                    .expect("the shard is listening");
            }
            if fetch.is_complete() {
                return;
            }
            let event = socket
                .next_event()
                .await
                .expect("the socket stayed up")
                .expect("the shard did not hang up mid-fetch");
            if let Event::Packet(packet) = event {
                fetch.on_packet(&packet).expect("the shard's own ground");
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), fetching)
        .await
        .expect("the chunks arrived inside the timeout");
    asked
}

/// What a fetch cost on the wire: how many requests it took, and how many chunks
/// they named between them.
///
/// Two numbers because two tests ask different questions of one loop — E2 wants
/// to know that a facet too big for one request was paced, and E3 that a client
/// with a cache asked for a *count* nobody had to pace at all.
struct Asked {
    requests: usize,
    chunks: usize,
}

/// Every tile of one world answers what the other's does.
///
/// Tile by tile and not sampled, because the failure this is against is a
/// *transposed* chunk — one that lands somewhere plausible and is wrong
/// everywhere.
fn assert_same_world(here: &WorldMap, there: &WorldMap) {
    assert_eq!((here.width(), here.height()), (there.width(), there.height()));
    assert_eq!(here.static_count(), there.static_count());
    for y in 0..u16::try_from(there.height()).unwrap() {
        for x in 0..u16::try_from(there.width()).unwrap() {
            assert_eq!(here.land(x, y), there.land(x, y), "the land at ({x}, {y})");
            let ours: Vec<StaticItem> = here.statics_at(x, y).copied().collect();
            let theirs: Vec<StaticItem> = there.statics_at(x, y).copied().collect();
            assert_eq!(ours, theirs, "the statics at ({x}, {y})");
        }
    }
}

/// Say one thing and collect what the shard says back, until it goes quiet.
///
/// `map_edit`'s helper, and the quiet is a timeout rather than a marker for its
/// reason: a command's reply is several lines and nothing on the wire says which
/// is the last one.
async fn say_and_hear(
    socket: &mut Socket<TcpStream>,
    view: &mut openshard_client_net::view::WorldView,
    words: &str,
) -> Vec<String> {
    let before = view.journal.len();
    socket
        .send(&talk::say(words, TalkMode::Regular))
        .await
        .expect("the shard is listening");
    let _ = tokio::time::timeout(Duration::from_millis(1500), async {
        while let Some(event) = socket.next_event().await.expect("the socket stayed up") {
            if let Event::Packet(packet) = event {
                view.apply(&packet);
            }
        }
    })
    .await;
    view.journal
        .iter()
        .skip(before)
        .map(|line| line.text.clone())
        .collect()
}

#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_client_asks_for_the_ground_and_gets_the_shards_own_bytes() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("chunks");
    let base_set = world_of_ours(&dir, &client, BLOCKS);

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
    let wanted = every_chunk(BLOCKS);
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
    for item in statics(BLOCKS) {
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

/// E3's "done", both clauses of it: a second connection over an unchanged world
/// asks for **no chunks at all**, and one over a world that moved by a single
/// `.setland` asks for **exactly the chunk that patch touched**.
///
/// Three connections to one shard, and each is a client starting up: it is told
/// what world it is standing in, it looks in the directory where it keeps
/// worlds, and it decides. The decision itself is `link.rs`'s `decide` — private
/// to `openshard-client-app`, which cannot see a shard — so it is written out
/// here in the same three branches, the way the fetch loop above is.
///
/// What only this can catch is the join: that the identity the shard puts in its
/// notice is stable across two connections, that a world written by
/// `openshard_client_net::cache` reads back as the world that was fetched, and
/// that the shard's answer about what moved names the chunk an operator's edit
/// actually landed in.
#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_client_that_kept_the_ground_asks_only_for_what_moved() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("kept-ground");
    // Where this client keeps worlds. A directory of the test's own rather than
    // the working directory the real client uses: two of these in one binary
    // would otherwise be one cache.
    let kept = dir.join("kept");
    std::fs::create_dir_all(&kept).expect("a writable temp directory");
    let base_set = world_of_ours(&dir, &client, BLOCKS);

    let (address, shard) = spawn(config_over(base_set.clone(), client));

    // The first connection: nothing kept, so the whole facet.
    let (mut socket, view) =
        tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");
    let notice = view.world.expect("the shard told us what world we are in");
    let world = notice
        .world
        .expect("a shard running on a base set of ours names its world");
    assert!(
        matches!(
            openshard_client_net::cache::read(&kept, notice),
            Err(openshard_client_net::cache::CacheError::Missing { .. })
        ),
        "nothing has been kept yet"
    );
    let mut fetch = Fetch::of(notice).expect("a facet the wire can name");
    let asked = drive(&mut socket, &mut fetch).await;
    assert_eq!(asked.chunks, 16, "the whole facet, which is sixteen chunks");
    let arrived = fetch.finish().expect("a complete set of chunks");
    let path = openshard_client_net::cache::write(&kept, notice, &arrived).expect("a writable temp dir");
    assert_eq!(path, openshard_client_net::cache::path_of(&kept, world, FACET));
    drop(socket);

    // The second: the world is kept and the shard has not moved, so the decision
    // ends before a single chunk is asked for.
    let (socket, view) =
        tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");
    let notice = view.world.expect("the shard told us what world we are in");
    assert_eq!(notice.world, Some(world), "the same world, named the same way");
    let held = openshard_client_net::cache::read(&kept, notice).expect("the world kept a moment ago");
    assert_eq!(
        held.revision().get(),
        notice.revision.0,
        "nothing moved, so there is nothing to ask for"
    );
    let on_disk = openshard_basemap::read(&base_set).expect("the base set reads back");
    assert_same_world(held.map(), on_disk.map());
    drop(socket);

    // An operator moves one tile of ground, which is one chunk of the facet.
    let (mut socket, mut view) =
        tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");
    let before = view.world.expect("the shard told us what world we are in");
    let said = say_and_hear(&mut socket, &mut view, ".setland 3 40").await;
    assert!(
        said.iter()
            .any(|line| line.contains("Committed") && line.contains("revision 2")),
        "the patch was committed and the facet moved: {said:?}"
    );
    drop(socket);

    // The third connection: the kept world is a revision behind, so the shard is
    // asked what moved and answers with the one chunk the edit landed in.
    let (mut socket, view) =
        tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");
    let notice = view.world.expect("the shard told us what world we are in");
    assert_eq!(notice.world, Some(world), "one edit is not another world");
    assert_eq!(notice.revision.0, before.revision.0 + 1, "and it moved by one");
    let held = openshard_client_net::cache::read(&kept, notice).expect("the world kept above");
    assert!(
        held.revision().get() < notice.revision.0,
        "the kept world is behind"
    );

    socket
        .send(
            &ChangesRequest {
                facet: FACET,
                revision: WorldRevision(held.revision().get()),
            }
            .encode(),
        )
        .await
        .expect("the shard is listening");
    let heard = hear(&mut socket, |so_far| {
        so_far
            .iter()
            .any(|packet| matches!(packet, ServerPacket::ChangesReply(_)))
    })
    .await;
    let reply = heard
        .iter()
        .find_map(|packet| match packet {
            ServerPacket::ChangesReply(reply) => Some(reply.clone()),
            _ => None,
        })
        .expect("what moved is answered exactly once");
    assert_eq!(reply.facet, FACET);
    assert_eq!(reply.revision.0, notice.revision.0);
    // The tile the operator moved is at START, which is inside chunk (2, 2) of a
    // facet cut into sixty-four-tile squares.
    let touched = ChunkAt {
        x: START.0 / 64,
        y: START.1 / 64,
    };
    assert_eq!(
        reply.changes,
        Changes::These(vec![touched]),
        "one patch, one chunk, and not the facet"
    );

    let mut fetch = Fetch::over(
        notice,
        held,
        match &reply.changes {
            Changes::These(chunks) => chunks.clone(),
            Changes::Everything => panic!("the shard knows what moved"),
        },
        openshard_map::snapshot::MapRevision::decoded(reply.revision.0),
    )
    .expect("the world kept is the world described");
    let asked = drive(&mut socket, &mut fetch).await;
    assert_eq!(asked.chunks, 1, "exactly the chunks that moved");
    let caught_up = fetch.finish().expect("the chunk that moved");
    assert_eq!(caught_up.revision().get(), notice.revision.0);

    // And it is the shard's world: the tile the operator moved is where the
    // operator put it, and every other tile of the facet is what it was.
    let moved = openshard_basemap::load(&base_set).expect("the world reads back");
    assert_eq!(moved.patches, 1, "one commit, one record");
    assert_same_world(caught_up.map(), moved.snapshot.map());
    assert_eq!(
        caught_up.map().land(START.0, START.1),
        Some(LandCell {
            tile: LandTileId(3),
            z: 40
        }),
        "the tile the operator moved arrived over the wire"
    );
    openshard_client_net::cache::write(&kept, notice, &caught_up).expect("a writable temp dir");

    shard.stop();
    std::fs::remove_dir_all(&dir).ok();
}

/// E2's own seam: the client does not compare bytes, it **holds a world**.
///
/// The test above is E1's — it checks each record against the file the shard cut
/// it from, which is a statement about the wire. This one drives
/// [`Fetch`](openshard_client_net::chunks::Fetch), the thing a window actually
/// runs, over the same socket and asks a different question: is the facet it
/// ends up holding the shard's facet, tile for tile.
///
/// Two things only this can catch. The fixture is **81 chunks**, which is more
/// than one `0xE002` may name, so the send-then-read loop has to pace itself and
/// the shard has to answer two requests on one connection. And what comes out is
/// a [`MapSnapshot`] rather than a pile of records: the facet, the revision and
/// the extent are read off it and compared with the base set the shard booted
/// from.
#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_client_with_no_map_files_ends_up_holding_the_shards_world() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("world-from-the-wire");
    let base_set = world_of_ours(&dir, &client, WIDE_BLOCKS);

    let (address, shard) = spawn(config_over(base_set.clone(), client));
    let (mut socket, view) =
        tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");

    let notice = view.world.expect("the shard told us what world we are in");
    let mut fetch = Fetch::of(notice).expect("a facet the wire can name");
    assert_eq!(fetch.wanted(), 81, "nine chunks square");

    // The loop `link.rs` runs — see [`drive`], which is that loop written out
    // rather than called: what is under test is that the two halves of it — ask
    // until the window is full, then read until something completes — make
    // progress against a real shard.
    let asked = drive(&mut socket, &mut fetch).await;
    assert_eq!(asked.chunks, 81);
    assert!(
        asked.requests > 1,
        "81 chunks is more than one request may name, and it was asked for in {}",
        asked.requests
    );

    let arrived = fetch.finish().expect("a complete set of chunks");

    // The oracle: the file the shard booted from, read back through the reader a
    // shard boots with. Not the bytes this time — the world.
    let on_disk = openshard_basemap::read(&base_set).expect("the base set reads back");
    assert_eq!(arrived.facet(), FACET);
    assert_eq!(arrived.revision(), on_disk.revision(), "a world nobody patched");
    // Every tile, because the failure this is against is a *transposed* chunk —
    // one that lands somewhere plausible and is wrong everywhere. Sampling would
    // find it only by luck.
    assert_same_world(arrived.map(), on_disk.map());

    shard.stop();
    std::fs::remove_dir_all(&dir).ok();
}
