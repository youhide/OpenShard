//! Read-only questions about the world and the frame — everything a panel or
//! a highlight needs answered and nothing that answers one by writing
//! anything back, except [`App::apply`], which is the one place a frame's
//! worth of the HUD's requests are folded in.
//!
//! [`App::tile_info`] and [`App::tile_ring`] are the Tile panel's own
//! column, read straight from the map; [`App::pick_tile`] and
//! [`App::resolve_selection`] turn a cursor or a frozen identity into one.
//! [`App::hud`] gathers all of it, plus [`App::perf`], into the snapshot the
//! shell draws from — the frame's answer to "what are the panels allowed to
//! know".

use openshard_client_render::bench::{self, Metrics};
use openshard_client_render::camera::{self, Camera, TileBounds};
use openshard_client_render::control::Follow;
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::depth;
use openshard_client_render::mobiles::{self, Mobile};
use openshard_client_render::{light, occlusion};
use openshard_map::grid::Tile;
use openshard_map::map::WorldMap;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Point;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::crowd::Who;
use crate::diagnostics::{
    CompositeTelemetry, HealthBar, HealthPoints, Height, Hud, InteriorCell, InteriorDoor, InteriorOverlay,
    OccluderSurface, Pick, PickedItem, PickedMobile, PickedTile, PriorityZ, RadarTelemetry, Route, Selection,
    TerrainOverlay, TileDepth,
};
use crate::graphics::HighlightTarget;
use crate::picking::SelectedIdentity;
use crate::world::{InteriorCache, footing, guide, terrain};
use crate::{desk, frames, shell, steer, tooltips};

/// The expensive sub-queries performed while assembling the development HUD.
/// These are diagnostic timings only; they deliberately do not change the HUD
/// snapshot's shape or the order in which its readers see the world.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HudTimings {
    pub terrain: Duration,
    pub route: Duration,
    pub occluders: Duration,
    pub picking: Duration,
    pub perf: Duration,
}

impl App {
    /// Common code for the two lookups in [`App::pick_tile`]: `unproject` hands
    /// back a signed pair that may be off the map in any direction, and a
    /// negative one is not expressible as the `u16` [`WorldMap::land`] wants.
    pub(crate) fn in_bounds(x: i32, y: i32, map: &WorldMap) -> Option<Tile> {
        if x < 0 || y < 0 || x as u32 >= map.width() || y as u32 >= map.height() {
            return None;
        }
        Some(Tile::new(x as u16, y as u16))
    }

    /// Everything the Tile panel shows about one tile, read straight from the
    /// map. Shared by the live hover and a click's frozen selection, so the two
    /// can never disagree about what a tile contains.
    pub(crate) fn tile_info(&self, tile: Tile) -> PickedTile {
        let x = tile.x;
        let y = tile.y;
        let land = self.resources.map().land(x, y);
        let statics = self
            .resources
            .map()
            .statics_at(x, y)
            .map(|item| {
                let priority_z =
                    depth::static_priority_z(item.z, self.resources.tiledata.static_tile(item.tile.0));
                (item.tile, Height(item.z), item.hue, PriorityZ(priority_z))
            })
            .collect();
        // A server item (the shard's own decoration, not the client's map art)
        // sorts exactly like a static — `statics::place` reads it through the
        // same `depth::static_priority_z` — but lives in a different list:
        // `self.resources.map().statics_at` only ever answers from the client's own files,
        // so a sign or a prop the shard's script placed is invisible to it.
        // Missing it here is what let a static-only panel misname what was
        // actually drawing over a mobile on screen.
        let items = self
            .world
            .presentation
            .items
            .iter()
            .filter(|item| item.at.x == x && item.at.y == y)
            .map(|item| {
                // The drawn graphic, not the shard's: this is what the frame
                // put on the screen, and both the sort and the answer have to
                // be about that picture. See `GroundItem::displayed`.
                let priority_z = depth::static_priority_z(
                    item.at.z,
                    self.resources.tiledata.static_tile(item.displayed().0),
                );
                (
                    item.displayed(),
                    Height(item.at.z),
                    item.hue,
                    PriorityZ(priority_z),
                )
            })
            .collect();
        let tile_depth = TileDepth(depth::base_for(i32::from(x), i32::from(y)));
        // Whichever drawn mobile — our own body included — is standing on this
        // exact tile right now, so its order can be read against the statics
        // above it: this is the comparison that tells "the coarse per-tile
        // scheme is doing what it was ported to do" apart from "something here
        // computed the wrong tile or z".
        let mobile_order = self
            .drawn_mobiles()
            .into_iter()
            .find(|(_, mobile)| mobile.at.x == x && mobile.at.y == y)
            .map(|(_, mobile)| depth::Order {
                tile: depth::mobile_tile(mobile.at, mobile.from),
                priority_z: depth::mobile_priority_z(mobile.at.z),
            });
        // The height anything drawn *on* this tile belongs at: the surface a body
        // would stand on, not the ground under it. On a pier those are thirteen
        // z-units apart — the land is water at -15 and the planks are at -3 — and
        // a marker drawn at the land's height sits a tile and a half down the
        // screen from the boards it is meant to be lying on, which is what made
        // the cursor unable to hit a pier tile at all. `predict_z` is the same
        // "which surface, coming from here" the walk itself uses, asked from the
        // body's own height so a floor overhead does not win over the street.
        let terrain = terrain(&self.resources);
        let stand = terrain.predict_z(x, y, i32::from(self.world.motion.planning_state().position.z));
        // Clamped rather than unwrapped: a `z` outside `i8` is a corrupt
        // block, and a diamond at the wrong height beats a panic in a HUD.
        let stand_z = stand.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
        // The shape of the surface being marked, and the decision belongs here
        // rather than in the painter: only the map knows whether the height a
        // body stands at is the land's own — in which case the surface is a
        // sloped quad and the marker has to be too — or the flat top of a
        // platform standing on it.
        //
        // `average_land_z` is the same number `predict_z` pushed as the land's
        // candidate, so this is a comparison of one arithmetic against itself
        // rather than a re-derivation. A platform whose deck happens to sit at
        // exactly the land's average height is drawn sloped; it is level ground
        // wherever that coincidence is not one, and a corner off by a unit or
        // two is a better wrong answer than a marker that ignores the hill.
        let corners = match self.resources.map().average_land_z(x, y) == Some(stand_z) {
            // `land_corners` reads top, right, *left*, bottom, and the facet
            // wants top, right, bottom, left — swapping the pair is what keeps
            // the quad from being a bow tie.
            true => match self.resources.map().land_corners(x, y) {
                Some([top, right, left, bottom]) => [top, right, bottom, left],
                None => [stand_z; 4],
            },
            false => [stand_z; 4],
        };
        // The same clamp, and the same reason: a corrupt block may name a height
        // no `i8` holds, and a level drawn at the edge of the world beats a
        // panic in a HUD.
        let drawn_z = |z: i32| z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
        // Whether a body fits is asked of the *cluttered* terrain — the client's
        // map with the shard's items laid over it — because that is what every
        // step decision on this end asks. A private "can I stand here" written
        // for the marker would be a second policy, and the first bug it hid
        // would be one of its own. The surfaces themselves come from the map:
        // where a floor *is* is a fact about the facet, and only whether a body
        // fits on it depends on what has been put there since.
        let cluttered = footing(&self.resources, openshard_map::overlay::Doors::AsTheyStand);
        let mut levels: Vec<(Height, bool)> = terrain
            .surfaces(x, y)
            .into_iter()
            .map(|z| {
                let fits =
                    openshard_movement::can_fit(&cluttered, tile, z, openshard_movement::PLAYER_HEIGHT);
                (Height(drawn_z(z)), fits)
            })
            .collect();
        // Sorted so the diagram reads bottom to top, and deduplicated because a
        // tile can carry two statics whose decks land on the same height — two
        // diamonds drawn on one line are one line drawn twice.
        levels.sort_unstable();
        levels.dedup();
        PickedTile {
            at: tile,
            land: land.map(|cell| Graphic(cell.tile.0)),
            land_z: Height(land.map_or(0, |cell| cell.z)),
            stand_z: Height(stand_z),
            corners: corners.map(Height),
            levels,
            ceiling: terrain.ceiling(x, y).map(drawn_z).map(Height),
            statics,
            items,
            tile_depth,
            mobile_order,
        }
    }

