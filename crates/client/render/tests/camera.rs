//! The camera bench: every rig over every script, as numbers and as a picture.
//!
//! `crates/client/render/src/bench.rs` is the arithmetic and this is the runner
//! — the split is the crate rule that a renderer opens no files, and it is also
//! what lets a live scope in the window compute the same numbers as this does
//! without dragging a filesystem into the window.
//!
//! Three of the tests below assert; the fourth writes a table and a chart for a
//! person to look at, because a camera is chosen by looking and a number that
//! disagrees with the picture means the metric is wrong.
//!
//! ```sh
//! cargo test -p openshard-client-render --test camera -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Duration;

use openshard_client_render::bench::{self, Cadence, Metrics, Sample, Script, Trace, run, scripts};
use openshard_client_render::chart;
use openshard_client_render::follow::{FLOOR, Rig};
use openshard_movement::WALK_HOLD;
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;

/// A rig with a filter in it, used here to prove the bench can tell two cameras
/// apart at all.
///
/// **Not a preset and not a proposal.** No camera is chosen until C3 has been
/// built and looked at (`docs/camera.md`, D9); what these two numbers are for is
/// that a table with one row in it and a chart with one curve on it cannot show
/// whether they would show anything.
const PROBE: Rig = Rig {
    plane_tau: 0.12,
    lift_tau: 0.25,
    lift_cut: FLOOR,
};

/// Somewhere in the middle of a facet, as the scripts start.
const START: Point = Point::new(1495, 1629, 0);

/// Sixteen milliseconds, which is what a window on a 60Hz screen mostly gets.
const FRAME: Cadence = Cadence::steady(Duration::from_millis(16));

/// Ten steps east and nothing after them, so the last instant is one the body
/// is still walking through.
///
/// The scripts in `bench.rs` end with the body standing, which is right for
/// measuring a rig and wrong for comparing two frame rates: a trailing stand
/// lets every cadence settle onto the same answer, and a comparison that only
/// looks at the end would pass for a filter that is wildly frame-rate
/// dependent in the middle.
fn walking() -> Script {
    (0..10).fold(Script::new("walking", START), |script, _| {
        script.step(Direction::East, WALK_HOLD)
    })
}

/// One of the bench's scenarios, by name.
fn script(name: &str) -> Script {
    scripts()
        .into_iter()
        .find(|script| script.name == name)
        .unwrap_or_else(|| panic!("{name} is one of the bench's scripts"))
}

/// The reference camera is the body, over every scenario the bench has.
///
/// The lag bound is the quantiser and nothing else: rounding each axis to the
/// nearest pixel can put the drawn eye at most half a pixel out on each, which
/// is `sqrt(0.5)` away. Anything above that is a rig trailing, and `HARD` does
/// not trail.
#[test]
fn the_reference_rig_keeps_the_eye_on_the_body() {
    for script in scripts() {
        let trace = run(Rig::HARD, &script, FRAME);
        let metrics = Metrics::of(&trace.samples);
        assert!(
            metrics.lag_max < 0.71,
            "{}: the reference eye was {:.3} pixels off the body",
            script.name,
            metrics.lag_max,
        );
        // The companion, per script: a corridor nothing walked down is not an
        // assertion, and every one of these but the still one walks.
        assert!(metrics.frames > 50, "{}: {} frames", script.name, metrics.frames);
        if script.name != "stand_still" {
            assert!(
                metrics.travel > 40.0,
                "{}: the eye travelled {:.1} pixels, so it was never asked to follow anything",
                script.name,
                metrics.travel,
            );
        }
    }
}

