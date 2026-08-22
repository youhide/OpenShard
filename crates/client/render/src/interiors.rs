//! The map facts behind an interior view.
//!
//! This module starts with the inexpensive, immutable part of the feature: a
//! map block's cells. Rooms, portals and a frame-specific view are built on
//! those cells later; ordinary frame assembly does not consult this module yet.

use std::collections::{BTreeMap, BTreeSet, VecDeque, btree_map::Entry};
use std::sync::Arc;

use openshard_map::grid::BlockCoord;
use openshard_map::map::{BLOCK_SIZE, WorldMap};
use openshard_movement::{MapTerrain, PLAYER_HEIGHT, Terrain};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::{StaticTile, TileData, TileFlags};

/// The stable identity of a cell within a baked map block.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CellId {
    pub block: BlockCoord,
    pub slot: u32,
}

/// The stable identity of a room local to a baked map block.
///
/// Rooms gain seam stitching before they become a whole-building identity. A
/// block-local id gives that stitcher stable endpoints without pretending an
/// eight-by-eight bake already knows the whole map.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RoomId {
    pub block: BlockCoord,
    pub slot: u32,
}

/// The stable representative of a room after the selected block bakes have
/// been joined across their shared edges.
///
/// A [`RoomId`] names one block's closed-door component.  It remains that
/// useful cache key; making it pretend to name a room beyond its block would
/// leave the result dependent on which neighbour happened to be baked first.
/// The stitcher instead chooses the least local id in each finished component.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StitchedRoomId {
    root: RoomId,
}

impl StitchedRoomId {
    /// The least local room id in this stitched component.
    pub const fn root(self) -> RoomId {
        self.root
    }
}

/// A deterministic address for one indexed building.
///
/// The least cell in the building is its stable representative.  Map statics
/// are immutable, so this identity does not depend on the order blocks entered
/// the lazy cache or the order a camera happened to inspect them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BuildingId {
    root: CellId,
}

impl BuildingId {
    /// The least cell in this building.
    pub const fn root(self) -> CellId {
        self.root
    }
}

/// One ordered structural floor within a [`BuildingId`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FloorId {
    building: BuildingId,
    slot: u32,
}

impl FloorId {
    /// The building that owns this floor.
    pub const fn building(self) -> BuildingId {
        self.building
    }

    /// The floor's low-to-high ordinal within its building.
    pub const fn slot(self) -> u32 {
        self.slot
    }
}

/// One connected structural floor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Floor {
    pub id: FloorId,
    cells: Vec<CellId>,
    min_z: i32,
    max_z: i32,
}

impl Floor {
    /// Cells on this floor, in deterministic map order.
    pub fn cells(&self) -> &[CellId] {
        &self.cells
    }

    /// The lowest surface in this floor band.
    pub const fn min_z(&self) -> i32 {
        self.min_z
    }

    /// The highest surface in this floor band.
    pub const fn max_z(&self) -> i32 {
        self.max_z
    }
}

/// A building and its structural floors, in low-to-high order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Building {
    pub id: BuildingId,
    floors: Vec<Floor>,
}

/// A node in the recursive interior-space tree.
///
/// The building is the root room. Ordinary stitched rooms are its children,
/// unless a low supported surface (a dais or stair tread) is contained by a
/// room below it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RoomNodeId {
    /// The root room that represents a whole building.
    Building(BuildingId),
    /// A closed-door room from the stitched room graph.
    Room(StitchedRoomId),
}

/// One node of a building's recursive room tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoomNode {
    id: RoomNodeId,
    parent: Option<RoomNodeId>,
    children: Vec<RoomNodeId>,
    floor: Option<FloorId>,
}

impl RoomNode {
    /// Stable identity of this node.
    pub const fn id(&self) -> RoomNodeId {
        self.id
    }

    /// The enclosing room, or `None` for the building root.
    pub const fn parent(&self) -> Option<RoomNodeId> {
        self.parent
    }

    /// Directly contained rooms, in deterministic order.
    pub fn children(&self) -> &[RoomNodeId] {
        &self.children
    }

    /// The structural floor that anchors this room, when it has one.
    pub const fn floor(&self) -> Option<FloorId> {
        self.floor
    }
}

/// Recursive ownership of stitched rooms by their building root rooms.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RoomTree {
    nodes: BTreeMap<RoomNodeId, RoomNode>,
}

impl RoomTree {
    fn bake(
        rooms: &StitchedRooms,
        building_of: &BTreeMap<CellId, BuildingId>,
        floor_of: &BTreeMap<CellId, FloorId>,
    ) -> Self {
        let mut nodes = BTreeMap::new();
        for &building in building_of.values() {
            nodes.entry(RoomNodeId::Building(building)).or_insert(RoomNode {
                id: RoomNodeId::Building(building),
                parent: None,
                children: Vec::new(),
                floor: None,
            });
        }

        let mut building_for_room = BTreeMap::new();
        for room in rooms.rooms().iter().filter(|room| !room.outdoors()) {
            let owners: BTreeSet<_> = room
                .cells()
                .iter()
                .filter_map(|cell| building_of.get(cell).copied())
                .collect();
            let Some(building) = (owners.len() == 1).then(|| *owners.first().expect("one owner")) else {
                continue;
            };
            let floor = room
                .cells()
                .iter()
                .filter_map(|cell| floor_of.get(cell).copied())
                .min_by_key(|floor| floor.slot());
            let id = RoomNodeId::Room(room.id);
            nodes.insert(
                id,
                RoomNode {
                    id,
                    parent: Some(RoomNodeId::Building(building)),
                    children: Vec::new(),
                    floor,
                },
            );
            building_for_room.insert(room.id, building);
        }

        let mut cells_by_tile: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for cell in rooms.cells() {
            cells_by_tile.entry(cell.tile).or_default().push(cell);
        }
        for room in rooms
            .rooms()
            .iter()
            .filter(|room| building_for_room.contains_key(&room.id))
        {
            let building = building_for_room[&room.id];
            let mut supports: BTreeMap<StitchedRoomId, usize> = BTreeMap::new();
            for &cell_id in room.cells() {
                let cell = rooms.cell(cell_id).expect("room cell is stitched");
                for lower in cells_by_tile.get(&cell.tile).into_iter().flatten() {
                    let Some(lower_room) = rooms.room_at(lower.id) else {
                        continue;
                    };
                    if lower_room == room.id
                        || building_for_room.get(&lower_room) != Some(&building)
                        || lower.floor_z >= cell.floor_z
                        || cell.floor_z - lower.floor_z >= i32::from(PLAYER_HEIGHT)
                        || lower.ceiling != Some(cell.floor_z)
                    {
                        continue;
                    }
                    *supports.entry(lower_room).or_default() += 1;
                }
            }
            let parent = supports
                .into_iter()
                .filter(|(_, supported)| *supported == room.cells().len())
                .map(|(parent, _)| parent)
                .min();
            if let Some(parent) = parent {
                nodes
                    .get_mut(&RoomNodeId::Room(room.id))
                    .expect("indexed room has a node")
                    .parent = Some(RoomNodeId::Room(parent));
            }
        }

        let relations: Vec<_> = nodes
            .values()
            .filter_map(|node| node.parent.map(|parent| (parent, node.id)))
            .collect();
        for (parent, child) in relations {
            nodes
                .get_mut(&parent)
                .expect("room parent exists")
                .children
                .push(child);
        }
        for node in nodes.values_mut() {
            node.children.sort_unstable();
        }
        Self { nodes }
    }

    /// The root node for this building.
    pub const fn building(building: BuildingId) -> RoomNodeId {
        RoomNodeId::Building(building)
    }

    /// A node by its stable identity.
    pub fn node(&self, id: RoomNodeId) -> Option<&RoomNode> {
        self.nodes.get(&id)
    }

    /// The outermost stitched room that contains `room`.
    pub fn enclosing_room(&self, room: StitchedRoomId) -> StitchedRoomId {
        let mut current = RoomNodeId::Room(room);
        while let Some(RoomNodeId::Room(parent)) = self.node(current).and_then(RoomNode::parent) {
            current = RoomNodeId::Room(parent);
        }
        match current {
            RoomNodeId::Room(room) => room,
            RoomNodeId::Building(_) => room,
        }
    }

    /// Add every ancestor and descendant of the supplied rooms.
    pub fn expand_visible(&self, rooms: &mut BTreeSet<StitchedRoomId>) {
        let mut pending: Vec<_> = rooms.iter().copied().map(RoomNodeId::Room).collect();
        while let Some(id) = pending.pop() {
            let Some(node) = self.node(id) else {
                continue;
            };
            if let Some(parent @ RoomNodeId::Room(room)) = node.parent() {
                if rooms.insert(room) {
                    pending.push(parent);
                }
            }
            for &child in node.children() {
                if let RoomNodeId::Room(room) = child {
                    if rooms.insert(room) {
                        pending.push(child);
                    }
                }
            }
        }
    }

    fn floor_of_room(&self, room: StitchedRoomId) -> Option<FloorId> {
        self.node(RoomNodeId::Room(self.enclosing_room(room)))?.floor()
    }
}

/// One cardinal, height-changing edge in a building's walk graph.
///
/// It is a diagnostic of the same `MapTerrain::can_step` relation that joins
/// structural floors into a building.  It does not assert that a particular
/// static graphic looks like stairs: ramps and drops have the same topological
/// role.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Stair {
    pub from: CellId,
    pub to: CellId,
}

impl Building {
    /// Structural floors ordered from low to high.
    pub fn floors(&self) -> &[Floor] {
        &self.floors
    }
}

/// One map door that was baked as shut.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DoorId {
    pub block: BlockCoord,
    pub slot: u32,
}

/// A door's immutable map position and graphic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Door {
    pub id: DoorId,
    pub at: Point,
    pub graphic: Graphic,
}

/// The two rooms a door joins when its leaf is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Portal {
    pub door: DoorId,
    pub rooms: [RoomId; 2],
}

/// A door portal after its two sides have been resolved across block seams.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StitchedPortal {
    pub door: DoorId,
    pub rooms: [StitchedRoomId; 2],
}

/// One connected set of cells on a structural floor band, bounded by walls.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Room {
    pub id: RoomId,
    cells: Vec<CellId>,
    outdoors: bool,
}

impl Room {
    /// Cells that belong to this room, in deterministic map order.
    pub fn cells(&self) -> &[CellId] {
        &self.cells
    }

    /// Whether any cell in this room reaches open sky.
    pub const fn outdoors(&self) -> bool {
        self.outdoors
    }
}

/// One closed-door room after the chosen block bakes have been stitched.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StitchedRoom {
    pub id: StitchedRoomId,
    cells: Vec<CellId>,
    outdoors: bool,
}

impl StitchedRoom {
    /// Cells that belong to this room, in deterministic map order.
    pub fn cells(&self) -> &[CellId] {
        &self.cells
    }

    /// Whether any cell in this room reaches open sky.
    pub const fn outdoors(&self) -> bool {
        self.outdoors
    }
}

/// The space above one standable surface and below the next one.
///
/// `ceiling == None` is open sky. A column can contain more than one cell: a
/// shop floor and its cellar, for example, are distinct even though their map
/// coordinates are the same.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub id: CellId,
    pub tile: (u16, u16),
    pub floor_z: i32,
    pub ceiling: Option<i32>,
}

impl Cell {
    /// Whether this cell reaches the sky without crossing another surface.
    pub const fn outdoors(self) -> bool {
        self.ceiling.is_none()
    }

    /// Whether a point in this column lies within the cell's vertical band.
    pub fn contains(self, point: Point) -> bool {
        self.tile == (point.x, point.y) && self.contains_z(point.z)
    }

    /// Whether a height falls inside this cell's vertical band.
    pub fn contains_z(self, z: i8) -> bool {
        i32::from(z) >= self.floor_z && self.ceiling.is_none_or(|ceiling| i32::from(z) < ceiling)
    }
}

/// The cells derived from one map block.
///
/// The bake is deliberately local and immutable. Joining compatible floor
/// bands across blocks, and then joining cells into rooms, is a later pass that
/// can stitch these stable local identities at block seams.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlockCells {
    pub id: BlockCoord,
    cells: Vec<Cell>,
    doors: Vec<Door>,
}

impl BlockCells {
    /// Bake one valid map block, or return `None` when the block is off-map.
    pub fn bake(map: &WorldMap, tiledata: &TileData, id: BlockCoord) -> Option<Self> {
        let (origin_x, origin_y) = id.origin();
        // A block past the `u16` a tile coordinate is expressed in is a block
        // no facet has; `map.contains` below would refuse it anyway, and the
        // conversion is where that becomes visible rather than a wrap.
        let origin_x = u16::try_from(origin_x).ok()?;
        let origin_y = u16::try_from(origin_y).ok()?;
        if !map.contains(origin_x, origin_y) {
            return None;
        }

        let mut cells = Vec::new();
        let mut doors = Vec::new();
        for local_y in 0..BLOCK_SIZE as u16 {
            for local_x in 0..BLOCK_SIZE as u16 {
                let x = origin_x.checked_add(local_x)?;
                let y = origin_y.checked_add(local_y)?;
                let mut floors = structural_surfaces(map, tiledata, x, y);
                floors.sort_unstable();
                floors.dedup();
                // A ceiling is not necessarily somewhere a body can stand.
                // In particular, the ordinary UO roof art is not a platform,
                // so `stand_surfaces` quite correctly leaves it out.  Treating
                // that omission as open sky made the first R1 overlay find a
                // few isolated platform cells instead of the area of a house.
                // The next standable floor is also a ceiling for the cell below
                // it, which is why this list starts with `floors`.
                let mut ceilings = floors.clone();
                ceilings.extend(map.statics_at(x, y).filter_map(|item| {
                    tiledata
                        .static_tile(item.tile.0)
                        .flags
                        .is_roof()
                        .then_some(i32::from(item.z))
                }));
                ceilings.sort_unstable();
                ceilings.dedup();
                for floor in floors {
                    let ceiling = ceilings.iter().copied().find(|&at| at > floor);
                    // A surface below a low ceiling is useful to movement as
                    // an obstruction, but it does not bound a room: a body
                    // cannot stand in the gap. Keeping it would let R1b join
                    // two rooms through a crawlspace the player can never
                    // occupy.
                    if ceiling.is_some_and(|at| i64::from(at) - i64::from(floor) < i64::from(PLAYER_HEIGHT)) {
                        continue;
                    }
                    let slot = u32::try_from(cells.len()).expect("one map block has too many cells");
                    cells.push(Cell {
                        id: CellId { block: id, slot },
                        tile: (x, y),
                        floor_z: floor,
                        ceiling,
                    });
                }
                for item in map.statics_at(x, y) {
                    if !tiledata.static_tile(item.tile.0).flags.has(TileFlags::DOOR) {
                        continue;
                    }
                    let slot = u32::try_from(doors.len()).expect("one map block has too many doors");
                    doors.push(Door {
                        id: DoorId { block: id, slot },
                        at: Point::new(x, y, item.z),
                        graphic: item.tile,
                    });
                }
            }
        }
        Some(Self { id, cells, doors })
    }