    /// The eight tiles around one, for the wireframe the HUD draws beside the
    /// marker.
    ///
    /// A box on its own says how high its tile is; a box among its neighbours
    /// says which way the ground *runs*, which is the question actually being
    /// asked while looking for the reason a step was refused or a marker sits
    /// where it does. The ring is what makes the relief readable — a stair's
    /// tread against its riser, a cliff edge one tile from level ground.
    ///
    /// Off the map is simply absent: `checked_add`/`checked_sub` at the world's
    /// corner, and [`WorldMap::land`](openshard_map::map::WorldMap::land) answers
    /// nothing for a block that never loaded, which `tile_info` already reports
    /// as `land: None`.
    ///
    /// Eight tiles and not a radius: each of these costs a `predict_z` and the
    /// statics list under it, per frame, and eight is what a slope needs to be
    /// legible. A wider ring is the terrain overlay's job, and it has one.
    pub(crate) fn tile_ring(&self, centre: &PickedTile) -> Vec<PickedTile> {
        let mut ring = Vec::with_capacity(8);
        for dy in [-1i32, 0, 1] {
            for dx in [-1i32, 0, 1] {
                if (dx, dy) == (0, 0) {
                    continue;
                }
                let x = i32::from(centre.at.x) + dx;
                let y = i32::from(centre.at.y) + dy;
                if let Some(tile) = Self::in_bounds(x, y, self.resources.map()) {
                    ring.push(self.tile_info(tile));
                }
            }
        }
        ring
    }

    /// What tile the cursor is over, read straight from the map.
    ///
    /// `unproject` needs the height the pixel is meant to be read at, and the
    /// ground is not flat — so this picks once at the player's height to find
    /// a candidate tile, then re-picks at *that* tile's own height, which is
    /// exact wherever the two tiles agree and wrong only at a slope's edge,
    /// same as the client's own click-to-walk.
    ///
    /// That height is the *surface*, not the land: a pier's planks stand at `-3`
    /// over water at `-15`, and reading the pixel at the water's height resolved
    /// every pier tile to one more than a tile away — the cursor could not be
    /// put on the boards at all, which is what this is written against. The
    /// same `predict_z` the walk uses, so the tile the cursor names and the tile
    /// a step lands on are one answer rather than two.
    ///
    /// `camera` is the frame's own and not `self.control`'s, for the reason
    /// [`App::frame_facts`] takes one: what tile a pixel is over is a question
    /// about the picture being drawn, and reading it from a camera that has
    /// moved since is how the highlight ends up a frame away from the ground
    /// under it.
    pub(crate) fn pick_tile(&self, camera: Camera) -> Option<PickedTile> {
        let cursor = self.control.cursor();
        let world_px = camera.pick(cursor);
        let planning = self.world.motion.planning_state();
        let near = i32::from(planning.position.z);
        let (mut x, mut y) = camera::unproject(world_px, planning.position.z);
        if let Some(tile) = Self::in_bounds(x, y, self.resources.map()) {
            let terrain = terrain(&self.resources);
            let z = terrain.predict_z(tile.x, tile.y, near);
            let z = z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
            (x, y) = camera::unproject(world_px, z);
        }
        let tile = Self::in_bounds(x, y, self.resources.map())?;
        Some(self.tile_info(tile))
    }