/// The bench's whole claim: it can tell two rigs apart, in both directions.
///
/// A filter buys smoothness with lag, and a bench that only measured one of the
/// two would score a camera that never keeps up as the best one there is. So
/// both halves are asserted on the same run: at a reversal the filtered rig's
/// worst acceleration is a fraction of the reference's, *and* it trails by an
/// order of magnitude more.
///
/// The numbers are the reference's own, not a target: what is pinned is that
/// the difference is large enough to see, which is what makes the table worth
/// printing.
#[test]
fn a_filter_trades_lag_for_smoothness_and_the_bench_measures_both() {
    let kite = script("back_and_forth");
    let hard = Metrics::of(&run(Rig::HARD, &kite, FRAME).samples);
    let probe = Metrics::of(&run(PROBE, &kite, FRAME).samples);

    assert!(
        probe.accel_max * 3.0 < hard.accel_max,
        "the filter smoothed the reversal by less than three times: {:.0} against {:.0}",
        probe.accel_max,
        hard.accel_max,
    );
    assert!(
        probe.lag_max > hard.lag_max * 10.0,
        "and it paid nothing for it: {:.2} against {:.2}",
        probe.lag_max,
        hard.lag_max,
    );
    // Both rigs were asked to follow the same body over the same ground, which
    // is what makes the two numbers comparable at all.
    assert_eq!(hard.frames, probe.frames);
    assert!(hard.travel > 100.0 && probe.travel > 100.0);
}

/// C2's pair, and the only reason the lift cut is a rule rather than a taste: a
/// stair and a floor are the same number of pixels and must not be the same
/// event.
///
/// A stair arrives through the glide, a few pixels a frame across a whole step,
/// and the eye is expected to trail it a little and smooth the corners. A floor
/// arrives whole in one frame, and the eye must be on it immediately — an eye
/// eased eighty pixels down through a storey is the lift shaft this milestone
/// exists to avoid.
///
/// The mirror is the same rig with the cut turned off, because "the eye is on
/// the body after a floor change" is also what a rig with no filter at all
/// does, and a test that cannot tell those apart is testing nothing.
#[test]
fn a_stair_is_eased_and_a_floor_is_cut() {
    let stairs = script("stairs");
    let eased = Metrics::of(&run(Rig::LIFT, &stairs, FRAME).samples);
    let reference = Metrics::of(&run(Rig::HARD, &stairs, FRAME).samples);
    assert!(
        eased.lift_lag_max > 2.0 && eased.lift_lag_max < 12.0,
        "a climbed stair left the eye {:.1} pixels behind, and a riser is 20",
        eased.lift_lag_max,
    );
    // And the finding this milestone turned on, pinned rather than described:
    // a filter on the height makes a *climbed* stair slightly worse, not
    // better. The glide already spreads a step's rise across the whole step, so
    // there is nothing left to smooth — and the lift was *cancelling* part of
    // the plane's motion, since walking east moves the eye down the screen and
    // climbing moves it up. Delaying one half of a cancellation is a transient
    // where there was none. If this ever reads the other way round, the rig has
    // changed shape and `docs/camera.md` C2 is the thing to re-read.
    assert!(
        eased.accel_max > reference.accel_max && eased.accel_max < reference.accel_max * 1.5,
        "a filtered stair was {:.0} against the reference's {:.0}",
        eased.accel_max,
        reference.accel_max,
    );

    let dungeon = script("dungeon");
    let cut = Metrics::of(&run(Rig::LIFT, &dungeon, FRAME).samples);
    let uncut = Metrics::of(
        &run(
            Rig {
                lift_cut: f32::INFINITY,
                ..Rig::LIFT
            },
            &dungeon,
            FRAME,
        )
        .samples,
    );
    assert!(
        cut.lift_lag_max < 1.0,
        "the floor changed and the eye was {:.1} pixels above it",
        cut.lift_lag_max,
    );
    assert!(
        uncut.lift_lag_max > 40.0,
        "with the cut off the same drop left the eye only {:.1} pixels behind, \
         so this scenario does not exercise the cut at all",
        uncut.lift_lag_max,
    );

    // And the kerb, which keeps the rule from being "cut on anything sudden":
    // two units arriving whole is a correction, and it is eased.
    let kerb = Metrics::of(&run(Rig::LIFT, &script("kerb"), FRAME).samples);
    let reference = Metrics::of(&run(Rig::HARD, &script("kerb"), FRAME).samples);
    assert!(
        kerb.lift_lag_max > 1.0,
        "a two-unit correction was cut rather than absorbed",
    );
    // Which is where the filter does pay: the correction arrives whole, and the
    // reference relays all eight pixels of it in one frame. Absorbed, it
    // disappears under the transient the *walk* itself makes — which the plane
    // owns and C3 answers.
    assert!(
        kerb.accel_max * 5.0 < reference.accel_max,
        "the correction was not absorbed: {:.0} against {:.0}",
        kerb.accel_max,
        reference.accel_max,
    );
}

