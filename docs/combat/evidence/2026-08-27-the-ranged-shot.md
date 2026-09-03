# Archery — arrows, ranged combat, and the flight itself

*A record, not a status. It names the documents by the names they had when it was
written: `docs/combat.md` is now [`design_fight_loop.md`](../design_fight_loop.md)
and `docs/combat_actions.md` is [`design_actions.md`](../design_actions.md). What
is open about ranged combat is ranked in [`README.md`](../README.md).*

> **`volleys()` no longer exists.** This document is the record of how the ranged
> path was built and why; [`combat_actions.md`](../design_actions.md)'s Ф2 retired
> that function into the three passes every action now runs through, so a shot is
> committed, sustained and resolved exactly like a blow. Everything below about
> *what* a shot does — reach off the weapon row, ammunition, the `0x70` flight —
> still holds; only the place it happens moved. Two rules it wrote were changed
> there and are named in that phase: a shot inside `MELEE_RANGE` is now fired
> rather than refused, and the round is drawn at the loose rather than tested and
> spent in the same tick.

Archery looked half-built and was actually three-quarters built: `WeaponKind::Ranged`,
the Archery skill, the whole weapon-data table for bow/crossbow/heavy crossbow, and
even the player's own nock-and-loose *animation* already existed and were already
wired correctly — `equipped_weapon_animation` in `runtime.rs` resolves `ShootBow`/
`ShootCrossbow` per attacker on every `animate()` call, no matter which combat
function put the swing in motion. What was missing were three specific, unrelated
gaps that together made archery a skin on wrestling rather than a fight at range:

1. **Range.** `swings()` — the only combat function that ever ran for a player —
   always required `MELEE_RANGE`. `volleys()`, the function that resolves a ranged
   attack (reach, line-of-sight, a projectile, damage), only ever fired for the
   handful of NPCs a spawner explicitly attached a `RangedAttack` component to. A
   player who equipped a bow got the bow animation and nothing else — they still had
   to stand next to their target.
2. **Ammo.** There was no arrow or bolt item anywhere in the codebase. A shot, if it
   could be fired, cost nothing.