    /// [`App::selected`] turned into what the panel actually shows — built
    /// fresh every frame from the identity a click froze, the same way
    /// [`App::tile_info`] always re-reads a column rather than remembering
    /// one. A moving mobile's row this way keeps up with it; a picked-up
    /// item's row goes away instead of the panel quietly lying about where it
    /// still is.
    pub(crate) fn resolve_selection(&self, identity: SelectedIdentity) -> Selection {
        match identity {
            SelectedIdentity::Tile { x, y } => Selection::Tile(self.tile_info(Tile::new(x, y))),
            SelectedIdentity::Static(picked) => Selection::Static {
                static_: picked,
                tile: self.tile_info(Tile::new(picked.at.x, picked.at.y)),
                prism: self
                    .resources
                    .surfaces
                    .as_ref()
                    .and_then(|surfaces| surfaces.shape(picked.graphic).prism),
            },
            SelectedIdentity::Mobile(who) => Selection::Mobile(
                self.drawn_mobiles()
                    .into_iter()
                    .find(|(drawn_who, _)| *drawn_who == who)
                    .map(|(drawn_who, mobile)| {
                        let order = depth::Order {
                            tile: depth::mobile_tile(mobile.at, mobile.from),
                            priority_z: depth::mobile_priority_z(mobile.at.z),
                        };
                        let picked = PickedMobile {
                            you: drawn_who.is_none(),
                            serial: match drawn_who {
                                Some(serial) => Some(serial),
                                None => self
                                    .world
                                    .authoritative
                                    .view
                                    .as_ref()
                                    .map(|view| view.player.serial),
                            },
                            body: mobile.body,
                            hue: mobile.hue,
                            at: mobile.at,
                            order,
                        };
                        (picked, self.tile_info(Tile::new(mobile.at.x, mobile.at.y)))
                    }),
            ),
            SelectedIdentity::Item(serial) => Selection::Item(
                self.world
                    .presentation
                    .item_serials
                    .iter()
                    .position(|held| *held == serial)
                    .map(|index| {
                        let item = self.world.presentation.items[index];
                        let priority_z = PriorityZ(depth::static_priority_z(
                            item.at.z,
                            self.resources.tiledata.static_tile(item.displayed().0),
                        ));
                        let picked = PickedItem {
                            serial,
                            graphic: item.displayed(),
                            hue: item.hue,
                            at: item.at,
                            priority_z,
                        };
                        (picked, self.tile_info(Tile::new(item.at.x, item.at.y)))
                    }),
            ),
        }
    }

    /// How much of the occlusion grid the two views of it draw this frame.
    ///
    /// The one place [`App::solids_everything`] — what the person picked — and
    /// the player's own `z` — what this frame is — are joined, so that no
    /// stale height can be stored anywhere and the wireframe and the solids
    /// pass cannot be cut differently. See
    /// [`solid::Cut`](openshard_client_render::solid::Cut).
    pub(crate) fn solid_cut(&self) -> openshard_client_render::solid::Cut {
        use openshard_client_render::solid::Cut;

        match self.graphics.solids_everything {
            true => Cut::Nothing,
            false => Cut::BelowFeet(self.world.motion.planning_state().position.z),
        }
    }

    /// What the Light tab has been turned to.
    ///
    /// [`light::Tuning::DEFAULT`] before there is a shell, which is every frame
    /// drawn with the HUD switched off and every frame of a test that drives
    /// this type without a window: the numbers a person turns live in the dev
    /// window's own [`Desk`](crate::desk::Desk), and where there is no window
    /// there is nothing turned.
    ///
    /// Read once and threaded through the frame rather than fetched wherever it
    /// is wanted: the occluder overlay's rectangle and the frame's own lighting
    /// have to be built from *one* answer, or the wireframe is a picture of a
    /// grid the shader did not walk.
    pub(crate) fn tuning(&self) -> light::Tuning {
        self.shell
            .as_ref()
            .map_or(light::Tuning::DEFAULT, shell::Shell::tuning)
    }

    /// What the Chat tab has been turned to — [`App::tuning`]'s own reason for
    /// being read live from the shell rather than from `self.desk`, which is
    /// only ever the value at load or at exit.
    pub(crate) fn chat_style(&self) -> desk::Chat {
        self.shell
            .as_ref()
            .map_or_else(desk::Chat::default, shell::Shell::chat)
    }

    /// The current text sizes, from the live HUD while it is open.
    ///
    /// Like [`Self::chat_style`] and [`Self::window_scale`], the app's desk is
    /// a startup-and-save snapshot. Reading it while a shell exists makes the
    /// font-size sliders appear inert until the next launch.
    pub(crate) fn font_sizes(&self) -> desk::FontSizes {
        self.shell.as_ref().map_or(self.desk.fonts, shell::Shell::fonts)
    }

    /// What `common/movement` makes of the ground on screen — the HUD's terrain
    /// overlay, gathered only while it is switched on.
    ///
    /// **Not a second opinion about walkability.** Every answer here comes from
    /// the same [`Terrain`] every step decision on this end asks — the client's
    /// map with the shard's items laid over it — so a tile the picture calls
    /// blocked is a tile the walk will refuse. A private "is this passable"
    /// written for the overlay would be a second policy, and the first bug it hid
    /// would be one of its own.
    ///
    /// Passability is asked per *tile* and not per step: `spawn_z` finds the
    /// surface a body would stand on regardless of how far that is from the
    /// player's own height — so a building's upper floor reads open from the
    /// street rather than blocked — and `can_stand` is what says this body may
    /// occupy that space, including its own reading of shut doors.
    ///
    /// The way *through* it is not here. A route is drawn whether this overlay
    /// is on or not, so that a Ctrl-drag shows where the body is about to go
    /// with no debugging switch thrown — see [`App::route_shown`].
    pub(crate) fn terrain_overlay(&self, bounds: TileBounds) -> TerrainOverlay {
        use openshard_map::grid::Tile;
        use openshard_movement::PLAYER_HEIGHT;

        let terrain = footing(&self.resources, self.walking_doors());
        let near = i32::from(self.world.motion.planning_state().position.z);
        let mut open = Vec::new();
        let mut blocked = Vec::new();
        // The same clamp the ground pass uses, so the wash covers exactly the
        // tiles that were drawn and no strip of it hangs off the map.
        if let Some((xs, ys)) = bounds.clamp_to(self.resources.map().width(), self.resources.map().height()) {
            for y in ys {
                for x in xs.clone() {
                    let tile = Tile::new(x, y);
                    // The height the diamond is drawn at, and the height the
                    // question is asked about, are one number — the surface a
                    // body would stand on here. A *blocked* tile has one too:
                    // the barrels on a pier stand on the planks, and washing
                    // their tile at the land's height (water, thirteen units
                    // down) drew the refusal a tile and a half away from the
                    // barrel that caused it. `ground_z` is only the fallback for
                    // a tile with no surface at all.
                    //
                    // `arrival_z` and not `MapTerrain::spawn_z`, which read the
                    // bare map: a house's floor and a ship's deck are the live
                    // world's, and washing them at the land underneath is the
                    // same defect as drawing the barrel's refusal at the water.
                    let surface = openshard_movement::arrival_z(&terrain, tile, near, PLAYER_HEIGHT);
                    // `clamp` rather than `unwrap`: a `z` outside `i8` is a
                    // corrupt block and not an invariant of ours, and a diamond
                    // drawn at the wrong height is a better answer than a panic
                    // in a debugging overlay.
                    let drawn_z = |z: i32| z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8;
                    match surface.filter(|&z| openshard_movement::can_stand(&terrain, tile, z, PLAYER_HEIGHT))
                    {
                        Some(z) => open.push(Point { x, y, z: drawn_z(z) }),
                        None => blocked.push(Point {
                            x,
                            y,
                            z: surface.map_or_else(
                                || terrain.map.and_then(|map| map.ground_z(tile)).unwrap_or(0),
                                drawn_z,
                            ),
                        }),
                    }
                }
            }
        }

        TerrainOverlay { open, blocked }
    }

