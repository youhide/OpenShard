# The inscription trade, and the runebook that had no source

> **This is a record.** It says what landed on 2026-09-03 and why. The model as
> built is [`../design_crafting.md`](../design_crafting.md) — where the two
> differ, the design is right — and what is still open is ranked in
> [`../README.md`](../README.md), the only status page for this domain.

## What was wrong

A **runebook was unmakeable**. Sixteen bound destinations, a charge counter, a
recharge path, a window with Recall and Gate Travel buttons, and a schema version
carrying all of it — and the only way one could exist on the shard was a staff
verb. No vendor sold one, no creature dropped one, no recipe made one. That is
the shape of defect the crafting review recorded as #11 for leather, one system
over: a feature whose whole supply is a GM.

ServUO makes one with Inscription, which was also one of the four `Def*` tables
this engine had never ported. So the fix for the runebook is the trade.

## The trade

`inscription` is the eighth `SystemRow`: skill `Inscribe`, floor 0, the default
flat-sixty exceptional curve (alchemy's, and now no longer alchemy's alone), no
workshop, sound `0x0249`, tool the scribe's pen `0x0FBF`/`0x0FC0`. **66 rows of
DefInscription's 72**: the sixty-four Magery scrolls, the runebook, the
spellbook.

```
reagents + blank scroll ─► [pen, and the mana the spell costs] ─► spell scroll
8 blank scrolls + Recall scroll + Gate Travel scroll ─► [pen] ─► runebook
```

The runebook row is the first in the whole catalogue whose ingredients are other
rows of **its own trade**: a scribe writes the two travel scrolls, then binds
them into the book. Nothing else on the shard makes either.

What was dropped is content this engine has not reached, not rows worth shipping
inert: sixteen necromancy scrolls (no such spells here, and their reagents grow
nowhere), the Mondain's Legacy artifact books, and the enchanted switch and runed
prism, whose own materials do not exist. The rule is stated as a predicate rather
than a list of names — a Magery scroll is exactly a row whose art falls in the
run the spellbook reads — so a re-port against a newer ServUO keeps the same
boundary without anybody re-deciding it.

## Two mechanisms no other trade has

- **A scroll costs mana**, ServUO's `SetManaReq`, which is the spell's own casting
  cost: four for the first circle up to fifty for the eighth. `Recipe::mana` is
  the only new column, checked with every other gate at both ends of the craft
  and **spent only when the item is actually made** — a failed roll ruins the
  scroll and costs no mana, and a refusal costs neither. Zero means "no
  requirement" rather than "free", so a crafter with no mana pool at all — an NPC
  smith, a creature — is refused nothing by it.
- **A scribe may only write down a spell they have**, ServUO's
  `DefInscription.CanCraft` (cliloc 1042404). This needed **no** new column: the
  art of a Magery scroll names its spell, so the gate reads the row's own output.
  A column beside it would have been a second place to be wrong. The search — a
  spellbook in your own pack with the bit set — moved down into
  `items::carries_spell`, because casting asks the identical question and two
  copies would be two answers the day one of them learns about a book worn on the
  hand.

Both refusals cost nothing and are checked twice, which is the crate's existing
rule and not a new one.

The **five-line ceiling** is the other thing that moved: `MAX_CRAFT_RESOURCE_LINES`
was four, and fourteen scrolls want four reagents plus the blank scroll they are
written on. Four held while the seven material trades were the whole catalogue.

## The runebook's charges

ServUO's `Runebook.OnCraft` is `5 + quality + Inscribe/30`, capped at ten, with
quality 1 ordinary and 2 exceptional — so a grandmaster's book is nine charges,
or ten when the roll comes out exceptional. A book nobody crafted keeps the flat
six `items::apply_core_defaults` gives it: a vendor's book is nobody's work.

**One deliberate divergence.** Upstream sets `MaxCharges` and leaves the current
count at zero, so a new book there is empty until Recall scrolls are dropped on
it. This engine hands a made book its charges the same way it already hands a
shelf book its six — the two rules side by side would mean a bought book that
works and a made one that does not.

## Three things found on the way

Each was found by a test written for this slice, and each was a live defect
before it.

