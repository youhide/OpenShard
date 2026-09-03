# The fight loop, phase by phase

The implementation record of the six phases that closed the client's half of the
fight: war mode and the blow, the bar over the head, the swing seen, dying, the
corpse as a body, and looting. It was written as a plan and kept as the plan was
worked, so it reads forwards — what each phase was going to do, and then what
landed and where the code came out differently.

The decisions these phases were built against are
[`design_fight_loop.md`](../design_fight_loop.md); what is built and what is open
today is [`README.md`](../README.md). Several comments in the tree cite the phase
numbers `P1`–`P6` from this record.

## The phases

Six, in the order they are worth having. Each one is playable on its own: a
session that lands only phase 1 leaves the client strictly better, and no phase
depends on a later one's shape.

---

### P1 — the stance and the blow

**What a player sees:** pressing the paperdoll's war toggle makes the character
stand differently, and clicking a creature while at war starts a fight that
actually happens.

1. `StatusFlags::WARMODE = 0x40` in `crates/common/protocol/src/mobile.rs`,
   with the bit documented against ClassicUO's `EntityFlags` (`Frozen 0x01`,
   `Female 0x02`, `Poisoned 0x04`, `YellowBar 0x08`, `IgnoreMobiles 0x10`,
   `Movable 0x20`, `WarMode 0x40`, `Hidden 0x80` — write the whole table down
   once, set only the one bit).
2. Server: the three `StatusFlags::NONE` sites in
   `crates/server/state/src/runtime.rs` (and `enter.rs`, `death.rs`) read the
   mobile's `Combat::warmode` and set the bit.
3. `AttackRequest::encode` in `crates/common/protocol/src/combat.rs`, the shape
   `WarMode::encode` already has — a free function on the type, no
   `ClientVersion`, because the client has no version in hand at a click.
4. `crates/client/net/src/combat.rs`: `attack(target: Option<Serial>) -> Vec<u8>`
   beside `doll::war_mode`, and `Command::Attack` in `link.rs`.
5. `view::Mobile::war`, folded from the flag byte on `0x77` and `0x78` — D1.
6. `BodyKind::standing_at_war` / `walking_at_war` — D2 — and `crowd.rs` asking
   them: `Crowd::step`/`stand` take a `war: bool` for the body they are about.
7. `App`'s single-click arm: at war, over a mobile with a serial → `link.attack`,
   and the click does not fall through to selection. Dead sends nothing (D9's
   field does not exist yet, so this reads `false` until P4 — write the branch,
   not a `TODO`).

**Done when:** the toggle changes how the character stands, in both directions,
without a relog; a stranger who enters war mode changes how *they* stand on the
next `0x77`; a click on a rat while at war lands a swing (the shard's own
`swings` does the rest) and the same click at peace still selects the tile it
always did. Tests: the flag byte round-trips the bit in `protocol`; `BodyKind`'s
four numbers in `uofiles`; a `crowd` unit test that a body at war stands in 7
and walks in 15 and an animal does neither; an e2e in `crates/e2e/shard` that a
`0x05` from this client's encoder reaches `combat::attack` and comes back as a
`0xAA`.

#### Built, and where D1 and D2 came out differently

All seven items are in. Two changed shape once the code was in front of them,
and both changes are in the same direction — one fact, one place:

- **`view::Mobile::war` is a method, not a field.** D1 said "fold it into a new
  field"; the view already stores the whole flag byte, so a `bool` beside it
  would be the same state in two shapes, with the packet that forgot to refold
  one of them drawing a body in the wrong stance. `Player::war` stays a field
  because no `0x77` ever describes our own body — its answer comes from the
  `0x72` and the `0x88`, which is a different fact with the same name.
- **The stance rides `Crowd::see`/`snap`, not a setter of its own.** It arrives
  in the packet that carries the position, so it is folded in where the position
  is: one more argument, restated on every sighting, and `Tracked::war` beside
  `Tracked::body` for the same stated reason — *a walk that ends has to know
  what standing means*. There are four doors into a group change (first sight, a
  stance change with no step, a step, and a walk timing out) and the test names
  all four; the one that was easiest to forget is the third, which is a sword
  nobody sees drawn until the body takes a step.

What is **not** in P1, and was not planned to be: `0xAA` is still undecoded, so
nothing is highlighted as the target; the e2e in `crates/e2e/shard` is not
written, so what is gated is the round trip through the decoder in a unit test
rather than through a socket.

---

### P2 — the bar over the head, and the status window

**What a player sees:** how hurt everything is, and their own numbers.

