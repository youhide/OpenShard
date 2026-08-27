//! A picture of one real place, and only what the caller asks to keep in it.
//!
//! The tool `docs/lighting.md`'s backlog wanted while chasing a corner where a
//! staircase's occlusion box met a lamp's soft pool: `OPENSHARD_FRAME_AT`
//! (`tests/cost.rs`) repoints the camera at a real place but still draws the
//! whole neighbourhood around it, and a house standing beside the thing under
//! test is a second variable in every picture. This draws a **synthetic**
//! [`openshard_map::map::WorldMap`] instead — `WorldMap::from_blocks` never carries
//! statics (see `crate::scene`'s own doc) — and puts back only what is asked
//! for: the real map's statics within a stated radius of a stated point,
//! optionally filtered to a list of tile IDs, the real ground under them or
//! none at all, everything the *shard* placed over the same window (see
//! [`shard`] — its decorations and dropped items, which no client file knows
//! about), and any hand-named extra items. Turn every knob down and what is
//! left is one tile.
//!
//! Real coordinates translate onto a fixed synthetic anchor
//! ([`SYN_ANCHOR_NEAR_THE_ORIGIN`]) the same way `crate::scene::CENTRE` does it for the
//! built-in scenes — far from the synthetic map's own edge, so a camera never
//! runs off it.
//!
//! # Knobs, all environment variables so no edit-run-revert is ever needed
//!
//! - `OPENSHARD_CLIENT` — the client's files. Required, like every tool here.
//! - `OPENSHARD_SCENE_AT=x,y,z` — the real place. Required: it is both the
//!   centre of the pulled radius and, unless `OPENSHARD_SCENE_LOOK` says
//!   otherwise, where the camera looks.
//! - `OPENSHARD_SCENE_LOOK=x,y,z` — where the camera looks, if not `_AT`.
//! - `OPENSHARD_SCENE_RADIUS=n` — pull real statics from the `(2n+1)^2` tiles
//!   around `_AT`. `0` (the default) is the one tile `_AT` stands on.
//! - `OPENSHARD_SCENE_STATICS=0` — skip the real map's statics entirely
//!   (default: pull them).
//! - `OPENSHARD_SCENE_TILES=0x0739,0x0738` — keep only these tile IDs among
//!   whatever the radius pulled. Unset keeps everything pulled.
//! - `OPENSHARD_SCENE_GROUND=0` — draw no land at all: an empty atlas, so the
//!   ground pass still runs (it always clears) but paints nothing. Default is
//!   the real land, read live off the same facet.
//! - `OPENSHARD_SCENE_SHARD=0` — read nothing out of the shard's database.
//!   Default is to read it, because **half of what a player is looking at in a
//!   town is not on the map**: a pack's decorations and everything anyone has
//!   dropped live in `openshard.db`, and a tool that reads only `statics.mul`
//!   answers "it does not reproduce" about every one of them. Both tables are
//!   pulled over `_AT ± (_RADIUS + light::light_margin_tiles(tuning))`, wider
//!   than the statics' own `_AT ± _RADIUS` — a lamp just outside `_RADIUS`
//!   still lights the scene under it, and this reader has to see the lamp to
//!   find that out. `_TILES` filters both windows the same way. Turning the
//!   reader off is a legitimate answer — the map's art alone — and it is why
//!   every failure to read is a panic naming this knob rather than an empty
//!   result.
//! - `OPENSHARD_SCENE_CONFIG=/path/openshard.toml` — whose shard. Default
//!   `openshard.toml` in the current directory, and its `[persistence].database`
//!   is where the world is (a relative path resolved against the config's own
//!   directory, which is where a shard is run from).
//! - `OPENSHARD_SCENE_SHARD_DB=/path/openshard.db` — that database directly,
//!   for a save that no config names. SQLite only; a `postgres://` config is a
//!   panic pointing here.
//! - `OPENSHARD_SCENE_EXTRA=x,y,z,graphic[,hue];...` — items to add by hand,
//!   semicolon-separated. No longer how a live decoration gets in — the reader
//!   above is — and still how a *hypothetical* one does: a torch put where no
//!   torch is, to find out what it would light.
//! - `OPENSHARD_SCENE_HOUSE=serial` — expand this classic house from the
//!   shard's `houses` table through the client's `multi.mul`. A house is a
//!   multi, rather than an ordinary item, so the regular shard reader cannot
//!   otherwise put its floors and walls in the diagnostic frame.
//! - `OPENSHARD_SCENE_CARRIED=x,y,z,direction` — give the pictured player a
//!   lantern at this real map position. `direction` is `N`, `NE`, `E`, `SE`,
//!   `S`, `SW`, `W`, `NW`, or wire value `0..7`; no walking offset is applied.
//! - `OPENSHARD_SCENE_NO_ROOFS=1`, `OPENSHARD_SCENE_MAX_Z=n` — the frame's own
//!   [`Cutaway`], which the client has and this tool did not: no roof drawn
//!   anywhere, and nothing at or above a height. What a player standing indoors
//!   is looking at, and the honest way to ask whether a defect belongs to the
//!   surface under a roof or to the roof itself.
//! - `OPENSHARD_SCENE_ANCHOR_REAL=1` — build the scene at the place's *own*
//!   coordinates instead of translating it next to the synthetic map's origin.
//!   Everything the impostor is met against is absolute and reaches the shader
//!   in `f32`, so the translation makes every tie at a shared plane some sixteen
//!   times more precisely decided here than in the client — see [`syn_anchor`].
//!   Default `0`, because the near-origin map is a few megabytes and a real one
//!   is a couple of hundred blocks a side.
//! - `OPENSHARD_SCENE_NO_FOOTPRINTS=1` — throw away every footprint the art
//!   measured ([`StaticAtlas::forget_footprints`]), so every picture in this
//!   scene stands on the whole tile again: the boxes that shipped before
//!   `docs/footprints.md`'s S3. The "before" half of a before-and-after pair,
//!   and the only way to get one out of a *drawn* frame — the measurement is a
//!   property of the art, so nothing else in the scene can state it away.
//!   Default `0`.
//! - `OPENSHARD_SCENE_IMPOSTOR=0` — meet no sprite against the grid, the way the
//!   live client draws with its lights *off*: `App::draw` builds no grid at all
//!   there, so every fragment takes `statics.wesl`'s billboard fallback. The
//!   client's F10 is this switch and nothing else as far as `View::Normal` is
//!   concerned, so two dumps either side of it are what a face that "disappears
//!   when the light goes on" actually is. Default `1`.
//! - `OPENSHARD_SCENE_VIEWPORT=960x720` — any size: the readback
//!   ([`openshard_client_render::dump::read_rect`]) pads its own rows. It used
//!   to want `width * 4` a multiple of 256 and panic otherwise, which was this
//!   file's own copy of the arithmetic showing through.
//! - `OPENSHARD_FLAME_RADIUS`, `OPENSHARD_SHADOW_RAYS`,
//!   `OPENSHARD_LIGHT_BRIGHTNESS`, `OPENSHARD_LIGHT_REACH`,
//!   `OPENSHARD_LIGHT_SKY`, `OPENSHARD_LIGHT_GROUND` — the client's own Light
//!   tab, as environment variables. See [`env_tuning`]: what a person is looking
//!   at in the client is reproducible here, which is where it can be dumped and
//!   diffed.
//! - `OPENSHARD_SCENE_ZOOM=n` — `n` notches of [`Zoom::scale_up`] past
//!   [`Zoom::ONE`], the live client's own wheel-in ladder (`2:1`, `3:1`,
//!   `4:1`, nearest-sampled so a texel still lands on a whole number of real
//!   pixels — see [`Zoom`]'s own doc for why that matters). Default `0`
//!   (`Zoom::ONE`, one world pixel to one viewport pixel). A crop blown up
//!   afterwards with an image tool is a *different* resample — bilinear or
//!   nearest, neither the client's own rule — and can show banding neither
//!   this tool nor the client actually draws; this is the honest way to look
//!   closer.
//! - `OPENSHARD_FRAME_DUMP=/tmp/x.png` — where to write the picture. Required;
//!   this tool has nothing else to do with a frame once it is drawn.
//! - `OPENSHARD_FRAME_VIEW=n` — index into `debug::View::ALL`, which is an
//!   order and not the enum's own numbering: `0` `Lit`, `1` `Place`, `2` `Kind`,
//!   `3` `Height`, `4` `Normal`, `5` `NormalGeometry`, `6` `NormalSprites`,
//!   `7` `SilhouetteArt`, `8` `SilhouetteBox`, `9` `Solid`, `10` `Occluders`,
//!   `11` `Light`, `12` `Flames`, `13` `Shadow`, `14` `Reach`, `15` `Sun`,
//!   `16` `Sky`. Default `Lit`. Ignored under `_SOLIDS`. The list is spelled
//!   out because it has shifted three times under readers who had memorised a
//!   number — and it had shifted a fourth by the time somebody asked this tool
//!   for `11` `Shadow` and was handed `Light`, which is why the two
//!   `Silhouette` views are in it now.
//!
//! # The occlusion grid alone, nothing drawn to cast a shadow of its own
//!
//! `OPENSHARD_SCENE_SOLIDS=white|lit` skips every sprite and every blade of
//! ground and draws only [`solid::standing`]'s boxes on black — the same pass
//! (`solids::SolidsRenderer`) the live client's own F5 overlay uses. Ground and
//! sprites are still built and drawn to their own offscreen textures first —
//! `_GROUND` and `_TILES` still apply to what is *collected into the grid* —
//! they are simply never composited into the picture this mode reads back.
//!
//! - `white` — one flat colour, so the geometry reads on its own with nothing
//!   about the flame in it.
//! - `lit` — each visible face coloured by one real [`light::sample`] taken at
//!   its own centre (the lid at `Surface::Flat`, the two sides at their own
//!   `Surface::Face`) — the flame's own light on the grid's own boxes, with no
//!   sprite's shading to tell it apart from.
//!
//! `OPENSHARD_SCENE_SOLIDS_EDGES=0` drops the black outline stroke, leaving the
//! translucent fills alone — see `SolidsRenderer::render`'s own doc for why
//! that is what shows a face wrongly surviving behind one that should have hidden
//! it. Default `1`, matching the live F5 overlay.
//!
//! `OPENSHARD_SCENE_SOLIDS_OPAQUE=1` swaps translucent fills for a straight
//! overwrite: a later, nearer face then genuinely hides an earlier, farther one
//! (painter's algorithm, on `solid::standing`'s own back-to-front order) rather
//! than tinting through it. Default `0`, matching the live F5 overlay — turn it
//! on to tell "hidden behind something nearer" from "showing through it".
//!
//! # Profiling a segment instead of drawing it
//!
//! Setting `OPENSHARD_SCENE_PROFILE_FACE` switches the tool from a picture to a
//! printed table and skips every GPU pass after the scene's lighting is built —
//! `OPENSHARD_FRAME_DUMP`/`_VIEW` are ignored in this mode. For a question a
//! picture cannot answer on its own: whether a hard edge in the render is the
//! occlusion walk (`Reach::through`) or the face's `faces()` cutoff folded into
//! `Reach::cone` (see `light.rs`'s own doc on the two).
//!
//! - `OPENSHARD_SCENE_PROFILE_FACE=north|east|south|west|flat|upright` — which
//!   [`light::Surface`] to sample. The four faces are `Spot::face(...)`'s
//!   `Face`. Required to enter profile mode.
//! - `OPENSHARD_SCENE_PROFILE_FROM=x,y,z` / `_TO=x,y,z` — the segment to walk,
//!   in real (fractional allowed) map coordinates.
//! - `OPENSHARD_SCENE_PROFILE_STEPS=n` — how many samples along it, both ends
//!   inclusive. Default `40`.
//! - `OPENSHARD_SCENE_PROFILE_LIGHT=n` — print only [`light::Reach`]s for this
//!   light index. Default: every light the scene collected.
//!
//! ```sh
//! OPENSHARD_CLIENT=… \
//!     OPENSHARD_SCENE_AT=1497,1626,10 OPENSHARD_SCENE_TILES=0x0739,0x0738 \
//!     OPENSHARD_SCENE_GROUND=0 OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
//!     OPENSHARD_SCENE_PROFILE_FACE=south \
//!     OPENSHARD_SCENE_PROFILE_FROM=1497.0,1627.0,10 OPENSHARD_SCENE_PROFILE_TO=1497.0,1627.0,16 \
//!     cargo run --release -p openshard-client-render --example isolated_scene
//! ```
//!
//! # Example: the one tile that made the corner in the user's screenshot
//!
//! ```sh
//! OPENSHARD_CLIENT=… \
//!     OPENSHARD_SCENE_AT=1497,1626,10 OPENSHARD_SCENE_TILES=0x0739,0x0738 \
//!     OPENSHARD_SCENE_GROUND=0 \
//!     OPENSHARD_SCENE_EXTRA=1498,1626,10,2852 \
//!     OPENSHARD_FRAME_DUMP=/tmp/corner.png OPENSHARD_FRAME_VIEW=5 \
//!     cargo run --release -p openshard-client-render --example isolated_scene
//! ```

