//! Deterministic editor brushes and their canonical map operations.
//!
//! A [`Gesture`] is deliberately smaller than a draft. It compiles one drag or
//! click against the world the user is looking at, remembering overlaps between
//! its dabs so a tile touched twice still records one truthful `was` value. A
//! later draft can own many gestures, preview their returned operations and
//! provide history without putting any UI or history policy in this module.

use std::collections::BTreeMap;
use std::num::NonZeroU8;

use openshard_map::map::{LandCell, StaticItem, WorldMap};
use openshard_map::patch::{PatchError, PatchOp, StaticId};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_tiles::LandTileId;

/// The small read surface a gesture needs from either the base map or a draft
/// preview.
///
/// Kept inside the crate: presenting a second general-purpose map interface is
/// not part of the editor API. [`Gesture::new`] remains the base-map entry
/// point, while the draft uses the same compiler through
/// [`Gesture::from_view`].
pub(crate) trait GestureView: std::fmt::Debug {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn contains(&self, x: u16, y: u16) -> bool;
    fn land(&self, x: u16, y: u16) -> Option<LandCell>;
    fn statics_at(&self, x: u16, y: u16) -> Vec<StaticItem>;
}

impl GestureView for WorldMap {
    fn width(&self) -> u32 {
        self.width()
    }

    fn height(&self) -> u32 {
        self.height()
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        self.contains(x, y)
    }

    fn land(&self, x: u16, y: u16) -> Option<LandCell> {
        self.land(x, y)
    }

    fn statics_at(&self, x: u16, y: u16) -> Vec<StaticItem> {
        self.statics_at(x, y).copied().collect()
    }
}

impl<T: GestureView + ?Sized> GestureView for &T {
    fn width(&self) -> u32 {
        T::width(self)
    }

    fn height(&self) -> u32 {
        T::height(self)
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        T::contains(self, x, y)
    }

    fn land(&self, x: u16, y: u16) -> Option<LandCell> {
        T::land(self, x, y)
    }

    fn statics_at(&self, x: u16, y: u16) -> Vec<StaticItem> {
        T::statics_at(self, x, y)
    }
}

/// One tile in world coordinates.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TilePoint {
    /// East/west coordinate.
    pub x: u16,
    /// North/south coordinate.
    pub y: u16,
}

impl TilePoint {
    /// Name one map tile.
    #[must_use]
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

/// The area selected around a brush centre.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum BrushShape {
    /// Euclidean disc: offsets whose squared distance is at most the squared radius.
    #[default]
    Circle,
    /// Every tile in the enclosing `(2r + 1)` by `(2r + 1)` square.
    Square,
}

/// Distance from the centre to the edge of a brush, in tiles.
///
/// It is an `u8` because a UI brush does not need a coordinate-sized radius,
/// and the bound prevents one accidental value from attempting billions of
/// tiles. Radius zero is the useful one-tile brush.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct BrushRadius(u8);

impl BrushRadius {
    /// A brush radius, in tiles.
    #[must_use]
    pub const fn new(tiles: u8) -> Self {
        Self(tiles)
    }

    /// The radius in tiles.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The geometry shared by terrain brushes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Brush {
    /// Shape of a dab.
    pub shape: BrushShape,
    /// Distance from its centre to its edge.
    pub radius: BrushRadius,
}

impl Brush {
    /// A brush with explicit shape and radius.
    #[must_use]
    pub const fn new(shape: BrushShape, radius: BrushRadius) -> Self {
        Self { shape, radius }
    }

    /// Tiles covered by one dab, clipped to `map`, once each in row-major order.
    #[must_use]
    pub fn footprint(self, map: &WorldMap, centre: TilePoint) -> Vec<TilePoint> {
        self.footprint_in(map, centre)
    }