    /// Every cell in this block, in tile order then floor order.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Door leaves in this block, treated as shut while the room graph bakes.
    pub fn doors(&self) -> &[Door] {
        &self.doors
    }

    /// The cell containing a world point, if that point is inside this block.
    pub fn cell_at(&self, point: Point) -> Option<CellId> {
        self.cells
            .iter()
            .copied()
            .find(|cell| cell.contains(point))
            .map(|cell| cell.id)
    }
}

/// Room components and their closed-door portals within one map block.
///
/// The map seam stitcher owns joining these local rooms with their neighbours;
/// this type deliberately makes no off-block inference.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlockRooms {
    cells: BlockCells,
    rooms: Vec<Room>,
    room_of: Vec<Option<RoomId>>,
    portals: Vec<Portal>,
}

impl BlockRooms {
    /// Bake the closed-door room graph of one valid map block.
    pub fn bake(map: &WorldMap, tiledata: &TileData, id: BlockCoord) -> Option<Self> {
        Self::bake_with_shapes(map, tiledata, id, &|_| crate::occlusion::Shape::UNREAD)
    }

    /// Bake with the measured wall-facing data for the client art.
    ///
    /// A wall is an edge, not a solid floor tile.  The art table is therefore
    /// consulted while the flood graph is made; an unread graphic conservatively
    /// occupies every edge of its tile, the established renderer fallback.
    pub fn bake_with_shapes(
        map: &WorldMap,
        tiledata: &TileData,
        id: BlockCoord,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    ) -> Option<Self> {
        let cells = BlockCells::bake(map, tiledata, id)?;
        // Furniture can block a character but it never makes two rooms.  Only
        // a catalogued wall consumes the floor cell it stands on; the same
        // wall's measured edge is the predicate below that stops the flood
        // crossing to its neighbour.
        let blocked: Vec<_> = cells
            .cells
            .iter()
            .copied()
            .map(|cell| {
                wall_edges(map, tiledata, shape_of, cell).raw() != 0 || door_occupies(map, tiledata, cell)
            })
            .collect();
        let mut by_tile: BTreeMap<(u16, u16), Vec<usize>> = BTreeMap::new();
        for (at, cell) in cells.cells.iter().enumerate() {
            if !blocked[at] {
                by_tile.entry(cell.tile).or_default().push(at);
            }
        }

        let mut room_of = vec![None; cells.cells.len()];
        let mut rooms = Vec::new();
        for start in 0..cells.cells.len() {
            if blocked[start] || room_of[start].is_some() {
                continue;
            }
            let id = RoomId {
                block: cells.id,
                slot: u32::try_from(rooms.len()).expect("one map block has too many rooms"),
            };
            let mut members = Vec::new();
            let mut pending = vec![start];
            room_of[start] = Some(id);
            while let Some(at) = pending.pop() {
                let cell = cells.cells[at];
                members.push(cell.id);
                for neighbour in neighbours(cell.tile) {
                    for &other in by_tile.get(&neighbour).into_iter().flatten() {
                        if room_of[other].is_none()
                            && cells_join(map, tiledata, shape_of, cell, cells.cells[other])
                        {
                            room_of[other] = Some(id);
                            pending.push(other);
                        }
                    }
                }
            }
            let outdoors = members.iter().copied().any(|member| {
                cells.cells[usize::try_from(member.slot).expect("cell slot fits usize")].outdoors()
            });
            rooms.push(Room {
                id,
                cells: members,
                outdoors,
            });
        }

        let mut portals = Vec::new();
        for door in &cells.doors {
            let mut joined = Vec::new();
            for tile in neighbours((door.at.x, door.at.y)) {
                for &at in by_tile.get(&tile).into_iter().flatten() {
                    let cell = cells.cells[at];
                    if cell.contains_z(door.at.z) {
                        if let Some(room) = room_of[at] {
                            if !joined.contains(&room) {
                                joined.push(room);
                            }
                        }
                    }
                }
            }
            if let [first, second] = joined.as_slice() {
                portals.push(Portal {
                    door: door.id,
                    rooms: [*first, *second],
                });
            }
        }

        Some(Self {
            cells,
            rooms,
            room_of,
            portals,
        })
    }

    /// The cell bake this room graph was derived from.
    pub const fn cells(&self) -> &BlockCells {
        &self.cells
    }

    /// Closed-door rooms, in deterministic map order.
    pub fn rooms(&self) -> &[Room] {
        &self.rooms
    }

    /// The room containing a non-wall cell, if it is in this block.
    pub fn room_at(&self, cell: CellId) -> Option<RoomId> {
        (cell.block == self.cells.id)
            .then(|| {
                self.room_of
                    .get(usize::try_from(cell.slot).ok()?)
                    .copied()
                    .flatten()
            })
            .flatten()
    }

    /// Door portals. A door with an ambiguous topology records no portal until
    /// its geometry can identify exactly two sides.
    pub fn portals(&self) -> &[Portal] {
        &self.portals
    }

    /// Rooms that may be drawn in one frame.
    ///
    /// Outside is always a source, and the room containing the player is a
    /// second source. From either, only portals whose leaves are currently open
    /// are crossed. Door locations and room topology are baked; the closure is
    /// deliberately the tiny live part supplied by the current world view.
    pub fn shown_rooms(
        &self,
        player: Option<CellId>,
        mut door_open: impl FnMut(Door) -> bool,
    ) -> BTreeSet<RoomId> {
        let mut shown: BTreeSet<RoomId> = self
            .rooms
            .iter()
            .filter(|room| room.outdoors())
            .map(|room| room.id)
            .collect();
        if let Some(player) = player.and_then(|cell| self.room_at(cell)) {
            shown.insert(player);
        }

        let mut pending: Vec<_> = shown.iter().copied().collect();
        while let Some(room) = pending.pop() {
            for portal in &self.portals {
                let Some(door) = self
                    .cells
                    .doors
                    .get(usize::try_from(portal.door.slot).expect("door slot fits usize"))
                    .copied()
                else {
                    continue;
                };
                if !door_open(door) || !portal.rooms.contains(&room) {
                    continue;
                }
                let other = if portal.rooms[0] == room {
                    portal.rooms[1]
                } else {
                    portal.rooms[0]
                };
                if shown.insert(other) {
                    pending.push(other);
                }
            }
        }
        shown
    }
}

/// Closed-door rooms across a selected set of cached map blocks.
///
/// The block bake remains the durable, lazy map fact.  This value is its
/// seam-only composition: a caller chooses the blocks relevant to its picture,
/// then the component is derived without rebaking a column or treating a block
/// edge as a wall.  It deliberately owns no mutable door state; that stays in
/// [`Self::shown_rooms`]'s small per-frame walk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StitchedRooms {
    cells: BTreeMap<CellId, Cell>,
    rooms: Vec<StitchedRoom>,
    room_of: BTreeMap<CellId, StitchedRoomId>,
    doors: BTreeMap<DoorId, Door>,
    portals: Vec<StitchedPortal>,
}

impl StitchedRooms {
    /// Stitch a complete set of already-baked blocks.
    ///
    /// Duplicate blocks are harmless.  A missing neighbour is intentionally
    /// not inferred: the caller has not supplied that map fact, so joining to
    /// it would reintroduce the false seam this type exists to remove.
    pub fn bake(blocks: impl IntoIterator<Item = BlockRooms>) -> Self {
        Self::bake_with_join(blocks, &bands_join)
    }

    /// Stitch with the same wall-edge predicate the block bakes used.
    pub fn bake_with_shapes(
        map: &WorldMap,
        tiledata: &TileData,
        blocks: impl IntoIterator<Item = BlockRooms>,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    ) -> Self {
        Self::bake_with_join(blocks, &|one, other| {
            cells_join(map, tiledata, shape_of, one, other)
        })
    }

    fn bake_with_join(
        blocks: impl IntoIterator<Item = BlockRooms>,
        joins_cells: &dyn Fn(Cell, Cell) -> bool,
    ) -> Self {
        let blocks: BTreeMap<_, _> = blocks.into_iter().map(|block| (block.cells.id, block)).collect();

        let mut local_rooms = BTreeMap::new();
        let mut cells = BTreeMap::new();
        let mut room_of = BTreeMap::new();
        let mut doors = BTreeMap::new();
        let mut by_tile: BTreeMap<(u16, u16), Vec<CellId>> = BTreeMap::new();
        for block in blocks.values() {
            for room in &block.rooms {
                local_rooms.insert(room.id, room.clone());
            }
            for cell in block.cells.cells() {
                cells.insert(cell.id, *cell);
                by_tile.entry(cell.tile).or_default().push(cell.id);
                if let Some(room) = block.room_at(cell.id) {
                    room_of.insert(cell.id, room);
                }
            }
            for door in block.cells.doors() {
                doors.insert(door.id, *door);
            }
        }

        let locals: Vec<_> = local_rooms.keys().copied().collect();
        let local_at: BTreeMap<_, _> = locals
            .iter()
            .copied()
            .enumerate()
            .map(|(at, room)| (room, at))
            .collect();
        let mut joins = Components::new(locals.len());
        for (&id, &cell) in &cells {
            let Some(&room) = room_of.get(&id) else {
                continue;
            };
            // East and south alone see every shared edge once.  Looking in all
            // four directions would give the same graph, but would make a
            // doorway on a seam appear to have two different construction
            // paths.
            for neighbour in [
                (cell.tile.0.checked_add(1), Some(cell.tile.1)),
                (Some(cell.tile.0), cell.tile.1.checked_add(1)),
            ]
            .into_iter()
            .filter_map(|(x, y)| x.zip(y))
            {
                for other in by_tile.get(&neighbour).into_iter().flatten() {
                    let Some(&other_room) = room_of.get(other) else {
                        continue;
                    };
                    if joins_cells(cell, cells[other]) {
                        joins.join(local_at[&room], local_at[&other_room]);
                    }
                }
            }
        }

        let mut members: BTreeMap<usize, Vec<RoomId>> = BTreeMap::new();
        for (at, room) in locals.iter().copied().enumerate() {
            members.entry(joins.root(at)).or_default().push(room);
        }
        let mut stitched_of = BTreeMap::new();
        let mut rooms = Vec::with_capacity(members.len());
        for local_ids in members.values() {
            let id = StitchedRoomId {
                root: *local_ids.first().expect("a stitched component has a local room"),
            };
            let mut cells = Vec::new();
            let mut outdoors = false;
            for local in local_ids {
                let room = &local_rooms[local];
                stitched_of.insert(*local, id);
                cells.extend_from_slice(room.cells());
                outdoors |= room.outdoors();
            }
            cells.sort_unstable();
            rooms.push(StitchedRoom { id, cells, outdoors });
        }
        let room_of: BTreeMap<_, _> = room_of
            .into_iter()
            .map(|(cell, local)| (cell, stitched_of[&local]))
            .collect();

        let mut portals = Vec::new();
        for block in blocks.values() {
            for door in block.cells.doors() {
                let mut joined = Vec::new();
                for tile in neighbours((door.at.x, door.at.y)) {
                    for cell in by_tile.get(&tile).into_iter().flatten() {
                        if cells[cell].contains_z(door.at.z) {
                            if let Some(&room) = room_of.get(cell) {
                                if !joined.contains(&room) {
                                    joined.push(room);
                                }
                            }
                        }
                    }
                }
                if let [first, second] = joined.as_slice() {
                    if first != second {
                        portals.push(StitchedPortal {
                            door: door.id,
                            rooms: [*first, *second],
                        });
                    }
                }
            }
        }
        portals.sort_unstable_by_key(|portal| portal.door);

        Self {
            cells,
            rooms,
            room_of,
            doors,
            portals,
        }
    }

    /// Closed-door rooms after all supplied block seams have been crossed.
    pub fn rooms(&self) -> &[StitchedRoom] {
        &self.rooms
    }

