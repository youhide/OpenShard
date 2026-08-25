//! An edit to the ground, made while the shard is running.
//!
//! `openshard-basemap`'s own tests cover a patch against a world in a file. This
//! is the half that only exists in a shard: the world in memory moves, the log
//! beside the base set records it, the span bake follows, the coarse router is
//! dropped — and a step a player was allowed a moment ago is refused.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_map::overlay::Doors;
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::{Footing, MapTerrain, NavigationGraph, spans::SpanIndex, step_allowed};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::{Facet, Point};
use openshard_state::facet_rules::FacetRules;
use openshard_state::{FacetState, WorldHome, WorldState};
use openshard_tiles::{LandTileId, TileData};
use openshard_world::mapedit::{self, CommitError};

const FACET: Facet = Facet(0);
const START: (u16, u16) = (8, 8);
/// Four blocks square: big enough that the tiles this edits are not on an edge.
const BLOCKS: u32 = 4;

/// Flat ground, so every step is legal until an edit makes one illegal.
const GROUND: LandCell = LandCell {
    tile: LandTileId(3),
    z: 0,
};

/// A base set of flat ground in the temp dir, and the log path beside it.
///
/// The tag keeps two tests in one binary off each other's files, and the pid
/// keeps two runs off each other's — `openshard-basemap`'s own fixtures do the
/// same, for the same reason.
fn base_set(tag: &str) -> (PathBuf, PathBuf) {
    let base_set =
        std::env::temp_dir().join(format!("openshard-mapedit-{tag}-{}.osbase", std::process::id()));
    let log = openshard_basemap::patches::log_path(&base_set);
    std::fs::remove_file(&base_set).ok();
    std::fs::remove_file(&log).ok();

    let map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| GROUND,
    );
    openshard_basemap::write(
        &base_set,
        &MapSnapshot::new(FACET, map),
        openshard_basemap::Identity::Mint,
    )
    .expect("a writable temp dir");
    (base_set, log)
}

fn clean(base_set: &Path, log: &Path) {
    std::fs::remove_file(base_set).ok();
    std::fs::remove_file(log).ok();
}

/// A shard holding the world in `base_set`.
///
/// `home` says whether the facet is a world of ours — `false` is the facet read
/// out of a client install, which cannot be edited. `router` gives it a coarse
/// graph baked over the world as loaded, which is the state a real shard boots
/// in.
fn shard(base_set: &Path, home: bool, router: bool) -> WorldState {
    let loaded = openshard_basemap::load(base_set).expect("the base set just written");
    let tiles = TileData::empty();
    let (width, height) = (loaded.snapshot.map().width(), loaded.snapshot.map().height());
    // Nothing live over it: a baked graph is a facet's *static* connectivity,
    // which is the same reading `openshard-navigation-bake` takes.
    let nothing_placed = openshard_map::overlay::Overlay::default();
    let coarse = router.then(|| {
        let spans = SpanIndex::build(loaded.snapshot.map(), &tiles);
        let footing = Footing::new(
            Some(MapTerrain::new(loaded.snapshot.map(), &tiles, &spans)),
            &nothing_placed,
            Doors::AsTheyStand,
        );
        NavigationGraph::build(&footing, width, height).expect("a facet this size has a graph")
    });
    let home = home.then(|| WorldHome {
        base_set: base_set.to_owned(),
        base: loaded.base,
        identity: openshard_basemap::identity_of(base_set).expect("the base set just written"),
    });

    let mut facets = BTreeMap::new();
    facets.insert(
        FACET,
        FacetState::new(
            Some(loaded.snapshot),
            coarse,
            width,
            height,
            FacetRules::classic(FACET),
            home,
            &tiles,
        ),
    );
    WorldState::new(
        facets,
        FACET,
        tiles,
        openshard_uofiles::multi::Multis::default(),
        START,
        1,
    )
}

/// The revision the shard is holding for the facet.
fn revision(state: &WorldState) -> openshard_map::snapshot::MapRevision {
    state
        .facet_state(FACET)
        .ground()
        .snapshot()
        .expect("the shard has a map")
        .revision()
}

/// A patch that raises one tile's four corners out of reach of a body standing
/// beside them.
///
/// Four cells and not one: a land tile's height is the average of the corners it
/// shares with its neighbours, so raising a single cell raises no tile at all.
fn a_wall_of_ground(state: &WorldState, at: (u16, u16), z: i8) -> Patch {
    let world = state
        .facet_state(FACET)
        .ground()
        .snapshot()
        .expect("the shard has a map");
    let corner = |x: u16, y: u16| {
        PatchOp::set_land(world.map(), x, y, LandCell { tile: GROUND.tile, z }).expect("a tile on the map")
    };
    Patch::new(
        FACET,
        world.revision(),
        PatchAuthor("a test".into()),
        PatchTime(0),
        vec![
            corner(at.0, at.1),
            corner(at.0 + 1, at.1),
            corner(at.0, at.1 + 1),
            corner(at.0 + 1, at.1 + 1),
        ],
    )
}

