# A house, and the ground it stands on

Placement, the walls that stop you, the door that knows you, the decay that
takes it away — **one plan, because they are one object**. A house is not a
feature made of parts that could ship separately: a house you can place and walk
through is not a house, and a house with a lock and no decay is a shard that
fills up and never empties.

**H1–H5 are built.** A house goes down from a deed, its walls stop you, its door
and its secures know you, its sign says who owns it and how it is wearing, and a
house nobody visits collapses into a crate that keeps what was inside. Each
phase's section below records what came out differently from the plan, which is
the half worth reading.

**H6 is a sixth phase of a five-phase plan**, for a reason worth reading before
the rest: three things this document published as *decided* were never built, and
they are one thing — housing and regions never met. Two of its three sub-phases
are now in; the third turned out to need a decision rather than a session, and
the note under D11 says which.

> Read [`architecture.md`](architecture.md) for where a system crate sits and
> what it may depend on, and [`style.md`](style.md) before writing any of it.
> This document does not restate either.

## What a multi is, and what is already read

A **multi** is one item that draws as many. The wire carries a house as an
ordinary world item whose graphic is `0x4000 + id`; the client looks that id up
in its own `multi.mul` or `MultiCollection.uop` and draws the hundred and
forty-eight statics a villa is made of. **The shard sends none of them.**

That is the whole reason this is tractable. The picture is free — every client
already owns every house. What the shard owes is everything the picture does not
say: where the walls are for the purpose of stopping somebody, who may open the
door, what happens when nobody pays the upkeep.

[`openshard_uofiles::multi`](../crates/common/uofiles/src/multi.rs) reads the
components and is **built**. Three things about that format are written down in
[`findings.md`](findings.md) and are not worth re-deriving: the High Seas widening
and the arithmetic that detects it, the drawn/skip flag that runs *opposite ways*
in the two files, and the fact that the two files disagree about how many multis
exist (326 against 862 on one install, so the UOP wins).

## What is missing, in one table

| piece | server | client (ours) | classic client |
|---|---|---|---|
| multi components | **built** (`uofiles::multi`) | — | reads its own files |
| `0x99` multi target cursor | **built** | **built** — read and drawn | speaks it |
| a house as a world item | **built** | **built** — `net_command::multi_pieces` | draws multis already |
| the footprint blocking a step | **built** | — | n/a, server-authoritative |
| the house sign, the deed | **built** | draws items already | ordinary items |
| door locks | **built** — `KeyValue`, the lock rules, and the house's own gate | — | n/a |
| co-owners, friends, bans | **built** | n/a | n/a |
| decay | **built** | n/a | n/a |
| where a house may not go (`no_housing`) | **built** — H6 gave the flag its reader, and closed 21 dungeons | n/a | n/a |
| customisation (`0xD7` house design) | — | — | speaks it |

`0x99` is the one packet that has to be written from nothing on both ends. It is
ServUO's `MultiTargetReq`: 26 bytes classic, 30 post-High-Seas — a target
request with the multi id and an offset appended, so the client draws the house
under the cursor while the player picks a spot. The reply is an ordinary `0x6C`,
which this engine already reads and our client already sends.

## Decisions, taken here

**D1 — a house is an entity with a `Multi` component, not a new kind of thing.**
It has a `Position`, a `Drawn` whose graphic is `0x4000 | id`, and a serial from
the item pool. Everything that already walks items — the sector index, the save,
the `0x1A` that draws it — works on it unchanged. What makes it a house is
a `House` component beside those, not a separate table.

**D2 — the footprint is an obstruction, computed at placement and stored.**
`FacetState::obstructions` is the index a step already asks. A house adds its
drawn components to it, each at its own `dz` with its tiledata height, exactly as
a static would be. Not computed per step from `multi.mul`: a step is ten a second
and a house does not move.

The consequence to accept up front: **a house is not in the map file**, so the
obstruction index is no longer purely a function of the client's files. It is the
files plus what the shard has placed. That is already true of doors and dropped
items; a house is the first thing that adds a *hundred* entries at once.

**D2a — `openshard-state` never holds the multi table.** It does not depend on
`openshard-uofiles` and must not start: the components are resolved **at
placement**, by a caller that has the table, and what is stored is the
obstruction entries and the `House` component. At boot the saved houses are
restored by the boot code, which already reads the client's files. This is D2's
"computed at placement and stored" spelled as a dependency rule, and it is what
keeps a *client file* out of the crate every gameplay system builds on.

**D2b — one entity blocks one tile at several heights.** `Obstructions::block`
keyed an obstacle by its entity, because the case it was written for was a door:
one thing, one height, re-registered to refine it. A house is one entity whose
walls stand on top of each other, so the key is the entity **and the z** — done,
with a test, before anything above it was written. Keyed by the entity alone the
second registration overwrote the first, which does not read as a missing wall
but as the wrong floor being sealed, since which one survived depended on the
order the components came out of the file in.

**D3 — the placement rules are ServUO's five, and the fifth is the one to get
right.** From `HousePlacement.Check`: nothing impassable around the outside, no
impassable tile touching the house, five tiles clear front and back, the
foundation rests flat, and **no foundation tile over a road**. The road rule is
the one a player notices the absence of, because without it houses appear across
Britain's streets.

Staff place anywhere, which is ServUO's own first branch and is what makes the
rules testable before the deed exists.

> **Not built until H6, and the sentence above went on claiming it.** `place`
> takes `owner: Serial` and never an actor `EntityId`, so it has nobody to ask
> `is_staff` about and every refusal applies to a game master exactly as it does
> to a player. Left standing here rather than rewritten, because what a decision
> *said* is worth keeping beside what the code did: this is the one D that was
> read as built for five phases.

**D4 — a region, and it comes free.** `Regions` already exists and already
carries flags. A house is a region with its own flags (no teleport in, no
recall out), placed and removed with the house. Nothing new is needed for this,
which is why it is a decision rather than a phase.

