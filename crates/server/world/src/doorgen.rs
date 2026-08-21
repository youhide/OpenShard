//! Generating a building's *functional* doors from the map's static art.
//!
//! A UO building's doorway is not a door item — it is a gap between two static
//! frame posts, with the door leaf baked into the map's static art (or left to
//! the server to add). A client can draw that art but cannot open it. So a shard
//! turns the implied door into a real one. The frame predicate lives in
//! `openshard_movement`: the offline client topology bake needs the exact same
//! ServUO tables to close those gaps without treating arbitrary walls as frames.

use openshard_protocol::wire::Graphic;

pub(crate) fn is_west_frame(id: Graphic) -> bool {
    openshard_movement::door_frames::is_west_frame(id.0)
}

pub(crate) fn is_east_frame(id: Graphic) -> bool {
    openshard_movement::door_frames::is_east_frame(id.0)
}

pub(crate) fn is_north_frame(id: Graphic) -> bool {
    openshard_movement::door_frames::is_north_frame(id.0)
}

pub(crate) fn is_south_frame(id: Graphic) -> bool {
    openshard_movement::door_frames::is_south_frame(id.0)
}

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

/// Index into the generated-door facing tables.
#[derive(Clone, Copy)]
struct DoorFacingIndex(u16);

/// The base graphic of a `DarkWoodDoor` — the closed WestCW leaf. Every other
/// leaf is `base + 2 * facing` closed, `+ 1` open, from ServUO's `DarkWoodDoor`.
const DARK_WOOD_BASE: u16 = 0x06A5;

impl GenFacing {
    /// This facing's `DoorFacing` index — its offset into ServUO's tables.
    fn index(self) -> DoorFacingIndex {
        match self {
            GenFacing::WestCw => DoorFacingIndex(0),
            GenFacing::EastCcw => DoorFacingIndex(1),
            GenFacing::SouthCw => DoorFacingIndex(4),
            GenFacing::NorthCcw => DoorFacingIndex(5),
        }
    }

    /// The closed graphic, open graphic, and hinge offset of a `DarkWoodDoor` at
    /// this facing — everything the [`Door`](openshard_state::components::Door)
    /// component needs.
    pub(crate) fn door(self) -> (Graphic, Graphic, i16, i16) {
        let index = self.index();
        let closed = DARK_WOOD_BASE + 2 * index.0;
        let (ox, oy) = OFFSETS[index.0 as usize];
        (Graphic(closed), Graphic(closed + 1), ox, oy)
    }
}

/// The hinge offset per `DoorFacing`, from `BaseDoor.m_Offsets`.
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
