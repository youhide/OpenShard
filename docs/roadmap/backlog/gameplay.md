# Gameplay backlog

[Backlog](../../../plans/roadmap/PLAN.md) · [Roadmap](../README.md)

## The tick moved and thirteen constants did not

`TICK_INTERVAL` went from 50ms to 25ms and `TICKS_PER_SECOND` from 20 to 40, and
every timer written as a bare tick count went on meaning what it meant at the old
rate — which is half the wall-clock it was chosen for. None of them was
arithmetically wrong; what changed underneath them was the unit.

The engine ones, now derived from `TICKS_PER_SECOND` and fixed:

- `combat::swing_ticks` ended `tenths * 2`, so **every swing on the shard was at
  twice its era's speed**. This is the one a player would have felt.
- `combat::MURDER_DECAY_TICKS` — eight hours had become four.
- `combat::vitals::{HITS_REGEN_TICKS, STAMINA_REGEN_TICKS}` — both twice as fast.
- `npc::live::BEAT_TICKS` — every townsperson living at double speed.
- `npc::guards::IDLE_TICKS`, `npc::vendor::RESTOCK_TICKS` — half their spans.
- `ai::{REPATH_TICKS, GUARD_TICKS, REFUSAL_TICKS}` — a two-second repath window
  became one, and two ten-second memories became five.
- `quests::progress::ESCORT_BEAT_TICKS` — an escortable ambling at 150ms a tile,
  faster than a player can run.
- `world::tick::defaults::SAVE_EVERY_TICKS` and `tick::status::STATUS_REFRESH_TICKS`
  — a world saving and a status bar refreshing at twice their documented rates.

`TICK_INTERVAL` and `TICKS_PER_SECOND` are now welded by a `const` assertion in
`tick/defaults.rs`, so the next person to move the tick gets a compile error
instead of a shard that quietly runs at half speed in a dozen places.

Left open:

- **Every remaining bare tick count is a latent one of these.** The sweep covered
  the constants that name a span of real time; it did not cover the tick counts
  passed as *arguments* — a spawner's `swing`, a script's `beat`, a decoration
  file's delay. Those are data, and data files carry the same unit ambiguity with
  nowhere to put a `const` assertion.

  **And it did not cover every constant either.** The `npc` migration found three
  survivors in one file: `GREET_COOLDOWN`, `GREET_COOLDOWN_JITTER` and
  `BARK_COOLDOWN` (`server/npc/src/live.rs`) are written `seconds * 20` against a
  `TICKS_PER_SECOND` of 40, so each runs at half the span its own doc comment
  states — a townsperson greets every 7.5 seconds where the comment says fifteen.
  `BEAT_TICKS` in the same file was converted and these were not, which is what a
  partial sweep looks like a month later. Ranked as row 3 of
  [`docs/npc/README.md`](../../npc/README.md).
