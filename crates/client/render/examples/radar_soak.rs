//! The soak `docs/world/design_radar.md` §10.1 asks for, with nobody in front of it.
//!
//! ```text
//! OPENSHARD_CLIENT=/path/to/client \
//!   cargo run --release -p openshard-client-render --example radar_soak [window_scale] [gump_scale]
//! ```
//!
//! R7 built the instrument and read nothing with it: "walking costs no raster
//! work" and "the GPU page cache is reached only by a pathological view" both
//! stayed arguments. What kept them arguments is that the development HUD shows
//! its numbers on **one** machine at **one** scale, and the worst case those two
//! claims are about — a HiDPI screen at a desk scale, fully zoomed out — is a
//! scale most machines are not.
//!
//! Nothing in that worst case needs a window. The level a view picks, the
//! rectangle it fetches, the chunks it draws, what the producer spends walking
//! the map and how close each bound gets are all functions of the map, the
//! colour table and four numbers a person could have set in a settings panel.
//! So this drives [`radar::advance`] — the same call `App::draw_from` makes,
//! deliberately the same one and not a second spelling of its order — frame
//! after frame, and prints what the HUD would have shown.
//!
//! Three readings:
//!
//! 1. **the floor fills in** — both windows open at the scenario's scale, from
//!    an empty cache until the sweep owes nothing and the queue is idle: what
//!    each frame spent, and how the fallback tally moves from *missing* through
//!    *coarser* to *exact*;
//! 2. **walking** — a minimap alone, a player stepping one tile a frame, which
//!    is the reading behind "walking costs no raster work": a step must cost
//!    nothing until the region crosses a chunk edge;
//! 3. **the bounds** — where the level lands at every zoom either window has,
//!    at four scales, and what that asks of the 1024-page GPU cache. This one
//!    builds nothing; it is arithmetic over [`RadarView`], which is the point:
//!    a bound crossed here is crossed before a single chunk is walked for.
//!
//! **What it cannot answer.** GPU residency and eviction are a real device's,
//! and so is the `over_capacity_draws` counter itself. What is measured here is
//! that counter's *predicate* — how many chunks a view hands `render_region`
//! against how many pages exist — which is the half that does not need one.
//!
//! The scenario's numbers are the panes' own, restated because
//! `openshard-client-render` cannot depend on `openshard-client-app` (and must
//! not: the dependency runs the other way). Each is named where it came from,
//! and a reading is only ever as good as the scenario printed above it.

use std::f32::consts::FRAC_PI_4;
use std::path::PathBuf;
use std::time::Duration;

use openshard_client_render::radar::{
    self,
    BASE_CHUNK_TILES,
    RadarBuildScratch,
    RadarCache,
    RadarExtent,
    RadarLod,
    RadarLodSelector,
    RadarStep,
    RadarTile,
    RadarView,
    RadarWorkQueue,
    SWEEP_LOD,
};
use openshard_client_render::radar_pass::{
    Placement,
    RADAR_CHUNK_CACHE_LAYERS,
};
use openshard_protocol::world::Facet;
use openshard_uofiles::radarcol::RadarColors;

/// The facet every reading here is taken on: the one a shipped install has.
const FACET: Facet = Facet(0);

/// `panes::world_map::EXTENT` less the plate `FALLBACK_INSET` reserves.
///
/// The fallback and not the measured inset, deliberately: an install's own
/// `0x0A28` moves this by a few pixels, and a scenario that changes with the
/// art is a reading nobody else can take again.
const FACET_MAP_CONTENT: (i32, i32) = (640 - 24 - 24, 480 - 38 - 24);
/// `panes::world_map::MIN_ZOOM_STEPS` — the whole facet fitted, which is as far
/// out as that window goes and the worst case for everything below.
const FACET_MAP_MIN_ZOOM: i8 = 0;
/// `panes::world_map::MAX_ZOOM_STEPS`.
const FACET_MAP_MAX_ZOOM: i8 = 20;

/// `panes::minimap::LARGE_EXTENT` less the 15% rim `Window::content` insets.
const MINIMAP_CONTENT: (i32, i32) = (140, 140);
/// The `clamp(-6, 12)` in `MinimapPane::handle`.
const MINIMAP_MIN_ZOOM: i8 = -6;
const MINIMAP_MAX_ZOOM: i8 = 12;
/// `panes::minimap::TANGENT_MARGIN_FRACTION`.
const TANGENT_MARGIN_FRACTION: f32 = 0.21;