    /// The terrain wash only changes when its view, the player's standing
    /// height, or the world it is read from changes. Keep its points between
    /// ordinary redraws: zoomed out, re-asking every tile is much dearer than
    /// painting the already-known diamonds.
    fn terrain_shown(&mut self, camera: Camera) -> Arc<TerrainOverlay> {
        let from = self.world.motion.route_origin();
        let bounds = camera.visible_tiles();
        if let Some(cached) = self
            .terrain_cache
            .as_ref()
            .filter(|cached| cached.bounds == bounds && cached.from == from)
        {
            return Arc::clone(&cached.overlay);
        }
        let overlay = Arc::new(self.terrain_overlay(bounds));
        self.terrain_cache = Some(crate::app::TerrainCache {
            bounds,
            from,
            overlay: Arc::clone(&overlay),
        });
        overlay
    }

    /// The visible slice of the facet-wide interior artifact.
    ///
    /// The camera only selects which already-baked labels to draw.  It never
    /// participates in joining them, so zooming or panning cannot recolour a
    /// house or make a wall seam appear and disappear.
    fn interiors_shown(&mut self, camera: Camera) -> Arc<InteriorOverlay> {
        let Some(graph) = self.resources.interiors.as_ref() else {
            return Arc::new(InteriorOverlay {
                cells: Vec::new(),
                doors: Vec::new(),
                stairs: Vec::new(),
                buildings: 0,
            });
        };
        let Some((xs, ys)) = camera
            .visible_tiles()
            .clamp_to(self.resources.map().width(), self.resources.map().height())
        else {
            return Arc::new(InteriorOverlay {
                cells: Vec::new(),
                doors: Vec::new(),
                stairs: Vec::new(),
                buildings: 0,
            });
        };
        let mut cells = Vec::new();
        let mut visible_buildings = std::collections::BTreeSet::new();
        for y in *ys.start()..=*ys.end() {
            for x in *xs.start()..=*xs.end() {
                let Some(building) = graph.building_at(x, y) else {
                    continue;
                };
                visible_buildings.insert(building);
                cells.push(InteriorCell {
                    at: Point::new(x, y, self.resources.map().land(x, y).map_or(0, |land| land.z)),
                    // This first artifact identifies a whole house.  The
                    // storey/room graph follows on top of these ids.
                    floor: 0,
                    room: building,
                    shown: true,
                });
            }
        }
        // Map doors are the immutable leaves placed in the facet.  Generated
        // doors arrive in the separate item layer, where their graphic is the
        // authoritative live open/shut state; both belong in this inspection
        // overlay.  The wall bake itself deliberately remains item-free.
        let map_doors = (*xs.start()..=*xs.end())
            .flat_map(|x| (*ys.start()..=*ys.end()).map(move |y| (x, y)))
            .filter_map(|(x, y)| {
                self.resources
                    .map()
                    .statics_at(x, y)
                    .find(|item| {
                        self.resources
                            .tiledata
                            .static_tile(item.tile.0)
                            .flags
                            .has(openshard_tiles::TileFlags::DOOR)
                    })
                    .map(|item| InteriorDoor {
                        at: Point::new(x, y, item.z),
                        shown: openshard_client_render::doors::is_open(item.tile),
                    })
            });
        let item_doors = self.world.presentation.items.iter().filter_map(|item| {
            let graphic = item.displayed();
            (graph.building_at(item.at.x, item.at.y).is_some()
                && openshard_client_render::doors::is_door(graphic))
            .then_some(InteriorDoor {
                at: item.at,
                shown: openshard_client_render::doors::is_open(graphic),
            })
        });
        let doors = map_doors.chain(item_doors).collect();
        Arc::new(InteriorOverlay {
            cells,
            doors,
            stairs: Vec::new(),
            buildings: visible_buildings.len(),
        })
    }