- **A kiting archer can livelock**, and that is how the tick change was caught: a
  turn costs a whole beat (`motion::step`'s turn-as-step), and combat re-faces a
  fighter at its target before each swing. Where the swing is quicker than the
  beat the creature spends every beat turning round and never opens the gap. The
  fixture now states 500ms rather than a tick count, which hides it again; the
  rule that a turn and a step compete for one beat is the real thing to look at.

## Not built, and until now not written down

A sweep of this file against the code turned up a set of gaps that were not
missing on purpose — they were simply never recorded, which is the difference
between a decision and an oversight. Listed here so they are visible; none is
started.

- ~~**Regions.**~~ and ~~**Day and night.**~~ Both landed together; see
  **Regions, guards and the world clock** in §6 below. What is still open from
  that entry: `0x65` weather, a calendar that turns the season, and the `safe`
  flag, which is carried in the data and has no consumer until PvP rules exist.
  `no_recall` got its first reader with travel.
- ~~**Fame, karma and titles.**~~ Landed; see **A character has a standing** in
  §6. The Felucca converter still falls back to a karma-sign heuristic for
  *notoriety*, which is a converter gap and is listed as one below.
- ~~**Resource gathering.**~~ Landed; see **Mining, Lumberjacking and Fishing**
  in §6 below.
- ~~**Crafting.**~~ Landed; see **Crafting** in §6 `crafting` below. Still open
  from that entry: the four remaining `Def*` tables, Repair/Enhance/AlterItem/
  Resmelt, recipe scrolls, make-number/make-max and the last-ten list, and the
  two material chains (hides → leather, cotton → cloth) that are addon
  interactions in ServUO rather than crafts.
- ~~**A lumberjack's logs have no sink; a carpenter's boards have no source.**~~
  **Landed** (`c2ae15e0`). `crafting::chop` is the bridge, reached through the
  lumberjack's own harvest cursor rather than a double click on the log: that
  cursor answers two things upstream, a tile to swing at and an item in the pack
  to cut up. The gate is either Carpentry or Lumberjacking at `harvest::WOODS`'
  own `req_skill` — upstream writes those numbers twice, in the harvest
  definition and in `Log.cs`, and they agree — so the wood a lumberjack cannot
  fell is the wood a carpenter cannot work. `known_gaps()` did have to split by
  era, exactly as the last paragraph of this entry predicted. The reading below
  is kept because it is what the fix was designed against:

  Mining closes its loop — ore comes off a vein carrying a `MaterialId`
  (`crates/server/state/src/harvest.rs`, `ORES`), and `crafting::smelt` turns
  the pile into ingots of the same grade for the smith's material axis. Wood
  does not: `WOODS` pays seven grades of `ItemKindId(3)` (log), while every
  carpentry, fletching and tinkering row spends `ItemKindId(36)` (board,
  `0x1BD7`), and nothing anywhere converts one into the other. The only board
  in the world is the twenty a vendor stocks
  (`crates/server/world/data/townsfolk.json:203`), plain wood at that, so the
  six special woods are unreachable by any crafter and chopping pays in an item
  with no use. ServUO's own bridge is `IAxe`, not a double click: an axe swung at
  a `BaseLog` calls `TryCreateBoards`, gated on Lumberjacking — 0 for plain wood,
  65 oak, 80 ash, 95 yew, 100 for heartwood, bloodwood and frostwood
  (`Scripts/Items/Resource/Log.cs`) — and pays boards of the log's own resource.
  So the missing piece is one recipe-shaped conversion, not a system.

  **Now checked rather than remembered.** `openshard_world::economy` builds the
  whole production graph — harvest tables, vendor shelves, loot tables, crop
  fields, butchery, shearing, the wheel, the loom, the three hand-written
  bridges in its `CONVERSIONS`, and every recipe at every grade of its trade's
  material axis — and runs reachability from the sources. `cargo run -p
  openshard-world --bin economy` prints it; a `#[test]` beside it pins today's
  holes both ways, so a new one is a red test and closing one is a red test
  until its row is deleted.

  **What the board gap actually costs, counted rather than estimated.** The
  report's 1,213 stalled steps are not all the boards': 780 of them name a board
  and nothing else, 108 name a board and an art, 197 name only art, and 128 are
  the horned and barbed leather below. So the bridge alone frees 780 steps and
  unblocks 888 in total, evenly split at 260 per special wood — plain wood is
  already reachable off the vendor's shelf, which is why this cost stayed
  invisible. By trade the stall is overwhelmingly one trade's: 876 Carpentry,
  129 Tinkering, 126 Tailoring, 33 Cooking, 30 Fletching, 9 Blacksmith,
  7 Alchemy.

  One decision the bridge forces, worth settling before it is written:
  `known_gaps()` is one list for both eras today. After the bridge the six
  special boards stay unreachable before Mondain's Legacy — no tree gives those
  logs — so the list has to split by era, or the pre-ML assertion goes red for
  the right reason.
- ~~**The audit found five more holes, and only two of them were written down.**~~
  **Every one of them is closed except the first**, and what is left of that one
  is a decision rather than an implementation. The order and the four tracks that
  remain — the peerless ingredients, the vendor shelves the converter dropped,
  and the catalogue's era leaks — are
  [`plans/items/economy_closure/PLAN.md`](../../../plans/items/economy_closure/PLAN.md).
  The report went from 56 unreachable resources to 25, from 1,213 stalled recipe
  rows to 127, and from nine raw materials nothing consumed to none.

  What each fix turned out to be, since none of them was the shape this entry
  guessed: wheat is a `CropKind` variant and nine `Regions.xml` fields; the flour
  sack opens on a double click (`items::flour`); the pitcher of water was a
  vendor line the converter dropped, because upstream's tavern beverages are
  `BeverageBuyInfo` and not `GenericBuyInfo`; a fish is `ICarvable` and cuts into
  four steaks; horned and barbed hides came from making the dragon family
  carvable, and every one of those bodies was already spawning; a bone is loot
  off the undead rather than butchery; tainted wool comes off a woolly *corpse*
  where the shear pays the fleece; and sand waited for the whole trade that
  spends it, `defs::glassblowing`. The reading below is kept as written:
  - **Twenty-two Mondain's Legacy ingredients (`0x3183`–`0x3199`)** have no
    source at all. Upstream pays them out of Heartwood quest turn-ins and
    champion drops, and this shard has neither, so every ML recipe that wants
    one is unbuildable. The largest single group in the report, and the one that
    most wants a decision rather than an implementation: a quest chain, a loot
    line, or the rows deleted as unshippable.
  - **The cooking chain never starts**, and it is short three roots rather than
    the dozen arts the report lists. Most of those arts are cascade: dough
    (`0x103D`), `0x103F`, `0x1042`, `0x1044` and `0x1083` all have recipes in
    `cooking.json` (two, three, six, one and two rows), and they are dead only
    because their inputs are. The roots are three.

    First, no field grows wheat (`0x1EBD`): `CropKind`
    (`crates/server/state/src/components.rs`) has exactly one variant, `Cotton`,
    which is upstream's own content — a wheat field is data, not a system. The
    recipes also declare a `Needs { mill: true }` that nothing fills.

    Second, and not previously written down: the flour sack is never opened.
    `cooking.json`'s first row mills wheat into `0x1039`, the **closed** sack,
    while all four of its consumers spend `0x103A`, the **open** one. Upstream
    opens it on double click. That is the same shape as the log bridge — a
    conversion between two arts of one item — so it belongs beside it in
    `CONVERSIONS`, and wheat alone would not close the chain without it.

    Third, a pitcher of water (`0x1F9D`) is spent by the dough row and no vendor
    stocks it and nothing fills it.
  - **Fish are caught and eaten by nothing.** `harvest::FISHES` pays `0x09CC`
    and no cooking row consumes it: upstream cuts steaks off a fish with a
    knife, an item action beside `items::cut` that does not exist here.
  - **Sand has the same shape.** The `SAND` harvest definition shipped ahead of
    glassblowing, which is not implemented, so a miner can fill a pack with
    something no trade spends.
  - **Horned and barbed leather are unreachable at both ends.** Tailoring spends
    them and no carvable body wears them — `items::carve`'s own doc says every
    ServUO creature that does is a dragon, drake, wyrm or serpent, and none of
    those bodies is carvable here. The hides are as unreachable as the leather,
    so this is a carve-table row, not a scissors one.
  - Smaller: a **bone** (`0x0F7E`) for the tailor's bone armour, with nothing
    carving an undead corpse; and **tainted wool** (`0x101F`), which the wheel
    knows how to spin and only a lich's flock grows.
- **Stamina has the mana bug mana just lost.** `0xA2` landed for mana
  (`WorldState::set_mana`, §6 `magic`); `0xA3` does not exist, and `Stamina` is
  mutated in place by every step and every regen tick with nothing sent, so the
  pool reaches a client only inside a `0x11` — which `refresh_statuses` sends on a
  diff of *inventory-derived* numbers. The stakes are higher than a stale bar: a
  client that believes it has no stamina **refuses to run**, with no error to show
  for it (`mobile::MobileStatus`'s own doc says so). The fix is the shape mana
  took — one `set_stamina` door beside `set_mana`, `0xA3` beside `0xA1`/`0xA2`,
  and this client's pool moving out of `Status` to `Player` as mana's did.
- **The status window now draws twenty numbers this shard always answers zero
  for.** `0x11`'s AoS tail — four elemental resistances, luck, tithing, and the
  fifteen-short type-6 block (the five resistance caps, defence chance and its
  cap, and the eight suit bonuses) — is decoded and drawn now rather than
  skipped, so the modern frame states what the shard says instead of nothing.
  What the shard says is `Resistances::NONE` and `AosStatus::NONE`, built in
  `tick/status.rs`: there is no resistance system, no luck, no tithing and no
  item property that grants a suit bonus. That is honest for a pre-AoS shard and
  is exactly why the classic frame is the default, but it means the modern frame
  has six columns of zeroes until an item-property system exists. Weapon damage
  is the one that is already real (`combat::melee_damage_range`), and the
  physical figure rides in `armor` as it always has.
- **Neither status frame draws its buff-icon button.** The reference client puts
  one at `(20, 42)` on the classic frame and `(40, 50)` on the modern one, and it
  opens the buff/debuff icon window. There is no buff window and no `0xDF` on the
  wire here, so the button is left out rather than drawn wired to nothing —
  `crates/client/render/src/status.rs`. It is two pictures and one `Effect` the
  day buffs exist.
- **Nothing but a player ever casts.** `crates/server/ai` has no notion of a
  spell: no mana on a creature, no choice of spell in `fight_phase`, no cast in
  the beat. A lich, a mage-brigand and a healing dragon are all impossible, so the
  whole of §6 `magic` is one-directional — the player casts at the world and the
  world never casts back. The cast path itself is reusable (`begin_cast` is a
  client seam, but `resolve_cast`/`apply_spell_effect` are not), so what is
  missing is the *decision*: which spell, at whom, and how often.
- ~~**A scroll is a textbook and not a spell.**~~ **Landed.** Double-clicking a
  Magery scroll in your own pack now casts its spell: the spellbook gate is
  skipped, no reagents are taken, the roll is easier, and the scroll is torn up
  when the cast lands. Dragging one onto a spellbook still teaches it instead —
  that is the drop path, and this is the click. Four things the entry did not
  say, each settled against ServUO rather than guessed:
  - **The relief is two circles, not one.** `MagerySpell.GetCastSkills` does
    `circle -= 2`, so an eighth-circle scroll is rolled as a sixth-circle spell
    and a first-circle scroll's whole band sits below zero. This entry said
    "less one" — one more for the rule the `Text::Cliloc(0)` entry above states:
    **check a backlog claim against the code before planning around it.**
  - **The mana is not discounted.** Only a wand casts free in ServUO; a scroll
    pays its circle's mana in full.
  - **The scroll is spent on a cast that *landed*.** ServUO consumes it inside
    `CheckSequence`'s `CheckFizzle` success branch and nowhere else, so it is
    deliberately *not* under `reagent_loss_on_fail`: that knob governs a pile of
    reagents, and a scroll is one item that is the whole cast. A fizzle still
    costs the mana while `mana_loss_on_fail` is on, so a retry is not free.
  - **A scroll can leave the pack mid-cast.** The rooted (ServUO-style) cast
    carries the scroll on `Casting` and re-checks it at resolution, so a scroll
    traded away during the delay fizzles rather than casting for free.

  Left open from it: **a scroll cast is a player's alone**, because a creature
  reaches no double-click — which is the "nothing but a player ever casts" entry
  above, not a second thing. And **the cast is unbounded by reach**: a scroll
  deep in a bag in the pack casts, which is ServUO's recursive
  `IsChildOf(from.Backpack)` and so is correct, but it means the shard has no
  notion of a scroll being *held* the way a wand would be.
- **Eleven of the fourteen unbuilt spells need no new subsystem.** They are
  `SpellEffect::Unimplemented` only because nobody has written the arm: Create
  Food (spawn into the pack), Mana Drain and Mana Vampire (`Mana` is right there),
  Arch Protection and Mass Curse (the area sweep plus the buff appliers that both
  already exist), Invisibility and Reveal (`Hidden`, `break_cover`, `refresh_around`
  all exist), Magic Lock and Unlock (`ILockable` exists), and Magic Trap and Untrap
  (`Trap`/`TrapKind` and `tick/traps.rs` exist). The genuinely blocked ones are
  Telekinesis, Incognito and Polymorph. (The eight summons and the three dispels
  behind them were the rest of this list, and have landed.)
- **Two functions answer "where is this mobile's backpack".**
  `openshard_items::backpack_of` finds the item on the backpack layer *and*
  checks it is a `Container`; `World::caster_pack`
  ([`tick/spells.rs`](../../../crates/server/world/src/tick/spells.rs)) does the
  same walk without that second half, so a non-container worn on the backpack
  layer would be handed to `pay_and_roll` as the pack reagents come out of. Found
  while wiring the scroll cast, which reaches the pack through the *other* one —
  so a cast now asks two different questions about the same backpack depending on
  which half of it is running. One of them should go.

- **House catalogue material-family umbrella rows.** The generated house item
  catalogue currently emits a material-less semantic identity as well as every
  concrete material for metal, wood and leather families. That material-less
  identity is not constructible; F1 filters it out. House search should model
  "any material" as a selector distinct from an exact item identity, or stop
  emitting the invalid exact row.
- ~~**Atomic item transactions and inventory search.**~~ **Landed.** Canonical
  ownership now maintains exact container membership; split, merge, give,
  withdrawal, and successful craft output use validated prepare/commit doors.
  Recursive backpack stock and catalogue work have measured hard tick bounds,
  and Ctrl+I provides permission-filtered, paginated house inventory search.
  Direct crafting from house boxes was deliberately rejected as a separate
  access-policy feature, not left half-built behind the search index. The
  contracts and release evidence are in
  [`item_transactions_plan.md`](../../items/design_transactions.md).
- ~~**Travel.**~~ Landed; see **Travel** in §6 `magic`. Still open from that
  entry: Sacred Journey, the moon-phase gates, red/young restrictions, ship-mark
  runes, and a tooltip that refreshes when a property changes — which travel gave
  its first real consumer, since a marked rune's name changes under the player.
- ~~**Party (`0xBF 0x06`).**~~ Landed; see **Parties** in §6 below, and guild
  chat landed on the router it built. Still open from that entry: the loot flag
  has no consumer. **Chat channels (`0xB3`/`0xB5`)** are untouched and are a
  separate thing — the channel window, not the group.
- ~~**Pets and taming.**~~ Landed with Animal Taming; see **Taming, and the pets
  it wanted** in §6 `skills`. Still open from that entry: **stabling** (which
  wants a pet saved with no position, the logged-out-character shape),
  **loyalty** (pointless without feeding) and **Herding**.
- ~~**CI.**~~ **Closed, and it had been for a while.** This entry said
  `.github/workflows` held a release workflow and nothing that ran `cargo test` /
  `clippy` / `fmt`. There is a `ci.yml`, on every pull request and every push to
  `main`, running all three with `-D warnings` and `--locked` — so the project's
  "all three silent" rule is enforced rather than asked for. Recorded as a
  correction rather than deleted, for the reason the `Text::Cliloc(0)` entry
  below is: **check a backlog claim against the code before planning around it.**
- Smaller, and each a slice of an hour or two: dyes and hues on crafted and
  looted items, writable books, the localized text on the signs the converter
  already places, and rate limiting beyond the walk-pace bucket.
## Backlog from the data-table sweep

The craft, body-type, mount, skill, creature-name, creature-sound, harvest-tile
and NPC-name tables moved out of Rust source and into `data/*.json` behind a
`build.rs` (18,155 lines of source became 5,521 of data; the rule is now in
[`architecture.md`](../../architecture.md#a-big-table-is-data-and-lives-in-datajson)).
The mount table has since moved again — it is thirty rows and the *client* needs
them too, so it is `openshard_protocol::mounts` and there is no
`state/data/mounts.json` any more. Found while doing the sweep, none started:

- **Three tables share the `body` key and are three files.** `body_types.json`
  answers what *type* a body is, `creature_names.json` what it is *called*, and
  `creature_sounds.json` what it *sounds* like — and `creature_base_sound`'s own
  doc already says "grow it alongside `creature_name`", which is an invariant
  stated in prose because nothing enforces it. They were left separate on
  purpose: the three disagree about which bodies share a row (the dire, grey and
  timber wolves are three names and one howl) and the sound rows carry trailing
  notes the other two have no column for. One file keyed by body, with three
  optional columns, would end the drift — at the cost of a format that has to
  express "these four bodies share a sound but not a name".

- ~~**The recipe invariants are tested, not enforced.**~~ **Done**
  ([`unenforced.md`](../../server/evidence/2026-07-31-invariants-nothing-enforces.md) S2). The headers joined the data as
  `crafting/data/craft_systems.json`, so `build.rs` has both halves and checks
  them: a recipe whose group index is out of range, or that does not lead with
  its system's main skill, is now a build failure naming the row. The two
  assertions in `defs/mod.rs` are gone rather than kept beside it — a check in
  two places drifts. Two coverage checks came with them, because "no bad rows"
  is worth nothing if the rows were never opened: a table no header claims, and
  a header whose table is empty, both fail the build too.
- ~~**`Text::Cliloc(0)` is a null.**~~ **Not true, checked:** of the 11,448
  clilocs the craft tables generate, none is `0` — whatever `generate.cjs` did
  when this was written, the data it produces today has no missing
  `TextDefinition` in it. The other half of the entry was real and is now fixed:
  `CraftSystemDef::needs_message` is an `Option<ClilocId>`, `None` on systems
  that need no workshop. Recorded rather than deleted because the entry
  sent a session looking for something that was not there: **check a backlog
  claim against the code before planning around it.**
- ~~**`Recipe::amount` has a column and no data.**~~ **Decided: the column
  stays**, with the reason in its doc. Every shipped row is 1; batch recipes such
  as shafts, arrows and bolts use `use_all_res`, while `amount` remains the
  multiplier available to custom recipes.
- **Three files are still over the 2k line, and every number in this entry was
  stale.** Measured 2026-09-03: `world/src/tick/tests.rs` is **25,135** lines, not
  12,964 — by a wide margin the largest file in the repository, and the split
  mechanics in `architecture.md` are written for exactly this;
  `state/src/runtime.rs` is **5,508**, not 2,169, and `state/src/components.rs`
  **3,908**, not 2,108. All three roughly doubled while the entry said otherwise,
  which is the entry's own lesson twice over: **a number in a queue goes stale
  without anybody seeing it**, and a claim is worth checking against the code
  before planning around it. `components.rs` is still the easier warm-up.
  Deliberately left out of
  [`unenforced.md`](../../server/evidence/2026-07-31-invariants-nothing-enforces.md) — see that file's last section for why a
  mechanical move of this size wants a session that owns the tree outright.

## ~~A double door is two leaves, and nothing links them~~ — linked

**Fixed: the reported diagonal through a double doorway no longer opens only one
leaf.** One leaf used to swing open while the other stayed shut, and the shut
one was what the step was actually refused by — but it was refused as a *flank*,
so nothing about the picture said which door was in the way.

The chain, end to end:

- `world/src/tick/decor.rs`'s `generate_doors` places **two** leaves in a
  two-tile gap (`GenFacing::WestCw` at `vx + 1` and `EastCcw` at `vx + 2`, and
  the north/south pair likewise). That much is ServUO's `DoorGenerator`.
- What was missing was the next two lines of it:
  `Scripts/Commands/DoorGenerator.cs:512` sets `first.Link = second; second.Link
  = first`, and `BaseDoor.Use` (`BaseDoor.cs:313`) opens the link along with the
  door. Our [`Door`](../../../crates/server/state/src/components.rs) component
  had no link field at all, and `items::doors::toggle_door` toggled exactly one
  entity.
- The client plans on `Doors::AllOpen` whenever auto-open is on (the default),
  and that reading is applied to the **flanks** of a diagonal as well as its
  landing — `steps_out_of` resolves all eight neighbours through one footing.
- `App::open_door_ahead` sends a use for the shut door on the tile the step
  *lands* on, and only that one. The other leaf is a flank, never the landing,
  so it is never used.
- The shard reads the flanks `AsTheyStand` (`WorldState::walking_doors` for a
  living mover), so `Walker::request` refuses the diagonal at the corner rule —
  a `0x21`, a rollback, and a walk-sequence reset, repeated at walking pace for
  as long as the order stands.

Every diagonal through a double doorway has the *other* leaf as one of its two
flanks, which is why it is every diagonal and not some of them. The cardinal
step through the same doorway works, because there the shut leaf is the landing
and auto-open reaches it.

The missing port is now in the engine, not patched into the client: generation
links both leaves by stable serial, `toggle_door` and the AI opener move the pair,
`DoorState` saves the link, and auto-close first takes and checks the whole pair.
With both leaves swinging together, the picture, obstruction index and timer
stay consistent; if a player stands under either closed position, neither leaf
closes until the doorway is clear.

**The narrower defect underneath it survived that fix**, and it is the one a
player kept hitting: the client read a shut door as passable in a flank it would
never open. The link only reaches a *generated* doorway — every door placed from
decoration data is placed with `link: None` (`World::decorate`), and so is every
door a house adopts — so in a live world one leaf swings, the other stays shut,
and the diagonal past it is refused at the corner rule. On screen the diagonals
are the horizontal walk, which is why the report was "doors block me sideways,
and only one of them opens".

**Now closed at the client, the first of the two ways named here**: the auto-door
opens every shut leaf a step needs — the landing *and*, on a diagonal, both
flanks. The tiles are [`world::doors_a_step_needs`](../../../crates/client/app/src/world.rs),
which takes them from `movement` (`intend` for the landing, `Direction::flanks`
for the pair, the same call `steps_out_of`'s corner rule is made of) rather than
from arithmetic of its own, so the end that *asks* for the step and the end that
*refuses* it cannot derive different tiles. `App::auto_opened_door` became
`auto_opened_doors`, so a locked leaf still receives one use and not one a beat.
Three scenarios in `client/app/src/dst.rs` hold it: the diagonal through a
two-leaf doorway takes no refusal and uses both leaves, the cardinal through the
same doorway uses only the leaf it lands on, and with the auto-door off no use is
sent and no step is asked for.

Two threads are left, and neither is a rubber-band:

- **A leaf the shard will not open** — locked, someone else's house door — is
  still planned through and still refused, once per press now rather than once
  per beat. The remedy is the second way named above: read the flanks
  `AsTheyStand` whatever the mover's door policy is, which costs a diagonal the
  shard would have allowed. Worth doing when a `0x21` at a locked door is what a
  player actually complains about.
- **A decoration double door still swings one leaf to a double-click**, and to a
  cardinal walk, because nothing links the pair. ServUO links only what its
  `DoorGenerator` places, and this engine copies that; OSI swings both. The fix
  is to pair adjacent decoration doors at placement by their hinge graphics
  (`doorgen::GenFacing`'s two pairs) — derivable from the door table rather than
  guessed — and it is a decision about world data, so it is not being taken
  here.

## Deferred / not yet ported (the Felucca converter)

The one-shot converter (`OpenShard-Community-Pack/tools/convert-servuo.cjs`) lays
the whole facet, but it skips or approximates a few things by design. Recorded
here so the gaps are visible, not silent:

- **Creatures with no literal body** are dropped from the spawns. `resolveBody`
  reads only a literal `Body =`, `Utility.RandomList(first, …)`, `SetBody(n)` or
  the first element of an `int[]` mount table. So `WanderingHealer`/`evilhealer`
  (body set indirectly), the **camp meta-spawners** `Orccamp`/`Ratcamp`/
  `LizardmenCamp` (a `BaseCamp` spawns creatures and tents but has no body of its
  own, so *its* creatures are lost with it), `Ridablellama`/`Forestostard` (mount
  tables / odd casing) and `Shadowfiend` fall through. `TreasureLevel1-4` are the
  loudest "unresolved" names but are not creatures at all — XmlSpawner sub-tier
  tokens. Where a body *does* resolve, `RandomList` keeps only the first, and
  `SetHits`/`SetDamage` are averaged.
- **Decoration whose point is a function, not art**, is dropped (`SKIP_DECO`):
  teleporters, blockers, warning/hint items, traps, levers, obelisks, serpent
  pillars. Placing the graphic as scenery would show a tile the client draws as
  nothing; the teleport destination, blocking volume and trap trigger are lost,
  not just the art.
- **Containers** are placed **empty** (no loot), and a container graphic not in
  the seeded gump table falls back to the plain wooden-box gump `0x3C`.
- **Signs** place the board art; the localized **cliloc text** is read past and
  discarded (a later slice).
- **Vendors**: town NPC types with no vendor class and no shop are skipped — which
  is where the quest NPCs (escortables, the Bard-Mastery knights) land today until
  `quests` claims them. Expansion-gated (`Core.AOS`/SE/SA) shop items are dropped
  (this is a pre-AoS shard), and `SBMage`'s scroll stock is circles 1–3 only, as
  ServUO ships it.
- **Notoriety** is a karma-sign heuristic (`Karma < 0` → enemy-orange, else grey),
  not ServUO's full alignment/fame computation.
- **Door generation** skips a town whose decoration bbox exceeds `MAX_DOOR_REGION`
  (350k tiles), so a stray far-flung entry can cost that town its generated shop
  doors rather than make `op_generate_doors` sweep millions of tiles.

The bridge is both event- and tick-driven now: the server calls the script's
`onEvent` with each tick's domain events, and the per-mobile `onTick` for every
mobile a script controls (`op_control`, the `Scripted` marker) — the hook the
benchmark priced. The script vocabulary — the events in, the commands out — grows
one gameplay area at a time, each new command mapped in `into_world`.

The balance data comes from the SphereServer scriptpack (`Scripts-X`): `items/`,
`skills/`, `spells/`, `npcs/`, `crafting/`. Numbers taken, arithmetic audited —
the same bargain as everywhere else Sphere is read.
