# Regions, guards, and the world clock

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

- [x] **Regions, guards and the world clock.** Two of the "never written down"
  gaps below, which turned out to be one slice: a place has to exist before
  anything can be true *there*.
  - **Regions** (`state::region`) are a facet's named areas — a name, a set of
    rectangles with a height band, and the few rules that hold inside them
    (`guarded`, `no_teleport`, `no_recall`, `no_housing`, `safe`), plus a music
    track and a light level. They live on `FacetState` beside the sector grid and
    the obstruction index, so two facets can never be confused for one another,
    and a coarse bucket grid finds them (the fine test is always
    rectangle-containment, so a wrong bucket can cost time and never an answer).
    **The nesting ServUO's data has is flattened where the data is written** — a
    child becomes a region of its own with a higher priority — so the engine holds
    a flat list and a number, walks no parent chain, and cannot build a cycle.
  - **A crossing is found, not announced.** A mobile moves through the player
    walk, the creature step, a teleport, a resurrection and a login, and a call
    beside each of those is five places to forget; so `tick/regions.rs` diffs each
    player's tile against the region they were last seen in (`InRegion`) once a
    tick — the shape `tick/status.rs` uses, and for the same reason. A crossing
    emits `RegionChanged` (both sides in one event, since a step out of one town
    and into another is one thing that happened) and starts the region's music
    (`0x6D`, new in `protocol`, Sphere and ServUO agreeing byte for byte). The
    music is compared before it is sent: re-sending the same track *restarts* it,
    so a player pacing a town line would hear the first bar over and over.
  - **The world has an hour** (`tick/ambient.rs`), derived from the tick counter
    at ServUO's rate (`Clock.SecondsPerUOMinute`, five real seconds to the UO
    minute — a UO day in two real hours), never from a wall clock, so it replays.
    `LightCycle.ComputeLevelFor` gives the curve: night until 04:00, a two-hour
    climb to full day, day until 22:00, a two-hour fall back. The `x / 16`
    longitude term is ServUO's and is not decoration — a map that flips to night
    in one instant reads as a light switch rather than a sunrise. **One pass
    sends `0x4F` for both reasons** (the sun moved, or someone walked into a
    cave), diffed per player, which retires the "Night Sight is a documented
    visual no-op" note: the precedence is Night Sight → the region's light → the
    hour. The season (`0xBC`) is a `[gameplay]` value sent on world entry, in
    ServUO's place in the login order (after the map change, before the player
    update).
  - **Guards** (`npc::guards`) are the consumer notoriety has been waiting for
    since it landed. ServUO's `WarriorGuard` is a *sentence, not a fight*: it
    materialises on the offender with the teleport sparkle and sound, says its
    line, and deals their whole hit point total through the one `combat::damage`
    door — so the corpse, the loot and `MobileDied` all happen the usual way. Two
    paths reach it: the "guards" keyword spoken inside a guarded region (the shape
    the banker's "bank" set), and a murderer *crossing into* one, off the
    `RegionChanged` event (ServUO's `GuardedRegion.OnEnter`). Candidacy is
    ServUO's `IsGuardCandidate` — a guard, a ghost, an invulnerable or a member of
    staff is never one, whatever they have done — and **a guard earns no murder
    count**, because executing the guilty is the whole of its purpose (ServUO says
    the same thing by clearing the guard's own `Criminal`/`Kills` every beat). It
    vanishes on a tick counter when its work is done.
  - **`no_teleport` has both ends.** `WorldState::may_teleport` is one predicate
    read by the staff `.tele` and the Teleport spell alike, and it refuses on the
    *origin* as well as the destination — a jail one can cast out of is not a
    jail. Staff pass, through `is_staff`, so `.gm off` puts a game master under
    the rule with everyone else.
  - **The data is the pack's, and it persists (schema v12).** The converter grew
    a pass over ServUO's `Data/Regions.xml` (129 Felucca regions: towns, dungeons,
    the jail, the moongates), mapping the region *type* to flags, `<music name>`
    to the client's `MusicName` index, and `<guards disabled="true"/>` to guards
    off. An `.admin` button sends `regions:felucca`; `op_register_regions` hands
    the whole facet over at once, replace-all like decoration and spawners.
    `RegionRecord` and the world clock ride in the snapshot, because without them
    a restart silently loses its guards, its music, the dark in its dungeons, and
    starts every night over. Two converter bugs worth remembering: `Number(null)`
    is **zero**, not `NaN`, which quietly made every rectangle one z-unit tall (a
    town nobody in a cellar was ever in); and a parent region's body *contains*
    its children's, so scanning it for rectangles gives the parent ground that
    belongs to the child.
  - Deferred: `0x65` weather, a calendar that turns the season, per-region light
    for creatures (only players are told), and the `safe` flag, which is carried
    in the data and waits on PvP rules to read it. (`no_recall` has its reader
    now — see **Travel** below.)
