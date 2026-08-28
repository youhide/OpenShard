//! `multi.mul`: the houses, the ships and the boats, as lists of tiles.
//!
//! A **multi** is one item that draws as many. The wire carries a house as an
//! ordinary world item whose graphic is `0x4000 + id`, and the client looks that
//! id up in its own copy of this file and draws the hundred and forty-eight
//! statics a two-storey villa is made of. The shard never sends any of them.
//!
//! Which is why the shard has to read the same file: the client draws the walls,
//! but only the shard decides whether you may walk through one. The components
//! are the footprint, the blocking and the roof, and none of that is derivable
//! from an item id alone.
//!
//! # The format changed and the file does not say so
//!
//! High Seas widened the per-component flags from four bytes to eight, exactly
//! as it widened [`tiledata`](crate::tiledata)'s — and here too there is no
//! magic and no version. The detection is the same arithmetic: an entry is
//! either 12 bytes or 16, so a run of lengths that divides by 16 and *not* by 12
//! cannot be the old layout. See [`MultiFormat::detect`], which is the honest
//! version of ServUO's `MultiData.PostHSFormat` — a static somebody has to
//! remember to set.
//!
//! # Two files hold the same thing, and one of them is not always there
//!
//! Modern installs ship `MultiCollection.uop` beside `multi.mul`, and the one
//! this crate prefers is the **UOP**, for [`uop`](crate::uop)'s reason: the
//! `.mul` is what an old tool reads, and there is no guarantee the client updated
//! both. Unlike `map0.mul` the stale one here is not zeroed — it is simply older
//! — which is a worse failure than a blank file, because a house from an
//! outdated `multi.mul` is a house whose walls are in the wrong place and whose
//! picture, drawn from the UOP the *client* read, is somewhere else.
//!
//! It is not a hypothetical. On the install these readers were built against the
//! `.mul` holds **326** multis and the UOP holds **862**, and a couple of dozen
//! of the shared ones differ by a component.
//!
//! [`Multis::load`] takes the directory and decides. Both readers are here
//! because both exist in the wild: a 2D-client install from before the UOP has
//! only the `.mul`.
//!
//! # Which components are real
//!
//! Not all of them. A multi's list holds tiles the client never draws, and the
//! flag that says so reads backwards from what the name suggests: **`0` means
//! skip it**, and the `Background` bit — value `1` — is on every tile that is
//! actually part of the house. Both references agree (ServUO's `i == 0 ||
//! m_Flags != 0`, ClassicUO's `if (flags == 0) continue`), and in the shipped
//! file the split is 57,784 real components against 2,030 skipped ones.
//!
//! **And the UOP writes it the other way round.** There it is a small enum, not
//! a flag word: `0` is drawn, `1` is skipped, `257` is generic — the inverse of
//! the `.mul`'s sense, with nothing in either file to say so. [`read_uop_multi`]
//! folds it onto the `.mul`'s convention so nothing downstream has to know which
//! file it read. Getting this backwards is invisible from one side: both readers
//! look right, and the two disagree about 309 of the 326 multis they share, with
//! the same graphics at the same offsets and every flag inverted.
//!
//! Entry **zero** is kept whatever its flag, because it is not really a tile: it
//! is a signature, usually item id `1` at the origin, and ServUO counts it in the
//! bounds. Counting it here too is what keeps [`Multi::center`] identical to the
//! reference's — and the centre is what a placement offset is measured from, so
//! a shard that computed it differently would put every house one tile off the
//! spot the player clicked.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use openshard_protocol::wire::{Graphic, MultiId};
use openshard_protocol::world::Point;

/// How many multi ids a client's index can name.
///
/// The index is a flat table and most of it is empty: the shipped file has 8,704
/// slots and 326 multis in them.
pub const MAX_MULTI_ID: u16 = 0x3FFF;

/// Whether `graphic` is the closed half of a standard UO door pair.
///
/// A custom house sends its ordinary structure as `HouseDesign` but places
/// doors as live entities.  Leaving a closed door in both representations
/// makes the entity open while the static design leaf remains shut and blocks
/// the doorway.  The families are the ones present in the client art used by
/// the legacy house packs; each has eight closed/open pairs in facing order.
#[must_use]
pub fn is_closed_door_graphic(graphic: Graphic) -> bool {
    [0x0675, 0x06A5, 0x06BD, 0x06D5, 0x06E5, 0x0839, 0x0866]
        .into_iter()
        .any(|base| (base..=base + 14).contains(&graphic.0) && (graphic.0 - base).is_multiple_of(2))
}

