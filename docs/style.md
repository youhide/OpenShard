# Style

Beyond `cargo fmt` and `cargo clippy`, which are not negotiable.

`rustfmt.toml` sets `max_width = 110` and `style_edition = "2024"`. The rest of
the intended house style — import granularity, one import per line, grouped
`std`/external/local — needs nightly rustfmt and sits commented out in that file,
because `rust-toolchain.toml` pins stable and stable warns once per unstable key
and then ignores it. `cargo fmt --all` is expected to print nothing at all, and a
config that warns costs that.

## Newtypes, not raw values

`Serial`, `EntityId`, `Graphic`, `Hue`, `SoundId`, `AuthKey`, `AccountName` — an
id or an index gets its own type, so two domains cannot be mixed up at a call
site and the compiler is the one that says so. `fn move_to(&mut self, id: u32,
graphic: u32)` accepts its arguments in either order; the newtyped version does
not compile wrong.

The type is carried through the whole call tree. `.0` — or the named accessor,
where the wrapper has an invariant to protect — appears only where a value leaves
the domain: the packet codec, a SQL bind, a JSON field. A signature in
the middle that takes `u32` because unwrapping was convenient two frames up has
moved the boundary rather than removed it, and everything below that point is
back to integers that all look alike.

Prefer a real index type to `usize` for the same reason: `usize` is what every
other index in the process is too.

## A newtype is opened with `.0`, never through a trait

`From`, `Into` and `Deref` are banned on the project's newtypes. Not discouraged
— banned. Each of them hands back exactly the coercion the newtype was created to
remove, and hands it back invisibly: the type still looks enforced in every
signature, while at the call sites it behaves like the raw value again.

**`Deref` is the worst of the three**, because it is not a conversion you invoke.
`impl Deref for Serial { type Target = u32 }` makes every `u32` method, every
`&u32` parameter position and every autoderefing operator accept a `Serial` with
*nothing written at the call site at all*. The hole is spelled with the empty
string, so there is no text to grep for and no line for a reviewer to object to.
A newtype with a `Deref` is a comment that claims to be a type.

**`From<u32> for Serial` moves the hole to construction.** `let s: Serial =
n.into()` compiles whether `n` was a serial, a graphic, a container index or a
length off the wire. Worse, `.into()` is inference-driven: which conversion runs
is decided by the signature currently in scope, so changing a parameter type
silently retargets every `.into()` at every call site — no error, different
behaviour. Wrapping is the one moment at which somebody could have established
that this number is a serial. A blanket trait impl is how that moment is thrown
away.

**`From<Serial> for u32` is the same hole with better manners:** an unwrap that
does not say what it unwrapped. Two of them in sequence cancel, so a value laundered
`EntityId → u32 → Serial` typechecks the entire way, and the compile error the
newtypes existed to produce never happens.

So, always something written down at the site:

```rust
let raw = hue.0;                  // open it — visible, greppable, one meaning
let hue = Hue(raw);               // build it — names the type at the site
```

Where a newtype has an invariant, the field is private and both directions get a
name instead. `Serial` is the worked example: `Serial(u32)` with no public field,
`Serial::new(raw) -> Option<Self>` because a value outside both pools is not a
serial, and `serial.raw()` to put it back on the wire. That is the same rule, one
step stricter — the wrap *is* the check, so it cannot be spelled with a trait at
all without throwing the check away.

What is fine, because none of it is a coercion:

- `Debug`, `Display`, `Clone`, `Copy`, `PartialEq`/`Eq`/`Hash`/`Ord`.
- `PartialEq<str> for AccountName` — a comparison cannot produce either the
  wrapped type or the raw one, so it buys test-fixture ergonomics without opening
  anything. See `protocol::identity`.
- Inherent named constructors and accessors — `Serial::new`, `Serial::raw`,
  `AccountName::normalized`. The name is the whole difference: it says which
  direction, and which check ran, and it does not silently retarget when a
  signature moves.
- `From` between *error* types. That is what `?` is built on, and an error
  conversion cannot smuggle a domain value past a check — see
  `protocol::error`.

## A value off the wire is `Raw` until something checks it

The newtype rule above says a `u16` gets a name. For a client-supplied packet
field, a name is not enough: `Hue(create.skin_hue)` is exactly as named as
`RawHue(create.skin_hue)`, and only one of the two says whether the value has
been checked against anything. So every client → server field is wrapped in a
`Raw*` type — `RawHue`, `RawSkillId`, `RawCharacterSlot` — that can become the
real domain type only through a named promotion method, and a server → client
field carries the validated type directly: the server does not send itself
hostile input, and a `Raw` on an outbound packet would claim a check happened
where none is needed.

