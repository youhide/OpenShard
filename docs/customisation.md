# A house whose shape nobody shipped

`docs/housing.md`'s **D7** put this out of scope by name, and called it "a second
system the size of this one: a design buffer, a preview state, a commit, and a
whole editor on the client." That estimate was right. **D7 is reverted** — this
document covers the whole of it.

The load-bearing decision is not the packet set and it is not the editor. It is
**where a per-house component list lives**, because the seam a house's shape
comes through today cannot hold one. Everything else in this plan follows from
the answer to that.

> Read [`housing.md`](housing.md) first — this is its D7 opened up, and it
> assumes H1–H5's decisions rather than restating them. `architecture.md` for
> where a system crate sits, `style.md` before writing any of it.

## What a design is, and why the picture is not free this time

Housing's whole tractability came from one sentence: *the picture is free,
because every client already owns every house.* The wire carries `0x4000 | id`
and the client draws the hundred and forty-eight statics a villa is made of out
of its own `multi.mul`.

A **designed** house has no id in that file. Its shape was made on this shard, by
a player, five minutes ago. So for exactly one kind of house the bargain inverts:
the shard owes the picture as well as the walls, and it owes it as a packet.

That is why the real protocol sends a design as `0xD8` rather than as an id, and
it is why "mint a synthetic multi id" is not an escape hatch — see D1's fifth
reason.

## What is missing, in one table

| piece | server | client (ours) | classic client |
|---|---|---|---|
| `0xD7` header decode | **built** — `encoded.rs`, total `Other(u16)` fallthrough | sends two subcommands | speaks it |
| `0xBF 0x1E` the client's ask | **answered** | **sent** | speaks it |
| the `0xD7` design subcommands | — | — | speaks them |
| `0xD8` the design itself | **sent**, on request | **drawn** | speaks it |
| `0xBF 0x1D` the design revision | **sent**, with the draw and on commit | **cached, and asked on a miss** | speaks it |
| a per-house component list | **built** — `HouseDesign` on the entity, D1 | — | n/a |
| a foundation on the ground | **placed, with a derived design** | **drawn** | draws multis already |
| the design saved | **built** — the `house_designs` table, schema v31 | n/a | n/a |
| the editor | — | — | **has one** |

The first two rows of this table were still saying "nowhere it can live" and "—"
after C1 and C2 had built both, which is what a table nobody re-reads does. The
phase sections below are the ones kept in step, and they are where the state is.

`0xD8` is this plan's `0x99`: the one packet that has to be written from nothing
on both ends. Zlib-compressed, and `Feature::CompressedGumps`
(`protocol/src/feature.rs:78`, gated to 5.0.0.0) is this codebase's precedent for
a compressed payload behind a version gate.

**The gate itself already exists and is read by nothing**:
`Feature::CustomMulti` — *"Custom (player-designed) house packets. Since
4.0.0a."* — at `protocol/src/feature.rs:74`. That is the second time this plan
has found its own version boundary already named and unwired, after `0x99`'s and
`SmoothShip`'s; whoever wrote the feature table wrote the whole table.

**And the compression has a home**: `miniz_oxide` is already a workspace
dependency (root `Cargo.toml:150`), pulled in directly rather than through
`flate2` because that crate's C backend is ruled out by `unsafe_code = "deny"`.
`uofiles` uses it for gumpart's zlib and — fittingly — for the UOP multi reader.
So `0xD8` costs one manifest line in `openshard-protocol`, not a dependency
decision.

## Why `Terrain::multi_components` cannot be the answer

`Terrain::multi_components(&self, id: u16) -> &[Component]`
(`movement/src/walk.rs:148`) is where a house's shape reaches gameplay today,
and it is the natural first guess. It cannot work, for five reasons. The first
four are structural; the fifth is the one that decides it.

1. **Its only key is `id: u16`.** A design is per *house*. Two houses on
   foundation `0x13EC` have two designs and one id, and the seam has nowhere to
   put the difference.
