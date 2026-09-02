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

### 🚩 The ghost filter was one fact, and one fact cannot answer this

`crowd` filtered out anything wearing a ghost's graphic, because "nothing on the
wire says a stranger died". Both halves of that are true and the rule was still
wrong: a **living** mobile a shard gave a ghost's graphic to is blocked by the
shard and was walked straight through by this end. A spectral NPC that
rubber-bands, waiting for somebody to write one.

It is a conjunction now — the graphic **and** the wire's `IGNORE_MOBILES` — and
each half is load-bearing in its own direction:

| what is asked | what it gets wrong alone |
|---|---|
| the body id | a living spectral NPC is called dead, and walked into |
| `IGNORE_MOBILES` | it is `walks_through_bodies`, staff *or* dead — so a **visible living game master** is called out of the way, and walked into |

Exact, to within a game master who has taken a ghost's graphic while alive.

**This first went in as a deletion, and that was wrong.** The argument was that
a client holding a ghost is itself dead or staff — `can_see_mobile` draws a
ghost only to those two, and both carry `IGNORE_MOBILES`, so their crowd is
empty before any filter runs. It is true of *this shard*, and it is not true of
UO: ServUO's `CanSee(Mobile m)` ends
`((m.Alive || (Core.SE && Skills.SpiritSpeak.Value >= 100.0)) || !Alive ||
IsStaff() || m.Warmode)` (`Server/Mobile.cs:9229`). **A ghost in war mode is
visible to the living** — the manifest, which is how a player who died in the
woods is found and resurrected. The clause is missing here, and
`can_see_mobile`'s own doc quoted `CanSee` *without* it, so the proof was
checking itself against a transcription of the rule with the interesting term
already dropped.

The lesson is narrower than "no proofs": a client's step rule may rest on what
the shard *does*, and must not rest on a gameplay rule the shard has **not
implemented yet**. The missing clause is filed, with what it costs; the doc it
was mis-cited from is fixed; and the crowd now answers for a manifested ghost
before there is one.

`is_ghost` still stands alone where a body id is genuinely the fact — the
drawing's translucency, and the sword a ghost is not holding.

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

**Two facts agreeing, and not a proof that the case cannot arise.** Death is the
one thing about a stranger that no packet states, so this end reads it off the
picture and the flag byte together and refuses to call it from either alone.
What settled it against the proof is that the proof's premise is a rule this
shard has not written yet: the manifest is real UO, and a client rule that
becomes wrong the day a gameplay rule lands is a trap laid for whoever lands it.

**Not a `0xBF` of our own saying "this one is dead".** This engine does invent
subcommands (`AuthorityNotice`), so it was on the table — and it buys one bit
that the two facts already answer, at the price of a packet the reference
clients do not send and a second thing to keep in step with `Ghost`.

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
**663 passed, 0 failed**, five ignored — 392 + 109 + 148 and the doctests, with
five new tests among them.
`cargo clippy` on the same three: the two findings that were already there
(`uofiles/src/map.rs`, `client/app/src/link.rs`), neither this session's.
`cargo fmt --all -- --check`: silent on every file this session touched.

⚠ **`cargo test --workspace` did not run at any point this session**, and not
for a reason that is here: a parallel session is mid-flight on the shove —
`FacetRules` in `server/state`, and `movement`'s `Walk::Refused` gaining a
`Refusal` payload — so the server crates and `crates/e2e` spent the session in
various states of not compiling. Nothing here touches a server crate except one
doc comment, and no client crate depends on one; that is the dependency
invariant, and it is what kept any of this runnable.

**The controls were run by hand**, each failing exactly its own test and nothing
else. Five, because the ghost rule is a conjunction and each half had to be
shown to carry weight:

| the control | what fails |
|---|---|
| the graphic alone decides the dead | `a_living_body_in_a_ghost_s_skin_is_in_the_way`, alone |
| `IGNORE_MOBILES` alone decides the dead | `a_visible_game_master_is_in_the_way_bit_or_no_bit`, alone |
| neither is asked (the deletion this started as) | `a_ghost_the_shard_called_dead_is_in_nobody_s_way`, alone |
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

And five this session made, filed in
[the shove entry](2026-08-24-mobiles-and-the-shove-rule.md) under the entry this
closed. **The first two are gameplay this shard owes UO**, and both
came out of the ghost tail rather than out of the code:

- 🚩 **A ghost in war mode is visible to the living, and this shard has never
  said so.** `CanSee`'s `|| m.Warmode` — the manifest, and the precondition for
  a stranger finding a corpse-side player and resurrecting them; `Core.SE`'s
  Spirit Speak at 100 is the other way in. Not a predicate change: a war toggle
  on a ghost has to become a `reveal` and its reverse a `hide` for every living
  watcher in range, so the `seen` set moves with `warmode` the way it moves with
  `break_cover`.
- **A ghost walks through a shut door, and neither end knows.** ServUO's
  `ignoreDoors` is `!m.Alive` among four terms, with `BaseHouseDoor.CheckAccess`
  kept as the one exception. Both ends here read the doors as they stand, so
  they agree and nothing rubber-bands — it is a missing rule, one argument wide
  at each end, and the house door is the part that wants thought.
- **A stranger's `IGNORE_MOBILES` is not "out of the way"** — half of the
  conjunction and not a rule on its own, with a test at the client's end named
  after the mistake.
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
