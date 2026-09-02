# AI

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../../../plans/roadmap/PLAN.md)

- [x] `ai` — brains, aggro, wandering
  - [x] **A built-in brain, and room for scripted ones.** A creature spawned with
    a `sight` or `wander` gets a `Brain`, and `think()` gives it a beat every so
    often (not every tick): it notices the nearest player within sight and takes
    a `Combat` aimed at them — so `swings()` attacks it with exactly the machinery
    a player fights with — chases when out of reach, drops a target that dies or
    flees, and drifts when idle. The decision uses the world's `Rng`, so a fight
    replays. Aggro range and wandering are spawn data (`op_spawn_mobile` grew
    `sight`/`wander`), the script-first knobs.
  - [x] **The fully script-driven brain** — the per-mobile `onTick` the scripting
    benchmark sized. A mobile carries a `Scripted` marker; the built-in `think`
    skips anything wearing it, and the server calls that mobile's `onTick` every
    tick instead. A script takes control with `op_control` — which it can only do
    once it knows a serial, so spawning a mobile emits `MobileSpawned`, delivered
    like every other domain event. The built-in `ai` and a scripted brain are the
    two paths, and a mobile is on one or the other, never fought over by both.
  - [x] **Creatures behave like the references say they should.** Movement sees
    the live world: each facet carries an obstruction index of shut doors and
    impassable decoration, and `LiveTerrain` lays it over the map for every walk,
    step and A\* plan — a closed door blocks players and NPCs alike. Aggro needs
    **line of sight** (`Terrain::sight_clear`, a Bresenham ray; windows pass,
    walls and NO_SHOOT statics do not, shut doors are opaque). A chase walks
    naive-step-first, plans once when blocked, follows a **cached `Route`**, and
    on an impossible route **gives up** — target dropped, ~10s standing guard,
    then back to its life; never the fence-shuffle. Every body that walks
    somewhere keeps its route now, and **how long it is kept is what it is
    walking toward** (`ai::Goal`): a body — a quarry, an owner, an escorter — is
    a guess, so the route to one is re-planned on the references' 2s cadence; a
    *place* does not move, so the route to one is walked to its end and a
    townsperson's minute-long walk home costs one search rather than one every
    two seconds. The other three ways a route ends are the world's and apply to
    both: the body is not standing where the next step starts, the goal drifted,
    or the live ground refuses the step. Humanoids (`body_opens_doors`) open
    unlocked doors in their way — townsfolk heading home, a chaser, and a pet
    whose body has hands, all through the one rule (`ai`'s `way_ahead`), which
    is applied on whichever step of a route meets the door. Creatures carry an
    `Aggression` posture (passive
    fauna flee when struck; defensive ones answer the first blow via
    `ai::retaliate`; aggressive ones hunt on sight), break off badly hurt unless
    too big to scare, and step at `gameplay.creature_step_ms` (400 classic — a
    running player outruns a base monster on purpose), each spawn able to
    override its beat.
  - [x] **Ranged creatures volley and kite.** A spawn with `ranged` reach fires
    through `combat::volleys` — typed damage, LOS-gated, sharing the swing timer —
    and keeps its distance at `KITE_GAP` instead of walking into melee.
  - [x] **Level of detail — the AI dozes where no one is watching.** `think` is
    the tick's most expensive per-mobile work: for every `Brain` it runs
    `ai::think_one`, which scans sectors, casts a Bresenham line of sight and
    plans a path. In a populated world most creatures are nowhere near a player,
    and no one sees what they do — so an opt-in `[gameplay]` flag skips that cost
    for them. When `lod` is on, a creature with no player within `lod_radius`
    tiles (and not already in a fight — a fight must not freeze because the target
    stepped a tile away) does not think this beat; its next think is pushed out by
    `lod_idle_factor`, and it wakes the instant a player comes within range. The
    gate leans on a new `WorldState::any_player_near`, cheap because players are
    few (it walks the player table, not the sector grid). `lod_radius` sits above
    the view range and the largest sight, so a creature a player can see is never
    dozed — "no player near" implies "no player in sight", so nothing is missed by
    skipping. Off by default; a shard turns it on to trade a little off-screen
    liveliness for tick budget. Determinism holds — the gate reads only
    `state.ticks` and positions, never a clock.

    The numbers (`cargo run -p openshard-world --example lod_bench --release`,
    Apple-silicon dev machine, release, 5 players clustered in one corner and
    creatures spread across a wide square — the lopsided load LOD is for; 81
    creatures fall within the radius and stay awake):

    | creatures | LOD off | LOD on | speedup |
    |---|---|---|---|
    | 2,000 | 0.44 ms/tick | 0.04 ms/tick | ~12× |
    | 10,000 | 2.23 ms/tick | 0.09 ms/tick | ~25× |

    The gain scales with how much of the world is idle: the awake set is fixed by
    the players, so ten thousand creatures cost barely more than two thousand once
    the frontier dozes. The benchmark is also the project's first whole-`tick`
    timing harness — the scripting one measured a script call in isolation.
  - [x] **A whole Felucca to run it against.** The `.admin` menu grew a
    **Populate Felucca** and **Decorate Felucca** button (verbs `populate:felucca`
    / `decorate:felucca`), and the Community Pack answers them from
    `felucca/_generated/` — ~1,400 monster spawn regions and ~18,400 statics /
    ~640 doors / ~5,600 containers laying the whole facet in one click. The data
    is not hand-entered: a one-shot converter (`tools/convert-servuo.cjs`, the
    "build tool, not an engine feature" the scriptpack note calls for) reads a
    ServUO checkout — `Spawns/felucca.xml` for the spawns, resolving creature
    class names to body ids by scraping `Body`/`SetHits`/`Karma` out of
    `Scripts/Mobiles`, and `Data/Decoration/**.cfg` for the deco, classifying each
    entry by class name (door offsets from ServUO's `BaseDoor` facing table). It
    also generates the town **vendors** — the `Vendors`/`TownsPeople` regions the
    spawn pass skips, placed with a body, dress and shop stock curated per
    profession in `tools/vendor-data.cjs` — and the shop **signs** (`signs.cfg`,
    its own flat format). At full population that is on the order of ten thousand
    creatures across the map — exactly the load the LOD numbers above are drawn
    from, and the reason it was built first.
  - [x] **And a full facet no longer freezes the tick to populate.** Laying ~1,400
    spawn regions at once exposed two costs a small world hid. First, every region
    started due the same tick — a thundering herd — so `register_spawner` now
    **jitters** each fresh region's first spawn across its respawn window (a
    restored region keeps its saved timer). Second, and worse, `maintain_spawners`
    counted each region's live members by scanning *all* creatures, O(regions ×
    creatures) — millions of comparisons a tick, the freeze itself. It now tallies
    every region in **one sweep** (a `HashMap<id, count>`), O(regions + creatures).
    And LOD reaches spawners too: with `lod` on, a region **no player is near is
    left dormant** — its timer held, nothing spawned — until someone approaches,
    the standard "smart spawning". The three together turn a whole-facet Populate
    from a stall into a shrug.
  - [x] **Body-type tables** — ServUO's `Data/bodyTable.cfg` is ported
    (`state::components::body_type`), so `body_opens_doors` is its rule verbatim
    (`!Body.IsAnimal && !Body.IsSea`) rather than a list of eight human ids, and
    rideability is derived from the `BaseMount` subclasses — thirty bodies, with
    `mount_body_for` derived from the same table rather than kept as a second
    hand-written half.
  - **Path to a tile *adjacent* to the quarry** rather than onto it — the
    remaining refinement from the A\* work; today a chase plans onto the target's
    own tile and stops one short by the reach check.
