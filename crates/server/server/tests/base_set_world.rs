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
use openshard_protocol::world::Facet;

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
    assert!(
        error.contains("the file is facet 0"),
        "the message has to say which facet the file really is: {error}"
    );
}