> **"Nothing new is needed" was true and "it comes free" was not.** Nothing *was*
> needed — `Regions` is per-facet on `FacetState` and `Regions::at` is a bucketed
> lookup that has worked the whole time. But a decision with no phase under it is
> a decision nobody is assigned, and five phases went by without one line of
> `openshard-housing` mentioning a region. **H6 is that phase.** The lesson is
> cheap and worth writing down: a "this comes free" decision needs a phase to be
> free *in*, or it is not a decision, it is a hope.

**D5 — the door is the door this engine already has.** `.key` and `KeyValue` and
the lock rules landed with the traps work, on the argument that Britannia locks
exactly one container and the rules would otherwise be unreachable. A house door
is that mechanism with the house's own key, and the reason it is D5 rather than
a phase is that there is nothing to build — only to connect.

**D6 — decay is a tick count, not a wall clock.** Everything in this engine that
measures duration counts ticks, because a tick count replays and a clock does
not. ServUO's five days becomes a tick count in `Gameplay`, an operator setting
like every other duration. A house refreshed by its owner walking in resets it.

**D7 — customisation is out of scope, by name.** ServUO's `HouseFoundation` and
the `0xD7` design packets are a second system the size of this one: a design
buffer, a preview state, a commit, and a whole editor on the client. A *classic*
house — placed from a deed, fixed shape — is the whole of this plan. The
foundation ids (`0x13EC`–`0x1D00`) are named here only so that the placement code
refuses them loudly rather than placing a house with no stairs.

**D8 — the moving crate is not deferred, it is the deletion rule.** When a house
goes, what was inside it has to go somewhere, and "somewhere" being the ground is
how a shard loses a player's belongings. ServUO's moving crate is a container the
contents land in. It is small and it belongs to the phase that can destroy a
house, not to a later one.

## The phases

Five, in the order they are worth having. Each leaves the shard strictly better
and none depends on a later one's shape.

---

### H1 — a house on the ground, placed by staff

**What a player sees:** a house, where a game master put it, and walls that stop
them.

1. A `House` component in `openshard-state`, and the multi table loaded at boot
   beside the map and the tiledata — in the *boot* code, not in `state`. See D2a.
   The obstruction key is already widened; see D2b.
2. `openshard-housing`: `place(state, at, facet, multi_id, owner)` — spawn the
   item, fold the components into `Obstructions`, refuse a footprint that does
   not fit.
3. D3's five rules, staff-exempt.
4. `.house <multi id>` — a staff command, `.add`'s shape.
5. Saved: a `HouseRecord` (serial, multi id, position, facet, owner) and the
   obstruction rebuilt at boot from it rather than saved. The components are a
   pure function of the id, so saving them would be saving a copy of the client's
   file.

**Done when:** `.house 0x64` puts a villa on the ground, both clients draw it,
walking into a wall is refused and walking through the door is not, and it is
still there after a restart.

#### Built

Items 1–4 are in. `openshard-housing` is the crate, `.house <multi id>` is the
command, and the footprint is folded into `Obstructions` at placement. Two things
came out differently from the plan and one is still open:

- **The multi table hangs off `Terrain`, not off world state.** D2a said the
  components are resolved by "a caller that has the table"; the code found a
  better answer, which is that `Terrain` is *already* the seam client-file facts
  reach gameplay through — `item_blocks` and `item_height` are how a placed
  static learns whether it stops anybody and how tall it is, and a multi's shape
  is the same kind of fact. So `Terrain::multi_components` is a new method with
  an empty default, `MapTerrain` carries an `Arc<Multis>`, and `openshard-housing`
  depends on neither `uofiles` nor a table on `WorldState`. It also answers the
  "what if the shard has no client files" question for free, the way every other
  method on that trait already does.
- **Only the floor question is decided at all.** A component is folded into the
  footprint when its tiledata says it blocks, which keeps a floor and a roof
  walkable — a house whose floor blocked would be sealed shut from the inside.
  That is the *component* half.
- **D3's rules are in, and two of the five turned out to be one question.**
  ServUO's rule two (nothing impassable in contact) and rule four (the foundation
  rests on a surface) are both "is there an open gap with a floor here", which is
  exactly what `Terrain::can_fit` already answers against the map's own statics —
  so they are one call and one refusal, `BadGround`. Rule five is the road, a
  land-tile id against ServUO's nine ranges. Rule three is the yard, and it is
  measured **wall to wall against the other house's own footprint** rather than
  against a stored rectangle: a footprint is what a house *is*, and a rectangle
  would be a second copy of it to keep in step.

  One divergence, deliberate: ServUO's yard is a *strip* five tiles off the front
  and back, because a foundation knows which way it faces. A classic multi does
  not carry a facing, so the yard here is a square. Named in the code rather than
  left to be discovered as a bug.
- **A house is two facts and they are undone separately.** `unblock` takes the
  walls out of the obstruction index; the *entity* still owns a yard until it is
  despawned. A demolition that called only the first would leave a plot nobody
  could ever build on again — asserted in the tests, and it is the shape H5's
  moving crate has to get right.
- **The save is in, and what it saves is where the house stands.** Not the
  components: a multi's shape is a pure function of its id and lives in the
  client's files, so a copy would go stale the day the operator updates their
  install — and then the shard's walls and the client's picture disagree with
  nothing to say which is right. The footprint is recomputed at boot from the
  same table placement read it from.

  A restore does **not** go through `place`. That decides whether a house *may*
  stand somewhere, and a house legal when it was built stays built: a shard that
  changed its yard size would otherwise demolish half of Britannia at the next
  restart, silently.

  Schema **v27**, and it is the first bump that is not about reading. A v26
  database opens fine and holds no houses, which is true of it. What it must not
  do is keep being written by a build that does not know about them — an older
  engine would agree about the version, ignore the table, and hand out item
  serials one of which a saved house already holds. The bump is for the *writer*.

  A shard booted **without** client files restores the houses and gives them no
  walls, rather than dropping them. Losing somebody's property over a
  misconfigured `world.client_files` is the worse failure, and it is gated.

