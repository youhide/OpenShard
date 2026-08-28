//! What a frame cost on the far side of the submit, and a socket for the rest.
//!
//! # The hole this fills
//!
//! [`crate::frames`] times the frame with a clock on this thread, and it is
//! honest about everything a clock on this thread can see: [`Frame::ui`] is
//! `egui` laying the panels out, [`Frame::scene`] is the world being walked and
//! turned into commands. Both of them stop at `queue.submit`, which **does not
//! block** — it hands a command buffer to the driver and returns. So every pass
//! those commands describe is timed by nobody at all, and a client whose GPU is
//! the bottleneck reports a small `ui`, a small `world`, and no explanation for
//! where the frame went.
//!
//! It does not vanish. It comes back one frame later inside
//! `get_current_texture`, where the swapchain has no image free because the GPU
//! is still drawing into the last one — and [`Frame::wait`] records that, under
//! a doc comment saying it "is not a cost, it is the pacer working". Which is
//! true under `PresentMode::Fifo` when the client is *idle*, and false in
//! exactly the case somebody opens the panel to diagnose. A saturated GPU and a
//! client asleep on vsync are the same reading. This module is what tells them
//! apart.
//!
//! [`Frame::ui`]: crate::frames::Frame::ui
//! [`Frame::scene`]: crate::frames::Frame::scene
//! [`Frame::wait`]: crate::frames::Frame::wait
//!
//! # Timestamps, and why the number is a few frames old
//!
//! [`Gpu`] writes a timestamp query on either side of each pass, resolves them
//! into a buffer at the end of the frame, and maps that buffer back. The map
//! cannot complete before the GPU has finished the work being timed — that is
//! the whole point — so the results this exposes are from a frame two or three
//! back, whichever one the device has caught up with.
//!
//! That staleness is deliberate and it is not a defect to be fixed by waiting.
//! A `poll(Wait)` after the submit would give the current frame's number and
//! would do it by blocking the CPU on the GPU, which is precisely the stall
//! being measured: the tool would create the reading. A number three frames old
//! answers "what is this frame costing" perfectly well, because the thing being
//! looked for is a standing cost and not one frame's spike — and a spike large
//! enough to matter is still in the panel's four-second window when it arrives.
//!
//! # Why absent rather than zero
//!
//! Timestamp queries are an optional device feature, and some adapters — most
//! software rasterisers, some drivers under some compositors — have not got
//! them. [`Gpu::new`] returns `None` there, and the panel says so in words.
//! A zero would read as "the GPU cost nothing", which is the opposite of what is
//! known, and it would be *the* wrong answer to give somebody chasing a frame
//! rate. Nothing here defaults.
//!
//! # Why the flamegraph is not in this window
//!
//! The per-pass totals below are the answer to "which pass", and they go in the
//! HUD beside `ui`/`world`/`waited` where that question is asked. "Which
//! *function*" is a different tool — a flamegraph, thousands of nested spans a
//! frame — and it is served over a socket to the standalone `puffin_viewer`
//! rather than drawn here.
//!
//! Not by preference: `puffin_egui` draws exactly that flamegraph inside an
//! `egui` window, and its latest release is on `egui 0.28` against this crate's
//! `0.35`. Taking it would put a second `egui` in the tree, with its own
//! context, its own fonts and its own idea of the pointer. The socket costs a
//! `cargo install puffin_viewer` and nothing else.
//!
//! The two halves meet anyway: [`Gpu::end_frame`] reports its spans into the
//! same puffin frame as the CPU's, on a thread called `GPU`, so one recording
//! holds both.
//!
//! **They are not on the same clock, and no arithmetic here can put them
//! there.** A timestamp query is a count on the device's own timebase, whose
//! zero `wgpu` does not define — `GpuTimerQueryResult::time` says as much — so
//! what a GPU row in the viewer means is its *width*, and where it sits along
//! the axis relative to a CPU row means nothing. Reading a pass's cost off it is
//! sound; reading "this pass ran while that function did" is not. The panel's
//! own numbers are widths only, which is why they are the ones to trust for
//! "which pass".

use std::time::Duration;

use wgpu_profiler::{GpuProfiler, GpuProfilerSettings, GpuTimerQueryResult};

/// What the GPU spent inside one scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    /// The label the scope was opened with — a pass name, and the same string
    /// every frame, so a row in the panel keeps its place.
    pub label: String,
    /// How many scopes this one is nested inside. Zero is a scope opened
    /// directly on the frame's encoder, which is all of them today; it is
    /// carried rather than flattened away so that [`Gpu::total`] can add up the
    /// top level only and not count a nested scope twice.
    pub depth: usize,
    /// The GPU's own elapsed time between this scope's two timestamps.
    pub cost: Duration,
}