use std::path::PathBuf;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{LandAtlas, StaticAtlas, TexmapAtlas};
use openshard_client_render::blit::{Blit, ViewportRect};
use openshard_client_render::camera::{Camera, RealPixel, Zoom};
use openshard_client_render::cutaway::Cutaway;
use openshard_client_render::geometry::Vec2;
use openshard_client_render::ground;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::renderer::{GroundRenderer, MeshFaceRenderer, SpriteRenderer, Target};
use openshard_client_render::{light, renderer};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_protocol::direction::Direction;
use openshard_protocol::items::ItemAmount;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;
use openshard_uofiles::art::Art;
use openshard_uofiles::multi::Multis;
use openshard_uofiles::texmaps::TexMaps;

// The shard's own `items` and `decorations`, without which this tool draws the
// client's art and none of the server's furniture — see its own doc, and
// `docs/parity.md`'s first backlog entry for what that cost.
mod shard;

/// Where every scene's real anchor lands in the synthetic map — far from its
/// edge, the same convention `crate::scene::CENTRE` uses.
const SYN_ANCHOR_NEAR_THE_ORIGIN: (u16, u16) = (100, 100);

/// Which facet every part of this scene comes off.
///
/// Felucca, and named once rather than typed at each reader: the map, the land
/// and the shard's own rows have to agree about it, and a `0` written out three
/// times is three places for them to stop agreeing. A knob when a second facet
/// is worth pointing this tool at; a constant until then.
const FACET: u8 = 0;

