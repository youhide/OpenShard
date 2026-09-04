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

## A worn layer can vanish for the same reason a mounted horse did

Fixed for the mount: a rider and its mount share one server-side frame index
(`Mobile::frame`), but the mount plays its own, separate animation group
(`mobiles::mount_of`). A mounted attack's frame count (five or seven, see
`crowd.rs`'s `action_on_mount`) can run past the mount's own — shorter — stand
animation, and the atlas lookup for that exact `(body, group, direction,
frame)` key then misses outright: no fallback frame is drawn, the whole mount
quad is filtered out, and the horse disappears for exactly those frames.
Fixed by `mobiles::mount_frame`, which wraps the shared index by the mount's
own frame count before building its atlas key (`crates/client/render/src/mobiles.rs`).

The same shape exists, unfixed, for worn equipment: a layer's picture is read
under its own resolved graphic but the *rider's* `mobile.group`/`mobile.frame`
(`push_quads`, `crates/client/render/src/mobiles.rs:762` and the equipment arm
of `pick_iter_with_interior`). If some item's animation for a given group has
fewer frames than the body wearing it, that one layer's atlas lookup misses on
the frames past its own count and is silently dropped — already handled
gracefully (the `AnimFrameSource::Frame(_)` filter drops just that layer, not
the whole mobile), so it reads as a flickering piece of gear rather than a
vanishing horse, and nobody has reported it. Worth the same `mount_frame`-style
wrap if it ever is.

## A big multi's other anchored entities may still pop in and out at the view boundary

`WorldState::refresh_around` (`state/src/runtime.rs`) tests every entity's
visibility against `centre` via the sector grid's own single point for that
entity — fine for a mobile or a one-tile item, wrong for anything expanded from
a multi table, whose drawn footprint can sit tiles away from the point on the
sector grid. Houses got a real fix for this: `houses_near` (was
`designed_houses_near` until it was widened to cover classic houses too, not
just `HouseDesign` ones) walks the actual drawn rectangle instead of the
anchor point, so a house stays on screen — and in the client's live overlay,
which is what its floor and walls actually *are* to the pathfinder — as long
as any part of it reaches the player's view square, not just its own anchor
tile.

Nothing else that is one entity expanded from a multi table got the same
treatment. `openshard-boats` is the other user of `MultiId` in this
workspace and goes through the same plain `everything_near(centre,
VIEW_RANGE)` as a mobile does — a large boat approached broadside, or any
future multi-shaped entity, can in principle flicker the same way a classic
house did before this session's fix. Not fixed here: boats move every tick
they are crewed, which changes the failure's shape (the anchor keeps sliding
past the boundary rather than sitting near it), and nobody has reported it.
Worth `design_reaches_view`-style treatment if it ever is.

## ~~An animal below body 200 is animated out of the monster table~~ — the table is read

`BodyKind::of` (`crates/common/uofiles/src/anim.rs`) decides which of the three
animation-group numberings a body uses from its id alone: below 200 monster,
below 400 animal, above human. That is the reference client's *fallback*, used
only when it has no `mobtypes.txt` — and the shipped file disagrees for **47
bodies under 200 that are `ANIMAL`**, among them the wolves (23, 25, 27, 34,
37, 97–100), the bears (167, 211-family) and the cougar/panther trio (63, 64,
65). ServUO's own `Data/bodyTable.cfg` says the same thing (`63 Animal`), so
both sides of the wire classify these bodies from the same table and we
classify them from a range.

For 40 of the 47 the *pictures* still come out right, because `Body.def`
redirects them into the animal range before a frame is read (63 → 214, 25 →
225, …) and `App::apply_body_def` translates the group across the families.
What does not come out right is everything that numbering decides:

- **Attack.** `Action::classic_action` (`state/src/runtime.rs:3214`) picks
  group 4 — `HighAnimationGroup.Attack1` — for a non-humanoid body, and
  `redirected_group` (`client/app/src/presentation.rs:1359`) deliberately does
  not translate combat poses, so group 4 arrives at animal body 214. In the low
  numbering 4 is `Unknown`, and for the cougar the file has *no* index entry for
  it in four of the five directions; the fifth decodes to three blank frames
  (its header says `width 8, height -258`). An attacking cougar therefore has
  no frame to draw and is dropped from the scene for the length of the swing.
  Its real `Attack1` is group 5, five frames in every direction.
- **Running.** `Tracked::moving_group` asks `BodyKind::of(63).running()`, which
  is `None` for a monster, so a running cougar is given the walk group; the
  redirect then maps walk to walk. `LowAnimationGroup.Run` (group 1, five
  frames per direction) never plays for any of these bodies.
- **Casting.** `classic_action` sends 12 for a non-humanoid cast, which is
  `Cast` in the high numbering and `Die2` in the low one. Nothing in this set
  casts today, so it is latent rather than visible.