---

### H2 — the deed, and the cursor that shows the house

**What a player sees:** they buy a deed, double-click it, and the house follows
the cursor until they pick a spot.

1. `MultiTargetRequest` in `openshard_protocol` — `0x99`, both lengths, the
   version boundary the other High Seas packets already use.
2. The client half: `WorldView` folds it, and the app draws the multi under the
   pointer. Our own client owns `multi.mul` too, so the picture is a lookup.
3. `TargetPurpose::PlaceHouse { deed }`, answered by the `0x6C` this engine
   already reads.
4. The deed as an item, and the vendor that sells one.

**Done when:** a deed bought from a vendor places a house where the player
clicked and is consumed; a refused placement says which of D3's rules it broke
and keeps the deed.

#### Built on the server; the client half is not drawn yet

Items 1, 3 and 4 are in, plus `.deed <multi id>` so the path is reachable on a
running shard before a vendor sells one. `.house` remains the staff shortcut that
places directly; a deed goes through every placement rule with the house drawn
under the pointer, which is the whole reason `0x99` exists.

- **`0x99` is not an `EncodePacket`.** Its `LENGTH` is a `const` and the packet
  has two — 26 classic, 30 from High Seas, the same bytes with four zeroes on the
  end. Declaring either would be a lie the framer's own assertion catches, so it
  follows `OpenContainer`'s precedent: an inherent `write_body`, a
  `multi_target_length(version)` beside it, and `ServerPacket::length` (which
  *does* see a version) picking between them.
- **`MultiId` exists now**, because the N10 gate refused a bare `u16` and was
  right to: a multi id and the graphic a placed multi draws as are two `u16`s
  that overlap, `0x99` writes the bare one and `0x1A` the masked one, and a value
  holding neither type cannot say which it had.
- **The cursor carries the *deed*, not the multi.** A deed sold, dropped or
  destroyed while the cursor was up must not still place a house, and a player
  with one deed and a fast hand must not place two — so the deed is re-read when
  the click lands, and the multi comes off it then.
- **The deed is spent on success and kept on a refusal.** A player who picked a
  bad spot has lost a click, not a house.

**The preview is drawn, and it went in the shape the deferral predicted.**
`0x99` is folded into `WorldView` as `OpenTarget`, which carries the cursor and
the house as **one value** — so a plain `0x6C` arriving after a house cursor
cannot leave a villa following the pointer. Two `Option`s side by side would have
been one packet away from exactly that, which is `combat.md`'s D1 in a different
colour.

The pieces live in `presentation.multi_preview`, a list of their own, and the two
reasons they are not in `presentation.items` are **not the same reason**. A
preview has no serial, so appending it would desync `item_serials`, which picking
indexes by position — and it must not be pickable in any case, because it is not
a thing in the world. It is chained onto `items` at the one call that builds
`frame::Inputs`, borrowed rather than copied when there is no preview, so the
renderer never learns there were two lists and an ordinary frame allocates
nothing.

It is rebuilt **once a frame** rather than once a packet, which is the one thing
in the draw list that is: it follows the pointer, and the pointer moves between
packets. And `items_fingerprint` is taken over the *chained* list, not over
`presentation.items` — the static geometry cache is keyed on what the frame
draws, and a preview that slid a tile without changing the key would be a house
frozen where the pointer first was. That is the one assertion the test makes,
because it is the one a reader would not think to check.

The tile is `pick_tile`'s, not the picked static's, and that differs from
`target_under_cursor` on purpose: a house goes on the *ground*, which is why the
shard raises a location cursor for it, so what the player sees is where the house
will land.

---

### H3 — who may come in

**What a player sees:** their door opens for them and not for a stranger.

1. The house sign, the ownership it names, and the gump it opens.
2. Co-owners, friends and bans — three lists with ServUO's own limits, and the
   same "an invitation is a consent" rule guilds already have.
3. The door: locked to the house, opened by owner and friends, and the key.
4. The ban: a banned player standing inside is moved out, which is the only rule
   here that acts on somebody rather than refusing them.

#### Built: all four

Item 2 is in, and the shape it took is worth naming: **one question, not four
booleans.** The reference's predicates are nested — `IsFriend` is
`IsCoOwner(m) || Friends.Contains(m)`, and `IsCoOwner` is `IsOwner(m) || ...` —
so four independent answers are four chances to ask the wrong one.
`Standing` is an ordered enum instead, and `standing_of` is the only place the
order of the checks lives. `Banned` is its *lowest* value, so a comparison reads
"at least this trusted" and a ban is never that.

Four rules came out of it:

- **The owner and staff cannot be banned**, which is the reference's own first
  branch and what stops somebody banning the owner out of their own house.
- **Only the owner names a co-owner**; a co-owner names friends. A co-owner who
  could name another would be handing the house to a crowd the owner never met.
- **Promotion moves rather than adds.** Somebody in both lists is two answers to
  one question, and `standing_of` would silently prefer whichever it checked
  first.
- **A ban wins over trust and takes it away.** "Banned but still a co-owner" has
  no useful answer, and the ban is the thing that was just decided. Lifting one
  gives back a *stranger*: undoing a ban grants nothing.

Saved, schema **v28** — v27's argument one turn further. A v27 build knows about
houses and not about who may enter one, so it would read a house, drop the three
lists and write it back. That is not a shard with no lists; it is a shard that
deletes them on the first save.

**The door is gated, and the layering question was answered by taking option 2.**
`Standing` and `standing_of` live on the `House` component in `openshard-state`
now, because a *door* has to ask them and the double-click dispatch is
`openshard-items`', which has no business depending on the housing crate — a door
is not a housing concept. It is `Guild::at_war_with`'s split exactly: the rules
(trusting, banning, the limits) stay in the system crate, and the question a wire
path asks lives on the component. `openshard-housing` re-exports the type.

