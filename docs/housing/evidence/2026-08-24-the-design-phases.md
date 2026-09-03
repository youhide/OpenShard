# A designed house, phase by phase

The implementation record of the two phases that made a house's shape the shard's
own: the seam a per-house component list lives in, both design packets, and a
foundation a player can own and stand in. The editor is the half that is not
here.

The decisions this was built against are
[`design_customisation.md`](../design_customisation.md); what is built and what
is open today is [`README.md`](../README.md); what the editor still needs is
[`plans/housing/customisation/PLAN.md`](../../../plans/housing/customisation/PLAN.md).

## What was missing when this was written

| piece | server | client (ours) | classic client |
|---|---|---|---|
| `0xD7` header decode | **built** — `encoded.rs`, total `Other(u16)` fallthrough | sends two subcommands | speaks it |
| `0xBF 0x1E` the client's ask | **answered** | **sent** | speaks it |
| the `0xD7` design subcommands | — | — | speaks them |
| `0xD8` the design itself | **sent**, on request | **drawn** | speaks it |
| `0xBF 0x1D` the design revision | **sent**, with the draw and on commit | **cached, and asked on a miss** | speaks it |
| a per-house component list | **built** — `HouseDesign` on the entity, C1 | — | n/a |
| a foundation on the ground | **placed, with a derived design** | **drawn** | draws multis already |
| the design saved | **built** — the `house_designs` table, schema v31 | n/a | n/a |
| the editor | — | — | **has one** |

The first two rows of this table were still saying "nowhere it can live" and "—"
after C1 and C2 had built both, which is what a table nobody re-reads does. That
is the reason a domain gets one status page and a record gets a date.

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
C3.  A foundation stands, draws, has stairs and can be locked down in; that is the
whole of what C2 promises.

### C3 — the session

Enter and leave, build and erase, floor selection, commit and revert. The editor,
and a session's work on its own. **Not built** —
[`plans/housing/customisation/PLAN.md`](../../../plans/housing/customisation/PLAN.md).

### C4 — roofs, backup and restore, and the validation

ServUO's `HouseFoundation.Check*`: every tile supported, stairs reachable, the
piece count under a ceiling. **C3 enforces only the cheap half** — inside the
foundation's box, under a component ceiling, storeys within the limit — and
defers the support-and-reachability half **by name**, because "is this design
structurally coherent" is a graph problem and a floating tower is a cosmetic bug
rather than a hole in the shard. **Not built** — the same plan.

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
