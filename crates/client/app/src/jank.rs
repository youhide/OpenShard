//! Structured diagnostics for frames that missed the 60 Hz rendering budget.
//!
//! The event is intentionally emitted at the frame boundary rather than from a
//! particular pass: it keeps the CPU, swapchain and delayed GPU measurements in
//! one log record. The target is `jank`, so `RUST_LOG=jank=warn` selects these
//! records without enabling every warning from the client. A caller that needs
//! a persistent trace can initialise [`start_log`] before starting the app.

use std::fs::{
    File,
    OpenOptions,
};
use std::io::{
    self,
    BufWriter,
    Write,
};
use std::path::Path;
use std::sync::{
    Mutex,
    OnceLock,
};
use std::time::Duration;

use crate::frames::{
    Frame,
    JANK_BUDGET,
};
use crate::profile::Pass;
use crate::window::AtlasWork;

static LOG: OnceLock<Mutex<BufWriter<File>>> = OnceLock::new();

/// Whether either destination for jank diagnostics is active.
///
/// Keeping this check at the caller avoids building the detailed timing record
/// in normal gameplay runs.
pub(crate) fn enabled() -> bool {
    LOG.get().is_some() || tracing::enabled!(target: "jank", tracing::Level::WARN)
}

/// CPU phases within [`crate::presentation::App::draw_from`] that are useful
/// when the single [`Frame::scene`] total says the world is slow.
///
/// They deliberately do not add up to a frame: UI, swapchain acquisition and
/// the advance step are accounted for separately by [`Frame`]. These are the
/// expensive, zoom-sensitive world-building phases where a more specific
/// answer is needed.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CpuPasses {
    pub ui_hud: Duration,
    pub ui_terrain: Duration,
    pub ui_route: Duration,
    pub ui_occluders: Duration,
    pub ui_picking: Duration,
    pub ui_perf: Duration,
    pub ui_layout: Duration,
    pub ui_paint: Duration,
    pub facts: Duration,
    pub atlases: Duration,
    pub targets: Duration,
    pub geometry: Duration,
    pub lighting: Duration,
    pub ground: Duration,
    pub statics: Duration,
    pub static_walk: Duration,
    pub static_sort: Duration,
    pub items: Duration,
    /// Copying map-static geometry in or out of the static-geometry cache.
    pub static_cache_copy: Duration,
    /// `sprite::split_corners` over the map statics and the server items.
    pub split_corners: Duration,
    /// The selection, outline and crowd collectors after the frame is
    /// assembled.
    pub overlays: Duration,
    /// How much world this frame is made of. A phase timing cannot be read
    /// without them — see [`crate::frame_geometry::GeometryCosts`].
    pub ground_quads: usize,
    pub static_rows: usize,
    pub item_rows: usize,
    /// Sources handed to the deferred lighting pass, including the carried
    /// lantern when F7 is on. This distinguishes a shader that is expensive
    /// because it walks a real source from one that is expensive while empty.
    pub light_sources: usize,
    /// The sun is a separate source, not one of `light_sources`.
    pub sun_active: bool,
    pub encode: Duration,
    pub encode_ground: Duration,
    pub encode_composites: Duration,
    pub encode_ground_detail: Duration,
    pub ground_detail_cpu_uniforms: Duration,
    pub ground_detail_cpu_serialize: Duration,
    pub ground_detail_cpu_upload: Duration,
    pub ground_detail_cpu_pass: Duration,
    pub encode_statics: Duration,
    pub encode_items: Duration,
    /// Ready immutable blocks restored through the deferred composite pass in
    /// this frame. Paired with `encode_composites` to expose per-block command
    /// recording overhead at far zoom.
    pub composite_blocks: usize,
    pub composite_bindings_created: usize,
    pub composite_bindings_reused: usize,
    pub composite_cpu_upload: Duration,
    pub composite_cpu_bindings: Duration,
    pub composite_cpu_pass: Duration,
    pub static_animated: bool,
}

/// Create a fresh file for jank records from this process.
///
/// Called by the playground before it opens the window, so each invocation has
/// one self-contained trace. The event path never opens files or reports write
/// errors: I/O must not turn a slow frame into a slower one.
pub fn start_log(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    // A client app owns one event loop and its caller initializes this once.
    // Treat a second request in the same process as an ordinary no-op rather
    // than replacing a log another thread might be writing.
    let _ = LOG.set(Mutex::new(BufWriter::new(file)));
    Ok(())
}

