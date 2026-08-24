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
//! # And then everybody standing on it is told
//!
//! [`PublishNotice`] — `to_the_client.md`'s E4, and the last piece of direction
//! C's own "done". It goes out **after** the log has the patch, for the reason
//! the order above exists: a commit whose log refuses puts the world back, and a
//! client told about a revision that was rolled back holds a world that never
//! existed and asks for chunks of it.
//!
//! What a client does with it is a client's business — see the packet's own doc
//! for why it is sent to everyone on the facet rather than to a list of
//! subscribers, and `crates/client/app/src/link.rs` for the one client that acts
//! on it.

use openshard_basemap::patches;
use openshard_gateway::ConnectionId;
use openshard_map::patch::{Patch, PatchError};
use openshard_map::snapshot::MapRevision;
use openshard_protocol::chunks::{Changes, ChunkAt, MAX_MOVED, PublishNotice, WorldRevision};
use openshard_protocol::server_packet::ServerPacket;
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
        Ok(()) => {
            let revision = state
                .facet_state(facet)
                .ground()
                .snapshot()
                .expect("a facet that just published a patch has ground")
                .revision();
            announce(state, facet, revision, patch);
            Ok(revision)
        }
        Err(source) => {
            state.undo_publish(facet, undo);
            Err(CommitError::NotLogged(source))
        }
    }
}

/// Tell everyone standing on `facet` that its ground has moved.
///
/// One packet per connection on that facet, naming the revision it moved to and
/// the chunks the patch touched. Called from [`commit`] alone, and after the log
/// has the patch — see the module header, which is where that order is argued.
///
/// **The audience is the facet and not a list of subscribers**, which is
/// [`PublishNotice`]'s own decision and is argued there. Here it costs one walk
/// of the players, which is what every other "everyone who can see this" answer
/// on this shard costs.
fn announce(state: &mut WorldState, facet: Facet, revision: MapRevision, patch: &Patch) {
    let touched = patch.touched_chunks();
    // An empty patch moves the revision without moving a tile. There is nothing
    // for a client to fetch and nothing for it to redraw, so there is nothing to
    // say — and saying `These([])` would be a packet whose only effect is to be
    // ignored.
    if touched.is_empty() {
        return;
    }
    // Past the cap the list stops fitting in a packet — see `MAX_MOVED` — and
    // the honest answer is the one a client can act on: take the facet again.
    let changes = if touched.len() > usize::from(MAX_MOVED) {
        Changes::Everything
    } else {
        Changes::These(
            touched
                .iter()
                .map(|at| ChunkAt {
                    x: u16::try_from(at.x).expect("a facet of fewer than 65,536 chunks across"),
                    y: u16::try_from(at.y).expect("a facet of fewer than 65,536 chunks down"),
                })
                .collect(),
        )
    };
    let notice = ServerPacket::PublishNotice(PublishNotice {
        facet,
        revision: WorldRevision(revision.get()),
        changes,
    });
    // Collected before anything is sent: naming the audience reads the world and
    // sending writes to it.
    let audience: Vec<ConnectionId> = state
        .players
        .iter()
        .filter(|(_, &entity)| state.facet_of(entity) == facet)
        .map(|(&connection, _)| connection)
        .collect();
    for connection in audience {
        state.send_packet(connection, &notice);
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