/// The GPU half of a frame's cost: a timestamp query around each pass, and the
/// last resolved frame's answer.
///
/// Held by the window rather than by the app, for a borrow reason that is worth
/// writing down: [`begin`] and [`end`] take the profiler by shared reference, so
/// a query can be opened on the encoder while the pass it brackets takes `&mut`
/// of a *different* field of the same screen. Were this on `App`,
/// `self.window.as_mut()` would already have borrowed all of it.
pub struct Gpu {
    profiler: GpuProfiler,
    /// The last frame whose queries came back, flattened depth-first — which is
    /// the order the passes were recorded in, so the panel reads down the frame.
    passes: Vec<Pass>,
    /// The sum of the top-level scopes in `passes`.
    total: Duration,
}

impl Gpu {
    /// What a device has to have before any of this measures anything.
    ///
    /// Both, and not just `TIMESTAMP_QUERY`: the scopes here are opened on the
    /// *encoder*, because the passes themselves are begun inside
    /// `openshard_client_render` and this crate never sees their descriptors to
    /// hang a `timestamp_writes` on. Writing a timestamp at encoder level is the
    /// second feature. Without it `wgpu-profiler` silently records no time at
    /// all — every scope comes back with `time: None` — which would be a panel
    /// full of dashes and no clue as to why.
    pub const REQUIRED: wgpu::Features =
        wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);

    /// A profiler for this device, or `None` if it cannot time anything.
    ///
    /// The caller is expected to have asked for [`Gpu::REQUIRED`] in its
    /// `DeviceDescriptor` — and to have asked for only what the adapter
    /// advertised, since requiring a feature that is not there fails the whole
    /// `request_device` and would cost the client its window over a diagnostic.
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if !device.features().contains(Self::REQUIRED) {
            tracing::info!(
                "the adapter cannot write timestamp queries: the frames panel will have no GPU column"
            );
            return None;
        }
        // Spelled out rather than defaulted, because two of the three are
        // decisions. `enable_debug_groups` makes each scope a debug group as
        // well as a query, which is what puts these same names down the side of
        // a RenderDoc capture — the labels are wanted for their own sake, not as
        // a side effect. `max_num_pending_frames` is how many frames of queries
        // may be in flight before `end_frame` starts dropping them: three is the
        // library's own figure and is comfortably more than the two this client
        // is ever behind by.
        let settings = GpuProfilerSettings {
            enable_timer_queries: true,
            enable_debug_groups: true,
            max_num_pending_frames: 3,
        };
        match GpuProfiler::new(device, settings) {
            Ok(profiler) => Some(Self {
                profiler,
                passes: Vec::new(),
                total: Duration::ZERO,
            }),
            // A profiler that would not start is a diagnostic that is absent,
            // not a frame that failed: the client goes on drawing without it.
            Err(error) => {
                tracing::warn!(%error, "starting the GPU profiler");
                None
            }
        }
    }

    /// Copy this frame's timestamps out of their query sets, into the buffers
    /// [`Gpu::end_frame`] will map.
    ///
    /// Recorded into the frame's own encoder and therefore **before** it is
    /// submitted — and after the last scope on it has closed, since a query
    /// still open has nothing to resolve.
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.profiler.resolve_queries(encoder);
    }

    /// Close the frame, and take whatever older frame the device has finished.
    ///
    /// Called after the submit. The poll is [`wgpu::PollType::Poll`] — a single
    /// non-blocking check — because the map callbacks fire on a poll and nothing
    /// else in this loop makes one; blocking here would be the stall this exists
    /// to measure. When no frame is ready yet the last one's numbers stand,
    /// which is what keeps the panel from flickering between a reading and a
    /// dash at the device's own cadence.
    pub fn end_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Err(error) = self.profiler.end_frame() {
            tracing::warn!(%error, "closing the GPU profiler's frame");
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        let Some(results) = self.profiler.process_finished_frame(queue.get_timestamp_period()) else {
            return;
        };
        // Into the same puffin frame the CPU's spans went into, under a thread
        // of its own — see the module docs, and the caveat there about the two
        // clocks. Skipped outright when nobody is recording: this one is not a
        // macro that compiles to an atomic load, it walks the tree and registers
        // a scope name per pass.
        if puffin::are_scopes_on() {
            wgpu_profiler::puffin::output_frame_to_puffin(&mut puffin::GlobalProfiler::lock(), &results);
        }
        self.passes.clear();
        flatten(&results, 0, &mut self.passes);
        self.total = self
            .passes
            .iter()
            .filter(|pass| pass.depth == 0)
            .map(|pass| pass.cost)
            .sum();
    }

    /// Every scope of the last resolved frame, in the order they were recorded.
    pub fn passes(&self) -> &[Pass] {
        &self.passes
    }

    /// What the whole frame cost the GPU: the top-level scopes added up.
    ///
    /// The number to put beside `build` — and the one that says whether a large
    /// `waited` is a client with room to spare or a client waiting on its own
    /// last frame.
    pub fn total(&self) -> Duration {
        self.total
    }
}