The seven that `Body.def` does not redirect (5, 6, 29, 52, 81, 95, 169) are the
harder half of the same fact: `mobtypes.txt` gives them
`CalculateOffsetLowGroupExtended`, which is *low group numbering stored in a
high, 22-slot block* — the shape of their index confirms it, groups 0–12
populated inside a 110-entry block. `BodyKind` used to conflate the numbering
with the block layout, and those bodies need the two to disagree.

**Fixed** by reading the file the reference client reads.
`openshard_uofiles::mobtypes` parses `mobtypes.txt` into a body-to-family
table; `BodyKind` is now the *numbering* only and the new `IndexLayout` is the
*block shape*, which is what lets a body be animal-numbered inside a
monster-shaped block. The client holds the table on `Crowd`, where group
numbers are chosen, and lends it to the renderer to address the index with; the
shard holds its own on `WorldState::mob_types`, because `classic_action`
chooses group numbers server-side — ServUO reads `Data/bodyTable.cfg` for the
same reason. Every creature branch of `classic_action` and of the client's
`modern_action` now goes through `BodyKind::attacking`/`casting` rather than a
literal, which is what also took the animal cast off group 12 (`Die2`: a
casting animal used to fall over and stay down). An install with no
`mobtypes.txt` keeps the range rule, which is what the reference does too.

Three things this turned up. One is fixed, two are not:

- ~~**A mount is drawn through neither table.**~~ **Fixed**, both tables at
  once, because either alone is no fix: the numbering without the redirect asks
  a body with no frames for a different group it also does not have.
  `mobiles::mount_of` now takes the install's `MobTypes` and asks it for the
  mount's stand/walk/run, which the range rule is wrong about for **19 of the
  30** rideable bodies — the nine below 200 (116, 117, 122, 132, 144, 169, 187,
  188, 190) it calls monsters and the ten at 400 and above (791, 793, 794, 799,
  1407, 1408, 1410, 1440, 1441, 1510) it calls *humans*; the shipped file calls
  every one of them `ANIMAL`. And `App::redirect_mount` applies `Body.def` to
  the `Layer::MOUNT` graphic beside the body it already redirected, which the
  stock file has an opinion about for 13 of the 30 (116 and 117 are body 200
  hued 1109 and 1154; 791 is body 220). The mount's group needs no translating
  the way the rider's does — it is derived from the redirected body rather than
  carried — so the two tables meet in one place: the redirect decides which
  body, the table decides how its actions are numbered. Threading `MobTypes`
  into the renderer's mobile entry points is what that cost. **Open question
  left behind:** the redirect's hue replaces the saddle's own wire hue whenever
  the file gives one, which is the rule `apply_body_def` already applies to a
  body — nobody has checked against the reference whether a *dyed* mount item
  is supposed to win over `Body.def`'s colour, and today it loses.
- **Body 95 loses its picture, and it is not the same bug as 826's — checked
  against the real install rather than guessed.** `mobtypes.txt` has a single
  row for 95, `95\tANIMAL\t0` at `mobtypes.txt:96`, no extended flag, which is
  the low layout applied to a body below its own first id (200). The reference
  subtracts anyway (`CalculateLowGroupOffset`, `(95 - 200) * 65 + 22000 =
  15175`, still positive) and reads block 15175 onward — inside body 137/138's
  own *high*-numbered block, so a live client draws a slice of a stranger's
  frames rather than nothing. `IndexLayout::base` answers `None` for any
  `Low` body under 200 instead (`crates/common/uofiles/src/anim.rs:543-551`,
  asserted at line 1084), so this engine draws nothing rather than the
  stranger — confirmed live by opening the stock install's `anim.idx` and
  scanning every group/direction body 95's resolved layout has: `has_frames`
  is false throughout, not just for the groups this class of bug usually
  hits. Less wrong than the reference's own mistake, but still a gap.

  826 first looked like the same class (its symptom is identical — no
  picture at all) but is not: the stock `mobtypes.txt` has no `ANIMAL` row for
  it, only `826\tEQUIPMENT\t0` at line 541 and `826\tMONSTER\t\t10008\t0 #
  Stygian Dragon` at line 1042, and duplicate-id resolution (last line wins,
  in both `MobTypes::from_text`'s `BTreeMap::insert` and the reference's own
  `_mobTypes[id] = ...` in `AnimationsLoader.cs`) lands it on `MONSTER`/`High`
  — `IndexLayout::base(826) = Some(826 * 110)`, never `None`. Opening the
  stock `anim.idx` and scanning all 22 high groups × 5 directions for body
  826 confirms zero frames there too, and `Bodyconv.def:486` reads
  `826\t-1\t-1\t-1\t826\t-1`, whose fourth column is `anim5.mul`.

  **That row is a stub, and the reason is the flag word rather than the
  table.** `anim5.idx` is 951,300 bytes — 79,275 blocks — and body 826 in the
  high layout begins at block 90,860, so the legacy pair has nothing for it
  either; `Bodyconv.def` and all six `anim*` pairs are read now and 826 still
  draws nothing. Its own `mobtypes.txt` flags are `10008`, and `0x10000` is
  `AnimationFlags.UseUopAnimation`: the reference takes the
  `AnimationFrame*.uop` path for such a body *before* it consults
  `Bodyconv.def` at all. So 826 is an example of the UOP half of item 3 of
  [`docs/client/README.md`](../../client/README.md)'s "What is open, ranked",
  which is the half still open — not of the `.mul` half, which landed, and not
  of this entry's low-layout bug.
