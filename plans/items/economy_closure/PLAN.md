# Closing the production graph

`openshard_world::economy` asks one question — can every resource some step wants
be produced by some other step, starting from the sources — and when this page
was written the answer was **no**: 56 resources unreachable, 1,213 recipe rows
that could never run, and 9 raw materials paid out that no trade spends.

Two commits later it is 26, 127 and 1. The audit and its ratchet are built; what
is open is the content behind them. This page is the order to take the rest in.
The report is the measurement:

```sh
cargo run -p openshard-world --bin economy
```

Every step below ends the same way, and it is not optional: the resource leaves
the report, **and its row is deleted from `known_gaps()` in the same commit**.
The ratchet compares both ways, so a step that closes a hole without deleting its
row is as red as one that opens a new hole.

## What the report is not

It is a reachability question, not a playability one. A resource is "reachable"
the moment *some* step can pay it — a vendor's shelf counts, a loot table counts,
a crop field counts. Closing a row therefore never means "the content is good",
only "the chain is not cut". Where a step below chooses a cheap source over the
upstream one, it says so and says why.

## Done

- [x] **1. An axe on a log makes boards.** `crafting::chop`, reached through the
      lumberjack's own harvest cursor, gated on Carpentry *or* Lumberjacking at
      `harvest::WOODS`' own `req_skill`. **1,213 stalled rows** — the largest
      thing on this page by a wide margin. Commit `c2ae15e0`.

- [x] **2. A sack of flour opens** (`items::flour`), **3. a field grows wheat**
      (nine `FarmableWheat` regions from `Regions.xml`, and `CropKind::Wheat`),
      **4. a blade cuts up a fish** (`ICarvable`, four steaks), **5. the carving
      table** gains the dragon family's horned and barbed hides and a wool column
      that pays *tainted* wool off a woolly corpse, **6. undead carry a bone** as
      loot rather than butchery. Commit `63f22a7a`.

      That commit also carries two things its message does not name, and they are
      recorded here instead: the **harvest bonus tables**
      (`harvest::BonusResource`, ServUO's `BonusHarvestResource`) — a bark
      fragment and a brilliant amber off a tree, six gems out of a rock face, a
      white pearl out of the sea, all Mondain's Legacy and all absolute-chance
      rows with the "nothing" slack left out — and the **vendor lines the
      converter dropped**: the barkeeper's ten beverages, which is where a
      pitcher of water comes from and therefore where dough does, and the mage's
      five necromancer reagents, which sat behind an `if (Core.AOS)`.

## What is left, and it is four tracks rather than six rows

The 26 rows that remain are not 26 problems. They are four, and only the first
is an implementation.

- [ ] **A. Glassblowing.** `harvest::SAND` shipped ahead of the trade that spends
      it, and sand is still the one raw material nothing consumes. Upstream is
      `DefGlassblowing`: thirteen rows in the Mondain's Legacy era, main skill
      **Alchemy**, a forge to work at, and a **blowpipe** (`0xE8A`) to work with.

      Two things make it more than a table:

      - **The tool cannot be told apart from a mortar and pestle by skill**, and
        `craft::tool_system` matches a tool to a trade *by skill*. Glassblowing
        and alchemy share Alchemy, so the mapping has to become trade-keyed:
        `CraftSystemDef` gains the `trade` name its JSON row already carries, and
        `CraftToolData` names a trade instead of a skill.
      - **Both halves are gated on a learned flag upstream** —
        `PlayerMobile.Glassblowing` and `PlayerMobile.SandMining`, taught by two
        books sold in Ter Mur, which is a facet this shard does not have. So the
        shipped gate is the other half of upstream's condition, the skill at 100,
        and the flag is a documented divergence rather than an invented seller.
        The same is already true of sand: this shard lets any miner at 100 dig
        it, and upstream does not.

- [ ] **B. The vendor shelves the converter dropped.** Not a row of the report —
      a class of them. Measured 2026-09-03: **35 alchemists, 22 innkeepers, 22
      tavernkeepers**, 7 scribes, 5 tinkers, 11 animal trainers, 9 shipwrights
      and every guildmaster are placed with an outfit and **no shelf at all**,
      though upstream gives each an `SB*.cs`. Two of the report's rows are this
      and nothing else: a banana (`0x171F`), which only the innkeeper sells, and
      the blowpipe track A needs, which only the alchemist does.

      The beverage and reagent lines already found are the same bug one level
      down — a row the converter skipped because it was not a `GenericBuyInfo` —
      so this track is worth doing as a sweep rather than a row at a time.

- [ ] **C. Content this shard does not have.** Fourteen rows, and no honest fix
      that is not a decision:

      | Rows | What they are | Where they come from upstream |
      |---|---|---|
      | `0x3183`–`0x318E` (12) | blight, corruption, scourge, putrefaction, taint, muculent, the lard of Paroxysmus, a dread horn's mane, diseased bark, grizzled bones, the eye of the Travesty, a captured essence | peerless bosses |
      | `0x315A`, `0x4005` | a pristine dread horn, a toxic venom sac | the same bosses under other names |
      | `0x0EF0`, `0x1879` | silver, copper wire | faction stores and the Mad Scientist quest |
      | `0x14F8`, `0x1374` | a rope, a bridle | quest statics |
      | `0x2F57`, `0x2F5C` | a runed prism, an enchanted switch | Heartwood turn-ins |
      | `0x1E25` | academic books | an artifact |

      Three ways to close them, and the choice is the shard's rather than the
      audit's: a **loot line** on bodies that already spawn, which is the closest
      honest analogue of a champion drop; a **vendor shelf**, which is cheap and
      economically wrong for a reward item; or **deleting the rows that want
      them**, which shrinks the catalogue and closes the report honestly.

- [ ] **D. The catalogue is not era-clean.** `alchemy.json` ships a Nexus Core,
      which is `if (Core.SA)` upstream; its crushed glass is an SA blacksmithy
      row that was *not* imported, so the consumer is here and the producer is
      not. Cocoa pulp and cocoa butter (`0x0F7C`, `0x1044`) are Time of Legends.
      This shard is Mondain's Legacy — `harvest` already keeps a pre-ML and an ML
      table — and the recipe tables have no era column at all.

      An era column on a recipe would settle five of the remaining rows by
      deleting nothing: they would simply not be in this shard's catalogue.

## Two rows nobody can close

Worth stating so they are not re-investigated: `0x15F8`, an empty wooden bowl,
appears in exactly two places in the whole of ServUO — its own class and the
recipe that eats it. Nothing sells, crafts or drops one, so `DefCooking`'s fruit
bowl is unbuildable on OSI's own shards, and the banana in track B only gets
that row half-way. Selling one is a divergence, and a defensible one; inventing a
crafted source is not.

## Definition of done

The report prints `verdict: the economy closes`, `known_gaps()` is empty, and the
ratchet that compares both ways is what keeps it that way. **Tracks C and D are
where that stops being reachable by implementation alone** — the rows there close
by a decision about what this shard ships, and until one is taken the ratchet
carries them.
