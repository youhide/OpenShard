# A house whose shape nobody shipped

[`design_house.md`](design_house.md)'s **D7** put this out of scope by name, and
called it "a second system the size of this one: a design buffer, a preview
state, a commit, and a whole editor on the client." That estimate was right, and
this document is that system.

The load-bearing decision is not the packet set and it is not the editor. It is
**where a per-house component list lives**, because the seam a house's shape
comes through today cannot hold one. Everything else here follows from the answer
to that.

**What is built and what is open is [`README.md`](README.md)**; how the seam and
the foundation were built is
[`evidence/2026-08-24-the-design-phases.md`](evidence/2026-08-24-the-design-phases.md),
and the editor that is still missing is
[`plans/housing/customisation/PLAN.md`](../../plans/housing/customisation/PLAN.md).

> Read [`design_house.md`](design_house.md) first — this is its D7 opened up, and
> it assumes the classic house's decisions rather than restating them.
> `architecture.md` for where a system crate sits, `style.md` before writing any
> of it.

## What a design is, and why the picture is not free this time

Housing's whole tractability came from one sentence: *the picture is free,
because every client already owns every house.* The wire carries `0x4000 | id`
and the client draws the hundred and forty-eight statics a villa is made of out
of its own `multi.mul`.

A **designed** house has no id in that file. Its shape was made on this shard, by
a player, five minutes ago. So for exactly one kind of house the bargain inverts:
the shard owes the picture as well as the walls, and it owes it as a packet.

That is why the real protocol sends a design as `0xD8` rather than as an id, and
it is why "mint a synthetic multi id" is not an escape hatch — see the fifth
reason below.

`0xD8` is this area's `0x99`: the one packet that had to be written from nothing
on both ends. Zlib-compressed, and `Feature::CompressedGumps`
(`protocol/src/feature.rs:78`, gated to 5.0.0.0) is this codebase's precedent for
a compressed payload behind a version gate. The gate for the design packets
themselves already existed and was read by nothing: `Feature::CustomMulti` —
*"Custom (player-designed) house packets. Since 4.0.0a."* — which is the second
time this area found its own version boundary already named and unwired, after
`0x99`'s and `SmoothShip`'s.

The compression has a home too: `miniz_oxide` is already a workspace dependency,
pulled in directly rather than through `flate2` because that crate's C backend is
ruled out by `unsafe_code = "deny"`. `uofiles` uses it for gumpart's zlib and —
fittingly — for the UOP multi reader.

## Why `Terrain::multi_components` cannot be the answer

`Terrain::multi_components(&self, id: u16) -> &[Component]`
(`movement/src/walk.rs:148`) is where a house's shape reaches gameplay, and it is
the natural first guess. It cannot work, for five reasons. The first four are
structural; the fifth is the one that decides it.

1. **Its only key is `id: u16`.** A design is per *house*. Two houses on
   foundation `0x13EC` have two designs and one id, and the seam has nowhere to
   put the difference.
2. **It returns a borrow out of `&self`.** Whatever holds a design would have to
   be owned by the terrain and outlive the call — it cannot be computed and
   returned.
3. **The one production store is `Option<Arc<Multis>>`, installed once at boot**,
   and `Multis` has no insert or mutate — only `Multis::of(iter)`, which builds a
   whole table at once.
4. **The trait is deliberately not world state.** Its own doc says why: putting a
   reader for a copyrighted file into the crate every gameplay system builds on.
   A design *is* world state, which is the opposite direction. D2a is the same
   rule stated for `openshard-state`.
5. **A synthetic multi id has no picture on any client.** `Multi::new` is public
   and `Component`'s fields are public, so a synthetic multi is constructible —
   and useless, because the client resolves `0x4000 | id` against its own
   `multi.mul` and has never heard of it.

**A live trap found while writing this, and since closed by the boats work.**
`CachedTerrain` (`movement/src/cache.rs`) and `LiveTerrain`
(`state/src/obstruct.rs`) both wrap a `Terrain` and forward its methods, and
**neither forwarded `multi_components`**, so both silently answered `&[]`.
Housing got away with it because its three readers reach the facet's own ground
directly — `WorldState::map_terrain` since
[`terrain_seam.md`](../world/research/terrain_seam.md)'s D; the first caller to
ask a *wrapped* terrain about a house's shape would have got an empty list and no
error. The shape of it is the lesson: a forwarding wrapper that drops one method
fails silently, and this codebase has two of them.

## Decisions, taken here

