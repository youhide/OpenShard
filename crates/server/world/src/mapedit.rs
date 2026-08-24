//! Changing the ground under a shard that is running.
//!
//! `openshard-map-patch` commits an edit to a world nobody is standing in: it
//! loads the base set, applies one op to a copy, writes the log and exits. This
//! is the same edit arriving while players are on the facet — which is direction
//! C's live publish, and the whole of what it adds is **an order**.
//!
//! # The order, and why it is this one
//!
//! Two things have to happen: the world in memory moves, and the patch is
//! written to the log beside the base set. Neither order is free of a failure
//! window, so the question is which failure is survivable.
//!
//! - **Log first.** Appending is what can realistically fail — a full disk, a
//!   read-only file, a log belonging to another world. But whether a patch
//!   *applies* is a question about a world, and the only honest way to ask it is
//!   to apply it. So a log-first order would either write down a patch nobody
//!   checked — and a patch that does not apply is a shard that will not boot
//!   next time, because [`openshard_basemap::load`] refuses the world rather
//!   than the record — or ask the question twice, in two places, with two
//!   spellings of the same rules.
//! - **World first, and put it back if the log refuses.** The apply *is* the
//!   check, and it hands back the way back — see
//!   [`Undo`](openshard_map::patch::Undo). So the window is closed rather than
//!   traded away: the world moves, the log is written, and if the log will not
//!   have it the world goes back to the revision it was at, with nothing to show
//!   that it ever left.
//!
//! That is the discipline the C handoff called for and left to this phase, and
//! it is why the whole of a commit is one function rather than three calls a
//! caller sequences.
//!
//! # What a commit costs
//!
//! The span bake over the facet, which is 0.07 s on Felucca and is paid inside
//! [`FacetState::publish`](openshard_state::FacetState::publish) — and **the
//! coarse router, which is dropped**. Rebuilding that one is a 52-second offline
//! bake; keeping it would be a router planning through a wall somebody just
//! built. Long routes fall back on the exact search until the shard is rebaked
//! and restarted, and the operator is told so.
//!
//! Both are facet-wide for an edit that touches one chunk, and both are direction
//! D's to make local.
//!
//! # What it does not do
//!
//! **Nothing tells a connected client.** Our own client draws the map it loaded
//! out of the install, and the classic client draws the one on the player's own
//! disk; neither has a packet that says "this tile is different now". So an edit
//! changes what the shard *allows* — where a body may stand, what a step is
//! refused for — while every picture on every screen is the world as it was.
//! That is direction E, and it is the last piece of C's own "done".

use openshard_basemap::patches;
use openshard_map::patch::{Patch, PatchError};
use openshard_map::snapshot::MapRevision;
use openshard_protocol::world::Facet;
use openshard_state::WorldState;

/// Why a live edit could not be committed.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommitError {
    /// The facet is not a world of ours, so there is nowhere to write a patch.
    ///
    /// A facet read out of a UO install has no base set beside it and no log to
    /// append to — `openshard_map::patch`'s header is where that rule lives.
    /// The way out is `world.base_sets`, and it is a conversion rather than a
    /// setting: see `openshard-map-import`.
    NotOurWorld {
        /// Which facet was asked.
        facet: Facet,
    },
    /// The patch does not apply to the world in hand, and nothing has moved.
    Refused(PatchError),
    /// The world moved and the log would not take the patch, so the world was
    /// put back.
    ///
    /// The revision is exactly where it was; what is lost is the edit.
    NotLogged(patches::LogError),
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOurWorld { facet } => write!(
                f,
                "facet {} is read from the client's files, and a world we do not own cannot be \
                 edited: import it with openshard-map-import and name it in world.base_sets",
                facet.0
            ),
            Self::Refused(source) => write!(f, "the patch was refused: {source}"),
            Self::NotLogged(source) => write!(
                f,
                "the world was changed and the change could not be written down, so it was put \
                 back: {source}"
            ),
        }
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotOurWorld { .. } => None,
            Self::Refused(source) => Some(source),
            Self::NotLogged(source) => Some(source),
        }
    }
}

/// Publish one patch into a running shard, and write it down.
///
/// The revision it returns is the world's new one, and it is durable: the log
/// has the patch before this function is done with it.
///
/// # Errors
///
/// [`CommitError`], one variant per way an edit does not become history. In
/// every one of them the world is at the revision it was at before the call.
pub fn commit(state: &mut WorldState, facet: Facet, patch: &Patch) -> Result<MapRevision, CommitError> {
    // Where the log is, before anything moves: a facet that cannot record an
    // edit must not perform one, and finding that out afterwards would mean
    // undoing a publish that never needed to happen.
    let Some(home) = state.facet_state(facet).home() else {
        return Err(CommitError::NotOurWorld { facet });
    };
    let log = patches::log_path(&home.base_set);
    let base = home.base;

    let undo = state.publish(facet, patch).map_err(CommitError::Refused)?;

    match patches::append(&log, facet, base, patch) {
        Ok(()) => Ok(state
            .facet_state(facet)
            .ground()
            .snapshot()
            .expect("a facet that just published a patch has ground")
            .revision()),
        Err(source) => {
            state.undo_publish(facet, undo);
            Err(CommitError::NotLogged(source))
        }
    }
}

/// What an operator has to do before the shard trusts long routes again.
///
/// A committed patch makes every bake over the facet stale, and the navigation
/// graph is the one that stops a shard booting — so the message says which
/// command rebuilds it rather than leaving the next start to explain it. The
/// same sentence `openshard-map-patch` prints, for the same reason.
#[must_use]
pub fn rebake_command(state: &WorldState, facet: Facet) -> Option<String> {
    let home = state.facet_state(facet).home()?;
    Some(format!(
        "cargo run --release -p openshard-movement --bin openshard-navigation-bake -- \
         --facet {} --base-set {:?}",
        facet.0,
        home.base_set.display()
    ))
}