/// Whether `graphic` is one of the wooden-banister pieces used by house packs.
///
/// Their client tiledata marks them blocking, which is appropriate for a loose
/// railing but wrong for the decorative rail lines built into these house
/// designs: it seals the floor directly underneath. Housing keeps the artwork
/// but leaves its collision to the floor and walls around it.
#[must_use]
pub const fn is_house_banister_graphic(graphic: Graphic) -> bool {
    graphic.0 >= 0x08B6 && graphic.0 <= 0x08CA
}

/// Whether `graphic` is the visible art of a functional house sign.
///
/// A content multi may contain this decal, while the server also creates a
/// live `HouseSign` entity for naming and interaction. Custom-house rendering
/// must omit the component in that case, or the two signs are drawn on top of
/// each other. `0x0B9E` is the legacy pack's `metal signpost`: it is the
/// template's actual house-menu attachment, even though the client tile name
/// does not call it a house sign.
#[must_use]
pub const fn is_house_sign_graphic(graphic: Graphic) -> bool {
    matches!(graphic.0, 0x0B9E | 0x0BD1 | 0x0BD2)
}

/// Bytes per `multi.idx` entry: lookup, length, extra.
const INDEX_ENTRY: usize = 12;
/// A component before High Seas: id, three offsets, `u32` flags.
const COMPONENT_OLD: usize = 12;
/// A component since High Seas: the same, with `u64` flags.
const COMPONENT_NEW: usize = 16;

/// `TileFlag.Background`, the bit that marks a component the client draws. The
/// name is the reference's and reads backwards from what it does — see the
/// module docs.
const TILE_BACKGROUND: u64 = 0x0000_0001;
/// `TileFlag.Generic`, the third value the UOP's enum can carry.
const TILE_GENERIC: u64 = 0x0000_0800;

/// The UOP entry name a multi's components live under. ServUO's
/// `GetHashFormat`, and the only string in the container that names an id.
const UOP_ENTRY: &str = "build/multicollection/{:06}.bin";

/// Which layout `multi.mul` is in. [`crate::tiledata::TileDataFormat`]'s twin,
/// and detected the same way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MultiFormat {
    /// Clients before High Seas: 4-byte flags, 12 bytes to a component.
    Legacy,
    /// Clients since High Seas: 8-byte flags, 16 bytes to a component.
    HighSeas,
}

impl MultiFormat {
    /// Bytes per component.
    const fn component(self) -> usize {
        match self {
            Self::Legacy => COMPONENT_OLD,
            Self::HighSeas => COMPONENT_NEW,
        }
    }

    /// Work out which layout a set of index lengths can be.
    ///
    /// A length is a whole number of components, so a length that does not divide
    /// by 12 rules the old layout out and one that does not divide by 16 rules the
    /// new one out. Across the shipped file's 326 multis that is decisive: every
    /// one divides by 16 and only 115 divide by 12.
    ///
    /// Ambiguity is possible in principle — a file whose every multi happens to
    /// hold a multiple of four components divides by both — and it resolves to
    /// [`HighSeas`](Self::HighSeas), because a modern install is the ordinary case
    /// and the ambiguous file is the one where the choice costs nothing to get
    /// wrong in one direction and everything in the other: reading 16-byte data as
    /// 12-byte shifts every component after the first.
    #[must_use]
    pub fn detect(lengths: impl IntoIterator<Item = u32>) -> Self {
        let mut old = true;
        let mut new = true;
        for length in lengths {
            old &= (length as usize).is_multiple_of(COMPONENT_OLD);
            new &= (length as usize).is_multiple_of(COMPONENT_NEW);
        }
        if new || !old { Self::HighSeas } else { Self::Legacy }
    }
}