/// Where the scene actually goes, which is [`SYN_ANCHOR_NEAR_THE_ORIGIN`] unless
/// `OPENSHARD_SCENE_ANCHOR=real` says to leave it where the map has it.
///
/// **The translation is not free, and this is the knob that says so.** Every
/// coordinate the impostor is met against — [`crate::occlusion::Solid`]'s own
/// `space`, the fragment's position, the ray — is *absolute* and lands in `f32`
/// on the way to the shader. An ulp at `100` is about `7.6e-6` and an ulp at
/// `1660` is about `1.2e-4`: sixteen times coarser. Any answer that turns on a
/// tie at a shared plane — and a seam between two abutting statics is exactly
/// that — is therefore decided at a different precision here than in the client,
/// which is a difference no amount of pointing this tool at the right
/// coordinates can remove. `real` removes it instead.
///
/// The cost is the synthetic map, which must then be big enough to hold the real
/// place: 8×8 tiles a block, so Britain wants a couple of hundred blocks a side
/// against the default sixteen. It is land cells and no statics, so it is
/// megabytes rather than anything to think about.
fn syn_anchor(real: (u16, u16)) -> (u16, u16) {
    match env_flag("OPENSHARD_SCENE_ANCHOR_REAL", false) {
        true => real,
        false => SYN_ANCHOR_NEAR_THE_ORIGIN,
    }
}

/// A required environment variable, or a clear panic naming which one.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The Light tab's own knobs, as environment variables — so that a picture a
/// person is looking at in the client can be reproduced here, where it can be
/// dumped, diffed and bisected.
///
/// The same fields and the same clamp as the tab: this builds a
/// [`light::Tuning`] and lets [`light::Tuning::clamped`] have the last word, so
/// a number refused here is refused there.
fn env_tuning() -> light::Tuning {
    let number = |name: &str, default: f32| -> f32 {
        env_opt(name).map_or(default, |value| {
            value.parse().unwrap_or_else(|_| panic!("{name}: {value:?}"))
        })
    };
    light::Tuning {
        flame_radius: number("OPENSHARD_FLAME_RADIUS", light::Tuning::DEFAULT.flame_radius),
        shadow_rays: light::ShadowRays::new(number(
            "OPENSHARD_SHADOW_RAYS",
            light::Tuning::DEFAULT.shadow_rays.raw() as f32,
        ) as u32),
        brightness: number("OPENSHARD_LIGHT_BRIGHTNESS", 1.0),
        reach: number("OPENSHARD_LIGHT_REACH", 1.0),
        sky: number("OPENSHARD_LIGHT_SKY", 1.0),
        ground: number("OPENSHARD_LIGHT_GROUND", 1.0),
        sun: light::Tuning::DEFAULT.sun,
        headlight_color: light::Tuning::DEFAULT.headlight_color,
        lantern_color: light::Tuning::DEFAULT.lantern_color,
        ambient_color: light::Tuning::DEFAULT.ambient_color,
    }
    .clamped()
}

fn env_flag(name: &str, default: bool) -> bool {
    match env_opt(name).as_deref() {
        None => default,
        Some("0") => false,
        Some("1") => true,
        Some(other) => panic!("{name} wants `0` or `1`, got {other:?}"),
    }
}

/// `x,y,z`, the same shape `tests/cost.rs`'s `OPENSHARD_FRAME_AT` reads.
fn parse_point(spec: &str) -> Point {
    let coords: Vec<&str> = spec.split(',').collect();
    let [x, y, z] = coords[..] else {
        panic!("wanted `x,y,z`, got {spec:?}");
    };
    Point::new(
        x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
        y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
        z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
    )
}

/// `x,y,z,direction`, a stationary carried lantern in real map coordinates.
fn parse_carried(spec: &str) -> (Point, Direction) {
    let parts: Vec<&str> = spec.split(',').collect();
    let [x, y, z, direction] = parts[..] else {
        panic!("OPENSHARD_SCENE_CARRIED wants `x,y,z,direction`, got {spec:?}");
    };
    let direction = match direction.trim().to_ascii_uppercase().as_str() {
        "N" | "0" => Direction::North,
        "NE" | "1" => Direction::NorthEast,
        "E" | "2" => Direction::East,
        "SE" | "3" => Direction::SouthEast,
        "S" | "4" => Direction::South,
        "SW" | "5" => Direction::SouthWest,
        "W" | "6" => Direction::West,
        "NW" | "7" => Direction::NorthWest,
        other => panic!("carried-light direction: {other:?}"),
    };
    (
        Point::new(
            x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
            y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
            z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
        ),
        direction,
    )
}

/// A tile ID, decimal or `0x`-prefixed hex — the two shapes `tiledata` dumps
/// and Sphere-flavoured docs both use.
fn parse_tile_id(spec: &str) -> u16 {
    let spec = spec.trim();
    match spec.strip_prefix("0x").or_else(|| spec.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("tile id: {spec:?}")),
        None => spec.parse().unwrap_or_else(|_| panic!("tile id: {spec:?}")),
    }
}

/// `x,y,z,graphic[,hue]`, one item.
fn parse_extra_item(spec: &str) -> GroundItem {
    let parts: Vec<&str> = spec.split(',').collect();
    let (x, y, z, graphic, hue) = match parts[..] {
        [x, y, z, graphic] => (x, y, z, graphic, "0"),
        [x, y, z, graphic, hue] => (x, y, z, graphic, hue),
        _ => panic!("OPENSHARD_SCENE_EXTRA item wants `x,y,z,graphic[,hue]`, got {spec:?}"),
    };
    GroundItem {
        amount: ItemAmount::ONE,
        at: Point::new(
            x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
            y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
            z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
        ),
        graphic: Graphic(parse_tile_id(graphic)),
        hue: Hue(hue.trim().parse().unwrap_or_else(|_| panic!("hue: {hue:?}"))),
    }
}

/// `x,y,z`, all three fractional — [`parse_point`]'s shape, but for a spot
/// [`run_profile`] wants to place anywhere inside a tile rather than on one.
fn parse_fpoint(spec: &str) -> (f32, f32, f32) {
    let coords: Vec<&str> = spec.split(',').collect();
    let [x, y, z] = coords[..] else {
        panic!("wanted `x,y,z`, got {spec:?}");
    };
    (
        x.trim().parse().unwrap_or_else(|_| panic!("x: {x:?}")),
        y.trim().parse().unwrap_or_else(|_| panic!("y: {y:?}")),
        z.trim().parse().unwrap_or_else(|_| panic!("z: {z:?}")),
    )
}

/// `OPENSHARD_SCENE_PROFILE_FACE`'s value, which names a whole [`light::Surface`]
/// and not just a [`Face`](openshard_client_render::facing::Face) — `flat` and
/// `upright` are the other two the place attachment can carry.
fn parse_surface(spec: &str) -> light::Surface {
    use openshard_client_render::facing::Face;
    match spec.trim().to_ascii_lowercase().as_str() {
        "north" => light::Surface::Face(Face::North),
        "east" => light::Surface::Face(Face::East),
        "south" => light::Surface::Face(Face::South),
        "west" => light::Surface::Face(Face::West),
        "flat" => light::Surface::Flat,
        "upright" => light::Surface::Upright,
        _ => panic!("OPENSHARD_SCENE_PROFILE_FACE wants north/east/south/west/flat/upright, got {spec:?}"),
    }
}

/// [`shift`], but for the fractional coordinates [`run_profile`] samples at
/// rather than the whole tiles the rest of this file moves onto the synthetic
/// map.
fn shift_f(anchor: (u16, u16), x: f32, y: f32) -> (f32, f32) {
    (
        x - f32::from(anchor.0) + f32::from(syn_anchor(anchor).0),
        y - f32::from(anchor.1) + f32::from(syn_anchor(anchor).1),
    )
}