/// D5: the same span of time moves the eye to the same place, whatever the
/// frame rate it arrives in.
///
/// Compared at the coarse run's own timestamps, which are a subset of the fine
/// run's — no interpolation, so a difference is a difference and not a sampling
/// artefact. The tolerance is two pixels rather than zero because the *target*
/// is sampled at those frames too: a filter fed a moving body at 32ms cannot
/// know what it did between the samples, and that is honest error rather than
/// frame-rate dependence.
///
/// The mirror is the point of the test. `lerp` by a constant per frame — the
/// form this repository bans — is written out below and fails the same
/// comparison by an order of magnitude, so the tolerance above is known to be
/// tight enough to catch what it is for.
#[test]
fn the_same_span_lands_in_the_same_place_at_any_frame_rate() {
    let script = walking();
    let fine = run(PROBE, &script, Cadence::steady(Duration::from_millis(4)));
    let coarse = run(PROBE, &script, Cadence::steady(Duration::from_millis(32)));
    let apart = furthest_apart(&coarse, &fine);
    assert!(apart < 2.0, "the eye was {apart:.2} pixels apart at 4ms and 32ms");

    // Jittered, where no timestamp lines up: the last frame is common to both
    // and the body is still walking through it, so the steady-state lag is
    // being compared rather than a settled position.
    let jittery = run(
        PROBE,
        &script,
        Cadence::jittered(Duration::from_millis(16), Duration::from_millis(12), 9),
    );
    let (last, reference) = (
        jittery.samples.last().unwrap().state.exact(),
        fine.samples.last().unwrap().state.exact(),
    );
    let off = (last.0 - reference.0).hypot(last.1 - reference.1);
    assert!(off < 2.0, "a jittery loop ended {off:.2} pixels out");

    // And the form that would pass every test anybody writes at one frame rate.
    let naive_apart = {
        let fine = naive_lerp(&script, Duration::from_millis(4));
        let coarse = naive_lerp(&script, Duration::from_millis(32));
        (fine.0 - coarse.0).hypot(fine.1 - coarse.1)
    };
    assert!(
        naive_apart > 10.0,
        "the banned form landed {naive_apart:.2} pixels apart, which is inside the tolerance above: \
         this test cannot tell the two forms apart and is not testing what it says",
    );
}