/// One tile of a multi, at its offset from the multi's own origin.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Component {
    /// The static's art id — what [`tiledata`](crate::tiledata) is asked about
    /// for its height and whether it blocks.
    pub graphic: Graphic,
    /// East, from the multi's origin.
    pub dx: i16,
    /// South, from the multi's origin.
    pub dy: i16,
    /// Up, from the ground the multi stands on.
    pub dz: i16,
    /// The raw flag word, widened to 64 bits whichever layout it was read from.
    ///
    /// Kept whole rather than reduced to a `bool`, because the bits above the
    /// first are High Seas' own and nothing here yet knows what they mean —
    /// throwing them away would make that unknowable from a save.
    pub flags: u64,
}

impl Component {
    /// Whether the client draws this one. See the module docs for why zero is the
    /// *skip* value.
    #[must_use]
    pub const fn drawn(self) -> bool {
        self.flags != 0
    }

    /// Where this component stands when the multi is placed at `origin`.
    ///
    /// **The one arithmetic**, and it is here because a multi is expanded in
    /// three places that are not in one crate: the shard's footprint, the
    /// shard's tile list, and the client's picture. Each had its own copy, and
    /// the copies did not agree about the edge of the world — the shard refused
    /// the placement, the client *wrapped* the offset, so a house built near
    /// x = 0 had a wall drawn on the far side of Britannia. See
    /// `docs/map/realtime_map.md`'s R3.
    ///
    /// `None` where the result does not fit the world's own coordinates: off
    /// the map east or west, or a `dz` that leaves the `i8` a z is. Refusing is
    /// the only honest answer — a saturated z draws a roof at the height of a
    /// floor, and a wrapped x draws it in another town.
    ///
    /// `origin` is the multi's **origin** and not the corner of its box; see
    /// [`Multi::center`].
    #[must_use]
    pub fn placed_at(self, origin: Point) -> Option<Point> {
        let x = u16::try_from(i32::from(origin.x) + i32::from(self.dx)).ok()?;
        let y = u16::try_from(i32::from(origin.y) + i32::from(self.dy)).ok()?;
        let z = i8::try_from(i32::from(origin.z) + i32::from(self.dz)).ok()?;
        Some(Point::new(x, y, z))
    }
}

/// One multi: a house, a ship, a boat's hold.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Multi {
    /// Its id, `0x4000` below the graphic the wire carries.
    pub id: u16,
    /// Every component, in file order, the undrawn ones included.
    ///
    /// The whole list rather than the drawn subset: what a *renderer* skips and
    /// what a *footprint* covers are two questions, and a reader that answered
    /// only the first would have thrown the second away.
    pub components: Vec<Component>,
    /// Where the multi's own origin sits inside its bounding box, in tiles east
    /// and south of the box's north-west corner.
    ///
    /// ServUO's `m_Center`, and the number a placement is measured from.
    pub center: (i16, i16),
    /// The bounding box, in tiles.
    pub size: (u16, u16),
}

impl Multi {
    /// Every component the client actually draws.
    pub fn drawn(&self) -> impl Iterator<Item = &Component> {
        self.components.iter().filter(|component| component.drawn())
    }

    /// Build one from a component list, deriving the box the way ServUO does.
    ///
    /// The bounds come from entry zero **and** every drawn component, which is
    /// the reference's own `i == 0 || m_Flags != 0`. Matching it exactly is what
    /// keeps [`center`](Self::center) the same number on both engines.
    #[must_use]
    pub fn new(id: u16, components: Vec<Component>) -> Self {
        let Some(box_) = bounds(&components) else {
            return Self {
                id,
                components,
                center: (0, 0),
                size: (0, 0),
            };
        };
        Self {
            id,
            components,
            center: (-box_.min_x, -box_.min_y),
            size: (
                (box_.max_x - box_.min_x).unsigned_abs() + 1,
                (box_.max_y - box_.min_y).unsigned_abs() + 1,
            ),
        }
    }
}

/// The corners of a multi's bounding box, in tiles from its origin.
///
/// Signed and origin-relative, which is what [`Multi::center`] and
/// [`Multi::size`] are each derived from — and what a caller that wants a
/// *corner* rather than a size needs, because those two numbers cannot be turned
/// back into one without redoing the same arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    /// The westmost component's offset.
    pub min_x: i16,
    /// The northmost.
    pub min_y: i16,
    /// The eastmost.
    pub max_x: i16,
    /// The southmost.
    pub max_y: i16,
}

