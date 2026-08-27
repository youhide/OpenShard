# The fight, and what it leaves behind

War mode, the blow, the bar, the death and the corpse — **one plan, because they
are one loop**. A stance is what makes a click an attack; an attack is what
empties a bar; an empty bar is a death; a death leaves a body on the ground and
a ghost standing over it; and the body is a container somebody loots. Six
milestones written apart would each re-take the same four decisions about whose
fact a thing is and which end draws it.

The loop now runs end to end. `openshard-combat` aims, swings, resolves damage,
records murder and kills; `world` lays the corpse and raises the ghost; the
client folds the answers and draws the stance, aim, health, actions, death and
corpse. The phases below are retained as the implementation record, with the
remaining adjacent work called out explicitly rather than leaving the opening
table to describe an earlier revision.

> Read [`client.md`](client.md) first for how a window, a packet arm and a
> renderer are put together here — every phase below is that shape again. This
> document does not restate M4's rules; it names which of them apply.

## What is already there, and what is missing

**The server owns the rules.** `crates/server/combat/src/lib.rs` has `attack`,
`swings`, `volleys`, `damage`, `die`, `war_mode`, poison, criminality and murder
decay; `crates/server/world/src/tick/death.rs` has `lay_corpse`, the ghost, the
death shroud, the seven-minute rot and the loot hooks;
`crates/server/server/src/dispatch.rs:143` turns a `0x05` into
`Command::Attack`. A `.set` on hits or an NPC that fights back exercises the
whole of it today, visibly.

**The wire and client now complete the loop.** The former gap was closed in
P1–P6; this table is the current contract, not a proposal:

| packet | direction | server writes | client decodes | client draws |
|---|---|---|---|---|
| `0x05` attack request | → server | n/a (decodes) | client encodes | starts/stops aim |
| `0x72` war mode | both | yes | yes | war toggle and stance |
| `0xAA` attack target | → client | yes | yes | target ring |
| `0xA1` health bar | → client | yes | yes | overhead bar and damage feedback |
| `0x11` mobile status | → client | yes | yes | own status window |
| `0x6E` / `0xE2` animation | → client | yes | yes | one-shot swing/death action |
| `0x2C` death status | → client | yes | yes | dead-state greyscale and interaction gates |
| `0x54` sound | → client | yes | yes | audio playback |
| `0x1A` world item | → client | yes, with corpse direction | yes | corpse body animation |
| `0x07`/`0x08`/`0x13` and `0x27` | both | yes | yes | lift, drop, equip and rollback |
| `0x89` corpse equipment | → client | yes, when a corpse opens | yes | corpse worn layers once its contents arrive |

**The client's own machinery is further along than it looks.** Three pieces this
plan leans on exist and are tested: `mobiles::pick` answers which body is under
the cursor (with the serial, through `Crowd::drawn_now`'s `(Who, Mobile)`
pairs); `mobiles::head_anchor` answers where a body's head is on screen, at the
right height for a rat and for a dragon; and `AnimAtlas` packs any
`(body, group, direction, frame)` that is asked for. Nothing below needs a new
renderer — it needs new *reasons* to ask those three.

## Ten decisions, taken here

Taken once so that six sessions do not take them six ways. Each is a decision
this plan is committing to, not a survey.

