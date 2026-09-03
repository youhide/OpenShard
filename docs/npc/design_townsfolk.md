# A townsperson

The model behind `crates/server/npc`: what makes the people in a town people
rather than props, and what a shard owes each of them. Four questions, one module
each — what it looks like, who it is, what it does with a beat, and what it
answers — and all four are keyed off one thing, the `Title` it was spawned with.

The step a townsperson takes is decided by [`design_brain.md`](design_brain.md);
what it *says* and who hears it is [`design_speech.md`](design_speech.md).

## Why this is a crate

`world/tick.rs` is orchestration — command dispatch, system order, the movement
machinery. Rules do not go there. So townsfolk behaviour is a `fn(&mut
WorldState)` here, the shape `combat`, `chat` and `skills` use, and the
components it hangs on (`Npc`, `Banker`, `Title`) live in `state`, below both.

## The base: a post and a leash

`Npc { home, wander }` is the whole of what a townsperson is structurally: a tile
it belongs to and how far it may drift from it. A banker, a shopkeeper, a
quest-giver and a wandering healer are all that base plus a `Title` plus,
sometimes, a worn crate.

**A spawn stands on the floor.** A placement drops onto the map's surface at its
tile through `Terrain::stand_z` — a building's raised floor and all — rather than
sinking to whatever z the data named, which reads as an NPC inside a wall.

## The beat

`live` runs on `BEAT_TICKS` — two seconds, written as `2 * TICKS_PER_SECOND`
because it is a span of real time. It does everything it can directly on the
world (greet, turn to face, bark) and returns the one thing it cannot: the step
it wants, because stepping is bound to the terrain and the walk machinery the
tick owns. Same decide-then-apply seam the creature brain has.

**A random heading does not make an NPC walk.** The motion path implements
turn-as-step: a step in a direction you are not already facing only *turns* you.
An idle NPC that picks a fresh random heading every beat therefore spends seven
beats in eight pirouetting and one moving, which reads exactly like standing
still. The fix is the reference's own — `BaseAI.WalkRandomInHome(2, 2, 1)`: one
chance in two of not moving, one in two of a new heading, so most beats continue
on the current one and the step *translates*.

**A shopkeeper serving a customer stands still.** `VendorAI.DoActionInteract`
turns the vendor to face whoever it is dealing with and takes no step, without
which the shopkeeper wanders off mid-transaction.

**Beats are jittered, and the jitter is a fraction of the interval.** Sphere
re-rolls an idle NPC's timer at the end of every beat; jittering only the first
would set the offsets once and then defend them, so anything that puts two
townsfolk on the same tick — a restore, a shared doze — welds them together for
the life of the shard. `next_beat` is the one place a beat is armed, so the
jitter cannot be forgotten at one of them (it had been, at four). The spread is
`BEAT_JITTER_FRACTION` — a quarter of the interval — rather than a fixed number
of ticks, because the same helper arms a townsperson's two seconds, a creature's
four hundred milliseconds and a dozing mobile's sixteen: a flat spread wide
enough to matter to the first would make `creature_step_ms` mean nothing.

## What it looks like

`dress` is `BaseVendor.InitBody`/`InitOutfit` ported constant for constant: a
rolled gender, one of 57 skin hues carrying the partial-hue bit, one of nine hair
styles and seven beards at a matching hue, a shirt or doublet or fancy shirt,
trousers or a kilt or a skirt, and shoes of the type its trade declares. The
variety is in the dice, so it belongs where the dice are — and they are the
world's seeded `Rng`, so a shard populates the same town twice.

Three rules hold the port together:

- **The trade's own additions are the pack's**, worn *over* the base and winning
  any layer both want — the precedence a ServUO override has when it calls
  `base.InitOutfit()`. The smith's ringmail, apron, bascinet and hammer are data.
