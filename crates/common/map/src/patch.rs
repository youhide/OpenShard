//! One committed change to the world, and what applying it means.
//!
//! `mechanics.md` keeps three words apart, and this module is the middle one:
//!
//! - **Base** — the world as imported, immutable. `openshard_basemap` is the
//!   file it lives in.
//! - **Patch** — this. One unit of change against a *known parent revision*,
//!   durable, ordered, attributable, revertible.
//! - **Snapshot** — [`MapSnapshot`], which is base plus the patches in force at
//!   one revision, and the only thing a reader ever sees.
//!
//! # A patch applies to a parent, and nothing else
//!
//! [`Patch::parent`] is the revision the change was made against, and
//! [`MapSnapshot::publish`] refuses a patch whose parent is not the revision it
//! is holding. That refusal is the whole conflict model: if the world moved
//! under an unpublished edit, the editor is told so and makes a new patch on the
//! new parent. Silent last-write-wins on terrain would let one operator's
//! hillside quietly eat another's.
//!
//! It is also what makes an *ordinal* a stable identity. Two identical rocks can
//! stand on one tile at one height, so "remove that rock" cannot be said with
//! coordinates and a graphic — but "the second static standing on this tile" is
//! exact, once the world it is said about is pinned. The parent revision pins
//! it, and that is why [`StaticId`] needs no bytes in the base format.
//!
//! # An op carries what it replaces
//!
//! [`PatchOp::SetLand`] carries the cell that was there and
//! [`PatchOp::RemoveStatic`] carries the item it takes away. Two things fall out
//! of that, and both are the reason for the extra bytes:
//!
//! - **The inverse is exact and needs no world.** A revert is a new patch, and
//!   its ops are these ops read backwards.
//! - **A log paired with the wrong base is caught.** The parent revision already
//!   says which world a patch was made against — but a *re-imported* facet is
//!   revision 1 again, so a log dropped beside a base set of some other place
//!   would apply, tile by plausible tile. `was` is what turns that into
//!   [`PatchError::LandNotAsRecorded`] on the first op instead.
//!
//! # Ops are a sequence, not a set
//!
//! They apply in order, and each one sees what the ones before it did. So an
//! ordinal in the third op counts the static the second op added. That is the
//! only reading under which an editor can express "remove both of these", and it
//! is why [`apply`] never reorders.
//!
//! # All of a patch, or none of it
//!
//! An op that cannot apply — a tile off the map, an ordinal past the end, a
//! recorded value that is not what is standing there — aborts the patch, and the
//! ops already applied are undone. [`apply`] gets the undo for free: applying an
//! op *returns* its inverse, which is what a revert will be built out of too.
//!
//! # Only a world we own can be patched
//!
//! A facet still read out of a UO install has nowhere to keep a patch log and no
//! guarantee the operator will not replace the files under it. Patches lie over
//! a **base set**; `world.base_sets` is what says a facet has one.

use openshard_protocol::world::Facet;

use crate::chunk::ChunkCoord;
use crate::map::{LandCell, StaticItem, WorldMap};
use crate::snapshot::MapRevision;

/// Which static standing on a tile, counted in the order the world hands them
/// out.
///
/// Only meaningful together with a tile *and* a revision: it is the position in
/// [`WorldMap::statics_at`]'s sequence, which the `(y, x)` sort of
/// [`WorldMap::from_parts`] makes stable for as long as the world does not change.
/// [`Patch::parent`] is what stops it from being read against a different one.
///
/// A `u16` because a tile is not a block: the densest block of the shipped
/// castle holds 339 items, and nothing says how many of them may share a tile.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StaticId(pub u16);

/// Who committed a patch.
///
/// Attribution, not authority: nothing here checks that the name means
/// anything. Whether an author *may* publish is the editor's question, and
/// `plan.md` puts it in direction F on purpose.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatchAuthor(pub String);