- **The first circle of scroll arts is not in spell order.** The run opens on
  Reactive Armor (`0x1F2D`, spell **6**), and only then goes Clumsy, Create Food,
  Feeblemind, Heal, Magic Arrow, Night Sight (`0x1F2E`–`0x1F33`, spells 0–5);
  Weaken is back in step at `0x1F34`, and from Agility on, position and spell id
  are the same number. `spell_scroll_graphic` was `0x1F2D + spell`, which is
  right for fifty-seven of the sixty-four spells and silently wrong for six: a
  Reactive Armor scroll dropped on a spellbook taught **Clumsy**, a Clumsy scroll
  taught Create Food, and so on up the circle. Recall (spell 31) sits above the
  rotation, which is why the runebook's recharge never noticed and why the bug
  had survived. Now a table both ways, with every pair read off a ServUO
  `SpellScroll(spellID, itemID)` constructor.
- **Fourteen spells were cast with the wrong reagents.** The scroll rows and the
  spell table were carried out of the same C# by hand into different crates, and
  nothing compared them. Cunning wanted ginseng where it wants nightshade;
  Protection, Arch Protection and Mass Curse each carried a spider's silk that
  belongs to nobody; Lightning carried a black pearl it does not want; Mana
  Drain, Mind Blast, Paralyze Field, Energy Field, Energy Vortex, Earthquake and
  three more each had one or two reagents transposed. All fourteen are now
  ServUO's, and the agreement is asserted rather than assumed — the same shape of
  slip the Gate Travel row had, which is on record two slices back.
- **Two vendors handed over a picture instead of the item.** ServUO's `SBMage`
  and `SBRealEstateBroker` display a blank scroll as `0x0E34` while the item they
  give is `BlankScroll`, `0x0EF3`; our shelves copied the display art, so the
  scrolls a scribe buys where scribes shop could not be written on. The mapmaker
  already sold the right one.

## Coverage

- `a_scribe_writes_a_recall_scroll_and_pays_the_spell_s_own_mana` — the whole
  path through the world, with the mana pool pinned exactly (it starts full, so
  the trickle that runs later in the same tick cannot add a stray point).
- `a_scribe_cannot_write_down_a_spell_their_own_book_has_not_got` — refused for
  nothing, with a book holding the spells either side of the one asked for, so
  the refusal is about the bit and not about an empty book.
- `a_scribe_without_the_mana_is_refused_and_keeps_the_reagents`.
- `a_scribe_binds_a_runebook_with_the_charges_their_skill_earns` — reads the
  quality off the made book rather than pinning one number, because which of the
  two it is belongs to the rng.
- `a_scroll_row_asks_for_the_spell_s_own_reagents_and_mana` — the cross-crate
  agreement, reporting **every** disagreement rather than the first: a
  permutation between two hand-carried tables is never one row. This is what
  found the fourteen.
- `every_magery_spell_has_exactly_one_scroll_row_and_only_inscription_writes_one`
  — both halves of the rule, the second because the spell gate reads an output
  art: a row of another trade landing in that run would start demanding a
  spellbook and a pool of mana.
- `the_runebook_row_binds_the_two_scrolls_it_travels_by`.
- `the_first_circle_of_scrolls_is_drawn_out_of_spell_order` and
  `every_scroll_art_is_one_spell_and_the_run_holds_all_of_them` — the rotation,
  pinned pair by pair and then checked as a bijection, since a rotation written
  down twice is a rotation that gets subtly wrong the second time.

## The tool that writes the tables

`tools/gen-craft-tables` learned `DefInscription`, which is the one table that
does not write its rows as `AddCraft`: the scrolls go through an `AddSpell`
helper reading two fields set between the circles. Those calls are rewritten into
the statements the parser already understands, so there is one parser and not
two, and the skill bands still come out of the C#'s own switch rather than out of
a transcription.

It also grew a guard it should always have had. The committed tables are edited
as data after they are generated — typed rows, addon rows, the hand-written dough
row — and a first run for a **new** trade rewrote all seven existing ones,
throwing that away. It now writes only the file that is missing; a deliberate
re-port against a newer ServUO says `--force` and reads the diff.

## What is still open

- **The other three tables**: glassblowing, masonry, cartography.
- **Necromancy and mysticism scrolls**, which wait on their spell schools.
- **Inscription's own use button.** The skill is pressable from the window —
  ServUO gives `SkillName.Inscribe` a callback, "target the book you wish to
  copy" — and this engine has no writable books, so pressing it announces the
  skill and does nothing. It is the one craft skill that is both pressable and a
  trade, which is why the tool table's test asks about the other seven.