/// How far the walking reading walks, in tiles: four chunk edges' worth, so the
/// pattern it is looking for repeats rather than happening once.
const WALK_TILES: u32 = 256;

/// A frame line every this many frames, so a fill of a hundred-odd frames reads
/// as a page rather than a scroll.
const PRINT_EVERY: usize = 16;

/// The two magnifications a radar view is placed and sampled under.
///
/// They are separate because they enter differently, and a soak that folded
/// them into one number would be measuring a client nobody runs.
/// `window` is the desk's own window scale (`crate::desk::WindowScale`), which
/// multiplies the window's gump-pixel extent; `gump` is `pixels_per_point`,
/// which is what turns those logical points into device pixels.
#[derive(Clone, Copy, Debug)]
struct Scale {
    window: f32,
    gump:   f32,
}

impl Scale {
    /// How many physical pixels a gump pixel covers — the product, which is the
    /// only combination of the two that any answer below depends on.
    fn product(self) -> f32 {
        self.window * self.gump
    }
}

/// `panes::world_map::tpp`: the zoom-0 view fits the whole facet in the content
/// rectangle, and each step is a 1.25× closer look.
fn facet_map_tiles_per_gump_pixel(extent: RadarExtent, zoom_steps: i8) -> f32 {
    let fit = (FACET_MAP_CONTENT.0 as f32 / f32::from(extent.width()))
        .min(FACET_MAP_CONTENT.1 as f32 / f32::from(extent.height()));
    1.0 / (fit * 1.25_f32.powi(i32::from(zoom_steps)))
}

/// The view `App::draw_from` builds for a `Drawn::WorldMap`.
fn facet_map_view(extent: RadarExtent, centre: RadarTile, zoom_steps: i8, scale: Scale) -> RadarView {
    RadarView::new(
        FACET,
        centre,
        extent,
        facet_map_tiles_per_gump_pixel(extent, zoom_steps) / scale.product(),
        Placement {
            origin:   (0.0, 0.0),
            extent:   (
                FACET_MAP_CONTENT.0 as f32 * scale.window,
                FACET_MAP_CONTENT.1 as f32 * scale.window,
            ),
            circle:   false,
            rotation: 0.0,
        },
        scale.gump,
    )
}

/// The view `App::draw_from` builds for a `Drawn::Minimap`, tangent margin and
/// all — the margin is a fraction of the window's *own* extent and is divided by
/// `zoom` alone, which is the one part of this file that would be wrong if it
/// were scaled here too.
fn minimap_view(extent: RadarExtent, player: RadarTile, zoom_steps: i8, scale: Scale) -> RadarView {
    let zoom = 1.25_f32.powi(i32::from(zoom_steps));
    RadarView::new(
        FACET,
        player,
        extent,
        1.0 / zoom,
        Placement {
            origin:   (700.0, 0.0),
            extent:   (
                MINIMAP_CONTENT.0 as f32 * scale.window,
                MINIMAP_CONTENT.1 as f32 * scale.window,
            ),
            circle:   true,
            rotation: FRAC_PI_4,
        },
        scale.gump,
    )
    .with_tangent_margin_fraction(MINIMAP_CONTENT, zoom, TANGENT_MARGIN_FRACTION)
}

/// How many chunks a view hands `render_region` — the count the page cache
/// truncates, and the only half of `over_capacity_draws` a run without a device
/// can measure.
fn drawn_chunks(view: RadarView, lod: RadarLod) -> usize {
    radar::region_chunks(view.region(), lod).count()
}

fn percent(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    part as f32 * 100.0 / whole as f32
}

fn print_frame_header() {
    println!(
        "  frame   owed  queued+flight   built      raster    exact  coarse   stale  missing     CPU MiB"
    );
}

