# 2026-08-23 — the other end of the wire gets the rule

The first of the three [the mobile obstacle
left](2026-08-23-a-body-is-in-the-way-of-the-plan-too.md), taken in the order
that entry gave: **the client plans through a crowd, which is the same
rubber-band from the other side.** It was right that the client had no
`crowd_near`. What it did not know is that the client was not walking through
bodies at all — it was walking around them by the *wrong rule*, and the packet
that would have told it otherwise had never carried the answer.

## Where it stands

### 🚩 The client already blocked on bodies, in a disguise that could not be fixed

`clutter::fill` laid every mobile in the view into the `Overlay` as a
`Cover::blocking` a body's height tall, under a comment that admitted what it
was: *"not a category the shared type names: nothing downstream reads a mobile
**as** one, so it goes in as furniture with a body's height."* So the entry's
"plans through a crowd" was false, and what was true is worse — the two ends
agreed by *resemblance*, which is the exact thing `clutter.rs`'s own module
header says it exists not to do.

Two divergences, and the second is why the disguise was never going to become
right by tuning:

| | |
|---|---|
| **Sixteen against fifteen** | A cover blocks over `[z, z + height)` and a mobile was given `PLAYER_HEIGHT`. The shard measures a body against another body with `MOBILE_OVERLAP`, which is deliberately one short of what it measures against a ceiling. At exactly the boundary this end refused a step the shard allows — a mezzanine walkable on the shard and not here |
| **A cover cannot name an exemption** | Staff walk through bodies and so do the dead. The overlay has no idea who is asking, so there was nowhere to put either rule. A ghost's walk home was refused by this end at every body it passed |

### `clutter::crowd` is the client's `crowd_near` ✅

The same bargain, from the end that has a view instead of a sector grid: filter,
sort by tile, hand back a `Vec` the caller owns for the length of one question.
It reaches every footing a **step** is decided against, through
`Footing::among` — the held arrow, the click-to-walk plan, and the route the HUD
draws, which is that same plan and not a second opinion about it
(`docs/parity.md`). `Readings::guide` keeps `Bodies::nobody`, because a
bystander must not be able to rewrite a corridor's topology; that is what
`Footing::guide`'s doc already said, and it now means something at both ends.

**Where the reach went.** `crowd_near` takes one because the sector grid holds a
whole facet. Here the *view is the reach*: this client has been shown the
mobiles near it and nothing else. The bound arrives from the shard rather than
being chosen, and it buys the same thing — a re-plan when the route reaches
somebody, never a wrong step.

### 🚩 And the bit did not reach the one client that needs it

`stance_of` fills the `0x77`/`0x78`, which is how a client learns about
**somebody else**. A client only ever predicts its **own** step. All three
senders of the `0x20` wrote `StatusFlags::NONE`, so a game master learned that
every *other* staff member walks through bodies and never that they do.

What let it survive is that the `0x78` a player is sent about *itself* does carry
the byte — so the flag is right from the moment of entering until the first step
or relocation sends a `0x20` over it. All three now read `stance_of`, and death's
own `0x20` is not a corner case: a ghost is exempt by the same rule, and its walk
home passes through the living, who cannot see it to move aside.

## What was decided

**A body is not clutter, and the fix was to take it out rather than to make the
cover honest.** A `Cover` with a fifteen-unit span would have matched the number
and still had nowhere to put "unless you are staff". The overlay says *what is
in the way*; only something that knows the mover can say *who is in the way of
whom*, and `Bodies` is that seam already.

**The client reads the exemption off the wire and derives nothing.** The bit
*is* `walks_through_bodies` — staff and the dead together, as the shard's own doc
insists — so this end does not re-decide it from `Player::dead` plus a guess at
staffness. One answer, sent.

**The shard's third clause has no counterpart here, and needs none.** A hidden
game master is in nobody's way; a hidden mobile is drawn only to a staff viewer
(`visible_to`), and a staff viewer's crowd is empty by the first rule. The case
cannot arise at this end, which is a *proof* rather than an omission — written
into `crowd`'s doc so the next person does not add a flag for it.

**`is_ghost` and not a wire fact,** because there is no wire fact: nothing tells a
client that a stranger died. The body id does, and it is the pair the drawing
already decides translucency by.

**Two diagnostics lost their bystanders on purpose.** `picking_query.rs`'s level
marker and `terrain_overlay` ask `can_fit`, whose own doc says "a body is not
what this places" — they were counting mobiles only because mobiles were
pretending to be furniture. A diagnostic about the ground no longer flickers as
people walk past.

## What is clean

`cargo test --workspace`: **3,520 passed, 0 failed**, 36 ignored (nine new — six
over `crowd`, two over a plan with somebody standing in it, and the `0x20`'s flag
byte at both ends of a life).
`cargo clippy --workspace --all-targets`: the same five findings in the same
three files, none this session's — `uofiles/src/map.rs`, `render/tests/traced.rs`
×3, `client/app/src/link.rs`. `cargo fmt --all --check` silent.

**The controls were run by hand.** With `stance_of` put back to
`StatusFlags::NONE` in `enter.rs` the game master's test fails and the ghost's
passes; in `death.rs`, the other way round — so neither is standing in for the
other. With the plan's crowd swapped for `Bodies::nobody` the route walks over
the bystander at (101, 100), which is the assertion that matters.

## What is next

The two the previous entry left, unchanged, plus what this one found.

| | what would close it |
|---|---|
| **`Sectors::nearby` is linear in a bucket**, and the shard's crowd is the second per-step reader on it. Nothing this session added a third — the client has no sectors | Unchanged: split a bucket into mobiles and items and let the caller say which it means |
| **The shove.** A player hard-blocked where UO would have let them past for 10 stamina | `Mobile.CheckShove`, four rules and two clilocs. Wants an owner for "may I walk into somebody" as a *gameplay* question |
| **Two bodies on a deck that moves under them** — still simply unexamined, and now at both ends | — |

And three this session made:

- **The client's crowd is built per ask; its clutter is built per view.** Two
  neighbouring functions reading the same list on two different clocks, and only
  one of them has a reason to. `Steering::steer` runs on every raw mouse-move. A
  screenful of points and a sort, so it does not matter yet.
- **A living mobile wearing a ghost graphic would be walked through**, because
  `is_ghost` is the whole of what this end knows about a stranger's death. Worth
  knowing before anybody writes a spectral NPC.
- **The `0x20`'s flag byte is now sent and half-ignored.** The client keeps
  `Player::war` out of it deliberately — `0x72` is the one home for the stance —
  so the byte arrives carrying a war bit read from nowhere. Honest, and the
  second place `WARMODE` travels.