1. `impl DecodePacket for HealthBar` (`0xA1`) and a `ServerPacket::decode` arm.
2. `view::Mobile::hits: Option<Vitals>` and `view::Player::hits` — `Option`
   because "the shard has not said" is a real state and a zero bar is a lie
   about a full-health mobile (`CLAUDE.local.md`'s `Option` rule, the honest
   direction of it).
3. `WorldView::apply` arm for `0xA1`, and one for `0x11` `MobileStatus` — the
   decoder has been there since the Status button was wired and *nothing reads
   it*, which is why the button opens nothing today.
4. `client/render`: the overhead bar. Two quads and a scissor, placed at
   `mobiles::head_anchor` minus the bar's height, hued by `Notoriety`
   (`Innocent` blue, `Criminal`/`Neutral` grey, `Enemy` orange, `Murderer` red)
   — the notoriety is already on `view::Mobile` and drawn nowhere.
5. The status window: `WindowSubject::Status`, `Drawn::Status`, frame `0x0802`,
   the numbers off `view::Player`. Same machinery as the skill window, opened by
   the Status button that already sends the `0x34`.

**Done when:** a wounded rat has a shorter bar than a fresh one and the bar
follows it as it walks; a bar appears the moment the shard says something about
a body and never before; the Status button opens a window with this character's
own hits, stamina, mana and stats in it; and `.set str 100` moves the number in
that window.

**The invariant to gate:** a stranger's bar and our own are drawn by the same
code from the same pair of numbers, and no branch anywhere asks whose it is
(D5).

---

### P3 — the blow, seen

**What a player sees:** the swing that is already happening.

1. `impl DecodePacket for Animation` (`0x6E`) and its `decode` arm.
2. `Crowd::play` — D7 — one-shot group, its own frame clock, cancelled by a
   step, falling back to the movement group when it ends.
3. `WorldView::apply` arm: `0x6E` is not view *state*, it is an event — so it
   does not belong in `WorldView` at all beyond being handed on. The seam is the
   one `link.rs` already has for packets the app acts on rather than stores.
4. `0x54` `PlaySound` decoding is M6's and is *not* in this phase; the note
   here is only that a swing without its sound is half the feedback, and that
   both hang off the same event.

**Done when:** an NPC that hits you visibly swings; a death plays the death
throe (which is the same packet, action 21/2 — D8) and the body is lying down by
the time the corpse arrives in P5; and a body that takes a step mid-swing walks
rather than sliding.

---

### P4 — dying

**What a player sees:** that they are dead.

1. `impl DecodePacket for DeathStatus` (`0x2C`), the arm, and
   `view::Player::dead`.
