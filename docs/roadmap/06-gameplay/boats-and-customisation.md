# Boats and house customisation

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../../../plans/roadmap/PLAN.md)

- `boats` — a multi that moves: a hull that blocks, a deck you can stand on,
  and everyone aboard arriving with it. **B1 and B2 built; B3–B4 and the tiller
  planned — see
  [`boats.md`](../../boats.md)**, which refuses a parent transform on the engine's own
  evidence (mounting *deletes* the mount rather than carrying it), keeps the hull
  out of `Obstructions` because that index only ever subtracts and a deck has to
  *add* a surface, and finds that `Feature::SmoothShip` already names `0xF6` and
  its 7.0.9.0 boundary with no packet behind it. It also supplies the repro the
  open pier/bridge defect below has been waiting for.
  - [x] **B1 — a ship on the water, moored.** `openshard-boats`, `.boat <multi
    id>`, `Terrain::land_is_water`, and the `Boats` index on `FacetState` that
    `LiveTerrain` consults as a third source. Saved at schema v32 and the berth
    recomputed at boot. Walking onto a deck lands you on it; walking into a hull
    does not. What the phase found is that `LiveTerrain` forwarded seven methods
    and answered the trait's no-client-files default for every other — a hole
    nothing had asked through until a boat did.
  - [x] **B2 — it moves.** `boats::step` decides then applies, the manifest is
    derived per move, each occupant is relocated absolutely through `move_to`,
    and the hull is redrawn by forget-then-reveal because no packet relocates a
    drawn item. `Sailing` holds the course, the tick's `sail_boats` steps every
    ship whose cadence is up on the reference's own intervals, and a ship whose
    way is blocked furls and its owner is told. `.sail <direction|stop> [fast]`
    steers. `two_boats_do_not_occupy_one_tile_when_one_is_under_way` is built,
    and what it caught is that the *berth* check would have refused a ship the
    right to move at all — every step overlaps the tiles it is leaving. **A move
    costs six packets** with one player aboard and one watching: two for the
    hull, and a `0x20` and a `0x77` for the passenger. B6's tiller is not built;
    `.sail` stands in for it.
  - **B3 — `0xF6`**, for the clients that can. Strictly additive.
  - **B4 — the boat as property**: the hold, the plank, the deed, decay.
    Housing's H2–H5 with a different noun.
- `customisation` — the `0xD7` house design system. **C1 and C2 built; C3–C4
  planned — see [`customisation.md`](../../customisation.md)**, which reverts housing's
  D7 in full. The decision it turned on was where a per-house component list
  lives: `Terrain::multi_components` cannot hold one — its only key is a `u16`,
  it returns a borrow out of `&self`, its store is fixed at boot, it is
  documented as deliberately not world state, and a synthetic multi id has no
  picture on any client. So a design is a `HouseDesign` component, saved as its
  own table at schema v31.
  - [x] **C1 — designs exist, and staff make them.** The seam and no editor: a
    house can be any shape, saved and restored, with `.hdesign <multi id>`
    copying an existing multi's components onto it. `0xBF 0x1D` and `0xD8` are
    written on both ends — `openshard-protocol`'s `design` module, the layout
    read out of `HouseFoundation.cs` — though nothing sends either yet. What the
    phase found is that a house's shape is read by four things holding a *house*
    rather than a multi id (the sign, the doors, the lockdown area, the walls the
    fall-down path removes), and two of them were already wrong for a designed
    house before one could exist.
  - [x] **C2 — a foundation is placeable.** Not by deleting the refusal: a
    foundation's own component list has no stairs, so one is placed *with* the
    initial design ServUO's `GetEmptyFoundation` derives — the platform, a floor
    around the perimeter, and a stair strip along a row the box is grown by. The
    refusal still stands where that design cannot be built, which is a shard with
    no client files or an id whose platform this install does not hold. The
    question it settled: the stair block is a **derivation**, not a per-house-type
    table. A player can own a foundation; reshaping it is C3's.
  - **C3 — the session**: enter and leave, build and erase, floor selection,
    commit and revert. The editor itself, on the `0xD7` subcommand set.
  - **C4 — roofs, backup and restore, and the validation.** ServUO's
    `HouseFoundation.Check*`, whose support-and-reachability half is deferred by
    name: a floating tower is cosmetic, not a hole in the shard.