    /// Every non-wall cell in the stitched graph, in deterministic map order.
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.room_of
            .keys()
            .filter_map(|cell| self.cells.get(cell).copied())
    }

    /// The baked cell at this address, including one under a wall.
    pub fn cell(&self, id: CellId) -> Option<Cell> {
        self.cells.get(&id).copied()
    }

    /// The indexed cell containing this world point.
    ///
    /// A stitched frame is one building-sized immutable value, so this small
    /// linear scan is paid once while its policy is resolved, never per drawn
    /// object. The render path uses [`InteriorFrame::shows_at`] afterwards.
    pub fn cell_at(&self, point: Point) -> Option<CellId> {
        self.cells
            .values()
            .copied()
            .find(|cell| cell.contains(point))
            .map(|cell| cell.id)
    }

    /// The stitched room containing this cell, if it is not occupied by a wall.
    pub fn room_at(&self, cell: CellId) -> Option<StitchedRoomId> {
        self.room_of.get(&cell).copied()
    }

    /// Door portals whose two sides may lie in different map blocks.
    pub fn portals(&self) -> &[StitchedPortal] {
        &self.portals
    }

    /// The immutable map door recorded for a portal.
    pub fn door(&self, id: DoorId) -> Option<Door> {
        self.doors.get(&id).copied()
    }

    /// Rooms that may be drawn in one frame.
    pub fn shown_rooms(
        &self,
        player: Option<CellId>,
        mut door_open: impl FnMut(Door) -> bool,
    ) -> BTreeSet<StitchedRoomId> {
        let mut shown: BTreeSet<_> = self
            .rooms
            .iter()
            .filter(|room| room.outdoors())
            .map(|room| room.id)
            .collect();
        if let Some(player) = player.and_then(|cell| self.room_at(cell)) {
            shown.insert(player);
        }

        let mut pending: Vec<_> = shown.iter().copied().collect();
        while let Some(room) = pending.pop() {
            for portal in &self.portals {
                let Some(door) = self.doors.get(&portal.door).copied() else {
                    continue;
                };
                if !portal.rooms.contains(&room) || !door_open(door) {
                    continue;
                }
                let other = if portal.rooms[0] == room {
                    portal.rooms[1]
                } else {
                    portal.rooms[0]
                };
                if shown.insert(other) {
                    pending.push(other);
                }
            }
        }
        shown
    }
}

/// Buildings and structural floors derived from one stitched room graph.
///
/// The graph is the map's actual step topology: [`MapTerrain::can_step`] is the
/// same answer movement uses for a cardinal transition. A building is a
/// connected component of that graph (plus its closed internal doors). A floor
/// is the component left after every height-changing step is removed. A second,
/// independent structural detector joins floor components that share a map
/// column. Thus a walk from a shop floor over a staircase to its upper floor
/// stays in one building while retaining two `FloorId`s; a stacked floor is
/// independent evidence for the same `BuildingId`, never a reason to collapse
/// the two `FloorId`s. A component with no sealed room remains outside the
/// building index, so the open world stays "not applicable" to R2.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Buildings {
    buildings: Vec<Building>,
    building_of: BTreeMap<CellId, BuildingId>,
    floor_of: BTreeMap<CellId, FloorId>,
    stairs: Vec<Stair>,
    rooms: RoomTree,
}

/// A facet-wide, baked answer to the first interior question: which ground
/// tiles are unreachable from the open world when walls and shut doors are
/// respected.
///
/// Label zero is the negative space (the exterior).  Every non-zero label is
/// one positive-space building.  The builder deliberately joins positive
/// components through *internal* door leaves, so this first artifact colours a
/// whole house.  A later room pass will retain the same exterior flood and
/// split one building at its doors.
///
/// This is planar on purpose.  It is the durable answer to "where is the
/// building footprint?" and does not borrow movement's notion of a standable
/// surface: a table, chest, stair tread or other clutter must not punch a hole
/// in the painted floor.  Storeys and stairs are a second graph over these
/// stable building ids.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BuildingMap {
    width: u32,
    height: u32,
    labels: Arc<[u32]>,
}

impl BuildingMap {
    /// Bake the positive space of one complete facet.
    ///
    /// The caller supplies the measured wall catalogue.  Keeping that input at
    /// this boundary is intentional: wall facing is an art fact owned by the
    /// renderer, while the result contains only map topology and is safe to
    /// load without opening a sprite archive.
    #[must_use]
    pub fn bake(
        map: &WorldMap,
        tiledata: &TileData,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    ) -> Self {
        let width = map.width();
        let height = map.height();
        let cells = usize::try_from(width)
            .expect("map width fits usize")
            .checked_mul(usize::try_from(height).expect("map height fits usize"))
            .expect("map dimensions fit address space");
        let at = |x: u32, y: u32| usize::try_from(y * width + x).expect("map index fits usize");

        let topology = PlanarTopology::bake(map, tiledata, shape_of);
        let walls = topology.walls;
        let wall_tiles = topology.wall_tiles;
        let doors = topology.doors;

        let mut labels = vec![0_u32; cells];
        let mut exterior = vec![false; cells];
        let mut pending = VecDeque::new();
        for x in 0..width {
            for y in [0, height.saturating_sub(1)] {
                let index = at(x, y);
                if !doors[index] && !wall_tiles[index] && !exterior[index] {
                    exterior[index] = true;
                    pending.push_back(index);
                }
            }
        }
        for y in 0..height {
            for x in [0, width.saturating_sub(1)] {
                let index = at(x, y);
                if !doors[index] && !wall_tiles[index] && !exterior[index] {
                    exterior[index] = true;
                    pending.push_back(index);
                }
            }
        }
        while let Some(one) = pending.pop_front() {
            let x = one % width as usize;
            let y = one / width as usize;
            for (other, side, opposite) in planar_neighbours(x, y, width as usize, height as usize) {
                if exterior[other]
                    || doors[other]
                    || wall_tiles[other]
                    || walls[one] & side != 0
                    || walls[other] & opposite != 0
                {
                    continue;
                }
                exterior[other] = true;
                pending.push_back(other);
            }
        }

        // First label the positive components with their least tile address.
        // That is both a deterministic identity and the union-find root used
        // below to bridge interior rooms through an internal door.
        let mut next = 1_u32;
        for start in 0..cells {
            if exterior[start] || doors[start] || wall_tiles[start] || labels[start] != 0 {
                continue;
            }
            labels[start] = next;
            next = next
                .checked_add(1)
                .expect("facet has fewer than u32::MAX buildings");
            pending.push_back(start);
            while let Some(one) = pending.pop_front() {
                let x = one % width as usize;
                let y = one / width as usize;
                for (other, side, opposite) in planar_neighbours(x, y, width as usize, height as usize) {
                    if exterior[other]
                        || doors[other]
                        || wall_tiles[other]
                        || labels[other] != 0
                        || walls[one] & side != 0
                        || walls[other] & opposite != 0
                    {
                        continue;
                    }
                    labels[other] = labels[start];
                    pending.push_back(other);
                }
            }
        }

        // Labels are dense and start at one, so this is proportional to actual
        // positive components rather than every tile in Britannia.
        let mut components = Components::new((next - 1) as usize);

        // A door may connect two rooms in the same house, but never connects a
        // positive room back to the exterior.  Joining only positive labels is
        // the key distinction between a front door and an internal one.
        for door in 0..cells {
            if !doors[door] {
                continue;
            }
            let x = door % width as usize;
            let y = door / width as usize;
            let mut sides = Vec::new();
            for (other, _, _) in planar_neighbours(x, y, width as usize, height as usize) {
                let label = labels[other];
                if label != 0 && !sides.contains(&label) {
                    sides.push(label);
                }
            }
            for pair in sides.windows(2) {
                components.join((pair[0] - 1) as usize, (pair[1] - 1) as usize);
            }
        }

        // Components was allocated for tile addresses so a label remains a
        // direct index.  Compact roots into 1..N only after every door has
        // joined its positive neighbours; colour ids are then stable across
        // camera positions and zoom levels.
        let mut compact = BTreeMap::new();
        for label in labels.iter_mut().filter(|label| **label != 0) {
            let root = components.root((*label - 1) as usize);
            let ordinal = u32::try_from(compact.len() + 1).expect("facet has fewer than u32::MAX buildings");
            *label = *compact.entry(root).or_insert(ordinal);
        }
        // The leaf is a barrier while the exterior flood runs, but it is still
        // part of the house picture.  Colour it from an adjacent positive tile
        // after all connectivity decisions are complete.  This fixes the
        // otherwise conspicuous unpainted square at an open or closed doorway
        // without ever allowing a front door to leak the outside label inside.
        for door in 0..cells {
            if !doors[door] {
                continue;
            }
            let x = door % width as usize;
            let y = door / width as usize;
            if let Some(label) = planar_neighbours(x, y, width as usize, height as usize)
                .map(|(other, _, _)| labels[other])
                .filter(|&label| label != 0)
                .min()
            {
                labels[door] = label;
            }
        }
        Self {
            width,
            height,
            labels: labels.into(),
        }
    }

    /// Reconstitute a validated baked payload.  File formats live in the
    /// offline tool crate; this keeps rendering free of filesystem policy.
    pub fn from_labels(width: u32, height: u32, labels: Vec<u32>) -> Option<Self> {
        let cells = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        (labels.len() == cells).then_some(Self {
            width,
            height,
            labels: labels.into(),
        })
    }

    /// Dimensions of the facet this map was baked from.
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The baked positive-space building at one tile, if any.
    pub fn building_at(&self, x: u16, y: u16) -> Option<u32> {
        (u32::from(x) < self.width && u32::from(y) < self.height)
            .then(|| self.labels[usize::from(y) * self.width as usize + usize::from(x)])
            .filter(|&label| label != 0)
    }

    /// Whether this tile touches the positive space of a different building.
    ///
    /// The planar bake intentionally leaves wall tiles unlabelled: a wall is a
    /// barrier *on an edge*, not a floor cell. Roof and wall art may be
    /// anchored up to two tiles beyond the enclosed floor, so the renderer
    /// assigns that visible shell by its nearby positive space without turning
    /// ordinary open terrain into building space.
    pub fn touches_other_building(&self, x: u16, y: u16, label: u32) -> bool {
        if self.building_at(x, y) == Some(label) {
            return false;
        }
        const SHELL_REACH: u16 = 2;
        let min_x = x.saturating_sub(SHELL_REACH);
        let max_x = x.saturating_add(SHELL_REACH);
        let min_y = y.saturating_sub(SHELL_REACH);
        let max_y = y.saturating_add(SHELL_REACH);
        (min_y..=max_y).any(|near_y| {
            (min_x..=max_x).any(|near_x| {
                self.building_at(near_x, near_y)
                    .is_some_and(|other| other != label)
            })
        })
    }

    /// Raw deterministic labels for the offline format only.
    pub fn labels(&self) -> &[u32] {
        &self.labels
    }

    /// Number of positive-space buildings in this facet.
    pub fn building_count(&self) -> usize {
        self.labels.iter().copied().max().unwrap_or(0) as usize
    }

    /// Map blocks touched by one immutable positive-space building label.
    ///
    /// The facet artifact is read only once per building by the app's interior
    /// cache. Returning blocks rather than a camera rectangle is crucial: a
    /// doorway at the far side of a house must not change room reachability
    /// merely because the camera panned away from it.
    pub fn blocks_for(&self, building: u32) -> BTreeSet<BlockCoord> {
        if building == 0 {
            return BTreeSet::new();
        }
        let width = usize::try_from(self.width).expect("facet width fits usize");
        self.labels
            .iter()
            .enumerate()
            .filter_map(|(at, &label)| (label == building).then_some(at))
            .map(|at| {
                let x = u16::try_from(at % width).expect("facet x fits u16");
                let y = u16::try_from(at / width).expect("facet y fits u16");
                BlockCoord::containing(x, y)
            })
            .collect()
    }

    /// One cardinal route from a tile the bake calls exterior to the actual
    /// map boundary.  This is an offline inspection aid: it exposes the wall
    /// or doorway the positive-space rule failed to cross, without making a
    /// camera frame reconstruct topology.
    pub fn exterior_path(
        map: &WorldMap,
        tiledata: &TileData,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
        start: (u16, u16),
    ) -> Option<Vec<(u16, u16)>> {
        let (width, height) = (map.width() as usize, map.height() as usize);
        let index = |x: usize, y: usize| y * width + x;
        let start = index(usize::from(start.0), usize::from(start.1));
        let topology = PlanarTopology::bake(map, tiledata, shape_of);
        if topology.wall_tiles[start] || topology.doors[start] {
            return None;
        }
        // A byte per tile is enough to reconstruct the route and avoids a
        // whole-facet usize parent table merely for a diagnostic.
        const UNSEEN: i8 = -1;
        const ROOT: i8 = 4;
        let mut previous = vec![UNSEEN; topology.walls.len()];
        let mut pending = VecDeque::from([start]);
        previous[start] = ROOT;
        let boundary = loop {
            let here = pending.pop_front()?;
            let x = here % width;
            let y = here / width;
            if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
                break here;
            }
            for (other, side, opposite) in planar_neighbours(x, y, width, height) {
                if previous[other] != UNSEEN
                    || topology.doors[other]
                    || topology.wall_tiles[other]
                    || topology.walls[here] & side != 0
                    || topology.walls[other] & opposite != 0
                {
                    continue;
                }
                // Direction from `other` back to `here`.
                previous[other] = match other as isize - here as isize {
                    delta if delta == -(width as isize) => 2,
                    delta if delta == 1 => 3,
                    delta if delta == width as isize => 0,
                    delta if delta == -1 => 1,
                    _ => unreachable!("cardinal neighbour"),
                };
                pending.push_back(other);
            }
        };
        let mut path = Vec::new();
        let mut here = boundary;
        loop {
            path.push((
                u16::try_from(here % width).expect("UO coordinate"),
                u16::try_from(here / width).expect("UO coordinate"),
            ));
            match previous[here] {
                ROOT => break,
                0 => here -= width,
                1 => here += 1,
                2 => here += width,
                3 => here -= 1,
                _ => unreachable!("visited tile has a predecessor"),
            }
        }
        path.reverse();
        Some(path)
    }
}