- **Only a human base body is dressed**, because `InitOutfit` dresses a human.
  Britannia's one non-human town NPC keeps its own body and its own bare skin
  rather than being replaced by a shopkeeper in a shirt.
- **Hair is an item, and that is a hazard.** UO has no hair field: hair and a
  beard are items on layers `0x0B` and `0x10`, drawn in the same `0x78` as a
  shirt. `FIXED_LAYERS` names the layers nothing may be lifted from — ServUO's
  `Movable = false` — without which a player drags the hair off a shopkeeper's
  head.

## Who it is

A label is two halves from two places: a `Title` its trade fixes ("the
blacksmith"), and a personal name in front of it. Sending only the title is what
made thirty-eight people in Felucca called "the banker".

The title comes with the spawn and is a **saved component**, because it is a
*key*: the keyword table an NPC answers from is looked up by it on every word
spoken nearby, so a binding that lived only in the spawn call would be lost at
the first restart. The personal name is generated from `data/names.json` off the
world's seeded `Rng`. That file carries a spread wide enough that a full Felucca
does not read as repetitive rather than ServUO's whole 3,632-name lists, which
belong to the operator's own checkout for the same reason no client files are
here.

## What it answers

`speech` is `VendorAI.OnSpeech`: an NPC overhears what is said within four tiles
(`HandlesOnSpeech`) and answers. Three rules make it a port rather than a
resemblance:

- **Keywords are whole words.** A substring test on the whole line is what made
  "that sword is unsellable" open a buy-back list. ServUO matches keyword *ids*
  the client encodes; whole-word matching is the closest honest equivalent
  without a cliloc keyword table.
- **Named, or not.** `vendor buy` / `vendor sell` work on whoever is nearest; a
  bare `buy` / `sell` works only when the vendor was named in the sentence
  (`BaseAI.WasNamed`), which is why saying "sell" in a crowded bank does not open
  four shops at once.
- **The mechanism is ServUO's and the words are data.** A `BaseVendor`'s entire
  vocabulary in the reference is two clilocs, so this module holds those two and
  a generic greeting — the fallback for a trade with no table — and the per-trade
  lines live in `state/data/speech.json`, keyed by `Title` and reaching the world
  through `server::content` the same way quests do. A shard that empties the file
  still speaks.

**Barks** are the same derivation with nobody to greet: a trade names itself and
what it actually stocks, on a long cooldown, because writing a personality per
trade is the one thing this deliberately does not do. A trade with no shop has
nothing to call out and stays quiet.

## The banker

A `Banker` is the base plus a service. Every character wears a bank box — a
container on the bank layer, graphic `0x0E7C` — so it persists and survives a
restart like any worn thing. Saying "bank" within reach of a banker opens it,
through the same `0x24`/`0x3C` a double-click sends; "balance" counts the gold in
it. The words are spoken, so it reads as a request the banker answers rather than
as a command.

## The shopkeeper

A vendor's stock is an ordinary container worn on the shop layer, which is what
makes the buy window the container machinery the game already has; the vendor
packets only add prices and labels alongside. Buying pays gold out of the
player's backpack (`0x74` contents, `0x3B` purchase); selling is the mirror
(`0x9E` list, `0x9F` sale) at half price, the classic margin. Price and name are
item components, so stock is pack data rather than engine code, and it persists
with the vendor.

Two details are the client's rather than ours. **The crate goes on both shop
layers `0x1A` and `0x1B`**, because ClassicUO's buy loop dereferences the
container on each with no null check. And the display is keyed on the vendor and
preceded by an equip per crate — ServUO's `SendPacksTo`.

**Access is one predicate at four doors.** A criminal, or a shop outside its
hours, is refused out loud at the open, the sell offer, the purchase *and* the
sale — because a client that already has the window up can still send a `0x3B`,
so refusing only at the open leaves the deal reachable.

**Restocking is checked when the shop is opened**, not on a tick pass — ServUO's
`DelayRestock`, an hour, and it costs nothing while nobody is shopping. What
"full" means has to be *remembered*, because the crate's live contents are what
is left and there is nothing else to compare them against; the price and label go
into the record too, since a sold-out line leaves no item behind to copy them
from. It is saved as seconds-still-to-wait, the `SpawnerRecord` rule, so a
restart does not come back either already due or an hour early.

## The routine, and where a night home comes from

`[gameplay] npc_schedule` (off by default, with `npc_work_hour` /
`npc_home_hour`) walks a townsperson to a night home outside working hours and
back to its post inside them, off the world clock the tick derives from the tick
counter — so it replays. It is marked **ours, not a port**: neither reference
ties an NPC to the hour. `config` refuses a working day that wraps midnight, so
the one comparison that reads the hours stays a comparison.

**Where the homes come from is a derivation, and the first one was the bug.** It
sent each townsperson to another townsperson's post in the same town, on the
reasoning that those are tiles ServUO itself stood a mobile on. They are — and
every one of them is somebody's workplace. A vendor's stock crate is worn, so a
shop is wherever the shopkeeper is standing: at dusk the tavernkeeper walked to
the innkeeper's counter with the shop on its back.

`Data/Decoration` has no bedrooms. It does have **chairs** — more seats than
there are townsfolk, every one indoors in a real room and none of them anybody's
post. So the destination is the nearest **unclaimed** seat, claimed as it is
taken, which makes the assignment a matching rather than a set of independent
nearest-picks: a collision is impossible rather than unlikely. Four rules hold
it, three asserted at generation time because a regression here is silent for
days and then looks like confused shopkeepers — never a vendor's tile, never a
tile already claimed, never a post whose owner is already walking here, and still
the nearest candidate between six and twenty tiles. Both bounds earn their place:
under six the NPC never leaves its wander range, and over twenty the bounded
search starts failing, at which point the naive fallback noses it into a wall all
night.

The engine settles an NPC *near* its post rather than on it — the walk home only
runs while it is further out than the wander radius — so this reads as people
drifting to the taverns at dusk rather than as a town standing on the furniture.
And the shop shuts, at the same access predicate all four doors already call.

## The guard

A guard is barely a creature, and that is the design. ServUO's `WarriorGuard` is
a **sentence, not a fight**: it materialises on the offender with the teleport
sparkle and sound, says its line, and deals their whole hit point total through
the one `combat::damage` door — so the corpse, the loot and the death event all
happen the usual way. A guard that can be fought is a guard that can be beaten,
and then a town is just a place with slightly more dangerous scenery.

Two paths reach it: the "guards" keyword spoken inside a guarded region, and a
murderer *crossing into* one, off the region-crossing event
(`GuardedRegion.OnEnter`). Candidacy is ServUO's `IsGuardCandidate` — a guard, a
ghost, an invulnerable or a member of staff is never one, whatever they have
done. **A guard earns no murder count**, because executing the guilty is the
whole of its purpose; ServUO says the same thing by clearing the guard's own
criminal flag and kill count on every beat. It vanishes on a tick counter when
its work is done.

## Pets and summons

`npc` owns what a creature *is* — it spawns them, dresses them and gives them a
brain — so making one somebody's belongs here rather than in `skills`, which only
decides that a taming resolved. The pet's *beat* is the AI's, for the same reason
a wild creature's is.

A **summon** is a pet with a deadline. `SetControlMaster` plus `Summoned = true`
is all ServUO's summon is, and everything a controlled creature does then follows
— friendly, heels, answers orders, counts against followers — so the marker
beside `Pet` carries only the tick it goes on. Three places outside this domain
read that marker and none of them could have been served from here: the save
sweep skips a summon (a restored five-minute daemon is a permanent one whose
caster no longer exists), the death path gives it no corpse (pre-AoS
`DeleteCorpseOnDeath`, without which a summoned daemon prints gold), and Dispel
is the question "is this thing summoned" and nothing else.
