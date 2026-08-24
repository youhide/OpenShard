# Housing

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

- [x] `housing` — **built, H1–H5.** A multi placed from a deed, walls
  that stop you, a door and secures that know you, a sign that says who owns it
  and how it is wearing, and a house nobody visits collapsing into a crate that
  keeps what was inside. **See [`housing.md`](../../housing.md)**, which takes the
  eight decisions, records what each phase came out differently on, and names
  what stays deferred (customisation, boats).
  - [x] **The multis are read.** `openshard_uofiles::multi`, both formats. The
    picture was never the problem — a multi is one item that draws as many, and
    every client already owns every house, so the shard sends no component of
    one. What it has to read the same file *for* is the half the picture does not
    carry: where a wall is for the purpose of stopping somebody.

    Three things about the format are in [`findings.md`](../../findings.md) and cost
    the derivation. High Seas widened the component from 12 bytes to 16 and put
    nothing in the file to say so — `tiledata.mul`'s trap again, and the same
    arithmetic settles it. The flag that marks a drawn component runs **opposite
    ways** in `multi.mul` and `MultiCollection.uop`, with nothing in either to
    say so: read one backwards and both parsers look right while disagreeing
    about 309 of the 326 multis they share. And the two files are not the same
    size — 326 against 862 on one install — so the UOP wins, which is the
    *opposite* of `map0.mul`, where the stale file is zeroed and therefore loud.
  - [x] **H1 — a house on the ground.** `openshard-housing`, `.house <multi id>`,
    and the footprint folded into `Obstructions` at placement so the walls stop
    people. A house is an ordinary item entity whose graphic is `0x4000 | id`
    with a `House` component beside it, so the sector grid, the interest sweep
    and the `0x1A` that draws it all work on one unchanged.

    The components reach gameplay through **`Terrain::multi_components`**, which
    is the seam `item_blocks` and `item_height` already use — a multi's shape is
    a client-file fact like a static's height, and routing it the same way means
    `openshard-housing` depends on no file reader and a shard with no client
    files places no houses instead of needing a second answer.

    Only components the tiledata calls impassable are folded in, so a floor and
    a roof stay walkable; a house whose floor blocked would be sealed shut from
    the inside.

    ServUO's five placement rules are in, and two of them turned out to be one
    question: "nothing impassable in contact" and "the foundation rests on a
    surface" are both *is there an open gap with a floor here*, which `can_fit`
    already answers against the map's own statics. The road is a land-tile id
    against nine ranges — the rule a player notices the absence of, since without
    it houses go up in Britain's streets. The yard is measured wall to wall
    against the other house's footprint rather than a stored rectangle, and it is
    a square rather than the reference's front-and-back strip, because a classic
    multi carries no facing to measure a strip from.

    **Saved, schema v27** — the first bump that is not about *reading*. What is
    saved is where a house stands and which multi it is, never its components:
    those are a pure function of the id and live in the client's files, so a copy
    would go stale the day the operator updates their install. The footprint is
    recomputed at boot, and a restore deliberately skips the placement rules —
    a house legal when it was built stays built, or a shard that changed its yard
    size would demolish half of Britannia at the next restart. A v26 database
    reads fine and holds no houses; the bump exists so an *older build* cannot go
    on writing to a database whose houses it does not know about while handing out
    item serials one of them already holds.
  - [x] **H2 — the deed, and the cursor that draws the house.** `0x99`
    `MultiTargetRequest`, written from nothing on both ends because neither
    engine had it. It is the one packet in this plan whose *length* depends on
    the client — 26 bytes classic, 30 post-High-Seas — which put it outside
    `EncodePacket` entirely, since that trait's `LENGTH` is a const. The
    `OpenContainer` precedent, and the second member of that club.

    The deed rides on the `TargetPurpose` rather than the multi id, and that is
    a rule: the id can be read back off the deed when the click lands, so a deed
    sold, dropped or destroyed while the cursor was up does not still place a
    house, and a player with one deed and a fast hand cannot place two. The deed
    is spent on success and kept on a refusal.

    Our own client draws the house it is told about, which was a silent bug
    before this: `render::items::collect` had no notion of a multi, and a static
    id space running to `0x10000` means `0x4064` is a *valid* art id — so a villa
    drew as whatever static happened to sit there, with no error anywhere.
    `net_command::multi_pieces` expands it at the seam where the view becomes a
    draw list, so the renderer never learns what a multi is. It answers `None`
    and not an empty list with no table, because falling through to the ordinary
    item path is precisely the old bug. `parity.md`'s question was asked: every
    other `GroundItem` producer builds from the map's own statics or a fixture,
    and a placed house is not in the map file, so there is one call site.

    What is not drawn is the *preview* under the cursor. The packet is folded
    into `WorldView`; the picture is not.
  - [x] **H3 — who may come in.** Co-owners, friends and bans with ServUO's own
    limits, the door, the eviction, and the sign.

    **One question, not four booleans.** The reference's predicates are nested —
    `IsFriend` is `IsCoOwner(m) || …`, `IsCoOwner` is `IsOwner(m) || …` — so four
    independent answers are four chances to ask the wrong one. `Standing` is an
    ordered enum and `standing_of` is the only place the order of the checks
    lives; `Banned` is its *lowest* value, so a comparison reads "at least this
    trusted" and a ban is never that.

    `Standing` lives on the component in `openshard-state` rather than in the
    housing crate, because a *door* has to ask it and the double-click dispatch
    is `openshard-items`'. That is `Guild::at_war_with`'s split, and it is the
    answer the secure gate and the storage ceiling both took later.

    **A house adopts the doors standing inside it**, which is a rule this plan
    chose rather than inherited: three of `multi.mul`'s 326 multis carry a door
    component, and ServUO's own answer is a per-house-class `AddDoor` table this
    engine does not have. The adoption reads the *drawn* tiles and not the
    blocking footprint — a door stands in a doorway, which is by construction the
    one place the footprint does not reach.

    The sign's position is the one number the reference *derives* rather than
    declares: its classic houses each carry a hand-written `SetSign` offset, but
    a customisable one cannot, so `HouseFoundation` computes the box's
    west-south corner at z+7. Reduced against `Multi::center`'s own definition it
    is `(min_x, max_y)`, and it holds for every multi.

    Saved, schema **v28**, and the bump found a defect underneath: the house
    entity has a graphic and a position, so `ground_items` was sweeping it up as
    an `ItemRecord` *as well as* writing a `HouseRecord`.
  - [x] **H4 — lockdowns and secures.** One component and not two, because a
    secure *is* a lockdown: neither lifts, releasing works on both, both count
    against one allowance. The access level is a `Standing` because ServUO's
    `SecureLevel` is the trusted half of `Standing` with a fourth name for its
    bottom — and `Stranger` being "anyone" means a banned player is still below
    it, which a separate four-value enum would have had to remember to give.

    **The allowance is derived from the multi's area**, not tabled. ServUO
    carries a lockdown count per multi id beside the price and the placement
    offset; plotted against each house class's own `Area` rectangles that table
    is close to linear (212 over 52 tiles, 290 over 59, 550 over 125), so four
    per tile lands within a sixth on every row. It is computed at placement and
    stored on the component — D2 one level up — because the path that needs it is
    the drop into a secure, which has no terrain in hand.

    Saved, schema **v29**, the sharpest of the three house bumps: this one is not
    a list on the house but a component on every pinned *item*, so an older build
    reads them as ground clutter and writes them back unpinned.
  - [x] **H5 — decay, and the crate.** Six stages at `GetOldDecayLevel`'s own
    thresholds, the sign as the refresh, demolition by the owner or by staff, and
    one crate holding everything the house was keeping.

    **The clock is an accumulator, and it is the only one in this engine.**
    `Decays` and `MurderDecay` are an absolute `at_tick`, which works because
    they are minutes long and die with the process. A house's is five days, and
    `WorldState::ticks` starts at zero every boot — the world saves a clock in UO
    minutes, not a tick count — so a deadline would mean nothing on the way back
    in and every house would come up freshly refreshed. D6 said "a tick count,
    not a wall clock" and still holds; what it did not say is which end of the
    interval to store.

    The crate does not decay and nothing collects it, which is stated rather than
    left to be discovered: ServUO internalises its own to the owner's bank after
    three hours, and a crate that rotted would be a shard that eats somebody's
    belongings on the day their house came down.

    Saved, schema **v30** — v27's case again, a bump for the *writer*: an older
    build ignores `houses.age` and writes every house back at the default, so
    nothing on the shard ever collapses again.
  - [x] **H6 — the region a house stands in.** The sixth phase of a five-phase
    plan, and half of it was a correction: three things this plan published as
    decided were never built, and they were one thing — housing and regions never
    met.

    **`no_housing` has a reader**, and twenty-one shipped dungeons close on the
    first boot: Covetous, Deceit, Despise, Destard, Hythloth, Shame, Wrong,
    Khaldun, Terathan Keep, Fire, Ice, the Solen Hives and nine more. The rule is
    stated over every tile the house *covers* rather than its origin — and the
    argument that decided it is not the boundary case but a blunter one: `at` is
    the multi's origin and "is not the corner of its box", so a multi whose
    components all sit at positive offsets has an origin outside its own drawn
    area, and an origin test can test a tile no wall stands on. At the house's own
    z rather than each component's, because 247 shipped rects carry a height band
    and a villa's roof would otherwise read as outside the dungeon its foundation
    is in. And **first among the judgements**, because every other refusal here
    means "try a tile over" — `Occupied` as much as `BadGround` — and inside
    Deceit that is a lie a player spends ten minutes proving.

    **`place` takes an actor**, so D3's "staff place anywhere" is true for the
    first time since H1. Not the reference's single early return: this engine's
    `Refusal` mixes judgements about the plot with facts about the id, and
    skipping the second kind would let a game master place a foundation with no
    stairs — the exact failure `NeedsCustomisation` exists to prevent.

    **D11 is blocked and stays deferred.** A house registering its own region
    needs `Regions` to accept a runtime insert and remove, and it has neither:
    `set` is replace-all *by design*, `RegionId` is a `Vec` index that `at()`
    indexes unchecked, the save sweep would write the derived region and outlive
    the house with it, and — decisively — `restore_houses` runs seven lines
    before `restore_regions`, whose `set` would wipe it on every boot. It needs a
    decision about the type's shape, which is D4's lesson arriving a second time
    one level down.
