//! The spike's whole reason for coming first: how much does a script call cost
//! inside a tick, and how many mobiles can fire a hook every tick before it
//! stops fitting?
//!
//! A tick is 50ms at 20Hz (`TICK_INTERVAL`). This measures the per-call cost of
//! [`ScriptEngine::tick`] for two hooks and reports, for each, how many calls
//! fit in one tick's budget — with the honest caveat that a real tick spends
//! most of that budget on everything *else* it does, so the script share is a
//! fraction of these ceilings, not the ceiling itself.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p openshard-scripting --example benchmark --release
//! ```
//!
//! Release matters: V8 is JIT and the surrounding Rust wants optimising too. The
//! numbers from a debug build are meaningless.

use std::time::{Duration, Instant};

use openshard_scripting::{DenoEngine, Event, ScriptEngine};

/// One tick's budget: 50ms at 20Hz, mirrored from `world::TICK_INTERVAL` (not
/// depended on, to keep this crate off the world graph).
const TICK_BUDGET: Duration = Duration::from_millis(50);

/// An empty hook — pure cost of crossing from Rust into V8 and back, nothing
/// done inside. The floor.
const EMPTY_HOOK: &str = "function onTick(serial) {}";

/// A hook a real gameplay rule might be: read the mobile's position through an
/// op, and on a condition enqueue a step. Two op crossings on the taken branch,
/// one on the other. Representative of "look, decide, maybe act".
const REALISTIC_HOOK: &str = "function onTick(serial) {\n\
    const p = Deno.core.ops.op_position(serial);\n\
    if (p !== null && ((p[0] + p[1]) & 3) === 0) {\n\
        Deno.core.ops.op_move(serial, (p[0] + p[1]) & 7);\n\
    }\n\
}";

fn main() {
    // Host the isolate inside a Tokio runtime, as the real shard does under
    // `#[tokio::main]`. V8 posts background GC and compilation as delayed tasks;
    // with a runtime present they are honoured, and the measured cost is the one
    // the shard would see rather than a fallback path.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();

    println!("script-call overhead inside a tick");
    println!(
        "budget: {:.0}ms per tick (20Hz)\n",
        TICK_BUDGET.as_secs_f64() * 1000.0
    );

    for (label, hook) in [
        ("empty hook", EMPTY_HOOK),
        ("read + maybe move", REALISTIC_HOOK),
    ] {
        bench(label, hook);
    }

    println!(
        "\nReading: the ceilings are script time only. A real tick also moves\n\
         mobiles, runs interest management and writes packets, so the script\n\
         budget is a slice of the 50ms, not all of it. Treat the ceiling as an\n\
         order of magnitude, and the per-call ns as the number that matters."
    );
}

/// Measure one hook across a range of mobile counts.
fn bench(label: &str, hook: &str) {
    let mut engine = DenoEngine::new();
    engine.load(hook).expect("hook loads");

    // The largest population we test, seeded once. A hook reads position through
    // the read model, so the entities have to exist for the realistic hook to do
    // its work rather than see `null` and bail.
    let max = 10_000u32;
    for serial in 1..=max {
        engine
            .deliver(&Event::PlayerEntered {
                serial,
                x: (serial % 6144) as u16,
                y: (serial % 4096) as u16,
                z: 0,
            })
            .expect("seed");
    }
    // Whatever the seeding queued, clear it; we measure ticks, not setup.
    engine.take_commands();

    // Warm up: let V8 tier the hook up to optimised code before timing.
    for _ in 0..3 {
        for serial in 1..=max {
            engine.tick(serial).expect("tick");
        }
        engine.take_commands();
    }

    println!("{label}:");
    for &count in &[1_000u32, 10_000u32] {
        // Repeat the sweep so the timed window is milliseconds, not microseconds,
        // and average out scheduler noise.
        let rounds = 20;
        let start = Instant::now();
        for _ in 0..rounds {
            for serial in 1..=count {
                engine.tick(serial).expect("tick");
            }
            engine.take_commands();
        }
        let elapsed = start.elapsed();

        let calls = (rounds * count) as f64;
        let per_call = elapsed.as_secs_f64() / calls;
        let per_call_ns = per_call * 1e9;
        let sweep_ms = elapsed.as_secs_f64() / rounds as f64 * 1000.0;
        let fit = (TICK_BUDGET.as_secs_f64() / per_call) as u64;

        println!(
            "  {count:>6} mobiles/tick  {per_call_ns:>7.0} ns/call  \
             {sweep_ms:>7.3} ms/sweep  ceiling {fit:>8} calls / 50ms",
        );
    }
    println!();
}