/// When a patch was committed, in seconds since the Unix epoch.
///
/// For a person reading a history. **Not** the order: the order is the chain of
/// revisions, and two shards whose clocks disagree still apply their patches in
/// exactly one sequence.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PatchTime(pub u64);

/// The smallest thing an edit can be.
///
/// Deliberately three. Editor brushes — raise, flatten, smooth, stamp — are
/// *editor* commands that compile down to these before publishing, which keeps
/// the diff explainable, the undo exact, and a brush algorithm out of the
/// world's history.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PatchOp {
    /// Replace the ground at one tile.
    SetLand {
        /// Where.
        x: u16,
        /// Where.
        y: u16,
        /// What is there now, checked before anything is written.
        was: LandCell,
        /// What to put there.
        now: LandCell,
    },
    /// Put a static on the map, at the coordinates the item carries.
    ///
    /// It goes in after everything already standing on its tile, which is
    /// [`WorldMap::place_static`]'s rule and is what makes the inverse's ordinal
    /// knowable: the added item is the last of its tile.
    AddStatic {
        /// What to add, coordinates included.
        item: StaticItem,
    },
    /// Take one static off the map.
    RemoveStatic {
        /// Which one on its tile.
        which: StaticId,
        /// What is standing there, checked before it is taken away. Its own
        /// coordinates are the tile.
        was: StaticItem,
    },
}

impl PatchOp {
    /// The tile this op is about.
    #[must_use]
    pub const fn at(&self) -> (u16, u16) {
        match self {
            Self::SetLand { x, y, .. } => (*x, *y),
            Self::AddStatic { item } | Self::RemoveStatic { was: item, .. } => (item.x, item.y),
        }
    }

    /// Replace the ground at one tile, reading what is there now out of `map`.
    ///
    /// The `was` half of an op is not the caller's to type: an op that recorded
    /// a cell nobody read is an op that describes a place that does not exist,
    /// and the whole point of the field is that such a patch is refused. So
    /// every op is built against a world, and these three are the only
    /// constructors that exist — the enum's fields are public for the *reader*,
    /// and a writer comes through here.
    ///
    /// # Errors
    ///
    /// [`PatchError::OffMap`] — the facet has no such tile.
    pub fn set_land(map: &WorldMap, x: u16, y: u16, now: LandCell) -> Result<Self, PatchError> {
        let was = map.land(x, y).ok_or(PatchError::OffMap { x, y })?;
        Ok(Self::SetLand { x, y, was, now })
    }

    /// Put a static on the map, at the coordinates the item carries.
    ///
    /// Nothing is read here — an addition replaces nothing — but the tile is
    /// still checked, because [`apply`] would refuse it later and a refusal at
    /// the point of building is one a caller can say something useful about.
    ///
    /// # Errors
    ///
    /// [`PatchError::OffMap`] — the facet has no such tile.
    pub const fn add_static(map: &WorldMap, item: StaticItem) -> Result<Self, PatchError> {
        if !map.contains(item.x, item.y) {
            return Err(PatchError::OffMap { x: item.x, y: item.y });
        }
        Ok(Self::AddStatic { item })
    }

    /// Take the `which`th static off a tile, reading what is standing there.
    ///
    /// # Errors
    ///
    /// [`PatchError::OffMap`] — the facet has no such tile.
    /// [`PatchError::NoSuchStatic`] — fewer things stand there than that.
    pub fn remove_static(map: &WorldMap, x: u16, y: u16, which: StaticId) -> Result<Self, PatchError> {
        if !map.contains(x, y) {
            return Err(PatchError::OffMap { x, y });
        }
        let standing = map.statics_at(x, y).count();
        let was = *map
            .statics_at(x, y)
            .nth(which.0 as usize)
            .ok_or(PatchError::NoSuchStatic {
                x,
                y,
                which,
                standing,
            })?;
        Ok(Self::RemoveStatic { which, was })
    }
}

