//! A map editor's batch crosses the real wire and remains the shard's world.
//!
//! Every layer of the conversation has a test where it lives: request and
//! reply framing in `openshard-protocol`, validation and commit order in
//! `openshard-world`, and chunk assembly in `openshard-client-net`. What none
//! of those catches is the seam this file owns: a `MapEditRequest` written to a
//! real socket reaches the authenticated tick, its typed reply and publish
//! notice survive server compression, and a subsequent chunk fetch observes
//! both kinds of edit in the same batch.
//!
//! The last leg stops the shard, rebuilds the navigation artifact the commit
//! deliberately invalidated, and starts a new shard over the same base set.
//! Its login notice and chunks therefore prove that boot replayed the patch log,
//! not merely that the first process retained the edit in memory.
//!
//! # Why it is `#[ignore]`
//!
//! It reads the install's `tiledata.mul` and bakes a navigation graph twice,
//! which is seconds rather than milliseconds, and it wants a UO install to
//! exist. The suite stays fast and this is run deliberately:
//!
//! ```sh
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
//!     cargo test -p openshard-e2e-shard --test map_edit -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::Duration;

use openshard_client_net::chunks::{
    Fetch,
    Fetched,
};
use openshard_client_net::connection::Event;
use openshard_client_net::transport::{
    Socket,
    enter_world,
};
use openshard_e2e_shard::{
    plan,
    spawn,
    version,
};
use openshard_map::map::{
    LandCell,
    StaticItem,
};
use openshard_map::overlay::Doors;
use openshard_map::snapshot::{
    MapRevision,
    MapSnapshot,
};
use openshard_movement::spans::SpanIndex;
use openshard_movement::{
    Footing,
    MapTerrain,
    NavigationGraph,
};
use openshard_protocol::chunks::{
    Changes,
    ChunkAt,
    WorldRevision,
};
use openshard_protocol::mapedit::{
    EditLandTile,
    EditTile,
    EditX,
    EditY,
    EditZ,
    MapEditOp,
    MapEditOutcome,
    MapEditRefusal,
    MapEditReply,
    MapEditRequest,
};
use openshard_protocol::server_packet::ServerPacket;
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
    scratch,
    world_of_ours,
};

/// 32 blocks square — 256x256 tiles and sixteen wire chunks.
const BLOCKS: u32 = 32;
const WAIT: Duration = Duration::from_secs(30);

/// Read packets until `done` is true, with a timeout that turns a missing wire
/// answer into a useful test failure rather than a hung test process.
async fn hear_until(
    socket: &mut Socket<TcpStream>,
    mut done: impl FnMut(&[ServerPacket]) -> bool,
) -> Vec<ServerPacket> {
    let mut heard = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let event = socket
                .next_event()
                .await
                .expect("the socket stayed up")
                .expect("the shard did not hang up before answering");
            if let Event::Packet(packet) = event {
                heard.push(*packet);
                if done(&heard) {
                    return;
                }
            }
        }
    })
    .await
    .expect("the shard answered inside the deadline");
    heard
}

/// Drive the same chunk-fetch state machine the client link drives.
async fn drive(socket: &mut Socket<TcpStream>, fetch: &mut Fetch) {
    tokio::time::timeout(WAIT, async {
        loop {
            while let Some(request) = fetch.next_request() {
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
                fetch.on_packet(&packet).expect("the shard's own chunk");
            }
        }
    })
    .await
    .expect("the chunks arrived inside the deadline");
}

/// A committed map patch makes the navigation graph stale by design. Rebuild
/// it from the replayed world before exercising a second shard boot.
fn rebake(base_set: &Path, client: &Path) {
    let loaded = openshard_basemap::load(base_set).expect("the committed world replays");
    let tiledata = client.join("tiledata.mul");
    let tiles = openshard_uofiles::tiledata::load(&tiledata)
        .expect("the install has a tile table")
        .tiles;
    let spans = SpanIndex::build(loaded.snapshot.map(), &tiles);
    let nothing_placed = openshard_map::overlay::Overlay::default();
    let footing = Footing::new(
        Some(MapTerrain::new(loaded.snapshot.map(), &tiles, &spans)),
        &nothing_placed,
        Doors::AsTheyStand,
    );
    let graph = NavigationGraph::build(
        &footing,
        loaded.snapshot.map().width(),
        loaded.snapshot.map().height(),
    )
    .expect("the edited facet has a graph");
    let stamp = openshard_movement::bake::stamp_of_base_set(
        base_set,
        loaded.log.as_deref(),
        &tiledata,
        FACET,
        loaded.snapshot.revision(),
    )
    .expect("the edited inputs can be stamped");
    openshard_movement::bake::save(
        &openshard_movement::bake::artifact_path(
            openshard_movement::bake::beside(base_set),
            Some(base_set),
            FACET,
        ),
        &graph,
        &stamp,
    )
    .expect("the rebuilt navigation artifact can be written");
}

fn assert_edited(snapshot: &MapSnapshot, placed: StaticItem) {
    assert_eq!(
        snapshot.map().land(START.0, START.1),
        Some(LandCell {
            tile: LandTileId(3),
            z:    7,
        }),
        "the land operation is visible in fetched ground"
    );
    assert!(
        snapshot
            .map()
            .statics_at(START.0, START.1)
            .any(|item| *item == placed),
        "the static operation from the same batch is visible in fetched ground"
    );
}

