//! A building somebody built, laid onto a facet the way a client lays one.
//!
//! # Why a loader and not a constant
//!
//! [`scene`](crate::scene) builds geometry a test chose: a floor here, a stair
//! there, a wall across. That is the right fixture for a rule. It is the wrong
//! fixture for the question *what does a house do to a route*, because the
//! houses players build are two thousand components with one door, an interior
//! courtyard and a roof reachable only by the stairs their owner drew — and
//! nothing anybody writes by hand has that shape.
//!
//! So this reads the shape out of a file: the `house_designs` rows of a real
//! shard, exported once, as `dx,dy,dz,graphic,flags` per component. What it
//! produces is an [`Overlay`] — the same live layer a client fills from a
//! multi it is shown — so a search over it is the search a player's click gets
//! and not an approximation of one.
//!
//! # What the flags column decides
//!
//! `0` is a component the house does not draw, and a component nobody draws is
//! in nobody's way either: it is skipped, exactly as `client/app`'s
//! `clutter::fill` skips it. Everything else becomes the covers its art
//! carries, based at the height the component stands at, and a leaf marked
//! `DOOR` in the tiledata is laid **as a door** — because a client plans its
//! route through a shut door it is going to open, and a design whose door was
//! laid as a wall would refuse every route into the building.

use std::collections::HashMap;

use openshard_map::grid::Tile;
use openshard_map::overlay::{
    Cover,
    Overlay,
};
use openshard_protocol::world::Point;

/// One component of a design: where it stands relative to the origin, what art
/// it is, and whether the house draws it.
#[derive(Clone, Copy, Debug)]
pub struct Component {
    /// Tiles east of the design's origin. Signed: a design is drawn around its
    /// origin rather than out of it.
    pub dx:      i32,
    /// Tiles south of the design's origin.
    pub dy:      i32,
    /// Height above the design's origin.
    pub dz:      i32,
    /// The static art this component is.
    pub graphic: u16,
    /// What the shard records about the component. Zero is a component the
    /// house does not draw — see the module docs.
    pub flags:   u64,
}

/// Why a design file could not be read as one.
#[derive(Debug)]
pub enum DesignError {
    /// A row that is not five comma-separated fields.
    Fields { line: usize, fields: usize },
    /// A field that is not the number its column is.
    Number {
        line:   usize,
        column: &'static str,
        text:   String,
    },
}

impl std::fmt::Display for DesignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fields { line, fields } => {
                write!(f, "line {line} has {fields} fields where a design row has five")
            }
            Self::Number { line, column, text } => {
                write!(f, "line {line}: {column} is not a number: {text}")
            }
        }
    }
}

impl std::error::Error for DesignError {
}

/// One field of a row, as the number its column is.
///
/// The column's own type and not a wide one narrowed afterwards: a graphic that
/// does not fit `u16` is a file that is not this format, and `as` would make it
/// a different building instead of an error.
fn field<T: std::str::FromStr>(text: &str, column: &'static str, line: usize) -> Result<T, DesignError> {
    text.parse::<T>().map_err(|_| {
        DesignError::Number {
            line,
            column,
            text: text.to_owned(),
        }
    })
}

/// A house as its owner drew it, in components.
#[derive(Clone, Debug)]
pub struct Design {
    components: Vec<Component>,
}

