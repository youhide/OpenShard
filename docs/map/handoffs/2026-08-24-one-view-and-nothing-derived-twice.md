# 2026-08-24 — one view, and nothing derived twice

The three [the client's half
left](2026-08-23-the-other-end-of-the-wire-gets-the-rule.md), taken together
because they turned out to be one sentence said three ways: **the client held
the same fact twice and derived it a third time.** A crowd built on a clock of
its own beside the furniture it belongs with, a stranger's death read off a
picture, and a stance in two fields where one of them is never updated.

None of the three needed a new fact from the wire. Two of them needed a fact
already on it, and the third needed a proof.

## Where it stands

### The crowd is a projection now, and the two halves are one call ✅

`clutter::fill` was a projection of the view; `clutter::crowd` was rebuilt at
every question, and `Steering::steer` runs on every raw mouse-move while the
right button is held. What replaces both is `clutter::project`, which writes the
furniture into the facet's live layer **and** the bodies into
`WorldState::bodies` in one statement — [`Ground::set_base`]'s trick, for
[`Ground::set_base`]'s reason: a view change that refreshes one half and not the
other is a step decided against two different moments, and the way to make that
unspellable is to leave nowhere to write one half alone.

The four readers — the held arrow, the walking clock, the mouse's heading, and
the HUD's route — read the field.

**The two ends of the wire still build it at different moments, and the reason
is in the arguments.** `crowd_near` takes a mover, a centre and a reach: its
answer is different for every asker, so it cannot be anything but built at the
question. The client's takes nothing — the mover is always this connection's own
body, and the reach is whatever the shard has shown it — so it is a function of
the view and is projected with the rest of the view. `Bodies`' own doc used to
say "built at the question, and never kept" as if that were one property of both
ends; it says the property it actually meant now, which is *never an index*:
built whole, thrown away whole, nothing to keep in step by hand.

[`Ground::set_base`]: ../../../crates/common/movement/src/ground.rs

### 🚩 The ghost filter could never fire, and fired only on the wrong body

`crowd` filtered out anything wearing a ghost's graphic, because "nothing on the
wire says a stranger died". Both halves of that are true and the conclusion was
still wrong, because of a fact neither end had written down:

**A client that can see a ghost is a client whose crowd is already empty.** The
shard draws a ghost only to another ghost and to staff — `can_see_mobile`, and
`show` is the one path a mobile reaches a screen by, so a living player never
has one in `view.mobiles` at all. Both of those viewers carry `IGNORE_MOBILES`
on their own body, which is the first thing `crowd` reads and the point at which
it returns nothing. So the filter had no reachable case.

What it *did* have was the unreachable case's shadow: a **living** mobile a
shard gave a ghost's graphic to. The shard blocks on that body, this end walked
straight through it, and the report would have been a spectral NPC that
rubber-bands. The filter is gone and the proof is in its place, in the same
shape as the hidden-game-master clause beside it.

`is_ghost` stays where a body id is genuinely the fact — the drawing's
translucency, and the sword a ghost is not holding. A step is decided against
what the shard says, and the shard says nothing about a stranger's death.

### The player's flag byte is one bit now, and the stance has one home ✅

`Player::flags: StatusFlags` became `Player::walks_through_bodies: bool`. Of the
eight bits, one is answered from at this end — `IGNORE_MOBILES`, which is the
shard telling the client that its own body-blocking rule does not apply to this
mover — and it is folded at the door by the two packets that carry the byte
about our own body (`0x20`, and the `0x78` a client is sent about itself).

The bit that made this worth doing is `WARMODE`. `Player::war` is the stance's
one home and is written by `0x72` and by our own `0x88`; the byte's war bit is
written by neither, and a `0x72` arrives with no `0x20` behind it. So the copy
in `flags` was not merely unread — it was **wrong for as long as the body stood
still**, sitting in a field whose name is the first place a reader would look.

The reference client makes the same split: ClassicUO's `Mobile.InWarMode` reads
`Flags & WarMode`, and `PlayerMobile` overrides it with a field of its own that
only the `0x72` handler writes, while `UpdatePlayer` stores the byte. So this is
the shard's `0x20` staying right for a stock client and this client keeping the
answer it can trust — not a divergence.

`Mobile` keeps the whole byte, and the asymmetry is the point: there the byte
*is* the stance's one home, because no `0x72` ever describes somebody else.