**C1 — a design is a component on the house entity.**
`HouseDesign { components: Vec<Component>, revision: u32 }` in `openshard-state`,
beside `House`.

`Component`'s fields are all public and the type is reachable —
`openshard-state` depends on `openshard-movement`, which depends on
`openshard-uofiles`. Had it not been reachable, D2a's rule would have forced a
different design entirely, which is why it was checked before deciding.

**But the manifest did not have that edge**, and this document's first draft said
"nothing new enters the dependency graph" without distinguishing the two.
`openshard-movement` re-exports only `LandTile` from `uofiles`, not `Component`.
So C1 adds `openshard-uofiles.workspace = true` to `server/state`'s manifest —
one line, and a real new edge in the graph a reader checks, which this repo
comments rather than leaves bare. `crates/server/housing/Cargo.toml` has the same
edge with a three-line justification, and that comment is the model.

**C2 — one chooser, not three.** `sign_spot`, `tiles_of` and `footprint_of` each
called `terrain.multi_components(multi)` directly. That is three copies of a
choice about to become two-way, and three places for one to be fixed and the
others not.

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

**The chooser has a second face, and it is the one most callers want.** Four
readers hold a *house entity* rather than a multi id — the sign's tile, the door
adoption, the lockdown area, and the walls the fall-down path removes — and a
parameter is no use to them. `design::shape_of_house` asks the entity instead,
and those four ask it.

**C3 — a foundation is never undesigned.** `FOUNDATION_IDS` is refused at
`place`'s first check, because a foundation's component list has no stairs and a
house nobody can enter is worse than no house. That reasoning is correct and
survives: the refusal is not deleted, it is **replaced** by placing the foundation
*with* its initial design. ServUO's `HouseFoundation` constructor lays a floor and
a stair set for exactly this reason.

So the invariant is statable: a house entity either carries a `HouseDesign` or is
a classic multi, and a foundation-id house with no design is a bug rather than a
state. That is the difference between C2's `Option` — a reader handling two kinds
of house — and a half-built object.

**The refusal still stands where the design cannot be built**, which is a shard
with no client files, or an id inside the range whose platform this install does
not hold: the design is built *out of* the foundation's own platform, so a
foundation nobody can read is still a house nobody can get into, and it is refused
for staff too.

**C4 — the persistence rule survives, restated precisely.** Housing's rule is
that components are *never* saved, because a multi's shape is a pure function of
its id and a copy goes stale the day the operator updates their install. A
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

**C5 — `0xBF 0x1D` is load-bearing, not an optimisation.** The custom-house
revision is what lets a client cache a design by `(serial, revision)` and ask for
the full `0xD8` only when what it holds is stale. Without it every client walking
into an area re-fetches every design in it, on every approach.

So the revision is a `u32` on `HouseDesign`, saved, and bumped on commit — and it
landed in the **first** phase rather than a later one, because retrofitting a
cache key after clients have cached under no key is a migration rather than a
feature. It bumps on every commit, **including one that produces identical
walls**: it is a cache key, not a change detector.

**C6 — the `0xD7` subcommand set, named by role.** `EncodedCommand`
(`protocol/src/encoded.rs`) decodes a header only and leaves the payload unread,
with `EncodedSubcommand::Other(u16)` as a total fallthrough — so adding
subcommands is purely additive and nothing already routed changes. That is a
better extension point than this system deserves and it is worth noticing.

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
worked example end to end: `encoded.rs` names the subcommand → `dispatch.rs`
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
and the door. **Redesigning is the owner's and not a co-owner's**: a co-owner may
lock things down and let people in; neither changes what the building *is*.
Staff read as a co-owner there, so the door is shut on them too, and that is the
right answer rather than an oversight: `.hdesign` is the staff path to a shape.

**The way in is the house's own window and the way out is the client's.** The
asymmetry is the reference's and it is not arbitrary. A session *begins*
server-side, from a button on the sign's window — ServUO's `HouseGumpAOS` draws
"Customize this house" and there is no packet a client can send to open one —
because the authority is a fact about the house, and asking a client to assert it
would be asking the wrong end. It *ends* from the client, `0xD7 0x0C`
(`Designer_Close`), because closing the window is the thing the client is the
only witness to. What the editing client sees at either end is `0xBF 0x20` with
the type byte `0x04` or `0x05`, and it goes to that one client: a session is a
state of one screen, and everyone else is still being shown the committed design.