fn print_frame(
    frame: usize,
    owed: usize,
    queue: &RadarWorkQueue,
    report: &radar::RadarStepReport,
    cache: &RadarCache,
) {
    let work = queue.counters();
    let counters = cache.counters();
    let demand = report.demand;
    println!(
        "  {frame:>5}  {owed:>5}  {:>6}+{:<6}  {:>6}  {:>10.2?}  {:>7}  {:>6}  {:>6}  {:>7}  {:>6.1}/{:.0}",
        work.queued,
        work.in_flight,
        report.built,
        report.raster,
        demand.exact,
        demand.coarser,
        demand.stale,
        demand.missing,
        counters.retained_bytes as f32 / (1024.0 * 1024.0),
        counters.tail_budget as f32 / (1024.0 * 1024.0),
    );
}

/// Reading 1: both windows open, an empty cache, and the frames it takes to
/// stop moving.
fn the_floor_fills_in(
    map: &openshard_map::map::WorldMap,
    colors: &RadarColors,
    extent: RadarExtent,
    scale: Scale,
) {
    let centre = RadarTile::new(u32::from(extent.width()) / 2, u32::from(extent.height()) / 2);
    let mut cache = RadarCache::default();
    let mut queue = RadarWorkQueue::default();
    let mut scratch = RadarBuildScratch::default();
    let mut facet_map_lod = RadarLodSelector::default();
    let mut minimap_lod = RadarLodSelector::default();
    let (producer_centre, _) = radar::world_tile_to_base_chunk(centre);

    let facet_map = facet_map_view(extent, centre, FACET_MAP_MIN_ZOOM, scale);
    let minimap = minimap_view(extent, centre, MINIMAP_MIN_ZOOM, scale);
    println!();
    println!("1. the floor fills in — both windows open, at their widest zoom, from nothing");
    println!(
        "   facet map: {:.3} tiles/px, lod {}, region {}x{}, {} chunks drawn",
        facet_map.tiles_per_pixel,
        facet_map.lod().value(),
        facet_map.region().extent().width(),
        facet_map.region().extent().height(),
        drawn_chunks(facet_map, facet_map.lod()),
    );
    println!(
        "   minimap:   {:.3} tiles/px, lod {}, region {}x{}, {} chunks drawn",
        minimap.tiles_per_pixel,
        minimap.lod().value(),
        minimap.region().extent().width(),
        minimap.region().extent().height(),
        drawn_chunks(minimap, minimap.lod()),
    );
    print_frame_header();

    let mut raster_total = Duration::ZERO;
    let mut raster_worst = Duration::ZERO;
    let mut built_total = 0_usize;
    let mut frames = 0_usize;
    let mut first_complete = None;
    loop {
        let views = [
            (facet_map, facet_map_lod.update(facet_map)),
            (minimap, minimap_lod.update(minimap)),
        ];
        let report = radar::advance(
            RadarStep {
                views: &views,
                sweep: Some(FACET),
                facet_extent: extent,
                producer_centre,
            },
            map,
            colors,
            &mut cache,
            &mut queue,
            &mut scratch,
        );
        raster_total += report.raster;
        raster_worst = raster_worst.max(report.raster);
        built_total += report.built;
        let owed = cache.sweep_owed_len(FACET);
        if report.demand.missing == 0 && first_complete.is_none() {
            first_complete = Some(frames);
        }
        let idle = report.built == 0 && queue.counters().queued == 0 && owed == 0;
        if frames.is_multiple_of(PRINT_EVERY) || idle {
            print_frame(frames, owed, &queue, &report, &cache);
        }
        frames += 1;
        if idle {
            break;
        }
    }
    let counters = cache.counters();
    println!(
        "   {frames} frames, {built_total} chunks walked out of the map in {raster_total:.2?} (worst frame {raster_worst:.2?})"
    );
    match first_complete {
        Some(frame) => println!("   no view was missing terrain from frame {frame} on"),
        None => println!("   SOME TERRAIN WAS MISSING IN EVERY FRAME"),
    }
    println!(
        "   cache: {} ready, {} stale, {} evicted, {:.1} MiB of a {:.0} MiB tail ({:.0}%)",
        counters.ready,
        counters.stale,
        counters.evicted,
        counters.retained_bytes as f32 / (1024.0 * 1024.0),
        counters.tail_budget as f32 / (1024.0 * 1024.0),
        percent(counters.retained_bytes, counters.tail_budget),
    );
}