## What was decided

**A proof, not a second filter.** The obvious repair for the spectral NPC was to
require `is_ghost` *and* the wire's `IGNORE_MOBILES` before letting a body
through. It is wrong, and it is filed in the roadmap so nobody tries it: a
stranger's `IGNORE_MOBILES` is `walks_through_bodies` — staff **or** dead —
while the crowd wants `body_blocks`, which a living, visible game master
satisfies. Reading the bit that way would walk this end into a game master
standing in a doorway and be refused a hold at a time. The shard keeps the two
questions in two functions for exactly this reason.

**The byte went, rather than the war bit being masked out of it.** Masking would
have left a `StatusFlags` that lies about its own name. Keeping the byte "for
the day somebody wants `hidden`" is how a fact ends up with two homes; the
protocol's own table already says a bit nobody sets is a bit nobody has tested,
and the same rule applies one layer up. When one of the other seven is wanted it
gets folded out the way this one is.

**A projection, and not a cache.** `WorldState::bodies` is the third projection
of the view, beside the presentation's pictures and the live layer's furniture,
and it is written in the one place the view is folded. It is not an index: there
is no incremental edit and no removal anybody can forget, which is the property
`Bodies`' doc was defending all along.

## What is clean

`cargo test -p openshard-client-app -p openshard-client-net -p openshard-movement`:
**648 passed, 0 failed**, five ignored — 390 + 109 + 148 and the doctests, with
three new tests among them, one per tail.
`cargo clippy` on the same three: the two findings that were already there
(`uofiles/src/map.rs`, `client/app/src/link.rs`), neither this session's.
`cargo fmt --all -- --check`: silent on every file this session touched.

⚠ **The workspace does not build, and not because of this** — say when the
numbers above were taken, because they were taken *before* the second half of
it. A parallel session is mid-flight on the shove: `FacetRules` in
`server/state` (so `cargo test --workspace` and `crates/e2e` cannot run at all),
and, landing later, `movement`'s `Walk::Refused` gaining a `Refusal` payload —
which its own crate's tests and `client/app`'s `dst.rs` have not caught up with
yet. So the client-app figure is this tree's, one minute before that reached it;
`client/net` still runs clean on its own (109 + 1) because it does not depend on
`movement`. Nothing here touches a server crate, and no client crate depends on
one — that is the dependency invariant, and it is what kept any of this
runnable.

**The controls were run by hand**, one per tail, each failing exactly its own
test and nothing else:

| the control | what fails |
|---|---|
| the `is_ghost` filter put back | `a_living_body_in_a_ghost_s_skin_is_in_the_way`, alone |
| `war` folded out of the `0x20`'s byte | `a_player_update_carries_the_body_blocking_exemption_and_not_the_stance`, alone |
| `project` writing the furniture and not the bodies | `one_call_replaces_the_furniture_and_the_crowd_together`, alone |

## What is next

The two the previous entries left, unchanged, and the four the last one filed
are still filed — this session closed only the client's three.

| | what would close it |
|---|---|
| **The shove.** A player hard-blocked where UO would have let them past for 10 stamina | `Mobile.CheckShove`, four rules and two clilocs — and still blocked first on this engine having no facet rulesets, which is what the parallel session's `FacetRules` is |
| **Two bodies on a deck that moves under them** — still unexamined at both ends | — |
| The four `a bucket is two lists` filed | [that entry](2026-08-24-a-bucket-is-two-lists.md) |

And three this session made, filed in [`roadmap.md`](../../roadmap.md) under the
entry this closed:

- 🚩 **A stranger's `IGNORE_MOBILES` is not "out of the way"** — the wrong repair
  for the ghost filter, written down before somebody makes it.
- **Nothing tests that `App::entered` calls `project`.** The seam has a test and
  the readers have theirs; the line between them has none, because an `App`
  wants a window and a GPU. It is one line wide and it is the line that would
  make every step decision a packet stale.
- **A lost shard puts the world out of the view and leaves every projection
  standing.** `shard_lost` clears the view's tables and nothing reprojects, so
  the live overlay, the draw lists and now `bodies` all keep the dead shard's
  world. Nothing walks over it — but `shard_lost` exists so that a picture which
  goes on looking right cannot outlive the connection, and the picture is
  exactly what does.
