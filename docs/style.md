# Style

Beyond `cargo fmt` and `cargo clippy`, which are not negotiable.

## Comments explain why

The code already says what it does. A comment earns its place by saying something
the code cannot.

```rust
// Bad — restates the line below.
// Bump the generation.
self.generations[slot] += 1;

// Good — says why it matters.
// Bump the generation so the stale handle can never match again.
self.generations[slot] += 1;
```

The best comments record a decision and its cost:

```rust
// Allocation is a monotonic watermark per pool — freed serials are *not*
// recycled. Reuse would let a client that is mid-packet-flight act on a
// serial that now names a different object.
```

Nobody can recover that from the code. That is the test.

## Doc comments say what something is for

Not what its signature already says.

```rust
/// Resolve a serial off the wire to a live entity.
///
/// This is the hot path for nearly every incoming packet.
pub fn entity_of(&self, serial: Serial) -> Option<EntityId>
```

Document the failure modes and the panics. If a function panics, say when.

## Tests name the behaviour they protect

The test name is the specification. When it fails at 3am, the name should be
enough.

```rust
// Bad
#[test]
fn test_serial_2() {}

// Good
#[test]
fn serials_are_not_reused_after_despawn() {}
```

Assertion messages explain the failure, not the assertion:

```rust
assert_eq!(reg.entity_of(s), None, "a dead serial resolves to nothing");
```

Where a test guards something non-obvious, say what:

```rust
// A client packet in flight may still name the old serial; handing it to a
// new object would let the client act on the wrong thing.
```

Test the boundaries and the failures, not the happy path. A test that only proves
`insert` then `get` works proves very little.

## Errors are types

No `String` errors, no `anyhow` in library crates. `anyhow` is fine in binaries.

```rust
pub enum BindSerialError {
    NoSuchEntity(EntityId),
    SerialTaken { serial: Serial, holder: EntityId },
    AlreadyBound { entity: EntityId, existing: Serial },
}
```

Carry what a caller needs to act, and implement `Display` + `std::error::Error`.

## Panics

Panic on programmer error — a broken invariant, a type mismatch that cannot
happen. Return `Result` for anything the outside world can cause.

Network input is never a panic. Ever. `ClientVersion::from_str` returns an error
because that string arrives in a packet from an untrusted client.

## No unsafe

Denied workspace-wide. If two mutable borrows into one structure are needed,
split a slice — see `Registry::for_each2_mut`. If a case looks genuinely
impossible without it, that is a design discussion, not a local decision.

## No globals

No `static mut`, no `lazy_static` singletons, no ambient state. Pass the
`Registry`. Pass the `EventBus`. This is what lets tests build worlds freely and
what will let the simulation shard across cores.

## Layering

A crate depends downward or sideways at the same level, never upward.
`entities` and `events` know nothing about gameplay and must stay that way —
if `entities` ever needs to know what a house is, the layering broke.

## Randomness and time

The tick must replay: the same commands twice produce the same world. Two rules
keep that true, and both are load-bearing:

- **Randomness inside a tick comes from `self.rng`** — the world's seeded
  xorshift, advanced only by the tick. Never `rand::thread_rng()`, never the OS.
  A skill roll, a brain's drift, a generated name all draw from the one stream.
- **Timers are tick counts, never wall clocks.** Decay, swing timers, criminal
  flags, poison pulses, buff expiry — all `u64` ticks compared against
  `state.ticks`. Saved timed state stores the *remaining* span and re-derives
  the deadline from "now" on restore, so downtime pauses a timer rather than
  eating it.

A system that reads `Instant::now()` or a thread-local rng inside the tick has
broken replay silently; no test without a replay in it will catch it.

## Ports name their source

Numbers and behaviour taken from the reference emulators cite the function they
came from, so the next reader can check the port against the original:

```rust
// Read out of Sphere's `CItem::UnStackSplit` rather than guessed: the
// original keeps its serial and holds the taken amount on the cursor.
```

`Calc_GetSCurve`, `PacketItemWorld`, ServUO's `GetStartZ` — the name is the
provenance. Take the numbers; audit the arithmetic (see `CLAUDE.md` on Sphere's
walk check). A port nobody can trace back is a magic constant with extra steps.

## Names

Use the domain's words. `Serial`, `Mobile`, `Multi`, `Hue`, `Notoriety` are UO
terms with precise meanings — use them exactly, and do not invent synonyms.

Prefer explicit over clever. `spawn_with_serial` over `spawn2`.

## Commits

The subject line says what changed, in the imperative, colon-scoped when it
helps: `Fix dismount: strip the self-double-click high bit`. The body — when the
change needs one — says *why*, the same test a comment passes.

Commit messages carry the message text only: no attribution lines, no
tool signatures, no trailers naming who or what wrote the code.
