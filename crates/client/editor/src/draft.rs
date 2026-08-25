//! An unpublished map edit, projected over one exact base revision.
//!
//! A draft never owns or mutates the authoritative [`WorldMap`]. Its sparse
//! land/static overlays are the map an editor previews and the source a later
//! [`Gesture`] reads, so successive gestures naturally build on each other.
//! The original state of a tile is captured only when a successful gesture
//! first touches it. History stores exact before/after tile states; the
//! eventual patch is derived afresh from those originals to the preview and
//! therefore contains no undone work or changes that returned to base.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use openshard_map::chunk::ChunkCoord;
use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::patch::{Patch, PatchAuthor, PatchError, PatchOp, PatchTime, StaticId};
use openshard_map::snapshot::MapRevision;
use openshard_protocol::world::Facet;

use crate::tools::{Gesture, GestureView, TilePoint};

/// A draft's pinned parent no longer names the world a caller is holding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DraftConflict {
    /// Facet selected when editing began.
    pub expected_facet: Facet,
    /// Revision selected when editing began.
    pub expected_revision: MapRevision,
    /// Facet held now.
    pub actual_facet: Facet,
    /// Revision held now.
    pub actual_revision: MapRevision,
}

impl fmt::Display for DraftConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "draft is for facet {} revision {}, but the world is facet {} revision {}",
            self.expected_facet.0,
            self.expected_revision.get(),
            self.actual_facet.0,
            self.actual_revision.get()
        )
    }
}

impl std::error::Error for DraftConflict {}

/// Why a completed gesture could not become a draft history command.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DraftError {
    /// An operation disagreed with the preview it was submitted against.
    Patch(PatchError),
    /// Adding another item would produce an ordinal canonical patches cannot
    /// address.
    TooManyStatics {
        /// Tile whose static sequence is full.
        at: TilePoint,
        /// Number already standing there.
        standing: usize,
    },
}

impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Patch(source) => write!(f, "gesture does not apply to the draft preview: {source}"),
            Self::TooManyStatics { at, standing } => write!(
                f,
                "tile ({}, {}) already has {standing} statics and cannot address another one",
                at.x, at.y
            ),
        }
    }
}

impl std::error::Error for DraftError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Patch(source) => Some(source),
            Self::TooManyStatics { .. } => None,
        }
    }
}

