# Findings

Things about the client, the reference emulators and the client's data files that
cost somebody a day to learn. Every entry here is here because the code was wrong
first: the rule is in [`../CLAUDE.md`](../CLAUDE.md) in one line, and this file is
the argument behind it.

None of it is architecture. For that, [`architecture.md`](architecture.md).

## Reference sources

Other people's emulators, and one other client. None is vendored, none is a
dependency, and — with the single exception called out below — none is copied:
they are read. Where your checkouts of them are is your own business: put the
paths in `CLAUDE.local.md`, which is gitignored beside this file.

**SphereServer**, if a checkout is available: `Source-X/` (the C++ engine) and
`Scripts-X/` (the .scp scriptpack). Read it for **observed protocol behaviour**,
which is two decades of finding out which client breaks on what and is genuinely
hard-won. `Source-X/src/common/sphereproto.h` is the single most valuable file in
it.

**ServUO** (C#, on GitHub), for a second opinion on the same problems. Where the
two agree about the client, that is as close to a specification as this genre has.

**Iris2** (C++/Ogre3D, on GitHub, archived), the one open reference that ever put
Ultima Online on screen in *actual* 3D rather than as sprites. Read it for the
`.grn` model format and for the tables that map a body to a model and an
animation — see "3D character models" below. It is **GPL**, and this project is
MIT/Apache-2.0: not a line of its code and not one of its assets may land here.
Its observations may, rewritten.

**anima-client** (Rust, on GitHub), a UO client written from scratch: a headless
sans-IO core (`anima-core` — protocol, world model, Z-aware A\*, asset parsers)
with the renderers on top, validated against a live ServUO. It is the only
reference here that implements *our counterpart* rather than another server, so
where it and this tree disagree about a packet, one of the two is wrong about
the wire and it is worth finding out which. It is **MIT OR Apache-2.0** —
declared in every crate from its first commit, licence files added 2026-08-07 —
which is this project's own pair, so unlike everything else on this list its code
may be borrowed outright, with the copyright and licence kept. Its author read
this file against that tree and sent back the stun/disarm finding below.

**Three engines that lit a two-dimensional world**, read for the lighting plan
and none of them a UO reference: **OpenNox** (GPL-3, Go, a reimplementation of
Westwood's Nox — `src/client/sight.go`), **DevilutionX** (Diablo 1's
reconstructed source — `lighting.cpp`), and **Godot** (MIT — `Light2D`,
`LightOccluder2D`). What they are worth is in "How three other engines lit a flat
world" below. Licences differ and none of them is compatible with this tree; they
are read, and what lands here is an observation in English.

Do **not** read any of them for architecture. Copying their structure is the one thing
this project exists to avoid — and where they agree about *engine* design, that
is often the strongest available argument for doing it differently. Both stop the
world to save it; `crates/server/persistence/src/journal.rs` explains at length
why this one does not.

## Reading the reference emulators

**Take Sphere's numbers; audit its arithmetic.** The `MINCLIVER_*` table, the
step vectors, the Huffman key, the 200ms walk interval — all hard-won, all worth
copying verbatim. But its walk speed check compares a duration against a count
and does not survive being read closely, so `WalkPace` is a token bucket instead.
Copying something that does not add up is worse than not copying it.

**Read Sphere's shifts, never Sphere's comments.** Its IP comments say the
opposite of what its code does — `send.cpp` calls the branch emitting
`C0 A8 0B 06` "in reverse", because it reverses the *dword*, and the dword is an
`s_addr` that is already network order. Both readings are articulate and one is
wrong. Trace the bytes for a concrete address, in C if need be; the answer takes
a minute and the alternative shipped a shard nobody could log into.

**A tiledata flag means what the reference *reads* it for, not what its comment
says.** Sphere's header calls `UFLAG2_WINDOW` "window/arch/door can walk thru it",
and Sphere never once consults it in `CWorldMap`: the only three uses in the
whole engine are line-of-sight tests in `CCharLOS.cpp`, gated on
`LOS_NB_WINDOWS`. Honouring the comment in the *movement* check let anything the
server moved walk out through every wall segment with a window in it. It never
showed for a player, because the client refuses the step before it is ever sent
— which is the general trap: a server-side movement hole is invisible from the
only end normally tested, and surfaces as NPCs strolling through walls.
(`NO_SHOOT` was mis-valued at `0x20` in the same file, which is `UFLAG1_DAMAGE`;
there is no `UFLAG1_NOSHOOT` at all. Pin a flag's value in a test next to the
constant.)

**One rule can live in two files, and the one you find first is the smaller
half.** ServUO answers "does a body block a step" twice, and reading only
`Movement.CheckMovement` gets it backwards. That file's mobile check
(`Scripts/Services/Pathing/Movement.cs:344`) is gated on
`p is BaseCreature && !Controlled && (xForward, yForward) != m_Goal` — so it is
**uncontrolled creatures only**, and it deliberately lets a route end on its
quarry's own tile. A *player* never reaches it: `Mobile.Move` asks the mobile
being walked into instead, through `OnMoveOver` → `CheckShove`
(`Server/Mobile.cs:3517`), and that is not a block at all — at full stamina a
player **shoves through** for 10 stamina and a message, and only a player
already below full stamina is stopped. Staff shove for free.

Three consequences, all of which a "bodies block" implementation gets wrong if
it reads the first file alone: the goal-tile exemption exists (without it no
chase is plannable, because the tile the route is *for* is the one it may not
end on); the flanks of a diagonal are checked, not just the destination
(`Movement.cs:552`); and hard-blocking players is a **divergence**, not parity.

**And the client has the same rule, which makes that divergence visible.**
ClassicUO's `Pathfinder.CreateItemList` decides in one line
(`Game/Pathfinder.cs:65`) whether bodies are in the way at all:

```csharp
bool ignoreGameCharacters = profile.IgnoreStaminaCheck
    || stepState == PSS_DEAD_OR_GM
    || _world.Player.IgnoreCharacters
    || !(_world.Player.Stamina < _world.Player.StaminaMax && _world.Map.Index == 0);
```

Read the last clause twice: mobiles block **only** below full stamina **and**
only on map 0. That is `CheckShove` seen from the other end — full stamina is
what buys the shove, and map 0 is Felucca, whose `FeluccaRules = None` is the
one ruleset without `MapRules.FreeMovement`
(`Server/Map.cs:129`, commented "anyone can move over anyone else without taking
stamina loss"). Two engines, one rule, expressed as a server permission and a
client prediction.

It is not only the pathfinder: `PlayerMobile.Walk` runs the held-direction step
through the same `Pathfinder.CanWalk` (`Game/GameObjects/PlayerMobile.cs:572`
and `:598`). So a shard that hard-blocks contradicts what a stock client has
already drawn — the client walks its body into the crowd and the shard snaps it
back, on every facet and at every stamina level.

Its `PSS_DEAD_OR_GM` is `IsDead || Graphic == 0x03DB` — the client recognises a
game master **by body graphic**, not by access level. A shard whose staff wear
ordinary bodies has exactly one lever, and it is the `0x10` flag below.

The overlap test in both files is `(other.Z + 15) > z && (z + 15) > other.Z` —
**fifteen**, where the same file's `PersonHeight` is sixteen. One unit, and it
is the difference between a mezzanine floor being walkable with somebody
standing under it and not.

**`IgnoreMobiles` is on the wire, and it has to be.** ServUO carries the
exemption to the client as bit `0x10` of the mobile flag byte in `0x77`/`0x78`
(`Server/Mobile.cs:8802`, and again in the pre-7.0 encoder). That is not
decoration: the client keeps its own copy of the body-blocking rule and applies
it to what it predicts, so a server that exempts a mobile without sending the
bit gets a step allowed at one end and refused at the other — which reads as a
rubber-band rather than as a permission.

## How three other engines lit a flat world

Read while planning the lighting rewrite ([`lighting.md`](lighting.md)), because
a day somebody else already spent is the cheapest day there is. Two findings came
out of it and the first is the one that changes what we build.

**An occluder is a shape declared per object type, not a shape derived from the
picture.** All three do this and none of them measures anything:

- **Nox** has exactly four constructors — `newFromWall(gx, gy, typ)` for a wall
  on the grid, and `newFromDrawableBox`, `newFromDrawableCircle`,
  `newFromDrawableDoor` for everything else. A box or a circle, per object.
- **Godot** authors an `OccluderPolygon2D` per sprite. Its editor can generate one
  from the sprite's alpha, which is the same silhouette pass
  `facing::facing_of` is — and it produces a polygon that a person then edits.
- **Diablo** does not have occluder shapes at all: light is a level per tile,
  flooded over the grid with radius tables.

So the hand-authored table of [`lighting.md`](lighting.md)'s decision 31.2 is not
a workaround for missing tooling. It is what everyone who has done this converged
on, and the part that is genuinely unusual here is the opposite one: **we derive
a solid from art we did not draw**, because the art is the client's and there is
no model behind it. Where Baldur's Gate shipped a light map and a height map
beside its background, those were *exports* from the 3D scene its artists built.

**And nobody marches a ray per fragment per light.** Two different answers, both
"compute the shadow once, look it up after":

- **Nox** builds shadow geometry on the CPU: each occluder becomes an angular
  interval (`SightObject.Ang40`/`Ang44` in a fixed-point circle of
  `sightAngSz = 75000`), a sorted active list sweeps them by angle with `DistSq32`
  deciding who is in front, and the result is extruded away from the eye, clipped
  to the view lines, and **filled black** — the entry point is called
  `Nox_xxx_drawBlack_496150`. Soft edges are ten passes of the same line offset a
  pixel with alpha falling 208, 188, 168…
- **Godot** renders a per-light shadow map — distance to the nearest occluder by
  angle — so a fragment costs one lookup plus a PCF tap or thirteen.

Neither is a plan for this engine and one of them is not even the same problem:
**Nox's is visibility from a single eye, not light from a lamp.** There are no
sources in it, no height (`image.Point` and `Rectf` throughout — `z` does not
appear in the file), and its cost is linear in occluders because it never touches
a pixel per light. Godot's `Light2D` has a `height`, but it feeds the normal map
only: the occluder is a polygon in the canvas plane and cannot say that light
passed *over* a low wall. `range_z_min`/`range_z_max` are `z_index` draw layers,
which is the manual-storeys trick — and the reason it is not enough here is that
UO puts a real `z` in every packet.

What both point at is written down and not acted on: this engine walks the grid
**per fragment per light**, which `lighting.md`'s step 6 measured as nearly all of
the pass's GPU time, and two independent predecessors say the standard cure is to
compute a light's shadow once. It is not a plan until something is measured that
says it must be.

## The client, as observed

**The game connection never says what the client is.** The version arrives in
the seed sent to the *login* server; the second connection opens with four bytes
of auth key and a `0x91`, and carries no version at all. A game session that only
knows its own socket defaults to the oldest dialect and sends a 1997 character
list to a modern client, which reads past the end of it and desynchronises —
surfacing as a garbage packet id hundreds of bytes later, looking nothing like a
version problem. The auth key is the only thing linking the two connections, so
anything that has to cross the gap rides on it. Sphere stashes it on the account
instead, which races when two clients share one.

**The 0x8C relay and the 0xA8 shard list carry the address in opposite orders.**
Relay: octets in order, always, no version gate. Shard list: reversed from 4.0.0
on, in order below it. Both SphereServer and ServUO agree exactly, which is as
close to a specification as this genre gets. A change that makes the two packets
consistent has broken one of them. And the relay is the expensive one: get it
wrong and the server sees a clean login and a clean disconnect, because the
client dialled a well-formed address that was not this machine and never came
back — the failure happens where this end cannot see it.

**Never trust a length off the wire.** Check it against the buffer before
reserving anything. `frame_client_packet` rejecting a claim above
`MAX_PACKET_SIZE` is what bounds gateway memory; nothing downstream re-checks.

**The server remembers what is on each client's screen.** There is no "what can
you see" packet — only "draw this" (`0x78`) and "forget that" (`0x1D`) — so the
only way to send a mobile exactly once is to know what was sent before. That is
what `World::seen` is. Skip it and every step redraws every neighbour, which
looks fine with two players and melts at two hundred.

**Distance in UO is Chebyshev, `max(|dx|, |dy|)`.** The client draws a *square*
region. A circle here leaves the corners of every screen empty, and the bug
looks like mobiles popping in and out at the edges.

**Every visible action plays a sound and an animation, not just a state change.**
A swing that lands, a spell that resolves, a door that opens, a potion that is
drunk, a mobile that dies — each one, to a real client, has a sound (`0x54`), a
mobile action animation (`0x6E`, or `0xE2` gated on
`Feature::NewMobileAnimation`) and often a graphical/particle effect
(`0x70`/`0xC0`/`0xC7`). Sphere and ServUO fire these on essentially every action,
and their action/`SpellInfo` tables already carry the ids. A state-only system
passes its test and feels dead in the client — a fireball with no bolt reads as
broken even when the damage is right. So when you build or review a gameplay
action, emit its feedback too: broadcast through the same `seen`/interest
machinery as `0x78`, encoder in `crates/common/protocol`, default in core and
overridable in the pack off the domain event — the split combat and magic already
use. This was a systemic miss for most of the project's life; do not add to it.

**`0x22` is two different packets, one per direction, and both are three
bytes.** Server to client it is the walk ack: sequence, notoriety. Client to
server it is `Resynchronize`: the id and two bytes of nothing. Same id, same
length, opposite meanings, and no field distinguishes them — only which way the
bytes are going. ServUO registers `0x22, 3, true, Resynchronize` beside a `0x22`
it also emits, ClassicUO has `Handler.Add(0x22, ConfirmWalk)` and
`Send_Resync()` writing id `0x22`, and both are right. A packet table that
resolves an id to one meaning is wrong for this one; ours is already two tables
(`ClientPacket` and `ServerPacket`), which is what makes it a non-event here
rather than a day.

**The walk handshake has a repair leg, and it is a request/response.** Reading
`WalkerManager.ConfirmWalk`, `DenyWalk` and `PacketHandlers.UpdatePlayer`
together, the whole cycle is: an ack the client cannot place is a *bad step* →
it sends one `0x22` resync (guarded by `ResendPacketResync`, so a burst of bad
acks asks once) and sets `WalkingFailed`, which is the first condition in
`PlayerMobile.Walk` and stops every further request → the server answers a
resync with the player's real position (`0x20`), everything in view again, and
its own sequence back to zero (ServUO's `Resynchronize`: `MobileUpdate`,
`MobileIncoming`, `SendEverything`, `state.Sequence = 0`,
`ClearFastwalkStack`) → the `0x20` handler clears `WalkingFailed` and
`ResendPacketResync` and forces the tile. Freezing the walk is only safe because
something is guaranteed to unfreeze it; a client that stopped walking on a
desync *without* sending the resync would stop walking for good.

**ClassicUO's ack is a search, not a queue pop, and that is a consequence of
where its steps live.** `ConfirmWalk` scans the ≤5 remembered `StepInfo`s for
the sequence and only marks the match `Accepted`; the step is *consumed* by the
animation (`Mobile.ProcessSteps`, which pops when the step's time has run out
and its `StepInfo` was accepted). Ours is a FIFO and the ack must match its
front. Neither is a port of the other and ours is not the poorer: the wire
answers each `0x02` exactly once and in order, so a queue is provably right,
while a search cannot tell a *stale* answer — one to a step a rollback already
voided — from a real desync at all. ClassicUO calls both a bad step, so every
wall hit on a slow link costs it a needless resync. Counting what is still owed
(`Walk::draining`) distinguishes them; see `docs/client.md`.

**ClassicUO has the pre-AOS stun and disarm subcommands swapped.** `0xBF`
subcommand `0x09` is *disarm* and `0x0A` is *stun*; the client says the opposite.
`OutgoingPackets.cs`'s `Send_StunRequest` writes `0x09` and `Send_DisarmRequest`
writes `0x0A`, while ServUO registers `RegisterExtended(0x09, true,
DisarmRequest)` and `RegisterExtended(0x0A, true, StunRequest)`
(`Server/Network/PacketHandlers.cs`), and Razor (`Razor/Network/Packets.cs`)
writes ServUO's pairing. Two independent receivers agreeing is suggestive, not
proof — what settles it is that the two handlers gate on *different skills*, so
the shard's own reply names which one you sent.
`Scripts/Items/Equipment/Weapons/Fists.cs` wants ArmsLore and Wrestling ≥ 80 to
ready a disarm, Anatomy and Wrestling ≥ 80 to ready a stun. With hands free,
ArmsLore and Wrestling at 100 and Anatomy at 0, `0x09` answers cliloc 1019013
*"You get yourself ready to disarm your opponent"* and `0x0A` answers 1004008
*"You are not skilled enough to stun your opponent"*, and inverting the skills
inverts both replies. The server registration is right and the client is the
outlier. Nothing about getting it backwards is loud: sending the client's pairing
arms the *other* special with no error at all, and a server that takes the
pairing from the client hands every player the wrong move. Both handlers return
immediately when `Core.AOS`, and an AOS-era client sends `Send_UseCombatAbility`
instead (`GameActions.cs`), so this is a pre-AOS-only trap — which is exactly the
era a from-scratch shard implements first. The live measurement is
anima-client's, on a running ServUO (its issue #1); everything else above is
checkable in the two checkouts.

**`0xAF` is the only packet that says which corpse was which body.** A death on
the wire is otherwise two unrelated facts — a mobile stops being drawn (`0x1D`)
and an item appears (`0x1A`) — so a client that wants to run the fall into the
body lying there has to pair them itself, and the only material to pair them with
is the tile. Two of the same creature dying on one tile swap falls under that
rule, which is not an exotic case where a spawn stands in a group. The packet is
thirteen bytes: killed serial, corpse serial (zero for a death that leaves none),
and a run flag ServUO always writes as zero. ServUO sends it from `Mobile.Kill` to
every client in range *except* the dying player's own — that one gets `0x2C` and
watches its own ghost. ClassicUO's whole `CorpseManager` is this pair plus a
direction: it plays the death group itself off this packet and refuses to draw the
corpse item at all (`ItemView.Draw` returns false while `CorpseManager.Exists`)
until the animation finishes.

**ServUO sends `0x1A`'s flags byte for nearly every item on the ground.**
`Item.GetPacketFlags` sets `0x20` for anything movable and `0x80` for anything
invisible, and the byte is written whenever either is set — so "the flags byte is
rare and can be refused" is wrong by an order of magnitude. A decoder that errors
on it drops most of a real shard's world; one that reads it loses, at worst, a
hint it does not model. The light byte in the same packet is the opposite case —
genuinely rare — but it shares the flag bit on `x` with a corpse's facing, so the
graphic is what picks which of the two a present byte was.

**A corpse is an item that faces somewhere, and the byte saying so is not where
the flag bit is.** `0x2006` is a corpse marker, and the client draws it through
the *mobile* renderer: the last frame of the dead body's death group, for one
direction. That direction rides `0x1A`'s direction/light byte — the top bit of
`x` (`0x8000`) announces it, but the byte itself is written after `y` and before
`z` (ClassicUO `PacketHandlers.UpdateItem`, ServUO `Packets.cs`'s `WorldItem`).
Put it after `x`, where the flag is, and every field from `y` on is one byte out.
ServUO writes it only when non-zero (`Corpse.Light = (LightType)Direction`), so a
corpse facing north sends no byte at all and the client's zero-initialised
`direction` means the two forms say the same thing. On the way in the client
stows it in a field it has spare — `item.Layer = (Layer)direction` for `0x2006`
only — and reads it back masked, `(byte)Layer & 0x7F & 7`, so the run bit is
never part of a corpse's facing. Miss the byte and the shard is not visibly
broken: the death *animation* plays correctly, because it is the mobile's own,
and only the corpse it leaves lies the wrong way.

## The client's data files

**A tiledata flag means what the engine reads it for, and a barrel is not
necessarily a barrel.** `water barrel` (`0x154D`) looks exactly like a barrel on
Britain's docks and is walked straight through. Its tiledata carries one flag,
`0x4000` — `ArticleA`, meaning the item's name takes "a" — and neither
`Impassable` nor `Surface`; its height is zero. The barrel a tile away
(`barrel`, `0x0E77`) is `Impassable`, height 5, and stops everybody. ServUO
decides with the same predicate (`ImpassableSurface = Impassable | Surface`,
`Scripts/Services/Pathing/Movement.cs`), so the reference walks through it too:
this is the client's data, not a defect in ours, and making it solid is a
gameplay decision rather than a fix. Pinned in
`client/app/src/clutter.rs`. What it cost was a day of looking for the bug in
three layers that were all behaving.

**The stacking flag is on the graphic the shard sends, and not always on the
one it draws as.** `tiledata`'s `0x0800` — ClassicUO's `Generic`, read as
`IsStackable`, ServUO's `TileFlag.Generic` — is what says several of a thing
are one item with a count. A pile also *changes art* as it grows (the coin
bands in `client/render/src/items.rs`), and the two do not agree in the file:
gold carries the flag on all three graphics (`0x0EED`/`0x0EEE`/`0x0EEF`,
`0x800` each), while **copper carries it only on the single coin** — `0x0EEA`
is `0x4800`, and its two pile graphics `0x0EEB`/`0x0EEC` are `0x4000`, no
stacking bit at all. So anything that asks "is this a pile" about the art on
screen answers *no* for a handful of coppers the moment there are two of them,
and *yes* for the identical coins in a bag, where the shard's own graphic is
what survives. The rule: keep the graphic the shard sent, and derive the drawn
one where it is drawn (`GroundItem::displayed`). Verified by reading the four
entries out of `tiledata.mul` by hand at the offsets below.

**Read a tiledata answer straight out of the file before believing a layer is
wrong.** The file is 3,188,736 bytes, which is the High Seas layout exactly —
41 bytes a static entry, 8-byte flags — and that arithmetic is what says the
reader is aligned at all. Two entries read by hand from those offsets agreed
with `TileData` to the bit, which is what turned "our reader is broken" into
"the client says so" in one step.

**The map is in the `.uop`, not the `.mul`.** Modern clients ship both and the
`.mul` may be a stub full of zeroes. `WorldMap::load_facet` prefers the UOP. See
`world::uop`.

**A zero pixel inside a land diamond is black, not transparent.** Statics carry
their transparency as `0x0000` pixels, and applying the same rule to the ground
is wrong: a land tile's shape is the diamond and nothing else, and real tiles
contain a handful of genuinely black pixels inside it — three to nine on a
typical one. `Image` cannot tell the two apart, because it stores the corners
outside the diamond as `Color16::TRANSPARENT` too, so the shape has to come from
`art::land_row` rather than from the colours. Reading it out of the colours
instead punches pinholes through the ground, which look exactly like dark
texture: the first renderer to do it covered 97.7% of a viewport instead of
100%, and the missing 2.3% was invisible on a screenshot.

**A land cell's `z` is the height of one corner, not of the tile.** It belongs to
the corner the tile shares with its neighbours to the north — the top of the
diamond on screen — and the other three corners are read from the cells at
`(x+1, y)`, `(x, y+1)` and `(x+1, y+1)`. The client stretches the tile over those
four points, so adjacent tiles are built from *the same* vertices and a gap
between them cannot occur. Drawing each tile as a flat 44×44 diamond at its own
`z` instead is not merely an approximation: neighbours pull apart along every
slope, and a screen of Britain loses 2.3% of its pixels to seams while the sea
still covers 100%, so a level-ground test says nothing about it.

**A sloped tile's texture comes from `texmaps.mul`, not from `art`.** The 44×44
land sprite is what the client draws when the four corners share a height; on a
slope it binds a square texture from `texmaps.mul` and maps it corner to corner,
because stretching the art diamond onto a steep quad smears it. Two shapes and
two texture sources, chosen per tile. Corner to corner is the identity — the
quad's top vertex takes the texture's top-left, the right vertex its top-right —
which is `_cornerOffsetX/Y` in ClassicUO's `DrawStretchedLand`.

**Which texture is not the land graphic.** It is a separate id in `tiledata`'s
land entry, two bytes between the flags and the name, in an index space of its
own. Nothing relates the two numbers, so reading that field at the wrong offset
still names *a* texture for every tile in the game: the ground comes out textured
with somebody else's terrain, which reads as a seasonal variant rather than as a
bug. The size is not stored either — 64 or 128 is decided by the entry's
*length*, and ClassicUO reads anything that is not `0x2000` as a 128.

**No texture means the client never stretches the tile at all.** `IsStretched` is
initialised to `TexID == 0 && IsWet` and then read as "do not", and
`ApplyStretch` gives up immediately when the texture entry is empty — so a tile
with no texture is drawn as a flat diamond however the ground around it stands,
seams and all, and water is never stretched. The decision is also made over a
wider neighbourhood than the tile's own four corners: it comes from the four
corner *normals*, each of which reads a cell beyond the corner.

**A quad's corner texture coordinates need a half-texel inset.** A region's edge
is the boundary *between* two texels, so a vertex at `u + du` samples the first
texel of whatever is packed next door in an atlas — a one-texel fringe of foreign
terrain along two edges of every stretched tile. ClassicUO insets by half a texel
in `CalculateHalfPixelUVs`, which makes the four corners sample the texture's own
first and last texel centres. This does not arise for a tile drawn 1:1 from its
own sprite, which is why it appears exactly when stretching starts.

**Doors are not in the map, and neither are shop signs.** A `.mul` static cannot
move, and a door has to open, so the client's files contain almost none: across
400x400 tiles covering the whole of Britain — 87,191 statics — there is exactly
**one** tile whose tiledata name is `door`. Signs split in two: the *post* is a
static (49 of them in the same rectangle, `metal signpost` / `wooden signpost`),
while the hanging board naming the shop is not there at all. Both are server-side
decoration, placed as items with `Drawn` + `Position` and drawn by `0x1A` — see
`world/tick/decor.rs`, which already carries doors and containers.

Worth knowing because of how it fails: a client rendering the map alone shows
cannons (49 tiles), fountains (31) and every wall and roof, so the scene looks
complete, and what is missing is exactly the furniture a player reads as part of
the building. The conclusion "our static renderer drops small sprites" is the
natural one and it is wrong. The population comes from the Community Pack's
`felucca/_generated/deco.js` — 18,832 statics, 638 doors, 5,598 containers,
converted once out of ServUO's `Data/Decoration/**.cfg` and `Data/signs.cfg` —
and it lands through the `.admin` "Decorate Felucca" button.

**A worn item's default picture is its own tiledata `AnimID`, and `Equipconv.def`
is an override table, not a lookup a worn item requires an entry in.** The
`AnimID` field of a static's tiledata entry (offset 14 of 21 in the High Seas
layout, `StaticTile::anim_id`) names the body-animation-space graphic an item
draws with — read through the same `anim.mul` machinery a mobile's body
itself uses — and *that* is what an ordinary shirt or pair of boots draws
from: it has no entry in `Equipconv.def` at all. The table only maps
`(body, AnimID)` to a *different* `AnimID`, for the pairs where this body
needs a different picture (chiefly a race or gender variant of the same
garment); confirmed by reading `AnimationsLoader.ProcessEquipConvDef` and
`MobileView.AddItem` in ClassicUO, a BSD-2 reference client kept outside this
repository — `graphic = item.ItemData.AnimID`, replaced only if
`EquipConversions[body][AnimID]` exists. Treating a missing entry as "draw
nothing" instead of "the `AnimID` already draws right" was the first
implementation of equipment rendering here, and it dropped every piece of
plain clothing on every NPC silently: no test caught it, because every test
built its own atlas and equipment table together and never had an item
*without* a conversion entry in the mix — the gap only showed on a live
client with a real `Equipconv.def`, which does not have entries for the
common case. Also not read: `mobtypes.txt`, which tells a human body from a
gargoyle one and shifts the resolved graphic accordingly — without it, a
gargoyle's equipment resolves through the same table a human's does.

`AnimID` had already been named in this reader's own layout comment for
`parse_static` since the day it was written — `crates/common/uofiles/src/tiledata.rs`
documents the byte offset in a doc comment on the function — and was simply
never wired into the `StaticTile` struct or read. It sat between two fields
that already were read (`layer` before it, `height` after): a field can be
*in the comment* and still not be in the code, and nothing short of a
consumer asking for it says so.

**3D character models exist in the client's files, and nobody ever built them.**
Origin shipped a 3D client — Third Dawn, later UO:3D — and its installer laid a
`Models/` directory beside the `.mul` files: skinned character meshes with
skeletal animation, in **Granny** (`.grn`, RAD Game Tools). Iris2, the archived
C++/Ogre3D client above, renders UO in 3D by reading exactly that directory out
of the player's own install, the same way anything here reads `art.mul`. It
authors no geometry of its own: its `data/grannys/` holds one material and the
skinning shaders, and nothing else. So "3D characters" is not a modelling
project — it is a file-format project, and the art is already on disk.

What that costs, in the order it has to be solved:

- **The format is proprietary and was reverse-engineered by hand.** Granny's
  official `granny2.dll` is a paid SDK and unusable in a GPL project, so Iris2
  wrote its own chunk walker: a magic of `0xCA5E____`, a visitor over a tree of
  chunks — points, normals, texcoords, polygons, weights, bones, bone ties,
  animation. It is **not fully decoded**. The structs are honest about it:
  fields named `iUnknown[7]`, comments reading "order unknown", "might be
  scale", "doesn't look like floats". Anyone porting this inherits the gaps, not
  a specification.
- **Three tables in the client answer "which file".** `Models.txt` maps a body
  id to a type (`0` monster, `1` sea, `2` animal, `3` human), a model name, a
  default hue and three scale factors; `Human.lst` / `Animal.lst` / `Monster.lst`
  / `Sea.lst` map an animation id to an animation name; together they produce a
  path like `Models/Animals/Deer_Stag_Walk.grn`, with textures in `Models/Maps/`.
  This is the 3D counterpart of `Bodyconv.def` and `anim*.mul`, and it is a
  separate id space again.
- **A humanoid is stitched from parts, not loaded as one mesh.** Head, torso,
  legs, hair and every worn item are separate meshes sharing one skeleton
  (`stitchin.def`). The invariant Iris2 states in a comment beside the code and
  is worth stealing: **the skeleton is chosen by the body id, never by what the
  body is wearing** — equipment supplies meshes, not bones.
- **Conventions differ from any engine you will bind it to.** Granny orders a
  quaternion `x,y,z,w` where Ogre wants `w,x,y,z`; bones carry a parent index
  plus a local translate and rotation, so a derived transform is a walk up the
  hierarchy; skin weights are two bones per vertex, which is what makes GPU
  skinning straightforward.

The reason this is a finding and not a plan: `Models/` is present only where a
Third Dawn / UO:3D install once existed, and later clients do not ship it at all.
Iris2 knows this and pops a dialog — "No 3D Character Models found in UO-dir" —
because for most players it simply is not there. Measured against the population
in [`client_versions.md`](client_versions.md), a renderer that requires those
files serves a small and shrinking slice, so the value here is the format and the
body→model→animation mapping, not a route to a 3D client.

**No client files are in this repository and none ever will be.** They are
copyrighted and they are not ours to redistribute. `world.client_files` points
at whatever install the operator already has; the tests that need one read
`OPENSHARD_CLIENT` and skip when it is unset. Do not commit a path to anyone's
machine, and do not name whose files you tested against — this crate reads a
*format*, not a particular shard's data.

**A multi's "draw me" flag runs opposite ways in the two files that hold it, and
nothing in either says so.** `multi.mul` stores a tile-flag word per component,
and the value that marks a component the client actually draws is `Background`
(`0x01`) — the *skip* value is zero, which reads backwards from the name. On the
shipped file that is 57,784 drawn components against 2,030 skipped. Both
references agree (ServUO's `i == 0 || m_Flags != 0`, ClassicUO's `if (flags == 0)
continue`).

`MultiCollection.uop` stores a **small enum instead**: `0` drawn, `1` skipped,
`257` generic. So `0` means *draw* in one file and *skip* in the other, and
ServUO's `UOPLoad` quietly translates between them in a `switch` with no comment
on it.

This is invisible from one side. Both readers parse cleanly, both produce
plausible houses, and the mistake only shows when the same multi is read out of
both files: it was 309 of the 326 they share, with the same graphics at the same
offsets and every flag inverted. `client_files.rs`'s cross-check is that
comparison, and the threshold in it is set to tell a *convention* error (nine in
ten disagree) from the files honestly drifting apart (a couple of dozen do,
because one is from 2021 and the other from 2024).

**And the two files are not the same size.** 326 multis in the `.mul`, 862 in the
UOP, on one install. The UOP is the newer and it wins where both exist — the
opposite of `map0.mul`'s trap, where the stale file is *zeroed* and therefore
loud. A stale `multi.mul` is simply older, and a house read out of it has walls
in places the client, which read the UOP, does not draw them.

**A shipped `multi.mul` has multis that draw nothing at all.** Five of the 326:
`0x03E8`–`0x03EB` are the `treasure` tiles a dug-up map site is decorated with,
and `0x0FAB` is a stack of hanging poles. Markers, not buildings. Any check of
the form "every multi has at least one drawn component" fails on a real file.

**High Seas widened the multi component too, and the arithmetic still settles
it.** Same problem `tiledata.mul` has and the same answer: a component is 12
bytes or 16, so a table of index lengths that divides by 16 and not by 12 cannot
be the old layout. On the shipped file every one of the 326 divides by 16 and
only 115 divide by 12 — decisive, and better than ServUO's `PostHSFormat`, a
static somebody has to remember to set.

**There is no shipped gump for "a blank rectangular plate behind arbitrary
text."** Looked for one to back a client-owned control (`docs/window_components.md`'s
container plate) the way `vendor.rs` backs its own window in real gump art,
before drawing anything synthetic. `gumpartLegacyMUL.uop` has plenty of
button-shaped graphics, but every one either has text baked into the art itself
— `0x0481`/`0x0482`/`0x0483`, ClassicUO's generic message-box OK button
(`MessageBoxGump.cs`), decode to 28×21 with "OK" burned into the pixels, so it
cannot carry a different caption — or is sized for a different job:
`0x0836` (this codebase's `BOTTOM_COMMENT`, `skills.rs`) is 210×19 and is **not
a plate at all** — its pixels are a picture of the sentence "Left-click the
button before a skill to use the skill. / Skills without buttons are accessed
in the world.", which is why ClassicUO adds it as `_bottomComment`
(`StandardSkillsGump.cs:59`). Nothing can be written on it; it is already
text. This entry originally described it by its *size* and called it "a
value-label plate three times too wide", and `container.rs` reused it on the
strength of that description — so for as long as that lasted, every bag in
this client drew that sentence under itself, tinted purple under the pointer.
**Measure a candidate gump by decoding its pixels and looking, never by its
dimensions.** `0x0837` (`USE_BUTTON`) is an 11×11 icon, not a
rectangle with room for a word. ClassicUO's own equivalent controls — its
tooltip box (`Tooltip.cs`), its right-click context menu (`ContextMenuControl.cs`),
and `GridLootGump`, the closest thing the reference client has to a
client-side container action — all draw their text's background as a
**solid-colour quad built in code**, not as gump art at all; `ContextMenuControl.cs`'s
only gump reference (`0x838`) is a checkmark icon beside a row, not the row's
background. The nearest thing to reusable "flat panel" art is `0x0BB8`, a
9-slice `ResizePic` ClassicUO puts behind *editable* textboxes at a fixed 25px
height (`UserMarkerGump.cs`, `LocationGoGump.cs`, `MessageBoxGump.cs`,
`MarkersManagerGump.cs`) — multi-piece, taller than the 18–20px target, and
built for a different control. Conclusion this cost the research to reach: a
generic blank text-plate is not a gump-art concept in this protocol at all: the
reference client renders that particular shape of "a box behind some words" as
paint, never as art, everywhere it needs one.

**So stop looking for a plate and use a button.** The question "what backs my
caption" was the wrong one; the right one is "what does the player press". The
client ships exactly one *generic* button — `0x0FA5`/`0x0FA7`, 30×22, the pair
every shard names as `4005`/`4007` in its own `0xB0` dialogs — and it carries
no baked-in word, unlike the paperdoll's six (`0x07D6` "OPTIONS", `0x07D9`
"LOG OUT", `0x07DF` "SKILLS", `0x07EB` "STATUS", `0x07EF` "HELP", `0x07E5`
"PEACE"). Its neighbours in the same family are the same size and shape with
a different picture on them: `0x0FB7` "OK", `0x0FB1` a cross, `0x0FBD` a
stack. Put the button in the art and the caption *beside* it, which is the
row every server dialog already draws, and the plate problem does not exist:
there is no rectangle to back, the press has a real pressed face instead of a
hue, and the caption is free to be any length. `container.rs`'s two actions
are drawn this way.

## Traps in tests and benchmarks

**A benchmark where nothing moves measures nothing.** A player who does not walk is
drawn once and never redrawn, so a standing world never pays interest management —
no `refresh_around`, no first-sight draw, none of the per-draw work of assembling
what a neighbour is wearing. `examples/town_bench.rs` reports standing and walking
side by side because the gap between them was three orders of magnitude: 0.107 ms
against 8.9 ms for the same town. The same applies to what a benchmark *builds* —
its predecessor spawned every creature with `equipment: Vec::new()` and placed no
decoration, so it exercised neither of the two columns a real facet spends its tick
in.

**A statistical test needs a companion that says the data is real.** The map test
asserting "neighbouring tiles have similar heights, so the block order is right"
passed against a `map0.mul` that was 90MB of zeroes — all-zero terrain is
perfectly smooth however you index it. `terrain::tests::the_map_is_not_degenerate`
exists to stop that. Any test that measures a property of real data can pass
vacuously on absent data.