/// The whole bench, written out for a person: a table, a CSV per run and a
/// chart per script with the rigs overlaid.
///
/// Asserts nothing. The table is the baseline C1 exists to record — every later
/// milestone is a diff against these numbers — and the charts are what a camera
/// is actually chosen by.
#[test]
#[ignore = "writes a table and charts for a person, and asserts nothing"]
fn dump_the_camera_bench() {
    let rigs = [("hard", Rig::HARD), ("lift", Rig::LIFT), ("probe", PROBE)];
    let out = dump_dir();
    std::fs::create_dir_all(&out).expect("a directory under target");

    println!(
        "\n{:<16} {:<6} {:>6} {:>8} {:>8} {:>8} {:>8} {:>9} {:>10} {:>8} {:>6}",
        "script",
        "rig",
        "frames",
        "travel",
        "lag max",
        "lag rms",
        "ahead",
        "speed max",
        "accel max",
        "jerk rms",
        "step σ²",
    );
    for script in scripts() {
        let traces: Vec<(&str, Trace)> = rigs
            .iter()
            .map(|(name, rig)| (*name, run(*rig, &script, FRAME)))
            .collect();
        for (name, trace) in &traces {
            let metrics = Metrics::of(&trace.samples);
            println!(
                "{:<16} {:<6} {:>6} {:>8.1} {:>8.2} {:>8.2} {:>8.2} {:>9.1} {:>10.0} {:>8.0} {:>6.3}",
                script.name,
                name,
                metrics.frames,
                metrics.travel,
                metrics.lag_max,
                metrics.lift_lag_max,
                metrics.ahead_max,
                metrics.speed_max,
                metrics.accel_max,
                metrics.jerk_rms,
                metrics.step_var,
            );
            let path = out.join(format!("{}-{name}.csv", script.name));
            std::fs::write(&path, csv(trace)).expect("writing a run");
        }
        let path = out.join(format!("{}.svg", script.name));
        std::fs::write(&path, chart(&script, &traces)).expect("writing a chart");
    }
    // And the sweep the lift's time constant was chosen from: what a climb
    // costs in trailing height, against what the same clock does to a
    // correction that arrives whole. Printed rather than asserted — it is the
    // shape of the trade, and the choice is `Rig::LIFT`.
    println!(
        "\n{:<10} {:>10} {:>12} {:>12} {:>12}",
        "lift tau", "stair lag", "kerb accel", "ledge px/s", "stair accel"
    );
    for tau in [0.05, 0.10, 0.15, 0.20, 0.30, 0.50] {
        let rig = Rig {
            lift_tau: tau,
            ..Rig::LIFT
        };
        let stairs = Metrics::of(&run(rig, &script("stairs"), FRAME).samples);
        let kerb = Metrics::of(&run(rig, &script("kerb"), FRAME).samples);
        let ledge = Metrics::of(&run(rig, &script("ledge"), FRAME).samples);
        println!(
            "{tau:<10.2} {:>10.1} {:>12.0} {:>12.0} {:>12.0}",
            stairs.lift_lag_max, kerb.accel_max, ledge.speed_max, stairs.accel_max,
        );
    }

    println!("\nwrote {}", out.display());
}

// --- The runner's own plumbing ---------------------------------------------

/// `target/camera`, or wherever `OPENSHARD_CAMERA_DUMP` says.
///
/// Under `target` by default and never in the source tree: this writes a file
/// per rig per script and none of them belongs in a diff.
fn dump_dir() -> PathBuf {
    if let Some(set) = std::env::var_os("OPENSHARD_CAMERA_DUMP") {
        return PathBuf::from(set);
    }
    // `CARGO_TARGET_TMPDIR` is `<target>/tmp` for an integration test, which is
    // the only pointer to the target directory a test is given.
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .expect("<target>/tmp has a parent")
        .join("camera")
}

/// The furthest the two runs were apart, at the timestamps they share.
///
/// Every timestamp of the coarse run must exist in the fine one — which it does
/// when one frame time divides the other — and a missing one is a failure
/// rather than a skip, because silently comparing nothing is how this kind of
/// test goes green.
fn furthest_apart(coarse: &Trace, fine: &Trace) -> f64 {
    let mut worst = 0.0f64;
    let mut compared = 0;
    for sample in &coarse.samples {
        let found = fine
            .samples
            .binary_search_by_key(&sample.at, |other| other.at)
            .map(|index| fine.samples[index].state.exact());
        let Ok(other) = found else {
            panic!("{:?} of the coarse run is not a frame of the fine one", sample.at);
        };
        worst = worst.max((sample.state.exact().0 - other.0).hypot(sample.state.exact().1 - other.1));
        compared += 1;
    }
    assert!(compared > 20, "only {compared} frames were compared");
    worst
}