impl From<PatchError> for DraftError {
    fn from(source: PatchError) -> Self {
        Self::Patch(source)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct TileState {
    at: TilePoint,
    land: LandCell,
    statics: Vec<StaticItem>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Command {
    before: Vec<TileState>,
    after: Vec<TileState>,
}

/// UI-independent unpublished changes over one facet revision.
#[derive(Debug)]
pub struct Draft {
    facet: Facet,
    revision: MapRevision,
    originals: BTreeMap<TilePoint, TileState>,
    land: BTreeMap<TilePoint, LandCell>,
    statics: BTreeMap<TilePoint, Vec<StaticItem>>,
    history: Vec<Command>,
    applied: usize,
    dirty_tiles: BTreeSet<TilePoint>,
    dirty_chunks: BTreeSet<ChunkCoord>,
}

impl Draft {
    /// Begin an empty draft pinned to an authoritative map revision.
    #[must_use]
    pub fn new(facet: Facet, revision: MapRevision) -> Self {
        Self {
            facet,
            revision,
            originals: BTreeMap::new(),
            land: BTreeMap::new(),
            statics: BTreeMap::new(),
            history: Vec::new(),
            applied: 0,
            dirty_tiles: BTreeSet::new(),
            dirty_chunks: BTreeSet::new(),
        }
    }

    /// Facet selected when editing began.
    #[must_use]
    pub const fn facet(&self) -> Facet {
        self.facet
    }

    /// Revision the eventual patch will name as its parent.
    #[must_use]
    pub const fn revision(&self) -> MapRevision {
        self.revision
    }

    /// Whether this draft can still be published against a current selection.
    #[must_use]
    pub fn conflict_with(&self, facet: Facet, revision: MapRevision) -> Option<DraftConflict> {
        if self.facet == facet && self.revision == revision {
            None
        } else {
            Some(DraftConflict {
                expected_facet: self.facet,
                expected_revision: self.revision,
                actual_facet: facet,
                actual_revision: revision,
            })
        }
    }

    /// Start a gesture against the current preview, including all applied
    /// history commands.
    #[must_use]
    pub fn gesture<'map>(&'map self, base: &'map WorldMap) -> Gesture<'map> {
        Gesture::from_view(DraftView { draft: self, base })
    }

    /// Current preview ground at one tile.
    #[must_use]
    pub fn land(&self, base: &WorldMap, x: u16, y: u16) -> Option<LandCell> {
        let at = TilePoint::new(x, y);
        self.land
            .get(&at)
            .copied()
            .or_else(|| self.originals.get(&at).map(|state| state.land))
            .or_else(|| base.land(x, y))
    }

    /// Current preview statics at one tile, in their canonical ordinal order.
    #[must_use]
    pub fn statics_at(&self, base: &WorldMap, x: u16, y: u16) -> Vec<StaticItem> {
        let at = TilePoint::new(x, y);
        self.statics
            .get(&at)
            .cloned()
            .or_else(|| self.originals.get(&at).map(|state| state.statics.clone()))
            .unwrap_or_else(|| base.statics_at(x, y).copied().collect())
    }

    /// Submit the operations returned by one finished [`Gesture`] as one undo
    /// unit.
    ///
    /// Returns `false` when the gesture has no net effect. Invalid operations
    /// leave the preview and history untouched.
    ///
    /// # Errors
    ///
    /// A canonical patch error when an operation disagrees with the current
    /// preview, or [`DraftError::TooManyStatics`] at the ordinal limit.
    pub fn apply_gesture(&mut self, base: &WorldMap, ops: Vec<PatchOp>) -> Result<bool, DraftError> {
        let touched: BTreeSet<TilePoint> = ops
            .iter()
            .map(|op| {
                let (x, y) = op.at();
                TilePoint::new(x, y)
            })
            .collect();
        if touched.is_empty() {
            return Ok(false);
        }
        if let Some(at) = touched.iter().find(|at| !base.contains(at.x, at.y)) {
            return Err(PatchError::OffMap { x: at.x, y: at.y }.into());
        }

        let before: Vec<TileState> = touched.iter().map(|at| self.tile_state(base, *at)).collect();
        let mut working: BTreeMap<TilePoint, TileState> =
            before.iter().cloned().map(|state| (state.at, state)).collect();

        for op in ops {
            Self::apply_op(&mut working, op)?;
        }

        let after: Vec<TileState> = working.into_values().collect();
        if before == after {
            return Ok(false);
        }

        for state in &before {
            self.originals.entry(state.at).or_insert_with(|| TileState {
                at: state.at,
                land: base
                    .land(state.at.x, state.at.y)
                    .expect("a touched tile is on the base map"),
                statics: base.statics_at(state.at.x, state.at.y).copied().collect(),
            });
        }
        self.history.truncate(self.applied);
        self.restore(&after);
        self.history.push(Command { before, after });
        self.applied += 1;
        Ok(true)
    }

    /// Whether an undo command is available.
    #[must_use]
    pub const fn can_undo(&self) -> bool {
        self.applied != 0
    }

    /// Whether a redo command is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.applied < self.history.len()
    }

    /// Number of currently applied gesture commands.
    #[must_use]
    pub const fn undo_len(&self) -> usize {
        self.applied
    }

    /// Number of gesture commands available to redo.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.history.len() - self.applied
    }

    /// Restore the exact preview before the most recent gesture.
    pub fn undo(&mut self) -> bool {
        let Some(index) = self.applied.checked_sub(1) else {
            return false;
        };
        let before = self.history[index].before.clone();
        self.restore(&before);
        self.applied = index;
        true
    }

    /// Reapply the exact preview after the next undone gesture.
    pub fn redo(&mut self) -> bool {
        let Some(command) = self.history.get(self.applied) else {
            return false;
        };
        let after = command.after.clone();
        self.restore(&after);
        self.applied += 1;
        true
    }

    /// Throw away preview state and all undo/redo history.
    pub fn discard(&mut self) {
        self.land.clear();
        self.statics.clear();
        self.originals.clear();
        self.history.clear();
        self.applied = 0;
        self.dirty_tiles.clear();
        self.dirty_chunks.clear();
    }

    /// Whether the preview differs from the base map.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.dirty_tiles.is_empty()
    }

    /// Tiles whose complete preview state differs from the base map.
    #[must_use]
    pub const fn dirty_tiles(&self) -> &BTreeSet<TilePoint> {
        &self.dirty_tiles
    }