    /// The way to wherever the body was last told to go, as the two-coloured
    /// line the player is owed for a Ctrl-drag — or, with no destination and the
    /// terrain overlay switched on, the way that *would* be walked to the tile
    /// under the cursor.
    ///
    /// **The plan itself, not a picture of one.** [`steer::plan`] is the same
    /// call `Steering` walks by, so the green half is the route the body is
    /// really taking, and the red half begins at a shut door — the only thing
    /// the two readings of the ground differ by (see [`steer::Readings`]). A cut
    /// written here for the drawing alone would be a second policy about the
    /// same question, which `docs/parity.md` is the standing argument against.
    ///
    /// **One plan per changed route, and only while there is something to
    /// draw.** The walk plans on its own beat — at most once a step, by design,
    /// since a drag restates the destination tens of times a second — so its
    /// stored route is up to a step stale and is *cleared* the moment the
    /// destination moves. Drawn from that, the line under a dragging cursor
    /// would blink out and catch up a beat later, which is the opposite of what
    /// a preview is for. The HUD therefore retains its last answer until its
    /// start, destination, or world snapshot changes: a standing route costs
    /// no A* searches per frame.
    pub(crate) fn route_shown(&mut self, hover: Option<&PickedTile>) -> Option<Arc<Route>> {
        // The movement core names the route origin explicitly; the HUD does
        // not infer it from either a renderer `Mobile` or Crowd's clock.
        let from = self.world.motion.route_origin();
        let goal = match self.steer.goal() {
            Some(at) => at,
            // No destination: the hover preview is the terrain overlay's own
            // question — "where would a click here take me" — and is asked only
            // while somebody has that overlay open to read the answer against.
            //
            // Asked exactly the way a click would ask it — `walk_destination`,
            // the same static-first rule and the same height — because the
            // preview's whole claim is "this is where clicking here takes you".
            // A second rule here would draw a route to the street under the roof
            // the cursor is on, which is `docs/parity.md`'s complaint in
            // miniature.
            None => {
                let tile = hover.filter(|_| self.graphics.show_terrain)?;
                self.walk_destination(tile)
            }
        };
        if let Some(cached) = self.route_cache.as_ref().filter(|cached| cached.goal == goal) {
            if cached.from == from {
                return cached.route.clone();
            }
            // While a destination is being walked, the body advances along the
            // already validated route.  Requiring an exact `from` match here
            // made the HUD re-run the expensive plan for every step, even when
            // neither the goal nor the terrain had changed.  Trim the consumed
            // prefix instead. `entered` clears this cache whenever the item
            // terrain changes, so this cannot preserve a route through a new
            // blocker.
            if self.steer.goal().is_some() {
                if let Some(route) = cached.route.as_ref() {
                    if let Some(index) = route
                        .open
                        .iter()
                        .position(|point| point.x == from.x && point.y == from.y)
                    {
                        let route = Route {
                            open: route.open[index..].to_vec(),
                            barred: route.barred.clone(),
                        };
                        let route = Arc::new(route);
                        self.route_cache = Some(crate::app::RouteCache {
                            from,
                            goal,
                            route: Some(Arc::clone(&route)),
                        });
                        return Some(route);
                    }
                }
            }
        }
        // And the same crowd, for the same reason: the green line a player sees
        // is the plan itself and not a second opinion about it (`docs/parity.md`),
        // so a route drawn through a bystander would be a picture of a walk this
        // client is not going to take.
        let ground = steer::Readings {
            // The route the HUD draws is the one a step would take, so it reads
            // the doors as they stand whatever the auto-door setting is — the
            // setting is an intention to open one, and a picture of a walk is
            // not the place for intentions.
            //
            // Being dead is not an intention, which is why it is passed and the
            // setting is not: a ghost's route runs through the shut leaf because
            // its step does (`crate::world::walking_doors`). Drawing that route
            // stopped at the door would be a picture of a refusal that is not
            // going to happen — `docs/parity.md`'s whole complaint.
            live: footing(
                &self.resources,
                crate::world::walking_doors(self.world.dead(), false),
            )
            .among(openshard_movement::Bodies::standing(&self.world.bodies)),
            guide: guide(&self.resources),
            coarse: self.resources.coarse.as_ref(),
        };
        let route = self.steer.plan_for(ground, from, goal).map(|plan| {
            // The body's own tile leads the open half, so a route of one step is a
            // line and not a dot. The barred half carries on from wherever the open
            // one stopped — the body's tile when nothing at all is walkable, which
            // is a body standing at the shut door.
            let mut open = vec![from];
            open.extend(plan.open_points);
            let from = *open.last().unwrap();
            let mut barred = plan.barred_points;
            if !barred.is_empty() {
                barred.insert(0, from);
            }
            Arc::new(Route { open, barred })
        });
        self.route_cache = Some(crate::app::RouteCache {
            from,
            goal,
            route: route.clone(),
        });
        route
    }

    /// The occluder wireframe is a second consumer of the frame's grid, but
    /// egui needs its shapes before the renderer has assembled that frame. Its
    /// source can still be retained while the grid's exact inputs are stable.
    fn occluders_shown(&mut self, camera: Camera, cutaway: &Cutaway) -> Arc<[OccluderSurface]> {
        let bounds = light::lit_tiles(&camera, &self.tuning());
        let atlas_revision = self
            .window
            .as_ref()
            .map(|window| window.atlases.statics.revision());
        if let Some(cached) = self.occluder_cache.as_ref().filter(|cached| {
            cached.bounds == bounds && cached.cutaway == *cutaway && cached.atlas_revision == atlas_revision
        }) {
            return Arc::clone(&cached.surfaces);
        }
        let occlusion = occlusion::collect(
            self.resources.map(),
            &self.world.presentation.items,
            bounds,
            &self.resources.tiledata,
            cutaway,
            self.window
                .as_ref()
                .map(|window| openshard_client_render::atlas::StaticArt::Pages(&window.atlases.statics)),
        );
        let bounds = occlusion.bounds();
        let mut surfaces = Vec::new();
        for y in bounds.min_y..=bounds.max_y {
            for x in bounds.min_x..=bounds.max_x {
                surfaces.extend(occlusion.solids_at(x, y).map(|solid| OccluderSurface {
                    x,
                    y,
                    solid: *solid,
                }));
            }
        }
        // The painter has no depth buffer. Preserve the grid's own back-to-front
        // order once with the cached surface list rather than sorting the same
        // thousand boxes for every egui redraw.
        surfaces.sort_by_key(|surface| (surface.x + surface.y, surface.solid.bottom(), surface.solid.top()));
        let surfaces: Arc<[OccluderSurface]> = surfaces.into();
        self.occluder_cache = Some(crate::app::OccluderCache {
            bounds,
            cutaway: *cutaway,
            atlas_revision,
            surfaces: Arc::clone(&surfaces),
        });
        surfaces
    }