/// The banned form: a constant fraction of the remaining distance, per frame.
///
/// Here so the test above can be shown to have teeth, and nowhere else. Its
/// time constant is `step / alpha`, so halving the frame time halves how far
/// the eye trails — which is a camera whose character changes with the frame
/// rate, and this client has two frame rates on purpose.
fn naive_lerp(script: &Script, step: Duration) -> (f64, f64) {
    const ALPHA: f64 = 0.15;
    let mut eye = script.gaze_at(Duration::ZERO).exact();
    let mut now = Duration::ZERO;
    while now < script.length {
        now = (now + step).min(script.length);
        let target = script.gaze_at(now).exact();
        eye = (
            eye.0 + (target.0 - eye.0) * ALPHA,
            eye.1 + (target.1 - eye.1) * ALPHA,
        );
    }
    eye
}

/// One run, frame by frame.
fn csv(trace: &Trace) -> String {
    let mut out = String::from("ms,body_x,body_y,body_lift,eye_x,eye_y,state_x,state_y,state_lift\n");
    for sample in &trace.samples {
        let body = sample.gaze.exact();
        out.push_str(&format!(
            "{},{:.3},{:.3},{:.3},{},{},{:.3},{:.3},{:.3}\n",
            sample.at.as_millis(),
            body.0,
            body.1,
            sample.gaze.lift,
            sample.eye.x,
            sample.eye.y,
            sample.state.x,
            sample.state.y,
            sample.state.lift,
        ));
    }
    out
}

/// Two panels — how fast the eye moved, and how far behind it was — with every
/// rig overlaid on each.
///
/// Overlaid deliberately: one curve says nothing, and two on one axis is how
/// raggedness stops being a feeling. The drawing itself is
/// [`openshard_client_render::chart`], which the walk harness in `client/app`
/// also draws with: two pictures of the same pipeline have to be readable
/// against each other.
fn chart(script: &Script, traces: &[(&str, Trace)]) -> String {
    let speed = |trace: &Trace| derivative(trace);
    let lag = |trace: &Trace| {
        trace
            .samples
            .iter()
            .map(|sample| {
                let body = sample.gaze.exact();
                let eye = (f64::from(sample.eye.x), f64::from(sample.eye.y));
                (sample.at.as_secs_f64(), (eye.0 - body.0).hypot(eye.1 - body.1))
            })
            .collect::<Vec<_>>()
    };

    let panels = vec![
        chart::Panel {
            title: "the eye's speed, pixels per second".to_string(),
            series: speed_series(traces, speed),
            baseline: None,
        },
        chart::Panel {
            title: "how far behind the body, pixels".to_string(),
            series: speed_series(traces, lag),
            // The eye is meant to be on the body; the dashed line is what every
            // rig is departing from.
            baseline: Some(0.0),
        },
    ];
    chart::svg(script.name, script.length.as_secs_f64(), &panels)
}

/// A named curve per rig, from whatever the caller measures.
fn speed_series(traces: &[(&str, Trace)], of: impl Fn(&Trace) -> Vec<(f64, f64)>) -> Vec<chart::Series> {
    traces
        .iter()
        .map(|(name, trace)| chart::Series {
            name: (*name).to_string(),
            points: of(trace),
        })
        .collect()
}

/// The eye's speed over time, from the one place the differencing is written.
///
/// `bench::readings` and not a copy of it here: the curve on this chart and the
/// peak in the table above it have to be the same arithmetic, or a disagreement
/// between the two says nothing about which is wrong.
fn derivative(trace: &Trace) -> Vec<(f64, f64)> {
    bench::readings(&trace.samples)
        .into_iter()
        .map(|reading| (reading.at.as_secs_f64(), reading.speed))
        .collect()
}

/// A sample is a plain value and the metrics take a slice of them, which is
/// what lets the DST harness and a live scope measure what this measures.
/// Pinned here because it is an API promise the bench depends on.
#[test]
fn the_metrics_take_nothing_but_samples() {
    let samples: Vec<Sample> = run(Rig::HARD, &walking(), FRAME).samples;
    let whole = Metrics::of(&samples);
    let half = Metrics::of(&samples[..samples.len() / 2]);
    assert!(half.frames < whole.frames);
    assert!(half.travel < whole.travel);
}
