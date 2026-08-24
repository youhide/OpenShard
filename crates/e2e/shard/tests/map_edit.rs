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

use std::time::Duration;

use openshard_client_net::transport::enter_world;
use openshard_map::map::LandCell;
use openshard_tiles::LandTileId;

use openshard_e2e_shard::{plan, spawn, version};

mod common;

use common::{START, config_over, install, say_and_hear, scratch, world_of_ours};

/// 32 blocks square — 256×256 tiles, which bakes in well under a second and is
/// still big enough for a graph with regions in it.
const BLOCKS: u32 = 32;

#[tokio::test]
#[ignore = "reads the install's tiledata and bakes a graph; run it deliberately"]
async fn a_game_master_changes_the_ground_and_the_shard_is_a_different_world() {
    let Some(client) = install() else {
        eprintln!("OPENSHARD_CLIENT is not set, so there is no tile table to read: skipping");
        return;
    };
    let dir = scratch("mapedit");
    let base_set = world_of_ours(&dir, &client, BLOCKS, &[]);

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