2. The tonemap uniform and its branch — D9. One switch, the whole frame.
3. The gates: no attack from a ghost (P1's branch), no war stance on the dead,
   and the paperdoll's toggle is the shard's answer as always.
4. The ghost body already draws — `anim::animation_body` remaps `0x0192`/`0x0193`
   to the living body two below, and `mobiles.rs` already refuses hair and a
   beard on the dead. Nothing to build; it wants a look, because nobody has seen
   it since it was written.

**Done when:** dying greys the world and resurrection un-greys it; a ghost
walks; a ghost's click sends no attack; and the paperdoll of a ghost is bald.

#### Built

All four items are in, and one detail changed shape once the code was in
front of it: the war-stance gate (item 3) is not only `Player::war`'s own
click-side branch — a stranger drawn as a ghost keeps no sword either, gated
off `anim::is_ghost(mobile.body)` rather than a field this client does not
have, since nothing on the wire says a stranger died but their body id
already does. `light::Lighting::dead` rides in `view`'s own padding, the same
word `shadow_rays` already claims — `blit.wesl`'s `dead()` reads it and
desaturates the shaded pixel to its Rec. 601 luma, after everything else the
pass computes, so a torch's pool is still brighter than the street and just
colourless. Item 4 needed no code, as written; it has now been looked at and
draws as described.

---

### P5 — the corpse as a body

**What a player sees:** a dead rat looks like a dead rat, lying the way it fell.

1. D10 in three commits — protocol, server, client — each with its own test,
   because a direction that only half exists is a corpse facing the wrong way
   with nothing to point at.
2. `client/render/src/corpses.rs`: the collector. It is `items::collect`'s list
   with `mobiles::place`'s pictures — the body from `amount`, the group from
   `BodyKind`, the direction from the item, the frame from the corpse's own
   clock (D11). Depth is the item's own tile, the way `items::collect` already
   sorts.
3. `AnimAtlas` has to be asked for those frames: `needed_animations` walks
   mobiles today and gains the corpses on screen. The key is the same
   `FrameKey`, so nothing about packing changes.
4. The pick: a corpse is picked as a *mobile* is (opaque texel of an anim
   frame), not as an item is (`StaticAtlas`) — otherwise the thing that is drawn
   and the thing that is clicked are two different rectangles. `items::pick`
   grows the same fork the collector has, or `corpses::pick` sits beside it;
   decide when the code is in front of you, and write down which and why.
5. Corpse equipment lands with looting: `0x89` carries the corpse serial and
   `(layer + 1, item serial)` pairs terminated by zero, while the `0x3C` that
   opens its container carries those items' graphics. The server captures the
   pair before stripping `Equipped`, persists it with the corpse, and sends it
   when the container opens. The client keeps the two packet facts separately
   and joins them whenever it rebuilds the corpse picture, so either arrival
   order draws the same layers.

**Done when:** a slain creature leaves a corpse of *its* body on the ground;
corpses face different ways; a corpse that comes into view falls and then lies
still; and clicking a corpse's picture picks the corpse and not the tile behind
it.

---

### P6 — looting

**What a player sees:** they can take things off the body.

1. Double-clicking a corpse already opens a container window — the shard sends
   the `0x24` with `CORPSE_GUMP` and this client draws containers. **Check it
   first**: it may already work, and if it does, this phase starts one item
   further along than it looks.
2. Lifting and dropping: `0x07` (pick up), `0x08` (drop), `0x13` (equip) — the
   client has never sent any of the three, and `0x27` (drag cancel) has no
   decoder. This is the first *drag* in this client, and it is a gesture with
   its own state: what is on the cursor, where it came from, and what happens
   when the shard says no.
3. The layer on a contained item — P5's item 5 — now arrives as `0x89`; once
   the `0x3C` listing has supplied its graphics, the corpse is redrawn dressed.

**Done when:** an item can be dragged out of a corpse into the backpack and back;
a refused drag puts the item back where it was rather than losing it; and a
corpse whose last item is taken still stands there until it rots (the shard's
rule, not the client's).

#### Built

All six phases are now in the client. The final wire seam is corpse equipment:
the layer relation is saved before worn gear moves into the corpse, survives a
restart, is sent as `0x89` with the opened corpse's `0x3C`, and is joined to the
container graphics on the client. A corpse therefore remains an animated body
from first sight and gains the equipment it was carrying as soon as its loot
listing is known; lifting an item removes it from that listing and thus from the
next rendered corpse frame.

## Backlog, found while planning this

- **`ServerPacket::decode`'s list is shorter than the encoder's, and this plan
  is eight more rows of that gap.** `client.md` already has the entry: a test
  that walks the send-side length table and *reports* the ids with no decoder
  would have made every row of the table at the top of this document visible
  years before somebody went looking. Worth writing while eight of them are
  being closed.
- **`StatusFlags` is one byte with eight meanings and this plan models one.**
  Poisoned (green bar), Hidden (translucent), Frozen, YellowBar and Flying all
  have pictures in the reference and none here. Writing the whole table in P1
  and setting one bit is the cheap half; the other seven are seven small
  features, each of which is "read a bit, change a hue".
- **The corpse's own name.** A `0x1A` carries no name and the shard names its
  corpses ("a corpse of a rat") — that string arrives only in answer to a single
  click (`0x09` → `0x1C`), which this client sends for nothing yet. Single-click
  naming is M5's third bullet and is a prerequisite for a corpse being
  identifiable before you open it.
- **Two clocks that are the same clock.** `Crowd` ages bodies at
  `CHARACTER_ANIMATION_DELAY` and P5 gives corpses their own copy of it. They
  are the same rate for the same reason, and a third caller (an effect, a door)
  would make it worth one type. Two is not enough to abstract.
- **A server packet cannot be round-tripped in a `protocol` unit test.**
  `decode_packet` asks the *client* length table whether to skip a variable
  packet's length word, which is the wrong table for a `0x78` — and
  `decode_server`, which asks the right one, is private to `server_packet`. So a
  test that wants to prove "what this engine writes is what a client reads" has
  to go through `ServerPacket::decode`, which is a dispatch as well as a
  decoder. Found writing the stance test in P1, which does exactly that and says
  so in a comment. Either `decode_server` becomes `pub(crate)`-plus-a-test-door,
  or `decode_packet` takes the table as a parameter; the second is the honest
  shape and the first is one line.
- **The war stance is the first thing that reads equipment for a *reason*.**
  D2's deferred armed variants and `MobileView.IsCovered` are still waiting on
  the wire graphic `crowd::worn` discards. Corpse layers no longer are: their
  wire relationship is `0x89`, which is now retained and rendered when the
  corpse container is opened.