/// A query opened on the frame's encoder and not yet closed.
///
/// `None` covers both "there is no profiler" and "the device reserved nothing",
/// so a call site carries no branch of its own: [`begin`] and [`end`] are called
/// unconditionally and do nothing when there is nothing to do.
pub type Open = Option<wgpu_profiler::GpuProfilerQuery>;

/// Start timing whatever is recorded into `encoder` from this point.
///
/// # Why this is not the RAII scope `wgpu-profiler` offers
///
/// `GpuProfiler::scope` returns a guard that borrows the encoder for its whole
/// life and closes the query when it drops — which is the better construct, and
/// it would mean enclosing every pass in `App::draw` in a block of its own to
/// end that borrow before the next pass needs the encoder. That is an extra
/// indent on some hundreds of lines of a function several other things are being
/// changed in, to buy a guarantee that is already enforced from the other end:
/// [`GpuProfiler::end_frame`] refuses a frame with a query still open, and
/// [`Gpu::end_frame`] logs the refusal. A forgotten [`end`] is a warning every
/// frame and an empty panel — loud, and not a silently wrong number.
pub fn begin(gpu: Option<&Gpu>, label: &str, encoder: &mut wgpu::CommandEncoder) -> Open {
    Some(gpu?.profiler.begin_query(label.to_string(), encoder))
}

/// Stop timing at this point, on the same encoder [`begin`] was given — which is
/// `wgpu-profiler`'s own precondition, and the only one a call site has to keep.
pub fn end(gpu: Option<&Gpu>, encoder: &mut wgpu::CommandEncoder, open: Open) {
    if let (Some(gpu), Some(query)) = (gpu, open) {
        gpu.profiler.end_query(encoder, query);
    }
}

/// Depth-first, so a nested scope lands directly under the one that contains it.
///
/// A scope whose `time` is `None` had no timestamp written for it — the device
/// declined the query — and it is dropped rather than recorded as zero, for the
/// reason [`Gpu::new`] returns `None` rather than a profiler that measures
/// nothing.
fn flatten(results: &[GpuTimerQueryResult], depth: usize, into: &mut Vec<Pass>) {
    for result in results {
        if let Some(time) = &result.time {
            match Duration::try_from_secs_f64(time.end - time.start) {
                Ok(cost) => into.push(Pass {
                    label: result.label.clone(),
                    depth,
                    cost,
                }),
                Err(error) => {
                    // These values have crossed the GPU/driver boundary. A bad
                    // sample is not a client invariant and must neither panic
                    // nor impersonate a physically plausible zero-cost pass.
                    tracing::warn!(
                        label = %result.label,
                        start = time.start,
                        end = time.end,
                        %error,
                        "dropping an invalid GPU timestamp range"
                    );
                }
            }
        }
        flatten(&result.nested_queries, depth + 1, into);
    }
}

/// The environment variable that opens the flamegraph socket, and what it holds:
/// an address to listen on, or `1` for [`PUFFIN_DEFAULT`].
///
/// An explicit switch rather than always-on. `puffin`'s scopes cost an atomic
/// load each when recording is off, which is nothing — but a process that binds
/// a TCP port because it was started is a surprise, and this one is often
/// started by a test harness.
pub const PUFFIN_ENV: &str = "OPENSHARD_PUFFIN";

/// Where `puffin_viewer` looks unless told otherwise.
pub const PUFFIN_DEFAULT: &str = "127.0.0.1:8585";

/// Open the flamegraph socket if [`PUFFIN_ENV`] asks for it, and turn recording
/// on.
///
/// The returned server is the whole of the subscription: dropping it closes the
/// port, so the caller has to keep it for as long as the client runs. `None` is
/// both "nobody asked" and "the port would not bind" — neither is a reason not
/// to draw, and both are logged.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn serve() -> Option<puffin_http::Server> {
    let asked = std::env::var(PUFFIN_ENV).ok()?;
    let address = match asked.trim() {
        "" | "0" | "false" => return None,
        "1" | "true" => PUFFIN_DEFAULT,
        other => other,
    };
    match puffin_http::Server::new(address) {
        Ok(server) => {
            // Off by default inside `puffin` itself, and this is the only place
            // it is turned on: a scope macro with recording off is an atomic
            // load, so the cost of leaving them in the source is not a reason to
            // gate them behind a build flag.
            puffin::set_scopes_on(true);
            tracing::info!(%address, "puffin: connect puffin_viewer to this address");
            Some(server)
        }
        Err(error) => {
            tracing::warn!(%address, %error, "opening the puffin socket");
            None
        }
    }
}

