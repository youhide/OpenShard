# Closing the production graph

`openshard_world::economy` asks one question — can every resource some step wants
be produced by some other step, starting from the sources — and as of 2026-09-03
the answer is **no**: 56 resources are unreachable, 1,213 recipe rows can never
run, and 9 raw materials are paid out that no trade spends.

The audit and its ratchet are built; what is open is the content behind them.
This page is the order to close it in. The report is the measurement:

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

## The order

The first two steps share one missing mechanism, which is why they are first and
why they are one step apart rather than one step: **a bridge that is code rather
than a recipe row**. The engine has three of them (smelting and the two cuts of
the scissors, declared in `economy::CONVERSIONS`) and needs three more.

- [ ] **1. An axe on a log makes boards.** Seven grades of log come off a tree
      (`state/src/harvest.rs`, `WOODS`) and seven grades of board are spent by
      every carpentry, fletching and tinkering row, and nothing turns one into
      the other. **1,213 of the report's stalled rows are this one gap** — by a
      wide margin the largest thing on this page.

      Upstream is *not* `BaseLog.OnDoubleClick`, which is what
      `docs/roadmap/backlog/gameplay.md` claimed and this plan corrects: the
      conversion is `IAxe.Axe`, reached through the lumberjack's own harvest
      cursor (`Services/Harvest/Core/HarvestTarget.cs`) — double-click an axe,
      click a log in your pack, get one board per log, sound `0x13E`. The gate is
      `Carpentry >= n || Lumberjacking >= n` with `n` per wood: regular 0, oak
      65, ash 80, yew 95, heartwood, bloodwood and frostwood 100 (`Log.cs`), and
      refusal is 1072652 "You cannot work this strange and unusual wood."

      Lands beside `crafting::smelt`, which is the same shape of bridge (a
      harvested pile a trade cannot spend until code converts it), and is
      declared in `CONVERSIONS` from the new module's own public constants.

      Closes: `board (36)` in all six special grades, and the plain one stops
      depending on a vendor stocking twenty.

- [ ] **2. A sack of flour opens.** The mill row makes `SackFlour` (`0x1039`) and
      every dough row eats `SackFlourOpen` (`0x103A`); upstream's bridge between
      them is `SackFlour.OnDoubleClick` (`Items/Consumables/Cooking.cs`), which
      drops **one** open sack where the stack was and spends one from it.

      The same class of gap as step 1, found by the same report, and the reason
      the cooking chain has two holes rather than one.

- [ ] **3. A field grows wheat.** `crops.json` grows cotton and only cotton, so
      no sheaf of wheat (`0x1EBD`) exists and the mill row above has nothing to
      grind. `CropKind` gains a variant with its standing arts, its picked art
      and its yield, and the world data gains fields.

      With steps 2 and 3 the whole cooking chain becomes reachable: wheat →
      flour → open flour → dough → everything `DefCooking` builds on it.

- [ ] **4. A blade on a fish makes steaks.** `harvest::FISHES` pays `0x09CC` and
      nothing spends it, while the cooking rows for raw and cooked fish steaks
      (`0x097A`, `0x097B`) already exist and are unreachable. Upstream, `Fish` is
      `ICarvable` and cuts into four raw steaks; the branch belongs in
      `items::carve` beside the living-mobile-is-shorn branch, which is the same
      dispatch upstream splits the same way.

- [ ] **5. The carving table gains what a body is worth.** Two rows, both of
      which `items::carve`'s own doc comment already describes as missing:

      - **Horned and barbed hides.** Tailoring spends both grades and no carvable
        body wears them; dragons (body 12) and drakes (60) are already spawned in
        `spawns.json`, so this is `carved_yield` and `hide_grade_of` rows, not
        content.
      - **Tainted wool** (`0x101F`), which the spinning wheel already knows how
        to take. Upstream pays it for *carving a woolly corpse*
        (`BaseCreature.OnCarve`) — shearing a live sheep gives ordinary wool —
        so `CarvedYield` gains a wool column rather than the shard gaining a
        lich's flock.

- [ ] **6. Undead carry a bone.** The tailor's bone armour rows spend `0x0F7E`
      and nothing on the shard makes one. Upstream it is loot, not butchery
      (`BaseCreature.PackItem(new Bone())` on the undead), so it is a `loot.json`
      row on bodies that are already spawned.

- [ ] **7. Glassblowing.** `harvest::SAND` shipped ahead of the trade that spends
      it, so a miner can fill a pack with something no recipe wants. The trade
      itself is missing: a system, its rows, its tool and its heat source. The
      only step on this page that is a build rather than a table.

- [ ] **8. The twenty-two Mondain's Legacy ingredients.** `0x3183`–`0x3199` is one
      contiguous run of ML special ingredients (`1032…` name clilocs), and
      upstream pays every one of them out of Heartwood quest turn-ins and
      champion drops — this shard has neither. The largest single group in the
      report after the boards, and the one that is a decision per row before it
      is an implementation: a loot line on bodies that already spawn, a vendor
      shelf, or the rows deleted as unshippable.

- [ ] **9. The remainder, row by row.** What is left of the report once the steps
      above land, each of which wants its own verdict rather than a shared one:

      | Art | What it is | Upstream source |
      |---|---|---|
      | `0x0EF0` | silver (1044572), 250–500 a row | faction currency; 45 steps want it |
      | `0x1879` | copper wire (1026265) | the Mad Scientist quest, and faction tinkering |
      | `0x14F8`, `0x1374` | rope (1020934), and a hitching row's own | quest statics and vendors |
      | `0x315A` | pristine dread horn (1032634) | a peerless boss |
      | `0x0F8A`, `0x0F8F` | two alchemy reagents | reagent vendors and spawns |
      | `0x0F7C`, `0x15F8`, `0x171F`, `0x1042`, `0x1044`, `0x1083`, `0x103F` | cooking oddments behind the chain in steps 2–3 | re-measure after step 3: some of these close on their own |
      | `0x1E25`, `0x2F57`, `0x2F5C`, `0x4005`, `0x573B` | carpentry and alchemy leaves | vendor or deletion |

      Step 3 is what makes this list honest: several of its cooking rows are
      unreachable only because the chain above them is cut, and re-running the
      report is cheaper than reasoning about which.

## Definition of done

The report prints `verdict: the economy closes`, `known_gaps()` is empty, and the
ratchet test that compares both ways is what keeps it that way.
