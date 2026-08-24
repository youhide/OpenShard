# Gameplay backlog

[Backlog](README.md) · [Roadmap](../README.md)

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
  from that entry: the six remaining `Def*` tables, Repair/Enhance/AlterItem/
  Resmelt, recipe scrolls, make-number/make-max and the last-ten list, and the
  two material chains (hides → leather, cotton → cloth) that are addon
  interactions in ServUO rather than crafts.
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
Found while doing it, none started:

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
  ([`unenforced.md`](../../unenforced.md) S2). The five headers joined the data as
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
  `CraftSystemDef::needs_message` is an `Option<ClilocId>`, `None` on the four
  systems that need no workshop. Recorded rather than deleted because the entry
  sent a session looking for something that was not there: **check a backlog
  claim against the code before planning around it.**
- ~~**`Recipe::amount` has a column and no data.**~~ **Decided: the column
  stays**, with the reason in its doc. Every one of the 485 rows is 1, but
  `craft::complete` already multiplies by it and the recipes that would use it
  are `DefBowFletching`'s arrows and bolts — porting that table is adding data,
  whereas dropping the field would mean the port had to put it back *and* touch
  the craft path to do it.
- **Three files are still over the 2k line.** `world/src/tick/tests.rs` is
  12,964 — by a wide margin the largest file in the repository, and the split
  mechanics in `architecture.md` are written for exactly this;
  `state/src/runtime.rs` is 2,169 and `state/src/components.rs` 2,108, and
  either is the easier warm-up. Deliberately left out of
  [`unenforced.md`](../../unenforced.md) — see that file's last section for why a
  13,000-line mechanical move wants a session that owns the tree outright.
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