2. **It returns a borrow out of `&self`.** Whatever holds a design would have to
   be owned by the terrain and outlive the call — it cannot be computed and
   returned.
3. **The one production store is `Option<Arc<Multis>>`, installed once at boot**
   (`server/src/boot.rs:637`), and `Multis` has no insert or mutate — only
   `Multis::of(iter)`, which builds a whole table at once.
4. **The trait is deliberately not world state.** Its own doc says why: putting a
   reader for a copyrighted file into the crate every gameplay system builds on.
   A design *is* world state, which is the opposite direction. D2a is the same
   rule stated for `openshard-state`.
5. **A synthetic multi id has no picture on any client.** `Multi::new` is public
   and `Component`'s fields are public, so a synthetic multi is constructible —
   and useless, because the client resolves `0x4000 | id` against its own
   `multi.mul` and has never heard of it.

**A live trap found while writing this — ~~fixed by boats B1~~.** `CachedTerrain`
(`movement/src/cache.rs`) and `LiveTerrain` (`state/src/obstruct.rs`) both wrap a
`Terrain` and forward its methods, and **neither forwarded `multi_components`**,
so both silently answered `&[]`. Housing got away with it because its three
readers reach the facet's own ground directly — `state.facet_state(facet).terrain`
then, `WorldState::map_terrain` since
[`map/terrain_seam.md`](world/research/terrain_seam.md)'s D; the first caller to
ask a *wrapped* terrain about a house's shape would have got an empty list and no
error.

Boats B1 was that caller, and it forwards both (`cache.rs:160`,
`obstruct.rs:315`). The trap is closed — kept here because the shape of it is the
lesson: a forwarding wrapper that drops one method fails silently, and this
codebase has two of them.

## Decisions, taken here

**C1 — a design is a component on the house entity.**
`HouseDesign { components: Vec<Component>, revision: u32 }` in `openshard-state`,
beside `House`.

`Component`'s fields are all public and the type is reachable —
`openshard-state` depends on `openshard-movement`, which depends on
`openshard-uofiles`. Had it not been reachable, D2a's rule would have forced a
different design entirely, which is why it was checked before deciding.

**But the manifest does not have that edge yet**, and this document's first draft
said "nothing new enters the dependency graph" without distinguishing the two.
`crates/server/state/Cargo.toml` names six `openshard-*` crates and `uofiles` is
not among them; `openshard-movement` re-exports only `LandTile` from it, not
`Component`. So C1 adds `openshard-uofiles.workspace = true` to that manifest —
one line, and a real new edge in the graph a reader checks, which this repo
comments rather than leaves bare. `crates/server/housing/Cargo.toml` has the same
edge with a three-line justification, and that comment is the model.

**C2 — one chooser, not three.** `sign_spot` (`housing/src/lib.rs:363`),
`tiles_of` (`:518`) and `footprint_of` (`:592`) each call
`terrain.multi_components(multi)` directly. That is three copies of a choice
about to become two-way, and three places for one to be fixed and the others not.

The three take a `design: Option<&[Component]>`:

- `None` — the terrain's fixed multi. Every classic house, still a borrow, so the
  common path allocates nothing and the restore sweep costs what it costs today.
- `Some(&[…])` — this house's own design.

`Option` rather than a modelled state, and the reason matters given `style.md`'s
rule that `Option` means absent and not unknown: a classic house genuinely **has
no design**. That is absence in the domain. A foundation with no design is a
different thing, and C3 makes it unrepresentable.

The design is a *parameter* rather than resolved inside, so the choice stays
visible at every call site — `place` passes what it is placing, `restore_houses`
passes what it loaded, the sign path reads the component and passes it. Same
argument `style.md` makes for `.0` over a `Deref`.