    /// Do what the HUD asked for on the frame before this one.
    ///
    /// Every writer the shell has, in one place and at one moment: the top of a
    /// frame, before anything reads. See [`App::pending`] for why it is a frame
    /// late and why that is the point rather than a compromise.
    ///
    /// The viewport is deliberately not in here. It is not something a widget
    /// *asked* for — it is what the layout left over, which `Shell` holds between
    /// frames — and it is applied beside this call rather than through it.
    pub(crate) fn apply(&mut self, request: shell::Request) {
        // **No amount answer here any more.** The picker was an `egui::Window`,
        // so what it decided arrived a frame late through this struct and had
        // to be translated out of the shell's vocabulary on the way. It is a
        // gump window now (`panes::split`) and its answer is an ordinary
        // `Effect` on the frame the button was pressed — see
        // `App::answer_prompt`, which is where the two lines that used to be
        // here went.
        // **No Add and no Leave here either.** Both were buttons on the shell's
        // roster window and are controls on the manifest now (`panes::party`),
        // where each is an ordinary `Effect::Net` from the window that was
        // pressed. Leaving is still `0x02` naming yourself — that is the wire's
        // shape and did not change — but the pane names it, not this.
        //
        // **No party invitation arm here any more.** The answer is a press on
        // this client's own `0x0816` plate now — see `panes::confirm` — which
        // reaches the shard as an ordinary `Effect::Net` from the window that
        // was pressed, the same as every other window's packet. Nothing about
        // the invitation is edge-triggered through the shell any more, because
        // the shell no longer draws it.
        if request.frame_dump {
            self.request_frame_dump();
        }
        if request.relock {
            self.relock();
        } else if request.unlock {
            self.control.unlock();
        }
        if let Some(rig) = request.rig {
            // The eye does not move — that is what `set_rig` promises — but the
            // frames before the swap were flown by another camera, and measuring
            // them together would average two rigs.
            self.control.set_rig(rig);
            self.scope.clear();
        }
        // The body's ease is not the rig and does not clear the scope: the frames
        // either side of it were flown by the same camera, and what the scope
        // measures is the eye against the body it was given.
        if let Some(ease) = request.ease {
            self.world.presentation.crowd.set_ease(ease);
        }
        if let Some(audio) = request.audio {
            self.audio.set_volumes(audio.effects, audio.music);
        }
        if let Some(always_run) = request.always_run {
            self.steer.set_always_running(always_run);
        }
        if let Some(auto_open_doors) = request.auto_open_doors {
            self.auto_open_doors = auto_open_doors;
        }
        if let Some(draw) = request.draw {
            self.graphics.drawing = draw;
        }
        if let Some(disabled) = request.cutaway_disabled {
            self.graphics.cutaway_disabled = disabled;
        }
        if let Some(disabled) = request.body_overlap_transparency_disabled {
            self.graphics.body_overlap_transparency_disabled = disabled;
        }
        if let Some(show) = request.show_terrain {
            self.graphics.show_terrain = show;
        }
        if let Some(show) = request.show_interiors {
            self.graphics.show_interiors = show;
        }
        if let Some(buildings) = request.buildings {
            if self.graphics.buildings != buildings {
                // The cache owns the baked cell/room/floor graph, not merely
                // the visible frame. Re-entering this mode must rebuild it
                // under the current topology rules.
                *self.world.presentation.interior_cache.get_mut() = InteriorCache::default();
            }
            self.graphics.buildings = buildings;
        }
        if let Some(z_slice) = request.z_slice {
            self.graphics.z_slice = z_slice;
        }
        if let Some(z_slice_view) = request.z_slice_view {
            self.graphics.z_slice_view = z_slice_view;
        }
        if let Some(floor_view) = request.floor_view {
            self.graphics.floor_view = floor_view;
        }
        if let Some(show) = request.show_occluders {
            self.graphics.show_occluders = show;
        }
        if let Some(show) = request.show_solids {
            self.graphics.show_solids = show;
        }
        if let Some(only) = request.solids_only {
            self.graphics.solids_only = only;
        }
        if let Some(opaque) = request.solids_opaque {
            self.graphics.solids_opaque = opaque;
        }
        // The variant and not the `z` in it: what the person picked holds across
        // frames, and the height they were standing at when they picked it is
        // this frame's business — see [`App::solid_cut`].
        if let Some(cut) = request.solid_cut {
            self.graphics.solids_everything = matches!(cut, openshard_client_render::solid::Cut::Nothing);
        }
        if let Some(target) = request.highlight {
            self.graphics.highlight = target;
        }
        if let Some(style) = request.highlight_style {
            self.graphics.highlight_style = style;
        }
        // The window the metrics are taken over, and not a clear: the frames
        // already held were flown by the same rig.
        if let Some(span) = request.scope_span {
            self.scope.set_span(span);
        }
        match request.script {
            Some(shell::ScriptRequest::Run(name)) => self.start_replay(name),
            Some(shell::ScriptRequest::Stop) => self.replay = None,
            None => {}
        }
        if let Some((graphic, prism)) = request.authored_prism {
            let shape = openshard_client_render::occlusion::Shape {
                prism: Some(prism),
                ..self
                    .resources
                    .surfaces
                    .as_ref()
                    .map_or(openshard_client_render::occlusion::Shape::UNREAD, |surfaces| {
                        surfaces.shape(graphic)
                    })
            };
            self.resources
                .surfaces
                .get_or_insert_default()
                .author(graphic, shape);
            self.resources.repack_forced = true;
            // Cleared like the eviction branch clears it: the next `wanted`
            // this frame computes has to be the whole visible set and not a
            // delta off the *old* atlas's coverage, since a delta would not
            // include `graphic` if it was already on screen before the edit
            // — which, for the debug HUD, it always was.
            self.graphics.covered = None;
        }
    }

    /// What the panels are allowed to know, gathered each frame.
    ///
    /// `camera` is the frame's own, handed in rather than read back from
    /// [`App::control`]: the overlay the shell draws from this and the world pass
    /// below it are two readers of one picture, and the only way they cannot
    /// disagree is for there to be one value. See [`App::draw`].
    /// Whether the world may read the cursor at all.
    ///
    /// Asked once and answered for the whole frame. A pointer over a panel picks
    /// no tile and lights no item, so nothing is highlighted under the panel and
    /// nothing is highlighted where the pointer *was* when it went over one; a
    /// pointer that has left the window is the other half, and the one no egui
    /// state can answer — see [`input::Input::pointer_inside`] and
    /// [`shell::Shell::holds_pointer`].
    pub(crate) fn world_owns_pointer(&self) -> bool {
        self.input.pointer_inside && !self.shell.as_ref().is_some_and(shell::Shell::holds_pointer)
    }

    /// Which object the pointer is asking about, or `None`.
    ///
    /// The pick order the rest of this file uses, with the windows in front of
    /// it: an open container's icon beats anything in the world behind the
    /// window, and in the world a creature beats an item. The map's own
    /// furniture is deliberately absent — a static is not the shard's object,
    /// has no serial, and there is nothing to ask about.
    fn tooltip_subject(&self) -> Option<Serial> {
        if let Some(item) = self.hovered_container_item() {
            return Some(item);
        }
        if !self.world_owns_pointer() {
            return None;
        }
        let view = self.world.authoritative.view.as_ref()?;
        // `Who` is `Option<Serial>` and its `None` is the player's own body,
        // which has a serial like anything else and a tooltip like anything
        // else — hovering your own character is the ordinary way to read it.
        if let Some(who) = self.picking.hover.mobile {
            return Some(who.unwrap_or(view.player.serial));
        }
        self.picking.hover.item.map(|item| item.serial)
    }

