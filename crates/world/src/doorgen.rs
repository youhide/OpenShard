//! Generating a building's *functional* doors from the map's static art.
//!
//! A UO building's doorway is not a door item — it is a gap between two static
//! frame posts, with the door leaf baked into the map's static art (or left to
//! the server to add). A client can draw that art but cannot open it. So a shard
//! turns the implied door into a real one: scan the static frames, and where two
//! posts face each other across a one- or two-tile gap, drop a functional [`Door`]
//! into the gap.
//!
//! This is ServUO's `DoorGenerator`, ported: the same four frame-graphic tables
//! (a post is a door frame only if its graphic is in the table for its side), the
//! same single/double-door geometry, and the same `DarkWoodDoor` for every
//! generated door — a shop door, the wooden kind. The metal and special doors a
//! city needs are placed by name from the decoration data instead; this fills in
//! the plain ones the map only implies.
//!
//! Tables and geometry are the client's, ported verbatim from
//! `Scripts/Commands/DoorGenerator.cs`. The scan itself lives in the tick, which
//! is the only thing that can read the terrain and place entities; this module is
//! the constants and the predicates.
//!
//! [`Door`]: openshard_state::components::Door

/// The four facings a generated door can take, as `DoorFacing` indices into the
/// hinge-offset table — the only ones `DoorGenerator` ever produces.
#[derive(Clone, Copy)]
pub(crate) enum GenFacing {
    /// A single or left leaf of an east/west doorway.
    WestCw,
    /// The right leaf of a double east/west doorway.
    EastCcw,
    /// A single or right leaf of a north/south doorway.
    SouthCw,
    /// The left leaf of a double north/south doorway.
    NorthCcw,
}

/// The base graphic of a `DarkWoodDoor` — the closed WestCW leaf. Every other
/// leaf is `base + 2 * facing` closed, `+ 1` open, from ServUO's `DarkWoodDoor`.
const DARK_WOOD_BASE: u16 = 0x06A5;

impl GenFacing {
    /// This facing's `DoorFacing` index — its offset into ServUO's tables.
    fn index(self) -> u16 {
        match self {
            GenFacing::WestCw => 0,
            GenFacing::EastCcw => 1,
            GenFacing::SouthCw => 4,
            GenFacing::NorthCcw => 5,
        }
    }

    /// The closed graphic, open graphic, and hinge offset of a `DarkWoodDoor` at
    /// this facing — everything the [`Door`](openshard_state::components::Door)
    /// component needs.
    pub(crate) fn door(self) -> (u16, u16, i16, i16) {
        let closed = DARK_WOOD_BASE + 2 * self.index();
        let (ox, oy) = OFFSETS[self.index() as usize];
        (closed, closed + 1, ox, oy)
    }
}

/// The hinge offset per `DoorFacing`, from `BaseDoor.m_Offsets`. Opening a door
/// hops its leaf by this; closing hops it back.
const OFFSETS: [(i16, i16); 12] = [
    (-1, 1),
    (1, 1),
    (-1, 0),
    (1, -1),
    (1, 1),
    (1, -1),
    (0, 0),
    (0, -1),
    (0, 0),
    (0, 0),
    (0, 0),
    (0, 0),
];

/// Whether a static graphic is a west-side door frame.
pub(crate) fn is_west_frame(id: u16) -> bool {
    WEST_FRAMES.binary_search(&id).is_ok()
}

/// Whether a static graphic is an east-side door frame.
pub(crate) fn is_east_frame(id: u16) -> bool {
    EAST_FRAMES.binary_search(&id).is_ok()
}

/// Whether a static graphic is a north-side door frame.
pub(crate) fn is_north_frame(id: u16) -> bool {
    NORTH_FRAMES.binary_search(&id).is_ok()
}

/// Whether a static graphic is a south-side door frame.
pub(crate) fn is_south_frame(id: u16) -> bool {
    SOUTH_FRAMES.binary_search(&id).is_ok()
}