/// Planar wall and door facts shared by the bake and its offline inspector.
struct PlanarTopology {
    walls: Vec<u8>,
    wall_tiles: Vec<bool>,
    doors: Vec<bool>,
}

impl PlanarTopology {
    fn bake(
        map: &WorldMap,
        tiledata: &TileData,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    ) -> Self {
        let (width, height) = (map.width() as usize, map.height() as usize);
        let cells = width
            .checked_mul(height)
            .expect("map dimensions fit address space");
        let mut walls = vec![0_u8; cells];
        let mut wall_tiles = vec![false; cells];
        let mut doors = vec![false; cells];
        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                let x = u16::try_from(x).expect("facet fits UO coordinates");
                let y = u16::try_from(y).expect("facet fits UO coordinates");
                for item in map.statics_at(x, y) {
                    let tile = tiledata.static_tile(item.tile.0);
                    doors[index] |= tile.flags.has(TileFlags::DOOR);
                    if !wall_supports_low_platform(map, tiledata, x, y, item.z, tile) {
                        walls[index] |= planar_wall_edges(tile, shape_of(item.tile));
                    }
                }
                wall_tiles[index] = walls[index] != 0;
            }
        }
        // Functional doors are server ground items, but the map encodes their
        // *places* as pairs of specific DoorGenerator frame art around a one-
        // or two-tile gap.  The first interior bake only read map statics, so a
        // real wooden double door at Britain 1435–1436,1599 was an unbounded
        // hole in the wall contour.  Recover that immutable anchor with the
        // server's exact frame tables and equal-height guard; the leaf's
        // open/closed graphic remains a live item-layer fact.
        let terrain = MapTerrain::new(map, tiledata);
        for y in 0..height {
            for x in 0..width {
                let x16 = u16::try_from(x).expect("facet fits UO coordinates");
                let y16 = u16::try_from(y).expect("facet fits UO coordinates");
                for frame in map.statics_at(x16, y16) {
                    if openshard_movement::door_frames::is_west_frame(frame.tile.0) {
                        if x + 2 < width
                            && generated_frame_at(
                                map,
                                x + 2,
                                y,
                                frame.z,
                                openshard_movement::door_frames::is_east_frame,
                            )
                        {
                            generated_door_anchor(&terrain, &wall_tiles, &mut doors, x + 1, y, frame.z);
                        } else if x + 3 < width
                            && generated_frame_at(
                                map,
                                x + 3,
                                y,
                                frame.z,
                                openshard_movement::door_frames::is_east_frame,
                            )
                        {
                            generated_door_anchor(&terrain, &wall_tiles, &mut doors, x + 1, y, frame.z);
                            generated_door_anchor(&terrain, &wall_tiles, &mut doors, x + 2, y, frame.z);
                        }
                    } else if openshard_movement::door_frames::is_north_frame(frame.tile.0) {
                        if y + 2 < height
                            && generated_frame_at(
                                map,
                                x,
                                y + 2,
                                frame.z,
                                openshard_movement::door_frames::is_south_frame,
                            )
                        {
                            generated_door_anchor(&terrain, &wall_tiles, &mut doors, x, y + 1, frame.z);
                        } else if y + 3 < height
                            && generated_frame_at(
                                map,
                                x,
                                y + 3,
                                frame.z,
                                openshard_movement::door_frames::is_south_frame,
                            )
                        {
                            generated_door_anchor(&terrain, &wall_tiles, &mut doors, x, y + 1, frame.z);
                            generated_door_anchor(&terrain, &wall_tiles, &mut doors, x, y + 2, frame.z);
                        }
                    }
                }
            }
        }
        Self {
            walls,
            wall_tiles,
            doors,
        }
    }
}

/// A matching, same-height `DoorGenerator` frame at this map tile.
fn generated_frame_at(map: &WorldMap, x: usize, y: usize, z: i8, side: fn(u16) -> bool) -> bool {
    map.statics_at(
        u16::try_from(x).expect("facet fits UO coordinates"),
        u16::try_from(y).expect("facet fits UO coordinates"),
    )
    .any(|item| item.z == z && side(item.tile.0))
}

/// Mark one server-generated door position, using the same stand-height test as
/// the server's placement pass.
fn generated_door_anchor<M, T>(
    terrain: &MapTerrain<M, T>,
    wall_tiles: &[bool],
    doors: &mut [bool],
    x: usize,
    y: usize,
    z: i8,
) where
    M: AsRef<WorldMap>,
    T: AsRef<TileData>,
{
    let width = terrain.map().width() as usize;
    let index = y * width + x;
    if wall_tiles[index] || doors[index] {
        return;
    }
    if terrain.can_fit(
        openshard_movement::Tile::new(
            u16::try_from(x).expect("facet fits UO coordinates"),
            u16::try_from(y).expect("facet fits UO coordinates"),
        ),
        i32::from(z),
        PLAYER_HEIGHT,
    ) {
        doors[index] = true;
    }
}

impl Buildings {
    /// Derive the structural buildings from an immutable stitched room graph.
    pub fn bake(map: &WorldMap, tiledata: &TileData, rooms: &StitchedRooms) -> Self {
        let cells: Vec<_> = rooms.cells().collect();
        let mut by_tile: BTreeMap<(u16, u16), Vec<usize>> = BTreeMap::new();
        for (at, cell) in cells.iter().enumerate() {
            by_tile.entry(cell.tile).or_default().push(at);
        }

        let terrain = MapTerrain::new(map, tiledata);
        let mut steps = BTreeSet::new();
        for (at, cell) in cells.iter().copied().enumerate() {
            let Ok(z) = i8::try_from(cell.floor_z) else {
                continue;
            };
            for (x, y) in neighbours(cell.tile) {
                let Some(landing) =
                    terrain.can_step(Point::new(cell.tile.0, cell.tile.1, z), Point::new(x, y, z))
                else {
                    continue;
                };
                let Some(other) = by_tile
                    .get(&(landing.x, landing.y))
                    .into_iter()
                    .flatten()
                    .copied()
                    .find(|&other| cells[other].contains_z(landing.z))
                else {
                    continue;
                };
                steps.insert((at.min(other), at.max(other)));
            }
        }

        // A floor is traversable without climbing or descending. The changed-z
        // edges are stairs, ramps and drops; they connect the building below,
        // but cannot flatten it into one selected structural floor.
        let mut floor_components = Components::new(cells.len());
        for &(one, other) in &steps {
            if cells[one].floor_z == cells[other].floor_z {
                floor_components.join(one, other);
            }
        }

        let mut floor_members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for at in 0..cells.len() {
            floor_members
                .entry(floor_components.root(at))
                .or_default()
                .push(at);
        }
        let floor_roots: Vec<_> = floor_members.keys().copied().collect();
        let floor_at: BTreeMap<_, _> = floor_roots
            .iter()
            .copied()
            .enumerate()
            .map(|(at, root)| (root, at))
            .collect();

        // Every walk edge, including a stair's changed-z edge, joins its floor
        // components into one building. The floor relation above deliberately
        // did not take that second step.
        let mut building_components = Components::new(floor_roots.len());
        for &(one, other) in &steps {
            building_components.join(
                floor_at[&floor_components.root(one)],
                floor_at[&floor_components.root(other)],
            );
        }

        // This is deliberately a second detector rather than an approximation
        // of `can_step`: floors sharing a footprint are structural evidence for
        // one building even when a stair is missing from the inspected blocks.
        // It joins only the *building* graph. The equal-height walk graph above
        // remains the sole source of a `FloorId`, so a vertical stack cannot
        // manufacture a walkable floor connection.
        for column in by_tile.values() {
            let mut here: Vec<_> = column.iter().map(|&cell| floor_components.root(cell)).collect();
            here.sort_unstable();
            here.dedup();
            for pair in here.windows(2) {
                building_components.join(floor_at[&pair[0]], floor_at[&pair[1]]);
            }
        }

        // Doors are intentionally walls in the ordinary walk graph. They still
        // belong to their building: an internal closed door does not turn a
        // bedroom into the house next door. Never bridge through the outdoor
        // room, which is the whole map's exterior component.
        let indoor_rooms: BTreeSet<_> = rooms
            .rooms()
            .iter()
            .filter(|room| !room.outdoors())
            .map(|room| room.id)
            .collect();
        let mut floors_by_room: BTreeMap<StitchedRoomId, Vec<usize>> = BTreeMap::new();
        for (at, cell) in cells.iter().enumerate() {
            let Some(room) = rooms.room_at(cell.id) else {
                continue;
            };
            floors_by_room
                .entry(room)
                .or_default()
                .push(floor_components.root(at));
        }
        for floors in floors_by_room.values_mut() {
            floors.sort_unstable();
            floors.dedup();
        }
        for portal in rooms.portals() {
            if !portal.rooms.iter().all(|room| indoor_rooms.contains(room)) {
                continue;
            }
            let [one, other] = portal.rooms;
            let (Some(one), Some(other)) = (floors_by_room.get(&one), floors_by_room.get(&other)) else {
                continue;
            };
            for &left in one {
                for &right in other {
                    building_components.join(floor_at[&left], floor_at[&right]);
                }
            }
        }

        let mut building_members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (at, floor) in floor_roots.iter().copied().enumerate() {
            building_members
                .entry(building_components.root(at))
                .or_default()
                .push(floor);
        }

        let mut buildings = Vec::new();
        let mut building_of = BTreeMap::new();
        let mut floor_of = BTreeMap::new();
        for floor_roots in building_members.values() {
            let has_indoor_room = floor_roots.iter().copied().any(|floor| {
                floor_members[&floor]
                    .iter()
                    .copied()
                    .filter_map(|at| rooms.room_at(cells[at].id))
                    .any(|room| indoor_rooms.contains(&room))
            });
            if !has_indoor_room {
                continue;
            }

            let root = floor_roots
                .iter()
                .copied()
                .flat_map(|floor| floor_members[&floor].iter().copied())
                .map(|at| cells[at].id)
                .min()
                .expect("a building has a floor cell");
            let id = BuildingId { root };
            let mut floors: Vec<_> = floor_roots
                .iter()
                .copied()
                .map(|floor| {
                    let members = &floor_members[&floor];
                    let mut floor_cells: Vec<_> = members.iter().map(|&at| cells[at].id).collect();
                    floor_cells.sort_unstable();
                    let min_z = members
                        .iter()
                        .map(|&at| cells[at].floor_z)
                        .min()
                        .expect("floor has cells");
                    let max_z = members
                        .iter()
                        .map(|&at| cells[at].floor_z)
                        .max()
                        .expect("floor has cells");
                    (floor_cells, min_z, max_z)
                })
                .collect();
            floors.sort_unstable_by_key(|(cells, min_z, max_z)| (*min_z, *max_z, cells[0]));
            let floors: Vec<_> = floors
                .into_iter()
                .enumerate()
                .map(|(slot, (cells, min_z, max_z))| Floor {
                    id: FloorId {
                        building: id,
                        slot: u32::try_from(slot).expect("one building has too many floors"),
                    },
                    cells,
                    min_z,
                    max_z,
                })
                .collect();
            for floor in &floors {
                for &cell in floor.cells() {
                    building_of.insert(cell, id);
                    floor_of.insert(cell, floor.id);
                }
            }
            buildings.push(Building { id, floors });
        }
        buildings.sort_unstable_by_key(|building| building.id);
        let mut stairs: Vec<_> = steps
            .iter()
            .copied()
            .filter(|&(one, other)| {
                cells[one].floor_z != cells[other].floor_z
                    && building_of.contains_key(&cells[one].id)
                    && building_of.contains_key(&cells[other].id)
            })
            .map(|(one, other)| Stair {
                from: cells[one].id,
                to: cells[other].id,
            })
            .collect();
        stairs.sort_unstable();
        let room_tree = RoomTree::bake(rooms, &building_of, &floor_of);
        Self {
            buildings,
            building_of,
            floor_of,
            stairs,
            rooms: room_tree,
        }
    }

    /// Indexed buildings, in deterministic map order.
    pub fn buildings(&self) -> &[Building] {
        &self.buildings
    }

    /// The indexed building containing this cell, if it is not ordinary open world.
    pub fn building_at(&self, cell: CellId) -> Option<BuildingId> {
        self.building_of.get(&cell).copied()
    }

    /// The structural floor containing this cell, if it is in an indexed building.
    pub fn floor_at(&self, cell: CellId) -> Option<FloorId> {
        self.floor_of.get(&cell).copied()
    }

    /// Recursive room ownership for all indexed buildings.
    pub fn rooms(&self) -> &RoomTree {
        &self.rooms
    }

    /// Height-changing walk transitions in indexed buildings.
    pub fn stairs(&self) -> &[Stair] {
        &self.stairs
    }
}

/// Which structural floor an interior picture is opened to.
///
/// `Manual` is deliberately relative to the floor containing the player, not
/// to a world height. The diagnostic z-slice reuses this relative value only
/// as a convenient control, not as its building-rendering definition.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FloorView {
    /// Follow the structural floor containing the player.
    #[default]
    Auto,
    /// Select a floor relative to the player's structural floor.
    Manual { relative: i8 },
}

/// The independent controls for the diagnostic height-only picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ZSliceView {
    /// Begin at the player's current height and span one ordinary storey band.
    #[default]
    Auto,
    /// Draw only the inclusive range explicitly entered by the person.
    Manual { lower: i8, upper: i8 },
}

