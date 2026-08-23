//! **One frame, however it was asked for.**
//!
//! A frame is the lighting, the ground and the statics of one place, collected
//! in one order out of one set of inputs. Before this module the sequence was
//! written out by hand in at least seven places — the client, three examples and
//! four tests — and every one of them was free to pass a different cutaway, a
//! different grid, a different clock. Each of them did, and the difference was
//! found by *reading* rather than by a gate: `examples/isolated_scene.rs` passed
//! no occlusion bake where the client passes one and `Cutaway::OPEN` where the
//! client passes the player's own, and `tests/cost.rs` collected its statics
//! against an empty grid so that every fragment in its frame took the billboard
//! fallback. Nothing said so, and the frames they dumped looked like frames.
//!
//! So parity between the client and the tools that are supposed to show what the
//! client does was a coincidence rather than a property. This module is the
//! first half of making it a property: **one assembly, so that most of the ways
//! two callers can differ are unexpressible**, and a caller that genuinely has
//! to differ says so by setting a field on [`Inputs`]. The second half is a gate
//! over the handful of fields that are left; see `docs/parity.md`.
//!
//! # What it does not do
//!
//! It does not draw. What comes back is [`Frame`] — three lists and a
//! [`Lighting`] — and the passes that turn those into pixels are the caller's,
//! because a tool that renders one crop and a client that renders a window into
//! a surface do genuinely different things with the same geometry.
//!
//! It also does not collect mobiles, the selection mask or the outline lists.
//! Those are not part of the sequence that was diverging, they read no grid, and
//! folding them in would make the struct a bag of everything a caller might want
//! rather than the inputs of one assembly.

use openshard_map::map::WorldMap;
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use openshard_tiles::TileData;
use std::time::{Duration, Instant};

use crate::animate::StaticAnimations;
use crate::atlas::{LandAtlas, StaticArt, TexmapAtlas};
use crate::camera::Camera;
use crate::cutaway::Cutaway;
use crate::debug::View;
use crate::geometry::Vec2;
use crate::ground::GroundQuad;
use crate::items::{GroundItem, ItemIndex};
use crate::light::{self, Ambient, Lighting, Sun, Tuning};
use crate::occlusion::Occlusion;
use crate::occlusion::bake::Bake;
use crate::statics::StaticGeometry;

/// Whether a static's fragments are met against this frame's own boxes.
///
/// **A field and not an omission.** Passing [`Occlusion::EMPTY`] to
/// [`crate::statics::collect`] used to be how a caller asked for the cheap
/// answer, and the cost of that was a frame nobody could tell from a real one:
/// `tests/cost.rs` priced a pass whose every fragment took the billboard
/// fallback, its `View::Normal` was not the client's and its `View::Solid` was
/// uniformly black. Named for what it does to the picture rather than for what
/// it saves, so that a frame built the cheap way says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impostor {
    /// A fragment's position and normal come from the box its pixel lands in —
    /// the client's own answer, and the one every plane of the G-buffer is read
    /// against.
    Met,
    /// Every fragment takes `statics.wesl`'s billboard fallback instead: a
    /// sprite's normal is its stance, and no pixel is anywhere but on the plane
    /// its quad was drawn at.
    ///
    /// The client reaches this itself with the lights off — no sky means no grid
    /// — so it is a picture a player can be looking at and not only a test's.
    Billboards,
}

/// Which of the world's producers this frame draws at all.
///
/// **A subset of the world, not a subset of the picture.** The G-buffer holds one
/// answer per pixel, so a diagnostic that wants to look at the walls cannot get
/// them out of a frame where a body is standing in front of one: whatever the
/// blit paints, that pixel belongs to the body. The way to see the wall is to
/// draw a frame the body is not in — and then the wall is *there*, with its own
/// normal and its own solid, rather than a hole shaped like a person.
///
/// **It filters the drawing and never the lighting.** Everything still stands in
/// the occlusion grid, still casts its shadow and still burns if it is a flame:
/// a wall lit as it really is, with the barrel in front of it not drawn, is the
/// picture a person is after. A knob that took the barrel out of the world too
/// would answer about a different world, and the difference would be invisible.
///
/// The tools have had this as three environment variables since before there was
/// a client that could ask for it (`OPENSHARD_SCENE_STATICS`, `_SHARD`,
/// `_GROUND`); this is that capability where the client can reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Draw {
    /// The land: every tile's own diamond.
    pub land: bool,
    /// The statics built into the map — walls, floors, roofs, and the furniture
    /// an artist put in a building.
    pub statics: bool,
    /// What the server put on the ground: dropped items and a pack's
    /// decorations, which reach the same pass through [`crate::items::collect`].
    pub items: bool,
    /// Mobiles — **honoured by the caller and not here.** This module does not
    /// collect them (see the module's own note), so nothing in [`assemble`] reads
    /// this field; the client's own mobile pass does.
    ///
    /// It is a field all the same, because [`Inputs::summary`] is what two dumps
    /// are diffed by: a frame drawn with the crowd and a frame drawn without it
    /// would otherwise carry identical summaries, which is the exact false
    /// negative `docs/parity.md` exists about.
    pub mobiles: bool,
}