/// One frame, as far as `puffin` is concerned: everything scoped since the last
/// call to this belongs to the frame it closes.
///
/// Called at the top of the draw, so the frame boundary in the viewer is the
/// same boundary [`crate::frames`] measures its interval across. Free when
/// nothing is recording.
pub fn frame() {
    puffin::GlobalProfiler::lock().new_frame();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A result with a time, in seconds, and whatever is nested inside it.
    fn query(label: &str, start: f64, end: f64, nested: Vec<GpuTimerQueryResult>) -> GpuTimerQueryResult {
        GpuTimerQueryResult {
            label: label.to_string(),
            pid: std::process::id(),
            tid: std::thread::current().id(),
            time: Some(start..end),
            nested_queries: nested,
        }
    }

    /// The order is the order the passes were recorded in, and a nested scope
    /// comes directly after the one it is inside: the panel reads down a frame
    /// the way the frame was drawn.
    #[test]
    fn the_passes_come_out_depth_first_and_carry_their_depth() {
        let results = vec![
            query("ground", 0.0, 0.001, Vec::new()),
            query(
                "blit",
                0.001,
                0.010,
                vec![query("lighting", 0.002, 0.009, Vec::new())],
            ),
        ];
        let mut passes = Vec::new();
        flatten(&results, 0, &mut passes);
        let read: Vec<(&str, usize)> = passes
            .iter()
            .map(|pass| (pass.label.as_str(), pass.depth))
            .collect();
        assert_eq!(read, [("ground", 0), ("blit", 0), ("lighting", 1)]);
    }

    /// The whole point of carrying the depth: a scope inside another one is not
    /// a second helping of the frame's time.
    #[test]
    fn a_nested_scope_is_not_added_to_the_frame_twice() {
        let results = vec![query(
            "blit",
            0.000,
            0.010,
            vec![query("lighting", 0.001, 0.009, Vec::new())],
        )];
        let mut passes = Vec::new();
        flatten(&results, 0, &mut passes);
        let total: Duration = passes
            .iter()
            .filter(|pass| pass.depth == 0)
            .map(|pass| pass.cost)
            .sum();
        assert_eq!(total, Duration::from_millis(10));
    }

    /// A sound pair of timestamps is converted to its width on the GPU's
    /// clock; their otherwise undefined absolute position is irrelevant.
    #[test]
    fn a_valid_timestamp_range_becomes_its_elapsed_duration() {
        let results = vec![query("ground", 42.0, 42.004, Vec::new())];
        let mut passes = Vec::new();
        flatten(&results, 0, &mut passes);
        assert_eq!(passes[0].cost, Duration::from_millis(4));
    }

    /// Timestamp data has crossed the GPU/driver boundary, so an inverted pair
    /// is a bad sample to omit rather than a client invariant to panic on or a
    /// zero-cost pass to invent.
    #[test]
    fn a_reversed_timestamp_range_is_left_out() {
        let results = vec![query("ground", 2.0, 1.0, Vec::new())];
        let mut passes = Vec::new();
        flatten(&results, 0, &mut passes);
        assert!(passes.is_empty());
    }

    /// NaN used to survive subtraction and `max` as zero, which made corrupt
    /// timestamp data look like a real pass that happened to be free.
    #[test]
    fn a_nan_timestamp_range_is_left_out() {
        let results = vec![query("ground", f64::NAN, 1.0, Vec::new())];
        let mut passes = Vec::new();
        flatten(&results, 0, &mut passes);
        assert!(passes.is_empty());
    }

    /// A scope the device wrote no timestamp for is absent, not nought: a pass
    /// reported as costing nothing is a wrong answer, and a missing row is a
    /// true one. Same reason [`Gpu::new`] hands back `None`.
    #[test]
    fn a_scope_with_no_timestamp_is_left_out_rather_than_recorded_as_zero() {
        let results = vec![GpuTimerQueryResult {
            label: "ground".to_string(),
            pid: std::process::id(),
            tid: std::thread::current().id(),
            time: None,
            nested_queries: Vec::new(),
        }];
        let mut passes = Vec::new();
        flatten(&results, 0, &mut passes);
        assert!(passes.is_empty());
    }
}