    pub(crate) fn footprint_in(self, map: &dyn GestureView, centre: TilePoint) -> Vec<TilePoint> {
        let radius = i64::from(self.radius.get());
        let centre_x = i64::from(centre.x);
        let centre_y = i64::from(centre.y);
        let width = i64::from(map.width());
        let height = i64::from(map.height());
        if width == 0 || height == 0 {
            return Vec::new();
        }

        let from_x = (centre_x - radius).max(0);
        let to_x = (centre_x + radius).min(width - 1);
        let from_y = (centre_y - radius).max(0);
        let to_y = (centre_y + radius).min(height - 1);
        if from_x > to_x || from_y > to_y {
            return Vec::new();
        }

        let side = usize::from(self.radius.get()) * 2 + 1;
        let mut footprint = Vec::with_capacity(side.saturating_mul(side));
        let squared_radius = radius * radius;
        for y in from_y..=to_y {
            for x in from_x..=to_x {
                let included = match self.shape {
                    BrushShape::Circle => {
                        let dx = x - centre_x;
                        let dy = y - centre_y;
                        dx * dx + dy * dy <= squared_radius
                    }
                    BrushShape::Square => true,
                };
                if included {
                    footprint.push(TilePoint {
                        x: u16::try_from(x).expect("a map coordinate fits u16"),
                        y: u16::try_from(y).expect("a map coordinate fits u16"),
                    });
                }
            }
        }
        footprint
    }
}

/// Non-zero amount added to or subtracted from terrain height by one dab.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct HeightStrength(NonZeroU8);

impl HeightStrength {
    /// Make a strength, rejecting the no-op value zero.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        match NonZeroU8::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// A one-height-step brush strength.
    pub const ONE: Self = Self(NonZeroU8::MIN);

    /// The number of height steps applied by one dab.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

impl Default for HeightStrength {
    fn default() -> Self {
        Self::ONE
    }
}

/// Absolute height selected by an editor tool.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TargetHeight(pub i8);

/// How a placed static chooses its base height.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum StaticHeight {
    /// Use the current (possibly already edited in this gesture) ground height.
    #[default]
    OnGround,
    /// Use an explicitly picked base height.
    Fixed(TargetHeight),
}

/// The non-coordinate part of a static placement click.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StaticPlacement {
    /// Static art id.
    pub tile: Graphic,
    /// How its base height is chosen.
    pub height: StaticHeight,
    /// Tint, or [`Hue::NONE`].
    pub hue: Hue,
}

/// One active editor tool.
///
/// Terrain variants use the full brush footprint. Static variants are point
/// tools and use only the click centre; removal's ordinal is interpreted
/// against earlier operations in the same gesture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Change art while preserving each cell's current height.
    PaintLand(LandTileId),
    /// Raise terrain with saturating `i8` arithmetic.
    Raise(HeightStrength),
    /// Lower terrain with saturating `i8` arithmetic.
    Lower(HeightStrength),
    /// Set every covered cell to one absolute height.
    Flatten(TargetHeight),
    /// Place one static at the click centre.
    PlaceStatic(StaticPlacement),
    /// Remove the named current static at the click centre.
    RemoveStatic(StaticId),
}

/// Compiler for all dabs and point clicks in one input gesture.
///
/// Repeated land writes are coalesced in their first-touch position: the op's
/// `was` remains the parent world's cell while `now` follows every later dab.
/// Static overlays retain ordinal changes after additions and removals.
#[derive(Debug)]
pub struct Gesture<'map> {
    map: Box<dyn GestureView + 'map>,
    ops: Vec<PatchOp>,
    land_ops: BTreeMap<TilePoint, usize>,
    statics: BTreeMap<TilePoint, Vec<StaticItem>>,
}

impl<'map> Gesture<'map> {
    /// Start a gesture against the map currently displayed by the editor.
    #[must_use]
    pub fn new(map: &'map WorldMap) -> Self {
        Self::from_view(map)
    }

    pub(crate) fn from_view(map: impl GestureView + 'map) -> Self {
        Self {
            map: Box::new(map),
            ops: Vec::new(),
            land_ops: BTreeMap::new(),
            statics: BTreeMap::new(),
        }
    }