impl Draw {
    /// The whole world, which is what the client draws when nobody has ticked
    /// anything off — and what every caller that is not a diagnostic wants.
    pub const EVERYTHING: Self = Self {
        land: true,
        statics: true,
        items: true,
        mobiles: true,
    };
}

/// Everything one frame is assembled out of.
///
/// **Every field a caller has ever differed on is a field here**, and none of
/// them has a default: a default is how the divergences this module exists to
/// remove got in, one call site at a time. A struct literal names all nineteen,
/// which is the point — a caller that wants the client's frame writes the
/// client's values, and a caller that wants something else can be read off
/// against it.
#[derive(Debug)]
pub struct Inputs<'a> {
    /// The world. Land and the statics built into it.
    pub map: &'a WorldMap,
    /// What the server has put in it: dropped items, decorations, and — for a
    /// tool that builds a place rather than loading one — the map's own statics
    /// translated onto a synthetic anchor.
    pub items: &'a [GroundItem],
    /// Where the eye is and how much it can see. Its zoom is in here too, so
    /// two callers at different zooms differ by this one field.
    pub camera: &'a Camera,
    /// The client's own table: what a graphic's flags and height are.
    pub tiledata: &'a TileData,
    /// The animation cycles, and the clock they have been advanced by — see
    /// [`StaticAnimations::advance`]. A frame collected off a table advanced to
    /// a different instant draws different pictures for the same map.
    pub animations: &'a StaticAnimations,
    /// What the frame has cut away above the player's head. The same one the
    /// lights are tested against as the pictures: a wall the frame did not draw
    /// must not darken the street, and a brazier on a hidden storey must not
    /// light the floor.
    pub cutaway: &'a Cutaway,
    /// The separate, building-local picture policy. `None` leaves the ordinary
    /// open-world frame untouched; it is never folded into [`Cutaway`].
    pub interior: Option<&'a crate::interiors::InteriorFrame>,
    /// The land art.
    pub land: &'a LandAtlas,
    /// The land textures.
    pub texmaps: &'a TexmapAtlas,
    /// The sprites — one atlas for the map's furniture and the server's items,
    /// because they go through one pass. It is also where an occluder's *facing*
    /// comes from, which is why a frame drawn from one atlas and lit from
    /// another would be two different sets of walls.
    pub statics: StaticArt<'a>,
    /// The sky, already flattened or not by whoever knows whether the field is
    /// wanted.
    ///
    /// `None` is the client's daylight: no grid is built at all, the lighting is
    /// [`Lighting::NONE`], and the blit is the copy it has always been. It is
    /// not "no lighting yet" — it is a frame nothing is multiplied by, and the
    /// statics of it are [`Impostor::Billboards`] whatever the field below says,
    /// because there is no grid for them to be met against.
    pub sky: Option<Ambient>,
    /// The sun, where the frame has one. Set on the lighting rather than walked
    /// with the tiles: it is one direction for the whole world.
    pub sun: Option<Sun>,
    /// The flame in the player's own hand — where they stand and which way they
    /// face. No walk of the map could find this one; nothing on the wire says a
    /// hand is carrying anything.
    ///
    /// Held rather than collected, and through [`Tuning::applied`] as every
    /// collected flame is, so a brightness knob that left the one light the
    /// player actually carries alone does not read as having no effect.
    ///
    /// The middle field is [`light::carried`]'s `offset`: how far past `at`'s
    /// tile the body is actually drawn this instant, so the pool glides with
    /// the sprite instead of jumping once a step.
    pub carried: Option<(Point, Vec2, Direction)>,
    /// What a person has turned. Read here rather than applied to the result —
    /// the reach is what the walk's rectangle is grown by, so a frame collected
    /// without it has already lost the flames outside the old margin.
    pub tuning: &'a Tuning,
    /// The flicker's clock, in seconds. An argument because this crate does not
    /// own one, and the same sampled instant every other clock in the frame was
    /// advanced by.
    pub flame_time: f32,
    /// The blocks of the occlusion grid built for earlier frames, where the
    /// caller keeps them across frames.
    ///
    /// `None` builds the grid from nothing, which is **the same grid** — that is
    /// `occlusion::bake`'s own first test — so this is a cost and not a picture.
    /// It is still a field: a caller that passes `None` where the client passes
    /// a bake is running different code to reach the same answer, and when the
    /// two disagree the gate should be able to say which one was asked for.
    pub bake: Option<&'a mut Bake>,
    /// The item the cursor is over, drawn in [`crate::items::HIGHLIGHT_HUE`]
    /// instead of its own. An index into `items`, and a fact about this frame
    /// rather than about the thing.
    pub highlight: Option<ItemIndex>,
    /// Whether the statics are met against this frame's boxes — see
    /// [`Impostor`].
    pub impostor: Impostor,
    /// Which producers this frame draws at all — see [`Draw`]. The lighting is
    /// collected from the whole world whatever this says.
    pub draw: Draw,
    /// Which of the pass's own values to draw instead of the lit frame. A
    /// property of the person looking, which is why it is set on the way out
    /// rather than walked with the tiles.
    pub view: View,
    /// Whether the player's own character is a ghost — `view::Player::dead`,
    /// `0x2C`. Set on the way out beside [`view`](Self::view) for the same
    /// reason: a fact about the person looking, not about the tiles walked to
    /// light them. `docs/combat.md`'s D9.
    pub dead: bool,
    /// Where the player's own picture lands on screen this frame —
    /// [`crate::mobiles::screen_rect`] — so a tree standing over it can be cut
    /// away. `None` for a caller with no player on screen to protect: the
    /// offline placeholder, a tool building a place rather than following a
    /// character.
    pub player_rect: Option<crate::geometry::Rect>,
    /// The player's non-transparent body texels at the same placement as
    /// [`player_rect`](Self::player_rect). Architectural cutaway candidates
    /// need a true pixel overlap with this mask; foliage deliberately keeps its
    /// separate rectangle policy.
    pub player_mask: Option<&'a crate::mobiles::OpaqueMask>,
    /// The long-lived opacity of cutaway candidates, owned by the presentation
    /// rather than reconstructed from a single frame.
    pub fades: &'a mut crate::cutaway::Fades,
}