/// What puts the world back where it was, if a publish has to be taken back.
///
/// **It is not a patch, and that distinction is the model.** A revert an
/// operator *asks* for is a new patch with a new revision, committed to the log
/// like any other — the history is append-only and a mistake is part of it. This
/// is the other case: a publish that never became history at all, because
/// writing it down failed after the world had already moved. Taking that back
/// leaves no trace, because there is nothing to leave a trace of.
///
/// It carries the revision as well as the ops, so a world put back is at the
/// number it was at rather than one further along. Two ways to spell an undo
/// would be two ways for the revision to drift from the map.
///
/// Ops come out of [`apply`] in the order they must be replayed, which is the
/// reverse of the order they were made in — each inverse was computed against
/// the world as it stood *after* the op before it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Undo {
    /// The inverses, in the order they apply.
    ops: Vec<PatchOp>,
    /// The revision the world was at before the publish.
    to: MapRevision,
}

impl Undo {
    /// Record the way back from a publish that produced `ops` over `to`.
    pub(crate) const fn new(ops: Vec<PatchOp>, to: MapRevision) -> Self {
        Self { ops, to }
    }

    /// The inverses, in the order they apply.
    pub(crate) fn ops(&self) -> &[PatchOp] {
        &self.ops
    }

    /// The revision the world goes back to.
    pub(crate) const fn to(&self) -> MapRevision {
        self.to
    }

    /// Which chunks putting the world back moves, each one once.
    ///
    /// [`Patch::touched_chunks`] for the way back, and it exists for the same
    /// caller: a bake over the world an undo just replaced is as stale as one
    /// over the world a publish replaced. The inverses touch exactly the tiles
    /// the ops did, so the answer is the same list — but it is asked of the undo
    /// rather than remembered from the patch, because an undo is a thing that
    /// can be held on its own.
    #[must_use]
    pub fn touched_chunks(&self) -> Vec<ChunkCoord> {
        touched_chunks(&self.ops)
    }
}

/// One committed unit of change.
///
/// Built by an editor, written to a patch log, and read back in revision order
/// at load. The fields are private because three of them are a claim about a
/// world — the facet, the parent and the ops — and a patch whose parent was
/// edited after it was made is a patch that would apply to something it was
/// never checked against.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Patch {
    facet: Facet,
    parent: MapRevision,
    author: PatchAuthor,
    at: PatchTime,
    ops: Vec<PatchOp>,
}

impl Patch {
    /// Record a change made against `parent`.
    ///
    /// Nothing is checked here: whether the ops can apply is a question about a
    /// world, and this type does not hold one. [`MapSnapshot::publish`] is where
    /// it is asked, and it is asked again every time the patch is replayed.
    ///
    /// An empty `ops` is legal and means a revision that changed nothing. It is
    /// not refused because there is nothing wrong with it — but it does
    /// invalidate every bake over the facet, so an editor should not publish
    /// one.
    #[must_use]
    pub const fn new(
        facet: Facet,
        parent: MapRevision,
        author: PatchAuthor,
        at: PatchTime,
        ops: Vec<PatchOp>,
    ) -> Self {
        Self {
            facet,
            parent,
            author,
            at,
            ops,
        }
    }

    /// Which facet it changes.
    #[must_use]
    pub const fn facet(&self) -> Facet {
        self.facet
    }

    /// The revision it was made against.
    #[must_use]
    pub const fn parent(&self) -> MapRevision {
        self.parent
    }

    /// The revision applying it produces.
    ///
    /// Derived rather than stored: a patch that recorded both could disagree
    /// with itself, and a chain of parents is what a history *is*.
    #[must_use]
    pub const fn revision(&self) -> MapRevision {
        self.parent.after()
    }

    /// Who committed it.
    #[must_use]
    pub const fn author(&self) -> &PatchAuthor {
        &self.author
    }

    /// When they did.
    #[must_use]
    pub const fn at(&self) -> PatchTime {
        self.at
    }

    /// What it does, in order.
    #[must_use]
    pub fn ops(&self) -> &[PatchOp] {
        &self.ops
    }