/// The immutable visibility policy for one frame.
///
/// This is intentionally separate from [`crate::cutaway::Cutaway`].  The
/// latter remains the global height-and-roof predicate. Normally this answers
/// which indexed cells of one building may contribute geometry. A separate
/// diagnostic constructor supplies an aggressive global z band instead.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InteriorFrame {
    building: BuildingId,
    selected_floors: BTreeSet<FloorId>,
    shown_rooms: BTreeSet<StitchedRoomId>,
    building_cells: BTreeSet<CellId>,
    shown_cells: BTreeSet<CellId>,
    /// The app and the render collectors meet at a world point, whereas the
    /// policy itself names cells.  Keep this private translation table in the
    /// resolved frame so no collector has to reconstruct room topology.
    cells_by_tile: BTreeMap<(u16, u16), Vec<Cell>>,
    z_range: Option<(i8, i8)>,
    /// The facet-wide positive-space map. Outside, every labelled tile is
    /// hidden; inside, only labels other than `visible_label` are hidden.
    building_map: Option<BuildingMap>,
    /// The positive-space label of the building whose room/floor picture this
    /// frame shows. `None` denotes an exterior frame.
    visible_label: Option<u32>,
}

impl InteriorFrame {
    /// Resolve one frame's building picture from immutable topology and the
    /// live state of its door leaves.
    ///
    /// `None` is the important outside answer: the player is not in an indexed
    /// building, so no caller has an interior policy to compose with its usual
    /// draw predicate.  A manual selection beyond the lowest cellar or highest
    /// storey clamps to that real structural endpoint.
    pub fn at(
        buildings: &Buildings,
        rooms: &StitchedRooms,
        player: Option<CellId>,
        view: FloorView,
        mut door_open: impl FnMut(Door) -> bool,
    ) -> Option<Self> {
        let player = player?;
        let building = buildings.building_at(player)?;
        let player_room = rooms.room_at(player)?;
        let player_floor = buildings
            .rooms()
            .floor_of_room(player_room)
            .or_else(|| buildings.floor_at(player))?;
        let floors = buildings
            .buildings()
            .iter()
            .find(|candidate| candidate.id == building)?
            .floors();
        let highest = i32::try_from(floors.len().checked_sub(1)?).ok()?;
        let player_slot = i32::try_from(player_floor.slot()).ok()?;
        let selected_slot = match view {
            FloorView::Auto => player_slot,
            FloorView::Manual { relative } => (player_slot + i32::from(relative)).clamp(0, highest),
        };
        let selected_floors: BTreeSet<_> = floors
            .iter()
            .filter(|floor| i32::try_from(floor.id.slot()).is_ok_and(|slot| slot <= selected_slot))
            .map(|floor| floor.id)
            .collect();
        let mut shown_rooms = rooms.shown_rooms(Some(player), &mut door_open);
        buildings.rooms().expand_visible(&mut shown_rooms);
        let mut building_cells = BTreeSet::new();
        let mut shown_cells = BTreeSet::new();
        let mut cells_by_tile: BTreeMap<(u16, u16), Vec<Cell>> = BTreeMap::new();
        for cell in rooms.cells() {
            if buildings.building_at(cell.id) != Some(building) {
                continue;
            }
            building_cells.insert(cell.id);
            cells_by_tile.entry(cell.tile).or_default().push(cell);
            let room_is_shown = rooms
                .room_at(cell.id)
                .is_some_and(|room| shown_rooms.contains(&room));
            let floor_is_selected = buildings
                .floor_at(cell.id)
                .is_some_and(|floor| selected_floors.contains(&floor))
                || rooms.room_at(cell.id).is_some_and(|room| {
                    buildings
                        .rooms()
                        .floor_of_room(room)
                        .is_some_and(|floor| selected_floors.contains(&floor))
                });
            if floor_is_selected && room_is_shown {
                shown_cells.insert(cell.id);
            }
        }
        Some(Self {
            building,
            selected_floors,
            shown_rooms,
            building_cells,
            shown_cells,
            cells_by_tile,
            z_range: None,
            building_map: None,
            visible_label: None,
        })
    }

    /// Hide the positive space of every building except the one this frame
    /// already describes.
    ///
    /// The room graph is deliberately baked for one building at a time. The
    /// facet map supplies the complementary, cheap test for all the other
    /// buildings in view, without making their room topology camera-local.
    #[must_use]
    pub fn with_other_buildings_hidden(mut self, buildings: BuildingMap, visible_label: u32) -> Self {
        debug_assert_ne!(visible_label, 0, "a visible building must have a positive label");
        self.building_map = Some(buildings);
        self.visible_label = Some(visible_label);
        self
    }

    /// A view from ordinary exterior space. Every tile the facet index names
    /// as building-positive is withheld; wall tiles remain because the index
    /// intentionally labels only the space *inside* their contour.
    pub fn outside(buildings: BuildingMap) -> Self {
        let root = CellId {
            block: BlockCoord { x: 0, y: 0 },
            slot: 0,
        };
        Self {
            building: BuildingId { root },
            selected_floors: BTreeSet::new(),
            shown_rooms: BTreeSet::new(),
            building_cells: BTreeSet::new(),
            shown_cells: BTreeSet::new(),
            cells_by_tile: BTreeMap::new(),
            z_range: None,
            building_map: Some(buildings),
            visible_label: None,
        }
    }

    /// The height covered by one deliberately coarse displayed band.
    pub const Z_BAND: i8 = 20;

    /// Resolve the simple runtime picture: every producer in this inclusive
    /// z range is drawn, and everything outside it leaves the cleared black
    /// frame visible. No artifact, room graph, door state or building
    /// membership participates.
    pub fn z_slice(player: Point, view: ZSliceView) -> Self {
        let root = CellId {
            block: BlockCoord { x: 0, y: 0 },
            slot: 0,
        };
        Self {
            building: BuildingId { root },
            selected_floors: BTreeSet::new(),
            shown_rooms: BTreeSet::new(),
            building_cells: BTreeSet::new(),
            shown_cells: BTreeSet::new(),
            cells_by_tile: BTreeMap::new(),
            z_range: None,
            building_map: None,
            visible_label: None,
        }
        .with_z_slice(player, view)
    }

    /// Add a height band to either the outside guard or a room/floor frame.
    /// The two tests compose by intersection, never by one mode replacing the
    /// other.
    pub fn with_z_slice(mut self, player: Point, view: ZSliceView) -> Self {
        self.z_range = Some(match view {
            ZSliceView::Auto => (player.z, player.z.saturating_add(Self::Z_BAND)),
            ZSliceView::Manual { lower, upper } => (lower.min(upper), lower.max(upper)),
        });
        self
    }

    /// The active inclusive z band, if this is the simplified runtime policy.
    pub const fn z_range(&self) -> Option<(i8, i8)> {
        self.z_range
    }

    /// The building this frame is a policy for.
    pub const fn building(&self) -> BuildingId {
        self.building
    }

    /// Structural floors included in this picture, from the bottom through
    /// the selected floor.
    pub fn selected_floors(&self) -> &BTreeSet<FloorId> {
        &self.selected_floors
    }

    /// Rooms reachable from the sky or the player's room through open doors.
    pub fn shown_rooms(&self) -> &BTreeSet<StitchedRoomId> {
        &self.shown_rooms
    }

    /// Whether this cell belongs to the building this frame governs.
    ///
    /// Walls do not have a room cell and thus normally answer false here.  A
    /// geometry caller must leave such an object on the ordinary path, which
    /// is how outer walls stay drawable around a black sealed room.
    pub fn applies_to(&self, cell: CellId) -> bool {
        self.building_cells.contains(&cell)
    }

    /// Whether an applicable cell contributes to this frame's picture.
    pub fn shows_cell(&self, cell: CellId) -> bool {
        self.shown_cells.contains(&cell)
    }

    /// Whether geometry standing at this world point belongs in the picture.
    ///
    /// A point outside this frame's building, including a wall tile with no
    /// inhabitable cell, remains on the ordinary render path. A positive-space
    /// tile belonging to another building is the exception when the app has
    /// supplied the facet-wide building map.
    pub fn shows_at(&self, point: Point) -> bool {
        if let Some((min, max)) = self.z_range {
            if !(min..=max).contains(&point.z) {
                return false;
            }
        }
        if let Some(buildings) = &self.building_map {
            match self.visible_label {
                None => return buildings.building_at(point.x, point.y).is_none(),
                Some(label)
                    if buildings
                        .building_at(point.x, point.y)
                        .is_some_and(|other| other != label) =>
                {
                    return false;
                }
                Some(_) => {}
            }
        }
        self.cells_by_tile
            .get(&(point.x, point.y))
            .and_then(|cells| {
                cells
                    .iter()
                    .copied()
                    .find(|cell| cell.contains(point))
                    // Roof/lid art stands exactly on the ceiling boundary,
                    // which is outside the half-open body band below it. It
                    // nevertheless belongs to that room's picture: otherwise
                    // a sealed room would lose its floor and furniture while
                    // retaining the roof that covers the resulting clear area.
                    .or_else(|| {
                        cells
                            .iter()
                            .copied()
                            .find(|cell| cell.ceiling == Some(i32::from(point.z)))
                    })
            })
            .is_none_or(|cell| self.shows_cell(cell.id))
    }

    /// [`shows_at`](Self::shows_at), preserving roof and window statics in an
    /// exterior view. They form the visible skin of a house, rather than its
    /// contents; the z band still applies.
    pub fn shows_static_at(&self, point: Point, tile: &StaticTile) -> bool {
        if let Some((min, max)) = self.z_range {
            if !(min..=max).contains(&point.z) {
                return false;
            }
        }
        if self.visible_label.is_none()
            && self.building_map.is_some()
            && (tile.flags.is_roof() || tile.flags.has(TileFlags::WINDOW))
        {
            return true;
        }
        if let (Some(buildings), Some(label)) = (&self.building_map, self.visible_label) {
            const BUILDING_SHELL: u64 = TileFlags::WALL | TileFlags::NO_SHOOT | TileFlags::WINDOW;
            if (tile.flags.is_roof() || tile.flags.has(BUILDING_SHELL))
                && buildings.touches_other_building(point.x, point.y, label)
            {
                return false;
            }
        }
        self.shows_at(point)
    }

    /// A stable, compact summary of every cell visibility decision.
    ///
    /// The geometry cache stores this value alongside its other inputs.  It is
    /// intentionally based on resolved cells rather than the relative UI
    /// number: two requests that draw the same building picture reuse exactly
    /// the same cache entry.
    pub fn fingerprint(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        fn mix(mut hash: u64, value: u64) -> u64 {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(PRIME);
            }
            hash
        }
        let mut hash = OFFSET;
        if let Some((min, max)) = self.z_range {
            hash = mix(hash, 1);
            hash = mix(hash, min as u8 as u64);
            hash = mix(hash, max as u8 as u64);
        } else {
            hash = mix(hash, 0);
        }
        if let Some(label) = self.visible_label {
            hash = mix(hash, 2);
            hash = mix(hash, u64::from(label));
        } else if self.building_map.is_some() {
            hash = mix(hash, 3);
        }
        hash = mix(
            hash,
            u64::try_from(self.shown_cells.len()).expect("cell count fits u64"),
        );
        for cell in &self.shown_cells {
            hash = mix(hash, u64::from(cell.block.x));
            hash = mix(hash, u64::from(cell.block.y));
            hash = mix(hash, u64::from(cell.slot));
        }
        hash
    }
}

/// A tiny union-find, used only while a stitched graph is constructed.
#[derive(Debug)]
struct Components {
    parent: Vec<usize>,
}

impl Components {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn root(&mut self, at: usize) -> usize {
        let parent = self.parent[at];
        if parent != at {
            self.parent[at] = self.root(parent);
        }
        self.parent[at]
    }

    fn join(&mut self, one: usize, other: usize) {
        let (one, other) = (self.root(one), self.root(other));
        if one != other {
            // The lower first room stays the representative, so stitching the
            // same blocks in another order keeps every diagnostic id stable.
            self.parent[other.max(one)] = one.min(other);
        }
    }
}

/// Cells join only if a body fits in their vertical overlap.
fn bands_join(one: Cell, other: Cell) -> bool {
    let floor = one.floor_z.max(other.floor_z);
    let ceiling = one
        .ceiling
        .unwrap_or(i32::MAX)
        .min(other.ceiling.unwrap_or(i32::MAX));
    // An open-sky cell deliberately uses `i32::MAX` as its sentinel.  A
    // cellar can have a negative floor, so subtracting the two in `i32`
    // overflows in debug builds.  This comparison is a height question, not a
    // 32-bit arithmetic requirement.
    i64::from(ceiling) - i64::from(floor) >= i64::from(PLAYER_HEIGHT)
}

/// Whether two adjacent floor cells share an open wall edge.
///
/// The map's wall sprites stand *on* an edge of a tile.  Treating the entire
/// tile as a wall loses the inside/outside topology, which is why a flood could
/// escape a real house through art that was visually a continuous wall.  The
/// art table's measured facing supplies the edge; an unread wall is the safe
/// four-edge fallback in `named_edges`.
fn cells_join(
    map: &WorldMap,
    tiledata: &TileData,
    shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    one: Cell,
    other: Cell,
) -> bool {
    if !bands_join(one, other) {
        return false;
    }
    let side = match (
        i32::from(other.tile.0) - i32::from(one.tile.0),
        i32::from(other.tile.1) - i32::from(one.tile.1),
    ) {
        (0, -1) => crate::occlusion::Edges::NORTH,
        (1, 0) => crate::occlusion::Edges::EAST,
        (0, 1) => crate::occlusion::Edges::SOUTH,
        (-1, 0) => crate::occlusion::Edges::WEST,
        _ => return false,
    };
    let one_edges = wall_edges(map, tiledata, shape_of, one);
    let other_edges = wall_edges(map, tiledata, shape_of, other);
    !one_edges.contains(side) && !other_edges.contains(crate::occlusion::opposite(side))
}