impl Inputs<'_> {
    /// **What this frame was asked for, one field a line.**
    ///
    /// A picture on its own cannot be reproduced: two frames that differ say
    /// nothing about *which* input differed, and a person reading a client's
    /// dump beside a tool's has to reconstruct the client's arguments by reading
    /// its source — which is the exact failure `docs/parity.md` was written
    /// about. So a dumped frame carries this beside it, and two of them diff.
    ///
    /// **Every field gets a line, including the four this cannot state.** A
    /// summary that quietly omitted the map and the atlases would be a second
    /// list of what a frame is made of, kept by hand, going out of step the
    /// first time [`Inputs`] grows a field — the shape this module exists to
    /// stop. What can be said about each of them is said (a facet's name, a
    /// static count, an atlas's size and revision), and where nothing can be, it
    /// says so in as many words rather than leaving the reader to notice a gap.
    ///
    /// Not [`std::fmt::Debug`]: `{:?}` on these fields is a whole facet's land
    /// and three atlases' pixels.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let mut line = |field: &str, value: String| {
            out.push_str(field);
            out.push_str(" = ");
            out.push_str(&value);
            out.push('\n');
        };
        line(
            "map",
            format!("{}, {} statics", self.map.facet_name(), self.map.static_count()),
        );
        line("items", format!("{} on the ground", self.items.len()));
        let (image_width, image_height) = self.camera.image_size();
        let (eye_x, eye_y) = self.camera.eye_tile();
        let eye = self.camera.eye_at();
        // The tile first, because that is what a caller *aims* — the pixel is
        // what the aiming came out as, and a difference of a pixel between two
        // frames aimed at one tile is a real difference this would otherwise
        // round away.
        //
        // **And the extent's own parity, spelled out** — `docs/parity.md` P5's
        // G3. It is derivable from the two numbers beside it and it is written
        // anyway, because the whole of the window-parity finding is that nobody
        // ever read it *as an input*: an odd extent centres the world half a
        // pixel differently and every tool ever built has drawn on an even one
        // by unanimous accident. A person diffing two dumps scans field names,
        // and "image 1919x2077" does not have one.
        let parity = |extent: u32| if extent.is_multiple_of(2) { "even" } else { "odd" };
        line(
            "camera",
            format!(
                "eye tile ({eye_x}, {eye_y}), pixel ({:.3}, {:.3}), zoom {:?}, image \
                 {image_width}x{image_height} ({} by {})",
                eye.x,
                eye.y,
                self.camera.zoom(),
                parity(image_width),
                parity(image_height),
            ),
        );
        // Two facts about the client's own files, which a summary cannot check
        // are the same two files. Named anyway: a reader comparing two dumps has
        // to be told what is *not* being compared, or an equal summary reads as
        // an equal frame.
        line("tiledata", "the client's own table (not summarised)".to_owned());
        line("animations", format!("{} cycles", self.animations.len()));
        line("cutaway", format!("{:?}", self.cutaway));
        line(
            "interior",
            self.interior.map_or_else(
                || "not applicable".to_owned(),
                |frame| {
                    frame.z_range().map_or_else(
                        || {
                            format!(
                                "building {:?}, fingerprint {:016x}",
                                frame.building(),
                                frame.fingerprint()
                            )
                        },
                        |(min, max)| format!("z={min}..={max}, fingerprint {:016x}", frame.fingerprint()),
                    )
                },
            ),
        );
        line("land", format!("{} graphics", self.land.len()));
        line("texmaps", format!("{} textures", self.texmaps.len()));
        line(
            "statics",
            format!(
                "{} graphics, revision {}",
                self.statics.len(),
                self.statics.revision()
            ),
        );
        line("sky", format!("{:?}", self.sky));
        line("sun", format!("{:?}", self.sun));
        line("carried", format!("{:?}", self.carried));
        line("tuning", format!("{:?}", self.tuning));
        line("flame_time", format!("{:.6}s", self.flame_time));
        // Whether, and not what: a bake is a cost rather than a picture (see the
        // field), so what a comparison wants to know is which of the two ways to
        // reach the same grid this caller took.
        line(
            "bake",
            match self.bake {
                Some(_) => "kept across frames".to_owned(),
                None => "built from nothing".to_owned(),
            },
        );
        line("highlight", format!("{:?}", self.highlight));
        line("impostor", format!("{:?}", self.impostor));
        // **Named producers and not `{:?}` of the struct**, because this is the
        // line a person scans for "why is there no floor in this dump": four
        // booleans in field order read as four booleans, and `land statics` reads
        // as what was drawn. `everything` is the ordinary frame, said in a word
        // so that a filtered dump stands out beside an unfiltered one.
        let drawn: Vec<&str> = [
            (self.draw.land, "land"),
            (self.draw.statics, "statics"),
            (self.draw.items, "items"),
            (self.draw.mobiles, "mobiles"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name)
        .collect();
        line(
            "draw",
            match (self.draw == Draw::EVERYTHING, drawn.is_empty()) {
                (true, _) => "everything".to_owned(),
                (false, true) => "nothing".to_owned(),
                (false, false) => drawn.join(" "),
            },
        );
        line("view", self.view.name().to_owned());
        line("dead", format!("{}", self.dead));
        line("player_rect", format!("{:?}", self.player_rect));
        line(
            "player_mask",
            match self.player_mask {
                Some(_) => "body silhouette".to_owned(),
                None => "none".to_owned(),
            },
        );
        out
    }
}