const WEST_FRAMES: &[u16] = &[
    0x0007, 0x000C, 0x001A, 0x001C, 0x0021, 0x0039, 0x0058, 0x0059, 0x005C, 0x005E, 0x0080, 0x0081,
    0x0082, 0x0084, 0x0090, 0x0092, 0x0095, 0x0097, 0x0098, 0x00A6, 0x00A8, 0x00AD, 0x00AE, 0x00AF,
    0x00B5, 0x00C7, 0x00C8, 0x00EA, 0x00F8, 0x00F9, 0x00FC, 0x00FE, 0x00FF, 0x0102, 0x0104, 0x0105,
    0x0108, 0x0127, 0x0128, 0x012C, 0x012E, 0x0130, 0x0132, 0x0133, 0x0135, 0x0136, 0x0138, 0x013A,
    0x014C, 0x014D, 0x014F, 0x0150, 0x0152, 0x0154, 0x0156, 0x0158, 0x0159, 0x015C, 0x015E, 0x0160,
    0x0163, 0x01CF, 0x01D0, 0x01D3, 0x01FF, 0x0200, 0x0203, 0x0207, 0x0209,
];

const EAST_FRAMES: &[u16] = &[
    0x0007, 0x000A, 0x001A, 0x001C, 0x001E, 0x0037, 0x0058, 0x0059, 0x005C, 0x005E, 0x0080, 0x0081,
    0x0082, 0x0084, 0x0090, 0x0092, 0x0095, 0x0097, 0x0098, 0x00A6, 0x00A8, 0x00AB, 0x00AE, 0x00AF,
    0x00B2, 0x00C7, 0x00C8, 0x00EA, 0x00F8, 0x00F9, 0x00FC, 0x00FE, 0x00FF, 0x0102, 0x0104, 0x0105,
    0x0108, 0x0127, 0x0128, 0x012B, 0x012C, 0x012E, 0x0130, 0x0132, 0x0133, 0x0135, 0x0136, 0x0138,
    0x013A, 0x014C, 0x014D, 0x014F, 0x0150, 0x0152, 0x0154, 0x0156, 0x0158, 0x0159, 0x015C, 0x015E,
    0x0160, 0x0163, 0x01CF, 0x01D0, 0x01D3, 0x01FF, 0x0203, 0x0205, 0x0207, 0x0209,
];

const NORTH_FRAMES: &[u16] = &[
    0x0006, 0x0008, 0x000D, 0x001A, 0x001B, 0x0020, 0x003A, 0x0057, 0x0059, 0x005B, 0x005D, 0x0080,
    0x0081, 0x0082, 0x0084, 0x0090, 0x0091, 0x0094, 0x0096, 0x0099, 0x00A6, 0x00A7, 0x00AC, 0x00AE,
    0x00B0, 0x00C7, 0x00C9, 0x00F8, 0x00FA, 0x00FD, 0x00FE, 0x0100, 0x0103, 0x0104, 0x0106, 0x0109,
    0x0127, 0x0129, 0x012B, 0x012D, 0x012F, 0x0131, 0x0132, 0x0134, 0x0135, 0x0137, 0x0139, 0x013B,
    0x014C, 0x014E, 0x014F, 0x0151, 0x0153, 0x0155, 0x0157, 0x0158, 0x015A, 0x015D, 0x015E, 0x015F,
    0x0162, 0x01CF, 0x01D1, 0x01D4, 0x01FF, 0x0201, 0x0204, 0x0208, 0x020A,
];

const SOUTH_FRAMES: &[u16] = &[
    0x0006, 0x0008, 0x000B, 0x001A, 0x001B, 0x001F, 0x0038, 0x0057, 0x0059, 0x005B, 0x005D, 0x0080,
    0x0081, 0x0082, 0x0084, 0x0090, 0x0091, 0x0094, 0x0096, 0x0099, 0x00A6, 0x00A7, 0x00AA, 0x00AE,
    0x00B0, 0x00B3, 0x00C7, 0x00C9, 0x00F8, 0x00FA, 0x00FD, 0x00FE, 0x0100, 0x0103, 0x0104, 0x0106,
    0x0109, 0x0127, 0x0129, 0x012B, 0x012D, 0x012F, 0x0131, 0x0132, 0x0134, 0x0135, 0x0137, 0x0139,
    0x013B, 0x014C, 0x014E, 0x014F, 0x0151, 0x0153, 0x0155, 0x0157, 0x0158, 0x015A, 0x015D, 0x015E,
    0x015F, 0x0162, 0x01CF, 0x01D1, 0x01D4, 0x01FF, 0x0204, 0x0206, 0x0208, 0x020A,
];