impl Design {
    /// Read a design out of `dx,dy,dz,graphic,flags` rows, one per line.
    ///
    /// Blank lines are skipped. Nothing else is: a row this cannot read is a
    /// file that is not the export it claims to be, and laying half a castle
    /// would produce a scene that looks plausible and answers about nothing.
    ///
    /// # Errors
    ///
    /// [`DesignError`] naming the line, because the file is somebody's export
    /// and the useful sentence is which row of it went wrong.
    pub fn parse(text: &str) -> Result<Self, DesignError> {
        let mut components = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let number = index + 1;
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let [dx, dy, dz, graphic, flags] = fields.as_slice() else {
                return Err(DesignError::Fields {
                    line:   number,
                    fields: fields.len(),
                });
            };
            components.push(Component {
                dx:      field(dx, "dx", number)?,
                dy:      field(dy, "dy", number)?,
                dz:      field(dz, "dz", number)?,
                graphic: field(graphic, "graphic", number)?,
                flags:   field(flags, "flags", number)?,
            });
        }
        Ok(Self { components })
    }

    /// Every component, drawn or not.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The highest drawn component of every tile this design covers — the point
    /// a click on the building lands on.
    ///
    /// **A click and not a place to stand.** What a cursor hits is art, at the
    /// height the art is drawn at; turning that into somewhere a body can stand
    /// is [`destination_place`](crate::destination_place)'s job and depends on
    /// the world the click was made in. So this hands back what the click
    /// carries, and the caller resolves it the way the client does.
    ///
    /// Ordered north to south and west to east, so a caller sampling every
    /// `n`-th of them walks the building rather than one corner of it.
    #[must_use]
    pub fn tops(&self, origin: Point) -> Vec<Point> {
        let mut highest: HashMap<Tile, i8> = HashMap::new();
        for component in self.components.iter().filter(|component| component.flags != 0) {
            let Some((tile, z)) = at(component, origin) else {
                continue;
            };
            highest
                .entry(tile)
                .and_modify(|standing| *standing = (*standing).max(z))
                .or_insert(z);
        }
        let mut tops: Vec<Point> = highest
            .into_iter()
            .map(|(tile, z)| Point::new(tile.x, tile.y, z))
            .collect();
        tops.sort_unstable_by_key(|point| (point.y, point.x));
        tops
    }

    /// The tiles this design covers when its origin is laid at `origin`.
    ///
    /// The drawn components only, which is the footprint a body meets. Tiles
    /// off the facet's coordinate space are dropped the same way [`Design::lay`]
    /// drops them, so the two always agree about where the building is.
    #[must_use]
    pub fn footprint(&self, origin: Point) -> Vec<Tile> {
        self.tops(origin)
            .into_iter()
            .map(|point| Tile::new(point.x, point.y))
            .collect()
    }

    /// Lay this design into `overlay`, with its origin at `origin`.
    ///
    /// **Every component of one tile goes in together**, because
    /// [`Overlay::set`] replaces a tile's covers rather than adding to them: a
    /// floor written after the wall above it would leave the wall out of the
    /// world. Tiles this writes are the design's own — whatever `overlay` held
    /// elsewhere is untouched.
    pub fn lay(&self, overlay: &mut Overlay, tiles: &openshard_tiles::TileData, origin: Point) {
        let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
        for component in &self.components {
            // The same skip the client makes: a component the house does not
            // draw is not in anybody's way either. See `multi::Component::drawn`.
            if component.flags == 0 {
                continue;
            }
            let Some((tile, z)) = at(component, origin) else {
                continue;
            };
            let art = tiles.static_tile(component.graphic);
            let laid = Cover::of_static(art).based_at(z);
            // A leaf is marked as one, because a client plans its own route
            // through a shut door it is going to open — `Doors::AllOpen`. The
            // tiledata flag rather than `client/render`'s open/shut table:
            // which of a pair a graphic is does not matter to a step that opens
            // either.
            let laid = match art.flags.has(openshard_tiles::TileFlags::DOOR) {
                true => laid.as_door(),
                false => laid,
            };
            covers.entry(tile).or_default().extend(laid);
        }
        for (tile, covers) in covers {
            overlay.set(tile, covers);
        }
    }
}

/// Where a component stands once the design is laid at `origin`, or `None`
/// where that is off the map or off `Point`'s height.
fn at(component: &Component, origin: Point) -> Option<(Tile, i8)> {
    let x = u16::try_from(i32::from(origin.x) + component.dx).ok()?;
    let y = u16::try_from(i32::from(origin.y) + component.dy).ok()?;
    let z = i8::try_from(i32::from(origin.z) + component.dz).ok()?;
    Some((Tile::new(x, y), z))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file format, in the three ways it is read: a drawn component, an
    /// undrawn one, and a blank line.
    #[test]
    fn a_design_is_its_drawn_components() {
        let design = Design::parse("0,0,0,100,1\n\n1,0,7,101,1\n2,0,0,102,0\n").expect("three rows");
        assert_eq!(design.components().len(), 3, "an undrawn row is still a row");
        let origin = Point::new(1_000, 1_000, 0);
        assert_eq!(
            design.footprint(origin),
            vec![Tile::new(1_000, 1_000), Tile::new(1_001, 1_000)],
            "the footprint is what the house draws, and the undrawn tile is not in it"
        );
        assert_eq!(
            design.tops(origin),
            vec![Point::new(1_000, 1_000, 0), Point::new(1_001, 1_000, 7)],
            "a click lands on the highest thing the tile draws"
        );
    }

    /// A row that is not the format is a file that is not the export, and the
    /// error names the line rather than the value.
    #[test]
    fn a_row_that_is_not_five_numbers_is_refused_by_line() {
        assert!(matches!(
            Design::parse("0,0,0,100,1\n0,0,0,100\n"),
            Err(DesignError::Fields { line: 2, fields: 4 })
        ));
        assert!(matches!(
            Design::parse("0,0,0,roof,1\n"),
            Err(DesignError::Number {
                line: 1,
                column: "graphic",
                ..
            })
        ));
    }

    /// A design laid off the west edge keeps the components that are on the
    /// map: half a castle at the border is what the client draws too.
    #[test]
    fn components_off_the_map_are_dropped_and_the_rest_stand() {
        let design = Design::parse("-4,0,0,100,1\n0,0,0,100,1\n").expect("two rows");
        assert_eq!(
            design.footprint(Point::new(2, 2, 0)),
            vec![Tile::new(2, 2)],
            "the component four tiles west of x=2 has no tile to stand on"
        );
    }
}