3. **The flight itself.** `volleys()` already emitted a `GraphicalEffect` (`0x70`,
   `EffectKind::Moving`) for every NPC ranged shot — but `0x70` had an
   `EncodePacket` impl and no `DecodePacket` impl anywhere, and the client had zero
   references to `GraphicalEffect`/`HuedEffect`/`EffectKind`. `docs/combat.md` names
   this exclusion explicitly ("the spell effects `0x70`/`0xC0`... none of them is on
   the loop this plan is about"). So even NPC archers fired silent, invisible
   arrows.

All three are closed. What follows is the record: the decisions taken, and where
each one landed once the code was in front of it.

## Decisions

**D1 — No component mirrored onto players.** `RangedAttack` stays exactly what it
was: spawner-authored data for a creature's *innate* ranged attack (a scripted
archer skeleton, a breath weapon). A mobile wielding an actual `WeaponKind::Ranged`
item gets its range and ammo derived fresh, every tick, from
`combat::weapons::equipped_weapon` — the same read-site derivation the melee path
already uses ("read fresh each swing, no mirror, so unequipping reverts them with
nothing to undo" is `equipped_weapon`'s own doc comment).

**D2 — `volleys()` grew a second admission branch, not a parallel copy.** It
already selected `Combat` holders carrying a `RangedAttack` component. It gained an
`else`: no `RangedAttack`, but `equipped_weapon(state, attacker).kind ==
WeaponKind::Ranged` → the same range/LOS/hit-roll/damage/effect pipeline, fed from
the weapon table instead of the component. One function still owns "loose a shot."

**D3 — Ammo is checked where the shot is about to leave, and a miss on ammo still
costs the swing timer.** The gate sits right after the existing `sight_clear` check,
before `combat.schedule_swing`. No arrows → a system message ("You do not have
enough arrows." / "...bolts.") and the timer still advances, the same as a whiffed
hit-roll — otherwise an empty quiver would retry every tick instead of once per
swing interval. Ammo present → consume exactly one, then the existing
hit-roll/damage/sound/effect code runs unchanged.

**D4 — Ammo and the effect's own art come from the weapon table.** `WeaponData`
gained three columns, populated only on the three `Ranged` rows:
`ammo: Option<Graphic>` (Arrow `0x0F3F` for the bow; Bolt `0x1BFB` for crossbow and
heavy crossbow — ServUO's `Arrow`/`Bolt` graphics), `effect_art: Option<Graphic>`
(`0x0F42` for the bow, `0x1BFE` for the crossbows — ServUO's per-weapon
`EffectID`), and `range: Option<RangedRange>` (ten tiles for the bow, eight for
both crossbows — ServUO's `DefMaxRange`; the bow genuinely outreaches a crossbow,
so this could not be one shared constant). `None` on every melee row: a melee
weapon has no ammo concept at all, which is exactly the case `Option` is for, not
"unknown."

**D5 — Ammo began as loot + vendor stock; Fletching landed later.** The archery
slice deliberately did not widen into crafting. The later Fletching port added
the material chain, fletcher's tools and recipes for arrows, bolts and the three
classic ranged weapons. Loot and bowyer stock remain alternative sources rather
than special prerequisites for ranged combat.

**D6 — The flying arrow is client state, not view state.** Same rule P3 gave `0x6E`
in `docs/combat.md`: it is an event, not a fact to keep redrawing from. It does not
join `WorldView` — it lands in `PresentationWorld::effects`, aged and culled every
frame the way `damage_numbers` are, fed through the same `link.rs` "packet the app
acts on rather than stores" seam `0x6E` already uses. An effect has no serial and no
persistent identity; it is spawned by one packet and dies on its own clock.

**D7 — Interpolation is linear in world space, then projected every frame.** The
packet's `from_point`/`to_point` are exact world tile coordinates. The client's own
travel time is a *chosen feel* (`EFFECT_TILES_PER_SECOND = 15.0`), not a ported
number: ServUO's `speed` byte and ClassicUO's own real-time pacing
(`MovingEffect.IntervalInMs`) are both expressed in that client's isometric
screen-pixel space, which has no honest conversion into this renderer's world tiles
without porting its skewed offset arithmetic wholesale. A bow's ten-tile max range
arriving in under a second reads as the fast, near-instant arrow real UO shows.

## What is drawn, and how

The arrow is the one sprite in this renderer that is not tile-snapped —
`client/render/src/effects.rs`'s whole reason to exist. It rides the static atlas
exactly as a ground item does (`0x0F42`/`0x1BFE` are ordinary item graphics), but
skips `items::collect`'s occlusion-aware walk entirely: there are at most a handful
of these on screen at once, none is an occluder, and none is a thing a click can
land on (`Place::NOWHERE`, `OwnerId::NONE`). Position comes from linearly
interpolating `WorldSpot::centre(from)` toward `WorldSpot::centre(to)` by the
effect's own `progress()`, then the ordinary `project_exact` → `camera.snap` →
`to_view_exact` pipeline every other sprite in this renderer already goes through.
Depth sorting reuses `depth::mobile_priority_z` — a shot in the air rises one above
the ground under it for the same reason a mobile does: it is a thing standing over
the tile, not a marking on it.

The effect sprite rotates clockwise in screen space to face its projected travel
vector, using the same `atan2(-offset.Y, -offset.X)` convention as ClassicUO's
`AngleToTarget`. Its quad expands to the rotated picture's bounding rectangle and
the static shader inverse-rotates sampling coordinates, so the arrow remains
centred on its flight path without adding a rotation field to all world sprites.

## What this does not cover

- **Expansion ranged weapons and fukiya darts.** They remain outside the craft
  table until combat has gameplay rows for them; otherwise crafting would create
  decorative props that cannot attack.
- **`HuedEffect`/`0xC0`** stays undecoded. Nothing sends it yet, and archery has no
  need of a tinted effect.
- **A visible ammo count or "out of arrows" icon.** The system message is the only
  feedback a player gets today.
- **`swings()` (melee) still hardcodes `DamageType::Physical`** regardless of weapon
  kind — pre-existing, untouched by this work.
