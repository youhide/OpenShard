# The fight, and what it leaves behind

War mode, the blow, the bar, the death and the corpse — **one model, because they
are one loop**. A stance is what makes a click an attack; an attack is what
empties a bar; an empty bar is a death; a death leaves a body on the ground and
a ghost standing over it; and the body is a container somebody loots. Described
apart, each of the five would re-take the same four decisions about whose fact a
thing is and which end draws it.

The loop runs end to end. `openshard-combat` aims, swings, resolves damage,
records murder and kills; `world` lays the corpse and raises the ghost; the
client folds the answers and draws the stance, aim, health, actions, death and
corpse. **What is built and what is open is [`README.md`](README.md)**; how each
piece was built is
[`evidence/2026-08-11-the-fight-loop-phases.md`](evidence/2026-08-11-the-fight-loop-phases.md),
whose `P1`–`P6` several comments in the tree still cite by number. This document
is the decisions, and it carries neither.

> Read [`client/README.md`](../client/README.md) first for how a window, a packet arm and a
> renderer are put together here — every piece below is that shape again. This
> document does not restate the client's own window rules; it names which of
> them apply.

## The two ends, and what each owns

**The server owns the rules.** `crates/server/combat/src/lib.rs` has `attack`,
the three passes a blow or a shot runs through (`commit_actions`,
`sustain_actions`, `resolve_actions` — see
[`design_actions.md`](design_actions.md)), `damage`, `die`, `war_mode`, poison,
criminality and murder decay; `crates/server/world/src/tick/death.rs` has `lay_corpse`, the ghost, the
death shroud, the seven-minute rot and the loot hooks;
`crates/server/server/src/dispatch.rs:143` turns a `0x05` into
`Command::Attack`. A `.set` on hits or an NPC that fights back exercises the
whole of it today, visibly.

**The wire and the client complete the loop.** This table is the contract as
built, not a proposal:

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

**The client's own machinery does more than it looks.** Three pieces the loop
leans on: `mobiles::pick` answers which body is under the cursor (with the
serial, through `Crowd::drawn_now`'s `(Who, Mobile)` pairs); `mobiles::head_anchor`
answers where a body's head is on screen, at the right height for a rat and for
a dragon; and `AnimAtlas` packs any `(body, group, direction, frame)` that is
asked for. None of what follows needed a new renderer — it needed new *reasons*
to ask those three.

## Eleven decisions

Taken once so that six sessions do not take them six ways. Each is a decision
this model commits to, not a survey.

**D1. War is a fact about a *body*, and it lives in one field.** Our own stance
is `view::Player::war`, folded from the `0x72` and the `0x88`'s
`PaperdollFlags::WARMODE` (bit `0x01` of the paperdoll's own byte). Everyone
else's is bit **`0x40`** of the `0x77`/`0x78` flag byte — `StatusFlags`, whose
bits this engine had never modelled and never set. Both ends changed: the server
sets the bit in the three places `runtime.rs` writes `StatusFlags::NONE`, and
the client folds it into `view::Mobile::war`. **Not** two questions with
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
`MobileView.IsCovered` wants (the paperdoll's entry in
[`client/evidence/2026-08-30-the-client-backlog.md`](../client/evidence/2026-08-30-the-client-backlog.md)).
One field on `EquipmentLayer`, two features. Until it lands, a body at war stands
one-handed, which is what an unarmed body should look like anyway.

**D3. An attack is a *single* left click while at war, and the client aims
nothing.** ClassicUO's `GameScene`: in war mode a left click on a mobile is
`GameActions.Attack`, not a selection. So `App`'s single-click arm has one branch
before it sets `self.selected`, and `use_under_cursor` — the double click — is
untouched. Nothing is done locally: no local target, no local highlight, no
"swinging" state. The shard answers with a `0xAA`, and what is drawn follows
*that*. This is the paperdoll rule (`doll_clicked`: "every one of these is a
request and nothing else") applied to the world.

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
all). This engine builds **the lines**: they are what a player means by
"полоски над мобами", they need no window machinery, and `head_anchor` already
puts them in the right place. The **status window** (`0x11`, frame `0x0802`) is
the other half and is *our own* character's, opened by the Status button that
sends the `0x34`. A per-stranger bar *window* is neither, and is deferred with
its opening gesture unresolved — the reference opens one by dragging a
name-plate, a gesture this client has neither half of.

**D7. A server-sent animation is a one-shot group with its own clock.** `Crowd`
holds a group and a clock per body and swaps groups on movement; a `0x6E` says
"play group *g* for *n* frames, once" and the body goes back to whatever
movement implies when it is done. So `Crowd` has `play(who, group, frames,
delay)` and a `Tracked::one_shot: Option<OneShot>`.

> **Amended by [`design_actions.md`](design_actions.md)'s D6, and half of this
> was never built.** The one-shot machinery is right and stands. The
> cancellation rule — *a step cancels it* — was in the wrong process: a client
> that cancels on its own is guessing at a fact only the server has, and it
> never actually did — `Crowd` clamps an overlapping action to the displayed
> group's frames rather than dropping it. What cancels a stroke is the shard
> saying so, `CombatActionEnded` (`0xBF 0xE011`), and *when* a step spoils a
> blow is the operator's condition table.

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

**D10. A corpse's direction has to be on the wire, and the field was already
there.** `0x1A`'s x has a top bit meaning "a direction/light byte follows", and
our encoder never set it while our decoder *refused* the packet outright when
it saw it (`items.rs`, `DecodeError::Unsupported`). A corpse is the first thing
in this engine that needs it: without a direction every corpse in the world lies
facing the same way. So:

- Server: `spawn_corpse` puts a `Heading(Facing)` on the corpse entity — the
  dead body's own facing, read before it leaves — and `WorldState::world_item`
  writes the byte when the component is there.
- Protocol: `WorldItem::direction: Option<Facing>`, encoded behind the x bit,
  decoded instead of refused. `None` for everything that is not a corpse, and
  that absence is *semantic*: an ordinary item has no facing, which is exactly
  the case the `Option` rule is for.
- Client: `view::Item::direction: Option<Facing>` carried through unchanged.

**D11. A corpse is drawn out of `anim.mul`, and item `0x2006`'s own art is never
shown.** The body is the item's `WorldItemPayload::CorpseBody`, the
group is the death group for that body's `BodyKind` — Human `21`, Animal `8`,
Monster `2`, all `Die1` — the direction is D10's, and the frame is **the
animation played once and then held on its last frame**: `Item.ProcessAnimation`
in the reference advances one frame per `CHARACTER_ANIMATION_DELAY` and clamps
at the end. So a corpse that comes into view *falls*, and then lies still. That
is a per-corpse clock, which is `Crowd`'s job for bodies and takes the same shape
in `client/render/src/corpses.rs`.