/// The profile mode: walk [`Spot::face`](light::Spot::face) along a segment and
/// print what [`light::sample`] says at each step, instead of drawing a frame.
///
/// `through` and `cone` are read straight off [`light::Reach`] rather than
/// re-derived here, which is the whole point — they are the production
/// function's own account of "was something in the way" and "was the surface
/// turned towards the flame", already kept apart for exactly this question. For
/// a scene with one point light (no [`crate::light::Beam`]), `cone` *is* the
/// `faces()` term alone.
fn run_profile(anchor: (u16, u16), lighting: &light::Lighting) {
    let face_spec = env("OPENSHARD_SCENE_PROFILE_FACE");
    let surface = parse_surface(&face_spec);
    let (fx, fy, fz) = parse_fpoint(&env("OPENSHARD_SCENE_PROFILE_FROM"));
    let (tx, ty, tz) = parse_fpoint(&env("OPENSHARD_SCENE_PROFILE_TO"));
    let steps: u32 = env_opt("OPENSHARD_SCENE_PROFILE_STEPS")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_PROFILE_STEPS: {s:?}"))
        })
        .unwrap_or(40);
    let only_light: Option<usize> = env_opt("OPENSHARD_SCENE_PROFILE_LIGHT").map(|s| {
        s.parse()
            .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_PROFILE_LIGHT: {s:?}"))
    });

    println!("profile: surface {face_spec}, {steps} steps from ({fx},{fy},{fz}) to ({tx},{ty},{tz})");
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y, z) = (fx + (tx - fx) * t, fy + (ty - fy) * t, fz + (tz - fz) * t);
        let (sx, sy) = shift_f(anchor, x, y);
        let spot = light::Spot {
            at: Vec2::new(sx, sy),
            z,
            // A point in the world rather than a fragment of one of its statics:
            // this tool walks a segment through the scene, so it is a point of no
            // solid and exempt from none. See `light::Spot::solid`.
            solid: None,
            // `floor()`, deliberately: this tool exists to bisect exactly the
            // ambiguity `Spot::tile` closes for a real caller
            // (`docs/lighting_raymarch.md` step 2), so the profile has to keep
            // showing what a naively-derived tile does at a boundary sample,
            // not paper over it with a guessed intent.
            tile: (sx.floor() as i32, sy.floor() as i32),
            surface,
        };
        let sample = light::sample(spot, lighting);
        print!(
            "t={t:.3} ({x:.2}, {y:.2}, z {z:.1}) -> brightness {:.3}",
            sample.brightness()
        );
        for reach in &sample.reaches {
            if only_light.is_some_and(|want| want != reach.light.position()) {
                continue;
            }
            match (reach.within, reach.stopped_by) {
                (false, _) => print!("  | light {}: outside radius", reach.light.position()),
                (true, Some(stopper)) => print!("  | light {}: stopped by {stopper}", reach.light.position()),
                (true, None) => print!(
                    "  | light {}: visible {:.3} delivers {:.3}",
                    reach.light.position(),
                    reach.through,
                    reach.delivered
                ),
            }
        }
        println!();
    }
}

/// One real [`light::sample`] per *corner* of every visible face of a drawn
/// solid — `[Top, East, South]`, [`openshard_client_render::solid::Solid::faces`]'s
/// own order, and within each a
/// [`solids::FaceColours`](openshard_client_render::solids::FaceColours) in
/// that same function's own four-corner ring order — as an RGB colour:
/// [`light::Sample::multiplier`] directly, clamped into `0..=255` rather than
/// reduced to [`light::Sample::brightness`]'s one number, so the flame's own
/// colour survives onto the box.
///
/// Four samples a face rather than one at its centre, so `SolidsRenderer`'s
/// ordinary per-vertex colour interpolation turns them into a gradient across
/// the fill instead of a flat tint with a step at every face's edge — see
/// `FaceColours`'s own doc for what this still is not: the real, non-linear
/// falloff, only four real points of it linearly blended.
fn face_light(
    solid: &openshard_client_render::solid::Solid,
    lighting: &light::Lighting,
) -> [openshard_client_render::solids::FaceColours; 3] {
    use openshard_client_render::facing::Face;
    use openshard_client_render::solids::FaceColours;

    let (lo, hi) = (solid.min, solid.max);
    let colour = |x: f64, y: f64, z: f64, surface: light::Surface| {
        let spot = light::Spot {
            at: Vec2::new(x as f32, y as f32),
            z: z as f32,
            tile: (x.floor() as i32, y.floor() as i32),
            surface,
            // The wireframe's own face colours: a probe of a *place*, not of a
            // drawn fragment, so it names no solid and is exempt from nothing —
            // which is what makes the picture show every solid's real effect,
            // including the one being coloured.
            solid: None,
        };
        let multiplier = light::sample(spot, lighting).multiplier;
        [
            multiplier[0].clamp(0.0, 1.0) * 255.0,
            multiplier[1].clamp(0.0, 1.0) * 255.0,
            multiplier[2].clamp(0.0, 1.0) * 255.0,
        ]
    };
    // The same four corners `Solid::faces` hands `SolidsRenderer`, in the same
    // order, so corner `i`'s sample here is corner `i`'s vertex there.
    let top: FaceColours = [
        colour(lo.x, lo.y, hi.z, light::Surface::Flat),
        colour(hi.x, lo.y, hi.z, light::Surface::Flat),
        colour(hi.x, hi.y, hi.z, light::Surface::Flat),
        colour(lo.x, hi.y, hi.z, light::Surface::Flat),
    ];
    let east: FaceColours = [
        colour(hi.x, lo.y, hi.z, light::Surface::Face(Face::East)),
        colour(hi.x, hi.y, hi.z, light::Surface::Face(Face::East)),
        colour(hi.x, hi.y, lo.z, light::Surface::Face(Face::East)),
        colour(hi.x, lo.y, lo.z, light::Surface::Face(Face::East)),
    ];
    let south: FaceColours = [
        colour(lo.x, hi.y, hi.z, light::Surface::Face(Face::South)),
        colour(hi.x, hi.y, hi.z, light::Surface::Face(Face::South)),
        colour(hi.x, hi.y, lo.z, light::Surface::Face(Face::South)),
        colour(lo.x, hi.y, lo.z, light::Surface::Face(Face::South)),
    ];
    [top, east, south]
}

fn shift(anchor: (u16, u16), real: (u16, u16)) -> (u16, u16) {
    (
        (real.0 as i32 - anchor.0 as i32 + syn_anchor(anchor).0 as i32) as u16,
        (real.1 as i32 - anchor.1 as i32 + syn_anchor(anchor).1 as i32) as u16,
    )
}

fn unshift(anchor: (u16, u16), syn: (u16, u16)) -> (u16, u16) {
    (
        (syn.0 as i32 - syn_anchor(anchor).0 as i32 + anchor.0 as i32) as u16,
        (syn.1 as i32 - syn_anchor(anchor).1 as i32 + anchor.1 as i32) as u16,
    )
}

/// Reads `surface` back to PNG bytes — the one step every mode of this
/// tool ends in, whatever filled the texture before it was called.
///
/// Through `dump::read_rect`, which is the client's own readback: this file used
/// to carry a fourth copy of the padded-row arithmetic, and the copy is what
/// made `OPENSHARD_SCENE_VIEWPORT` insist on a 256-byte-aligned width.
fn dump_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: &wgpu::Texture,
    viewport: (u32, u32),
) -> Vec<u8> {
    let (w, h) = viewport;
    let pixels = openshard_client_render::dump::read_rect(
        device,
        queue,
        surface,
        ViewportRect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
    );
    openshard_client_render::png::encode_rgba(w, h, &pixels)
}