/// The wall panels that occupy a cell at its floor band.
fn wall_edges(
    map: &WorldMap,
    tiledata: &TileData,
    shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    cell: Cell,
) -> crate::occlusion::Edges {
    const WALLISH: u64 = TileFlags::WALL | TileFlags::NO_SHOOT;
    map.statics_at(cell.tile.0, cell.tile.1)
        .fold(crate::occlusion::Edges::NONE, |edges, item| {
            let tile = tiledata.static_tile(item.tile.0);
            let flags = tile.flags;
            // A roof is vertical headroom, not a room boundary.  A platform is a
            // floor/ceiling, and a door is held shut by the portal graph rather
            // than turned into four permanent walls here.
            if !flags.has(WALLISH) || flags.is_roof() || flags.is_platform() || flags.has(TileFlags::DOOR) {
                return edges;
            }
            if !wall_supports_low_platform(map, tiledata, cell.tile.0, cell.tile.1, item.z, tile) {
                let shape = shape_of(item.tile);
                // `BLOCK` is a movement fact — a table, a bed or a crate — and
                // cannot become a room boundary.  A measured facing is the wall
                // catalogue's positive answer.  The raw WALL flag retains the
                // conservative fallback for art the catalogue has not reached.
                if shape.facing.is_some() || flags.has(TileFlags::WALL) {
                    edges.union(crate::occlusion::named_edges(tile, &shape))
                } else {
                    edges
                }
            } else {
                edges
            }
        })
}

/// Whether a low wall is the vertical face below a platform.
///
/// Such a course has no inhabitable volume below its upper edge: it is the
/// riser of a stage, stair, or deck. It must not become a wall in either the
/// room graph or the facet-wide house contour. A normal wall made of short art
/// courses remains structural unless it directly supports that platform.
fn wall_supports_low_platform(
    map: &WorldMap,
    tiledata: &TileData,
    x: u16,
    y: u16,
    wall_z: i8,
    wall: &StaticTile,
) -> bool {
    let Some(floor_z) = map.average_land_z(x, y).map(i32::from) else {
        return false;
    };
    let top = i32::from(wall_z) + i32::from(wall.height);
    top - floor_z < i32::from(PLAYER_HEIGHT)
        && map.statics_at(x, y).any(|item| {
            let tile = tiledata.static_tile(item.tile.0);
            tile.flags.is_platform()
                && tile.flags.is_background()
                && i32::from(item.z)
                    + if tile.flags.is_climbable() {
                        i32::from(tile.height) / 2
                    } else {
                        i32::from(tile.height)
                    }
                    == top
        })
}

/// The same wall catalogue as [`wall_edges`], at the terrain floor band.
///
/// `BuildingMap` is deliberately a footprint bake: every storey of a house
/// shares the same exterior boundary, while storeys themselves are derived by a
/// later graph. A wall normally names a planar barrier; a low wall directly
/// supporting a platform is the riser of a stage and does not. In particular,
/// furniture carrying `BLOCK` alone is not considered here.
fn planar_wall_edges(tile: &openshard_uofiles::tiledata::StaticTile, shape: crate::occlusion::Shape) -> u8 {
    const WALLISH: u64 = TileFlags::WALL | TileFlags::NO_SHOOT;
    let flags = tile.flags;
    if !flags.has(WALLISH) || flags.is_roof() || flags.is_platform() || flags.has(TileFlags::DOOR) {
        return 0;
    }
    if !shape.facing.is_some() && !flags.has(TileFlags::WALL) {
        return 0;
    }
    let edges = crate::occlusion::named_edges(tile, &shape);
    // `named_edges` correctly makes an ordinary BACKGROUND tile a horizontal
    // lid.  Some legacy wall rows, however, carry BACKGROUND as well as WALL;
    // letting that render-only classification erase their boundary opened an
    // entire roofed shop at Britain 1433,1596 to the exterior flood.  The wall
    // bit is the stronger architectural statement here.  A measured face still
    // wins; a zero result is the conservative full-tile wall fallback.
    if edges.raw() != 0 {
        edges.raw()
    } else {
        crate::occlusion::Edges::ANY.raw()
    }
}

/// The index and opposite wall masks of every cardinal neighbour in a planar
/// facet.  It is intentionally separate from [`neighbours`]: the offline bake
/// addresses a flat raster in `usize`, not UO's `u16` point type.
fn planar_neighbours(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, u8, u8)> {
    [
        (
            y.checked_sub(1).map(|y| y * width + x),
            crate::occlusion::Edges::NORTH.raw(),
            crate::occlusion::Edges::SOUTH.raw(),
        ),
        (
            (x + 1 < width).then_some(y * width + x + 1),
            crate::occlusion::Edges::EAST.raw(),
            crate::occlusion::Edges::WEST.raw(),
        ),
        (
            (y + 1 < height).then_some((y + 1) * width + x),
            crate::occlusion::Edges::SOUTH.raw(),
            crate::occlusion::Edges::NORTH.raw(),
        ),
        (
            x.checked_sub(1).map(|x| y * width + x),
            crate::occlusion::Edges::WEST.raw(),
            crate::occlusion::Edges::EAST.raw(),
        ),
    ]
    .into_iter()
    .filter_map(|(index, side, opposite)| index.map(|index| (index, side, opposite)))
}

/// A closed-door graph omits the floor cell occupied by its leaf; the portal
/// below reconnects the two sides only when that leaf is open in the frame.
fn door_occupies(map: &WorldMap, tiledata: &TileData, cell: Cell) -> bool {
    map.statics_at(cell.tile.0, cell.tile.1).any(|item| {
        let tile = tiledata.static_tile(item.tile.0);
        if !tile.flags.has(TileFlags::DOOR) {
            return false;
        }
        let bottom = i32::from(item.z);
        let top = bottom + i32::from(tile.height.max(PLAYER_HEIGHT as u8));
        bottom < cell.floor_z + PLAYER_HEIGHT && cell.floor_z < top
    })
}

/// Floors for architectural topology, deliberately narrower than movement's
/// standable surfaces.
///
/// A character may stand on a table or a chest; neither is a storey.  The
/// interior index therefore takes land and static art explicitly marked FLOOR,
/// while movement continues to use every PLATFORM in `stand_surfaces`.
fn structural_surfaces(map: &WorldMap, tiledata: &TileData, x: u16, y: u16) -> Vec<i32> {
    let mut surfaces = Vec::new();
    if let Some(land) = map.land(x, y) {
        let flags = tiledata.land(land.tile.0).flags;
        if !flags.is_water() && !flags.is_blocking() {
            surfaces.push(i32::from(
                map.average_land_z(x, y).expect("land was just present"),
            ));
        }
    }
    for item in map.statics_at(x, y) {
        let tile = tiledata.static_tile(item.tile.0);
        if !tile.flags.is_platform() || !tile.flags.is_background() {
            continue;
        }
        let height = i32::from(tile.height);
        let top = i32::from(item.z)
            + if tile.flags.is_climbable() {
                height / 2
            } else {
                height
            };
        surfaces.push(top);
    }
    surfaces
}

/// Cardinally adjacent tiles, omitting map-underflow neighbours.
fn neighbours((x, y): (u16, u16)) -> impl Iterator<Item = (u16, u16)> {
    [
        x.checked_sub(1).map(|x| (x, y)),
        x.checked_add(1).map(|x| (x, y)),
        y.checked_sub(1).map(|y| (x, y)),
        y.checked_add(1).map(|y| (x, y)),
    ]
    .into_iter()
    .flatten()
}

/// Lazily baked, read-only interior facts of the map.
///
/// Map statics cannot change during a run, so every block is derived at most
/// once. Door *locations* are immutable and baked with the room graph; their
/// open/closed reachability remains a per-frame question.
#[derive(Default, Debug)]
pub struct Index {
    blocks: BTreeMap<BlockCoord, BlockRooms>,
}

impl Index {
    /// Return a cached cell block, baking its room graph from the read-only map
    /// on first use.
    pub fn block(&mut self, map: &WorldMap, tiledata: &TileData, id: BlockCoord) -> Option<&BlockCells> {
        Some(self.rooms(map, tiledata, id)?.cells())
    }

    /// Return a cached room graph, baking it from the read-only map on first
    /// use. Cells and rooms deliberately share this one cache entry: a second
    /// bake would give the two phases two opportunities to disagree about a
    /// wall or a doorway in the same map block.
    pub fn rooms(&mut self, map: &WorldMap, tiledata: &TileData, id: BlockCoord) -> Option<&BlockRooms> {
        self.rooms_with_shapes(map, tiledata, id, &|_| crate::occlusion::Shape::UNREAD)
    }

    /// Return a cached block baked with the install's measured wall faces.
    ///
    /// The shape source is immutable client data.  Callers must use one source
    /// for one `Index`: changing it would make a cached map fact stale.
    pub fn rooms_with_shapes(
        &mut self,
        map: &WorldMap,
        tiledata: &TileData,
        id: BlockCoord,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    ) -> Option<&BlockRooms> {
        if let Entry::Vacant(entry) = self.blocks.entry(id) {
            entry.insert(BlockRooms::bake_with_shapes(map, tiledata, id, shape_of)?);
        }
        self.blocks.get(&id)
    }

    /// Stitch a caller-selected set of cached blocks without treating their
    /// shared edges as walls.
    ///
    /// The result owns the immutable room graph, so baking another block later
    /// cannot change a frame that is still using this one.  Supplying the
    /// camera's complete inspected region is the caller's responsibility;
    /// [`StitchedRooms::bake`] deliberately does not guess at an absent block.
    pub fn stitched(
        &mut self,
        map: &WorldMap,
        tiledata: &TileData,
        ids: impl IntoIterator<Item = BlockCoord>,
    ) -> Option<StitchedRooms> {
        self.stitched_with_shapes(map, tiledata, ids, &|_| crate::occlusion::Shape::UNREAD)
    }

    /// Stitch blocks using the measured faces of the install's wall art.
    pub fn stitched_with_shapes(
        &mut self,
        map: &WorldMap,
        tiledata: &TileData,
        ids: impl IntoIterator<Item = BlockCoord>,
        shape_of: &dyn Fn(Graphic) -> crate::occlusion::Shape,
    ) -> Option<StitchedRooms> {
        let ids: BTreeSet<_> = ids.into_iter().collect();
        let mut blocks = Vec::with_capacity(ids.len());
        for id in ids {
            blocks.push(self.rooms_with_shapes(map, tiledata, id, shape_of)?.clone());
        }
        Some(StitchedRooms::bake_with_shapes(map, tiledata, blocks, shape_of))
    }

    /// Derive the indexed buildings and floors for a caller-selected map region.
    ///
    /// The durable work remains the per-block room cache; this is its immutable
    /// seam composition for one debug frame or, later, one interior frame.
    pub fn buildings(
        &mut self,
        map: &WorldMap,
        tiledata: &TileData,
        ids: impl IntoIterator<Item = BlockCoord>,
    ) -> Option<Buildings> {
        Some(Buildings::bake(
            map,
            tiledata,
            &self.stitched(map, tiledata, ids)?,
        ))
    }

    /// Look up the cell containing a point, baking only that point's block.
    pub fn cell_at(&mut self, map: &WorldMap, tiledata: &TileData, point: Point) -> Option<CellId> {
        let id = BlockCoord::containing(point.x, point.y);
        self.block(map, tiledata, id)?.cell_at(point)
    }

    /// How many map blocks have been baked in this process.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::{LandCell, LandTile, StaticItem};
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_uofiles::tiledata::{StaticTile, TileFlags};

    use super::*;