/// Reading 2: the claim that a step costs nothing until the region crosses a
/// chunk edge.
fn walking_costs_no_raster_work(
    map: &openshard_map::map::WorldMap,
    colors: &RadarColors,
    extent: RadarExtent,
    scale: Scale,
) {
    let mut player = RadarTile::new(u32::from(extent.width()) / 2, u32::from(extent.height()) / 2);
    let mut cache = RadarCache::default();
    let mut queue = RadarWorkQueue::default();
    let mut scratch = RadarBuildScratch::default();
    let mut minimap_lod = RadarLodSelector::default();

    // No facet map: its region is the whole facet, and a producer still filling
    // that in would be the thing measured instead of the step.
    let mut step_once = |player: RadarTile, cache: &mut RadarCache, queue: &mut RadarWorkQueue| {
        let view = minimap_view(extent, player, 0, scale);
        let views = [(view, minimap_lod.update(view))];
        let (producer_centre, _) = radar::world_tile_to_base_chunk(player);
        radar::advance(
            RadarStep {
                views: &views,
                sweep: None,
                facet_extent: extent,
                producer_centre,
            },
            map,
            colors,
            cache,
            queue,
            &mut scratch,
        )
    };

    println!();
    println!("2. walking — a minimap alone at zoom 0, one tile a frame");
    let mut settle = 0_usize;
    loop {
        let report = step_once(player, &mut cache, &mut queue);
        settle += 1;
        if report.built == 0 && queue.counters().queued == 0 {
            break;
        }
    }
    let view = minimap_view(extent, player, 0, scale);
    println!(
        "   standing still: {} frames to fill a {}x{} region at lod {}, {} chunks drawn",
        settle,
        view.region().extent().width(),
        view.region().extent().height(),
        view.lod().value(),
        drawn_chunks(view, view.lod()),
    );

    let mut raster_total = Duration::ZERO;
    let mut raster_worst = Duration::ZERO;
    // Kept apart from the total rather than subtracted out of it afterwards:
    // "a step costs nothing" is a claim about the frames that built nothing,
    // and an average over all of them with the building frames merely trimmed
    // is that claim answered with the wrong frames in it.
    let mut idle_raster = Duration::ZERO;
    let mut idle_frames = 0_u32;
    let mut built_total = 0_usize;
    let mut frames_that_built = 0_usize;
    let mut missing_frames = 0_usize;
    for _ in 0..WALK_TILES {
        player = RadarTile::new(player.x() + 1, player.y());
        let report = step_once(player, &mut cache, &mut queue);
        raster_total += report.raster;
        raster_worst = raster_worst.max(report.raster);
        built_total += report.built;
        if report.built == 0 {
            idle_raster += report.raster;
            idle_frames += 1;
        } else {
            frames_that_built += 1;
        }
        if report.demand.missing != 0 {
            missing_frames += 1;
        }
    }
    let edges = WALK_TILES / u32::from(BASE_CHUNK_TILES);
    println!(
        "   {WALK_TILES} steps: {frames_that_built} frames did raster work, {built_total} chunks, {raster_total:.2?} in total (worst frame {raster_worst:.2?})"
    );
    println!(
        "   the walk crossed {edges} chunk edges; the {idle_frames} steps that crossed none cost {:.2?} each",
        idle_raster / idle_frames.max(1),
    );
    if missing_frames == 0 {
        println!("   no frame of the walk was missing terrain");
    } else {
        println!("   {missing_frames} FRAMES OF THE WALK DREW BACKDROP — a step outran the producer");
    }
}

