# 5. Scripting — spiked, proven, and deleted

It was the largest open technical risk, and it was answered twice: first that an
embedded V8 *works*, then that this project does not want one.

Issues [#7](https://github.com/youhide/OpenShard/issues/7) and
[#17](https://github.com/youhide/OpenShard/issues/17) settled on pure Rust.
Everything the pack held is in the tree —
`crates/server/state/data/{quests,speech,regions}.json` and
`crates/server/world/data/{spawns,deco,townsfolk,loot}.json`, laid by
`server::content` (the first two at boot, the rest on their admin verbs), plus
the two item behaviours as `world::tick::shipped_items` — and each dataset moved
under a test that compared its `Command`s against the pack's. When the last one
agreed, `crates/server/scripting`, the bridge beside it, `deno_core` and the
`[scripting]` config section were deleted, and `cargo test --workspace` began
running the whole workspace with nothing excluded.

Removing `deno_core` did **not** lower the MSRV. It was measured rather than
assumed: the highest demand in the tree is now `wesl` 0.4.2 at 1.96, a
build-dependency of the client's renderer, so the declared 1.88 had already
stopped being true. See [`development.md`](../../development.md) § What holds the MSRV.

The checklist below is what the spike delivered, kept as the record of what was
built and thrown away; the decision is in
[`architecture.md`](../../architecture.md) § Scripting.

- [x] `deno_core` embedded, one V8 isolate — `DenoEngine`, one `JsRuntime`
- [x] `ScriptEngine` trait — four methods, nothing V8-shaped in a signature, so
  the runtime stays replaceable
- [x] Entity and event bindings exposed to TypeScript — domain events in through
  `deliver`, a read model a hook reads through `op_position`, commands out
  through `op_move`; ops declared with `extension!` and `#[op2]`, all synchronous
- [x] Hot reload without a restart — `load` rebinds the hooks in the live
  isolate; `reload_if_changed` polls a watched file's mtime
- [x] **Benchmark** — `examples/benchmark.rs`, numbers below

## The numbers

The question was whether a per-entity hook fits the tick. The budget is
`TICK_INTERVAL`: **25ms at 40Hz**. Measured on an Apple-silicon dev machine, V8
hosted in a Tokio runtime, release build, warmed up so the JIT has tiered the
hook. `cargo run -p openshard-scripting --example benchmark --release`.

| Hook | per call | 10k mobiles/tick | share of a 25ms tick |
|---|---|---|---|
| empty (`onTick(){}`) — pure Rust↔V8 crossing | ~170 ns | ~1.7 ms | ~7% |
| read + maybe move — `op_position`, then conditionally `op_move` | ~490 ns | ~4.9 ms | ~20% |

The realistic hook — the one a gameplay rule looks like: read the mobile's tile
through an op, decide, and on a condition enqueue a step — costs about half a
microsecond a call. Ten thousand mobiles each firing it every tick spend roughly
a fifth of the budget. **It fits, with room.**

Two honest caveats. The ceiling is *script* time only; a real tick also moves
mobiles, runs interest management and writes packets, so the script share is a
slice of the 25ms, not all of it — the per-call nanoseconds are the number that
travels, not the "calls per tick" ceiling. And the crossing cost is per call, so
a design that calls one hook over a batch of entities will always beat one that
crosses per entity; that is a knob for §6, not a problem for the spike.

The design does not have to change. Gameplay can depend on it.