/// The picture where `OPENSHARD_FRAME_DUMP` asked for it, and the inputs it was
/// assembled from beside it under the same name.
///
/// Both, always, and through one function so that neither mode of this tool can
/// write a picture nobody can reproduce — which is what every dump before this
/// one was. The client's F12 writes the same summary from the same
/// [`Inputs::summary`](openshard_client_render::frame::Inputs::summary).
fn write_dump(path: &std::path::Path, png: &[u8], asked_for: &str) {
    std::fs::write(path, png).expect("writing the frame");
    std::fs::write(path.with_extension("inputs.txt"), asked_for).expect("writing the frame's inputs");
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: openshard_client_render::gbuffer::required_limits(),
        ..Default::default()
    }))
    .ok()
}

/// `960x720`-shaped, defaulting to a size whose row is already 256-byte
/// aligned — `wgpu`'s `COPY_BYTES_PER_ROW_ALIGNMENT`, which the readback at
/// the bottom of this file needs and does not itself enforce.
fn viewport() -> (u32, u32) {
    let Some(spec) = env_opt("OPENSHARD_SCENE_VIEWPORT") else {
        return (960, 720);
    };
    let (w, h) = spec
        .split_once('x')
        .unwrap_or_else(|| panic!("OPENSHARD_SCENE_VIEWPORT: {spec:?}"));
    // No alignment to keep any more: `dump::read_rect` pads the copy and strips
    // the padding back off, the way the client's own dump does. A width is
    // whatever the picture wants to be.
    (
        w.trim().parse().unwrap_or_else(|_| panic!("width: {w:?}")),
        h.trim().parse().unwrap_or_else(|_| panic!("height: {h:?}")),
    )
}