/// The box a component list occupies, or `None` if the list is empty.
///
/// Entry zero **and** every drawn component, which is the reference's own `i ==
/// 0 || m_Flags != 0`. Public and separate from [`Multi::new`] so that a caller
/// deriving a position from the box — where a house's sign stands, which ServUO
/// computes as `(Min.X, Height - 1 - Center.Y)` — asks the same function the
/// centre was computed by, rather than a second copy of it that can drift.
#[must_use]
pub fn bounds(components: &[Component]) -> Option<Bounds> {
    let (mut min_x, mut min_y) = (i16::MAX, i16::MAX);
    let (mut max_x, mut max_y) = (i16::MIN, i16::MIN);
    for (nth, component) in components.iter().enumerate() {
        if nth != 0 && !component.drawn() {
            continue;
        }
        min_x = min_x.min(component.dx);
        min_y = min_y.min(component.dy);
        max_x = max_x.max(component.dx);
        max_y = max_y.max(component.dy);
    }
    if components.is_empty() {
        return None;
    }
    Some(Bounds {
        min_x,
        min_y,
        max_x,
        max_y,
    })
}

/// Every multi a client knows about.
#[derive(Clone, Default, Debug)]
pub struct Multis {
    multis: BTreeMap<u16, Multi>,
    format: Option<MultiFormat>,
}

impl Multis {
    /// Build a table from multis already in hand.
    ///
    /// For a caller that has components without a file behind them — a test, or
    /// an editor that has just drawn one. The readers do not use it; they insert
    /// as they go, because they know the id before they know the components.
    #[must_use]
    pub fn of(multis: impl IntoIterator<Item = Multi>) -> Self {
        Self {
            multis: multis.into_iter().map(|multi| (multi.id, multi)).collect(),
            format: None,
        }
    }