A house door refuses a stranger **before** the lock is asked about, because a
stranger at a friend's door is refused for *being* a stranger, and "that is
locked" would send them looking for a key.

**A house adopts the doors standing inside it**, and that is a rule this plan
chose rather than inherited. The obvious source is the multi, and the shipped
file says no: **three** of its 326 multis carry a door component. The reference
agrees — ServUO calls `AddDoor` from each house class with an explicit graphic
and position, which is a per-house-type table of content this engine does not
have and should not invent. So the rule is the one a player would state: a door
inside your house is your house's door. It needs no table and it is right for a
door put down by a pack, by a staff command, or by a customisation system that
does not know about it.

The adoption uses `tiles_of`, **not** the footprint, and the difference is the
whole of it: a door stands in a *doorway*, which is by construction a gap in the
walls — the one place a blocking footprint does not reach. Using the footprint
adopted nothing, which a test caught rather than a player.

**The eviction is the one rule that acts on somebody.** A ban that only locked
the door would leave whoever was already inside there for good. `evict_the_banned`
puts them one tile past the box's west edge. ServUO moves them to a
`BaseBanLocation` each house class declares — a hand-written table, like the door
positions and the sign offsets — and "just outside, on the side the box ends" is
the same intent from data that exists. It is deliberately **not** the sign's tile
now that there is one: the sign hangs on the wall at z+7, which is a place for a
plaque and not for a person.

**Reachable through five staff commands** — `.hfriend`, `.hcoowner`, `.hdrop`,
`.hban`, `.hunban` — each raising an object cursor, because naming a mobile needs
a lookup this engine has no verb for and *picking* one is what the reference's own
sign does. When the sign exists it is a window over exactly these five calls.

**The sign is up, and its position is the one number the reference derives.**
ServUO's fourteen classic houses each declare theirs — `SetSign(2, 4, 5)`,
`SetSign(5, 12, 16)` — which is the same per-house-type table the doors are, and
for the same reason it is not copied. But its *customisable* houses cannot have
one, because the multi is built at run time, so `HouseFoundation` computes a spot:
`Components.Min.X`, `Components.Height - 1 - Components.Center.Y`, `z + 7`.
Reduce that against `Multi::center`'s own definition and the y is just `max_y` —
so the rule is **the box's west-south corner**, and it holds for every multi
rather than only the ones somebody typed a number for.

The arithmetic is `uofiles::multi::bounds`, pulled out of `Multi::new` and made
public so the sign asks the same function the centre was computed by. A second
copy of it in the housing crate would be a copy that can drift, and the whole
point of matching the reference's bounds was that `center` agrees on both
engines.

The hanger (`0xB98`) ServUO puts on the same tile is left out: it draws a bracket,
does nothing, and is one more entity per house to save, restore and take down.

**The sign is not saved.** It is derived from the house — position from the
multi's box, ownership from the `House` component — so `restore_houses` hangs a
fresh one, exactly as the walls are recomputed rather than stored. Which uncovered
a defect in H1: the house entity *itself* has a graphic and a position, so
`ground_items` was sweeping it up as an `ItemRecord` **as well as** writing a
`HouseRecord`, and the restore — houses first, items second — then found its own
serial already spoken for. Both are excluded now, with a test.

**The window is a window over the five verbs.** The five buttons raise the same
cursor `.hfriend` and its four siblings do; the rows are the half a cursor cannot
do — taking somebody *off* a list without asking them to stand still for it. The
cursor's answer and the window's rows both go through `sign::apply`, so there is
one authority check and one eviction rather than two that must agree.

Which list a row was drawn under decides its verb — a co-owner or a friend is
dropped, a banned player is let back to the door — so one button id serves all
three columns, and `HouseGumpContext` remembers which column each row came from.
That is `openshard_guilds::gump`'s rule: a reply names a *number*, and what the
number meant is the server's memory.

**One thing it inherits rather than fixes:** a name is only there while its owner
is logged in, because a serial resolves to an entity and an offline character has
none. The fallback is the serial rather than "someone", so two absent friends are
two rows a player can tell apart. The guild roster has the same gap and the fix is
the same one — a name read off the character store — which neither has.

**H3 is complete.**

---

### H4 — lockdowns and secures

**What a player sees:** they can put things down inside and find them there.

An item inside a house is ordinarily loose and decays. A **lockdown** is an item
pinned in place; a **secure** is a container only named people may open. Both are
counts against a house's own storage allowance, which is what stops a house being
a bank box with a roof.

`items::capacity` is the shape this reuses — the ceiling exists, and a secure is
a container with an owner list on top of one.

#### Built, and the allowance is derived rather than tabled

**A secure is a lockdown, so there is one component and not two.** `LockedDown`
carries the house and an `Option<Standing>`: `None` is a plain lockdown, `Some`
is a secure and the value is the least standing that may open it. Two components
would be two facts that must agree about three separate rules — that neither
lifts, that releasing works on both, that both count against the same allowance —
and the reference's own model is this one, since `BaseHouse.Release` takes a
secure off the list in a single step.

The access level is a `Standing` because ServUO's `SecureLevel` is the *trusted
half of it* with a fourth name for its bottom: `Owner`, `CoOwners`, `Friends`,
`Anyone`. `Standing::Stranger` **is** "anyone", and a banned player is still
below it — which is the right answer and one a separate four-value enum would
have had to remember to give.

**The allowance is derived from the multi's own area.** ServUO's is a table:
`HousePlacementEntry` carries a lockdown count for each of its thirty-odd multi
ids, hand-written beside the price and the placement offset — the same kind of
per-house-type content the door positions and the sign offsets are, and not
copied for the same reason. What the table *is*, plotted against the `Area`
rectangles each matching house class declares, is roughly linear:

| house | tiles | ServUO lockdowns | per tile |
|---|---|---|---|
| small old house | 52 | 212 | 4.08 |
| small tower | 59 | 290 | 4.92 |
| two-storey villa | 125 | 550 | 4.40 |

