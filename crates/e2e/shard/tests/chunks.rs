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
//! # Four tests, and they are four phases
//!
//! The first is E1's and is about the **wire**: sixteen chunks asked for by
//! hand, and every record compared byte for byte against what `Chunk::of` cuts
//! out of the file. E2's is about the **client**: it drives
//! `openshard_client_net::chunks::Fetch` — the thing a window runs — over
//! eighty-one chunks, which is more than one request may name, and asks whether
//! the facet it ends up holding is the shard's facet tile for tile. E3's is
//! about **three connections to one shard**, with a `.setland` between the
//! second and the third, and it asks what each of them costs. E4's is about
//! **one**: the operator edits the ground while standing on it, and the shard
//! says so on the connection that is already open.

use std::time::Duration;

use openshard_client_net::chunks::{
    Fetch,
    Fetched,
};
use openshard_client_net::connection::Event;
use openshard_client_net::talk;
use openshard_client_net::transport::{
    Socket,
    enter_world,
};
use openshard_e2e_shard::{
    plan,
    spawn,
    version,
};
use openshard_map::chunk::{
    Chunk,
    ChunkCoord,
    assemble,
    chunks_of,
};
use openshard_map::codec;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{
    LandCell,
    StaticItem,
    WorldMap,
};
use openshard_protocol::chunks::{
    Changes,
    ChangesRequest,
    ChunkAt,
    ChunkData,
    ChunkRequest,
    Refusal,
    WorldRevision,
    join,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::TalkMode;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_tiles::LandTileId;
use tokio::net::TcpStream;

mod common;

use common::{
    FACET,
    START,
    config_over,
    install,
    say_and_hear,
    scratch,
    world_of_ours,
};

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
        .map(|(n, (x, y))| {
            StaticItem {
                tile: Graphic(0x4000 + u16::try_from(n).unwrap()),
                x,
                y,
                z: i8::try_from(n).unwrap(),
                hue: Hue(0),
            }
        })
        .collect();
    for n in 0..2u16 {
        items.push(StaticItem {
            tile: Graphic(0x4100 + n),
            x:    20,
            y:    21,
            z:    5,
            hue:  Hue(n),
        });
    }
    items
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
                heard.push(*packet);
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
    .map(|at| {
        ChunkAt {
            x: u16::try_from(at.x).unwrap(),
            y: u16::try_from(at.y).unwrap(),
        }
    })
    .collect()
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
        chunks:   0,
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
    chunks:   usize,
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

#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_client_asks_for_the_ground_and_gets_the_shards_own_bytes() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("chunks");
    let base_set = world_of_ours(&dir, &client, BLOCKS, &statics(BLOCKS));

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
                facet:  FACET,
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
            .filter_map(|packet| {
                match packet {
                    ServerPacket::ChunkData(data) if data.at == *at => Some(data.clone()),
                    _ => None,
                }
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
                facet:  FACET,
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
        .find_map(|packet| {
            match packet {
                ServerPacket::ChunkRefused(refused) => Some(*refused),
                _ => None,
            }
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
    let base_set = world_of_ours(&dir, &client, BLOCKS, &statics(BLOCKS));

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
    let arrived = fetch.finish().expect("a complete set of chunks").world();
    let written = openshard_client_net::cache::write(&kept, notice, &arrived).expect("a writable temp dir");
    assert_eq!(
        written.path,
        openshard_client_net::cache::path_of(&kept, world, FACET)
    );
    assert!(
        written.swept.is_empty(),
        "the first world this client kept let go of nothing"
    );
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
                facet:    FACET,
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
        .find_map(|packet| {
            match packet {
                ServerPacket::ChangesReply(reply) => Some(reply.clone()),
                _ => None,
            }
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
        held.into_snapshot(),
        match &reply.changes {
            Changes::These(chunks) => chunks.clone(),
            Changes::Everything => panic!("the shard knows what moved"),
        },
        openshard_map::snapshot::MapRevision::decoded(reply.revision.0),
    )
    .expect("the world kept is the world described");
    let asked = drive(&mut socket, &mut fetch).await;
    assert_eq!(asked.chunks, 1, "exactly the chunks that moved");
    let caught_up = fetch.finish().expect("the chunk that moved").world();
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
            z:    40,
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
    let base_set = world_of_ours(&dir, &client, WIDE_BLOCKS, &statics(WIDE_BLOCKS));

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

    let arrived = fetch.finish().expect("a complete set of chunks").world();

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

/// E4's "done": the ground moves under a client that is already holding it, and
/// the client is told **on the connection it is already on**.
///
/// One connection, and that is the whole difference from the test above: there
/// the world moved between two logins and the client asked what had happened on
/// the way in; here it is standing on the facet when an operator types
/// `.setland`, and the shard is what starts the conversation.
///
/// Two things only this can catch. That a commit reaches the wire at all —
/// `mapedit::commit` sends the notice out of the tick that ran the command, and
/// nothing below the socket can tell whether it was queued for this connection.
/// And that what the notice names is fetchable *as chunks*: the client's world
/// belongs to its window by then, so `Fetch::moved` ends in the squares
/// themselves and `chunk::apply` — which is what the window runs — is what puts
/// them in.
#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_publish_reaches_a_client_that_is_already_standing_on_the_ground() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("published");
    let base_set = world_of_ours(&dir, &client, BLOCKS, &statics(BLOCKS));

    let (address, shard) = spawn(config_over(base_set.clone(), client));
    let (mut socket, mut view) =
        tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
            .await
            .expect("the login conversation finished inside the timeout")
            .expect("the client reached the world");
    let notice = view.world.expect("the shard told us what world we are in");

    // The ground, the way E2 takes it: the facet whole, before anything moves.
    let mut fetch = Fetch::of(notice).expect("a facet the wire can name");
    drive(&mut socket, &mut fetch).await;
    let held = fetch.finish().expect("a complete set of chunks").world();
    assert_eq!(held.revision().get(), notice.revision.0);

    // And now the operator — who is this same character — moves one tile of it.
    // Said rather than committed, so what is under test includes the command
    // path: `.setland` is a `0xAD` that becomes a patch inside one tick.
    socket
        .send(&talk::say(".setland 3 40", TalkMode::Regular))
        .await
        .expect("the shard is listening");
    let heard = hear(&mut socket, |so_far| {
        so_far
            .iter()
            .any(|packet| matches!(packet, ServerPacket::PublishNotice(_)))
    })
    .await;
    // The journal is read for the same reason `map_edit` reads it: a commit that
    // was refused says so in words, and a test that only looked for the notice
    // would report "no packet" for it.
    for packet in &heard {
        view.apply(packet);
    }
    let published = heard
        .iter()
        .find_map(|packet| {
            match packet {
                ServerPacket::PublishNotice(published) => Some(published.clone()),
                _ => None,
            }
        })
        .unwrap_or_else(|| {
            let said: Vec<String> = view.journal.iter().map(|line| line.text.clone()).collect();
            panic!("no publish notice; the shard said {said:?}")
        });
    assert_eq!(published.facet, FACET);
    assert_eq!(
        published.revision.0,
        notice.revision.0 + 1,
        "one patch, one revision"
    );
    // START is inside chunk (2, 2) of a facet cut into sixty-four-tile squares.
    let touched = ChunkAt {
        x: START.0 / 64,
        y: START.1 / 64,
    };
    assert_eq!(
        published.changes,
        Changes::These(vec![touched]),
        "the chunk the edit landed in, and not the facet"
    );

    // What a connected client does with it: fetch those chunks, and put them
    // into the world it already has. `Fetched::Chunks` rather than a world is the
    // point — the facet is the window's by now, and this is what crosses the
    // seam to it.
    let Changes::These(moved) = published.changes.clone() else {
        panic!("the shard knows what moved");
    };
    let mut fetch = Fetch::moved(
        notice,
        moved,
        openshard_map::snapshot::MapRevision::decoded(published.revision.0),
    )
    .expect("a facet the wire can name");
    let asked = drive(&mut socket, &mut fetch).await;
    assert_eq!(asked.chunks, 1, "exactly the chunk that moved");
    let Fetched::Chunks(chunks) = fetch.finish().expect("the chunk that moved") else {
        panic!("a fetch of what moved ends in the chunks themselves");
    };

    // The window's half, which is `Ground::take_chunks` and is `chunk::apply`
    // underneath. Done here through the world type, because that is what holds
    // the rule this is about: the ground moves and the revision moves with it.
    let mut world = openshard_map::world::World::new(Some(held));
    let now = world.take_chunks(&chunks).expect("a chunk of this facet");
    assert_eq!(now.get(), published.revision.0);

    // And it is the shard's world: the file on disk, which is where the log put
    // the same patch.
    let after = openshard_basemap::load(&base_set).expect("the world reads back");
    assert_eq!(after.patches, 1, "one commit, one record");
    let caught_up = world.snapshot().expect("it was given ground");
    assert_same_world(caught_up.map(), after.snapshot.map());
    assert_eq!(
        caught_up.map().land(START.0, START.1),
        Some(LandCell {
            tile: LandTileId(3),
            z:    40,
        }),
        "the tile the operator moved arrived over the same connection"
    );

    shard.stop();
    std::fs::remove_dir_all(&dir).ok();
}