    /// Which chunks it changes, each one once, in the order chunks are stored.
    ///
    /// Derived from the ops rather than carried beside them, for the reason
    /// [`Patch::revision`] is: a stored list could disagree with the ops it
    /// claims to describe, and then an invalidation would miss a chunk that
    /// really did change. Direction D is the caller — every bake over a touched
    /// chunk is stale the moment this patch is published.
    #[must_use]
    pub fn touched_chunks(&self) -> Vec<ChunkCoord> {
        touched_chunks(&self.ops)
    }
}

/// Which chunks a run of ops changes, each one once, in the order chunks are
/// stored.
///
/// Shared by [`Patch::touched_chunks`] and [`Undo::touched_chunks`] because the
/// question is the same one: an op names a tile and a tile is in one chunk. Two
/// spellings of it would be two answers waiting to disagree, and what reads them
/// is a bake deciding what it still holds.
fn touched_chunks(ops: &[PatchOp]) -> Vec<ChunkCoord> {
    let mut touched: Vec<ChunkCoord> = ops
        .iter()
        .map(|op| {
            let (x, y) = op.at();
            ChunkCoord::containing(x, y)
        })
        .collect();
    touched.sort_unstable();
    touched.dedup();
    touched
}

/// Why a patch could not be applied.
///
/// Every variant is a disagreement between a patch and the world it was handed,
/// and none of them is recoverable by trying harder: the answer is always a new
/// patch made against the world as it now stands.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum PatchError {
    /// The patch changes a different facet than the snapshot holds.
    WrongFacet {
        /// The facet the snapshot is.
        wanted: Facet,
        /// The facet the patch names.
        found: Facet,
    },
    /// The world moved under the edit.
    ///
    /// The conflict, and the only one: an editor that meets this rebases onto
    /// the revision now in force and publishes a new patch.
    Conflict {
        /// The revision the snapshot is at.
        holding: MapRevision,
        /// The revision the patch was made against.
        parent: MapRevision,
    },
    /// An op names a tile the facet does not have.
    OffMap {
        /// Where.
        x: u16,
        /// Where.
        y: u16,
    },
    /// An op names a static past the end of what stands on its tile.
    NoSuchStatic {
        /// Where.
        x: u16,
        /// Where.
        y: u16,
        /// Which one it asked for.
        which: StaticId,
        /// How many are actually standing there.
        standing: usize,
    },
    /// The ground an op recorded is not the ground that is there.
    LandNotAsRecorded {
        /// Where.
        x: u16,
        /// Where.
        y: u16,
        /// What the op says was there.
        recorded: LandCell,
        /// What is there.
        found: LandCell,
    },
    /// The facet has no map at all, so there is nothing to change.
    ///
    /// A shard with no client files and no base set is a real configuration —
    /// no floor, no walls, every step allowed — and it is the one world a patch
    /// cannot be about. Unreachable from [`MapSnapshot::publish`], which is
    /// handed the map by holding it; it is [`crate::world::World::publish`]'s,
    /// one level up, where the ground is optional.
    NoGround,
    /// The static an op recorded is not the static that is there.
    StaticNotAsRecorded {
        /// Which one on its tile.
        which: StaticId,
        /// What the op says was standing there.
        recorded: StaticItem,
        /// What is standing there.
        found: StaticItem,
    },
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongFacet { wanted, found } => write!(
                f,
                "a patch to facet {} was published to facet {}",
                found.0, wanted.0
            ),
            Self::Conflict { holding, parent } => write!(
                f,
                "the patch was made against revision {} and the world is at revision {}",
                parent.get(),
                holding.get()
            ),
            Self::OffMap { x, y } => write!(f, "tile ({x}, {y}) is not on this facet"),
            Self::NoGround => write!(f, "this facet has no map, so there is nothing to patch"),
            Self::NoSuchStatic {
                x,
                y,
                which,
                standing,
            } => write!(
                f,
                "tile ({x}, {y}) has {standing} statics on it, and the patch names number {}",
                which.0
            ),
            Self::LandNotAsRecorded {
                x,
                y,
                recorded,
                found,
            } => write!(
                f,
                "tile ({x}, {y}) holds land {} at z {}, and the patch was made against land {} at z {}",
                found.tile.0, found.z, recorded.tile.0, recorded.z
            ),
            Self::StaticNotAsRecorded {
                which,
                recorded,
                found,
            } => write!(
                f,
                "static {} on tile ({}, {}) is graphic {} at z {}, and the patch was made against \
                 graphic {} at z {}",
                which.0, found.x, found.y, found.tile.0, found.z, recorded.tile.0, recorded.z
            ),
        }
    }
}