So `LOCKDOWNS_PER_TILE` is **4** and the derived numbers land within a sixth of
the reference's on every row — a shard's own tuning knob rather than a promise of
parity, and one an operator turns without editing thirty ids. The second ceiling,
on what sits *inside* the secures, is exactly twice the first on every row of
ServUO's own table, so it is derived from the first and there is one number.

**Computed at placement and stored, which is D2 one level up.** The count is a
`u32` on the `House` component, because the path that needs it — the drop into a
secure, in `openshard-items` — has no terrain in hand and has no business
acquiring one. ServUO stores its own `MaxLockDowns` on `BaseHouse` and saves it
for the same reason. It is **saved** rather than recomputed, unlike the walls and
the sign, and the difference is the tuning constant: recomputing at boot would
mean an operator who lowered `LOCKDOWNS_PER_TILE` finding half the shard over the
new ceiling with nothing to say which lockdowns to drop.

**Both gates ask the component, not the crate** — the third time the layering
question has been answered the same way, after `Standing` and the door. A lift
refuses anything with a `LockedDown` and needs no housing rule at all, because
the answer does not depend on who is asking: a co-owner cannot lift their own
lockdown either, they release it first. Opening a secure asks
`WorldState::may_open_secure`, which lives beside the data exactly as
`standing_of` does.

The secure's refusal is said with the *door's* line and not the lock's, which is
why it is a separate check from the lock at all: a stranger at a secure is
refused for being a stranger, and "that is locked" would send them looking for a
key that does not exist.

**Two ceilings over one drop**, and they count different things: the container's
is about that container's own subtree, and the house's is about everything stored
across all of its secures, one level deep. A bag inside a secure chest is one
item against the house and its own contents are `capacity`'s problem.

Saved, schema **v29** — v28's argument a third time and the sharpest of them,
because this one is not a list on the house but a component on every pinned
*item*. A v28 build reads those as ordinary ground clutter, writes them back
without the pin, and a shard comes up with every lockdown released and every
secure standing open.

**Reachable from the sign**, which is where a player would look: five more
buttons beside the five list ones, raising a cursor for the item. The cursor
carries the *house*, unlike the list cursors which resolve to "the house the
actor is standing in" — a list change is about a person and the actor is inside
their own house while making it, but a lockdown is about an item, and somebody
who pressed the button by the sign is standing outside the walls the item is
behind.

**H4 is complete.**

---

### H5 — decay, and the crate

**What a player sees:** a house nobody has visited falls down, and what was in it
is not lost.

1. D6's tick count, the refresh, and the five stages ServUO names.
2. D8's moving crate.
3. Demolition by the owner, which is the same path arriving deliberately.

#### Built, and D6 needed amending on one point

**The clock is an accumulator, not a deadline** — the one timer in this engine
that is. `Decays` and `MurderDecay` are both an absolute `at_tick`, which works
because they are minutes long and die with the process. A house's is five days,
and `WorldState::ticks` **starts at zero every boot**: the world saves a clock in
UO minutes and not a tick count, so a deadline written as an absolute tick means
nothing on the way back in, and every house on the shard would come up freshly
refreshed. So `House::age` counts up — one add per house per tick over a handful
of them — and saves and restores as the one number it is. D6 said "a tick count,
not a wall clock" and that still holds; what it did not say is which end of the
interval to store.

**Six stages, and they are the reference's own thresholds** — 5, 250, 500, 750,
950, 1000 per mille of the period, from `GetOldDecayLevel`. Two of ServUO's nine
are dropped by name: `Ageless` is a staff flag this engine has no concept for,
and `DemolitionPending` means a rented vendor is still standing inside. The
thresholds are written out rather than divided, because they are **not evenly
spaced**: the first band is half a percent of the period and the last is five.

**The sign refreshes it, and the walk does not.** ServUO refreshes on its own
house menu post-AoS and on the owner walking in before that; the walk is not
copied because `house_at` is a scan over every house on the shard — fine when
somebody presses a button, not fine ten times a second per player. The sign is
where the reference itself moved to, and the sign already draws the condition
line, so the player who checks is the player who refreshes.

**A period of zero turns decay off**, and turns the *counting* off with it: a
shard that never wants a plot to free up does not want a counter climbing toward
a threshold nobody will read.

**The crate is the deletion rule, which is D8, and it holds.** Everything locked
down, everything secured, and everything inside a secure goes into one crate on
the house's own tile — the secures go in whole, so a chest keeps its contents
rather than being emptied beside them. What is *not* swept up is the loose
clutter: an item somebody dropped on the floor and never pinned was on the ground
before the demolition and is on the ground after, which is where it already was.

What the crate does not do is stated rather than left to be discovered: it does
not decay and nothing collects it. ServUO internalises its own after three hours
and hands it to the owner's bank — a real feature, and not this one. A crate that
rotted would be a shard that eats somebody's belongings on the day their house
came down, which is the failure this phase exists to prevent, so the crate stands.

**A lockdown does not rot**, which needed fixing in two places rather than one:
`mark_decay` skips a pinned item, and the sweep skips one too. The second is the
case that needs it — an item is dropped loose, which marks it, and locked down
*after*, so the clock is already running when the pin arrives. `lock_down` also
takes the component off, or releasing it would restart whatever remained of a
twenty-minute timer set before the house existed.

**Demolition is the same path arriving deliberately.** The owner's button on the
sign, and `.hdemolish` for staff — the case the sign is no help for, which is an
abandoned house whose owner will never open it, standing on a plot somebody else
wants. The owner check is re-asked when the button comes back, because a window
outlives the standing that drew it.

Schema **v30**, and it is v27's case rather than v28's: one column, and the bump
is for the *writer*. A v29 build opens the database, ignores `houses.age`, and
writes every house back at the default — so every house becomes freshly refreshed
on the first save and nothing ever collapses again.

**H5 is complete, and so is this plan.**

### H6 — the region a house stands in

**What a player sees:** they cannot build in Covetous, and standing in their own
hall stops a stranger recalling in on top of them.