**C3 — a foundation is never undesigned.** `FOUNDATION_IDS`
(`housing/src/lib.rs:59`) is refused at `place`'s first check today, because a
foundation's component list has no stairs and a house nobody can enter is worse
than no house. That reasoning is correct and survives: the refusal is not
deleted, it is **replaced** by placing the foundation *with* its initial design.
ServUO's `HouseFoundation` constructor lays a floor and a stair set for exactly
this reason.

So the invariant is statable: a house entity either carries a `HouseDesign` or is
a classic multi, and a foundation-id house with no design is a bug rather than a
state. That is the difference between C2's `Option` — a reader handling two kinds
of house — and a half-built object.

**C4 — the persistence rule survives, restated precisely.** Housing's rule is
that components are *never* saved, because a multi's shape is a pure function of
its id and a copy goes stale the day the operator updates their install
(`persistence/src/record.rs:349-361`, and `tick/houses.rs`'s module header). A
designed house's components have no file behind them, so the rule **as written**
cannot cover them.

It does not need abandoning; it needs saying accurately. **What is never saved is
a copy of something the client's files already state.** A design says nothing the
client's files say — the design *is* the original, and there is nothing for it to
go stale against. Both halves then hold at once, and the boundary between them is
exactly `HouseDesign`'s presence. Same shape as H5's amendment to D6: the
decision was right and its statement was one case too narrow.

**Shape: a table keyed by the house's serial, not a blob column on
`HouseRecord`.** A `HouseRecord` is small and swept for every house on every
save; a design is a few hundred rows. A classic house writes **no design rows at
all**, so the overwhelmingly common case pays nothing. The cost, named: a second
query on restore, joined by serial.

**Schema v31, and for once it is the *reader's* case.** The last four bumps were
about the writer. This one is not. A v30 build opens the database, does not know
the design table so does not drop it, reads a house, sees a foundation multi id,
and computes the footprint from `multi_components` — which for a foundation is a
bare platform. The shard comes up with a customised house wearing the
foundation's walls, and nothing says so. That is worse than a house with no walls
at all, which is at least visible.

**One thing gets better, and it is worth writing down so nobody "fixes" it:** a
designed house restores with real walls on a shard with **no client files**,
because its components never came from client files. It is the one place H1's
stated bargain improves.

**C5 — `0xBF 0x1D` is load-bearing, not an optimisation.** It does not exist
anywhere in this repo. The custom-house revision is what lets a client cache a
design by `(serial, revision)` and ask for the full `0xD8` only when what it
holds is stale. Without it every client walking into an area re-fetches every
design in it, on every approach.

So the revision is a `u32` on `HouseDesign`, saved, and bumped on commit — and it
lands in the **first** phase rather than a later one, because retrofitting a
cache key after clients have cached under no key is a migration rather than a
feature.

**C6 — the `0xD7` subcommand set, named by role.** `EncodedCommand`
(`protocol/src/encoded.rs`) decodes a header only and leaves the payload unread,
with `EncodedSubcommand::Other(u16)` as a total fallthrough — so adding
subcommands is purely additive and nothing already routed changes. That is a
better extension point than this plan deserves and it is worth noticing.

| role | what it changes |
|---|---|
| begin / end customisation | the session's brackets |
| build / erase at a tile | the working design |
| select floor | which storey the editor edits |
| roof place / delete | the working design, on the roof plane |
| commit / revert | the committed design, or nothing |
| backup / restore | a second working copy |
| synch | the client asking for the authoritative design back |
| clear | empties the working design |

**The hex values are read out of the reference at implementation time and cited
at the constant**, per `style.md`'s "ports name their source". A plan that
guessed them would be shipping magic constants with an extra step.

The dispatch path is four files deep and no more, and `QuestGumpRequest` is the
worked example end to end: `encoded.rs` names the subcommand → `dispatch.rs:47`
maps it to a `Command` → `tick/command.rs` declares the variant → `tick.rs`
routes it to `openshard-housing`.

**C7 — the session is a state, and the working design touches nothing.**
`DesignSession { editor, working: Vec<Component>, floor: u8 }` as its own
component on the house entity — a separate component rather than a field on
`House`, because absence-as-no-component is what a sparse set is for and it keeps
`House` from growing a field most houses never carry.

The rule that makes the whole thing tractable: **while a session is open, the
world still shows and blocks the committed design.** ServUO puts the editor onto
the foundation and freezes them; nobody walks around inside a half-finished edit.
So there is no incremental obstruction churn, no partial design on the wire, and
no question about what a stranger standing outside sees. One commit, one swap.

Entry is the owner's, asked through `standing_of` — reused rather than rewritten,
which is the third time that has been the right answer after `Standing` itself
and the door.

**Commit is six steps and the fifth is the one that gets forgotten:** validate
the working design; replace `HouseDesign` and bump `revision`; `unblock` the old
footprint and `block` the new; re-run `adopt_doors`, because a design can cut a
doorway where there was none; **re-hang the sign**, because `sign_spot` is
derived from the multi's *box* and a design that grew the box moved the sign; and
send the new revision.

**The lockdown allowance is recomputed on commit, and it is the one place H4's
argument does not apply.** H4 stores the allowance rather than recomputing it at
boot, so that an operator who lowered `LOCKDOWNS_PER_TILE` does not find half the
shard over the new ceiling with nothing to say which lockdowns to drop. That
failure is about the *constant* changing. Here the house's own area changed,
which is a fact about this house, and the operator-constant failure is untouched.

**A session outlives nothing.** Logout, death and `collapse_houses` all have to
end one. Named because a dangling `DesignSession` on a despawned house surfaces
as a panic rather than as a missing feature.

**C8 — our own client draws a designed house as nothing, and it is the old bug in
a new colour.** `net_command::multi_pieces` expands `0x4000 | id` against the
client's own table. A designed house's foundation id is almost never in it —
`FOUNDATION_IDS` runs `0x13EC..0x1D00` and a shipped `multi.mul` holds 326
entries — so it falls through to the ordinary item path, which is *precisely* the
"a villa drew as whatever static happened to sit there" failure `housing.md`'s
backlog records as fixed.

> **Built, and it was live already.** The fix is not the one this paragraph
> describes, because the diagnosis was half wrong in a way worth recording.
> `multi_pieces` *did* answer `None` for an unknown id — and `None` meant **three
> different things**: not a multi, no table, and a multi the table does not hold.
> The caller could only act on one, so it fell through on all three and drew the
> static. **The bug the comment claimed to prevent was live the whole time**, for
> every multi any client's files lack — an install older than the shard's as much
> as a foundation.
>
> Its own test asserted `is_none()` and passed, because it tested the return
> value rather than the behaviour its name claims. That is the failure mode to
> take away: a test can pin a function's answer and say nothing about what the
> caller does with it.
>
> So the return type is an enum with the three answers named — `NotAMulti`,
> `Pieces`, `Unknown` — and `Unknown` draws nothing *and picks nothing*, since
> `items` and `item_serials` run parallel and pushing to neither is what keeps
> them so. A house this client has no shape for is one it cannot show, and one
> unrelated static in its place is worse than an empty tile, because an empty
> tile is visibly empty.

## The phases

Four. A full editor is several sessions, and **the first phase that is genuinely
useful builds the seam and no editor at all** — because everything hard here is
the seam.

### C1 — designs exist, and staff make them

1. `HouseDesign` in `openshard-state`; C2's chooser through the three readers.
2. The design table, the restore join, schema v31.
3. `0xBF 0x1D` and `0xD8`, both ends.
4. `net_command::multi_pieces` refuses to draw a house it has no shape for — C8.
5. `.hdesign <multi id>` — a staff verb that copies an existing multi's
   components onto a house as its design.

> **Steps 3 and 4 are built, and both of C1's open questions answered yes.**
> `openshard-protocol`'s `design` module encodes and decodes both packets, with
> the layout read out of `HouseFoundation.cs` rather than guessed. `miniz_oxide`
> cost the one manifest line this document predicted, and `Feature::CustomMulti`
> was already there waiting.
>
> Two things the plan did not know, and they are the half worth reading:
>
> - **`0xD8` cannot be encoded by this crate alone.** Which plane a tile goes in
>   turns on whether its graphic is a *floor*, and that is `tiledata`'s height —
>   a client file `openshard-protocol` has never read and must not start. So
>   `encode` takes the predicate and the caller, which holds a `Terrain`,
>   supplies it. It is the same seam `Terrain::multi_components` is, one crate
>   lower down.
> - **Decoding needs the house's width and height, and no field carries them.**
>   The grid stride *is* the height. A real client reads it off the foundation's
>   own multi; ours is handed a `DesignBounds`. That is a property of the packet
>   rather than a shortcut — the two ends have to agree on the box before a byte
>   of it means anything.
>
> Why the layout is shaped that way is not obvious and is worth one line: it is a
> **sparse encoding by elevation**. A house's tiles cluster at five `dz` values,
> so each becomes a fixed-stride grid of `u16` graphics with zero meaning
> "nothing here" — and deflate erases the zeroes. Everything that breaks the
> assumption, a tile at an odd height or outside its plane's grid, falls into a
> *stair buffer* written longhand. No tile is ever dropped for being unusual; it
> is only ever more expensive.
>
> One departure from the reference, and it changes no byte on the wire: ServUO
> writes the plane count and the buffer length as placeholders and seeks back to
> offsets 15 and 17 to patch them. Here the planes are built first, so both are
> known before anything goes out.

**No `0xD7` at all, and that is the point.** It proves `0xD8` against a real
client with components that came out of a file, so a bug in the packet is a bug
in the packet rather than a bug in an editor nobody has written yet.

**Done when** `.house 0x64` then `.hdesign 0x65` makes a small house draw and
block as a villa on a real client, and it is still that after a restart.

**Useful on its own**, which is the test of a phase boundary: it is what lets a
pack ship its own architecture without a client-file edit and without an editor.

#### Built

All five steps. What came out differently:

**The chooser was the easy half; the *commit tail* was the phase.** C2 named
three readers of `multi_components` and threading a `design` parameter through
them took an afternoon. What it did not name is that a house's shape is also
read by four things that hold a **house entity** rather than a multi id — the
sign's tile, the door adoption, the lockdown area, and the walls the fall-down
path removes — and every one of them was passing `None`. So the chooser has a
second face, `design::shape_of_house`, which asks the entity rather than taking
a parameter, and those four ask it.

Two of them were already wrong for a designed house before `.hdesign` existed:
`decay::demolish` unblocked the *foundation's* footprint, and `storage`'s
allowance counted the foundation's area. Neither could be reached yet, which is
exactly why they were worth finding now rather than as a bug report.

**"Nothing comes down until the new shape is legal" is a rule, not an
optimisation.** The refusal for a design that draws nothing has to happen before
the old walls are unblocked, or a mistyped command leaves a house standing that
you can walk straight through. `redesign` computes the new footprint first and
refuses on it.

**And the old walls come out as the old shape.** Unblocking with the *new*
design leaves every tile the two do not share blocked forever, by an entity that
no longer stands there — a leak nothing reports and a player finds by walking
into thin air. It is the test the phase is worth having.

**Redesigning is the owner's, not a co-owner's.** A co-owner may lock things
down and let people in; neither changes what the building *is*. Not a decision
this document had taken, and it follows from the same reasoning D6 uses.

**The revision bumps on every commit, including one that produces identical
walls.** It is a cache key, not a change detector — C5's argument taken to its
conclusion.

### C2 — a foundation is placeable

`Refusal::NeedsCustomisation` goes away, replaced by C3's initial design at
placement. The deed sells a foundation. A player can own one; it is a bare shell
with a floor and stairs, and it draws.

#### Built

**The refusal did not go away, and that is the correction.** It stands wherever
the design cannot be built — which is a shard with no client files, or an id
inside the range whose platform this install does not hold. The design is built
*out of* the foundation's own platform, so a foundation nobody can read is still
a house nobody can get into, and it is still refused, for staff too.

**The stair block is a derivation, which is what this phase went to find out.**
`GetEmptyFoundation` copies the platform, grows the box one row south, lays four
floor graphics around the perimeter and a stair along the new row. Every position
falls out of the box. There is no per-house-type table to port.

**There is one table, and it is a material rather than a house type.**
`GetFoundationGraphics` is eight rows keyed by what the owner chose the floor to
look like. That is the other side of the line that kept the door positions and
the sign offsets out of this engine, so it is here — but only the reference's own
default arm, because *which* material is the editor's question. A foundation
placed today is dark wood and the constant says so, rather than seven rows
nothing can reach.

**The initial design is revision 1, not 0.** Zero is what `design::revision`
answers for a house that has never been designed, so a foundation sitting at zero
would be indistinguishable from a classic house and no client would ever be told
its picture had arrived.

**And the deed was no lines at all.** The plan called it "a line"; it turned out
the deed hands its multi id straight to `housing::place`, which is where the
refusal lived, so it started working the moment `place` did. That is worth a test
rather than a claim — "it should already work" is how a path goes untested — and
`a_deed_for_a_foundation_builds_a_house_with_a_design` crosses the deed, the
cursor and the placement in one.

**Still not reshapeable by a player.** `.hdesign` is staff-only and the editor is
C3. A foundation stands, draws, has stairs and can be locked down in; that is the
whole of what C2 promises.

### C3 — the session

Enter and leave, build and erase, floor selection, commit and revert. The editor,
and a session's work on its own.

### C4 — roofs, backup and restore, and the validation

ServUO's `HouseFoundation.Check*`: every tile supported, stairs reachable, the
piece count under a ceiling. **C3 enforces only the cheap half** — inside the
foundation's box, under a component ceiling, storeys within the limit — and
defers the support-and-reachability half **by name**, because "is this design
structurally coherent" is a graph problem and a floating tower is a cosmetic bug
rather than a hole in the shard.

## What this plan does not cover

- **House resizing and foundation upgrade.** ServUO's foundation can be enlarged
  for gold. It is a *placement* question wearing a design costume — it re-asks
  D3's five rules on a bigger footprint — and it belongs with placement.
- **A design catalogue** — saving a design and applying it to another house. It
  is C1's `.hdesign` generalised, and it is content plumbing rather than a system.
- **Stairs as generated content.** C3 lays the reference's initial stair set; a
  system that *reasons* about where stairs must go is C4's validation problem.
- **An editor in our own client.** C8 makes a designed house draw; a client that
  can edit one is the client-side half of C3 and is its own plan.
- **Minting synthetic multi ids.** Refused in D1's fifth reason, and recorded
  here so the next reader who notices `Multi::new` is public knows it was
  considered rather than missed.

## Backlog, found while planning this

- ~~**`CachedTerrain` and `LiveTerrain` drop `multi_components`.**~~ **Fixed by
  boats B1** (`cache.rs:160`, `obstruct.rs:315`). They forwarded thirteen and
  seven `Terrain` methods and not this one, so both answered `&[]` with no error
  — latent only because housing's three readers reach the terrain directly, and
  B1 was the first caller that did not. It is the shape of defect a default
  method on a trait invites: an override that is missing looks exactly like an
  override that was not needed.
- **`housing::place` re-reads the multi table three times** — `footprint_of`,
  `tiles_of` for the allowance, and `sign_spot`. Cheap at a click, and it becomes
  three reads of a `Vec<Component>` on the entity once designs exist, which is a
  different cost with the same shape.
- **The design is the first thing a house owns that is large.** Every other
  per-house fact is a serial, a set of serials, or a `u32`. Whatever the save
  cadence does with a few hundred rows per house has not been asked, and H4's
  lockdowns were the last time a question of that shape came up.