    /// Chunks containing dirty tiles.
    #[must_use]
    pub const fn dirty_chunks(&self) -> &BTreeSet<ChunkCoord> {
        &self.dirty_chunks
    }

    /// Canonical operation count of the patch the draft would currently make.
    #[must_use]
    pub fn op_count(&self, base: &WorldMap) -> usize {
        self.commit_ops(base).len()
    }

    /// Canonical operations transforming the pinned base into this preview.
    #[must_use]
    pub fn commit_ops(&self, base: &WorldMap) -> Vec<PatchOp> {
        let mut ops = Vec::new();
        for at in &self.dirty_tiles {
            let original = self.original(at);
            let base_land = original.land;
            let preview_land = self
                .land(base, at.x, at.y)
                .expect("a dirty tile is on the preview");
            if base_land != preview_land {
                ops.push(PatchOp::SetLand {
                    x: at.x,
                    y: at.y,
                    was: base_land,
                    now: preview_land,
                });
            }

            let base_statics = &original.statics;
            let preview_statics = self.statics_at(base, at.x, at.y);
            if base_statics.as_slice() == preview_statics {
                continue;
            }
            let common = base_statics
                .iter()
                .zip(&preview_statics)
                .take_while(|(base, preview)| base == preview)
                .count();
            for index in (common..base_statics.len()).rev() {
                ops.push(PatchOp::RemoveStatic {
                    which: StaticId(
                        u16::try_from(index).expect("a draft accepts addressable static ordinals"),
                    ),
                    was: base_statics[index],
                });
            }
            ops.extend(
                preview_statics[common..]
                    .iter()
                    .copied()
                    .map(|item| PatchOp::AddStatic { item }),
            );
        }
        ops
    }

    /// Build the patch to publish, or `None` when the preview equals its base.
    #[must_use]
    pub fn patch(&self, base: &WorldMap, author: PatchAuthor, at: PatchTime) -> Option<Patch> {
        let ops = self.commit_ops(base);
        (!ops.is_empty()).then(|| Patch::new(self.facet, self.revision, author, at, ops))
    }

    fn tile_state(&self, base: &WorldMap, at: TilePoint) -> TileState {
        TileState {
            at,
            land: self
                .land(base, at.x, at.y)
                .expect("a canonical operation's tile is on the preview"),
            statics: self.statics_at(base, at.x, at.y),
        }
    }

    fn original(&self, at: &TilePoint) -> &TileState {
        self.originals
            .get(at)
            .expect("every dirty tile captured its original state")
    }

    fn apply_op(working: &mut BTreeMap<TilePoint, TileState>, op: PatchOp) -> Result<(), DraftError> {
        let (x, y) = op.at();
        let at = TilePoint::new(x, y);
        let Some(state) = working.get_mut(&at) else {
            return Err(PatchError::OffMap { x, y }.into());
        };
        match op {
            PatchOp::SetLand { was, now, .. } => {
                if state.land != was {
                    return Err(PatchError::LandNotAsRecorded {
                        x,
                        y,
                        recorded: was,
                        found: state.land,
                    }
                    .into());
                }
                state.land = now;
            }
            PatchOp::AddStatic { item } => {
                if state.statics.len() > usize::from(u16::MAX) {
                    return Err(DraftError::TooManyStatics {
                        at,
                        standing: state.statics.len(),
                    });
                }
                state.statics.push(item);
            }
            PatchOp::RemoveStatic { which, was } => {
                let standing = state.statics.len();
                let Some(found) = state.statics.get(usize::from(which.0)).copied() else {
                    return Err(PatchError::NoSuchStatic {
                        x,
                        y,
                        which,
                        standing,
                    }
                    .into());
                };
                if found != was {
                    return Err(PatchError::StaticNotAsRecorded {
                        which,
                        recorded: was,
                        found,
                    }
                    .into());
                }
                state.statics.remove(usize::from(which.0));
            }
        }
        Ok(())
    }

    fn restore(&mut self, states: &[TileState]) {
        for state in states {
            let original = self
                .originals
                .get(&state.at)
                .expect("every history tile captured its original state");
            let base_land = original.land;
            if state.land == base_land {
                self.land.remove(&state.at);
            } else {
                self.land.insert(state.at, state.land);
            }

            if state.statics == original.statics {
                self.statics.remove(&state.at);
            } else {
                self.statics.insert(state.at, state.statics.clone());
            }

            if self.land.contains_key(&state.at) || self.statics.contains_key(&state.at) {
                self.dirty_tiles.insert(state.at);
            } else {
                self.dirty_tiles.remove(&state.at);
            }
        }
        self.dirty_chunks = self
            .dirty_tiles
            .iter()
            .map(|at| ChunkCoord::containing(at.x, at.y))
            .collect();
    }
}