/// Reading 3: what each zoom asks of the page cache, before anything is built.
fn the_bounds(extent: RadarExtent) {
    let centre = RadarTile::new(u32::from(extent.width()) / 2, u32::from(extent.height()) / 2);
    let pages = u64::from(RADAR_CHUNK_CACHE_LAYERS);
    println!();
    println!("3. the bounds — the level each zoom picks, and what it hands the {pages}-page cache");
    println!("   a row per level change, plus each window's own extremes");
    // The *product* is the axis, because it is the only combination either
    // window's answer depends on: the facet map's region is scale-invariant by
    // construction (its pane divides by exactly what its placement multiplies
    // by), the minimap's grows with the product, and both levels are chosen
    // from tiles per *physical* pixel. Splitting the two factors again here
    // would be four spellings of these four rows.
    for physical_per_gump in [1.0_f32, 2.0, 4.0, 8.0] {
        let scale = Scale {
            window: 1.0,
            gump:   physical_per_gump,
        };
        println!();
        println!(
            "   {physical_per_gump:.0} physical pixels to a gump pixel — a desk scale and a HiDPI surface multiplied together"
        );
        println!(
            "   window      zoom   tiles/px  lod   region tiles   drawn   of cache  built at  floor chunks"
        );
        let facet_map: Vec<(i8, RadarView)> = (FACET_MAP_MIN_ZOOM..=FACET_MAP_MAX_ZOOM)
            .map(|zoom| (zoom, facet_map_view(extent, centre, zoom, scale)))
            .collect();
        let minimap: Vec<(i8, RadarView)> = (MINIMAP_MIN_ZOOM..=MINIMAP_MAX_ZOOM)
            .map(|zoom| (zoom, minimap_view(extent, centre, zoom, scale)))
            .collect();
        print_bound_rows("facet map", &facet_map, pages);
        print_bound_rows("minimap", &minimap, pages);
        // One `RadarChunkRenderer` serves both windows (defect 3.5), so the
        // number that meets its capacity is the sum and never either row.
        let widest = |rows: &[(i8, RadarView)]| {
            rows.iter()
                .map(|(_, view)| drawn_chunks(*view, view.lod()))
                .max()
                .unwrap_or(0)
        };
        let together = widest(&facet_map) + widest(&minimap);
        println!(
            "   both windows at their own worst zoom: {together} pages of {pages} ({:.0}%){}",
            percent(together as u64, pages),
            if together as u64 > pages {
                " — one shared array, so this is what truncates"
            } else {
                ""
            },
        );
    }
}

/// One window's zoom range, at the rows where the answer changes.
///
/// Every step of both windows is forty rows of a table nobody reads; a level
/// boundary and the two extremes are the four or five that carry the reading.
fn print_bound_rows(name: &str, rows: &[(i8, RadarView)], pages: u64) {
    let mut previous = None;
    for (index, (zoom, view)) in rows.iter().enumerate() {
        let lod = view.lod();
        let extreme = index == 0 || index + 1 == rows.len();
        if !extreme && previous == Some(lod) {
            continue;
        }
        previous = Some(lod);
        let drawn = drawn_chunks(*view, lod);
        let build_lod = lod.min(SWEEP_LOD);
        let floor = radar::region_chunks(view.region(), build_lod).count();
        let over = if drawn as u64 > pages { "  OVER" } else { "" };
        println!(
            "   {name:<10}  {zoom:>4}  {:>9.3}  {:>3}   {:>5}x{:<5}  {drawn:>6}   {:>6.0}%{over}  lod {}     {floor:>6}",
            view.tiles_per_pixel,
            lod.value(),
            view.region().extent().width(),
            view.region().extent().height(),
            percent(drawn as u64, pages),
            build_lod.value(),
        );
    }
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let mut args = std::env::args().skip(1);
    let parse = |value: Option<String>, fallback: f32| {
        value.map_or(fallback, |value| value.parse().expect("a scale is a number"))
    };
    // Two by two: a HiDPI screen at a desk scale, which is the case R7's own
    // "worst case" names and the one a single development machine is least
    // likely to be.
    let scale = Scale {
        window: parse(args.next(), 2.0),
        gump:   parse(args.next(), 2.0),
    };

    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("facet 0");
    let colors = RadarColors::load(dir.join("radarcol.mul")).expect("radarcol.mul");
    let extent = RadarExtent::new(
        u16::try_from(map.width()).expect("a facet the radar can address"),
        u16::try_from(map.height()).expect("a facet the radar can address"),
    )
    .expect("a facet with an extent");

    println!(
        "radar soak — facet 0 {}x{}, SWEEP_LOD={} max_lod={}",
        map.width(),
        map.height(),
        SWEEP_LOD.value(),
        radar::max_lod(extent).value(),
    );
    println!(
        "scenario: window_scale={} gump_scale={}, facet map content {}x{} gump px, minimap content {}x{}",
        scale.window,
        scale.gump,
        FACET_MAP_CONTENT.0,
        FACET_MAP_CONTENT.1,
        MINIMAP_CONTENT.0,
        MINIMAP_CONTENT.1,
    );

    the_floor_fills_in(&map, &colors, extent, scale);
    walking_costs_no_raster_work(&map, &colors, extent, scale);
    the_bounds(extent);
}