    /// Read a client's multis out of `dir`, preferring `MultiCollection.uop`.
    ///
    /// See the module docs for why the UOP wins where both exist. An install with
    /// neither is an error rather than an empty table: a shard that silently knew
    /// about no houses would place one and find it had no walls.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, MultiError> {
        let dir = dir.as_ref();
        let uop = dir.join("MultiCollection.uop");
        if uop.exists() {
            return Self::load_uop(uop);
        }
        Self::load_mul(dir.join("multi.idx"), dir.join("multi.mul"))
    }

    /// Read the pair of `.mul` files directly.
    pub fn load_mul(index: impl AsRef<Path>, data: impl AsRef<Path>) -> Result<Self, MultiError> {
        let index_path = index.as_ref().to_path_buf();
        let data_path = data.as_ref().to_path_buf();
        let index_bytes = std::fs::read(&index_path).map_err(|source| MultiError::Read {
            path: index_path.clone(),
            source,
        })?;
        let data_bytes = std::fs::read(&data_path).map_err(|source| MultiError::Read {
            path: data_path.clone(),
            source,
        })?;

        // Two passes over the index: the first only to settle the layout, because
        // a component's width has to be known before a single one can be read.
        let entries: Vec<(u16, usize, usize)> = (0..index_bytes.len() / INDEX_ENTRY)
            .filter_map(|nth| {
                let at = nth * INDEX_ENTRY;
                let lookup = i32::from_le_bytes(index_bytes[at..at + 4].try_into().ok()?);
                let length = i32::from_le_bytes(index_bytes[at + 4..at + 8].try_into().ok()?);
                let id = u16::try_from(nth).ok()?;
                (lookup >= 0 && length > 0).then_some((
                    id,
                    lookup.unsigned_abs() as usize,
                    length.unsigned_abs() as usize,
                ))
            })
            .collect();
        let format = MultiFormat::detect(entries.iter().map(|&(_, _, length)| length as u32));

        let mut multis = BTreeMap::new();
        for (id, lookup, length) in entries {
            let end = lookup.saturating_add(length);
            if end > data_bytes.len() {
                return Err(MultiError::Truncated {
                    path: data_path,
                    id,
                    wanted: end,
                    had: data_bytes.len(),
                });
            }
            let components = (0..length / format.component())
                .map(|nth| read_component(&data_bytes[lookup + nth * format.component()..], format))
                .collect();
            multis.insert(id, Multi::new(id, components));
        }
        Ok(Self {
            multis,
            format: Some(format),
        })
    }

    /// Read `MultiCollection.uop`.
    ///
    /// A different shape for the same data, and the difference worth naming is the
    /// flags: the UOP writes a **small enum** rather than the tile flag word — `0`
    /// for a skipped tile, `1` for a drawn one, `257` for a generic — which is the
    /// inverse of the `.mul`'s convention and the reason both are folded onto
    /// [`Component::flags`] here rather than carried as they were written. What
    /// comes out of either reader means the same thing.
    pub fn load_uop(path: impl AsRef<Path>) -> Result<Self, MultiError> {
        let path = path.as_ref().to_path_buf();
        let uop = crate::uop::Uop::open(&path).map_err(|source| MultiError::Container {
            path: path.clone(),
            source,
        })?;
        let mut multis = BTreeMap::new();
        for id in 0..=MAX_MULTI_ID {
            let name = UOP_ENTRY.replace("{:06}", &format!("{id:06}"));
            let Some(raw) = uop.raw_entry(&name).map_err(|source| MultiError::Container {
                path: path.clone(),
                source,
            })?
            else {
                continue;
            };
            // `raw_entry` rather than `entry`, because every entry in this
            // container is zlib and `entry` refuses those by design — see its
            // docs, and `gumpart`, which is the other caller that has to invert
            // its own compression. Plain zlib here: no second pass, unlike the
            // gump container's flag 3.
            let inflated;
            let bytes = if raw.compression == crate::uop::UopCompression::STORED {
                raw.bytes
            } else {
                inflated = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
                    raw.bytes,
                    raw.decompressed_length,
                )
                .map_err(|error| MultiError::Malformed {
                    path: path.clone(),
                    detail: format!(
                        "multi {id:#06x} did not inflate ({:?} after {} bytes)",
                        error.status,
                        error.output.len()
                    ),
                })?;
                &inflated[..]
            };
            let components = read_uop_multi(bytes).ok_or_else(|| MultiError::Malformed {
                path: path.clone(),
                detail: format!("multi {id:#06x} ends mid-component"),
            })?;
            multis.insert(id, Multi::new(id, components));
        }
        Ok(Self { multis, format: None })
    }

    /// One multi, by id. The graphic on the wire is `0x4000` above this.
    #[must_use]
    pub fn get(&self, id: u16) -> Option<&Multi> {
        self.multis.get(&id)
    }

    /// What multi `id` is made of — the tiles a house draws as, at their offsets
    /// from its own origin. Empty for an id no client knows.
    ///
    /// The whole list, undrawn components included: what a *renderer* skips and
    /// what a *footprint* covers are two questions. See [`Component::drawn`].
    #[must_use]
    pub fn components(&self, id: u16) -> &[Component] {
        self.get(id).map_or(&[], |multi| &multi.components)
    }

    /// The multi a world item's graphic names, or `None` if the graphic is not a
    /// multi at all.
    ///
    /// The mask is ServUO's `multiID &= 0x3FFF`, and the reason it is a *mask*
    /// rather than a subtraction: a house arrives as `0x4000 | id` and a client
    /// that has been told about one already holds the id, so both spellings reach
    /// the same row.
    #[must_use]
    pub fn for_graphic(&self, graphic: Graphic) -> Option<&Multi> {
        let id = MultiId::from_graphic(graphic);
        self.get(id.0 & MAX_MULTI_ID)
    }

    /// Every multi, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Multi> {
        self.multis.values()
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.multis.len()
    }

    /// Whether the client knows about none, which is a broken install rather than
    /// a shard with no houses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.multis.is_empty()
    }

    /// Which `.mul` layout this was read as, or `None` when it came from a UOP —
    /// where the question does not arise.
    #[must_use]
    pub const fn format(&self) -> Option<MultiFormat> {
        self.format
    }
}

/// One component out of the `.mul`.
fn read_component(bytes: &[u8], format: MultiFormat) -> Component {
    let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let flags = match format {
        MultiFormat::Legacy => u64::from(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]])),
        MultiFormat::HighSeas => u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]),
    };
    Component {
        graphic: Graphic(word(0)),
        dx: word(2) as i16,
        dy: word(4) as i16,
        dz: word(6) as i16,
        flags,
    }
}

