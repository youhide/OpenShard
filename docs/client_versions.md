# Client versions

Which clients exist, which ones people actually play, what changes between them,
and what each one costs this server. Written after reading ClassicUO's loaders
(`ClassicUO.Assets`, `ClassicUO.IO`), `uo-rust-libs`, and what shards ship today.

The rule this document serves is the one in `CLAUDE.md`: **never branch on `Era`
for a protocol decision** — ask `version.supports(Feature::X)`. What follows is
the evidence behind the boundaries in `Feature::since`, plus the boundaries that
are not in there yet.

## Two clusters have almost all the players

**7.0.x — the big shards.** UO Outlands, the largest freeshard, has supported
ClassicUO exclusively since September 2021, ships it in its own launcher with
Razor, and refuses any build but the current one. ServUO's easy path is
7.0.15.1.

**5.0.8.3 — the T2A and Renaissance shards.** UO Second Age, running since 2007,
ships its own installer built on 5.0.8.3 — not a 1999 client. The ServUO forums
say the same: 5.0.8.3 "seems to be the one most people go with for the T2A and
Ren eras".

Everything below 5.0 is a handful of preservation shards.

The lesson worth keeping: **the era a shard emulates and the client it runs on
are different things.** A T2A shard in 2026 runs a 2007 client. It is the
client, not the era, that decides which bytes we send.

## Why the old-era shards do not simply use 7.0.x

Not the protocol. The client *is* content and UI, not just transport.

A 7.x client knows about elves and gargoyles, professions, the buff bar, the
modern paperdoll and gump set. A shard recreating 1999 has to subtract: races
appear in character creation that the shard does not have, buttons lead nowhere,
the skill window is the wrong shape. None of that is fixable server-side,
because the client draws it. And its files are the modern world — rebuilt
cities, the ML expansion out to 7168 — so an old-era shard ships its own `.mul`
set regardless.

The naive "newer is simpler" is also backwards. On a modern client a great deal
is **mandatory rather than optional**: with AoS the client shows item properties
through OPL (`0xD6`), and a server that does not send them leaves every hover
empty. On 3.0.8 the name arrives from one old packet on click and everything
works. The newer the client, the more must be implemented before the game stops
looking broken.

Plus inertia: those shards' server code (RunUO forks) was written against the
old protocol fifteen years ago.

## What changes between versions

### Files