**D1. War is a fact about a *body*, and it lives in one field.** Our own stance
is `view::Player::war`, folded from the `0x72` and the `0x88`'s
`PaperdollFlags::WARMODE` (bit `0x01` of the paperdoll's own byte). Everyone
else's is bit **`0x40`** of the `0x77`/`0x78` flag byte — `StatusFlags`, whose
bits this engine has never modelled and never sets. Both ends change: the server
sets the bit in the three places `runtime.rs` writes `StatusFlags::NONE`, and
the client folds it into a new `view::Mobile::war`. **Not** two questions with
two answers: `App` asks "is this body at war" of the view and gets one `bool`,
whoever the body is.

**D2. The stance is an animation *group*, not a second animation state.** The
crowd already picks a group per body per frame (`BodyKind::standing()`,
`walking()`, `running()`); war changes *which* group, and nothing else. So the
new knowledge goes in `openshard_uofiles::anim::BodyKind` beside the three that
are there, in the same shape and with the same warning attached — the three
numberings are three enumerations and a number means three different actions:

- `standing_at_war() -> Option<u8>`: `Some(7)` for `Human`
  (`PeopleAnimationGroup.StandOnehandedAttack`), `None` for `Monster` and
  `Animal` — the MUL path has no separate war stand for either, and the
  reference draws them standing exactly as they stand at peace.
- `walking_at_war() -> Option<u8>`: `Some(15)` for `Human`
  (`PeopleAnimationGroup.WalkWarmode`), `None` for the other two.
- `None` means "use the peacetime group", which is what makes the call site one
  `unwrap_or(kind.standing())` rather than a second `match` over `BodyKind`.

**Deferred on purpose, and it is one missing field:** the *armed* variants —
`8` (`StandTwohandedAttack`), `1`/`3` (`WalkArmed`/`RunArmed`) — need to know
what is in the hands, and `crowd::worn` resolves every worn item to its `AnimID`
and throws the wire graphic away. That is the same field
`MobileView.IsCovered` wants (`client.md`, the paperdoll's backlog). One field
on `EquipmentLayer`, two features. Until it lands, a body at war stands
one-handed, which is what an unarmed body should look like anyway.

**D3. An attack is a *single* left click while at war, and the client aims
nothing.** ClassicUO's `GameScene`: in war mode a left click on a mobile is
`GameActions.Attack`, not a selection. So `App`'s existing single-click arm
gains one branch before it sets `self.selected`, and `use_under_cursor` — the
double click — is untouched. Nothing is done locally: no local target, no local
highlight, no "swinging" state. The shard answers with a `0xAA`, and what is
drawn follows *that*. This is M4's paperdoll rule (`doll_clicked`: "every one of
these is a request and nothing else") applied to the world.

**D4. `0xAA` is the aim, and the aim is not the highlight.**
`view::Player::attacking: Option<Serial>` is a *third* thing beside
`App::on_mobile` (what the cursor is over) and `App::selected` (what the Tile
panel is showing). Three fields because they are three facts and a client that
merged any two would show the wrong body ringed the moment the cursor moved.

**D5. A stranger's health is a percentage, and the client must never care.**
`HealthBar::scaled` sends `max = 100` and `current` as a percent; `exact` sends
the real pair to the mobile's own client. The view stores the pair it was given
and the bar draws `current / max`. **No arm anywhere asks which kind arrived** —
that is the whole point of the server having made the choice, and a client that
special-cased "is this mine" would have two ways to be wrong about one bar.

**D6. Two health pictures, and they are two different things.** ClassicUO has
`HealthBarGump` (a draggable window per mobile, art `0x0803` at peace, `0x0807`
at war, `0x0804` for somebody else, lines `0x0805` red and `0x0806` blue) *and*
`HealthLinesManager` (thin bars drawn over heads in the world, no window at
all). This plan builds **the lines first** — they are what a player means by
"полоски над мобами", they need no window machinery, and `head_anchor` already
puts them in the right place. The **status window** (`0x11`, frame `0x0802`) is
the other half and is *our own* character's; it is the window `client.md`'s
backlog already names as the next one after the skill list, and the Status
button already sends the `0x34` that fetches it. A per-stranger bar *window* is
neither, and is deferred with its opening gesture unresolved — see the backlog.

**D7. A server-sent animation is a one-shot group with its own clock.** `Crowd`
holds a group and a clock per body and swaps groups on movement; a `0x6E` says
"play group *g* for *n* frames, once" and the body goes back to whatever
movement implies when it is done. So `Crowd` gains `play(who, group, frames,
delay)` and a `Tracked::one_shot: Option<OneShot>`, and **a step cancels it** —
the reference's own rule, and the one that keeps a body from moonwalking through
its own swing.

> **Amended by `combat_actions.md`'s D6, and half of this was never built.** The
> one-shot machinery is right and stands. The cancellation rule was in the wrong
> process: a client that cancels on its own is guessing at a fact only the server
> has, and it never actually did — `Crowd` clamps an overlapping action to the
> displayed group's frames rather than dropping it. What cancels a stroke now is
> the shard saying so, `CombatActionEnded` (`0xBF 0xE011`), and *when* a step
> spoils a blow becomes the operator's condition table in that plan's Ф3.

**D8. `0x6E` now, `0xE2` when a client asks for it.** The server picks between
them by the connection's features (`Feature::NewMobileAnimation`); `0x6E`'s
action number is body-specific and `0xE2`'s is a body-agnostic category, which
is exactly why `feedback.rs` refuses to give them a shared newtype. This client
decodes `0x6E` because that is what our shard sends it today, and the arm for
`0xE2` is written down as a gap rather than guessed at. The numbers line up
already: `Action::Die` goes out as `(21, 6)` for a humanoid and `(2, 4)` for a
creature, which are `PeopleAnimationGroup.Die1` and `HighAnimationGroup.Die1` —
the same two numbers D11 needs for the corpse.

**D9. Death greys the world in the tonemap, not in every quad.** `0x2C` is a
*screen* state — ClassicUO's `DEAD_RANGE_COLOR` / `EnableBlackWhiteEffect` — and
this renderer has a tonemap pass with uniforms. One uniform, one branch, and the
whole frame goes grey; a per-sprite hue would be the same fact stated at ten
thousand call sites and would still miss the ground. `view::Player::dead: bool`
is where the packet lands, and it gates more than colour: no attack goes out
from a ghost (D3's branch asks it), and the reference draws no war stance on the
dead either (`!mobile.InWarMode || mobile.IsDead` in every group arm).

**D10. A corpse's direction has to be on the wire, and the field is already
there.** `0x1A`'s x has a top bit meaning "a direction/light byte follows", and
our encoder never sets it while our decoder *refuses* the packet outright when
it sees it (`items.rs`, `DecodeError::Unsupported`). A corpse is the first thing
in this engine that needs it: without a direction every corpse in the world lies
facing the same way. So:

- Server: `spawn_corpse` puts a `Heading(Facing)` on the corpse entity — the
  dead body's own facing, read before it leaves — and `WorldState::world_item`
  writes the byte when the component is there.
- Protocol: `WorldItem::direction: Option<Facing>`, encoded behind the x bit,
  decoded instead of refused. `None` for everything that is not a corpse, and
  that absence is *semantic*: an ordinary item has no facing, which is exactly
  the case `CLAUDE.local.md`'s `Option` rule is for.
- Client: `view::Item::direction: Option<Facing>` carried through unchanged.

**D11. A corpse is drawn out of `anim.mul`, and item `0x2006`'s own art is never
shown.** The body is the item's `WorldItemPayload::CorpseBody`, the
group is the death group for that body's `BodyKind` — Human `21`, Animal `8`,
Monster `2`, all `Die1` — the direction is D10's, and the frame is **the
animation played once and then held on its last frame**: `Item.ProcessAnimation`
in the reference advances one frame per `CHARACTER_ANIMATION_DELAY` and clamps
at the end. So a corpse that comes into view *falls*, and then lies still. That
is a per-corpse clock, which is `Crowd`'s job for bodies and wants the same
shape here — see phase 5 for where it lives.

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

## What this plan does not cover

- **Sound** — M6, and it lands across all six phases at once when it lands.
- **The party, the buff bar, damage numbers over heads** and the spell effects
  `0x70`/`0xC0`. The damage number is ServUO's `DamagePacket` (`0x0B`, serial
  and a `u16`) with `DamagePacketOld` (`0xBF` subcommand `0x22`) behind it for
  old clients — **neither exists in `openshard_protocol`**, so this one is a
  packet to add on both ends rather than a decoder to write. None of them is on
  the loop this plan is about.
- **A per-stranger health bar *window*** — D6's third picture. Its gesture is
  unresolved: the reference opens one by dragging a name-plate, which is a
  gesture this client has neither half of.

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
