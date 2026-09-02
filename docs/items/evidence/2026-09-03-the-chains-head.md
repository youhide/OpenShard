# The chain's head: a field to pick and a sheep to shear

> **This is a record.** It was written as part of `docs/crafting.md` and is kept
> as it was written. The model it describes as built is
> [`../design_crafting.md`](../design_crafting.md) — where the two differ, the
> design is right — and what is still open is ranked in
> [`../README.md`](../README.md), which is the only status page for this domain.
>
> **Its section numbers are that document's, not this file's.** §1–§4 are
> [`../design_crafting.md`](../design_crafting.md); §5, §6 and the numbered
> review are the three sibling records dated 2026-09-02 beside this one.

## 7. The chain's head (2026-09-03)

§6 left the chain standing on a shelf: cotton, flax and wool reached a player
from a vendor and nowhere else, so a tailor still *bought* the first link of
everything they made. Two of those three now grow.

```
[cotton field, 8 or 6 plants] ─ double-click ─► 1 cotton on the plant's tile
[live sheep in fleece] ─ blade ─► 2 wool, and a shorn sheep for two hours
```

- **A field is a spawn region for items.** `CropField` is a box, a crop and a
  ceiling, and `maintain_crops` runs beside `maintain_spawners` with the same
  rules: skip unless due, one plant per pass, level of detail holds a field
  nobody is near, every pick drawn from the seeded rng. Two of them, both
  ServUO's — Moonglow (`4557,1471`, 20×10, eight plants) and Skara Brae
  (`816,2344`, 16×24, six), read off the `<spawning>` blocks of `Regions.xml`
  rather than the spawner map, which is why they are `data/crops.json` and not a
  section of `spawns.json`: the converter that built that file has never seen an
  object spawn. Registering a field **plants it full**, ServUO's own `Respawn`
  on a region loading, so a shard that has just laid its world does not hand the
  first player to reach the farm a patch of bare soil.
- **Nothing about a field is saved**, and that is the one design decision here
  worth arguing over. A plant carries no field id, because there is nothing for
  an id to survive: it is world furniture the `populate:` verb lays, like the
  townsfolk that verb also re-places on every boot, so a restored plant would be
  a second copy of one the boot is about to sow. It is the *picked stub* that
  settles it — restored, a stub is a permanent bare furrow with no timer left to
  clear it, which is exactly why a spell's `Field` tile is excluded from the
  save on the line above. What a pick *paid* is an ordinary item on the ground
  and is saved like one. The cost is that a field counts what stands inside its
  box, so **no two fields of one crop may overlap** — `build.rs` refuses the data
  that would.
- **The shear is the blade's, not the scissors'.** UO lore says shears; ServUO
  wires a sheep as `ICarvable` and reaches it through `BladedItemTarget`, so a
  dagger shears and a pair of scissors does not, and this is that verbatim. The
  branch lives inside `carve` because upstream's *target* is one target. One
  thing there is load-bearing and was got wrong first: it must come **before**
  `carve`'s reach check, because `in_reach` answers where an *item* is and a
  mobile has no item location at all — asked about a sheep it says "too far
  away" whatever the distance, so the shear measures its own.
- **The fleece timer is not saved and the body is**, which is the wheel's
  bargain and needs the wheel's second half. `Shorn` is transient like
  `Spinning`; without anything else, a sheep saved shorn would come back shorn
  for ever with no timer left to regrow it — the wheel that turns for ever, one
  shelf over. `persist` stamps the woolly body back on restore for that reason.
  Nothing is lost with the timer: no one spent anything to shear, and the worst
  a restart pays is one early fleece.
- **Flax still has no field, and that is upstream's content rather than a gap
  here.** `Regions.xml` spawns `FarmableCotton` in two Felucca fields and
  `FarmableFlax` in none at all — the class exists and only the staff `[add`
  menu reaches it. So there is no `CropKind::Flax`: a crop nothing plants is the
  dead content `build.rs` already refuses of a creature no region spawns. Flax
  stays vendor stock, and it spins into the same thread cotton does, so nothing
  downstream is unreachable for want of it.
- **Numbers, all ServUO's.** Two wool on Felucca and one elsewhere (the era's
  own reward for shearing in the dangerous world, kept as a facet test rather
  than folded into the one facet that exists); two hours between fleeces; five
  minutes before a picked stub is taken away; one cotton per plant. The regrowth
  pace is the one place a range became a number: upstream draws each plant's
  wait between ten and thirty seconds, and one delay takes the middle.
- **One redraw, three callers.** Swapping a thing's art where it stands —
  forget, insert, reveal — was the spinning wheel's private trick and is now
  `items::redraw_item` and `items::redraw_body`, which the wheel, the picked
  furrow and the shorn sheep all go through.

Coverage: `a_cotton_plant_pays_cotton_once_and_stands_picked` (both packets,
and a second double-click on purpose — verified to fail with the picked
transition dropped, which is an unlimited cotton fountain at one click per
tick), `a_cotton_plant_cannot_be_lifted_out_of_the_field`,
`a_crop_field_plants_itself_full_and_regrows_what_was_picked`,
`a_field_of_cotton_is_not_saved_but_the_cotton_it_paid_is`,
`a_blade_shears_a_sheep_once_and_leaves_it_shorn`,
`a_blade_on_something_alive_that_is_not_a_sheep_takes_nothing`, and
`a_shorn_sheep_comes_back_in_fleece_after_a_restart` (verified to fail with the
body stamp removed — the sheep comes back `0xDF` and stays it).