fn main() {
    let dir = PathBuf::from(env("OPENSHARD_CLIENT"));
    let (device, queue) = gpu().expect("an adapter");

    let real_map = openshard_uofiles::map::read_facet(&dir, FACET).expect("Felucca");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");

    let at = parse_point(&env("OPENSHARD_SCENE_AT"));
    let anchor = (at.x, at.y);
    let look = env_opt("OPENSHARD_SCENE_LOOK")
        .map(|s| parse_point(&s))
        .unwrap_or(at);

    let radius: u16 = env_opt("OPENSHARD_SCENE_RADIUS")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_RADIUS: {s:?}"))
        })
        .unwrap_or(0);
    // Read early: the shard window below needs it to widen past `radius`, and
    // `env_tuning` is pure — reading it twice would just be reading it twice.
    let tuning = env_tuning();
    let want_statics = env_flag("OPENSHARD_SCENE_STATICS", true);
    let tile_filter: Option<Vec<u16>> =
        env_opt("OPENSHARD_SCENE_TILES").map(|s| s.split(',').map(parse_tile_id).collect());
    let want_ground = env_flag("OPENSHARD_SCENE_GROUND", true);

    // The frame's own cutaway, which the client has and this tool never did —
    // one of the differences `docs/lighting_rebuild.md` lists between the two.
    // A roof is what a player standing indoors has cut away over their head, and
    // "does the artefact survive without it" is a question about which surface a
    // fragment belongs to rather than about the roof's own pixels.
    let cutaway = Cutaway {
        max_z: env_opt("OPENSHARD_SCENE_MAX_Z")
            .map(|v| {
                v.parse()
                    .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_MAX_Z: {v:?}"))
            })
            .unwrap_or(Cutaway::OPEN.max_z),
        no_draw_roofs: env_flag("OPENSHARD_SCENE_NO_ROOFS", false),
        ..Cutaway::OPEN
    };

    // Land, borrowed live from the real facet at every synthetic cell's real
    // coordinate — or nothing, `_GROUND=0`'s whole point. `WorldMap::from_blocks`
    // never carries statics regardless, so a house never comes along by
    // accident; keeping this a live read rather than a stored blob is what
    // lets `_RADIUS` and `_AT` move the scene to any corner of Britain with no
    // second file to keep in step.
    // Sixteen blocks a side is plenty around `SYN_ANCHOR_NEAR_THE_ORIGIN`, and
    // nowhere near enough to hold a real Britain coordinate — so the map is
    // sized from wherever the anchor actually landed, plus a frame's worth of
    // tiles so a camera never runs off it. See [`syn_anchor`].
    let syn = syn_anchor(anchor);
    let blocks = |along: u16| u32::from(along / 8 + 9).max(16);
    let synthetic = WorldMap::from_blocks(
        BlockExtent {
            wide: blocks(syn.0),
            down: blocks(syn.1),
        },
        |sx, sy| {
            if !want_ground {
                return LandCell {
                    tile: openshard_tiles::LandTileId(0),
                    z: at.z,
                };
            }
            let (rx, ry) = unshift(anchor, (sx, sy));
            real_map.land(rx, ry).unwrap_or(LandCell {
                tile: openshard_tiles::LandTileId(0),
                z: at.z,
            })
        },
    );

    // The real map's own statics within the radius, translated onto the
    // synthetic anchor and filtered to `_TILES` if it named any. This is what
    // makes the tool general: the caller does not hand-transcribe a tile's
    // graphic and `z` the way this file's first draft did — it is read here,
    // the same way `examples/dump_statics.rs` (this session's throwaway) read
    // it by hand.
    let mut items: Vec<GroundItem> = Vec::new();
    if want_statics {
        for x in at.x.saturating_sub(radius)..=at.x.saturating_add(radius) {
            for y in at.y.saturating_sub(radius)..=at.y.saturating_add(radius) {
                for s in real_map.statics_at(x, y) {
                    if tile_filter
                        .as_ref()
                        .is_some_and(|allowed| !allowed.contains(&s.tile.0))
                    {
                        continue;
                    }
                    let (sx, sy) = shift(anchor, (x, y));
                    items.push(GroundItem {
                        amount: ItemAmount::ONE,
                        at: Point::new(sx, sy, s.z),
                        graphic: s.tile,
                        hue: s.hue,
                    });
                }
            }
        }
    }

    let from_the_map = items.len();

    // **What the server placed**, read out of the shard's own database over a
    // window *wider* than the statics came from — `docs/parity.md`'s backlog:
    // "the shard reader windows by the tool's radius; the client windows by
    // what the server sent". `_RADIUS` is chosen to keep a house from standing
    // beside the thing under test, and both tables here can carry a light (a
    // street lamp is a `decorations` row, a dropped torch an `items` one) whose
    // pool reaches past whatever stands beside it. The same margin
    // `light::lit_tiles` grows the occlusion grid by — [`light::light_margin_tiles`]
    // — widens this window too, so a lamp just outside `_RADIUS` is a lamp this
    // frame's own lighting can still find, the way the client's does. Half of
    // what a player is looking at in a town is not on the map at all: a pack's
    // decorations and anything anyone has dropped live in `openshard.db`, and a
    // tool that reads only `statics.mul` answers "it does not reproduce" about
    // every one of them.
    //
    // On by default and loud when it cannot read, because the quiet failure is
    // exactly the one being removed — see [`shard::database_in`], every panic of
    // which names `OPENSHARD_SCENE_SHARD=0`.
    //
    // `_TILES` applies here as it does to the map's statics: it is a filter on
    // what the scene keeps, not on which file a graphic came out of.
    let from_the_shard = if env_flag("OPENSHARD_SCENE_SHARD", true) {
        let database = match env_opt("OPENSHARD_SCENE_SHARD_DB") {
            Some(path) => PathBuf::from(path),
            None => shard::database_in(&PathBuf::from(
                env_opt("OPENSHARD_SCENE_CONFIG").unwrap_or_else(|| "openshard.toml".to_owned()),
            )),
        };
        let light_radius =
            radius.saturating_add(u16::try_from(light::light_margin_tiles(&tuning)).unwrap_or(u16::MAX));
        let placed = shard::read(
            &database,
            shard::Window {
                facet: FACET,
                min_x: at.x.saturating_sub(light_radius),
                max_x: at.x.saturating_add(light_radius),
                min_y: at.y.saturating_sub(light_radius),
                max_y: at.y.saturating_add(light_radius),
            },
        );
        let (mut ground, mut decorations) = (0u32, 0u32);
        for one in &placed {
            if tile_filter
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&one.graphic))
            {
                continue;
            }
            match one.source {
                shard::Source::Ground => ground += 1,
                shard::Source::Decoration => decorations += 1,
            }
            let (sx, sy) = shift(anchor, (one.x, one.y));
            items.push(GroundItem {
                amount: ItemAmount::ONE,
                at: Point::new(sx, sy, one.z),
                graphic: Graphic(one.graphic),
                hue: Hue(one.hue),
            });
        }
        format!(
            "{ground} ground items and {decorations} decorations from {}",
            database.display()
        )
    } else {
        "off (OPENSHARD_SCENE_SHARD=0): nothing the server placed is in this frame".to_owned()
    };
    let from_the_house = if let Some(serial) = env_opt("OPENSHARD_SCENE_HOUSE") {
        let serial: u32 = serial
            .parse()
            .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_HOUSE: {serial:?}"));
        let database = match env_opt("OPENSHARD_SCENE_SHARD_DB") {
            Some(path) => PathBuf::from(path),
            None => shard::database_in(&PathBuf::from(
                env_opt("OPENSHARD_SCENE_CONFIG").unwrap_or_else(|| "openshard.toml".to_owned()),
            )),
        };
        let house = shard::house(&database, serial)
            .unwrap_or_else(|| panic!("house {serial} is not in {}", database.display()));
        let multis = Multis::load(&dir).expect("the client's multi files");
        let components = multis.components(house.multi);
        assert!(
            !components.is_empty(),
            "house {serial} names multi {:#06x}, which this client does not know",
            house.multi
        );
        let before = items.len();
        for component in components.iter().copied().filter(|component| component.drawn()) {
            if tile_filter
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&component.graphic.0))
            {
                continue;
            }
            let Some(at) = component.placed_at(house.at) else {
                continue;
            };
            let (sx, sy) = shift(anchor, (at.x, at.y));
            items.push(GroundItem {
                amount: ItemAmount::ONE,
                at: Point::new(sx, sy, at.z),
                graphic: component.graphic,
                hue: Hue::NONE,
            });
        }
        format!(
            "{} components of house {serial} (multi {:#06x}) from {}",
            items.len() - before,
            house.multi,
            database.display(),
        )
    } else {
        "off (OPENSHARD_SCENE_HOUSE unset)".to_owned()
    };
    let before_the_extras = items.len();

    // Hand-named extras — anything not on the map and not in the shard's own
    // database either: a graphic somebody wants dropped into the scene to see
    // what it does. It stopped being the way a live-shard decoration gets in
    // (that is the reader above), and is still the way a *hypothetical* one
    // does — a torch put where no torch is, to find out what it would light.
    if let Some(spec) = env_opt("OPENSHARD_SCENE_EXTRA") {
        for entry in spec.split(';').filter(|s| !s.trim().is_empty()) {
            let mut item = parse_extra_item(entry);
            let (sx, sy) = shift(anchor, (item.at.x, item.at.y));
            item.at = Point::new(sx, sy, item.at.z);
            items.push(item);
        }
    }
    let hand_named = items.len() - before_the_extras;
    let carried = env_opt("OPENSHARD_SCENE_CARRIED").map(|spec| {
        let (at, facing) = parse_carried(&spec);
        let (sx, sy) = shift(anchor, (at.x, at.y));
        (Point::new(sx, sy, at.z), Vec2::new(0.0, 0.0), facing)
    });
    assert!(
        !items.is_empty(),
        "the scene is empty: turn on OPENSHARD_SCENE_STATICS, widen OPENSHARD_SCENE_RADIUS, \
         loosen OPENSHARD_SCENE_TILES, or add OPENSHARD_SCENE_EXTRA"
    );

    let viewport = viewport();
    let (look_sx, look_sy) = shift(anchor, (look.x, look.y));
    let mut camera = Camera::new(Point::new(look_sx, look_sy, look.z), viewport.0, viewport.1);
    let zoom_notches: u32 = env_opt("OPENSHARD_SCENE_ZOOM")
        .map(|s| {
            s.parse()
                .unwrap_or_else(|_| panic!("OPENSHARD_SCENE_ZOOM: {s:?}"))
        })
        .unwrap_or(0);
    let mut zoom = Zoom::ONE;
    for _ in 0..zoom_notches {
        zoom = zoom.scale_up();
    }
    // About the viewport's own centre, not a corner: `zoom_about` holds
    // whatever world point sits under the given screen point fixed, and the
    // scene is built around `look`, which `Camera::new` already centred.
    // Zooming about `(0, 0)` would hold the *corner* fixed instead and walk
    // the framed static out from under the crop this tool's callers take.
    camera.zoom_about(
        RealPixel::new((viewport.0 / 2) as i32, (viewport.1 / 2) as i32),
        zoom,
    );
    let (width, height) = camera.image_size();

    let land_atlas = if want_ground {
        LandAtlas::build(
            &art,
            ground::visible_graphics(&synthetic, &camera).iter().copied(),
        )
        .expect("land fits")
    } else {
        LandAtlas::build(&art, std::iter::empty()).expect("an empty atlas always fits")
    };
    let texmaps = {
        let texmaps_src = TexMaps::open(&dir).expect("texidx.mul and texmaps.mul");
        let wanted = match want_ground {
            true => ground::visible_graphics(&synthetic, &camera),
            false => Default::default(),
        };
        TexmapAtlas::build(&texmaps_src, &tiledata, wanted).expect("textures fit")
    };
    let animations = StaticAnimations::default();
    let needed = items::needed_graphics(&items, &animations);
    let mut static_atlas = StaticAtlas::build(&art, needed).expect("the scene's own items fit");
    // The counterfactual `docs/footprints.md`'s post item asks for: the same
    // place, drawn with the boxes that shipped before a base edge was ever
    // measured. One knob rather than two builds of the tool a session apart.
    if env_flag("OPENSHARD_SCENE_NO_FOOTPRINTS", false) {
        static_atlas.forget_footprints();
    }

    // Which plane to draw. Read here rather than beside the blit below, because
    // it is one of `frame::Inputs`'s own fields now: what this tool draws is a
    // frame like any other, and which of its values is coloured in is an input to
    // the assembly rather than something done to the answer afterwards.
    let wanted_view = env_opt("OPENSHARD_FRAME_VIEW")
        .and_then(|v| v.parse::<usize>().ok())
        .and_then(|v| openshard_client_render::debug::View::ALL.get(v).copied())
        .unwrap_or_default();

    // **One assembly, the client's own** — `docs/parity.md`, decision D1. Every
    // way this tool used to differ from `App::draw` is a field below, so a
    // difference between the two pictures is a difference in these values and
    // not in which calls somebody wrote out.
    //
    // The land atlas is empty under `_GROUND=0`, so there is no branch here for
    // it: a ground pass over an atlas holding nothing paints nothing, and it
    // still clears the targets exactly as the live one does.
    let mut fades = openshard_client_render::cutaway::Fades::default();
    let inputs = openshard_client_render::frame::Inputs {
        map: &synthetic,
        items: &items,
        drawn_items: &items,
        // No storey selection here: this scene is one hand-built rectangle, not
        // a building, and `parity.md`'s rule is that a diagnostic differs from
        // the client only in values it states out loud. `None` is what every
        // other frame caller outside the app passes.
        interior: None,
        camera: &camera,
        tiledata: &tiledata,
        animations: &animations,
        // The same cutaway the pictures get, which is the app's own rule: a wall
        // the frame did not draw must not darken the street.
        cutaway: &cutaway,
        land: &land_atlas,
        texmaps: &texmaps,
        statics: openshard_client_render::atlas::StaticArt::Single(&static_atlas),
        // Flat by default, as the client is: what a roof does to the light under
        // it is a second thing changing every tile of the picture. The client's
        // own F6 is `OPENSHARD_SCENE_SKY_FIELD=1` here, and it is a knob because
        // the field is drawn **per tile with no interpolation** — a candidate
        // whenever what a person is looking at is a tile-shaped step.
        sky: Some(match env_flag("OPENSHARD_SCENE_SKY_FIELD", false) {
            true => light::NIGHT,
            false => light::NIGHT.flattened(),
        }),
        // A caller may place a stationary player-lantern in the frame. The
        // real client supplies a walk interpolation here; this diagnostic has
        // no motion, so its deliberately explicit offset is zero.
        sun: None,
        carried,
        tuning: &tuning,
        // The flicker, held at the instant every scene here is read at, so a
        // dump taken twice is the same dump.
        flame_time: 0.0,
        // No bake. The scene is lit once, and an uncached grid is the same grid.
        bake: None,
        // Nothing is under a cursor here.
        highlight: None,
        // The client's own F10, at the one place it reaches the *normal* plane.
        //
        // With the lights off, `App::draw` builds no grid at all (`sky` is
        // `None` ⇒ `Lighting::NONE`), so every fragment takes `statics.wesl`'s
        // billboard fallback — which always has a facing. Turning the lights on
        // builds the grid, and from then on a fragment's normal is the
        // *impostor's* answer. So a face that changes when a person presses F10
        // is not a lighting difference at all: it is the two answers
        // disagreeing, and this is the switch that puts them side by side in one
        // tool.
        //
        // The lighting keeps its own grid either way — only what the sprites are
        // met against changes, which is what keeps the difference readable in
        // `View::Normal` instead of moving every other plane with it.
        impostor: match env_flag("OPENSHARD_SCENE_IMPOSTOR", true) {
            true => openshard_client_render::frame::Impostor::Met,
            false => openshard_client_render::frame::Impostor::Billboards,
        },
        // **This tool filters earlier and by name**, so it asks for everything
        // here: `OPENSHARD_SCENE_STATICS`/`_SHARD`/`_GROUND` decide what is in
        // the scene at all, which for a tool that *builds* a place is the
        // stronger knob — it takes a thing out of the grid and out of the light
        // too. `frame::Draw` is the client's version of the same question, where
        // the world is given and only the drawing can be narrowed.
        draw: openshard_client_render::frame::Draw::EVERYTHING,
        view: wanted_view,
        // Not a fact this tool has an opinion on: it builds a place, not a
        // character standing in it.
        dead: false,
        player_rect: None,
        player_mask: None,
        fades: &mut fades,
    };
    // **What this frame was asked for**, printed and written beside the picture.
    // The client's F12 dump writes the same block from the same function, so two
    // frames that disagree are diffed here first: an input that differs is set
    // the same or the case is not compared at all — `docs/parity.md` D6.
    //
    // Three lines the client's own dump does not have, and cannot: its `items`
    // list *is* the server's, arriving on the wire as it is placed, while this
    // one is assembled out of three sources by hand. `Inputs::summary` says how
    // many items a frame drew and has no way to say where they came from — so a
    // frame missing everything the shard placed and a frame with nothing to
    // place read identically there, which is the false negative this session
    // closed. These say which.
    let asked_for = format!(
        "{}scene.map = {from_the_map} statics pulled from the map\n\
         scene.shard = {from_the_shard}\n\
         scene.house = {from_the_house}\n\
         scene.extra = {hand_named} hand-named\n",
        inputs.summary(),
    );
    eprint!("{asked_for}");
    let openshard_client_render::frame::Frame {
        lighting,
        ground: ground_quads,
        statics:
            openshard_client_render::statics::StaticGeometry {
                quads: item_quads,
                cutaway_quads: _,
                cutaway_boxes: _,
                mesh_vertices,
                mesh_rows,
                // The impostor's boxes — `docs/lighting_rebuild.md` phase 6c —
                // which this scene's own pixels are met against.
                boxes,
            },
    } = openshard_client_render::frame::assemble(inputs);

    let format = openshard_client_render::blit::WORLD_FORMAT;
    let world = openshard_client_render::blit::world_texture(&device, width, height);
    let world_view = world.create_view(&wgpu::TextureViewDescriptor::default());
    let gbuffer = openshard_client_render::gbuffer::Gbuffer::new(&device, width, height);
    let gbuffer_views = gbuffer.views();
    let depth = renderer::depth_texture(&device, width, height);
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

    let mut ground_pass = GroundRenderer::new(&device, &queue, format, &land_atlas, &texmaps);
    let mut items_pass = SpriteRenderer::new(
        &device,
        &queue,
        format,
        static_atlas.pixels(),
        &openshard_client_render::hue::HueRamp::build(
            &openshard_uofiles::hues::Hues::load(dir.join("hues.mul")).expect("hues.mul"),
        ),
    );
    // The fringe, as the client's own F2 sets it — `impostor::Fringe`. Here so
    // that "I pressed the key and saw nothing" is a question this tool can be
    // asked: two dumps either side of the switch, diffed, say how many pixels it
    // actually moves in a *drawn frame* rather than in a census of art.
    let fringe = openshard_client_render::impostor::Fringe::from_env();
    items_pass.set_fringe(fringe);
    eprintln!("scene.fringe = {}", fringe.name());
    let mut mesh_pass = MeshFaceRenderer::new(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = Target {
        gbuffer: &gbuffer_views,
        view: &world_view,
        depth: &depth_view,
        width,
        height,
        projection: camera.projection(),
    };
    // Always run, even with no ground quads: the ground pass is what clears
    // the world/place/depth targets — see its own doc — so skipping it here
    // would leave the items pass drawing over whatever the textures happened
    // to hold.
    ground_pass.render(&device, &queue, &mut encoder, target, &ground_quads);
    // Use the live client's dynamic-item route, rather than merely sharing the
    // static renderer: the deferred pass resolves an item's tile from its own
    // instance buffer when this bit is present.  A house is expanded into this
    // list, so a diagnostic that leaves the bit clear can prove only the map
    // static path and miss the exact join under investigation.
    items_pass.render_with_id_bits(
        &device,
        &queue,
        &mut encoder,
        target,
        &item_quads,
        &boxes,
        None,
        openshard_client_render::gbuffer::IDS_DYNAMIC_ITEM,
    );
    // Right after, into the same pixels its own billboard just drew — the same
    // order `lib.rs`'s real frame loop uses and for the same reason
    // (`docs/gbuffer.md` step 4c): without this, a climbable, prism-fit item
    // pulled from the real map never gets its honest per-face normal, and
    // this tool would go on showing the flat corner-stance reading a live
    // frame does not.
    mesh_pass.render(&device, &queue, &mut encoder, target, &mesh_vertices, &mesh_rows);
    queue.submit([encoder.finish()]);

    let surface = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d {
            width: viewport.0,
            height: viewport.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let surface_view = surface.create_view(&wgpu::TextureViewDescriptor::default());

    eprintln!(
        "{} items, {} flames, {} standing cells",
        items.len(),
        lighting.lights.len(),
        lighting.occlusion.boxes().count(),
    );
    // **How many drawn pictures the impostor has no shape for** —
    // `docs/lighting_rebuild.md` phase 6c, and the census the `View::Normal`
    // grey is read against. A picture with no box is a billboard: it stands in
    // the middle of its tile with its height running down the sprite and no
    // facing at all, so it is lit from every side. That is the honest answer
    // for a thing the grid holds nothing for, and it is the *wrong* answer for
    // a surface the grid simply refused — which is what this counts, by the
    // stance the art was read as, so the two can be told apart.
    {
        let mut without = std::collections::BTreeMap::new();
        for quad in &item_quads {
            if quad.volumes.count == 0 {
                *without.entry(format!("{:?}", quad.place.stance)).or_insert(0u32) += 1;
            }
        }
        eprintln!(
            "  {} of {} pictures stand as no box at all: {without:?}",
            without.values().sum::<u32>(),
            item_quads.len(),
        );
    }
    // Every solid `_AT`'s own tile holds, in real coordinates — the geometry a
    // profile below is walking across, printed once so the segment it is given
    // does not have to be guessed from the picture.
    let (syn_x, syn_y) = shift(anchor, (at.x, at.y));
    for solid in lighting.occlusion.solids_at(i32::from(syn_x), i32::from(syn_y)) {
        eprintln!(
            "  solid: x {:.3}..{:.3}, y {:.3}..{:.3}, z {:.1}..{:.1}, edges {:#06b}, opacity {}",
            solid.space.min.x,
            solid.space.max.x,
            solid.space.min.y,
            solid.space.max.y,
            solid.space.min.z,
            solid.space.max.z,
            solid.edges.raw(),
            solid.opacity,
        );
    }

    // A profile instead of a picture: `light::sample` walked along a segment
    // rather than `Blit` rasterising a frame, for the question a picture cannot
    // answer — whether a hard edge on screen is `Reach::through` (occlusion) or
    // `Reach::cone` (which one of a face's `faces()` and its beam folds into,
    // see `light.rs`) that is doing the cutting. No GPU work past this point.
    if env_opt("OPENSHARD_SCENE_PROFILE_FACE").is_some() {
        run_profile(anchor, &lighting);
        return;
    }

    // The grid alone: every solid `lighting.occlusion` holds, on black, with no
    // sprite and no ground under it to explain a shape away. The same list and
    // the same pass the live client's F5 overlay draws (`solid::standing` into
    // `solids::SolidsRenderer`) — only the colour changes, and nothing else is
    // composited under it, since a ray does not care which kind of thing it hit.
    //
    // - `OPENSHARD_SCENE_SOLIDS=white` — one flat colour, so the geometry reads
    //   on its own with nothing about the flame in it.
    // - `OPENSHARD_SCENE_SOLIDS=lit` — `light::sample` run once per visible
    //   face (`Solid::faces`'s `[Top, East, South]`, the lid at `Surface::Flat`
    //   and the two sides at their own `Surface::Face`), the real multiplier
    //   painted straight on rather than the kind-coloured, [`Side::shade`]d
    //   picture `render` draws — see `solids::SolidsRenderer::render_lit`.
    if let Some(mode) = env_opt("OPENSHARD_SCENE_SOLIDS") {
        let standing = openshard_client_render::solid::standing(
            &lighting.occlusion,
            openshard_client_render::solid::Cut::Nothing,
        );
        let count = standing.len();

        let mut solids_pass = openshard_client_render::solids::SolidsRenderer::new(&device, format);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        // `SolidsRenderer` only ever loads (see its own doc) — this clear is
        // the black the boxes sit on, and the only reason a pass runs first.
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("solids-only clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &surface_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let rect = ViewportRect {
            x: 0,
            y: 0,
            width: viewport.0,
            height: viewport.1,
        };
        // On by default, like the live F5 overlay — off to see the fills alone,
        // unmixed with a stroke's own colour. See `SolidsRenderer::render`'s own
        // doc: with the strokes gone, a face that something nearer should have
        // hidden and did not reads as a wrongly bright translucent patch rather
        // than a line quietly sitting on top of it.
        // On by default, like the live F5 overlay — off to see the fills alone,
        // unmixed with a stroke's own colour. See `SolidsRenderer::render`'s own
        // doc: with the strokes gone, a face that something nearer should have
        // hidden and did not reads as a wrongly bright translucent patch rather
        // than a line quietly sitting on top of it.
        //
        // `opaque` is off by default too, matching the live overlay's own
        // translucency, and on for the question that overlay was never asked —
        // whether a face is genuinely hidden or only tinted through.
        let style = openshard_client_render::solids::Style {
            edges: env_flag("OPENSHARD_SCENE_SOLIDS_EDGES", true),
            opaque: env_flag("OPENSHARD_SCENE_SOLIDS_OPAQUE", false),
        };
        match mode.as_str() {
            "white" => {
                let coloured: Vec<_> = standing
                    .into_iter()
                    .map(|(solid, _)| (solid, [255.0; 3]))
                    .collect();
                solids_pass.render(
                    &device,
                    &queue,
                    &mut encoder,
                    openshard_client_render::solids::Frame {
                        target: &surface_view,
                        size: viewport,
                        rect,
                    },
                    &camera,
                    &coloured,
                    style,
                );
            }
            "lit" => {
                let coloured: Vec<_> = standing
                    .into_iter()
                    .map(|(solid, _)| (solid, face_light(&solid, &lighting)))
                    .collect();
                solids_pass.render_lit(
                    &device,
                    &queue,
                    &mut encoder,
                    openshard_client_render::solids::Frame {
                        target: &surface_view,
                        size: viewport,
                        rect,
                    },
                    &camera,
                    &coloured,
                    style,
                );
            }
            other => panic!("OPENSHARD_SCENE_SOLIDS wants `white` or `lit`, got {other:?}"),
        }
        queue.submit([encoder.finish()]);

        let png = dump_frame(&device, &queue, &surface, viewport);
        let path = PathBuf::from(env("OPENSHARD_FRAME_DUMP"));
        write_dump(&path, &png, &asked_for);
        eprintln!(
            "wrote {} ({count} solids, {mode}, edges {}, opaque {})",
            path.display(),
            style.edges,
            style.opaque,
        );
        return;
    }

    let mut blit = Blit::new(&device, format);
    // No map-static or mobile pass in this scene: the dummies stand in for
    // their distinct storage buffers.  `item_instances` is deliberately the
    // real item buffer above, matching `App::encode_world_passes`.
    let dummy_instances = openshard_client_render::blit::dummy_instances(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    blit.render(
        &device,
        &queue,
        &mut encoder,
        openshard_client_render::blit::Frame {
            target: &surface_view,
            world: &world_view,
            gbuffer: &gbuffer_views,
            face_instances: &dummy_instances,
            item_instances: items_pass.instances_buffer(),
            mobile_instances: &dummy_instances,
            mesh_instances: mesh_pass.rows_buffer(),
            ground_instances: ground_pass.instances_buffer(),
            zoom: Zoom::ONE,
            rect: ViewportRect {
                x: 0,
                y: 0,
                width: viewport.0,
                height: viewport.1,
            },
        },
        &lighting,
    );
    queue.submit([encoder.finish()]);

    let png = dump_frame(&device, &queue, &surface, viewport);
    let path = PathBuf::from(env("OPENSHARD_FRAME_DUMP"));
    write_dump(&path, &png, &asked_for);
    eprintln!("wrote {} ({:?})", path.display(), wanted_view);
}