/// One frame's geometry and the light on it.
///
/// The three lists are exactly what the passes take, and they are separate
/// because the passes are: the ground clears the targets and draws land, the
/// sprite pass draws every static and item there is, and the mesh pass draws
/// whatever faces the geometry named.
#[derive(Debug)]
pub struct Frame {
    /// The flames, the grid they are occluded by, the ambient, and the two
    /// per-fragment knobs — everything the lighting pass reads.
    pub lighting: Lighting,
    /// The land, back to front.
    pub ground: Vec<GroundQuad>,
    /// The map's furniture and the server's items, as one set — see
    /// [`StaticGeometry::absorb`] for why joining them is not three `extend`s.
    pub statics: StaticGeometry,
}

/// The same frame before immutable map statics and server-owned items are
/// joined for the shared sprite pass.
///
/// The ordinary [`Frame`] deliberately preserves the historical joined shape:
/// most callers draw one static pass and do not need to know where a row came
/// from.  A block-composite renderer does need that distinction, however.  It
/// may replace only map rows; a row sent by the server must remain a live
/// producer with a current-frame id, depth and selection mask.  Keeping the
/// split at the assembly boundary means that policy cannot be reconstructed
/// later from two visually identical `SpriteQuad`s.
#[derive(Debug)]
pub struct SplitFrame {
    /// Lighting for the complete current world.
    pub lighting: Lighting,
    /// Immutable map land, before a composite consumer elects to omit blocks.
    pub ground: Vec<GroundQuad>,
    /// Immutable map statics only.
    pub map_statics: StaticGeometry,
    /// Server-owned ground items only.
    pub items: StaticGeometry,
}