    /// The tooltip to draw at the cursor this frame, and the request that fills
    /// it in if this client does not hold one yet.
    ///
    /// Asking here rather than when the `0xDC` arrived is the whole shape of it:
    /// the shard announces a revision for every object it draws, and turning
    /// each of those into a request would put every tooltip in view back on the
    /// wire and leave the announcement doing nothing. So the hover is what asks,
    /// and [`tooltips::Tooltips`] is what stops it asking sixty times a second
    /// while the answer is in flight.
    ///
    /// Empty rather than a placeholder when nothing is held yet: the first hover
    /// over a fresh object draws no box for one round trip, which is what every
    /// reference client does too.
    pub(crate) fn hover_tooltip(&mut self) -> Vec<String> {
        let Some(serial) = self.tooltip_subject() else {
            return Vec::new();
        };
        let held = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.tooltips.get(&serial));
        if self.tooltips.should_ask(serial, held) {
            if let Some(link) = self.world.shard.link() {
                link.query_properties(vec![serial]);
            }
        }
        held.map(|held| tooltips::lines(&held.entries, self.resources.cliloc.as_ref()))
            .unwrap_or_default()
    }

    /// `pick` is what [`items::pick`], [`mobiles::pick`] and
    /// [`statics::pick`] answered for this frame, handed in as the one value
    /// [`App::frame_facts`] built them into rather than asked again: the HUD
    /// and the world passes are two readers of one picture, and the tile
    /// marker is drawn or not drawn on the strength of whether an item took
    /// the highlight. Asking twice would be two answers to "what is the
    /// cursor on", and the frame where they disagree is the frame a barrel is
    /// ringed *and* the ground under it is diamonded.
    ///
    /// `cutaway` is handed in for the third reader of that same rule: the
    /// occluder overlay draws the grid the frame's lighting is about to build,
    /// and a grid built from a second cutaway would draw boxes for the storey
    /// this frame took away.
    pub(crate) fn hud(
        &mut self,
        camera: Camera,
        pick: &Pick,
        cutaway: &Cutaway,
        drawn_mobiles: Option<&[(Who, Mobile)]>,
    ) -> (Hud, HudTimings) {
        let perf_started = Instant::now();
        let perf = self.perf();
        let perf = (perf, perf_started.elapsed());

        let terrain_started = Instant::now();
        let terrain = self.graphics.show_terrain.then(|| self.terrain_shown(camera));
        let terrain_cost = terrain_started.elapsed();

        let route_started = Instant::now();
        let route = self.route_shown(pick.tile.as_ref());
        let route_cost = route_started.elapsed();

        let occluders_started = Instant::now();
        let occluders = self
            .graphics
            .show_occluders
            .then(|| self.occluders_shown(camera, cutaway));
        let occluders_cost = occluders_started.elapsed();

        let interiors = self.graphics.show_interiors.then(|| self.interiors_shown(camera));

        let picking_started = Instant::now();
        let hud = Hud {
            locked: self.control.follow() == Follow::Body,
            rig: self.control.rig(),
            perf: perf.0,
            scripts: self.scripts.iter().map(|script| script.name).collect(),
            replay: self.replay.as_ref().map(|replay| {
                let length = replay.length().as_secs_f32().max(0.001);
                (replay.name(), replay.at().as_secs_f32() / length)
            }),
            draw: self.graphics.drawing,
            cutaway_disabled: self.graphics.cutaway_disabled,
            body_overlap_transparency_disabled: self.graphics.body_overlap_transparency_disabled,
            show_terrain: self.graphics.show_terrain,
            // The tile is lit when nothing else took the highlight. Under
            // `Items` nothing ever does, which is the mode's whole content; the
            // ground is still hovered and the panel still reads it.
            hover_lit: match self.graphics.highlight {
                // The map's own furniture counts here as much as an item does,
                // and it is the case this rule was missing: a wall under the
                // cursor is what a click takes, so a diamond drawn on the ground
                // *behind* it — which is where the cursor unprojects to, a wall
                // being taller than the cell it stands on — is the client
                // pointing at two tiles at once. That is the disagreement this
                // arm exists to stop, and it had one more source than it knew.
                HighlightTarget::Auto => {
                    pick.item.is_none() && pick.mobile.is_none() && pick.static_.is_none()
                }
                HighlightTarget::Items => false,
                HighlightTarget::Tiles => true,
            },
            highlight: self.graphics.highlight,
            highlight_style: self.graphics.highlight_style,
            terrain,
            show_interiors: self.graphics.show_interiors,
            interiors,
            buildings: self.graphics.buildings,
            z_slice: self.graphics.z_slice,
            z_slice_view: self.graphics.z_slice_view,
            floor_view: self.graphics.floor_view,
            route,
            show_occluders: self.graphics.show_occluders,
            show_solids: self.graphics.show_solids,
            solids_only: self.graphics.solids_only,
            solids_opaque: self.graphics.solids_opaque,
            solid_cut: self.solid_cut(),
            solids: (self.graphics.solids_held, self.graphics.solids_drawn),
            // The grid the lighting will build a few lines later in the same
            // frame, built here a second time rather than kept from the last
            // one: the HUD is drawn before the world passes, and a wireframe a
            // frame behind the picture it is a claim about slides off every wall
            // as the camera pans — which is the one artefact an instrument for
            // finding misplaced occluders must not have.
            //
            // `light::lit_tiles`, not `camera.visible_tiles`: the grid is grown
            // by the widest pool's reach, and a box drawn over a rectangle the
            // shader did not walk would be a picture of this overlay's own
            // bounds rather than of the lighting's.
            occluders,
            pick: pick.clone(),
            selected: self
                .picking
                .selected
                .map(|identity| self.resolve_selection(identity)),
            health_bars: self.health_bars(camera, drawn_mobiles),
            goal: self.steer.goal().map(|at| self.tile_info(Tile::new(at.x, at.y))),
            ttf_active: self.resources.ttf_font.is_some(),
            composites: self
                .window
                .as_ref()
                .map_or_else(CompositeTelemetry::default, |window| CompositeTelemetry {
                    ready: window.composites.len(),
                    pending: self.composite_work.pending_len(),
                    prepared: self.composite_work.prepared_len(),
                    in_flight: self.composite_work.in_flight_len(),
                    gpu_bytes: window.composites.gpu_bytes(),
                    gpu_budget_bytes: window.composites.limits().max_gpu_bytes,
                    quarantined: window.composites.quarantined_len(),
                    latest_quarantine: window.composites.latest_quarantine(),
                }),
            radar: RadarTelemetry {
                // A frame behind, and the only part of this that is — see
                // `RadarFrame`. The three counter sets below are live.
                frame: self.radar_frame.clone(),
                cache: self.radar_cache.counters(),
                queue: self.radar_queue.counters(),
                // No window is no GPU page cache, and a zeroed snapshot is the
                // truthful answer for it: there is nothing resident because
                // there is nothing to be resident in.
                pages: self
                    .window
                    .as_ref()
                    .map_or_else(Default::default, |window| window.radar_chunks.counters()),
            },
        };
        let picking_cost = picking_started.elapsed();

        (
            hud,
            HudTimings {
                terrain: terrain_cost,
                route: route_cost,
                occluders: occluders_cost,
                picking: picking_cost,
                perf: perf.1,
            },
        )
    }

    fn health_bars(&self, camera: Camera, drawn_mobiles: Option<&[(Who, Mobile)]>) -> Vec<HealthBar> {
        let Some(view) = self.world.authoritative.view.as_ref() else {
            return Vec::new();
        };
        let (Some(drawn_mobiles), Some(window)) = (drawn_mobiles, self.window.as_ref()) else {
            return Vec::new();
        };

        drawn_mobiles
            .iter()
            .filter_map(|(who, drawn)| {
                let (hits, mana, notoriety, targeted) = match who {
                    Some(serial) if *serial == view.player.serial => (
                        view.player.hits,
                        view.player.status.as_ref().map(|status| status.mana),
                        Notoriety::Innocent,
                        false,
                    ),
                    Some(serial) => {
                        let mobile = view.mobiles.get(serial)?;
                        (
                            mobile.hits,
                            None,
                            mobile.notoriety,
                            view.player.attacking == Some(*serial),
                        )
                    }
                    None => return None,
                };
                let hits = hits?;
                let serial = who.expect("the None identity returned above");
                let anchor = mobiles::head_anchor(drawn, &camera, &window.atlases.mobiles)?;
                Some(HealthBar {
                    anchor,
                    current: HealthPoints::new(hits.current),
                    estimated: HealthPoints::new(
                        self.world.presentation.estimated_health(serial, hits.current),
                    ),
                    mana: mana.map(|mana| crate::diagnostics::ResourceBar {
                        current: HealthPoints::new(mana.current),
                        max: HealthPoints::new(mana.max),
                    }),
                    max: HealthPoints::new(hits.max),
                    notoriety,
                    targeted,
                })
            })
            .collect()
    }

    /// The perf snapshot the HUD panels read, gathered on its own because
    /// nothing in it needs the camera or this frame's picks — see
    /// [`frames::Perf`].
    pub(crate) fn perf(&self) -> frames::Perf {
        frames::Perf {
            readings: bench::readings(self.scope.samples()),
            // Two frames is one difference and no derivative of it. Absent
            // rather than a zero, which would read as "the eye was perfectly
            // smooth" on the frame the window opened.
            metrics: (self.scope.samples().len() > 2).then(|| Metrics::of(self.scope.samples())),
            scope_span: self.scope.span(),
            frames: self.frames.frames().to_vec(),
            frames_span: self.frames.span(),
            worst_fps: self.frames.worst_fps(),
            // Which pass ate the device's frame. Cloned into the snapshot like
            // everything else here — a dozen short strings a frame — for the
            // reason `Perf` exists: a panel that could reach back into the
            // screen would be reading a frame the screen has moved past.
            gpu_passes: self
                .window
                .as_ref()
                .and_then(|window| window.gpu.as_ref())
                .map(|gpu| gpu.passes().to_vec())
                .unwrap_or_default(),
            repacks: self.repacks,
            // What is currently *asking* for frames, which is the other half of
            // any answer about the frame rate: a picture drawn every 80ms is not
            // a slow frame if the loop is on the animation clock, it is a frame
            // nobody asked for sooner.
            pacing: self.pacing(),
        }
    }
}