| Boundary | What changes |
|---|---|
| 5.0.0a | `verdata.mul` is dropped; mega-cliloc arrives |
| 7.0.9.0 | tiledata flags widen from `u32` to `u64` (High Seas) |
| 7.0.24 | `.mul` → `.uop` for maps, art, gumps, sound, animations |
| 7.0.104.0 | another file format change (ClassicUO's `CV_7010400`) |

Below 5.0.0a `verdata.mul` is **not optional** — ClassicUO forces it on for
those clients regardless of the setting.

### The world itself is not identical

Three separate axes, and they are often confused for one.

**How many facets exist.** Each expansion added a file:

| # | Facet | Size | Arrived |
|---|---|---|---|
| 0 | Felucca | 7168×4096 | 1997 |
| 1 | Trammel | 7168×4096 | Renaissance, 2000 |
| 2 | Ilshenar | 2304×1600 | LBR, 2000 |
| 3 | Malas | 2560×2048 | AoS, 2003 |
| 4 | Tokuno | 1448×1448 | SE, 2004 |
| 5 | Ter Mur | 1280×4096 | SA, 2009 |

On a 3.0.8 install there are three map files, and `map3`..`map5` do not exist —
not empty, absent. ClassicUO carries the mirror-image hack for the other
direction: if `map1` is missing or zero-length it substitutes `map0`'s files
into that slot, so a shard that says "you are in Trammel" does not kill the
client (`MapLoader.cs:234-246`).

**A facet's size changed.** Felucca and Trammel were 6144×4096 and became
7168×4096 with Mondain's Legacy — 1024 columns appended to the east. The Lost
Lands (T2A, 1998) were already inside the 6144. ClassicUO clamps this **by
client version, not by file length**: `if (length/blocksize == 393216 ||
Version < CV_4011D) width = 6144` (`MapLoader.cs:228-231`), where 393216 =
768×512 blocks = exactly 6144×4096.

**A facet's contents changed continuously.** Cities were rebuilt, roads moved,
dungeons redone. `map0.mul` from 3.0.8 and from 7.0.x are the same geography and
not the same file. The delivery mechanism has three eras, and all three left
traces in the loaders:

- `verdata.mul` — early patches, up to 5.0.0a. One file overriding entries in
  all the others.
- `mapdif*` / `stadif*` — per-block diffs: "block N now lives over here".
- A whole new `map0.mul` — what has been done since bandwidth got cheap.

All three are implemented the same way, and it is a good idea: `IndexMap` holds
`MapAddress`/`StaticAddress` next to `OriginalMapAddress`/`OriginalStaticAddress`,
and a patch **repoints an entry at another file** rather than rewriting data.
The same trick runs from verdata in 1998 to UltimaLive streaming map blocks over
the network today.

### Protocol

The boundaries are in `Feature::since` (`crates/common/protocol/src/feature.rs`),
ported from Sphere's `MINCLIVER_*`. The load-bearing ones:

| Feature | Since |
|---|---|
| OPL tooltips (`0xD6`) | 4.0.0a |
| Tooltip hashes (`0xDC`) | 4.0.5a |
| Stat locks | 4.0.1a |
| Buff icons (`0xDF`) | 5.0.2b |
| Compressed gumps | 5.0.0a |
| New context menu form | 6.0.0.0 |
| Container grid indices | 6.0.1.7 |
| New mobile animation (`0xE2`) | 7.0.0.0 |
| New `0x78` mobile spawn | 7.0.33.1 |

Note that AoS features arrived in the client at **3.0.8z**, which ClassicUO
labels "Age of Shadows. Adds paladin, necromancer, custom housing, resists",
while Sphere's `MINCLIVER_AOS` — and therefore ours — is 4.0.0.0. Every client
in `[3.0.8z, 4.0.0)` is told it has no AoS support when it does.

## Why server and client must read the same files

There is no "what does your map say" packet. If the server's `statics0.mul`
disagrees with the client's, nothing errors: the server sees a wall and refuses
the step, the client draws open floor, and the player walks into nothing.

That is why shards distribute their own archive of client files instead of
pointing at the official installer, and why Outlands enforces an exact client
build. It is also why CentrED exists — shards draw their own maps. ClassicUO
anticipates this with a `MapsLayouts` setting that declares an arbitrary number
of maps at arbitrary sizes (`MapLoader.cs:106-138`).

## What our readers support

`crates/common/uofiles`, three files with different reach:

- **`map.rs` — every version.** The `.mul` block layout has not changed since
  1997. The facet size is derived from the block count against a table that
  includes 6144×4096, so a 3.0.8 map reads like a 7.0.x one. It also has a check
  ClassicUO lacks: `IndexMismatch`, "the map has N blocks but staidx has M
  entries; they are from different clients".
- **`uop.rs` — 7.0.24+**, since that is when `.uop` starts existing.
  zlib-compressed entries are rejected; map containers store theirs
  uncompressed. The name hash is computed properly — entry order in the
  container is not guaranteed, so skipping the hash silently scrambles the map.
- **`tiledata.rs` — both forms.** `Legacy` (32-bit flags) and `HighSeas`
  (64-bit), chosen by which one divides the file length exactly, not by client
  version. Short old files are padded so a lookup for a tile this client never
  heard of cannot panic.

Not covered: `verdata.mul`, `mapdif`/`stadif`, the `x` files (`map0x.mul`,
`statics0x.mul`), and everything a renderer needs but a server does not — see
[`client/design_picture.md`](client/design_picture.md) M2.

## The other Rust reader, for comparison

`uo-rust-libs` (MIT, on crates.io, actively maintained) states its own range
twice: "pre-UOP data files", "tested on Age of Shadows, should support clients
up to Mondain's Legacy". By its code the real ceiling is lower — `Flags` is a
`u32` bitflags with the tiledata offsets hardcoded (`STATIC_OFFSET = 428032`),
so a High Seas tiledata is not rejected, it is read as garbage.

It cannot be a dependency for us: no `.uop`, wrong tiledata above 7.0.9.0. Two
things in it are worth reading anyway:

- **`src/map/diff.rs`** is a working `mapdif`/`stadif` reader, which we do not
  have. The `*difl` format does not announce itself: it is a flat `u32` array
  where the *value* is the block index in the map and the *position* is the
  index into the data file.
- **Independent confirmation of column-major.** Its
  `read_block_from_coordinates` computes `y + (x * height)` — the same order we
  derived from Sphere's `CServerMap.cpp:445`. Two independent implementations
  agree, so the invariant can be considered pinned.

It also has `art`, `anim`, `gump`, `hue`, `font`, `texmap` with optional `image`
conversion — none of which we have, and all of which are pre-UOP-only.

## What targeting an old client would cost us

Two gaps left, in order of how much work they are — a third, the map width, is
closed; see [the compatibility backlog](client/evidence/2026-08-24-the-client-compatibility-backlog.md):

- **`verdata.mul` support.** Mandatory below 5.0.0a and entirely absent here:
  `grep -rn verdata --include='*.rs' crates` finds nothing.
- **The lower half of two protocol boundaries.** `Feature::NewContextMenu`
  (6.0.0.0) gates the *new* `0xBF.0x14.0x02` form, so nothing stops us sending
  the old form to a client with no popup menus at all. Same shape of gap for
  cliloc: `Feature::Tooltips` (4.0.0a) covers OPL, but the plain localized
  message `0xC1` has no entry.

## Getting the files

None are in this repository and none ever will be — see
[`findings.md`](findings.md). Tests read `OPENSHARD_CLIENT` and skip when it is
unset.

**The current 7.0.x files are the only set available legally and for free**,
from the official Classic Client download at <https://uo.com/client-download/>.
An account is needed to play on the official shards; the files themselves
install without one. That settles which version to bring up first as much as any
technical argument does: 5.0.8.3 is distributed by shards, not by Broadsword.

On Linux, install into a wine prefix and point `OPENSHARD_CLIENT` at the data
directory — ClassicUO itself has a native Linux build and does not need wine;
only the installer does. What the installer produces is data, not a program, so
copying the directory off a Windows machine works just as well.

The installed set is 7.0.x: the map is in `map0LegacyMUL.uop`, statics are in
`statics0.mul` + `staidx0.mul` (there is no `.uop` form of statics, at any
version), and tiledata is the 64-bit-flag High Seas layout.
