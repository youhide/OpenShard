# The chat and world-administration phase

*The roadmap's own record of the widest of the gameplay phases: speech and who
hears it, the staff command layer, the `.admin` gump and the pack behind it, and
the townsfolk that turned a populated facet from props into people. A record, not
a status — and a record of more than one domain, because the slice that gave a
town its shopkeepers also gave a backpack its ceiling, a trade window its escrow
and a chase its A\*. What is built and what is open today is
[`README.md`](../README.md); the model is
[`design_speech.md`](../design_speech.md) and
[`design_townsfolk.md`](../design_townsfolk.md), and the rows that belong to
another domain are ranked in that domain's own README.*

- [x] `chat` — speech, journal routing
  - [x] **Speech, heard and answered.** A player says something (`0x03`), and the
    world puts it over their head for everyone within `SPEECH_RANGE` (`0x1C`,
    ported from Sphere's `PacketMessageASCII`) and on the bus as `MobileSpoke`.
    That event is the hook: a script reads the words and answers — a keyword, an
    NPC's line, a command — through `op_say`/`Command::Speak`, and the answer
    goes back out as another `0x1C`. Combat's decoupling for the fourth time; the
    round-trip is tested end to end. This is why the script `Event` and `Command`
    stopped being `Copy`: speech carries an owned `String`, and the bus never
    required `Copy` — only the enums had assumed it.
  - [x] **The Unicode talk packet** (`0xAD`), which is what a modern client
    actually sends when you type — the plain UTF-16 form and the keyword-encoded
    one, ported from Sphere. The classic `0x03` alone left live chat silent for
    every ClassicUO client; this is the fix.
  - [x] **The Unicode reply** (`0xAE`, ported from Sphere's `PacketMessageUNICODE`).
    Speech chooses its encoder by content: pure-ASCII stays on `0x1C`, universally
    understood, but text Latin-1 cannot carry — an accent, a non-Latin script —
    goes out as big-endian UTF-16 `0xAE`, so a player who types "olá" gets the
    accent back intact. A player could only have typed such text through `0xAD` to
    begin with, so the content test doubles as the client-capability one, sidestepping
    that the game connection never states its version.
  - [x] speech *modes* widening or narrowing the range: a whisper (`;`, mode 8)
    carries three tiles, a yell (`!`, mode 9) thirty-one, everything else the
    eighteen-tile screen — Sphere's `DISTANCEWHISPER`/`DISTANCETALK`/`DISTANCEYELL`
    defaults, chosen by the mode byte the client already sends. `speak` picks the
    range; the rest of the path is unchanged.
  - [x] **The living do not hear the dead.** A ghost was drawn only to other
    ghosts and to staff but was still *audible* to everyone in earshot — invisible
    and talking, which reads as a client bug and was an engine one. `chat::speak`
    filters its listeners through the same `WorldState::can_see_mobile` that gates
    drawing (ServUO's `CanSee` decides both), so the gate stays one choke point
    rather than a second rule that can drift from the first.
  - [x] **The logout ack** (`0xD1`). The client's "Log Out" is a *notification*
    that then waits to be told it may go; the id was in the length table and
    nothing answered it, so the paperdoll button hung until the client timed out
    with nothing anywhere to say why. Both references ack it with the same two
    bytes (Sphere's `PacketLogoutAck`, ServUO's `LogoutAck`), queued like every
    other reply so it comes out of a tick. The one entry the two references
    *disagree* about is how long the incoming packet is — Sphere reads one byte,
    ServUO two — and the table takes ServUO's, with the reasoning written where the
    length is.
  - [x] **the guarded staff-command layer** (`.`-prefixed speech, Sphere's
    convention). An account carries an `AccessLevel` — `player`, `gamemaster`,
    `administrator` — set in `[[accounts]]` config (`access = "gm"`), looked up at
    login and carried into the world as an `Access` component, re-derived each
    login so a demotion takes effect and never saved with the character. A game
    master's `.`-prefixed speech is split off in the `Command::Say` handler and
    run as a command instead of reaching anyone's screen; an ordinary player
    saying `.hello` just talks, so there is no leak and no surprise. The commands
    — `.where`, `.go`, `.tele`, `.add`, `.set`, `.skill`, `.admin` — lean on the systems
    that own their rules (`items` spawns, `skills` re-caps the stat) rather than
    reaching into the registry, and answer the actor privately with a `0x1C`
    system line. `.go <x> <y>` jumps to coordinates; `.tele` raises a targeting
    cursor (`0x6C`) and jumps to the tile clicked — Sphere's split, and the
    teleport pushes a `0x20` to the mover's own client so the screen refreshes on
    the spot rather than a step late. The gate lives in the world, not the `gm`
    module, so a command function may assume its caller cleared it. The vocabulary
    grows one verb at a time in `world::gm`.
  - [x] **A container has a ceiling now — `items::capacity`.** ServUO's
    `Container.CheckHold`, and the gap the harvest slice deferred: nothing capped
    what a backpack held, so "your backpack is full, so the ore you mined is lost"
    was a line only a mobile wearing *no pack at all* could reach, and a miner
    mined into a pack with no bottom.

    Two ceilings, and only one of them is reliable here. **Items** is a count —
    125, `GlobalMaxItems` — and works on any shard, because counting rows needs
    nothing but the registry. **Weight** is in stones and comes from the tiledata,
    which is a client file, so a shard with no map weighs everything at zero and
    the weight ceiling silently does not apply. That is the same bargain
    `total_weight` and the step checks already make, and it is why the item count
    is the half worth trusting. A player's own backpack takes ServUO's ML ceiling
    of 550 stones rather than the global 400, and the expansion gate is real.

    Both halves are **recursive and both walk upward**: a bag counts its own
    contents against the pack it is in, and every container up the chain is asked,
    so filling a pack with bags of bags is not a way around it. Staff are never
    refused, which is what lets a game master fill a chest to see what a full one
    does. And a stackable that merges onto a pile already in there costs **no
    slot** — ServUO asks `CheckStack` before `CheckHold`, and a ceiling that
    skipped the question would stop a miner at a hundred and twenty-five swings
    with a pack that had room for all of it.

    Two doors are gated and no more: the player's own drag-and-drop, where the
    item bounces back to the hand that offered it so the refusal is readable, and
    `give_to_backpack`. A corpse being filled and a vendor's shelf being stocked
    are decrees, not offers, and go on taking whatever they are given.
  - [x] **`.skill <name> <value>`, and the `0x3A` a moved skill owes a window.**
    `Command::SetSkill` existed and only tests reached it, so the one way to move
    a skill on a running shard was to train it — which makes half the engine hard
    to try, since a miner needs Mining before a vein gives anything and a smith
    needs Blacksmithy before the ore is worth digging. The command takes a **name**
    (`Skill::from_name`, punctuation-insensitive because the table's own spelling
    is the client's — "Bowcraft/Fletching") and **whole points with one decimal**,
    because 95 is what a player reads off their own window and `.skill mining 950`
    is a trap laid for whoever types the obvious thing.

    Two silences came out with it, and the second is the one that mattered.
    `set_skill` moved the sheet and sent nothing, so a window standing open drew
    a stale number. And `apply_stats` — the one door stats change through — moved
    every skill's *drawn* value without announcing any of them: what a window
    shows is the trained number **plus what the stats lend it** before AoS, so
    `.set str 10` moved twenty-seven numbers on the shard and none on the screen.
    Both emit `SkillChanged` now; the stat door takes all fifty-eight drawn values
    before and after and announces the difference, rather than deciding from the
    scale columns which skills *could* have moved — the same table read, plus a
    rule to get wrong. Those events carry `previous` equal to `value`, which is
    honest (the trained number did not move) and is also what keeps "your skill
    has increased" quiet for a change that is not a gain.
  - [x] **The `.admin` gump and a pack-driven world.** `.admin` opens a staff-only
    gump (`0xB0`, answered on `0xB1`, re-checked GM+ on the button, not only on
    open) whose buttons populate cities and lay down decoration. The *data* lives
    in the community pack, not the engine: a button emits an `AdminAction` event
    the pack reads, and the pack answers with `op_register_spawner`, `op_decorate`
    and `op_generate_doors` — so spawns and scenery are edited in a hot-reloaded
    script, no rebuild. **Spawners** are tick-maintained regions (`maintain_spawners`):
    a region holds creature templates, a max count and a respawn delay in ticks,
    and a `SpawnedBy` marker lets it refill as its creatures die — replayable, like
    decay. **Decoration** is what a shard adds on top of the map's static art, all
    marked `Decoration` (never decays, never lifts): plain statics (walls, signs,
    furniture), **doors** that toggle open/shut on double-click and swing closed on
    their own (`Door`, a two-graphic-plus-hinge toggle in `items`, auto-closed by
    the tick), and **containers** that open onto a gump (town chests, crates,
    barrels — reusing the `Container` open path, placed empty). The whole of Britain
    is migrated from ServUO's `britain.cfg` and `signs.cfg` (door graphics/offsets
    from its door tables, container gumps from the client's own `containers.cfg`),
    resolved to raw graphics *at pack time* so the engine stays a generic
    toggle/open and knows nothing of door or container families.
  - [x] **Doors generated from the map's own art.** A building's plain wooden shop
    doors are not in the decoration data — they are *implied* by the static door
    frames the client map draws, so the shard generates them: `op_generate_doors`
    scans a region's statics for facing frame posts and drops a functional
    `DarkWoodDoor` into each one- or two-tile gap. This is ServUO's `DoorGenerator`,
    ported (`world::doorgen`) — the same four frame-graphic tables and single/double
    geometry — reusing the statics the engine already parses through a new
    `Terrain::statics_at`. The metal and special doors are placed by name from the
    data; this fills in the ones the map only implies.
  - [x] **The pack is a directory now.** `scripting.main` may point at a folder, not
    just a file: the engine concatenates every `.js` under it (organised by facet
    and place — `felucca/britain/spawns.js`, `deco.js`), `index.js` last, into the
    one script it still evaluates, and hot-reload watches the newest mtime across
    the tree. Data files register into a shared `Pack` namespace under a verb;
    `index.js` wires `onEvent` over it. Deco and spawn are separate files, so a
    shard edits one without touching the other. Still deferred: container **loot
    tables**, door **keys/locks**, sign **text** (a cliloc slice), and the
    furniture/addon *behaviours* (a real armoire versus a scenery one).
  - [x] **Inventory persists.** A character's carried things — worn gear, its
    backpack and everything nested inside — and loose ground clutter now survive a
    restart, not just its position. See §4; this is the foundation a bank and a
    vendor stand on, because a service that forgets your gold on logout is a demo,
    not a service.
  - [x] **Bankers, and a bank box that holds value.** Every character wears a bank
    box (a container on `Layer.Bank`, graphic `0x0E7C`) alongside its backpack, so
    it persists and its contents survive a restart. A `Banker` NPC — a standing,
    named, invulnerable townsperson the pack places once (`op_spawn_mobile` grew a
    `name` and a `banker` flag) — answers the keyword: saying "bank" within twelve
    tiles of one opens your box (the same `0x24`/`0x3C` a double-click sends,
    reused through `items::open_worn_container`), and "balance" counts the gold in
    it. The words are still spoken, so it reads as a request the banker answers.
    And it has life, in its own crate — **`crates/server/npc`**, so the townsfolk rules do
    not pile into `tick.rs` (the banker logic *moved out* of it). An NPC is
    **dressed** (`op_spawn_mobile` grew an `equipment` list — a robe, hair — worn
    like any gear and drawn in its `0x78`), **named** (a generated personal name and
    the "the banker" title, from the seeded generator so a replay names it the
    same), **stands on the floor** (a spawn drops onto the map's surface at its
    tile, a building's raised floor and all, through a new `Terrain::stand_z`,
    rather than sinking to a given z and reading as inside a wall), **greets** with
    a line chosen fresh each time and by name, turning to face the visitor, and
    **keeps to a home** — an `Npc { home, wander }` base (the part vendors reuse)
    lets it shuffle a couple of tiles near its post rather than stand frozen. The
    AI seam is decide-then-apply, like the creature brain: `npc::live` greets and
    faces itself, and returns the idle steps the tick applies through its
    terrain-checked `step`. This is the first of the living NPCs; **vendors** (buy
    `0x74`/`0x3B`, sell `0x9E`/`0x9F`) reuse the `Npc` base.
  - [x] **Vendors trade.** A `vendor` spawn wears a stock crate a script prices
    (`op_stock` — price and name are item components, so stock is pack data, not
    engine code); double-click opens the classic buy flow (`0x74` contents +
    `0x3B` purchase), and saying "sell" nearby offers the mirror (`0x9E` list,
    `0x9F` sale) at half price. Stock persists with the vendor (§4, schema v5) —
    a restart does not lose the shelf.
  - [x] **Mounts.** Double-click a horse, llama or ostard to ride: the creature
    leaves the world into limbo and a `0x19`-layer saddle item draws the rider
    mounted; double-click yourself to dismount, and the creature is reconstituted
    whole — heading, walker, brain — beside you. The ride persists through the
    saddle item saved with the character, so logging out mounted logs back in
    mounted; the ridden creature itself is the one mobile the world sweep skips.
  - [x] **Townsfolk are people, not props.** Every one of Felucca's 738 town NPCs
    was the same male body at hue 0 in the same robe and haircut, named after its
    trade ("the blacksmith", thirty-eight of them called "the banker"), silent
    unless it was a banker, and — because a fresh random heading each beat only
    *turns* a mobile on the turn-as-step motion path — pirouetting rather than
    walking. Four things fixed it, all ServUO:
    - **`npc::dress`** is `BaseVendor.InitBody`/`InitOutfit` ported constant for
      constant: a rolled gender (body `0x0190`/`0x0191`), one of 57 skin hues with
      the partial-hue bit (`Utility.RandomSkinHue`), one of nine hair styles and
      seven beards at a matching hue (`RaceDefinitions.Human.RandomHair`), a
      shirt/doublet/fancy-shirt, trousers or a kilt or a skirt, and shoes of the
      `VendorShoeType` its trade declares. All on the world's seeded `Rng`, so a
      populated facet replays. The **trade's own additions are the pack's** — the
      converter reads the 248 `InitOutfit`/`ShoeType` overrides in
      `Scripts/Mobiles/NPCs` and emits the smith's ringmail, apron, bascinet and
      hammer — and are worn *over* the base, winning any layer both want, which is
      the precedence a ServUO override has when it calls `base.InitOutfit()`.
      The roll only takes over a **human** base body, since `InitOutfit` dresses a
      human: Britannia's one non-human town NPC (`FrightenedDryad`, `Body = 266`)
      keeps its own body and its own bare skin rather than being replaced by a
      shopkeeper in a shirt. Hair is an ordinary worn item on the wire, so
      `items::FIXED_LAYERS` refuses a lift from layers `0x0B`/`0x10` — ServUO's
      `Movable = false`, without which a player pulls the hair off a shopkeeper's
      head.
    - **A `Title`** ("the blacksmith") is now a component and the pack sends *that*,
      not a name; `npc::names` puts a person in front of it ("Rowena the
      blacksmith") from the `Data/names.xml` lists. It is a **key**, so it is saved
      (schema v14): the trade is what an NPC's keyword table is looked up by on
      every word spoken nearby, and a binding that lives only in the spawn call is
      the `quest_giver` bug again.
    - **`npc::live`** is `BaseAI.WalkRandomInHome(2, 2, 1)`: one chance in two of
      not moving and one in two of a new heading, so most beats continue on the
      current one and the step *translates*. Every trade greets and turns to face a
      visitor, not only bankers, and a shopkeeper with a customer inside four tiles
      stands still (`VendorAI.DoActionInteract`) instead of wandering off
      mid-transaction. Every townsperson gets the `Npc` beat now, which woke the 257
      of 738 that had neither a bank nor a shop and so had no life at all. LOD gates
      it, like the creature brains.
    - **`npc::speech`** is `VendorAI.OnSpeech`: townsfolk in earshot (four tiles,
      `HandlesOnSpeech`) match **whole-word** keywords and answer. That replaced a
      substring test on the whole line, under which "that sword is unsellable"
      opened a buy-back list; a bare "buy"/"sell" now needs the shopkeeper named
      (`WasNamed`), and `vendor buy`/`vendor sell` work unqualified. A criminal is
      refused out loud (`CheckVendorAccess`, cliloc 501522) at **all four** doors
      into a shop — the open, the sell offer, the purchase and the sale — because a
      client that already has the window up can still send a `0x3B`, so refusing only
      at the open leaves the deal reachable. The **lines are in the tree**,
      sixty-eight trades in `state/data/speech.json` — and are themselves
      ServUO-derived rather than invented: the greeting is cliloc 500186, the
      "what is thy trade" answer is built from the title, and "what dost thou
      sell" lists the trade's actual `SB*.cs` stock. The core default is a plain
      greeting, so a shard that empties the file still speaks.
  - [x] **Vendor restock timers.** ServUO's `BaseVendor.Restock`: a shelf tops every
    line back up to its original amount, checked when the shop is opened
    (`DelayRestock`, an hour) rather than on a tick pass — the reference's own choice,
    and it costs nothing while nobody is shopping. What "full" means has to be
    *remembered*, because the crate's live contents are what is left and there is
    nothing else to compare them against; the price and label go in the record too,
    since a sold-out line leaves no item behind to copy them from. It is saved with
    the vendor as seconds-still-to-wait, the `SpawnerRecord` rule, so a restart does
    not come back either already due or an hour early.
  - [x] **A townsfolk routine, behind a flag.** `[gameplay] npc_schedule` (off, with
    `npc_work_hour`/`npc_home_hour`) walks a townsperson to a `NightHome` outside
    working hours and back to its post inside them, off the world clock
    `tick/ambient.rs` already derives from the tick counter — so it replays like
    everything else. Marked as **ours, not a port**: neither reference ties an NPC to
    the hour, and ServUO's nearest equivalent is a hand-placed `WayPoint` chain with
    no notion of one. `config` refuses a working day that wraps midnight, so the one
    comparison that reads the hours stays a comparison. A spawn names the home
    (`night_home`), which is what makes the setting reachable at all — it was briefly
    a flag with no path to data, restored from a record nothing ever wrote.

    **Where the homes come from is a derivation, and it is ours — and the first one
    was the bug.** It sent each townsperson to *another townsperson's post in the same
    town*, on the reasoning that those are tiles ServUO itself stood a mobile on, so
    they are on the floor and reachable. They are, and every one of them is somebody's
    workplace. Measured on the file it produced: 292 townsfolk homed, **292 of 292
    landing exactly on another NPC's post**, 187 of them on a *vendor's*, and 118
    mutual swaps. A vendor's stock crate is worn, so a shop is wherever the shopkeeper
    is standing: at dusk the tavernkeeper walked to the innkeeper's counter and the
    innkeeper to the tavernkeeper's, each with its shop on its back, and the person
    behind the smithy counter opened the tailor's buy window.

    `Data/Decoration` has no bedrooms, which is where that version stopped. It does
    have **chairs** — `WoodenChair`, `BambooChair`, the cushioned pair, `FootStool`,
    `WoodenBench`, `Stool`, both thrones, and the handful of beds. 401 placements in
    `britain.cfg` alone and well over a thousand across the two facets: more seats
    than there are townsfolk, every one indoors in a real room, and none of them
    anybody's post. So the destination is the nearest **unclaimed** seat, claimed as
    it is taken — which makes the assignment a matching rather than a set of
    independent nearest-picks, so a collision is impossible rather than unlikely.

    Four rules, three of them asserted at generation time because a regression here is
    silent for days and then looks like confused shopkeepers: never a vendor's *tile*
    (checked against the tile, since ServUO stands two of its shopkeepers on their own
    furniture), never a tile already claimed, never a post whose owner is already
    walking here, and still the nearest candidate between six and twenty tiles. Both
    bounds earn their place: under six the NPC never leaves its two-tile wander range,
    and over twenty the bounded A\* (`PATH_BUDGET`, 400 nodes) starts failing, at which
    point `step_toward`'s naive fallback noses it into a wall all night — a first
    attempt shifted by index rather than distance produced a median walk of 79 tiles
    and a worst case of 442. Now: **404 of 726 homed, 0 on a vendor post, 0 shared, 0
    swaps**, walks of 6/9/20 tiles min/median/max. The 322 with nothing free in the
    band keep to their posts, which is what the setting being off looks like anyway.

    The engine settles an NPC *near* its post rather than on it — `wander_step` walks
    home only while further than the wander radius — so this reads as people drifting
    to the taverns at dusk, not as a town standing on the furniture. And **the shop
    shuts** outside working hours, at `check_vendor_access`, the predicate all four
    doors into a shop already call: with the stock crate riding on the shopkeeper's
    body, a destination is only ever a matter of flavour once the shop itself is
    closed.

    LOD makes the cost bearable — the towns nobody is standing in do not path at all.
  - [x] **Barks, and the travellers speak up.** `npc::live` says a trade's `barks`
    when nobody is within greeting range, on its own long cooldown. The lines are the
    same derivation the wares answer uses — the trade names itself and what it
    actually stocks, off ServUO's own `SB*.cs` list — because ServUO's townsfolk are
    silent here and writing a personality per trade is the one thing this slice
    deliberately does not do. A trade with no shop has nothing to call out and stays
    quiet. (The **Town Crier**, ServUO's real source of street noise, is still its own
    feature: it wants a news queue and a staff gump.)

    **`BaseEscortable` is one of the few NPC classes ServUO does give lines**, so
    those are ported as speech rather than as private system messages — a traveller's
    ask, its thanks and its "Hmmm. I seem to have lost my master." (cliloc 1005653,
    1042809) are *heard*, which is what makes sixty of them scattered across a facet
    findable and what tells a bystander an escort has just set out. The ask rides the
    greeting seam (`BaseEscortable.OnMovement`) and stops once someone is leading it.
  - [x] **Locks and keys on doors and chests.** A `Lock { key_value }` beside the
    `Door` (and on a container), ServUO's `ILockable`. A lock is a *refusal*, not a
    second kind of door: the graphic, the swing, the auto-close and the obstruction are
    all unchanged, and the only difference is that the two things which would open it
    do not — a player's double-click (answered with cliloc 502503, "That is locked.")
    and **the AI's decree**, without which a townsperson walking home strolls through a
    locked shopfront and the lock is decoration. Staff walk through both. A locked chest
    does not open either (`LockableContainer`). A key is a `KeyValue` item whose
    double-click raises a target cursor — ServUO's `Key.OnDoubleClick`, a cursor rather
    than a guess, because most of Britannia's shops have two doors within arm's reach —
    and a fitting key both unlocks *and* locks, which is ServUO's one-key-two-directions.
    The **value** matches, not the item, so a copied key works. The lock persists on the
    decoration record, or a set-piece unbars itself at every reboot.

    **The note this replaces claimed the pack already names locked doors. It does
    not, and neither does ServUO**: `Data/Decoration` has exactly one `Locked` entry in
    the whole game and it is a container in Malas. ServUO's locked doors are all
    scripted set-pieces (Doom's Gauntlet) and player houses. So the mechanism ships with
    a way to *reach* it — `op_decorate`'s door and container entries take a `key_value`,
    and a staff `.key <value>` drops a key that locks whatever it is turned on — rather
    than as a rule with no path to data, which is the mistake `NightHome` made first.
  - [x] **Mounted movement speed at the pace budget.** The budget charged every mobile
    the on-foot rate, so a mounted runner — legitimately twice as fast as anything it
    knew about — spent credit faster than it earned and rubber-banded on a long gallop.
    It now takes ServUO's four rates (`Mobile.WalkFoot` 400, `RunFoot` 200, `WalkMount`
    200, `RunMount` 100).

    **The two references look like they contradict each other here and do not**, which
    is worth writing down because the temptation is to "fix" one to match. ServUO's
    numbers are the real step gaps; Sphere's single 200ms walking interval is half
    ServUO's foot walk, because it is a *floor* in an anti-speedhack check and is
    deliberately lenient — jitter, batching and a bad connection must never trip it,
    which is the whole argument of `WalkPace`. So the floors are ServUO's rates halved:
    200 on foot, 100 running on foot or walking a mount, 50 running a mount. `mounted`
    is a parameter of `Walker::request` rather than a field on the walker, the
    read-site-derivation rule `equipped_weapon` follows — a mount goes on and comes off,
    and a copy here is one more thing to keep in step.
  - [x] **Secure trade between players** (`0x6F`). Handing goods over by dropping
    them on the ground and trusting the other party is the oldest scam in the
    genre; this is the window UO answered it with, and it was the last thing
    missing from *players interacting with each other*. Drag an item onto another
    player within two tiles (ServUO's `InRange(Location, 2)`, tighter than
    `ITEM_REACH`) and a window opens on both screens; either side adds and removes
    with the ordinary drag machinery; when both boxes are ticked the goods swap
    packs. Ported from ServUO's `SecureTrade.cs`/`SecureTradeContainer.cs`.

    **The escrow is a worn container, and that is the load-bearing choice.** Each
    party's half is an item on ServUO's own `Layer.SecureTrade` (`0x1E`, graphic
    `0x1E5E`) carrying a `Container` — so `items::in_reach` works with nothing
    written, since it already answers "your own worn container is always in reach"
    and "somebody else's is at their tile", which are exactly the right rules for
    your half of the window and theirs. Adding and taking back are
    `drop_into_container` and `pick_up` unchanged. The price is that a worn thing
    is drawn and saved by default, which one `TradeWindow` marker undoes in the
    two places it must: `equipment_of` (or every onlooker's `0x78` hangs a mystery
    box off both traders) and `inventory_of` (or the escrow *and everything in it*
    is restored into a trade that no longer exists and can never be closed — the
    argument `ground_items` already makes for a spell field and a moongate). It
    also cannot be lifted, ServUO's `CheckLift`.

    **A cancel is found, not announced.** ServUO revalidates every trade from
    `Mobile.Location`'s setter — a call beside every mover, and this engine has
    five of them. `items::validate_trades` runs once a tick over a list that is
    almost always empty instead, the `tick/regions.rs` shape, and ends a trade
    whose parties are no longer both online, alive, on one facet and in range.
    The same pass is ServUO's `ClearChecks`: if the goods change after somebody
    agreed to them, *both* boxes untick — but the contents are only fingerprinted
    while at least one box is ticked, because an unticked pair has nothing to
    clear and the walk is over the whole `Contained` column.

    **Every ending returns the goods**, through one `cancel`: the client's own
    close, a step out of range, a death, a logout — placed in `disconnect`
    *before* the record and inventory are read, or the item would be in neither
    the save nor the world — and the shutdown flush, which cancels every trade
    before its final snapshot for the same reason. A crash without a clean stop
    is the only remaining window, and it is the same one every unsaved second has.

    Two fixes came with it, both of which the window needed and a chest also
    wanted: `drop_into_container` and `pick_up` now tell **every** client watching
    a container, not only the one acting (the "a second viewer must re-open to
    refresh" limitation noted under **Containers** above), which is what makes an
    offer visible across the window at all. **Where the references disagree this
    follows ServUO**: Sphere pads Close/Update with a trailing `false` byte (17
    bytes against 8 and 16) and its own `Trade_UpdateGold` reader contradicts its
    writer about gold-versus-platinum order; ServUO is self-consistent and is what
    a current ClassicUO is tested against. Deferred: the `NewSecureTrade`
    gold/platinum half (actions `UpdateGold`/`UpdateLedger`), which is ServUO's
    *account-level* virtual currency — gold is an item here, and it trades by
    being dragged into the window like anything else; the inbound action is
    decoded and ignored.
  - [x] **A* pathfinding**, so pursuit and homing route *around* walls instead of
    shuffling into them — the thing Sphere does badly. `movement::find_path` is a
    bounded A* over the `Terrain` (the same `can_step` the client's walk uses), with
    a Chebyshev heuristic, a node budget so it can never stall a tick, and a
    corner-cut guard (a diagonal is only taken when both tiles beside it are open,
    so a path never clips a building's edge). It is a pure, dice-free function —
    same map and endpoints, same path — so a replay's monsters keep the same trail.
    The creature chase (`ai::step_toward`) and a townsperson heading back to its
    post both plan through it, falling back to the straight line only when there is
    no map or no route within budget. The path *cache* this once named as a next
    step landed with the creature-behaviour work above (`ChasePath`, a 2s repath);
    adjacent-tile pathing is still open, listed under `ai`.
  - [x] **A name on single-click, a tooltip on hover, a menu on right-click.**
    Clicking a mobile (`0x09`) draws its name over its head for the clicker alone
    — a `0x1C` label in the notoriety colour (ServUO's `Notoriety.Hues`: blue
    innocent … yellow invulnerable), so a banker reads as "the banker" before you
    know to ask. An item labels too now, in the default text hue with its tiledata
    name (Sphere's `addItemName`, "3 gold coins" and all), read through a new
    `Terrain::item_name` beside the `item_blocks`/`item_height` tile accessors.
    That is the classic 2D feel — what a modern client shows on hover, this one
    asks for a click at a time. **And the modern feel is here as well.** AoS object
    tooltips are the "cliloc" system: when the server draws a thing it sends the
    tooltip *revision* (`0xDC`), the client asks for the list (`0xD6` in), and the
    server answers (`0xD6` out) with cliloc numbers the client localizes — a mobile
    is cliloc `1050045` with its name, an item cliloc `1020000 + graphic` (the
    client's own tiledata-name range, so no string travels), pluralised through
    `1050039` for a stack. The revision hash is one value in both packets (Sphere),
    and the whole thing is default-in-core the way names and spells are:
    `WorldState::object_properties` builds the list from components. **Context menus**
    round it out (`0xBF` `0x13` request → `0x14` popup → `0x15` select): a
    container offers Open, a vendor Buy/Sell, any mobile a Paperdoll — each routed
    to the very handler a double-click reaches, so the menu decides *what* and the
    existing rule does *how*. Ported from ServUO's `ObjectPropertyList`/`OPLInfo`/
    `DisplayContextMenu`, cross-checked against Sphere's `PacketPropertyList` and
    `Event_AOSPopupMenuRequest`. Two `[gameplay]` knobs shape it, Sphere's
    `TOOLTIPMODE` made an operator setting: `tooltips` (`"off"` | `"version"` |
    `"full"`) and `context_menus` (bool). **What actually enables them on a modern
    client is the character-list (`0xA9`) flags — bit `0x20` tooltips, `0x08`
    context menus — not `0xB9`** (ClassicUO's `ClientFeatures.SetFlags` reads the
    `0xA9`; the `0xB9` AoS bit is sent too but does not gate OPL). Live testing
    against ClassicUO cost several rounds on the wrong packet before its source
    settled it. Menu-entry clilocs are the `3006xxx` range a modern `cliloc.enu`
    carries (`3006103` Buy, `3006123` Open Paperdoll), not ServUO's short `6xxx`.
    A vendor's buy window needs a crate on **both** shop layers `0x1A` and `0x1B`
    (ClassicUO's buy loop dereferences each with no null check), the display
    (`0x24`) keyed on the vendor and preceded by an equip per crate — ServUO's
    `SendPacksTo`. Still on the list: richer per-object menus, the old (`0x01`)
    popup format for pre-6.0 clients, and a tooltip that refreshes mid-life when a
    property changes (names do not, so nothing needs it yet). **Two things a live
    test surfaced landed with this:** a creature with no name given now takes a
    default from its body (`state::creature_name`, ServUO's ids — "a chicken", "a
    horse"), so an unnamed animal or monster reads on single-click and in its
    tooltip, the pack still free to override per spawn; and a mobile's health bar
    (`0xA1`) is sent *on sight*, riding along with its `0x78` the way the tooltip
    revision does, so the bar reads full from the moment you see a thing rather
    than staying an empty frame until the first blow moved it.