The promotion has exactly two names. `interpret(self) -> X` for a field where
every bit pattern means something, including "something odd" — total, no
`Result`. `validate(self, …) -> Result<X, InvalidX>` for a field with real
out-of-domain values, refused with a typed error. A field the client claims
and the server never reads gets the `Raw*` type and no second method at
all — that absence is the record of the decision, not an omission a reviewer
should fill in.

`docs/protocol_newtypes.md` is the full argument, the worked classification
(four shapes, called A–D, and which of the two method names each one gets),
and the running sweep applying it across `crates/common/protocol`. Read it
before adding a bare integer field to a packet struct; do not re-derive the
four classes from scratch, and do not restate them here — this section is a
pointer, and a second copy of the table is exactly the kind of stale summary
`CLAUDE.md` warns a documentation index against.

## `unwrap` where the invariant already holds

This codebase prefers `unwrap()` to a `?` or a `match` that cannot fail. That is
contract programming, and it is a readability argument first: an error path that
can never be taken still has to be read, and every reader has to work out for
themselves that it is dead.

It is a design argument second, and that one is the reason it is a rule. A
defensive `?` does not stay local. Making one function return `Result` makes its
callers return `Result`; a few rounds of that and half the engine is
`Result<_, Box<dyn Error>>`, with no way left to tell a failure that cannot
happen from one that happens on Tuesdays.

So when the invariant was established earlier — the entity was spawned three
lines up, the serial was resolved at the top of the function, the table was
filled at load — unwrap it:

```rust
// `spawn` just returned this id, so the row is there.
let mobile = self.registry.mobile(id).unwrap();
```

Use `expect` when the line does not already say what the invariant is, and a
comment when it takes a sentence. What must not happen is *stating* an invariant
the code does not have: `unwrap()` is a claim, and the claim gets checked at 3am
by a panic.

`Result` is not optional for anything originating outside the process: I/O, the
database, the client's own files, and everything off the wire. A packet is not an
invariant, it is an input, and a hostile one.

## Panics

Panic on programmer error — a broken invariant, a type mismatch that cannot
happen. Return `Result` for anything the outside world can cause.

Network input is never a panic. Ever. `ClientVersion::from_str` returns an error
because that string arrives in a packet from an untrusted client.

A panic drops the task it happened in rather than the process: `panic = "abort"`
is deliberately off, so one connection's failure is not the shard's — the
workspace manifest says as much next to the profile. That is what makes fail-fast
affordable here. It is not a licence to panic on player data.

## `Option` means absent, not unknown

`Option` is for absence that is part of the domain: a brain with no target, an
item in no container, an empty slot in a character list. It is not a way to say
"not computed yet" or "not loaded" — that files a missing value under a normal
one, and the bug surfaces somewhere else, later, as a `None` that nobody thought
was reachable.

A default is worse than `Option` for the same job. `0`, `""` and `Hue(0)` are all
plausible values, so a field that was never filled reads exactly like a field
that was filled deliberately. If a value is not yet known, that is a *state* —
model the state.

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

## Import from where it is declared

Avoid `pub use`. A re-export gives one type two paths, and then "who depends on
this?" has two answers — the crate map stops being readable from the imports,
which is the only place anybody ever reads it.
`use openshard_protocol::identity::AccountName` says where the type lives;
`use openshard_protocol::AccountName` says only that somebody was tidying.

Several `lib.rs` files are still a wall of `pub use` from before this was a rule.
Removing them is one mechanical sweep, planned as D8 in
[`protocol_rewrite.md`](protocol_rewrite.md) — do not drip-feed it, and do not add
to it in the meantime.

## Look for it before writing it

Search before adding a function, and extend what is already there. This is a
correctness argument rather than a tidiness one: the existing code has been run
and the new copy has not. Two implementations of one rule also means one of them
gets fixed and the other does not — and nothing tells you which one the bug
report came from.

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

Comment generously. Length is not the cost here — an invariant nobody wrote down
is. A precondition, a non-obvious property, the reason an order matters: above
the item, in as many lines as it takes.

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
`insert` then `get` works proves very little. Two traps that have already cost
this project real time are in [`findings.md`](findings.md) § Traps in tests and
benchmarks.

## No unsafe

Denied workspace-wide. If two mutable borrows into one structure are needed,
split a slice — see `Registry::for_each2_mut`. If a case looks genuinely
impossible without it, that is a design discussion, not a local decision.

## No fudge constants: a mismatch is fixed in the geometry

**When two representations of one thing disagree, the answer is to change one of
them, never to add a constant that hides the difference.** A number introduced
to close a gap, cover a seam, grow a shape past its own edge or nudge a value
off a boundary is forbidden, whatever it is called and however small it is.

This is a rule and not a preference because the failure mode is always the
same, and the renderer has paid for it three times:

- `SEAM_OVERLAP`, `0.15` of a `z` unit, grew every stair riser at both ends to
  cover a hairline along the tread/riser edge. **There was no hairline** — the
  two quads are built from the same arithmetic, so their corners are bit-
  identical and the rasteriser's fill rule already closes the edge. The
  constant cost 1120 pixels of a single flight drawn outside their own plane
  and displaced every step's corner by 2.4 px, in exchange for nothing.
- `WIDTH_OVERLAP`, `0.03` of a tile, grows every mesh face past its tile
  because the fitted prism is narrower than the art. It draws a two-pixel tooth
  around a flight at `4:1` — a 1355-pixel border, measured — and nobody ever
  measured the sliver it hides against the tooth it draws. In a scene with no
  sprite it buys nothing at all.
- `STAND_OFF` and `ON_TOP` started a shadow ray away from where the fragment
  really was, because the fragment's own position was not exactly known. They
  were **numbers off a byte layout**, and the engine was brighter than the
  geometry allows by up to half a channel until the position became data and
  both went to zero.

What the three have in common is the diagnosis: a fudge constant is a *second*
statement of a shape, in a unit that has nothing to do with the shape, tuned
against one picture. It cannot be right at another zoom, on another sprite, or
in a scene where the thing it compensates for is absent, and — worse — it makes
the real defect unmeasurable, because the instrument now shows the compensation
instead of the error.

So, in order:

1. **Make the two representations one.** Two shapes built from one expression
   cannot disagree; that is what retired `SEAM_OVERLAP` and what one silhouette
   per static does for `WIDTH_OVERLAP`.
2. **If they must stay two, measure the disagreement and carry it as data.** A
   number a caller can read and a test can bound — `impostor::Meeting::outside`
   is the shape of it: how far, in tiles, a fragment fell outside its own
   volume. A frame where it grows is a frame whose geometry is wrong, which is
   a thing a person can act on.
3. **Fix the geometry.** A prism that does not fit its art is a fitting
   problem; the answer lives in the fit, not in a border.

The one number this does *not* forbid is a rounding-scale tolerance whose size
comes from the arithmetic rather than from a picture — `RAY_TANGENT_TOLERANCE` —
and each of those has to state the measurement it was sized against, in its own
doc comment, next to the number.

**Nor a quantum, which is a different thing wearing the same shape.**
`impostor::FRAGMENT` is one step of the screen's sample grid expressed in tiles,
and it bounds a comparison for the reason a rounding tolerance never can: two
points closer together than one sample are not two points this renderer can
distinguish. It replaced a rounding tolerance that had been standing in for it —
`impostor::TANGENT`, `1e-4` of a tile, sized against a corner's rounding — and
the replacement was visible on the screen, because every pixel between the two
sizes had been answered "a point of nothing". A quantum has to name **which two
grids** it converts between (`docs/pixels.md`) and carry a control that fails
when it shrinks; a rounding tolerance has to name the arithmetic. Neither may be
sized by looking at a picture until it looks right.

## No globals

No `static mut`, no `lazy_static` singletons, no ambient state. Pass the
`Registry`. Pass the `EventBus`. This is what lets tests build worlds freely and
what will let the simulation shard across cores.

## Layering

A crate depends downward or sideways at the same level, never upward.
`entities` and `events` know nothing about gameplay and must stay that way —
if `entities` ever needs to know what a house is, the layering broke.

The crate tree says the same thing in the directory names: `crates/server` and
`crates/client` may both depend on `crates/common`, and never on each other.
A type that both sides of the wire must agree on belongs in
`crates/common/protocol` — putting it under `server` and reaching for it from a
client is the layering breaking in a new direction.

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

Where the stream *starts* is an input like any other, and is written down in two
places rather than compiled in. A fresh world takes `world.seed` from the config
when an operator pinned one (`World::with_seed`), and otherwise the engine's
`DEFAULT_SEED`. A world with a save behind it takes neither: the save carries
where the generator got to (`WorldRecord::rng_state`, `World::with_rng_state`) and
a restart resumes from there. The distinction is not tidiness — a shard that
re-seeded at boot would not roll *differently* after a restart, it would roll the
previous run's sequence again, in order, which turns "get the shard restarted"
into a way of asking for a roll a second time.

## Ports name their source

Numbers and behaviour taken from the reference emulators cite the function they
came from, so the next reader can check the port against the original:

```rust
// Read out of Sphere's `CItem::UnStackSplit` rather than guessed: the
// original keeps its serial and holds the taken amount on the cursor.
```

`Calc_GetSCurve`, `PacketItemWorld`, ServUO's `GetStartZ` — the name is the
provenance. Take the numbers; audit the arithmetic (see
[`findings.md`](findings.md) § Reading the reference emulators). A port nobody
can trace back is a magic constant with extra steps.

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