    fn walled_block(open: &[(u16, u16)]) -> (WorldMap, TileData) {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL),
                height: 20,
                ..StaticTile::default()
            },
        );
        for y in 0..8 {
            for x in 0..8 {
                if open.contains(&(x, y)) {
                    continue;
                }
                map.place_static(StaticItem {
                    tile: Graphic(1),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }
        (map, tiledata)
    }
    #[test]
    fn stacked_cells_are_structural_evidence_but_not_an_invented_walk() {
        let (mut map, mut tiledata) = walled_block(&[(1, 1)]);
        tiledata.set_static_tile(
            2,
            StaticTile {
                flags: TileFlags::new(TileFlags::FLOOR | TileFlags::PLATFORM),
                height: 16,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(2),
            x: 1,
            y: 1,
            z: 0,
            hue: Hue(0),
        });

        let block = BlockRooms::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("map block");
        let rooms = StitchedRooms::bake([block]);
        let lower = rooms
            .cells()
            .find(|cell| cell.tile == (1, 1) && cell.floor_z == 0)
            .expect("lower room cell");
        let upper = rooms
            .cells()
            .find(|cell| cell.tile == (1, 1) && cell.floor_z == 16)
            .expect("upper platform cell");

        let buildings = Buildings::bake(&map, &tiledata, &rooms);

        assert_eq!(buildings.building_at(lower.id), buildings.building_at(upper.id));
        assert_ne!(buildings.floor_at(lower.id), buildings.floor_at(upper.id));
    }

    #[test]
    fn an_upper_floor_makes_two_cells_in_one_column() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::FLOOR | TileFlags::PLATFORM),
                height: 20,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(1),
            x: 2,
            y: 3,
            z: 0,
            hue: Hue(0),
        });

        let cells = BlockCells::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("map block");
        let column: Vec<_> = cells
            .cells()
            .iter()
            .copied()
            .filter(|cell| cell.tile == (2, 3))
            .collect();

        assert_eq!(column.len(), 2);
        assert_eq!(column[0].floor_z, 0);
        assert_eq!(column[0].ceiling, Some(20));
        assert_eq!(column[1].floor_z, 20);
        assert!(column[1].outdoors());
        assert_eq!(cells.cell_at(Point::new(2, 3, 19)), Some(column[0].id));
        assert_eq!(cells.cell_at(Point::new(2, 3, 20)), Some(column[1].id));
    }

    #[test]
    fn a_roof_is_the_ceiling_of_the_house_cell_below_it() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::ROOF | TileFlags::NO_SHOOT),
                height: 0,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(1),
            x: 2,
            y: 3,
            z: 20,
            hue: Hue(0),
        });

        let cells = BlockCells::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("map block");
        let cell = cells
            .cells()
            .iter()
            .copied()
            .find(|cell| cell.tile == (2, 3) && cell.floor_z == 0)
            .expect("ground cell below roof");

        assert_eq!(cell.ceiling, Some(20));
        assert!(!cell.outdoors());
    }

    #[test]
    fn a_roofed_enclosure_is_an_indexed_building_area() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            2,
            StaticTile {
                flags: TileFlags::new(TileFlags::ROOF | TileFlags::NO_SHOOT),
                height: 0,
                ..StaticTile::default()
            },
        );
        for y in 1..=3 {
            for x in 1..=3 {
                if (x, y) == (2, 2) {
                    continue;
                }
                map.place_static(StaticItem {
                    tile: Graphic(1),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }
        map.place_static(StaticItem {
            tile: Graphic(2),
            x: 2,
            y: 2,
            z: 20,
            hue: Hue(0),
        });

        let block = BlockRooms::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("map block");
        let rooms = StitchedRooms::bake([block]);
        let centre = rooms
            .cells()
            .find(|cell| cell.tile == (2, 2) && cell.floor_z == 0)
            .expect("the room cell");
        let room = rooms.room_at(centre.id).expect("the room");
        assert!(
            !rooms
                .rooms()
                .iter()
                .any(|candidate| candidate.id == room && candidate.outdoors()),
            "the roofed centre must not merge with outdoor sky"
        );

        let buildings = Buildings::bake(&map, &tiledata, &rooms);
        assert!(buildings.building_at(centre.id).is_some());
    }

    #[test]
    fn open_sky_and_a_negative_floor_can_join_without_overflowing() {
        let block = BlockCoord { x: 0, y: 0 };
        let low = Cell {
            id: CellId { block, slot: 0 },
            tile: (0, 0),
            floor_z: i32::MIN,
            ceiling: None,
        };
        let high = Cell {
            id: CellId { block, slot: 1 },
            tile: (1, 0),
            floor_z: 0,
            ceiling: None,
        };

        assert!(bands_join(low, high));
    }

    #[test]
    fn a_measured_wall_blocks_only_its_named_shared_edge() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(1),
            x: 1,
            y: 1,
            z: 0,
            hue: Hue(0),
        });
        let block = BlockCoord { x: 0, y: 0 };
        let here = Cell {
            id: CellId { block, slot: 0 },
            tile: (1, 1),
            floor_z: 0,
            ceiling: None,
        };
        let north = Cell {
            id: CellId { block, slot: 1 },
            tile: (1, 0),
            floor_z: 0,
            ceiling: None,
        };
        let east = Cell {
            id: CellId { block, slot: 2 },
            tile: (2, 1),
            floor_z: 0,
            ceiling: None,
        };
        let north_wall =
            |_| crate::occlusion::Shape::faced(crate::facing::Facing::One(crate::facing::Face::North));

        assert!(!cells_join(&map, &tiledata, &north_wall, here, north));
        assert!(cells_join(&map, &tiledata, &north_wall, here, east));
    }

    #[test]
    fn facet_bake_marks_space_unreachable_from_the_world_as_a_building() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        // A wall loop around a three-by-three ground area.  No roof, platform,
        // or movement surface participates in this test: the wall topology is
        // the sole reason the centre is positive space.
        for y in 1..=5 {
            for x in 1..=5 {
                if x == 1 || x == 5 || y == 1 || y == 5 {
                    map.place_static(StaticItem {
                        tile: Graphic(1),
                        x,
                        y,
                        z: 0,
                        hue: Hue(0),
                    });
                }
            }
        }

        let graph = BuildingMap::bake(&map, &tiledata, &|_| crate::occlusion::Shape::UNREAD);

        assert!(
            graph.building_at(3, 3).is_some(),
            "enclosed ground is positive space"
        );
        assert_eq!(graph.building_at(0, 0), None, "map boundary is exterior");
        assert_eq!(graph.building_at(1, 3), None, "a wall is not a painted floor");
    }

    #[test]
    fn a_background_tag_cannot_turn_a_wall_into_open_world() {
        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::WALL | TileFlags::FLOOR | TileFlags::NO_SHOOT),
            height: 20,
            ..StaticTile::default()
        };

        assert_eq!(
            planar_wall_edges(&wall, crate::occlusion::Shape::UNREAD),
            crate::occlusion::Edges::ANY.raw(),
            "WALL outranks BACKGROUND for the exterior flood"
        );
    }

    #[test]
    fn a_short_wall_on_a_stage_is_not_a_room_or_building_boundary() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 20,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            0x003E,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 10,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            1240,
            StaticTile {
                flags: TileFlags::new(TileFlags::FLOOR | TileFlags::NO_SHOOT | TileFlags::PLATFORM),
                height: 0,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(0x003E),
            x: 1,
            y: 1,
            z: 20,
            hue: Hue(0),
        });
        map.place_static(StaticItem {
            tile: Graphic(1240),
            x: 1,
            y: 1,
            z: 30,
            hue: Hue(0),
        });

        let topology = PlanarTopology::bake(&map, &tiledata, &|_| crate::occlusion::Shape::UNREAD);
        assert!(!topology.wall_tiles[9], "a stage riser is not a house contour");
        let cell = Cell {
            id: CellId {
                block: BlockCoord { x: 0, y: 0 },
                slot: 9,
            },
            tile: (1, 1),
            floor_z: 20,
            ceiling: Some(56),
        };
        assert_eq!(
            wall_edges(&map, &tiledata, &|_| crate::occlusion::Shape::UNREAD, cell),
            crate::occlusion::Edges::NONE,
            "the stage wall does not divide the room either"
        );
    }

    #[test]
    fn a_doorway_is_coloured_as_part_of_its_positive_building() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            2,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::DOOR),
                height: 20,
                ..StaticTile::default()
            },
        );
        for y in 1..=5 {
            for x in 1..=5 {
                if x != 1 && x != 5 && y != 1 && y != 5 {
                    continue;
                }
                map.place_static(StaticItem {
                    tile: Graphic(if (x, y) == (3, 1) { 2 } else { 1 }),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }

        let graph = BuildingMap::bake(&map, &tiledata, &|_| crate::occlusion::Shape::UNREAD);

        assert_eq!(graph.building_at(3, 1), graph.building_at(3, 3));
        assert!(graph.building_at(3, 1).is_some());
    }

    #[test]
    fn a_double_gap_between_wall_frames_is_a_door_anchor() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            0x00AD,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            0x00AB,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::NO_SHOOT),
                height: 20,
                ..StaticTile::default()
            },
        );
        for y in 1..=5 {
            for x in 1..=5 {
                if x != 1 && x != 5 && y != 1 && y != 5 || (y, x) == (1, 3) || (y, x) == (1, 4) {
                    continue;
                }
                map.place_static(StaticItem {
                    tile: Graphic(match (x, y) {
                        (2, 1) => 0x00AD,
                        (5, 1) => 0x00AB,
                        _ => 1,
                    }),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }

        let graph = BuildingMap::bake(&map, &tiledata, &|_| crate::occlusion::Shape::UNREAD);

        assert_eq!(graph.building_at(3, 1), graph.building_at(3, 3));
        assert_eq!(graph.building_at(4, 1), graph.building_at(3, 3));
    }

    #[test]
    fn blocking_furniture_does_not_split_a_room() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::BLOCK | TileFlags::PLATFORM),
                height: 12,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(1),
            x: 1,
            y: 1,
            z: 0,
            hue: Hue(0),
        });

        let rooms = BlockRooms::bake_with_shapes(&map, &tiledata, BlockCoord { x: 0, y: 0 }, &|_| {
            crate::occlusion::Shape::UNREAD
        })
        .expect("map block");
        let table = rooms
            .cells()
            .cells()
            .iter()
            .copied()
            .find(|cell| cell.tile == (1, 1) && cell.floor_z == 0)
            .expect("floor below table");
        assert!(
            rooms
                .cells()
                .cells()
                .iter()
                .all(|cell| cell.tile != (1, 1) || cell.floor_z == 0),
            "a table top is not an interior floor"
        );
        let neighbour = rooms
            .cells()
            .cells()
            .iter()
            .copied()
            .find(|cell| cell.tile == (1, 0) && cell.floor_z == 0)
            .expect("adjacent floor");

        assert_eq!(rooms.room_at(table.id), rooms.room_at(neighbour.id));
    }

    #[test]
    fn an_off_map_block_is_not_baked() {
        let map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        assert!(BlockCells::bake(&map, &TileData::empty(), BlockCoord { x: 1, y: 0 }).is_none());
    }

    #[test]
    fn a_point_bakes_its_block_once() {
        let map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let tiledata = TileData::empty();
        let mut index = Index::default();

        assert!(index.cell_at(&map, &tiledata, Point::new(2, 3, 0)).is_some());
        assert!(index.cell_at(&map, &tiledata, Point::new(6, 7, 0)).is_some());
        assert_eq!(index.block_count(), 1);
    }

    #[test]
    fn an_open_block_is_not_indexed_as_a_building() {
        let map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let tiledata = TileData::empty();
        let mut index = Index::default();

        let buildings = index
            .buildings(&map, &tiledata, [BlockCoord { x: 0, y: 0 }])
            .expect("map block");

        assert!(buildings.buildings().is_empty());
        assert_eq!(index.block_count(), 1);
    }

    #[test]
    fn a_low_ceiling_is_not_a_cell() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::FLOOR | TileFlags::PLATFORM),
                height: 12,
                ..StaticTile::default()
            },
        );
        map.place_static(StaticItem {
            tile: Graphic(1),
            x: 2,
            y: 3,
            z: 0,
            hue: Hue(0),
        });

        let cells = BlockCells::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("map block");
        let column: Vec<_> = cells
            .cells()
            .iter()
            .copied()
            .filter(|cell| cell.tile == (2, 3))
            .collect();

        assert_eq!(column.len(), 1);
        assert_eq!(column[0].floor_z, 12);
        assert!(column[0].outdoors());
    }

    #[test]
    fn a_door_is_a_closed_wall_and_a_portal_between_its_two_rooms() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            2,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::DOOR),
                height: 20,
                ..StaticTile::default()
            },
        );
        for y in 0..8 {
            for x in 0..8 {
                let graphic = match (x, y) {
                    // Two floor cells with a shut doorway between them.
                    (1, 3) | (3, 3) => continue,
                    (2, 3) => 2,
                    _ => 1,
                };
                map.place_static(StaticItem {
                    tile: Graphic(graphic),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }

        let rooms = BlockRooms::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("map block");

        assert_eq!(rooms.rooms().len(), 2, "a shut door does not merge its rooms");
        assert_eq!(
            rooms.portals().len(),
            1,
            "the door records the two rooms it can join"
        );
        let portal = rooms.portals()[0];
        assert_ne!(portal.rooms[0], portal.rooms[1]);
        for room in portal.rooms {
            assert_eq!(
                rooms.rooms()[usize::try_from(room.slot).unwrap()].cells().len(),
                1
            );
        }
    }

    #[test]
    fn an_open_portal_reveals_a_sealed_room_but_a_shut_one_does_not() {
        let block = BlockCoord { x: 0, y: 0 };
        let outside = RoomId { block, slot: 0 };
        let sealed = RoomId { block, slot: 1 };
        let door = Door {
            id: DoorId { block, slot: 0 },
            at: Point::new(1, 0, 0),
            graphic: Graphic(2),
        };
        let rooms = BlockRooms {
            cells: BlockCells {
                id: block,
                cells: Vec::new(),
                doors: vec![door],
            },
            rooms: vec![
                Room {
                    id: outside,
                    cells: Vec::new(),
                    outdoors: true,
                },
                Room {
                    id: sealed,
                    cells: Vec::new(),
                    outdoors: false,
                },
            ],
            room_of: Vec::new(),
            portals: vec![Portal {
                door: door.id,
                rooms: [outside, sealed],
            }],
        };

        assert_eq!(rooms.shown_rooms(None, |_| false), BTreeSet::from([outside]));
        assert_eq!(
            rooms.shown_rooms(None, |_| true),
            BTreeSet::from([outside, sealed])
        );
    }

    #[test]
    fn a_door_and_its_rooms_cross_an_eight_tile_block_seam() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 2, down: 1 }, |_, _| LandCell {
            tile: LandTile(0),
            z: 0,
        });
        let mut tiledata = TileData::empty();
        tiledata.set_static_tile(
            1,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            2,
            StaticTile {
                flags: TileFlags::new(TileFlags::WALL | TileFlags::DOOR),
                height: 20,
                ..StaticTile::default()
            },
        );
        tiledata.set_static_tile(
            3,
            StaticTile {
                flags: TileFlags::new(TileFlags::FLOOR | TileFlags::PLATFORM),
                height: 20,
                ..StaticTile::default()
            },
        );
        for y in 0..8 {
            for x in 0..16 {
                if ![(6, 3), (7, 3), (8, 3), (9, 3)].contains(&(x, y)) {
                    map.place_static(StaticItem {
                        tile: Graphic(1),
                        x,
                        y,
                        z: 0,
                        hue: Hue(0),
                    });
                }
            }
        }
        // Each platform gives its tile a sealed ground cell.  The right side
        // also reaches the open tile at (9, 3); the left side can only reach it
        // through the door at (7, 3), which lies at the block edge.
        for x in [6, 8] {
            map.place_static(StaticItem {
                tile: Graphic(3),
                x,
                y: 3,
                z: 0,
                hue: Hue(0),
            });
        }
        map.place_static(StaticItem {
            tile: Graphic(2),
            x: 7,
            y: 3,
            z: 0,
            hue: Hue(0),
        });

        let left = BlockRooms::bake(&map, &tiledata, BlockCoord { x: 0, y: 0 }).expect("left block");
        let right = BlockRooms::bake(&map, &tiledata, BlockCoord { x: 1, y: 0 }).expect("right block");
        assert!(
            left.portals().is_empty(),
            "the other side is beyond this local bake"
        );

        let rooms = StitchedRooms::bake([left, right]);
        assert_eq!(rooms.portals().len(), 1, "the seam doorway has both sides");
        let left = rooms
            .room_at(CellId {
                block: BlockCoord { x: 0, y: 0 },
                slot: 30,
            })
            .expect("left ground cell");
        // Resolve this from the baked data rather than treating a block-local
        // slot as a map-coordinate formula: the platform creates a second cell
        // in this column.
        let right_cell = rooms
            .rooms()
            .iter()
            .flat_map(StitchedRoom::cells)
            .copied()
            .find(|cell| cell.block == BlockCoord { x: 1, y: 0 })
            .expect("right room cell");
        let right = rooms.room_at(right_cell).expect("right room");
        assert_ne!(left, right);

        let shut = rooms.shown_rooms(None, |_| false);
        assert!(shut.contains(&right));
        assert!(!shut.contains(&left), "the sealed left room stays hidden");
        let open = rooms.shown_rooms(None, |_| true);
        assert!(open.contains(&right));
        assert!(
            open.contains(&left),
            "opening the seam door reveals the left room"
        );
    }

    #[test]
    fn a_low_supported_platform_is_a_child_of_its_surrounding_room() {
        let block = BlockCoord { x: 0, y: 0 };
        let base = CellId { block, slot: 0 };
        let platform = CellId { block, slot: 1 };
        let base_room = StitchedRoomId {
            root: RoomId { block, slot: 0 },
        };
        let platform_room = StitchedRoomId {
            root: RoomId { block, slot: 1 },
        };
        let building = BuildingId { root: base };
        let ground = FloorId { building, slot: 0 };
        let raised = FloorId { building, slot: 1 };
        let rooms = StitchedRooms {
            cells: BTreeMap::from([
                (
                    base,
                    Cell {
                        id: base,
                        tile: (2, 3),
                        floor_z: 20,
                        ceiling: Some(30),
                    },
                ),
                (
                    platform,
                    Cell {
                        id: platform,
                        tile: (2, 3),
                        floor_z: 30,
                        ceiling: Some(65),
                    },
                ),
            ]),
            rooms: vec![
                StitchedRoom {
                    id: base_room,
                    cells: vec![base],
                    outdoors: false,
                },
                StitchedRoom {
                    id: platform_room,
                    cells: vec![platform],
                    outdoors: false,
                },
            ],
            room_of: BTreeMap::from([(base, base_room), (platform, platform_room)]),
            doors: BTreeMap::new(),
            portals: Vec::new(),
        };
        let tree = RoomTree::bake(
            &rooms,
            &BTreeMap::from([(base, building), (platform, building)]),
            &BTreeMap::from([(base, ground), (platform, raised)]),
        );

        assert_eq!(
            tree.node(RoomNodeId::Room(platform_room))
                .expect("the platform room is indexed")
                .parent(),
            Some(RoomNodeId::Room(base_room))
        );
        assert_eq!(tree.enclosing_room(platform_room), base_room);
        assert_eq!(tree.floor_of_room(platform_room), Some(ground));
        let mut visible = BTreeSet::from([platform_room]);
        tree.expand_visible(&mut visible);
        assert_eq!(visible, BTreeSet::from([base_room, platform_room]));

        let buildings = Buildings {
            buildings: vec![Building {
                id: building,
                floors: vec![
                    Floor {
                        id: ground,
                        cells: vec![base],
                        min_z: 20,
                        max_z: 20,
                    },
                    Floor {
                        id: raised,
                        cells: vec![platform],
                        min_z: 30,
                        max_z: 30,
                    },
                ],
            }],
            building_of: BTreeMap::from([(base, building), (platform, building)]),
            floor_of: BTreeMap::from([(base, ground), (platform, raised)]),
            stairs: Vec::new(),
            rooms: tree,
        };
        let frame = InteriorFrame::at(&buildings, &rooms, Some(platform), FloorView::Auto, |_| false)
            .expect("the platform is inside the indexed building");
        assert_eq!(frame.selected_floors(), &BTreeSet::from([ground]));
        assert!(frame.shows_cell(base));
        assert!(frame.shows_cell(platform));
    }

    /// A deliberately small completed index used to pin R2's frame policy
    /// independently of the expensive map bake. `outside` stands at the
    /// building's open entrance; `sealed` is behind its portal; `upper` is the
    /// next structural floor of that sealed room.
    fn indexed_picture() -> (Buildings, StitchedRooms, CellId, CellId, CellId) {
        let block = BlockCoord { x: 0, y: 0 };
        let outside = CellId { block, slot: 0 };
        let sealed = CellId { block, slot: 1 };
        let upper = CellId { block, slot: 2 };
        let outside_room = StitchedRoomId {
            root: RoomId { block, slot: 0 },
        };
        let sealed_room = StitchedRoomId {
            root: RoomId { block, slot: 1 },
        };
        let door = Door {
            id: DoorId { block, slot: 0 },
            at: Point::new(1, 0, 0),
            graphic: Graphic(1),
        };
        let rooms = StitchedRooms {
            cells: BTreeMap::from([
                (
                    outside,
                    Cell {
                        id: outside,
                        tile: (0, 0),
                        floor_z: 0,
                        ceiling: None,
                    },
                ),
                (
                    sealed,
                    Cell {
                        id: sealed,
                        tile: (2, 0),
                        floor_z: 0,
                        ceiling: Some(20),
                    },
                ),
                (
                    upper,
                    Cell {
                        id: upper,
                        tile: (2, 0),
                        floor_z: 20,
                        ceiling: None,
                    },
                ),
            ]),
            rooms: vec![
                StitchedRoom {
                    id: outside_room,
                    cells: vec![outside],
                    outdoors: true,
                },
                StitchedRoom {
                    id: sealed_room,
                    cells: vec![sealed, upper],
                    outdoors: false,
                },
            ],
            room_of: BTreeMap::from([
                (outside, outside_room),
                (sealed, sealed_room),
                (upper, sealed_room),
            ]),
            doors: BTreeMap::from([(door.id, door)]),
            portals: vec![StitchedPortal {
                door: door.id,
                rooms: [outside_room, sealed_room],
            }],
        };
        let building = BuildingId { root: outside };
        let ground = FloorId { building, slot: 0 };
        let first = FloorId { building, slot: 1 };
        let buildings = Buildings {
            buildings: vec![Building {
                id: building,
                floors: vec![
                    Floor {
                        id: ground,
                        cells: vec![outside, sealed],
                        min_z: 0,
                        max_z: 0,
                    },
                    Floor {
                        id: first,
                        cells: vec![upper],
                        min_z: 20,
                        max_z: 20,
                    },
                ],
            }],
            building_of: BTreeMap::from([(outside, building), (sealed, building), (upper, building)]),
            floor_of: BTreeMap::from([(outside, ground), (sealed, ground), (upper, first)]),
            stairs: Vec::new(),
            rooms: RoomTree::default(),
        };
        (buildings, rooms, outside, sealed, upper)
    }

    #[test]
    fn an_interior_frame_blacks_a_sealed_room_until_its_door_opens() {
        let (buildings, rooms, outside, sealed, upper) = indexed_picture();
        let shut = InteriorFrame::at(&buildings, &rooms, Some(outside), FloorView::Auto, |_| false)
            .expect("player is in an indexed building");
        assert!(shut.applies_to(sealed));
        assert!(shut.shows_cell(outside));
        assert!(!shut.shows_cell(sealed), "the sealed ground room is black");
        assert!(!shut.shows_cell(upper), "Auto stops at the player's floor");

        let open = InteriorFrame::at(&buildings, &rooms, Some(outside), FloorView::Auto, |_| true)
            .expect("player is in an indexed building");
        assert!(open.shows_cell(sealed), "the open portal reaches the sealed room");
        assert_ne!(
            shut.fingerprint(),
            open.fingerprint(),
            "a cache must not reuse the shut picture"
        );
    }

    #[test]
    fn an_interior_frame_hides_other_buildings_but_keeps_its_own() {
        let (buildings, rooms, outside, _sealed, _upper) = indexed_picture();
        let frame = InteriorFrame::at(&buildings, &rooms, Some(outside), FloorView::Auto, |_| true)
            .expect("player is in an indexed building")
            .with_other_buildings_hidden(
                BuildingMap::from_labels(5, 1, vec![1, 0, 1, 0, 2]).expect("one label per tile"),
                1,
            );

        assert!(
            frame.shows_at(Point::new(0, 0, 0)),
            "the current building remains visible"
        );
        assert!(frame.shows_at(Point::new(1, 0, 0)), "the street remains visible");
        assert!(
            frame.shows_at(Point::new(2, 0, 0)),
            "every cell of the current building remains visible"
        );
        assert!(
            !frame.shows_at(Point::new(4, 0, 0)),
            "the positive space of another building is hidden"
        );

        let wall = StaticTile {
            flags: TileFlags::new(TileFlags::WALL),
            ..StaticTile::default()
        };
        assert!(
            frame.shows_static_at(Point::new(1, 0, 0), &wall),
            "the current building's wall remains visible"
        );
        assert!(
            !frame.shows_static_at(Point::new(3, 0, 0), &wall),
            "the unlabelled wall contour of another building is hidden"
        );

        let overhang = InteriorFrame::at(&buildings, &rooms, Some(outside), FloorView::Auto, |_| true)
            .expect("player is in an indexed building")
            .with_other_buildings_hidden(
                BuildingMap::from_labels(5, 1, vec![1, 0, 0, 0, 2]).expect("one label per tile"),
                1,
            );
        assert!(
            !overhang.shows_static_at(Point::new(2, 0, 0), &wall),
            "a roof or wall anchored two tiles beyond another building's floor is hidden"
        );
    }

    #[test]
    fn a_z_slice_draws_exactly_its_selected_band() {
        let auto = InteriorFrame::z_slice(Point::new(100, 100, 20), ZSliceView::Auto);
        assert_eq!(auto.z_range(), Some((20, 40)));
        assert!(!auto.shows_at(Point::new(100, 100, 19)));
        assert!(auto.shows_at(Point::new(100, 100, 20)));
        assert!(auto.shows_at(Point::new(100, 100, 40)));
        assert!(!auto.shows_at(Point::new(100, 100, 41)));

        let below = InteriorFrame::z_slice(
            Point::new(100, 100, 20),
            ZSliceView::Manual { lower: -20, upper: 0 },
        );
        assert_eq!(below.z_range(), Some((-20, 0)));
        assert_ne!(auto.fingerprint(), below.fingerprint());
    }

    #[test]
    fn an_outside_frame_blacks_positive_building_space_but_keeps_the_street() {
        let buildings = BuildingMap::from_labels(2, 1, vec![0, 1]).expect("one label per tile");
        let outside = InteriorFrame::outside(buildings);
        assert!(outside.shows_at(Point::new(0, 0, 0)), "street remains visible");
        assert!(
            !outside.shows_at(Point::new(1, 0, 0)),
            "the building's interior is absent from an exterior picture"
        );
    }

    #[test]
    fn an_exterior_guard_and_z_band_intersect_but_keep_house_skin() {
        let buildings = BuildingMap::from_labels(2, 1, vec![0, 1]).expect("one label per tile");
        let frame = InteriorFrame::outside(buildings)
            .with_z_slice(Point::new(0, 0, 0), ZSliceView::Manual { lower: 0, upper: 20 });
        assert!(
            !frame.shows_at(Point::new(1, 0, 10)),
            "the band does not reveal interior contents"
        );
        assert!(
            !frame.shows_at(Point::new(0, 0, 21)),
            "outside the z band is black too"
        );

        let roof = StaticTile {
            flags: TileFlags::new(TileFlags::ROOF),
            ..StaticTile::default()
        };
        let window = StaticTile {
            flags: TileFlags::new(TileFlags::WINDOW),
            ..StaticTile::default()
        };
        assert!(frame.shows_static_at(Point::new(1, 0, 10), &roof));
        assert!(frame.shows_static_at(Point::new(1, 0, 10), &window));
        assert!(!frame.shows_static_at(Point::new(1, 0, 21), &roof));
    }

    #[test]
    fn an_interior_frame_never_blacks_the_players_room_and_selects_real_floors() {
        let (buildings, rooms, _outside, sealed, upper) = indexed_picture();
        let inside = InteriorFrame::at(&buildings, &rooms, Some(sealed), FloorView::Auto, |_| false)
            .expect("player is in an indexed building");
        assert!(
            inside.shows_cell(sealed),
            "the player is a second reachability source"
        );
        assert!(!inside.shows_cell(upper), "Auto leaves the storey above closed");

        let raised = InteriorFrame::at(
            &buildings,
            &rooms,
            Some(sealed),
            FloorView::Manual { relative: 99 },
            |_| false,
        )
        .expect("player is in an indexed building");
        assert!(raised.shows_cell(sealed));
        assert!(
            raised.shows_cell(upper),
            "the manual level resolves to the actual upper FloorId"
        );
        assert_eq!(raised.selected_floors().len(), 2, "floors below stay visible");
    }
}