impl crate::App {
    /// Rebuild the house drawn under a `0x99` cursor.
    ///
    /// Once a frame, before the geometry is assembled, because a preview follows
    /// the *pointer* and the pointer moves between packets. Cheap when there is
    /// no multi cursor up, which is nearly always: one `Option` test and a
    /// `clear` over an already-empty vector.
    ///
    /// The pieces go into `presentation.multi_preview` rather than
    /// `presentation.items` — see that field for the two reasons, which are not
    /// the same reason.
    pub(crate) fn refresh_multi_preview(&mut self, camera: Camera) {
        let multi = self
            .world
            .authoritative
            .view
            .as_ref()
            .and_then(|view| view.target)
            .and_then(|target| target.multi);
        let (Some(multi), true) = (multi, self.world_owns_pointer()) else {
            self.world.presentation.multi_preview.clear();
            return;
        };
        // The same tile the click would answer with, so what a player sees is
        // where the house lands. `target_under_cursor` reads a picked static
        // first and this does not: a house is placed on the *ground*, and the
        // shard raises a location cursor for exactly that reason.
        let Some(tile) = self.pick_tile(camera) else {
            self.world.presentation.multi_preview.clear();
            return;
        };
        let at = openshard_protocol::world::Point::new(tile.at.x, tile.at.y, tile.stand_z.0);
        // `Unknown` and `NotAMulti` both draw nothing here, and for once that is
        // the same answer: a preview this client has no shape for is a preview it
        // cannot show, and the cursor is still up either way.
        self.world.presentation.multi_preview = match crate::net_command::multi_pieces(
            self.resources.multis.as_deref(),
            // A placement preview is never a designed house: the cursor draws a
            // multi the player is about to put down, and a design belongs to one
            // that already stands.
            None,
            multi.graphic(),
            at,
            openshard_protocol::wire::Hue::NONE,
        ) {
            crate::net_command::MultiDraw::Pieces(pieces) => pieces,
            crate::net_command::MultiDraw::Unknown | crate::net_command::MultiDraw::NotAMulti => Vec::new(),
        };
    }
}