/// One multi out of a decompressed UOP entry, or `None` if it is truncated.
///
/// The per-component cliloc block is skipped rather than read: it is the
/// customisation system's tooltip text, and nothing about a footprint needs it.
fn read_uop_multi(bytes: &[u8]) -> Option<Vec<Component>> {
    // A leading word nobody has a name for, then the count.
    let count = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let mut at = 8;
    let mut components = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let entry = bytes.get(at..at + 14)?;
        let word = |offset: usize| u16::from_le_bytes([entry[offset], entry[offset + 1]]);
        // The UOP writes a small enum where the `.mul` writes a tile-flag word,
        // and the two run **opposite ways**: `0` here is the `.mul`'s
        // `Background` (`1`, the drawn tile) and `1` here is the `.mul`'s `None`
        // (`0`, the skipped one). ServUO's own switch says so, and the shipped
        // files confirm it component for component — the two disagreed on 309 of
        // 326 multis until this was turned around, with the *same* graphics at
        // the *same* offsets in the same order and every flag inverted. Folded
        // onto the `.mul`'s convention here so no caller downstream has to know
        // which file it came from.
        let flags = match word(8) {
            0 => TILE_BACKGROUND,
            1 => 0,
            _ => TILE_GENERIC,
        };
        components.push(Component {
            graphic: Graphic(word(0)),
            dx: word(2) as i16,
            dy: word(4) as i16,
            dz: word(6) as i16,
            flags,
        });
        let clilocs = u32::from_le_bytes(entry[10..14].try_into().ok()?) as usize;
        at += 14 + clilocs * 4;
    }
    Some(components)
}

/// A multi table could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum MultiError {
    /// A file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The UOP container would not open.
    Container {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: crate::uop::UopError,
    },
    /// An entry could not be made sense of.
    Malformed {
        /// Which file.
        path: PathBuf,
        /// What was wrong.
        detail: String,
    },
    /// The index points past the end of the data file.
    Truncated {
        /// Which file.
        path: PathBuf,
        /// Whose entry.
        id: u16,
        /// Where the index said the multi ended.
        wanted: usize,
        /// How long the file actually is.
        had: usize,
    },
}

impl fmt::Display for MultiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "could not read {}: {source}", path.display()),
            Self::Container { path, source } => {
                write!(f, "could not open {}: {source}", path.display())
            }
            Self::Malformed { path, detail } => write!(f, "{}: {detail}", path.display()),
            Self::Truncated {
                path,
                id,
                wanted,
                had,
            } => write!(
                f,
                "{}: multi {id:#06x} ends at {wanted} but the file is {had} bytes",
                path.display()
            ),
        }
    }
}