    /// Compile one dab or point click.
    ///
    /// Terrain operations are clipped by [`Brush::footprint`], so they cannot
    /// fail off-map. Static point tools report the canonical map error.
    ///
    /// # Errors
    ///
    /// Returns [`PatchError::OffMap`] for a static click outside the facet, or
    /// [`PatchError::NoSuchStatic`] when removal names no current item.
    pub fn apply(&mut self, tool: Tool, brush: Brush, centre: TilePoint) -> Result<(), PatchError> {
        match tool {
            Tool::PaintLand(tile) => self.apply_land(brush, centre, move |mut cell| {
                cell.tile = tile;
                cell
            }),
            Tool::Raise(strength) => self.apply_land(brush, centre, move |mut cell| {
                cell.z = saturating_raise(cell.z, strength);
                cell
            }),
            Tool::Lower(strength) => self.apply_land(brush, centre, move |mut cell| {
                cell.z = saturating_lower(cell.z, strength);
                cell
            }),
            Tool::Flatten(target) => self.apply_land(brush, centre, move |mut cell| {
                cell.z = target.0;
                cell
            }),
            Tool::PlaceStatic(placement) => self.place_static(centre, placement)?,
            Tool::RemoveStatic(which) => self.remove_static(centre, which)?,
        }
        Ok(())
    }

    /// Operations compiled so far, in stable first-touch order.
    #[must_use]
    pub fn ops(&self) -> &[PatchOp] {
        &self.ops
    }

    /// Finish the gesture and return its canonical operations.
    #[must_use]
    pub fn finish(self) -> Vec<PatchOp> {
        self.ops
    }

    fn apply_land(&mut self, brush: Brush, centre: TilePoint, mut edit: impl FnMut(LandCell) -> LandCell) {
        for at in brush.footprint_in(self.map.as_ref(), centre) {
            let current = self.current_land(at);
            let now = edit(current);
            if now == current {
                continue;
            }

            if let Some(index) = self.land_ops.get(&at).copied() {
                let PatchOp::SetLand { now: prior, .. } = &mut self.ops[index] else {
                    unreachable!("land operation indices only name SetLand operations");
                };
                *prior = now;
            } else {
                let op = PatchOp::SetLand {
                    x: at.x,
                    y: at.y,
                    was: current,
                    now,
                };
                self.land_ops.insert(at, self.ops.len());
                self.ops.push(op);
            }
        }
    }

    fn current_land(&self, at: TilePoint) -> LandCell {
        self.land_ops.get(&at).map_or_else(
            || self.map.land(at.x, at.y).expect("a footprint tile is on the map"),
            |index| {
                let PatchOp::SetLand { now, .. } = self.ops[*index] else {
                    unreachable!("land operation indices only name SetLand operations");
                };
                now
            },
        )
    }

    fn place_static(&mut self, at: TilePoint, placement: StaticPlacement) -> Result<(), PatchError> {
        if !self.map.contains(at.x, at.y) {
            return Err(PatchError::OffMap { x: at.x, y: at.y });
        }
        let z = match placement.height {
            StaticHeight::OnGround => self.current_land(at).z,
            StaticHeight::Fixed(target) => target.0,
        };
        let item = StaticItem {
            tile: placement.tile,
            x: at.x,
            y: at.y,
            z,
            hue: placement.hue,
        };
        let op = PatchOp::AddStatic { item };
        self.current_statics(at).push(item);
        self.ops.push(op);
        Ok(())
    }

    fn remove_static(&mut self, at: TilePoint, which: StaticId) -> Result<(), PatchError> {
        if !self.map.contains(at.x, at.y) {
            return Err(PatchError::OffMap { x: at.x, y: at.y });
        }
        let items = self.current_statics(at);
        let standing = items.len();
        let Some(was) = items.get(usize::from(which.0)).copied() else {
            return Err(PatchError::NoSuchStatic {
                x: at.x,
                y: at.y,
                which,
                standing,
            });
        };
        items.remove(usize::from(which.0));
        self.ops.push(PatchOp::RemoveStatic { which, was });
        Ok(())
    }

