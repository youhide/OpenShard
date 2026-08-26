# 3. World — a client walks in Britannia

> Open work and follow-up findings from this phase are tracked in the
> [consolidated backlog](../backlog/README.md).

- [x] `Direction` / `Facing` — steps ported verbatim from Sphere's `sm_Moves`
- [x] World entry: 0x5D, 0x1B, 0xBF.0x08, 0x20, 0x4F, 0x55
- [x] `movement`: the walk handshake, turning as a step, the world edge
- [x] `WalkSequence` — 0 means fresh, 255 wraps to 1, a reject resets both ends
- [x] `tiledata.mul` — both layouts, told apart by arithmetic
- [x] UOP containers — the map is in `map0LegacyMUL.uop`, not `map0.mul`
- [x] `map*.mul` / `statics*.mul` — column-major blocks, 2.9M statics
- [x] `MapTerrain` — real heights, walls, water, the two-unit step limit
- [x] **The movement check matches the 2D client**, a blend of both references:
  ServUO/RunUO's `GetStartZ`+`Check` for *reach* (a step reaches the top of the
  surface underfoot plus two, not the feet — the fix for slope rubber-band) and
  Sphere's `GetFixPoint` for *selection* (stand on the highest surface in reach,
  not the nearest — the fix for climbing building stairs). See the note below.
- [x] `MobileStatus` (`0x11`) — the status bar, and the only packet carrying
  **stamina**; without it the client sees zero stamina and silently refuses to
  run. Sent on world entry and answered on `0x34`. Versioned 3–6 by
  `status_packet_version` (type 6 is the 121-byte High Seas shape).
- [x] `WalkPace` — a token bucket; a client can no longer walk as fast as it sends
- [x] `World::tick` — a fixed 40Hz timestep; commands in, events and packets out
- [x] Core components: `Position`, `Heading`, `Body`, `Name`, `Client`, `Movement`
- [x] Domain events: `PlayerEntered`, `MobileMoved`, `StepRefused`, `PlayerLeft`
- [x] Spatial index — a 64-tile sector grid, Chebyshev range
- [x] Other mobiles: 0x77/0x78/0x1D, and the `seen` set that sends each once
- [x] Character creation (0x00 and 0xF8), not just playing a configured name
- [x] Starting cities — the nine classic Felucca towns, filtered to the loaded
  facets; a new character spawns in the one it picked
- [x] Multiple facets — `[world] facets`, terrain and interest per facet

**Three things about the client file formats that are not written down
anywhere**, each of which parses cleanly and produces a plausible, wrong world
if guessed:

- **`map0.mul` may be a stub.** It can be 90MB of zeroes, at exactly the right
  size. The real map is `map0LegacyMUL.uop`. Reading the stub raises no error
  and yields a flat, empty, perfectly smooth world.
- **UOP entries need not be in index order.** Sorting by file offset — the
  obvious shortcut — scrambles the map. The entries are named by a 64-bit hash
  and it has to be computed.
- **The UOP hash packs its halves `(b << 32) | c`.** Jenkins' own signature is
  `hashlittle2(key, len, &pc, &pb)`, so `(c << 32) | b` is the natural reading.
  It matches zero entries.

**The map tests no longer share one path under `temp_dir()`.** Two of them wrote
fixtures to `std::env::temp_dir()/openshard-map-test/` — one fixed directory in a
place every process on the machine shares — and deleted them at the end, so two
concurrent runs of the workspace's tests interleaved a write, a read and a remove
on the same file. `a_map_with_no_statics_loads_as_bare_ground` was seen failing
once under a full `cargo test --workspace` and passing alone immediately after,
which is how that flake always presents. Both now take a `ScratchDir`: a
directory named by pid and a counter, removed on `Drop` — so a failing assertion
also stops leaving the fixture behind, which the old explicit `remove_file` did
not.

## Contents

- [Movement surface investigation](movement-surface-investigation.md)
- [Mobiles and the shove rule](mobiles-and-shove.md)
- [Runtime lookups and the tick](runtime-and-tick.md)

## The pace limiter takes Sphere's numbers and not its arithmetic

The intervals are Sphere's — 200ms on foot, 100ms running — and those are worth
having: two decades of tuning against real clients.

The arithmetic is ours. Sphere's `Event_Walking` keeps a running average in
milliseconds and clamps it against `WALKBUFFER`, which defaults to `15` — a
duration compared against what its own docs call a count of "points". Read
literally, a normal walker sits at a balance of 15ms and one early step puts it
at `15 - 200 = -185`, refused instantly, with none of the burst tolerance the
buffer exists to give. Either the constant means something undocumented or the
check does not do what it says. `movement::WalkPace` is a token bucket instead:
the same intent, stated plainly.
## The walk check is one part ServUO, one part Sphere

The client draws z it computes itself — the walk ack carries none — so the server
has to land a step on the *same* height the client does or every step
rubber-bands. Neither reference alone matches the 2D client; the working check
takes one half from each.

- **Reach is ServUO's `GetStartZ`+`Check`.** A step reaches `start_top + 2`, where
  `start_top` is the top of the surface the mobile stands on — a sloped land
  tile's highest corner, a stair's full height — not its feet. Reaching from the
  feet (`from_z + 2`) refuses steps up a slope the client took: measured against a
  real facet, that was 10,620 steps around Britain the server blocked and the
  client allowed. Land reachability is the tile's *lowest* corner and you stand at
  its `GetAverageZ` centre, floored toward negative infinity.
- **Selection is Sphere's `GetFixPoint`.** Among the surfaces in reach, stand on
  the **highest**, not — as ServUO's `Check` does — the one nearest the current
  height. A stair tile carries the floor below it and the step above; ServUO's
  nearest-z keeps you on the floor while the client climbs, so building stairs
  "drop" you and you cannot get in. The highest-in-reach rule climbs them.

The two rules agree on bare ground — one surface, so highest *is* nearest — which
is why the ServUO half tested clean on open terrain and the divergence only
surfaced on stacked geometry (stairs, house floors). The whole of it is
`MapTerrain::check` / `start_surface`, ported with the arithmetic audited as
everywhere else.