**Three refusals beyond the standing.** A **classic** house is refused
(`NotDesignable`) because its shape is a multi id in every client's own files and
there is nothing here to edit — inventing a design for one would give it walls no
client could draw. A house **already open** is refused, because two working
copies of one house are two commits racing to be the shape. And a client below
`Feature::CustomMulti` is refused, which is the one that is a lifetime argument
rather than a permission: such a client has no editor to open *and no way to say
it closed one*, so a session opened for it could only ever be ended by a logout.

**Commit is six steps and the fifth is the one that gets forgotten:** validate
the working design; replace `HouseDesign` and bump `revision`; `unblock` the old
footprint and `block` the new; re-run `adopt_doors`, because a design can cut a
doorway where there was none; **re-hang the sign**, because `sign_spot` is
derived from the multi's *box* and a design that grew the box moved the sign; and
send the new revision.

Two rules of that swap are worth stating separately, because each is a defect
when it is got wrong: **nothing comes down until the new shape is legal** — a
design that draws nothing is refused before the old walls are unblocked, or a
mistyped command leaves a house you can walk straight through — and **the old
walls come out as the old shape**, since unblocking with the *new* design leaves
every tile the two do not share blocked for ever, by an entity that no longer
stands there.

**The lockdown allowance is recomputed on commit, and it is the one place H4's
argument does not apply.** H4 stores the allowance rather than recomputing it at
boot, so that an operator who lowered `LOCKDOWNS_PER_TILE` does not find half the
shard over the new ceiling with nothing to say which lockdowns to drop. That
failure is about the *constant* changing. Here the house's own area changed,
which is a fact about this house, and the operator-constant failure is untouched.

**A session outlives nothing.** Logout, death and `collapse_houses` all have to
end one. Named because a dangling `DesignSession` on a despawned house surfaces
as a panic rather than as a missing feature.

Built as three calls, and the third covers two events: `session::end_for` from
the disconnect and from `become_ghost`, and `session::end_over` from
`decay::demolish` — which is the one call that destroys a house, so the clock's
collapse and the owner's own Demolish button need no hook of their own. The
logout one is an **ordering** as much as a rule: the session names its editor by
serial, and the disconnect releases that serial a few lines further down, so the
ender runs before the despawn or it is naming nobody. Death ends one for the
entry rule's own reason — a ghost is refused a session, so one left open would be
a state the entry says cannot exist.

**A session is never saved**, and it is the only thing in this document that is
not. Both ends of it are gone at the next boot: the player, and the editor window
on their client. A restored session would be a house nobody is editing and
nothing can close.

**C8 — a client that has no shape for a house draws nothing, and picks nothing.**
`net_command::multi_pieces` expands `0x4000 | id` against the client's own table.
A designed house's foundation id is almost never in it — `FOUNDATION_IDS` runs
`0x13EC..0x1D00` and a shipped `multi.mul` holds 326 entries — so it used to fall
through to the ordinary item path, which is *precisely* the "a villa drew as
whatever static happened to sit there" failure the house record names as fixed.

The diagnosis was half wrong in a way worth keeping: `multi_pieces` *did* answer
`None` for an unknown id — and `None` meant **three different things**: not a
multi, no table, and a multi the table does not hold. The caller could only act on
one, so it fell through on all three and drew the static, which means the bug the
comment claimed to prevent was live the whole time, for every multi any client's
files lack. Its own test asserted `is_none()` and passed, because it tested the
return value rather than the behaviour its name claims.

So the return type is an enum with the three answers named — `NotAMulti`,
`Pieces`, `Unknown` — and `Unknown` draws nothing *and picks nothing*, since
`items` and `item_serials` run parallel and pushing to neither is what keeps them
so. A house this client has no shape for is one it cannot show, and one unrelated
static in its place is worse than an empty tile, because an empty tile is visibly
empty.

## What this does not cover

- **House resizing and foundation upgrade.** ServUO's foundation can be enlarged
  for gold. It is a *placement* question wearing a design costume — it re-asks
  D3's five rules on a bigger footprint — and it belongs with placement.
- **A design catalogue** — saving a design and applying it to another house. It
  is `.hdesign` generalised, and it is content plumbing rather than a system.
- **Stairs as generated content.** The initial stair set is the reference's; a
  system that *reasons* about where stairs must go is the validation problem.
- **An editor in our own client.** C8 makes a designed house draw; a client that
  can edit one is the client-side half of the session and is its own plan.
- **Minting synthetic multi ids.** Refused in the fifth reason above, and recorded
  so the next reader who notices `Multi::new` is public knows it was considered
  rather than missed.
