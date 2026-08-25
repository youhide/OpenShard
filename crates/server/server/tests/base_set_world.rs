//! The shard boots on a world it owns.
//!
//! Direction B's last step, asserted at the place it happens: [`load_world`]
//! reads a facet out of a base set, and the navigation artifact beside that
//! base set validates against it. Nothing here is about the *contents* of the
//! world — `openshard-movement`'s `base_set_terrain` test is what pins that the
//! ground answers the same. This one is about the boot path: which reader runs,
//! which stamp is checked, and where the artifact is looked for.
//!
//! # It needs two files nobody ships, so it skips
//!
//! A navigation graph over Felucca takes the best part of a minute to build,
//! which is more than a test may spend and far more than a machine without a
//! client install can do at all. So this runs only when `OPENSHARD_BASE_SET`
//! names one, and `OPENSHARD_CLIENT` names the install its tile data comes
//! from. Make them with:
//!
//! ```sh
//! cargo run --release -p openshard-uofiles --bin openshard-map-import -- \
//!     --facet 0 --out /tmp/felucca.osbase
//! cargo run --release -p openshard-movement --bin openshard-navigation-bake -- \
//!     --facet 0 --base-set /tmp/felucca.osbase
//! OPENSHARD_BASE_SET=/tmp/felucca.osbase cargo test -p openshard-server \
//!     --test base_set_world
//! ```
//!
//! The artifact lands beside the base set on its own, which is half of what is
//! being asserted: a shard reading a base set must not find the artifact of the
//! install, and an operator must not have to say where it went.

use std::path::{Path, PathBuf};

use openshard_config::{Config, FacetKey};
use openshard_map::map::LandCell;
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;

/// The two paths, or `None` to skip.
fn sources() -> Option<(PathBuf, PathBuf)> {
    let base_set = PathBuf::from(std::env::var_os("OPENSHARD_BASE_SET")?);
    let client = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    (base_set.exists() && client.join("tiledata.mul").exists()).then_some((base_set, client))
}

fn config(client: &Path, base_set: Option<&Path>) -> Config {
    let mut config = Config::default();
    config.world.client_files = client.to_string_lossy().into_owned();
    config.world.facets = vec![0];
    if let Some(base_set) = base_set {
        config
            .world
            .base_sets
            .insert(FacetKey(Facet(0)), base_set.to_owned());
    }
    config
}

/// The shard boots facet 0 out of a base set, and takes the artifact beside it.
///
/// Succeeding is the whole assertion, and it is a compound one: `load_world`
/// read the base set, checked the file's own facet against the config's,
/// stamped the base set rather than the install, found the artifact beside the
/// base set, and matched the stamp — refusing at any of those is an `Err`.
///
/// The last of those is the one worth naming. A UO install that has ever been
/// baked has an `openshard-navigation-0.bin` sitting in it, stamped against
/// `map0LegacyMUL.uop` and `statics0.mul`. If this boot were still looking
/// there, it would find that file and refuse it — the stamp names files this
/// world was not built from. So a green run is also the statement that it
/// looked somewhere else.
#[test]
fn a_shard_loads_facet_zero_out_of_a_base_set() {
    let Some((base_set, client)) = sources() else {
        return;
    };
    openshard_server::boot::load_world(&config(&client, Some(base_set.as_path())))
        .expect("a base set and the navigation artifact beside it");
}

