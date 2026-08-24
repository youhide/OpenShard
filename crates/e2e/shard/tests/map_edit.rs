//! A game master changes the ground, and the shard is a different world after.
//!
//! Every layer of the live publish has a test where it lives — the patch model
//! in `openshard-map`, the bake following the ground in `openshard-movement`,
//! the commit order in `openshard-world`. What none of them can catch is the
//! seam this file is about: that a **sentence typed by a player** reaches
//! `mapedit::commit` at all, over the real login conversation, on a shard that
//! booted from a base set of ours rather than from an install.
//!
//! # Why it is `#[ignore]`
//!
//! It reads the install's `tiledata.mul` and bakes a navigation graph, which is
//! seconds rather than milliseconds, and it wants a UO install to exist. The
//! suite stays fast and this is run deliberately:
//!
//! ```sh
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
//!     cargo test -p openshard-e2e-shard --test map_edit -- --ignored --nocapture
//! ```
//!
//! # The world it edits is its own
//!
//! A small base set in a temp directory, written and baked by the test. Editing
//! the operator's own Felucca would move its revision and leave every bake
//! beside it stale — the test would be a shard's worth of work to undo.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::talk;
use openshard_client_net::transport::enter_world;
use openshard_config::{Config, FacetKey, RawAccessLevel};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_map::overlay::{Doors, Overlay};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::spans::SpanIndex;
use openshard_movement::{Footing, MapTerrain, NavigationGraph, bake};
use openshard_protocol::speech::TalkMode;
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;

use openshard_e2e_shard::{ACCOUNT, plan, spawn, stock_config, version};

const FACET: Facet = Facet(0);
/// 32 blocks square — 256×256 tiles, which bakes in well under a second and is
/// still big enough for a graph with regions in it.
const BLOCKS: u32 = 32;
const START: (u16, u16) = (128, 128);
/// The ground the fixture is made of: grass, flat, at zero.
const GROUND: LandCell = LandCell {
    tile: LandTileId(3),
    z: 0,
};

/// The install, from the environment. `None` skips the test rather than failing
/// it: an ignored test that is run deliberately still deserves to say why it
/// cannot run.
fn install() -> Option<PathBuf> {
    std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from)
}

/// Write a small world of ours into `dir`, bake its graph beside it, and hand
/// back the base set's path.
///
/// This is `openshard-map-import` and `openshard-navigation-bake` in miniature,
/// and deliberately through the same functions: a fixture that wrote the files
/// its own way would be testing a pipeline the shard does not have.
fn world_of_ours(dir: &Path, client: &Path) -> PathBuf {
    let base_set = dir.join("fixture.osbase");
    let map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| GROUND,
    );
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
    // No log yet: the world has never been patched, and the stamp says so. The
    // first commit in this test is what makes that stamp stale — which is the
    // cost the shard prints and this test reads back.
    let stamp = bake::stamp_of_base_set(&base_set, None, &tiledata, FACET, snapshot.revision())
        .expect("the two inputs exist");
    bake::save(
        &bake::artifact_path(bake::beside(&base_set), Some(&base_set), FACET),
        &graph,
        &stamp,
    )
    .expect("a writable temp directory");

    base_set
}

/// The stock config, pointed at a world of ours, with the development account
/// promoted so its `.`-commands are commands rather than speech.
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

/// Say one thing and collect what the shard says back, until it goes quiet.
///
/// The quiet is a timeout rather than a marker, because a command's reply is
/// several lines and nothing on the wire says which is the last one.
async fn say_and_hear(
    socket: &mut openshard_client_net::transport::Socket<tokio::net::TcpStream>,
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
    // `skip` rather than a range: the journal is a `VecDeque` with a ceiling on
    // it, so it is a queue and not a slice.
    view.journal
        .iter()
        .skip(before)
        .map(|line| line.text.clone())
        .collect()
}

#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_game_master_changes_the_ground_and_the_shard_is_a_different_world() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("openshard-mapedit-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a writable temp directory");
    let base_set = world_of_ours(&dir, &client);

    // Held for the length of the test: dropping the handle stops the shard.
    let (address, shard) = spawn(config_over(base_set.clone(), client));
    let entered = tokio::time::timeout(Duration::from_secs(30), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout")
        .expect("the client reached the world");
    let (mut socket, mut view) = entered;

    // What the ground is, before anything touches it. A shard that had booted
    // from the install rather than from the base set would answer this too, so
    // the revision is what makes the line worth reading.
    let before = say_and_hear(&mut socket, &mut view, ".tile").await;
    assert!(
        before.iter().any(|line| line.contains("revision 1")),
        "the fixture is a fresh base set, so it is revision 1: {before:?}"
    );
    assert!(
        before.iter().any(|line| line.contains("land 3 at z 0")),
        "and it is the flat grass the fixture was written as: {before:?}"
    );

    let committed = say_and_hear(&mut socket, &mut view, ".setland 3 40").await;
    assert!(
        committed
            .iter()
            .any(|line| line.contains("Committed") && line.contains("revision 2")),
        "the patch was committed and the facet moved: {committed:?}"
    );
    assert!(
        committed.iter().any(|line| line.contains("navigation-bake")),
        "and the operator was told what is now stale: {committed:?}"
    );

    let after = say_and_hear(&mut socket, &mut view, ".tile").await;
    assert!(
        after.iter().any(|line| line.contains("land 3 at z 40")),
        "the ground under the player is what the patch made it: {after:?}"
    );

    // The durable half: the log is beside the base set, and it holds one patch.
    let log = openshard_basemap::patches::log_path(&base_set);
    assert!(log.exists(), "the commit wrote {}", log.display());
    let loaded = openshard_basemap::load(&base_set).expect("the world reads back");
    assert_eq!(loaded.patches, 1, "one commit, one record");
    assert_eq!(loaded.snapshot.revision().get(), 2);
    assert_eq!(
        loaded.snapshot.map().land(START.0, START.1),
        Some(LandCell {
            tile: LandTileId(3),
            z: 40
        }),
        "and replaying the log puts the ground where the game master put it"
    );

    shard.stop();
    std::fs::remove_dir_all(&dir).ok();
}