/// Whether a body on `from` may step east, which is the whole of what a player
/// notices about an edit until direction E gives them a picture of it.
fn may_step_east(state: &WorldState) -> bool {
    step_allowed(
        &state.footing(FACET, Doors::AsTheyStand),
        Point::new(START.0, START.1, 0),
        Direction::East,
    )
    .is_some()
}

/// The live publish, end to end: the shard's answer changes, the log records
/// why, and a shard started again over the same files agrees.
#[test]
fn a_committed_patch_changes_what_the_shard_allows_and_survives_a_restart() {
    let (base_set, log) = base_set("committed");
    let mut state = shard(&base_set, true, false);
    assert!(
        may_step_east(&state),
        "flat ground, so the step is legal to begin with"
    );

    let patch = a_wall_of_ground(&state, (START.0 + 1, START.1), 60);
    let published =
        mapedit::commit(&mut state, FACET, &patch).expect("the world in hand, and a writable log");

    assert_eq!(published.get(), 2, "the base set was revision 1");
    assert_eq!(revision(&state), published);
    assert!(
        !may_step_east(&state),
        "the ground east is sixty units up now, and the shard says so"
    );

    // The log is the durable half, and a second shard over the same files is how
    // a player would meet it: the world resolves to the same revision, and the
    // same step is refused.
    let again = shard(&base_set, true, false);
    assert_eq!(revision(&again), published);
    assert!(!may_step_east(&again), "and it is the same world");

    clean(&base_set, &log);
}

/// The bake is a projection of the ground, so a published patch has to move it
/// — otherwise the step above would be decided by the heights of a world the
/// shard no longer holds.
#[test]
fn the_span_bake_follows_a_live_patch() {
    let (base_set, log) = base_set("bake");
    let mut state = shard(&base_set, true, false);
    let east = (START.0 + 1, START.1);
    let tiles = TileData::empty();
    let surface = |state: &WorldState| {
        state
            .facet_state(FACET)
            .ground()
            .terrain(&tiles)
            .expect("the shard has a map")
            .surface_at(east.0, east.1, 60)
    };
    assert_eq!(surface(&state), Some(0));

    let patch = a_wall_of_ground(&state, east, 60);
    mapedit::commit(&mut state, FACET, &patch).expect("the world in hand");

    assert_eq!(
        surface(&state),
        Some(60),
        "the span layer answers for the map the patch made"
    );

    clean(&base_set, &log);
}

/// The router is baked over the world as it stood, so an edit retires it. It is
/// dropped rather than quietly kept, because a graph of somewhere else is worse
/// than no graph at all.
#[test]
fn a_live_patch_retires_the_coarse_router() {
    let (base_set, log) = base_set("router");
    let mut state = shard(&base_set, true, true);
    assert!(
        state.facet_state(FACET).coarse_router().is_some(),
        "the shard booted with a graph"
    );

    let patch = a_wall_of_ground(&state, (START.0 + 1, START.1), 60);
    mapedit::commit(&mut state, FACET, &patch).expect("the world in hand");

    assert!(
        state.facet_state(FACET).coarse_router().is_none(),
        "the graph was built over the world before the edit, so it is gone"
    );

    clean(&base_set, &log);
}

/// A facet read out of the client's files has nowhere to write a patch, and the
/// refusal happens before anything moves.
#[test]
fn a_facet_that_is_not_ours_is_refused_before_it_changes() {
    let (base_set, log) = base_set("install");
    let mut state = shard(&base_set, false, false);
    let before = revision(&state);
    let patch = a_wall_of_ground(&state, (START.0 + 1, START.1), 60);

    let refusal = mapedit::commit(&mut state, FACET, &patch).expect_err("no home, no commit");

    assert!(matches!(refusal, CommitError::NotOurWorld { facet } if facet == FACET));
    assert_eq!(revision(&state), before, "nothing moved");
    assert!(
        !log.exists(),
        "and nothing was written beside a world that cannot be edited"
    );

    clean(&base_set, &log);
}

/// The order the whole module exists for: if the log will not take the patch,
/// the world goes back to where it was — revision, ground, bake and router.
///
/// The log is made unusable by putting a file there that is not a log of this
/// world, which is the failure an operator is most likely to actually meet.
#[test]
fn a_world_that_cannot_be_written_down_is_put_back() {
    let (base_set, log) = base_set("unwritable");
    let mut state = shard(&base_set, true, true);
    let before = revision(&state);
    // After the load, not before it: a log that is not a log stops a shard
    // *booting*, and the case here is the one that only shows up at the commit.
    std::fs::write(&log, b"not a patch log at all").expect("a writable temp dir");

    let patch = a_wall_of_ground(&state, (START.0 + 1, START.1), 60);
    let refusal = mapedit::commit(&mut state, FACET, &patch).expect_err("the log is not one");

    assert!(matches!(refusal, CommitError::NotLogged(_)));
    assert_eq!(
        revision(&state),
        before,
        "the revision is where it was, not one further along"
    );
    assert!(may_step_east(&state), "and the ground is back, bake and all");
    assert!(
        state.facet_state(FACET).coarse_router().is_some(),
        "the router was never stale, so it came back too"
    );

    clean(&base_set, &log);
}