#[tokio::test]
#[ignore = "reads the install's tiledata and bakes two navigation graphs; run it deliberately"]
async fn a_gm_batch_is_published_fetchable_conflict_checked_and_replayed_after_restart() {
    // `common` also owns the command-path helper used by sibling tests. Keep it
    // live in this integration-test crate without making this wire proof send a
    // staff speech command.
    let _command_path_helper = common::say_and_hear;

    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("mapedit-wire");
    let base_set = world_of_ours(&dir, &client, BLOCKS, &[]);
    let initial = openshard_basemap::read(&base_set).expect("the fresh base set reads");

    let (address, shard) = spawn(config_over(base_set.clone(), client.clone()));
    let (mut socket, view) = tokio::time::timeout(WAIT, enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the game master reached the world");
    let before = view.world.expect("the shard described its ground at login");
    assert_eq!(before.facet, FACET);
    assert_eq!(before.revision, WorldRevision(1));

    let placed = StaticItem {
        tile: Graphic(0x4001),
        x:    START.0,
        y:    START.1,
        z:    9,
        hue:  Hue(17),
    };
    let request = MapEditRequest {
        facet:  FACET,
        parent: before.revision,
        ops:    vec![
            MapEditOp::SetLand {
                at:   EditTile {
                    x: EditX(START.0),
                    y: EditY(START.1),
                },
                tile: EditLandTile::from_wire(3).expect("a land tile"),
                z:    EditZ(7),
            },
            MapEditOp::AddStatic {
                at:      EditTile {
                    x: EditX(START.0),
                    y: EditY(START.1),
                },
                graphic: placed.tile,
                z:       EditZ(placed.z),
                hue:     placed.hue,
            },
        ],
    };
    socket
        .send(&request.encode())
        .await
        .expect("the shard is listening");

    let heard = hear_until(&mut socket, |packets| {
        packets
            .iter()
            .any(|packet| matches!(packet, ServerPacket::MapEditReply(_)))
            && packets
                .iter()
                .any(|packet| matches!(packet, ServerPacket::PublishNotice(_)))
    })
    .await;
    let replies: Vec<MapEditReply> = heard
        .iter()
        .filter_map(|packet| {
            match packet {
                ServerPacket::MapEditReply(reply) => Some(*reply),
                _ => None,
            }
        })
        .collect();
    assert_eq!(replies.len(), 1, "one request has exactly one typed reply");
    assert_eq!(replies[0].facet, FACET);
    assert_eq!(replies[0].revision, WorldRevision(2));
    assert_eq!(replies[0].outcome, MapEditOutcome::Accepted);

    let publishes: Vec<_> = heard
        .iter()
        .filter_map(|packet| {
            match packet {
                ServerPacket::PublishNotice(notice) => Some(notice),
                _ => None,
            }
        })
        .collect();
    assert_eq!(publishes.len(), 1, "one accepted batch publishes once");
    assert_eq!(publishes[0].facet, FACET);
    assert_eq!(publishes[0].revision, WorldRevision(2));
    let touched = ChunkAt {
        x: START.0 / 64,
        y: START.1 / 64,
    };
    assert_eq!(publishes[0].changes, Changes::These(vec![touched]));

    // Fetch exactly what the publish named and apply it over the pre-edit
    // snapshot. Both operations must arrive through the chunk socket path.
    let mut changed = Fetch::over(
        before,
        initial,
        vec![touched],
        MapRevision::decoded(replies[0].revision.0),
    )
    .expect("the published chunk belongs to this world");
    drive(&mut socket, &mut changed).await;
    let fetched = changed.finish().expect("the changed chunk is complete").world();
    assert_eq!(fetched.revision().get(), 2);
    assert_edited(&fetched, placed);

    // Reusing the old parent is a protocol-level conflict, not a second patch.
    socket
        .send(&request.encode())
        .await
        .expect("the shard is listening");
    let refused = hear_until(&mut socket, |packets| {
        packets
            .iter()
            .any(|packet| matches!(packet, ServerPacket::MapEditReply(_)))
    })
    .await;
    let refusal = refused
        .iter()
        .find_map(|packet| {
            match packet {
                ServerPacket::MapEditReply(reply) => Some(*reply),
                _ => None,
            }
        })
        .expect("the stale draft receives a typed reply");
    assert_eq!(refusal.revision, WorldRevision(2));
    assert_eq!(refusal.outcome, MapEditOutcome::Refused(MapEditRefusal::Conflict));

    drop(socket);
    shard.stop();

    let logged = openshard_basemap::load(&base_set).expect("the stopped shard's log replays");
    assert_eq!(logged.patches, 1, "the refused draft appended nothing");
    assert_eq!(logged.snapshot.revision().get(), 2);
    assert_edited(&logged.snapshot, placed);

    // A graph at revision one is intentionally stale now. Build its derived
    // replacement, then prove a new shard process loads revision two from the
    // log and serves it over fresh login and chunk connections.
    rebake(&base_set, &client);
    let (address, restarted) = spawn(config_over(base_set.clone(), client));
    let (mut socket, view) = tokio::time::timeout(WAIT, enter_world(address, plan(), version()))
        .await
        .expect("the restarted login finished inside the timeout")
        .expect("the game master reached the restarted world");
    let after_restart = view
        .world
        .expect("the restarted shard described its replayed ground");
    assert_eq!(after_restart.revision, WorldRevision(2));

    let mut all = Fetch::of(after_restart).expect("the restarted facet is fetchable");
    drive(&mut socket, &mut all).await;
    let replayed = match all.finish().expect("the restarted facet is complete") {
        Fetched::World(snapshot) => snapshot,
        Fetched::Chunks(_) => panic!("a whole-facet fetch returns a world"),
    };
    assert_edited(&replayed, placed);

    restarted.stop();
    std::fs::remove_dir_all(&dir).ok();
}