impl std::error::Error for MultiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Container { source, .. } => Some(source),
            Self::Malformed { .. } | Self::Truncated { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped file's own numbers, and what makes them decisive: every live
    /// multi divides by 16 and most do not divide by 12, so the arithmetic has an
    /// answer rather than a preference.
    #[test]
    fn a_length_that_only_divides_by_sixteen_settles_the_layout() {
        assert_eq!(MultiFormat::detect([608, 2368, 400]), MultiFormat::HighSeas);
        assert_eq!(MultiFormat::detect([12, 36, 60]), MultiFormat::Legacy);
        // Divisible by both, which is the case the doc says resolves to the
        // modern layout rather than to an error.
        assert_eq!(MultiFormat::detect([48, 96]), MultiFormat::HighSeas);
        // And an empty install is not evidence for the old one.
        assert_eq!(MultiFormat::detect([]), MultiFormat::HighSeas);
    }

    fn component(graphic: u16, dx: i16, dy: i16, flags: u64) -> Component {
        Component {
            graphic: Graphic(graphic),
            dx,
            dy,
            dz: 0,
            flags,
        }
    }

    /// Entry zero counts toward the box and an undrawn tile does not — ServUO's
    /// own rule, and the one that keeps a centre the same number on both engines.
    #[test]
    fn the_box_is_the_signature_tile_and_the_drawn_ones() {
        let multi = Multi::new(
            0x64,
            vec![
                component(1, 0, 0, 0),
                component(0x04B0, -2, -1, 1),
                component(0x04B0, 2, 3, 1),
                // Undrawn, and well outside the others: it must not widen the box.
                component(0x06A5, 40, 40, 0),
            ],
        );
        assert_eq!(multi.center, (2, 1));
        assert_eq!(multi.size, (5, 5));
        assert_eq!(multi.drawn().count(), 2, "the skipped tile was drawn");
        assert_eq!(
            multi.components.len(),
            4,
            "the skipped tile was thrown away rather than marked"
        );
    }

    /// A multi with nothing in it answers rather than panicking on the empty
    /// min/max — an index entry can name a length of zero.
    #[test]
    fn an_empty_multi_has_no_box() {
        let multi = Multi::new(7, Vec::new());
        assert_eq!(multi.size, (0, 0));
        assert_eq!(multi.center, (0, 0));
    }

    /// The wire carries `0x4000 | id`, and a caller holding either spelling
    /// reaches the same row.
    #[test]
    fn a_graphic_and_an_id_name_the_same_multi() {
        let mut multis = Multis::default();
        multis.multis.insert(0x64, Multi::new(0x64, Vec::new()));
        assert!(multis.for_graphic(Graphic(0x4064)).is_some());
        assert!(multis.for_graphic(Graphic(0x0064)).is_some());
        assert!(multis.for_graphic(Graphic(0x4065)).is_none());
    }

    /// The UOP's flag enum folds onto the `.mul`'s convention, so nothing
    /// downstream can tell which file a component came from.
    #[test]
    fn a_uop_entry_reads_as_the_mul_would() {
        let mut bytes = vec![0u8; 8];
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        // A drawn tile with one cliloc after it, then a skipped one with none —
        // and the flags are the *UOP's* way round, which is the inverse of the
        // `.mul`'s: `0` draws, `1` skips.
        for (graphic, dx, flag, clilocs) in [(0x04B0u16, 1i16, 0u16, 1u32), (0x06A5, 2, 1, 0)] {
            bytes.extend_from_slice(&graphic.to_le_bytes());
            bytes.extend_from_slice(&dx.to_le_bytes());
            bytes.extend_from_slice(&0i16.to_le_bytes());
            bytes.extend_from_slice(&7i16.to_le_bytes());
            bytes.extend_from_slice(&flag.to_le_bytes());
            bytes.extend_from_slice(&clilocs.to_le_bytes());
            bytes.extend(std::iter::repeat_n(0u8, clilocs as usize * 4));
        }
        let components = read_uop_multi(&bytes).expect("a whole entry");
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].graphic, Graphic(0x04B0));
        assert_eq!(components[0].dz, 7);
        assert!(components[0].drawn());
        assert!(
            !components[1].drawn(),
            "the UOP's one is the mul's zero, and it is the skip"
        );
        assert_eq!(
            components[1].graphic,
            Graphic(0x06A5),
            "the cliloc block after the first entry was not skipped"
        );
    }

    /// A component off the edge of the world is refused, not wrapped.
    ///
    /// The three expansions of a multi disagreed about exactly this: the shard
    /// refused, the client wrapped, so a house built at x = 1 had a wall the
    /// shard never placed drawn at x = 65535 — a whole map away. And a `dz`
    /// past what an `i8` holds was clamped rather than dropped, which draws a
    /// roof at the height of a floor.
    #[test]
    fn a_component_off_the_map_is_refused_rather_than_wrapped() {
        let west = Component {
            graphic: Graphic(0x0006),
            dx: -3,
            dy: 0,
            dz: 0,
            flags: 1,
        };
        assert_eq!(west.placed_at(Point::new(10, 10, 0)), Some(Point::new(7, 10, 0)));
        assert_eq!(west.placed_at(Point::new(1, 10, 0)), None, "it wrapped east");

        let high = Component {
            graphic: Graphic(0x0006),
            dx: 0,
            dy: 0,
            dz: 100,
            flags: 1,
        };
        assert_eq!(high.placed_at(Point::new(10, 10, 100)), None, "the z was clamped");
    }

    /// A truncated entry answers `None` rather than reading past its end.
    #[test]
    fn a_short_uop_entry_is_refused() {
        let mut bytes = vec![0u8; 8];
        bytes[4..8].copy_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 14]);
        assert!(read_uop_multi(&bytes).is_none());
    }
}