#[derive(Debug)]
struct DraftView<'a> {
    draft: &'a Draft,
    base: &'a WorldMap,
}

impl GestureView for DraftView<'_> {
    fn width(&self) -> u32 {
        self.base.width()
    }

    fn height(&self) -> u32 {
        self.base.height()
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        self.base.contains(x, y)
    }

    fn land(&self, x: u16, y: u16) -> Option<LandCell> {
        self.draft.land(self.base, x, y)
    }

    fn statics_at(&self, x: u16, y: u16) -> Vec<StaticItem> {
        self.draft.statics_at(self.base, x, y)
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_map::snapshot::MapSnapshot;
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_tiles::LandTileId;

    use super::*;
    use crate::tools::{Brush, HeightStrength, StaticHeight, StaticPlacement, TargetHeight, Tool};

    const FACET: Facet = Facet(2);
    const AT: TilePoint = TilePoint::new(3, 4);

    fn flat() -> WorldMap {
        WorldMap::from_blocks(BlockExtent { wide: 16, down: 8 }, |_, _| LandCell {
            tile: LandTileId(3),
            z: 10,
        })
    }

    fn rock(tile: u16, x: u16, y: u16) -> StaticItem {
        StaticItem {
            tile: Graphic(tile),
            x,
            y,
            z: 10,
            hue: Hue::NONE,
        }
    }

    fn apply(draft: &mut Draft, base: &WorldMap, tool: Tool, at: TilePoint) {
        let ops = {
            let mut gesture = draft.gesture(base);
            gesture.apply(tool, Brush::default(), at).unwrap();
            gesture.finish()
        };
        assert!(draft.apply_gesture(base, ops).unwrap());
    }

    #[test]
    fn successive_gestures_compile_against_the_preview() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);

        assert_eq!(draft.land(&base, AT.x, AT.y).unwrap().z, 12);
        assert_eq!(
            draft.op_count(&base),
            1,
            "land changes coalesce in the commit patch"
        );
        assert!(matches!(
            draft.commit_ops(&base).as_slice(),
            [PatchOp::SetLand {
                was: LandCell { z: 10, .. },
                now: LandCell { z: 12, .. },
                ..
            }]
        ));
    }

    #[test]
    fn draft_is_owned_and_keeps_touched_originals_after_the_caller_map_moves() {
        fn assert_static<T: 'static>() {}
        assert_static::<Draft>();

        let mut base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        base.set_land(
            AT.x,
            AT.y,
            LandCell {
                tile: LandTileId(99),
                z: 50,
            },
        );

        assert_eq!(draft.land(&base, AT.x, AT.y).unwrap().z, 11);
        assert!(matches!(
            draft.commit_ops(&base).as_slice(),
            [PatchOp::SetLand {
                was: LandCell { z: 10, .. },
                now: LandCell { z: 11, .. },
                ..
            }]
        ));
    }

    #[test]
    fn undo_and_redo_restore_exact_gesture_boundaries() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        apply(&mut draft, &base, Tool::PaintLand(LandTileId(90)), AT);

        assert!(draft.undo());
        assert_eq!(
            draft.land(&base, AT.x, AT.y).unwrap(),
            LandCell {
                tile: LandTileId(3),
                z: 11
            }
        );
        assert!(draft.undo());
        assert_eq!(draft.land(&base, AT.x, AT.y), base.land(AT.x, AT.y));
        assert!(!draft.undo());
        assert!(draft.redo());
        assert!(draft.redo());
        assert_eq!(
            draft.land(&base, AT.x, AT.y).unwrap(),
            LandCell {
                tile: LandTileId(90),
                z: 11
            }
        );
        assert!(!draft.redo());
    }

    #[test]
    fn a_new_gesture_after_undo_invalidates_redo() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        assert!(draft.undo());
        apply(&mut draft, &base, Tool::Flatten(TargetHeight(-4)), AT);

        assert!(!draft.can_redo());
        assert_eq!(draft.redo_len(), 0);
        assert_eq!(draft.land(&base, AT.x, AT.y).unwrap().z, -4);
    }

    #[test]
    fn discard_clears_preview_history_and_dirt() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        draft.discard();

        assert_eq!(draft.land(&base, AT.x, AT.y), base.land(AT.x, AT.y));
        assert!(!draft.is_dirty());
        assert!(!draft.can_undo());
        assert!(!draft.can_redo());
        assert!(
            draft
                .patch(&base, PatchAuthor("test".into()), PatchTime(1))
                .is_none()
        );
    }

    #[test]
    fn dirty_tiles_fold_into_unique_chunks_and_clear_on_undo() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        let first = TilePoint::new(1, 1);
        let same_chunk = TilePoint::new(63, 63);
        let other_chunk = TilePoint::new(64, 1);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), first);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), same_chunk);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), other_chunk);

        assert_eq!(draft.dirty_tiles().len(), 3);
        assert_eq!(
            draft.dirty_chunks(),
            &BTreeSet::from([ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 1, y: 0 }])
        );
        assert!(draft.undo());
        assert_eq!(draft.dirty_chunks(), &BTreeSet::from([ChunkCoord { x: 0, y: 0 }]));
    }

    #[test]
    fn land_that_returns_to_base_makes_no_commit() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        apply(&mut draft, &base, Tool::Lower(HeightStrength::ONE), AT);

        assert!(!draft.is_dirty());
        assert_eq!(draft.op_count(&base), 0);
        assert!(
            draft
                .patch(&base, PatchAuthor("test".into()), PatchTime(2))
                .is_none()
        );
    }

    #[test]
    fn static_add_remove_sequences_keep_ordinals_and_cancel_noops() {
        let mut base = flat();
        let old = rock(1, AT.x, AT.y);
        base.place_static(old);
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        let placement = StaticPlacement {
            tile: Graphic(2),
            height: StaticHeight::OnGround,
            hue: Hue(7),
        };
        apply(&mut draft, &base, Tool::PlaceStatic(placement), AT);
        apply(&mut draft, &base, Tool::RemoveStatic(StaticId(1)), AT);

        assert_eq!(draft.statics_at(&base, AT.x, AT.y), vec![old]);
        assert!(!draft.is_dirty());
        assert_eq!(draft.op_count(&base), 0);

        apply(&mut draft, &base, Tool::RemoveStatic(StaticId(0)), AT);
        apply(&mut draft, &base, Tool::PlaceStatic(placement), AT);
        assert_eq!(draft.op_count(&base), 2);

        assert!(draft.undo());
        assert!(draft.statics_at(&base, AT.x, AT.y).is_empty());
        assert!(draft.redo());
        assert_eq!(draft.statics_at(&base, AT.x, AT.y)[0].tile, Graphic(2));
    }

    #[test]
    fn invalid_completed_gestures_are_typed_and_atomic() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        let error = draft
            .apply_gesture(
                &base,
                vec![PatchOp::SetLand {
                    x: u16::MAX,
                    y: u16::MAX,
                    was: LandCell::default(),
                    now: LandCell {
                        tile: LandTileId(1),
                        z: 1,
                    },
                }],
            )
            .unwrap_err();

        assert!(matches!(error, DraftError::Patch(PatchError::OffMap { .. })));
        assert!(!draft.is_dirty());
        assert_eq!(draft.undo_len(), 0);
        assert!(draft.conflict_with(FACET, MapRevision::INITIAL.after()).is_some());
    }

    #[test]
    fn the_built_patch_publishes_against_an_equivalent_base_snapshot() {
        let base = flat();
        let mut draft = Draft::new(FACET, MapRevision::INITIAL);
        apply(&mut draft, &base, Tool::Raise(HeightStrength::ONE), AT);
        apply(
            &mut draft,
            &base,
            Tool::PlaceStatic(StaticPlacement {
                tile: Graphic(55),
                height: StaticHeight::OnGround,
                hue: Hue::NONE,
            }),
            AT,
        );
        let patch = draft
            .patch(&base, PatchAuthor("editor".into()), PatchTime(7))
            .expect("a dirty draft makes a patch");
        assert_eq!(patch.parent(), MapRevision::INITIAL);

        let mut snapshot = MapSnapshot::new(FACET, flat());
        snapshot.publish(&patch).unwrap();
        assert_eq!(snapshot.revision(), MapRevision::INITIAL.after());
        assert_eq!(snapshot.map().land(AT.x, AT.y).unwrap().z, 11);
        assert_eq!(snapshot.map().statics_at(AT.x, AT.y).count(), 1);
    }
}