- **Equipment in the animal id range moved.** 55 bodies gain an index block
  they did not have — mostly `EQUIPMENT` rows between 318 and 340, which the
  range rule read at the animal stride and the table reads at the human one.
  Nothing checks those visually yet.

## ~~A body id names a file too, and five of the six were never opened~~ — read

**Fixed: `Bodyconv.def` is read and all six `anim`/`anim2`–`anim6` pairs are
opened.** A body added by a later expansion is not appended to `anim.mul`'s
index — it is re-numbered from zero and put in one of the other files, and this
table is the only thing that says which file and which id. The stock install
moves 875 bodies that way, 460 of which have a standing animation the first pair
has none for, so a reader that opened only `anim.idx`/`anim.mul` drew every one
of them as nothing: no error, no log, a creature hitting a player from an empty
tile. Body 752 is the whole shape of it — its entire existence in the client's
files is one row saying "id 29 of `anim2`", and `anim.mul` has its own,
different body 29 for a wrong reader to land on.

A lookup now carries where it reads from (`openshard_uofiles::anim::AnimSource`:
file, id-in-that-file, block shape), the reader owns the redirect table because
it is a fact about the files it opened, and `mobtypes.txt` keeps deciding the
block shape as before. Held by three tests against a real install in
`crates/common/uofiles/tests/client_files.rs`, one of which counts the whole
table both ways so that a redirect which *takes* frames away cannot hide among
the ones that add them.

Left open, each found while doing it:

- ~~**The numbering half of the fallback is still the general range rule.**~~
  **Fixed: `BodyConv` is threaded into the crowd.** Where `mobtypes.txt` has
  no line for a body, the *block shape* was already resolved against the file
  the body lands in (`BodyKind::in_file` — `anim2` has no people at all,
  `anim3` puts animals below monsters); which numbering names its *actions*
  now is too. `Anim::redirect_kinds` walks every row `Bodyconv.def` moves and
  `mobtypes.txt` is silent about, resolves the same redirect `Anim::source`
  already does — including which of the five files this install actually
  ships, so a body sent to a file that is not open still reads under its own
  id — and hands `BodyKind::in_file` the landing rather than the original id.
  `client/app`'s `Crowd` reads this table beside `MobTypes::kind_of` rather
  than falling straight through to `BodyKind::of`. Snapshotted once at
  startup, beside `mobtypes.txt`, because it cannot change once the install is
  open and `Crowd` needs the answer long before it has any reason to hold the
  (large, file-backed) reader itself. No shipped row's numbering actually
  changes on the stock install — its five such bodies already land where the
  two rules agree — so this closes an install-shaped gap rather than a bug the
  shipped files can be seen to hit; a later install that ships a row breaking
  that tie is what this was for.
- **`Bodyconv.def`'s mount height is not ported.** The reference attaches a
  vertical offset to a mounted body per redirect file (`-9` for most of `anim5`,
  `+9` for one `anim3` body, `0` for two ids), and it reads those numbers out of
  its own source rather than out of the file. Nothing here offsets a rider by
  file, so a mount drawn from a redirected body sits where an unredirected one
  would. Worth a look the next time a mount is measured against a screenshot.
- **Three install-backed tests fail on client 7.0.116.0, and none of them is
  about animations**: `a_real_fonts_mul_parses_to_ten_plausible_faces` (font 3's
  CP1251 `А` has no ink), `a_real_tiledata_name_carries_the_plural_marker_the_client_resolves`
  (`board%s` where it wants `boards`), and `the_two_multi_readers_agree_with_each_other`
  (478 of 800 shared multis describe different buildings — the loudest of the
  three by far). Not this work's: none of the three reaches the animation
  reader. Not bisected either, so what they are is still open — a reader defect,
  or a suite written against a different install than the one
  `OPENSHARD_CLIENT` points at, and which of the two is itself the first
  question. `crates/client/render`'s
  `the_two_silhouette_layers_are_two_lines_and_a_frame_agrees_about_both` fails
  beside them, on a scene with no mobile in it at all.