/// A shard that was edited and restarted boots on the artifact it left behind.
///
/// **The one an operator actually meets.** The coarse graph follows a patch on
/// the tick that commits it and nothing writes the file, so every restart after
/// a `.setland` used to be refused outright — *"built from map revision 7,
/// expected 9"* — and the way out was a whole-facet bake measured in half
/// minutes. Now the log carries the graph the two revisions forward and the file
/// is written back, which is what the second half of this asserts: an artifact
/// that is *not* rewritten would leave the next start doing this again, and the
/// one after that.
///
/// Everything happens in a copy of the base set, log and artifact, because the
/// test commits an edit and the operator's world is not a fixture.
#[test]
fn an_artifact_left_behind_by_an_edit_is_caught_up_from_the_log() {
    let Some((base_set, client)) = sources() else {
        return;
    };
    let Some((copy, artifact)) = copied_world(&base_set, &client) else {
        return;
    };

    // An edit nobody was running a shard for: written straight to the log, which
    // is the state a shard leaves behind when it publishes and then stops.
    let world = openshard_basemap::load(&copy).expect("the copy reads");
    let at = (2000, 1500);
    let op = PatchOp::set_land(
        world.snapshot.map(),
        at.0,
        at.1,
        LandCell {
            tile: LandTileId(0x3FF),
            z: 7,
        },
    )
    .expect("a tile of facet 0");
    let patch = Patch::new(
        Facet(0),
        world.snapshot.revision(),
        PatchAuthor("a test".to_owned()),
        PatchTime(0),
        vec![op],
    );
    openshard_basemap::patches::append(
        &openshard_basemap::patches::log_path(&copy),
        Facet(0),
        world.base,
        &patch,
    )
    .expect("the log takes the patch");

    openshard_server::boot::load_world(&config(&client, Some(copy.as_path())))
        .expect("an artifact one revision behind is carried forward, not refused");

    // And it was written back at the revision it was carried to: this is the
    // strict loader, over a stamp taken fresh off the world as it now stands.
    let now = openshard_movement::bake::FacetWorld::read(
        &client,
        openshard_movement::bake::WorldSource::BaseSet(&copy),
        Facet(0),
    )
    .expect("the edited world reads");
    let stamp = now.stamp(&client, Facet(0)).expect("its inputs are all there");
    openshard_movement::bake::load(&artifact, &stamp)
        .expect("the caught-up graph was saved, so the next start pays nothing");

    std::fs::remove_dir_all(copy.parent().expect("the copy is in a directory of its own")).ok();
}

/// The world — base set, log and navigation artifact — copied into a directory
/// of this test's own, or `None` where the copy could not be made.
///
/// **Under the same file names, with the same mtime on the base set.** A stamp
/// records an input's name, its length and when it was last written, so a copy
/// that renamed the base set or took a fresh mtime is a *different world* as far
/// as the artifact beside it is concerned — and the test would be asserting the
/// stamp's refusal instead of the catch-up. A directory of its own is what makes
/// keeping the names possible.
///
/// The log comes too, so that the copy stands at the revision the original does
/// and the artifact is behind it by exactly what it is behind the original by.
fn copied_world(base_set: &Path, client: &Path) -> Option<(PathBuf, PathBuf)> {
    let source = openshard_movement::bake::FacetWorld::read(
        client,
        openshard_movement::bake::WorldSource::BaseSet(base_set),
        Facet(0),
    )
    .ok()?;
    let from = source.navigation_path(client);
    let dir = std::env::temp_dir().join(format!("openshard-catch-up-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let copy = dir.join(openshard_movement::bake::file_name_of(base_set));
    let artifact = openshard_movement::bake::artifact_path(&dir, Some(&copy), Facet(0));
    // `OPENSHARD_NAVIGATION` names one artifact for every world, so under it the
    // copy and the original are the same file and there is nothing to copy.
    if artifact == from {
        return None;
    }
    std::fs::copy(base_set, &copy).ok()?;
    std::fs::File::options()
        .write(true)
        .open(&copy)
        .ok()?
        .set_modified(std::fs::metadata(base_set).ok()?.modified().ok()?)
        .ok()?;
    if let Some(log) = &source.log {
        std::fs::copy(log, openshard_basemap::patches::log_path(&copy)).ok()?;
    }
    std::fs::copy(&from, &artifact).ok()?;
    Some((copy, artifact))
}

/// A base set for a facet the file is not is refused, not loaded sideways.
///
/// The failure it prevents is the quiet one: every coordinate in the wrong
/// facet is a valid coordinate, so the shard would run, and Britain would be
/// somewhere else.
#[test]
fn a_base_set_filed_under_the_wrong_facet_is_refused() {
    let Some((base_set, client)) = sources() else {
        return;
    };
    let mut config = config(&client, None);
    config.world.facets = vec![1];
    config.world.base_sets.insert(FacetKey(Facet(1)), base_set);

    let error = openshard_server::boot::load_world(&config)
        .expect_err("facet 0's world must not load as facet 1")
        .to_string();
    // The wording is `FacetWorld::read`'s, in `openshard_movement::bake`. This
    // test only runs with two files nobody ships, so a reworded message is not
    // caught by CI — it was found by hand on 2026-08-25, a rewording later.
    assert!(
        error.contains("holds facet 0"),
        "the message has to say which facet the file really is: {error}"
    );
}