impl std::error::Error for PatchError {}

/// Apply a patch's ops to a map, or leave the map exactly as it was.
///
/// The revision and the facet are [`MapSnapshot::publish`]'s to check — this
/// takes a bare `WorldMap` because it is the half that touches cells, and keeping it
/// separate is what lets the rollback below be about ops alone.
///
/// On success it hands back the way *back*: the inverses, in the order they
/// would have to be replayed. A caller that will never need it — a load
/// replaying a committed history, where the failure is a world that does not
/// open — drops it; a live publish holds it until the patch is safely written
/// down. It is a return value rather than something a caller could derive,
/// because [`PatchOp::AddStatic`]'s inverse names an ordinal only the applier
/// knows.
///
/// # Errors
///
/// The first op that cannot apply, after undoing the ones that already did.
pub(crate) fn apply(map: &mut WorldMap, ops: &[PatchOp]) -> Result<Vec<PatchOp>, PatchError> {
    let mut undo: Vec<PatchOp> = Vec::with_capacity(ops.len());
    for op in ops {
        match apply_op(map, op) {
            Ok(inverse) => undo.push(inverse),
            Err(error) => {
                // Backwards, because each inverse was computed against the world
                // as it stood *after* the op before it. Nothing here can fail:
                // every one of these was true a moment ago, in the reverse
                // order it is being undone in.
                revert(map, &reversed(undo));
                return Err(error);
            }
        }
    }
    Ok(reversed(undo))
}

/// The same inverses, in the order they must be replayed.
fn reversed(mut undo: Vec<PatchOp>) -> Vec<PatchOp> {
    undo.reverse();
    undo
}

/// Replay inverses this module itself produced.
///
/// Infallible by construction and not by check: every op here was true of this
/// world a moment ago, and they are replayed in the one order that keeps each of
/// them true. A failure is this module disagreeing with itself, which is a bug
/// rather than a state a caller could be handed.
pub(crate) fn revert(map: &mut WorldMap, ops: &[PatchOp]) {
    for op in ops {
        apply_op(map, op).expect("an op this module itself inverted");
    }
}