/// CPU cost of the three map walks [`assemble_profiled`] makes.
///
/// This is diagnostic data rather than part of the picture: the ordinary
/// [`assemble`] API deliberately keeps returning only its [`Frame`]. The app
/// keeps these measurements beside a jank record to distinguish the lighting
/// grid from ground and sprite collection at large zoom-outs.
#[derive(Clone, Copy, Debug, Default)]
pub struct AssemblyCosts {
    /// Lights and the occlusion grid.
    pub lighting: Duration,
    /// Land-tile quads.
    pub ground: Duration,
    /// Map statics and server items, including their impostor volumes.
    pub statics: Duration,
    /// Map-static placement, culling and volume construction.
    pub static_walk: Duration,
    /// Stable ordering of opaque and cutaway map statics.
    pub static_sort: Duration,
    /// Whether the visible map-static list had clock-driven art this frame.
    pub static_animated: bool,
    /// Server-owned items folded into the static render pass.
    pub items: Duration,
}

/// **Assemble one frame.** The only place the four collectors are called in
/// sequence.
///
/// # The order, and why it is this one
///
/// The grid before the pictures. A drawn static's row carries the number the
/// occlusion grid gave it ([`Occlusion::owner_at`]), so that a fragment of it
/// can say which occluder it is a point of instead of having that guessed from
/// its height — and a frame that collected its pictures first would stamp them
/// with numbers off the grid of the frame before. `docs/lighting_height.md`
/// phase 3 is where that cost was taken on.
///
/// Then the ground, then the map's statics, then the server's items absorbed
/// into them. The last two are one list because they are one pass over one
/// atlas, and three of [`StaticGeometry`]'s four fields are addressed by index.
pub fn assemble(inputs: Inputs<'_>) -> Frame {
    assemble_profiled(inputs).0
}

/// [`assemble`], with the cost of each map walk kept for a caller's profiler.
///
/// The geometry is byte-for-byte the same as [`assemble`]'s: timing surrounds
/// existing phases only. Most callers should use [`assemble`]; the client uses
/// this when it needs a jank log that can tell which collection phase grew.
pub fn assemble_profiled(inputs: Inputs<'_>) -> (Frame, AssemblyCosts) {
    let (split, costs) = assemble_split_profiled(inputs);
    let SplitFrame {
        lighting,
        ground,
        mut map_statics,
        items,
    } = split;
    map_statics.absorb(items);
    (
        Frame {
            lighting,
            ground,
            statics: map_statics,
        },
        costs,
    )
}

