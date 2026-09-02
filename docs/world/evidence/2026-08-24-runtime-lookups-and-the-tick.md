# Runtime lookups and the tick

> A record. It was part of the roadmap's world phase until 2026-09-02; what is
> open in this domain is now ranked in [`world/README.md`](../README.md).

## Closed: `can_step` does not check the corner, and two obstruct tests are red

**Both are green**, and were closed by the corner-rule repair recorded in
[`navigation_spans.md`](2026-08-25-the-span-layer.md#out-of-scope-named) — *"`can_step`
has no corner rule, and the shard walked a creature with it"*. The two tests in
`state/src/obstruct.rs` were what found it: they had been asking `can_step` for a
rule that moved into `steps_out_of` in N3, and the answer taken was that
**`step_allowed` owns the corner** — which is the "same one for both callers" this
entry asked for. `a_diagonal_is_refused_when_either_flank_is_blocked` and
`a_live_terrain_with_no_map_reports_no_water` both ask it now.

Kept as a stub rather than deleted because the entry names the question — *which
layer owns the corner rule* — and the answer is a deliberate divergence from
ServUO, which keeps two rules and gives the lax one to creatures. That
divergence is recorded where it was taken, not here.

## ✅ A sector lookup was linear in a bucket, and a house made the bucket fat

`Sectors` (`state/src/sectors.rs`) was right where it was measured. Buckets are
64 tiles square, `located` maps an entity to **its bucket and its row in it**,
so insert, move and remove are all O(1) — the row half was already the lesson
learned from this exact case, and its own doc says so: "in a decorated town
that is thousands of entries, and finding an entity's own row in it by scanning
was paid on *every step by anyone*".

What was still linear was the read. `Sectors::nearby` walked **every entry** in
up to four buckets and filtered by Chebyshev distance. That is correct and was
cheap while a bucket held mobiles; a decorated house made it not cheap.
Housing's own caps say how not: `LOCKDOWNS_PER_TILE` is 4, so a castle's 992
tiles are worth about **4,000 locked-down items**, and at 64 tiles a side that
castle sits in one or two buckets. Every `nearby` touching it compared four
thousand rows — asked per NPC per tick by AI sight, and again by guards, pets,
chat, area spells, quest listeners, the broadcast audience, and, since "a mobile
is not an obstacle" closed, by `crowd_near` on **every step by anyone**. The
cost landed on the NPC that happened to share a sector with somebody's
decorated house, not on the house.

**Closed the way it says: a bucket is two lists, and the caller says which it
means.** `nearby` is gone as a name, which is the point — every call site had to
be revisited rather than keeping the old cost by inheritance.
[`mobiles_near`](../../../crates/server/state/src/sectors.rs), `items_near`,
`everything_near`, `mobiles_in_block`. Of the nineteen readers, **seventeen
wanted mobiles**, one wanted items (the crafting workshop scan) and one wanted
both (`refresh_around`, which fills a screen and so is about the furniture as
much as the people). **Six** of the seventeen also re-filtered by Chebyshev
distance after a lookup whose doc already promised exactness — chat, both
stealth sweeps, the bard's audience, a guard's call and the AI's sight; those
went with the rename.

The count this entry got wrong: it named `tick/fields.rs` as an item reader. A
field damages whoever *stands on it* and filtered its sweep by `Body` — it was a
mobile reader all along, and one of the several that had been paying for the
furniture twice, once to walk it and once to reject it.

**The kind is declared at the insert and never derived.** `Occupant::Mobile` /
`Occupant::Item`, named at each of the twenty-five places the shard puts
something on the grid, and seven more in tests. The alternative — reading `Body` off the registry inside the index — makes
the answer depend on whether the component went on before the index did, which
is a bug that only appears in whichever spawn path someone reorders later. The
cost of declaring it is the one thing no compiler catches, a caller naming the
wrong list, so `tick/tests.rs`'s
`the_shard_files_what_it_spawns_as_what_it_is` runs the real spawn paths — a
player entering, a creature spawned, an item and a container placed, a corpse
left by a death — and holds every row of both lists against the registry's
`Body`. Its controls: filing the corpse as a mobile fails it on the corpse
assertion; filing an entering player as an item fails **fifty** tests across
sight, chat, guards and the chase, which is the asymmetry to remember — a body
in the item list is invisible, an item in the mobile list is merely wasteful.

### Found while closing it

- ✅ **`FacetState::sectors` was public, and forty-five places across six crates
  wrote to it** — thirty-two inserts and thirteen removals. Its two neighbours in
  the same struct are private on an argument
  that applies here word for word: "every write here has to be followed by …, and
  a public field is a way to forget". The sector grid's forgettable half is
  `remove` — a despawn that misses it leaves a row pointing at an entity that no
  longer exists, which is the "ghost that never leaves" `despawn_mobile` already
  has a written-down order for, and nothing made anyone follow it.

  **Closed with the seam it asked for**, in the shape the field forced rather
  than the one this entry guessed. The field is private and read through
  `FacetState::sectors()`; the writes are
  [`WorldState::place_mobile`](../../../crates/server/state/src/runtime.rs) /
  `place_item` / `unplace(facet, entity)` — 19 · 12 · 12 of the forty-three
  mutation sites. **Two calls, not one call with an argument**: that is what
  makes `Occupant` named once per kind of thing rather than once per call site,
  and it keeps the previous entry's rule intact — the caller still *declares* the
  kind, now by which of the two it reaches for, so it still cannot go stale.
  Behaviour-preserving by construction, and the suite says so: 3,524 passed, the
  same count and the same five pre-existing clippy findings.

  The asymmetry from the entry above was re-run **against the seam itself**,
  which is the point of having one — flipping `place_mobile` to file items
  fails **64** tests across sight, chat, guards, the chase and death; flipping
  `place_item` to file mobiles fails exactly one, the dedicated guard. One place
  to break it now, and it is loud.

  It also took `britannia_with` in housing's tests off four public-field pokes: a
  facet's extent is `FacetState::new`'s argument, and writing `width`, `height`,
  a fresh `Sectors` and a fresh `Regions` over a built facet is four chances to
  put three sized-from-the-same-pair indexes out of step. Which leaves
  `FacetState::width` and `height` with **no writer anywhere in the shard** — two
  accessors and they are private, with no call site to revisit. `regions` is a
  public field whose writes already go through `Regions::set`/`clear`, so it
  leaks `&mut` rather than being the API; the weaker case, and the one left.
- **`WorldState::move_to` files its traveller as a mobile, and its callers make
  that true rather than its signature.** Every one of the six is a body — a gate,
  a recall, a `.go`, a ship relocating who is standing on it. An item put through
  it would land in the mobile list and be invisible to the crafting scan, which
  is the one reader of the item list. The doc says so, and it now says so by
  calling `place_mobile`; the signature is still the thing that does not.
- **`openshard_boats::aboard` sweeps a square around the ship's *first* covered
  tile.** The reach is the greatest Chebyshev distance from that tile to any
  other, so a galleon moored east-west sweeps a box as wide as it is long in both
  axes. It is mobiles-only now, which is most of the fix by accident; the shape
  is still wrong and the deck test would not notice.
- **One full-suite run reported a single failure with no name captured**, and
  three consecutive full runs since have been clean. Nothing to chase without the
  panic line — recorded so the next person who sees one knows it is not the
  first.

## The tick

`World::tick` is the deterministic half of the boundary the gateway's channel
draws. Commands queue from network tasks and are applied in a fixed order at a
fixed rate; nothing inside a tick awaits, reads a clock or touches a socket.

That is what makes anything that happens *without* a client asking possible at
all — decay, regeneration, an NPC deciding to move. It is also what makes replay
possible: the same commands produce the same world.

Two things worth knowing:

- **`select!` is `biased`** so the tick cannot be starved. Without it a flood of
  packets keeps `recv` ready forever and the world stops simulating under
  exactly the load that needs it most.
- **A late tick does not catch up.** `MissedTickBehavior::Delay`, because running
  several ticks back-to-back turns a hiccup into a stall and a fixed timestep
  into a variable one.

**What is still missing:** persistence. The world is built at start and lost
at stop.

Two players do now see each other. Verified over real TCP, on the real map:
each is drawn on the other's screen exactly once, steps arrive as `0x77`,
walking past 18 tiles sends `0x1D` and walking back re-draws, and a dropped
connection takes the mobile off every screen that had it.