    fn current_statics(&mut self, at: TilePoint) -> &mut Vec<StaticItem> {
        self.statics
            .entry(at)
            .or_insert_with(|| self.map.statics_at(at.x, at.y))
    }
}

fn saturating_raise(z: i8, strength: HeightStrength) -> i8 {
    let raised = i16::from(z) + i16::from(strength.get());
    raised.min(i16::from(i8::MAX)) as i8
}

fn saturating_lower(z: i8, strength: HeightStrength) -> i8 {
    let lowered = i16::from(z) - i16::from(strength.get());
    lowered.max(i16::from(i8::MIN)) as i8
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use openshard_map::grid::BlockExtent;

    use super::*;

    fn map(cell: impl FnMut(u16, u16) -> LandCell) -> WorldMap {
        WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, cell)
    }

    fn flat(z: i8) -> WorldMap {
        map(|_, _| LandCell {
            tile: LandTileId(3),
            z,
        })
    }

    #[test]
    fn radius_zero_is_one_tile_and_circle_radius_one_is_a_cross() {
        let map = flat(0);
        let centre = TilePoint::new(3, 3);
        assert_eq!(
            Brush::new(BrushShape::Circle, BrushRadius::new(0)).footprint(&map, centre),
            vec![centre]
        );
        assert_eq!(
            Brush::new(BrushShape::Circle, BrushRadius::new(1)).footprint(&map, centre),
            vec![
                TilePoint::new(3, 2),
                TilePoint::new(2, 3),
                TilePoint::new(3, 3),
                TilePoint::new(4, 3),
                TilePoint::new(3, 4),
            ]
        );
    }

    #[test]
    fn footprint_is_clipped_unique_and_row_major_at_a_corner() {
        let map = flat(0);
        let footprint =
            Brush::new(BrushShape::Square, BrushRadius::new(2)).footprint(&map, TilePoint::new(0, 0));
        assert_eq!(footprint.len(), 9);
        assert_eq!(footprint[0], TilePoint::new(0, 0));
        assert_eq!(footprint[8], TilePoint::new(2, 2));
        assert_eq!(
            footprint.iter().copied().collect::<BTreeSet<_>>().len(),
            footprint.len()
        );
    }

    #[test]
    fn overlapping_dabs_coalesce_and_use_the_gesture_current_height() {
        let map = flat(10);
        let mut gesture = Gesture::new(&map);
        let brush = Brush::default();
        gesture
            .apply(Tool::Raise(HeightStrength::ONE), brush, TilePoint::new(2, 2))
            .unwrap();
        gesture
            .apply(Tool::Raise(HeightStrength::ONE), brush, TilePoint::new(2, 2))
            .unwrap();

        assert_eq!(
            gesture.finish(),
            vec![PatchOp::SetLand {
                x: 2,
                y: 2,
                was: LandCell {
                    tile: LandTileId(3),
                    z: 10,
                },
                now: LandCell {
                    tile: LandTileId(3),
                    z: 12,
                },
            }]
        );
    }

    #[test]
    fn raise_and_lower_saturate_at_i8_limits() {
        let map = map(|x, _| LandCell {
            tile: LandTileId(3),
            z: if x == 0 { i8::MAX - 1 } else { i8::MIN + 1 },
        });
        let strength = HeightStrength::new(20).unwrap();
        let mut gesture = Gesture::new(&map);
        gesture
            .apply(Tool::Raise(strength), Brush::default(), TilePoint::new(0, 0))
            .unwrap();
        gesture
            .apply(Tool::Lower(strength), Brush::default(), TilePoint::new(1, 0))
            .unwrap();

        assert!(matches!(
            gesture.ops()[0],
            PatchOp::SetLand {
                now: LandCell { z: i8::MAX, .. },
                ..
            }
        ));
        assert!(matches!(
            gesture.ops()[1],
            PatchOp::SetLand {
                now: LandCell { z: i8::MIN, .. },
                ..
            }
        ));
    }

    #[test]
    fn flatten_preserves_land_art_and_reads_nonuniform_was_cells() {
        let map = map(|x, y| LandCell {
            tile: LandTileId(x + y * 8),
            z: i8::try_from(x + y).unwrap(),
        });
        let mut gesture = Gesture::new(&map);
        gesture
            .apply(
                Tool::Flatten(TargetHeight(-7)),
                Brush::new(BrushShape::Square, BrushRadius::new(1)),
                TilePoint::new(3, 3),
            )
            .unwrap();

        assert_eq!(gesture.ops().len(), 9);
        for op in gesture.ops() {
            let PatchOp::SetLand { was, now, .. } = op else {
                panic!("flatten only emits land operations");
            };
            assert_eq!(now.tile, was.tile);
            assert_eq!(now.z, -7);
        }
    }

    #[test]
    fn static_tools_construct_add_and_remove_ops_against_gesture_state() {
        let mut map = flat(4);
        let old = StaticItem {
            tile: Graphic(10),
            x: 5,
            y: 6,
            z: 8,
            hue: Hue::NONE,
        };
        map.place_static(old);
        let mut gesture = Gesture::new(&map);
        let at = TilePoint::new(5, 6);
        let placed = StaticPlacement {
            tile: Graphic(20),
            height: StaticHeight::OnGround,
            hue: Hue(30),
        };
        gesture
            .apply(Tool::PlaceStatic(placed), Brush::default(), at)
            .unwrap();
        gesture
            .apply(Tool::RemoveStatic(StaticId(1)), Brush::default(), at)
            .unwrap();
        gesture
            .apply(Tool::RemoveStatic(StaticId(0)), Brush::default(), at)
            .unwrap();

        let added = StaticItem {
            tile: Graphic(20),
            x: 5,
            y: 6,
            z: 4,
            hue: Hue(30),
        };
        assert_eq!(
            gesture.finish(),
            vec![
                PatchOp::AddStatic { item: added },
                PatchOp::RemoveStatic {
                    which: StaticId(1),
                    was: added,
                },
                PatchOp::RemoveStatic {
                    which: StaticId(0),
                    was: old,
                },
            ]
        );
    }

    #[test]
    fn flatten_and_fixed_static_share_one_absolute_height_type() {
        let map = flat(4);
        let mut gesture = Gesture::new(&map);
        let target = TargetHeight(-7);
        gesture
            .apply(Tool::Flatten(target), Brush::default(), TilePoint::new(1, 1))
            .unwrap();
        gesture
            .apply(
                Tool::PlaceStatic(StaticPlacement {
                    tile: Graphic(20),
                    height: StaticHeight::Fixed(target),
                    hue: Hue::NONE,
                }),
                Brush::default(),
                TilePoint::new(2, 2),
            )
            .unwrap();

        assert!(matches!(
            gesture.ops()[0],
            PatchOp::SetLand {
                now: LandCell { z: -7, .. },
                ..
            }
        ));
        assert!(matches!(
            gesture.ops()[1],
            PatchOp::AddStatic {
                item: StaticItem { z: -7, .. }
            }
        ));
    }

    #[test]
    fn static_point_tools_reject_off_map_and_missing_ordinals() {
        let map = flat(0);
        let mut gesture = Gesture::new(&map);
        let off_map = TilePoint::new(8, 0);
        assert_eq!(
            gesture.apply(
                Tool::PlaceStatic(StaticPlacement {
                    tile: Graphic(1),
                    height: StaticHeight::Fixed(TargetHeight(2)),
                    hue: Hue::NONE,
                }),
                Brush::default(),
                off_map,
            ),
            Err(PatchError::OffMap { x: 8, y: 0 })
        );
        assert!(matches!(
            gesture.apply(
                Tool::RemoveStatic(StaticId(0)),
                Brush::default(),
                TilePoint::new(0, 0),
            ),
            Err(PatchError::NoSuchStatic { standing: 0, .. })
        ));
    }
}