/// Apply one op, and hand back the op that would undo it.
///
/// The inverse is a *return value* rather than something derived beside the
/// call because [`PatchOp::AddStatic`]'s inverse names an ordinal, and the
/// ordinal is only knowable once the item is in. That is the same reason a
/// revert has to be built by replaying rather than by reading the patch.
fn apply_op(map: &mut WorldMap, op: &PatchOp) -> Result<PatchOp, PatchError> {
    match *op {
        PatchOp::SetLand { x, y, was, now } => {
            let found = map.land(x, y).ok_or(PatchError::OffMap { x, y })?;
            if found != was {
                return Err(PatchError::LandNotAsRecorded {
                    x,
                    y,
                    recorded: was,
                    found,
                });
            }
            map.set_land(x, y, now);
            Ok(PatchOp::SetLand {
                x,
                y,
                was: now,
                now: was,
            })
        }
        PatchOp::AddStatic { item } => {
            if !map.contains(item.x, item.y) {
                return Err(PatchError::OffMap { x: item.x, y: item.y });
            }
            map.place_static(item);
            // It went in after everything already on its tile, so it is the
            // last one — and the tile now holds at least one thing.
            let which = StaticId(
                u16::try_from(map.statics_at(item.x, item.y).count() - 1)
                    .expect("fewer than 65,536 statics on one tile"),
            );
            Ok(PatchOp::RemoveStatic { which, was: item })
        }
        PatchOp::RemoveStatic { which, was } => {
            let (x, y) = (was.x, was.y);
            if !map.contains(x, y) {
                return Err(PatchError::OffMap { x, y });
            }
            let standing = map.statics_at(x, y).count();
            let found = *map
                .statics_at(x, y)
                .nth(which.0 as usize)
                .ok_or(PatchError::NoSuchStatic {
                    x,
                    y,
                    which,
                    standing,
                })?;
            if found != was {
                return Err(PatchError::StaticNotAsRecorded {
                    which,
                    recorded: was,
                    found,
                });
            }
            map.remove_static(x, y, which.0 as usize)
                .expect("the static this call just looked at");
            Ok(PatchOp::AddStatic { item: was })
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::wire::{Graphic, Hue};

    use super::*;
    use crate::grid::BlockExtent;
    use crate::snapshot::MapSnapshot;
    use openshard_tiles::LandTileId;

    /// A facet of two chunks by two chunks, all of it one flat land tile.
    fn flat() -> MapSnapshot {
        MapSnapshot::new(
            Facet(0),
            WorldMap::from_blocks(BlockExtent { wide: 16, down: 16 }, |_, _| LandCell {
                tile: LandTileId(3),
                z: 0,
            }),
        )
    }

    fn rock(x: u16, y: u16, z: i8) -> StaticItem {
        StaticItem {
            tile: Graphic(0x1234),
            x,
            y,
            z,
            hue: Hue::NONE,
        }
    }

    fn patch(parent: MapRevision, ops: Vec<PatchOp>) -> Patch {
        Patch::new(Facet(0), parent, PatchAuthor("a test".into()), PatchTime(0), ops)
    }

    #[test]
    fn a_published_patch_changes_the_world_and_the_revision() {
        let mut world = flat();
        let was = world.map().land(10, 10).expect("on the map");
        let now = LandCell {
            tile: LandTileId(9),
            z: 40,
        };
        world
            .publish(&patch(
                world.revision(),
                vec![
                    PatchOp::SetLand {
                        x: 10,
                        y: 10,
                        was,
                        now,
                    },
                    PatchOp::AddStatic {
                        item: rock(10, 10, 40),
                    },
                ],
            ))
            .expect("a patch against the revision in hand");

        assert_eq!(world.revision(), MapRevision::INITIAL.after());
        assert_eq!(world.map().land(10, 10), Some(now));
        assert_eq!(world.map().statics_at(10, 10).count(), 1);
    }

    /// A publish taken back leaves the world at the number it left, not one
    /// further along — and every op of it undone, including the addition whose
    /// inverse names an ordinal nobody could have written down in advance.
    #[test]
    fn an_undone_publish_leaves_no_trace_of_itself() {
        let mut world = flat();
        let before = world.revision();
        let land = world.map().land(4, 4).expect("on the map");
        assert_eq!(
            world.map().statics_at(4, 4).count(),
            0,
            "the fixture starts empty"
        );

        let undo = world
            .publish(&patch(
                before,
                vec![
                    PatchOp::SetLand {
                        x: 4,
                        y: 4,
                        was: land,
                        now: LandCell {
                            tile: LandTileId(1),
                            z: 60,
                        },
                    },
                    PatchOp::AddStatic { item: rock(4, 4, 60) },
                    PatchOp::AddStatic { item: rock(4, 4, 61) },
                ],
            ))
            .expect("a patch against the revision in hand");
        assert_eq!(world.map().statics_at(4, 4).count(), 2);

        world.undo(&undo);

        assert_eq!(world.revision(), before, "the revision goes back too");
        assert_eq!(world.map().land(4, 4), Some(land));
        assert_eq!(world.map().statics_at(4, 4).count(), 0);
    }

    /// The three constructors are the only way a writer builds an op, and the
    /// `was` half is the world's answer rather than the caller's claim.
    #[test]
    fn an_op_reads_what_it_replaces_out_of_the_world() {
        let mut world = flat();
        world
            .publish(&patch(
                world.revision(),
                vec![PatchOp::AddStatic { item: rock(7, 7, 3) }],
            ))
            .expect("a patch against the revision in hand");
        let map = world.map();

        assert_eq!(
            PatchOp::set_land(
                map,
                7,
                7,
                LandCell {
                    tile: LandTileId(5),
                    z: 9
                }
            ),
            Ok(PatchOp::SetLand {
                x: 7,
                y: 7,
                was: LandCell {
                    tile: LandTileId(3),
                    z: 0
                },
                now: LandCell {
                    tile: LandTileId(5),
                    z: 9
                },
            })
        );
        assert_eq!(
            PatchOp::remove_static(map, 7, 7, StaticId(0)),
            Ok(PatchOp::RemoveStatic {
                which: StaticId(0),
                was: rock(7, 7, 3),
            })
        );
        assert_eq!(
            PatchOp::remove_static(map, 7, 7, StaticId(1)),
            Err(PatchError::NoSuchStatic {
                x: 7,
                y: 7,
                which: StaticId(1),
                standing: 1,
            })
        );
        // Off the map is refused by all three, at the point of building.
        let far = u16::try_from(map.width()).unwrap_or(u16::MAX);
        assert_eq!(
            PatchOp::add_static(map, rock(far, 0, 0)),
            Err(PatchError::OffMap { x: far, y: 0 })
        );
    }

    /// The conflict model, and the only one there is.
    #[test]
    fn a_patch_against_a_revision_the_world_has_left_is_refused() {
        let mut world = flat();
        let was = world.map().land(1, 1).expect("on the map");
        let step = |z| PatchOp::SetLand {
            x: 1,
            y: 1,
            was,
            now: LandCell { tile: was.tile, z },
        };
        world
            .publish(&patch(MapRevision::INITIAL, vec![step(5)]))
            .expect("the first patch");

        assert_eq!(
            world.publish(&patch(MapRevision::INITIAL, vec![step(7)])),
            Err(PatchError::Conflict {
                holding: MapRevision::INITIAL.after(),
                parent: MapRevision::INITIAL,
            })
        );
        // And the world is untouched by the refusal, not half of it.
        assert_eq!(world.map().land(1, 1).expect("on the map").z, 5);
    }

    #[test]
    fn a_patch_to_another_facet_is_refused() {
        let mut world = flat();
        let stray = Patch::new(
            Facet(1),
            world.revision(),
            PatchAuthor("a test".into()),
            PatchTime(0),
            Vec::new(),
        );
        assert_eq!(
            world.publish(&stray),
            Err(PatchError::WrongFacet {
                wanted: Facet(0),
                found: Facet(1),
            })
        );
    }

    /// The thing a coordinate and a graphic cannot say.
    #[test]
    fn an_ordinal_tells_two_identical_rocks_on_one_tile_apart() {
        let mut world = flat();
        world
            .publish(&patch(
                world.revision(),
                vec![
                    PatchOp::AddStatic { item: rock(4, 4, 0) },
                    PatchOp::AddStatic { item: rock(4, 4, 0) },
                    PatchOp::AddStatic { item: rock(4, 4, 0) },
                ],
            ))
            .expect("three rocks");
        assert_eq!(world.map().statics_at(4, 4).count(), 3);

        world
            .publish(&patch(
                world.revision(),
                vec![PatchOp::RemoveStatic {
                    which: StaticId(1),
                    was: rock(4, 4, 0),
                }],
            ))
            .expect("the middle rock");
        assert_eq!(world.map().statics_at(4, 4).count(), 2);
    }

    /// Ops see what the ops before them did — which is what makes "remove both
    /// of these" expressible in one patch.
    #[test]
    fn ops_apply_in_order_and_see_each_other() {
        let mut world = flat();
        world
            .publish(&patch(
                world.revision(),
                vec![
                    PatchOp::AddStatic { item: rock(6, 6, 1) },
                    PatchOp::AddStatic { item: rock(6, 6, 2) },
                    // Counts the two above, not the empty tile the patch began
                    // against.
                    PatchOp::RemoveStatic {
                        which: StaticId(0),
                        was: rock(6, 6, 1),
                    },
                ],
            ))
            .expect("two in and one out");
        let left: Vec<i8> = world.map().statics_at(6, 6).map(|item| item.z).collect();
        assert_eq!(left, vec![2]);
    }

    /// All of a patch or none of it, and the rollback is the ops' own inverses.
    #[test]
    fn a_patch_that_fails_halfway_leaves_the_world_where_it_was() {
        let mut world = flat();
        let was = world.map().land(2, 2).expect("on the map");
        let doomed = patch(
            world.revision(),
            vec![
                PatchOp::AddStatic { item: rock(2, 2, 0) },
                PatchOp::SetLand {
                    x: 2,
                    y: 2,
                    was,
                    now: LandCell {
                        tile: LandTileId(1),
                        z: 20,
                    },
                },
                // Nothing stands on this tile, so the patch cannot finish.
                PatchOp::RemoveStatic {
                    which: StaticId(0),
                    was: rock(3, 3, 0),
                },
            ],
        );

        assert_eq!(
            world.publish(&doomed),
            Err(PatchError::NoSuchStatic {
                x: 3,
                y: 3,
                which: StaticId(0),
                standing: 0,
            })
        );
        assert_eq!(world.revision(), MapRevision::INITIAL);
        assert_eq!(world.map().land(2, 2), Some(was));
        assert_eq!(world.map().statics_at(2, 2).count(), 0);
        assert_eq!(world.map().static_count(), 0);
    }

    /// The check that catches a patch log dropped beside the wrong base set —
    /// where the parent revision agrees and the world does not.
    #[test]
    fn an_op_whose_recorded_value_is_not_there_is_refused() {
        let mut world = flat();
        let elsewhere = LandCell {
            tile: LandTileId(77),
            z: -3,
        };
        assert_eq!(
            world.publish(&patch(
                world.revision(),
                vec![PatchOp::SetLand {
                    x: 5,
                    y: 5,
                    was: elsewhere,
                    now: LandCell {
                        tile: LandTileId(1),
                        z: 0,
                    },
                }],
            )),
            Err(PatchError::LandNotAsRecorded {
                x: 5,
                y: 5,
                recorded: elsewhere,
                found: LandCell {
                    tile: LandTileId(3),
                    z: 0,
                },
            })
        );
    }

    #[test]
    fn an_op_off_the_map_is_refused() {
        let mut world = flat();
        assert_eq!(
            world.publish(&patch(
                world.revision(),
                vec![PatchOp::AddStatic {
                    item: rock(9_000, 1, 0),
                }],
            )),
            Err(PatchError::OffMap { x: 9_000, y: 1 })
        );
    }

    /// What direction D asks a patch: which bakes died.
    #[test]
    fn touched_chunks_are_the_chunks_the_ops_are_in_each_once() {
        let was = LandCell {
            tile: LandTileId(3),
            z: 0,
        };
        let now = LandCell {
            tile: LandTileId(4),
            z: 0,
        };
        let ops = vec![
            PatchOp::SetLand {
                x: 70,
                y: 5,
                was,
                now,
            },
            PatchOp::AddStatic { item: rock(3, 3, 0) },
            // The same chunk as the first op, and it is not listed twice.
            PatchOp::SetLand {
                x: 100,
                y: 60,
                was,
                now,
            },
        ];
        assert_eq!(
            patch(MapRevision::INITIAL, ops).touched_chunks(),
            vec![ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 1, y: 0 }]
        );
    }
}
