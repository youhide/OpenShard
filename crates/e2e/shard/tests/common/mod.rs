//! What a test that boots a shard on a world of *ours* needs, written once.
//!
//! Two integration tests here — [`chunks`](../chunks.rs) and
//! [`map_edit`](../map_edit.rs) — start the same way: write a small base set into
//! a temp directory, bake a navigation graph beside it, point a stock config at
//! it, and promote the development account so that a `.`-command is a command
//! rather than speech. Both had their own copy of all four, and the copies had
//! begun to drift — `say_and_hear` was written twice and `world_of_ours` two and
//! a half times, since one of them had grown a `blocks` argument the other had
//! not.
//!
//! **A `tests/` module and not the `openshard-e2e-shard` library**, which is the
//! decision worth recording: everything here reads a UO install, bakes a graph
//! or drives a socket with a timeout on it, and none of that belongs in a crate
//! that a non-test caller can link. `tests/common/mod.rs` is compiled into each
//! test binary that asks for it and costs the library nothing.
//!
//! Adding a third test that boots a shard on a world of ours: `mod common;` and
//! nothing else.

use std::net::SocketAddr;
use std::path::{
    Path,
    PathBuf,
};
use std::time::Duration;

use openshard_client_net::connection::Event;
use openshard_client_net::talk;
use openshard_client_net::transport::Socket;
use openshard_config::{
    Config,
    FacetKey,
    RawAccessLevel,
};
use openshard_e2e_shard::{
    ACCOUNT,
    stock_config,
};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{
    LandCell,
    StaticItem,
    WorldMap,
};
use openshard_map::overlay::{
    Doors,
    Overlay,
};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::spans::SpanIndex;
use openshard_movement::{
    Footing,
    MapTerrain,
    NavigationGraph,
    bake,
};
use openshard_protocol::speech::TalkMode;
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;
use tokio::net::TcpStream;

/// The facet every one of these fixtures is.
pub const FACET: Facet = Facet(0);

/// Where a character stands when it enters one.
pub const START: (u16, u16) = (128, 128);

/// The ground the fixtures are made of: grass, flat, at zero.
///
/// Uniform on purpose. A fixture whose heights wandered would be one whose
/// navigation bake could refuse to build for reasons that have nothing to do
/// with the test reading it.
pub const GROUND: LandCell = LandCell {
    tile: LandTileId(3),
    z:    0,
};

/// The install, from the environment.
///
/// `None` skips a test rather than failing it: an `#[ignore]`d test that is run
/// deliberately still deserves to say why it cannot run.
pub fn install() -> Option<PathBuf> {
    std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from)
}

/// A temp directory of one test's own, named for it as well as for the process.
///
/// Two of these run in one binary and in parallel, and a shared directory would
/// be one bake racing another.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openshard-{name}-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a writable temp directory");
    dir
}

/// Write a small world of ours into `dir`, bake its graph beside it, and hand
/// back the base set's path.
///
/// `openshard-map-import` and `openshard-navigation-bake` in miniature, and
/// deliberately through the same functions: a fixture that wrote the files its
/// own way would be testing a pipeline the shard does not have.
///
/// `blocks` is a side of the square, so the facet is `blocks * 8` tiles across;
/// `statics` is what to place on it, and an empty slice is a world of nothing but
/// ground. Both are parameters because the two callers genuinely differ — one
/// wants a facet large enough that a fetch has to pace itself, and one wants the
/// smallest thing that bakes.
///
/// No patch log is written. The world has never been changed, and the stamp says
/// so — which is what makes the first commit in a test that makes one able to
/// find that stamp stale.
pub fn world_of_ours(dir: &Path, client: &Path, blocks: u32, statics: &[StaticItem]) -> PathBuf {
    let base_set = dir.join("fixture.osbase");
    let mut map = WorldMap::from_blocks(
        BlockExtent {
            wide: blocks,
            down: blocks,
        },
        |_, _| GROUND,
    );
    for item in statics {
        map.place_static(*item);
    }
    let snapshot = MapSnapshot::new(FACET, map);
    openshard_basemap::write(&base_set, &snapshot, openshard_basemap::Identity::Mint)
        .expect("a writable temp directory");

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
        &bake::artifact_path(bake::beside(&base_set), Some(&base_set), FACET),
        &graph,
        &stamp,
    )
    .expect("a writable temp directory");

    base_set
}

/// The stock config, pointed at a world of ours, with the development account
/// promoted so its `.`-commands are commands rather than speech.
pub fn config_over(base_set: PathBuf, client: PathBuf) -> impl FnOnce(SocketAddr) -> Config + Send {
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
pub async fn say_and_hear(
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
    // `skip` rather than a range: the journal is a `VecDeque` with a ceiling on
    // it, so it is a queue and not a slice.
    view.journal
        .iter()
        .skip(before)
        .map(|line| line.text.clone())
        .collect()
}