/// Assemble a frame without joining immutable map statics to dynamic items.
///
/// [`assemble_profiled`] remains the compatibility entry point for consumers
/// that have one static pass.  The client far-LOD path calls this form so it
/// can replace *only* map geometry with deferred block composites before it
/// draws server items and mobiles.
pub fn assemble_split_profiled(inputs: Inputs<'_>) -> (SplitFrame, AssemblyCosts) {
    let Inputs {
        map,
        items,
        camera,
        tiledata,
        animations,
        cutaway,
        interior,
        land,
        texmaps,
        statics,
        sky,
        sun,
        carried,
        tuning,
        flame_time,
        bake,
        highlight,
        impostor,
        draw,
        view,
        dead,
        player_rect,
        player_mask,
        fades,
    } = inputs;

    let lighting_started = Instant::now();
    let mut lighting = match sky {
        Some(ambient) => light::collect_with_interior(
            map,
            items,
            camera,
            tiledata,
            cutaway,
            ambient,
            tuning,
            flame_time,
            // The pictures, which is where an occluder's *facing* comes from: a
            // wall stops a ray only where the ray crosses the side the wall
            // stands on, and only the art says which side that is. The same
            // atlas the statics pass below draws from, so the grid and the
            // picture cannot be about two different sets of sprites.
            Some(statics),
            bake,
            interior,
        ),
        // No sky, no grid, no walk. The identity — see [`Lighting::NONE`].
        None => Lighting::NONE,
    };
    lighting.sun = sun;
    // Only where the frame has a sky at all: with no ambient the pass is a copy,
    // and a beam over an already-white multiplier would cost a loop to change
    // nothing. After the sort, and `hold` is what says this flame is never the
    // one dropped when a tavern's candles fill the array.
    if let Some((at, offset, facing)) = carried.filter(|_| sky.is_some()) {
        lighting.hold(tuning.applied_headlight(light::carried(at, offset, facing, flame_time)));
    }
    lighting.view = view;
    lighting.dead = dead;
    let lighting_cost = lighting_started.elapsed();

    // **The lighting above was collected from the whole world, and only the
    // drawing below is filtered** — see [`Draw`]. A frame with the crowd ticked
    // off is the same street, lit the same way, with nobody standing in front of
    // the wall a person is trying to look at.
    let ground_started = Instant::now();
    let ground = match draw.land {
        true => crate::ground::collect_with_interior(map, camera, land, texmaps, cutaway, interior),
        false => Vec::new(),
    };
    let ground_cost = ground_started.elapsed();

    // What the sprites are met against. `Occlusion::EMPTY` is not a second way
    // to assemble a frame — it is what [`Impostor::Billboards`] *is*, named at
    // one place instead of passed at four.
    //
    // Named rather than written inline, because a `const`'s reference is a
    // temporary and the calls below outlive it.
    let no_grid = Occlusion::EMPTY;
    let met = match impostor {
        Impostor::Met => &lighting.occlusion,
        Impostor::Billboards => &no_grid,
    };

    // **An empty list and not an empty *map*.** Both halves below are switched
    // off by collecting nothing, which leaves everything else about the frame —
    // the grid, the flames, the ambient the tiles were given — exactly as it was.
    // Reaching the same picture by handing this function fewer items would be a
    // different world, lit differently, and the summary would not say so.
    let statics_started = Instant::now();
    let (map_statics, static_costs) = match draw.statics {
        true => crate::statics::collect_with_fades_profiled_with_interior(
            map,
            camera,
            tiledata,
            animations,
            statics,
            cutaway,
            met,
            player_rect,
            player_mask,
            fades,
            interior,
        ),
        false => (StaticGeometry::default(), crate::statics::CollectCosts::default()),
    };
    // Through the same pass as the map's statics, because they are the same
    // atlas: one draw call binds one texture, and what covers what is the depth
    // these carry rather than the order they are appended in.
    let items_started = Instant::now();
    let items = if draw.items {
        crate::items::collect_with_fades_with_interior(
            items,
            camera,
            tiledata,
            animations,
            statics,
            cutaway,
            highlight,
            met,
            player_mask,
            fades,
            interior,
        )
    } else {
        StaticGeometry::default()
    };
    let items_cost = items_started.elapsed();

    let statics_cost = statics_started.elapsed();
    (
        SplitFrame {
            lighting,
            ground,
            map_statics,
            items,
        },
        AssemblyCosts {
            lighting: lighting_cost,
            ground: ground_cost,
            statics: statics_cost,
            static_walk: static_costs.walk,
            static_sort: static_costs.sort,
            static_animated: static_costs.animated,
            items: items_cost,
        },
    )
}