/// Write one event for a frame whose CPU or measured GPU work exceeded the
/// budget. GPU timestamps belong to an earlier submitted frame; they remain in
/// the record because a standing GPU cost, rather than exact per-frame pairing,
/// is what identifies a saturated device.
pub fn record(frame: Frame, cpu: CpuPasses, atlas: AtlasWork, gpu_passes: &[Pass]) {
    if !frame.janks() {
        return;
    }
    if !enabled() {
        return;
    }
    let ms = |duration: Duration| duration.as_secs_f64() * 1_000.0;
    let gpu_ms = frame.gpu.map(ms);
    let passes: Vec<_> = gpu_passes
        .iter()
        .map(|pass| (&pass.label, ms(pass.cost)))
        .collect();
    let cpu_passes = [
        ("ui_hud", ms(cpu.ui_hud)),
        ("ui_terrain", ms(cpu.ui_terrain)),
        ("ui_route", ms(cpu.ui_route)),
        ("ui_occluders", ms(cpu.ui_occluders)),
        ("ui_picking", ms(cpu.ui_picking)),
        ("ui_perf", ms(cpu.ui_perf)),
        ("ui_layout", ms(cpu.ui_layout)),
        ("ui_paint", ms(cpu.ui_paint)),
        ("facts", ms(cpu.facts)),
        ("atlases", ms(cpu.atlases)),
        ("targets", ms(cpu.targets)),
        ("geometry", ms(cpu.geometry)),
        ("lighting", ms(cpu.lighting)),
        ("ground", ms(cpu.ground)),
        ("statics", ms(cpu.statics)),
        ("static_walk", ms(cpu.static_walk)),
        ("static_sort", ms(cpu.static_sort)),
        ("items", ms(cpu.items)),
        ("static_cache_copy", ms(cpu.static_cache_copy)),
        ("split_corners", ms(cpu.split_corners)),
        ("overlays", ms(cpu.overlays)),
        ("encode", ms(cpu.encode)),
        ("encode_ground", ms(cpu.encode_ground)),
        ("encode_composites", ms(cpu.encode_composites)),
        ("encode_ground_detail", ms(cpu.encode_ground_detail)),
        ("ground_detail_uniforms", ms(cpu.ground_detail_cpu_uniforms)),
        ("ground_detail_serialize", ms(cpu.ground_detail_cpu_serialize)),
        ("ground_detail_upload", ms(cpu.ground_detail_cpu_upload)),
        ("ground_detail_pass", ms(cpu.ground_detail_cpu_pass)),
        ("composite_upload", ms(cpu.composite_cpu_upload)),
        ("composite_bindings", ms(cpu.composite_cpu_bindings)),
        ("composite_pass", ms(cpu.composite_cpu_pass)),
        ("encode_statics", ms(cpu.encode_statics)),
        ("encode_items", ms(cpu.encode_items)),
    ];
    let atlas_overflowed = atlas.overflow.map(|overflow| overflow.atlas);
    let atlas_packed_graphics = atlas.overflow.map(|overflow| overflow.packed_graphics);
    let atlas_newly_requested_graphics = atlas.overflow.map(|overflow| overflow.newly_requested_graphics);
    if let Some(log) = LOG.get() {
        if let Ok(mut log) = log.lock() {
            let _ = writeln!(
                log,
                "jank frame budget_ms={:.3} interval_ms={:.3} build_ms={:.3} ui_ms={:.3} \
                 scene_ms={:.3} wait_ms={:.3} gpu_ms={gpu_ms:?} repacked={} atlas_uploaded_bytes={} ground_quads={} static_rows={} item_rows={} light_sources={} sun_active={} composite_blocks={} composite_bindings_created={} composite_bindings_reused={} static_animated={} atlas_overflowed={atlas_overflowed:?} atlas_packed_graphics={atlas_packed_graphics:?} atlas_newly_requested_graphics={atlas_newly_requested_graphics:?} cpu_passes={cpu_passes:?} gpu_passes={passes:?}",
                ms(JANK_BUDGET),
                ms(frame.interval),
                ms(frame.build()),
                ms(frame.ui),
                ms(frame.scene),
                ms(frame.wait),
                frame.repacked,
                atlas.uploaded_bytes,
                cpu.ground_quads,
                cpu.static_rows,
                cpu.item_rows,
                cpu.light_sources,
                cpu.sun_active,
                cpu.composite_blocks,
                cpu.composite_bindings_created,
                cpu.composite_bindings_reused,
                cpu.static_animated,
            );
            // Do not flush from the render thread.  A jank record is itself
            // diagnostic output; waiting for the filesystem here can turn one
            // slow frame into a much longer stall, especially when several
            // janks arrive in succession.  BufWriter flushes its full chunks
            // as needed and the final buffer is flushed when the process exits.
        }
    }
    tracing::warn!(
        target: "jank",
        budget_ms = ms(JANK_BUDGET),
        interval_ms = ms(frame.interval),
        build_ms = ms(frame.build()),
        ui_ms = ms(frame.ui),
        scene_ms = ms(frame.scene),
        wait_ms = ms(frame.wait),
        gpu_ms = ?gpu_ms,
        repacked = frame.repacked,
        atlas_uploaded_bytes = atlas.uploaded_bytes,
        ground_quads = cpu.ground_quads,
        static_rows = cpu.static_rows,
        item_rows = cpu.item_rows,
        atlas_overflowed = ?atlas_overflowed,
        atlas_packed_graphics = ?atlas_packed_graphics,
        atlas_newly_requested_graphics = ?atlas_newly_requested_graphics,
        cpu_passes = ?cpu_passes,
        gpu_passes = ?passes,
        "jank frame"
    );
}