The sixth phase of a five-phase plan, and it is half a correction. Three things
this document published as decided were never built, and they are all the same
thing: **housing and regions never met.** `grep -rn region crates/server/housing/src/`
finds test scaffolding and nothing else.

1. `no_housing` — a `RegionFlags` field with data behind it and no reader.
2. D3's staff exemption — `place` has no actor to exempt.
3. D4's house-as-region — decided, never assigned to a phase.

#### The flag is dead and the data is not

`RegionFlags` (`crates/server/state/src/region.rs:93-112`) is five bools, fully
plumbed: `data/regions.json` → `build.rs` codegen → `Regions` → `RegionRecord` →
save → restore. Two of the five are read by nothing: `no_housing` and `safe`.

They are not the same case, and the shipped dataset is what tells them apart.
`regions:felucca`, facet 0, **128 regions**:

| flag | rows | reader |
|---|---|---|
| `guarded` | 51 | `npc::guards.rs:128` |
| `music` / `light` | 38 / 23 | the region-crossing pass |
| **`no_housing`** | **21** | **none** |
| `no_teleport` | 2 | `runtime.rs:2096`, `magic/travel.rs:88` |
| `no_recall` | 2 | `magic/travel.rs:88` |
| **`safe`** | **0** | **none** |

So waking `no_housing` closes twenty-one places on the first boot — Covetous,
Deceit, Despise, Destard, Hythloth, Shame, Wrong, Khaldun, Terathan Keep, Fire,
Ice, the Solen Hives, Sanctuary and seven more. Waking `safe` closes nothing,
and would commit the engine to a PvP rule whose other half does not exist.

**So `safe` stays asleep, deliberately and in writing.** A dead flag with a
reason recorded is a different thing from a dead flag nobody mentioned, and the
whole reason this phase exists is that the second kind is invisible.

#### Decisions

**D9 — the rule is stated over the whole footprint, not over the origin.** A
house is many tiles and a region is a set of rectangles with a height band, so a
villa can straddle a dungeon mouth. Testing the origin alone would let a player
build a house whose back half is inside Shame by standing one tile outside it —
and the failure is invisible until somebody notices the walls. `check_ground`
already walks every footprint tile for the road and `can_fit` rules; the region
check walks the same list, and one tile inside a `no_housing` region refuses the
house.

There is a third reason and it is the one that would have bitten first: **the
origin is not reliably part of the house at all.** `place`'s own doc says so —
`at` is the multi's origin and "is not the corner of its box". A multi whose
components all sit at positive `dx`/`dy` has an origin outside its own drawn
area, so an origin test can test a tile no wall ever stands on.

The set walked is **`tiles_of` and not the footprint**. A floor is inside a
dungeon as surely as a wall is, and `footprint_of` deliberately drops everything
that does not block. This costs nothing: `place` already calls `tiles_of` (at
`lib.rs:177`, to size the lockdown allowance), so hoisting that one call above
the checks gives both readers the same list.

The cost to name: `Regions::at` is a bucket lookup plus a linear rectangle test
per candidate, run once per covered tile rather than once. A hundred lookups when
somebody clicks is the bargain `check_yard` already takes.

**D9a — the height tested is the house's own `z`, once, and never the
component's.** `RegionRect` carries a `z_min`/`z_max` band (`region.rs:52-54`)
and 247 of the shipped rects use one — that is what keeps the open sky above a
dungeon open. A villa's roof stands twenty units above its foundation, so
testing each tile at its component's z would put the roof *outside* a banded
dungeon region and answer "not in Covetous" for the top half of a house that is
unambiguously in Covetous. A house is sited at one height.

**None of the 21 `no_housing` regions is banded today**, so this decision changes
no answer on the shipped data. It is written down because the failure it prevents
is a player building a tower in a banded dungeon, and that is discovered by the
player rather than by a reader.

**D9b — the region refusal comes before the ground refusal.** The order of the
checks is the *message*, not an implementation detail. `BadGround` means "try a
tile over"; inside Deceit that sentence is a lie, and a player who believes it
spends ten minutes proving it. So `check_region` sits between `footprint_of` and
`check_ground`: it needs the covered tiles, so it cannot come earlier, and it is
the only one of the refusals that is a statement about the *place* rather than
about this attempt.

**D10 — `place` takes the actor, not just the owner.** It cannot ask `is_staff`
about a `Serial`, which is why D3's exemption has never existed. The signature
gains an actor `EntityId` and the exemption is the shape every other one in this
engine has — `if state.is_staff(actor) { … }` as a first branch, as
`WorldState::may_teleport` (`runtime.rs:2089`) and `magic::travel::may_travel`
(`travel.rs:82`) both do.

**Which rules the exemption covers is its own decision, and it is not "all of
them".** ServUO's is a single early return — `if (from.AccessLevel >=
AccessLevel.GameMaster) return Valid;` — and copying that shape literally would
be wrong here, because this engine's `Refusal` mixes two kinds of answer:

| refusal | what it is | staff-exempt |
|---|---|---|
| `Occupied`, `OnARoad`, `BadGround`, `TooCloseToAHouse`, `NoHousingHere` | a judgement about the plot | **yes** |
| `NoSuchMulti`, `DrawsNothing`, `NeedsCustomisation`, `OffTheMap`, `NoSerials` | there is nothing to place, or the shard is broken | **no** |

Exempting the second row spawns an invisible house out of a treasure-site marker,
or a foundation with no stairs — which is the exact failure `NeedsCustomisation`
exists to prevent. **A staff bypass that reopens a hole another decision closed
is not an exemption; it is a regression with a permission check on it.**

The `staff: bool` is computed once at the top of `place` and threaded, which is
this crate's own idiom: `trust`, `distrust`, `ban`, `unban` and `standing_of` all
take it that way rather than each asking `is_staff` again.

Both callers change: `gm::place_house` (`world/src/gm.rs:488`) already has the
actor in hand, and `houses::place_house_from_deed` (`tick/houses.rs:138`) has it
as the mobile whose deed is being spent.

> **D11 is not implementable as written, and the check that found it is the one
> this phase exists to institutionalise.** `Regions`' whole public surface is
> `new`, `set`, `clear`, `at`, `get`, `iter`, `len`, `is_empty` — **there is no
> runtime insertion or removal**, and `set` is replace-all by design: its own doc
> argues for that, because "a registration carries the whole set, so registering
> twice cannot leave a stale half behind". D11 contradicts a stated property
> rather than filling a gap.
>
> Four blockers, and the third is decisive on its own:
>
> 1. **`RegionId` is a `Vec` index and `at()` indexes it unchecked.** A removal
>    that shifts the vector renumbers everything above it, silently invalidating
>    every live `InRegion` and every saved `RegionRecord::id`. Removal has to be a
>    tombstone, which changes the type rather than adding a method.
> 2. **The save sweep would write the house's region.** `region_records` maps
>    `regions.iter()` straight out, so a derived region is saved, restored, *and*
>    re-derived — two regions over one house, and the saved one outlives the
>    demolition for ever as a no-recall zone in an empty field. That is the exact
>    failure D11's own test list names.
> 3. **The boot order destroys it.** `restore_houses` rides inside
>    `restore_guilds` at `boot.rs:263`; `restore_regions` runs at `:270` and ends
>    in `Regions::set`, which replaces everything. Any house region registered at
>    restore is wiped seven lines later, on every boot, silently.
> 4. `reindex()` is a full rebuild, so an insert either pays it per placement or
>    re-implements the grid fill.
>
> **So D11 stays deferred, and what it needs first is a decision rather than
> code**: how a derived region gets an id that `InRegion`'s crossing diff and
> `RegionRecord` can both live with. The shape that keeps every property D11
> states is a second, house-keyed layer inside `Regions` — consulted by `at()`,
> invisible to `iter()`, with ids from a range that is not a `regions` index. That
> belongs in this document before it is in the tree, which is the whole lesson of
> the note under D4.

**D11 — a house's own region is derived from its footprint, not stored beside
it.** D4 said a house *is* a region and left the shape open. The shape follows
from H1's own rule: what is saved is where a house stands and which multi it is,
and the footprint is recomputed at boot from the same table placement read it
from. A stored rectangle would be a second copy of that, free to disagree with
the walls after an install update — the exact failure H1 refused for the
components.

So the region is registered from `tiles_of` at placement and at restore, on the
same call that blocks the walls, and removed by `decay::demolish` on the same
call that unblocks them. `Regions` is per-facet on `FacetState` (`runtime.rs:399`),
which is where the house's own `Obstructions` entries already live, so the two
halves cannot end up on different facets.

**What flags a house's region carries** is the smaller half and D4 already named
it: `no_teleport` and `no_recall`. Not `no_housing` — a house is not a place
another house may not go, that is what D3's yard is for, and setting both would
make the yard rule unreachable.

#### Phases within the phase

1. `no_housing` gets its reader — D9's footprint walk, a `Refusal` variant, and
   the twenty-one dungeons close.
2. `place` takes the actor and D3's exemption becomes true — D10.
3. ~~A house registers and unregisters its own region~~ — **blocked**, see the
   note under D11. It needs a decision about `Regions`' shape, not a session of
   typing.

Each stands alone. The first is worth having with neither of the others, and the
third turned out to want a design pass of its own — which is D4's lesson
arriving a second time, at one level down.

#### What a test would pin

- A house refused inside a shipped `no_housing` region, by name, so the data and
  the reader are tested together rather than against a fixture.
- The same spot taken by staff, which is D10 and is the only proof the exemption
  exists at all.
- **A house whose origin is outside a `no_housing` region and whose footprint
  reaches in.** D9's whole reason, and the one case an origin-only check passes.
- **A spot that is both inside a dungeon and bad ground reporting the region.**
  D9b's ordering, which is invisible otherwise.
- **A banded region refusing a two-storey house placed at its floor.** D9a. No
  shipped row exercises it, so the test carries a synthetic region — the one
  place in this phase where a fixture beats the data.
- **Staff still refused a multi that draws nothing.** D10's other half, and the
  one a careless "staff bypass everything" breaks silently.
- A house's region present after placement and gone after `decay::demolish`,
  because a region outliving its house is a permanent no-recall zone in an empty
  field and nothing else would notice.

#### Built: the first two, and the third is blocked

**Sub-phases 1 and 2 are in.** Twenty-one dungeons are closed to building, and
D3's staff exemption exists for the first time since it was written.

Three things came out differently.

**The region check is first among the judgements, not merely before the ground.**
D9b argued the ordering from `BadGround` — "try a tile over" is a lie inside
Deceit — and the same argument turned out to apply to `Occupied` word for word.
Every refusal below the region check means *try a tile over*; the region one is
the only one that is a statement about the place. So it went above all four
rather than between two of them.

**The origin argument that decided D9 was not either of the two written down.**
The plan gave the boundary case and the floor-versus-wall case. The one that
would have bitten first is neither: `place`'s own doc says `at` is the multi's
*origin* and "is not the corner of its box", so a multi whose components all sit
at positive offsets has an origin **outside its own drawn area** — an origin test
can test a tile no wall ever stands on, with no boundary involved at all.

**One test runs against the shipped dataset by name, and the rest are fixtures.**
That split is deliberate and it is not the usual one. Every other test in
`housing/src/tests.rs` uses a fixture because what they check is arithmetic, and
Covetous' real rectangle is not something a reader can hold in their head. But
the thing this phase is *about* is a flag that was plumbed from JSON through
codegen through the save and back for five phases while nothing read it — and a
fixture cannot say that the twenty-one real rows reach the rule. So one test
carries a facet-sized world just to name Covetous and be refused by it.

**The exemption is a row of a table, not an early return.** The reference's is
`if (from.AccessLevel >= GameMaster) return Valid;`, and copying that shape would
have reopened `NeedsCustomisation` — a game master placing a foundation with no
stairs is the exact failure that refusal was written to prevent. Three tests pin
the second row: a marker that draws nothing, an id no client knows, and a
foundation. `OffTheMap` needed no guard at all, because it comes out of
`footprint_of` above where the exemption starts.

#### Stale text this phase corrects

H6 is a correction pass, so the rot found while planning it rides along rather
than waiting for a phase of its own:

- `RegionFlags::no_recall` — "Nothing reads this yet: travel is not built."
  Travel is built and reads it (`magic/src/travel.rs:88`).
- `EncodedSubcommand::GuildGumpRequest` — "Not acted on: `guilds` is a stub."
  Guilds is complete, with five ranks, wars and alliances.
- This document's own D1 — "the `0x1A`/`0xF3` that draws it". This engine draws
  an item with `0x1A` and has no `0xF3`; the byte appears in the tree once, as a
  deliberately-unknown packet id in a decoder test
  (`client_packet.rs:283`).

---

## What this plan does not cover

- **Customisation** — D7, by name. **Reverted**: see
  [`customisation.md`](customisation.md), which covers the whole `0xD7` design
  system rather than a first cut.
- **Boats.** They are multis too and this reader already reads them, and a boat
  is a house that *moves*, which is a different problem: every component's
  position changes together and the obstruction index has to follow. **Planned
  separately**: see [`boats.md`](boats.md).
- ~~**Ilshenar, T2A and Eodon's no-housing rules.**~~ Folded in as **H6** above.
  The reason given here — "they are region flags (D4) and this engine has one
  facet loaded" — was half right and the wrong half was load-bearing. They *are*
  region flags. But the flag had no reader on the one facet that *is* loaded, so
  "one facet" was never what stood in the way, and deferring on it left
  twenty-one shipped dungeons open to building for five phases.

## Backlog, found while planning this

- **`Obstructions` has never had a hundred entries added at once.** It is filled
  from the map at boot and poked at by doors since. Placing a house is the first
  bulk write, and whether it wants one it can undo cheaply — a demolition is the
  same hundred entries coming back out — has not been asked.
- ~~🚩 **A house has no floors, for movement — the step check has nothing to pick
  between.**~~ **Repaired 2026-08-23**, by
  [`realtime_map.md`](map/realtime_map.md#r3--a-house-has-floors)'s R3.
  `Cover::of_static` reads a platform now and lays two covers for one — the
  surface a body on top stands on and the body a mobile beside it walks into —
  and `walk::climbed` takes the highest of those *in reach and above what the
  map answered*, which is how a body gets upstairs. The in-game confirmation
  this entry asked for has an automated stand-in:
  `a_villa_stair_carries_a_body_to_its_first_floor`, over multi `0x0064`'s real
  geometry. **What stays housing's** is the content half named at the end of
  this entry: which components a shipped house calls floors, and what a
  demolition takes back out. The record of the defect follows.
  This entry used to read *"a two-storey house has two floors over one
  tile and the step check has to pick the one the walker is on"*, which assumed
  the floors were in the step check at all. They are not.
  `grep -rn "CoverKind::Stands" crates` has exactly one producer in the whole
  workspace — a ship's plank — so the only live thing anything can stand on is a
  deck. A placed multi contributes `Blocks` covers for its walls and nothing
  else; `block_footprint` folds in the components whose tiledata says they block,
  and the note above about that *"keeping a floor and a roof walkable"* is true
  only in the sense of **un-blocked**, which is not the same as standable. The
  ground floor works because the map's own ground is under it. An upper storey
  has no surface, and neither end of the wire disagrees — the client cannot
  stand on one either.
  `CoverKind::Stands` is already the right type for the repair: a house floor is
  the general case of what `aboard` does for one ship. Found while writing
  [`docs/map/navigation_spans.md`](map/navigation_spans.md), which names it
  because a span grid baked from client files will contain no player house
  either, and the gap would otherwise read as a pathfinding regression the day
  that lands. **Wants an in-game confirmation before the repair is scoped** —
  walk a character upstairs in a placed villa and see.
  **The repair is owned**, since 2026-08-23, by
  [`map_rebuild.md`](map/map_rebuild.md#r3--a-house-is-a-layer-and-it-has-floors)'s
  R3: a house is the live layer over the map rather than a patch to it — which is
  what closed `mechanics.md`'s open row — and `Cover::of_static` grows the arm
  that makes a platform component a surface. This entry stays here because the
  *content* is housing's: which components a shipped house calls floors, and what
  a demolition takes back out.
- **The five multis that draw nothing** (`findings.md`) are treasure-site markers,
  and placement must refuse an id with no drawn components rather than spawn an
  invisible house.
- ~~**Our own client would draw a house as one unrelated sprite.**~~ Fixed, and
  it was as bad as it looked: `render::items::collect` had no notion of a multi,
  and a static id space running to `0x10000` means `0x4064` is a *valid* art id,
  so a villa drew as whatever static happened to sit there — silently, with no
  error anywhere.

  `net_command::multi_pieces` is the expansion, at the seam where the view
  becomes a draw list, so the renderer never learns what a multi is: it is handed
  more items and nothing else changes. Every piece takes the *house's* serial,
  which is what makes clicking any wall pick the house.

  The load-bearing detail is that it answers `None` and not an empty list when
  the client has no multi table. Falling through to the ordinary item path is
  precisely the old bug.

  **`parity.md`'s question was asked and the answer is no divergence.** Changing
  what a shard view becomes is exactly the class of change that leaves one of the
  seven frame assemblies behind, so every other `GroundItem` producer was
  checked: `render/tests/parity.rs` builds its list from the *map's own statics*
  and `render/src/scene.rs` from a synthetic fixture. Neither sees a shard item,
  and a placed house is not in the map file — so `net_command` is the only place
  a multi can arrive, and the only place that has to expand one. Recorded because
  it is cheaper to read than to re-derive.
