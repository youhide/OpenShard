# Roadmap

Order, not dates.

## 0. Foundation — done

- [x] Cargo workspace, all 20 crates declared
- [x] `entities` — generational `EntityId`, UO `Serial`, sparse-set columns, `Registry`
- [x] `events` — double-buffered `Events<E>`, `Cursor<E>`, `EventBus`
- [x] `protocol` — `ClientVersion`, `Era`, `Feature`, `FeatureSet`
- [x] `cargo test --workspace` green: 125 tests, clippy clean, fmt clean

## 1. Protocol — mostly done

- [x] `PacketReader` / `PacketWriter` — std only, every read fallible
- [x] Client packet length table ported from Sphere's `receive.h` (70 packets)
- [x] `frame_client_packet` — split a TCP stream into packets
- [x] Seed handshake state: old 4-byte form, new `0xEF` form, lone-`0xEF` segment
- [x] Login sequence: `0x80`, `0x82`, `0xA8`, `0xA0`, `0x8C`, `0x91`, `0xA9`
- [x] `0xBD` client version report → `ClientVersion` → `FeatureSet`
- [x] Server→client Huffman compression (Sphere's "golden key" table)
- [ ] Login encryption — see below
- [ ] Packet tests against captured dumps from real clients

Version-gate everything from the first packet. Retrofitting is the thing this
crate exists to avoid.

The codec deliberately has no dependencies — not even `bytes`. Keeping the
foundation crates dependency-free is what lets them build in environments where
crates.io is unreachable.

### Backlog from the newtype sweep (`docs/protocol_newtypes.md`) — sweep done

Found while wrapping `world.rs`'s remaining bare integers, back when the sweep
was only N1. The sweep itself (N-pilot through N8) is now complete: every
bare-integer field in `crates/common/protocol`'s packet structs is either a
named type or on the reasoned, machine-checked allowlist
`crates/common/protocol/tests/bare_integer_fields.rs` enforces. What is left
below is what the sweep found but could not fix, because the fix crosses out
of `protocol` — into `state`, `config`, or the tick — which the sweep's own
rule (`common/*` is below the server) puts out of its reach on purpose.

- ~~**Two types for one facet byte.**~~ Fixed: `protocol` owns the one
  `world::Facet(pub u8)` now, the way `Serial` is owned there and borrowed by
  `entities`; `state::components::Facet` is gone, and every crate that used it
  (`world`, `npc`, `ai`, `items`, `skills`, `magic`, `server`, the client) reads
  `openshard_protocol::world::Facet` directly instead. The two `MapId(facet.0)`
  double-conversions collapse to a plain `facet` — the packet's own field and
  the world's notion of a facet are the same value now, not two synchronised
  ones.
- ~~**A region's light level is never bounded.**~~ Fixed: `World::register_regions`
  (`world/src/tick/regions.rs`, the one place every `Command::RegisterRegions`
  — today only from `scripting::op_register_regions` — lands) now warns per
  region whose `light` is above `0x1F`. `world::Light` still does not clamp,
  deliberately, because the client does; this only makes a shard's own typo
  audible instead of silent.
- ~~**The tick keeps light and music as bare numbers.**~~ Fixed: `last_light`
  is `HashMap<_, Light>`, `last_music` is `HashMap<_, MusicId>`, and the
  `LIGHT_*` constants in `tick/defaults.rs` are `Light`, not `u8`. Only the
  seam where a `Region`'s own `Option<u8>`/`Option<u16>` data enters the tick
  (`light_for`, `start_music`) still wraps — the same boundary every other
  newtype in `state` converts at.
- ~~**`gameplay.season` is still a `u8` in config and in `WorldState`.**~~
  Fixed: `GameplayConfig::season` and `Gameplay::season` are both `Season`
  now. Config deserializes it through a `#[serde(with = "season")]` module
  (`crates/common/config/src/lib.rs`, the way `AccountName` already does)
  that calls the new `Season::try_from_bits` — unlike `from_bits`, which
  silently falls back to spring, this refuses a sixth season at parse time,
  so `ConfigError::UnknownSeason` (which duplicated the same check one step
  later) is gone. `tick/enter.rs`'s world-entry send no longer calls
  `Season::from_bits` at all — the value has been a `Season` since boot.
- ~~**`mobile::OpenPaperdoll::flags` is a bare `u8`**~~ Fixed in N2:
  `PaperdollFlags` replaced the two loose `pub const u8`s
  (`PAPERDOLL_WARMODE`, `PAPERDOLL_CAN_LIFT`) with a named `with`, on N10's
  allowlist for nothing because there is no bare field left to allowlist.

### Login encryption is deliberately deferred

Sphere ships `sphereCrypt.ini`: a per-client-version key table for the login
stream, and separate game-stream encryption. It is a real lift and it buys
nothing — the keys are extracted from the client binary, so anyone can read the
stream. It is obfuscation, not security.

ClassicUO connects with encryption off, which is what freeshards use in
practice. So: support unencrypted first, get a client on screen, and revisit
only if a real client turns up that cannot be configured without it. Do not
mistake this for a security feature when it lands.

## 2. Gateway and login — done

- [x] Sans-io `Connection`: handshake then framing, no async, no sockets
- [x] Tokio listener, one task per connection, events onto a channel
- [x] Disconnect handling; every protocol violation is fatal
- [x] `Accounts` trait + `DevAccounts` in-memory store
- [x] Sans-io `LoginServer`: 0x80 → 0xA8 → 0xA0 → 0x8C → 0x91 → 0xA9
- [x] Auth key issued at relay, one-shot, expiring, bound to its account
- [x] `crates/server/server` — a binary that runs and reaches a character list
- [x] `config` — TOML, validated at load; accounts and addresses come from it
- [x] A fresh checkout writes a default `openshard.toml` and runs
- [x] **Character deletion** (`0x83`). The delete button on the character-select
  screen works: `DeleteCharacter::decode` reads the slot, and the handler (beside
  `create_character`, where both login and world are in reach) drops it from the
  account's list and queues a `Command::DeleteCharacter` the tick turns into a
  `Journal::forget_serial` — so the next snapshot carries the serial in `removed`
  and the store drops the character row and its whole inventory, off the tick like
  every other write. The **serial stays reserved** (`reserve_serial` at boot is
  never undone — a packet in flight may still name it). A character *being played*
  cannot be deleted (`World::is_online`, a synchronous read between ticks): the
  reply is `0x85 CharBeingPlayed`; a bad slot is `0x85 CharNotExist`; a good delete
  resends the list with `0x86` (the `0xA9` character block reused). Ported from
  ServUO's `DeleteCharacter`/`DeleteResult`/`CharacterListUpdate`.
- [x] **Store-backed accounts and password hashing.** Credentials are argon2 PHC
  hashes now, never plaintext (`crates/server/login/src/password.rs`, over the `argon2`
  crate; the salt comes from the `getrandom` the auth keys already use). Boot reads
  `store.accounts()` into the in-memory `DevAccounts` as the source of truth for
  `verify`, and config `[[accounts]]` **seeds only what the store has never seen** —
  the plaintext hashed once, written to both memory and the store, and thereafter
  the store wins (changing a config password does nothing). `verify` runs argon2 in
  the sync `Accounts` path (hashing is CPU-only, so no async bridge into the sans-io
  `LoginServer`); `SCHEMA_VERSION` moved to 10, so an older database is recreated and
  re-seeds hashed. `access` stays config-derived and re-looked-up each login, never
  saved. The `constant_time_eq` shim is gone — argon2's own verify is constant-time
  over the digest.

`config` refuses to start on a wildcard `advertise` rather than accepting it and
failing silently for every remote client. That check is the reason the crate
exists; parsing TOML is three lines of serde.

The connection logic is a pure state machine on purpose. Everything hard about a
gateway is byte boundaries — a seed split across three segments, four packets in
one read — and a real socket will not reproduce those on demand. As a state
machine each one is a deterministic test with no ports and no sleeps.

`Server` hands events to a channel rather than calling back. A callback would run
world code inside a network task, on whatever thread Tokio picked, whenever bytes
arrived. The channel is where async stops and the tick begins.

### Entering the world says which character, once

`Command::Enter` carried seven fields, four of them `Option`s that were only ever
all present or all absent together: the saved serial, the saved spot, the look and
the sheet. Four correlated `Option`s are four chances to build a state that cannot
happen — a saved serial with no saved position puts a character every packet ever
sent refers to back at the start city — and nothing could check it. Every caller
had to unpack a row correctly instead.

It now carries one `Entering`, whose `character` is a two-variant `Character`:
`Fresh(FreshCharacter)` has a facet, an optional start and an optional look and
sheet; `Stored(StoredCharacter)` has all five and no `Option` at all, so a
half-restored character cannot be spelled. `StoredCharacter::from_record` is the
one place a `CharacterRecord` is unpacked, and the relogin tests call it rather
than keeping their own copy of the unpacking. The private `struct Entering` beside
`World::enter` — a field-for-field copy of the command's own payload — is gone.

The remaining step is the one this makes worth doing:

- [x] **The roster belongs in the world.** Done — S4 of
      [connection_state.md](connection_state.md). `restore_characters` hands the
      store's rows into `World` like every other `restore_*`, and `reserve_serial`
      is something the world does to itself on the way in rather than a favour the
      shard asks for. `departed` and its drain are gone: the logout writes the
      roster at the same instant it hands the journal its copy. `Character::Saved`
      and `Command::DeleteCharacter { account, name }` name a character instead of
      carrying what the shard looked up, and `run_shard` no longer holds a roster
      to look anything up in. `pending_inventories` stayed — it is keyed by mobile
      serial and holds NPC gear too; see the finding in connection_state.md.
      Accounts stay outside: argon2 must not run inside a tick, and
      `openshard-login` exists to be sans-io. This is also the shape UO itself has
      — an account is global, a character belongs to a shard. What is left is the
      character *list*: `0xA9` becoming a `Command` answered out of a tick, the way
      `RequestStatus` is answered with a status, is S5.

- [x] **The character screen belongs in the world too.** Done — S5 of
      [connection_state.md](connection_state.md). The roster stopped being "where
      the saved characters were" and became the account's list, which is the fact
      the login crate's `Accounts` used to hold: a character exists from the moment
      it is created, and carries a record only once something has written one. With
      that, `0xA9`, `0x00`/`0xF8`, `0x83` and `0x5D` are all answered out of a tick,
      and `Accounts` is down to credentials, blocking and access — the things a
      *login* is about. `LoginServer` lost its starting cities and its two
      capability masks to the world's `CharacterScreen`, and its game-login handler
      now returns `Response::Idle`: the crate ends where the world begins.

      Two hazards closed themselves on the way. `0x83`'s slot indexes the list
      `0xA9` was built from — one value, one process — instead of two lists that
      merely happened to be ordered alike; and "is this character being played",
      which has now been asked three ways (a serial the caller had to look up, a
      scan of the shard's session table, and this), is asked of the entity that is
      the fact.

Found while doing the above, none of them blockers — all fixed:

- ~~**A player's saved `facing` is written and never read.**~~ Fixed:
  `StoredCharacter` now carries `facing` (`StoredCharacter::from_record` reads
  `record.facing` the same way `restore_mobiles` already did), and `enter` restores
  it instead of hardcoding `Facing::walking(Direction::South)` — that default is
  now only for a genuinely fresh character, which never faced anywhere before.
- ~~**A zero stat age means two different things.**~~ Fixed: `enter` no longer
  feeds a zero age through `now - age`, which landed on `now` itself and read as
  "rose this instant". An age of zero now restores to a `LastStatGain` stamp of
  zero — "never gained" — for exactly that stat, whether the character is brand
  new or an old save whose age happened to be zero.
- **Belongings already live in the world**, in `pending_inventories` keyed by
  serial, filled by `restore_items` at boot and by logout. So a `StoredCharacter`
  that carried its own items would duplicate what the world holds, not simplify
  it — another argument for the roster moving in rather than the items moving out.
- ~~**The newtypes stop one line short.**~~ Fixed: `WorldState::facets` is keyed
  by `Facet` and `Body` holds `Graphic`/`Hue`, both throughout `crates/server/state`
  — the seam, not every crate's own internal `facet: u8`/`u16` currency, which
  stays raw and converts at the point it crosses into `WorldState` (a packet
  encoder, a SQL record and a `deno_core` op are boundaries in exactly the same
  sense). `enter` carries the facet and the look unopened now; only a `warn!`
  field still spells out `.0`, because a tracing field wants something printable.
- ~~`dispatch_world_packet` opens with `account().cloned().unwrap_or_default()`.~~
  Fixed: it now drops the connection on no account, the same
  `let Some(account) = … else { return false }` guard `create_character` and
  `delete_character` already use — a `CharacterPlay` this early is a client that
  never completed a game login, not a default account to invent.

### Still to do: the character screen is one conversation, split across two files

The design this works toward is settled and written down — see "Sessions and the
character registry" in [architecture.md](architecture.md). Done so far:
`LoginSession` carries the account and the socket kind in its state rather than
in fields kept in step by hand; `Sessions` answers who is playing what; `Roster`
replaced a bare `HashMap<(String, String), CharacterRecord>` threaded through
five functions. What is left:

- [x] **`charscreen.rs`.** Done in S5, and one module further out than this
      asked: create (`0x00`/`0xF8`), delete (`0x83`) and select (`0x5D`) are one
      conversation, and it is the *world's* — `world/src/tick/screen.rs`, answered
      out of the tick that owns the roster.
- [x] **Nothing on the character screen takes `&mut World`.** Done in S3 and S5.
      `dispatch_world_packet` is `fn(ClientPacket, ConnectionId) -> Option<Command>`
      and the screen's three packets are commands. What is left holding a `&mut
      World` is the binary's own router, which queues what the translation
      produced — that is the seam, not a rule broken.
- [x] **`run_shard` shrinks.** Done in S6. The seven `restore_*` functions and
      the accounts are `boot::restore`, in `boot.rs` beside `load_world` and
      `open_store`; the loop's own state is one `Shard` value instead of eight
      locals; and `keys.expire` has its own `select!` arm on its own timer, where
      memory upkeep for abandoned relay keys belongs.

One smaller thing noticed on the way through, not blocking:

- `Command::Enter` carries a `CharacterSheet` built out of four
  `openshard-persistence` record types. The tick's input vocabulary should not be
  shaped by the database's row format; it is also the single reason `Command`
  cannot move below `openshard-world` should that ever be wanted.

### A connection's state is kept in two tables that must agree

Read while asking why `world_handle_network` has to hold `Sessions`, `LoginServer`,
`World` and `Roster` at once. None of these was a bug on a working shard; all of
them were the same seam being unnamed. The plan that acted on them, including the
steps above, is [`connection_state.md`](connection_state.md).

**All five are closed — S1 through S7 have landed.** What is left of that plan is
its backlog, and that backlog is the live one: a dozen findings, each with the
file it is in and what it would cost to act on, at
[`connection_state.md` → "Backlog, found while doing S1 through S7"](connection_state.md).
Anyone picking this area up should start there rather than here; the summaries
below are kept only so a reader who arrives at this section knows what the plan
was *for*.

- ~~**Presence is a bool, and it is set optimistically.**~~ Fixed in S2.
  `Session::playing` was set as `Command::Enter` was *queued*, while `World::enter`
  had three early returns that refused silently — after any of them the session
  said it was playing, the world had no entity, and the client sat on "logging into
  shard" being told nothing. `WorldPhase` names the state
  `Option<PlayedCharacter>` could not (`Outside → Entering → Playing → LoggingOut
  → Left`), and a refusal comes back as an event rather than as silence.
- ~~**`in_world()` is a second copy of `players.contains_key`.**~~ Fixed in S3.
  Thirty arms of `dispatch_world_packet` opened with the same guard; the phase is
  matched once now, in `handle_world_packet`. It is still a projection of the
  world's fact — see D4 — but there is one place that reads it.
- ~~**The world cannot answer a connection that has no entity.**~~ Fixed in S1.
  The client version lives on the connection's row in `openshard-state`, not on
  the entity, which is what let the character screen become world commands in S5.
- ~~**argon2 runs on the tick's task.**~~ Fixed in S6, and it was not as cheap as
  this said. Moving the hash to a blocking task means the login conversation has
  to *suspend*: `LoginServer::handle` returns an `Outcome` — bytes to send, or a
  `CredentialCheck` to run — the session waits in a state named for it
  (`VerifyingAccount`, `GameState::Verifying`), and the verdict comes back through
  `LoginServer::resume` on a `select!` arm of its own. The account stays in the
  state machine and the check carries no identity, so a verdict that reached the
  wrong connection authenticates nobody. The blocking pool is bounded by a
  semaphore: 19 MiB times `spawn_blocking`'s 512 threads is ten gigabytes, and the
  loop used to bound that by having no choice but to run one at a time.
- ~~**Per-connection world state is seven maps and a hand-written teardown.**~~
  Fixed in S7, in two halves, because the maps turned out to be two problems: what
  is keyed by a connection moved onto its row, and what was keyed by the *player's
  entity* — the four gump tables and the targeting cursor — was re-keyed and moved
  there too. `disconnect` reads one field off the row it removed. The exception is
  `open_containers`, an inverted index whose every read asks "who is watching this
  container"; putting it on the row would turn that into a scan per item change,
  and the backlog says so.

## 3. World — a client walks in Britannia

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
- [x] `World::tick` — a fixed 20Hz timestep; commands in, events and packets out
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

### The pace limiter takes Sphere's numbers and not its arithmetic

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

### The walk check is one part ServUO, one part Sphere

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

### ~~Backlog: a pier or bridge over low ground can drop a walker under it~~ — the mechanism is refuted

`MapTerrain::check`'s `landCheck` guard (`movement/src/terrain.rs:207-217`) is
ServUO's own `Movement.cs` `landCheck`, ported variable-for-variable and
direction-for-direction — audited against the reference, not a porting bug. It
exists to discard a low decorative static the terrain visibly pokes through (a
rock embedded in a hillside): when the land under a platform static is walkable
and its average height (`land_center`) is close to or above the static's own
stand height (`our_z`), the static is dropped from the candidate list and the
walker falls through to the land instead.

ServUO's own `landCheck` does not exempt `Bridge`/climbable statics from this
either — the flag only changes `itemTop` (how high a step must reach to clear
the static), never the guard itself. That is fine as long as a bridge or pier
sits over water, where `land_is_ground` is false and the guard never fires. It
is not fine at the shore end of a pier or the bank end of a bridge over a
ravine, where the ground underneath is ordinary walkable land whose average
height can read close to the deck: the guard fires, the deck static is
discarded, and the walker lands on `land_center` — which for a structure
spanning a drop is often well below the deck. That reads as "fell under the
bridge," and matches a player report (2026-08-02) of falling underground
specifically on piers and bridges.

Not fixed yet because it is a real divergence from the cited reference, not an
arithmetic slip, and needs a decision rather than a silent patch: exempting
`is_climbable()` statics from this guard would be a deliberate deviation from
`Movement.cs`, and wants a repro against real client files (a pier whose shore
tiles are dry land, not water) confirming the fall before touching it.

**Re-evaluated 2026-08-16 and kept**, so the next reader knows it was looked at
rather than merely untouched. Two things changed around it and neither moves the
decision. [`boats.md`](boats.md)'s B5 found that the repro **does not need client
files** after all — a synthetic multi carrying a climbable platform component at
a known z over land of known height reproduces the shore-end case, and
`Multi::new` is public. And it found a second consequence: turning
`MapTerrain::swimming` on would fire this same guard under every boat deck and
drop a boarding player into the sea, which is why that flag stays false. The
deviation is still a deviation, and taking it is still a decision nobody has
taken.

#### 🚩 The repro was finally run, 2026-08-23, and **this mechanism does not exist**

Two surveys over the whole of facet 0, both `#[ignore]`d in
[`terrain.rs`](../crates/common/movement/src/terrain.rs) —
`land_check_survey` and `predicted_step_survey`. Neither is an assertion, for
`boat_step_cost`'s reason: an assertion over a facet's worth of shipped art is
an assertion about the art.

| | |
|---|---|
| the guard discards a platform | **2,381** pairs of (tile, static); **596** of them climbable |
| and the body then lands **below** the surface it discarded | **0** |
| and the tile is refused outright instead | 722 (378 climbable) |
| of those, walled *by the guard* — the body would have fit | **242** (71 climbable) |

**The fall cannot happen, and the guard's own third condition is why.**
`landCenter > ourZ` means the guard only ever fires where the *land is higher
than the deck*. So discarding the deck moves the body **up** onto the land, never
down — which is the opposite of what this entry has claimed since it was
written. The 2026-08-02 report is real; `landCheck` does not explain it.

**And the client is not dropping the body either.** The second survey walks
every step a body can take off a bridge or pier — 224,950 of them that the shard
*allows* — and compares `predict_step`, which is what the client draws
immediately, against `check`, which is what the shard decides. Only permitted
steps count: a refusal comes back as a `0x21`, which carries x, y **and** z, so
the client is corrected and never shows it. The permitted step is the one that
goes uncorrected, because its `0x22` carries no position.

| | |
|---|---|
| permitted steps off a bridge or pier | 224,950 |
| client and shard disagree at all | **77** (0.03%) |
| client draws the body **lower** | **0** |

**What the guard actually costs is 242 tiles a body cannot enter** — an invisible
wall, not a fall — and **that is parity, not a defect**: the port is
character-for-character ServUO's
(`Scripts/Services/Pathing/Movement.cs:238`, `landCheck = itemZ; if (Height >=
StepHeight) landCheck += StepHeight; else landCheck += Height;` and the same
four-clause guard), so the same 242 walls stand on a ServUO shard.

**So the decision this entry was holding itself back for is not owed.** Nothing
should exempt climbable statics from the guard on the strength of a fall that
does not happen. What *is* still owed is the report's real cause, and these two
surveys say where not to look. The suspects left, in order:

- **A boat moored at a pier.** `walk.rs`'s `aboard` takes the *nearest* live
  surface at any distance and has no reach filter — its own backlog entry, under
  R3 below. A pier with a ship beside it is exactly the shape that puts a live
  surface near a body standing on map terrain, and neither survey here can see
  it: both walk the bare map with no overlay at all.
- **A multi-step walk**, where a single step is right each time and the sequence
  drifts. Both surveys measure one step from a known surface.
- **Arriving rather than walking** — a login, a spawn, a gate or a teleport onto
  a deck, which reach `spawn_z` and not `check`.

### ~~Backlog: a mobile is not an obstacle~~ — closed, and it was two entries

**A mobile is an obstacle, on both sides of the step.** The method is the one
this entry chose — ask the sector grid, not a second copy in `Obstructions` —
and it is now where every caller reads it rather than at two call sites.

Read the shape before the history: `Footing` has a **fourth field**,
[`Bodies`](../crates/common/movement/src/footing.rs), and `walk::landing` asks
it last, at the height the body would arrive at, the way ServUO's `Check` does
(`Movement.cs:344`). Because `landing` is what each of `steps_out_of`'s eight
answers is, the diagonal's flanks obey it too, and so does everything above it:
`can_step`, `step_allowed`, `find_path`, the coarse route's refinement.

`Bodies` holds feet and no identity — sorted by tile, borrowed, **built at the
question and thrown away**, exactly as a `MapTerrain` is. `WorldState::crowd_near`
builds one out of the sector grid; there is nothing to keep in step and no
`unblock` to forget. A step asks for a reach of 1, a plan for the distance to its
goal capped at `CROWD_REACH`, and the bound costs a re-plan and never a wrong
step, because the step reads its own crowd.

The three rules are in `body_blocks` and `walks_through_bodies`, where the
registry is:

- **The dead do not block, and the dead are stopped by nobody** — ServUO's
  `CanMoveOver`, both halves. A corpse is a `Drawn` item and never was a body; a
  *ghost* keeps its `Body` (a shroud is a body graphic) and used to wall a
  doorway the living could neither see nor pass.
- **A mobile may always step off its own tile**, which comes to nothing more
  than the mover being absent from its own crowd.
- **Staff walk through bodies as through walls** — `Staff`, the flag a `.gm`
  puts down, not the account's access level. A *hidden* game master is in nobody
  else's way either, which is ServUO's `t.Hidden && t.IsStaff()`; a hidden player
  still blocks.

And the exemption **reaches the client**: `stance_of` sets
`StatusFlags::IGNORE_MOBILES` (`0x10`) on a staff mobile's `0x77`/`0x78`. The
client keeps its own copy of the rule and applies it to what it predicts, so
without the bit a game master's step is allowed at one end and refused at the
other. That bit was in `StatusFlags`' table with nothing setting it, under the
note that a constant nobody sets is a constant nobody has tested; this is the
day it was wanted.

#### What this entry got wrong, and what is left

**Half of it was already built.** `WorldState::mobile_occupies` had been refusing
a step onto an occupied tile since 2026-08-14, in `tick/motion.rs`'s two step
paths. So "a player walks through a standing NPC" was false when it was written.
What was true — and worse than the entry said — is that the *step* knew about
bodies and the *plan* did not: a creature whose quarry stood behind a bystander
walked into the bystander, was refused, re-decided the same direction next beat,
and never went round. `a_chase_rounds_a_line_of_bystanders` is that bug, and its
second assertion is that the creature went round rather than through — reaching
the quarry on its own would also pass on a shard that had forgotten bodies
entirely.

~~**The client plans through bodies.**~~ **Closed.** `clutter::crowd` is the
client's `crowd_near`: the mobiles in its view, filtered, sorted by tile, built
at the question and thrown away, and handed to every footing a *step* is decided
against through `Footing::among` — the held arrow, the click-to-walk plan, and
the route the HUD draws, which is the same plan. The guide reading keeps
`Bodies::nobody`, because a bystander must not rewrite a corridor's topology.

What the client had was not a crowd but a *disguise*: `clutter::fill` laid every
mobile into the overlay as furniture a body's height tall, under a comment
admitting it was "not a category the shared type names". Two things were wrong
with it, and the second is why the disguise could never have been made right:

- **Sixteen against fifteen.** A cover blocks `[z, z + height)` and a mobile was
  given `PLAYER_HEIGHT`; the shard measures body against body with
  `MOBILE_OVERLAP`, which is one less on purpose. At exactly the boundary this
  end refused a step the shard allows.
- **A cover cannot name an exemption.** The overlay has no idea who is asking, so
  staff and the dead were held to a rule the shard exempts them from.

And the bit that carries the exemption **did not reach the one client that needs
it**. `stance_of` fills the `0x77`/`0x78`, which is how a client learns about
*somebody else* — but a client only ever predicts its *own* step, and all three
senders of the `0x20` wrote `StatusFlags::NONE` into it. A game master learned
that every other staff member walks through bodies and never that they do. The
`0x78` a player is sent about itself does carry the byte, which is what let the
gap survive: it is true until the first step or relocation sends a `0x20` over
it. All three now read `stance_of`, which is also how a *ghost* is told — death's
own `0x20` — and that one is not a corner case: a ghost's walk home passes
through the living, who cannot see it to move aside.

Left open, and none of it blocks anything:

- **A player does not shove, and in UO a player shoves.** This engine
  hard-blocks, which is not parity and is not invisible: the stock client has
  the mirror of the rule and draws the step we refuse. Its own entry follows.
- **A boat's deck and a moving multi.** The crowd is read off the sector grid,
  which holds a mobile's own tile. Nothing here asks what happens to two bodies
  on a deck that moves under them.
- ~~**`Sectors::nearby` is still linear in a bucket**, and this entry is the second
  per-step reader that was predicted below. It is now real.~~ **Closed** — a
  bucket is two lists now and `crowd_near` reads the mobile one. See the backlog
  entry below.

#### Found while closing it

- **`ai::step_toward` has no production caller.** Every body that walks goes
  through `step_body_toward`, which is the sibling with somewhere to write a
  refusal down; the only thing left calling the plain one is
  `tick/tests.rs`'s `walk_toward`. It is `pub`, it carries the doc comment that
  explains the two-search fall-back for both of them, and it gained a `mover`
  argument this session for a crowd only its test will ever have. Either its
  test uses `step_body_toward` and it goes, or its doc moves to the sibling and
  it stays as the deliberate pure-function reading — but "public, documented,
  and called once from a test" is not a state anybody chose.
- **The crowd is now built on every walk request, and used on some of them.**
  `mobile_occupies` was asked only when the walk had already succeeded; the
  crowd has to exist before `Walker::request` is called, so a *turn* and a
  pace-refused step pay a sector sweep for nothing. It is one `nearby` call and
  turns are a small share of requests, so it was left alone deliberately — but
  it is the same sweep the entry above says is linear in a decorated town, and
  if that ever needs shrinking, `intend` is the shared function that says
  whether a request steps at all.
- **Two client diagnostics quietly stopped counting bodies, and they are right
  to.** `picking_query.rs`'s level marker and `terrain_overlay` both ask
  `can_fit` over the cluttered footing, so while mobiles were covers a bystander
  made a tile read "blocked" in a debug wash and on the height diagram. `can_fit`
  has said all along that "a body is not what this places", so removing them puts
  the two back in step with their own contract — a diagnostic about the *ground*
  no longer flickers as people walk about. Written down because it is a visible
  change nobody asked for, not because it wants undoing.
- **The client's crowd is built per ask, not per view.** `clutter::fill` is a
  projection rebuilt when the view changes; `clutter::crowd` is rebuilt at every
  question, and `Steering::steer` is called on *every raw mouse-move*. It is a
  screenful of points and a sort, so it does not matter yet — but the two
  neighbouring functions read the same list on two different clocks, and only one
  of them has a reason to.
- **`is_ghost` is the client's whole answer to "is that one dead".** Nothing on
  the wire says a stranger died; the body id does. That is fine for the crowd,
  and it is the same pair the drawing reads — but it means a shard that gave a
  living mobile a ghost body graphic would have it walked through. Worth knowing
  before anybody writes a spectral NPC.
- **The `0x20`'s flag byte is now sent and still half-ignored.** The client keeps
  `Player::war` out of it deliberately (`0x72` is the one home for the stance), so
  the byte now arrives carrying a war bit that is read from nowhere. That is the
  honest arrangement — the packet says what the shard sent — but it is the second
  place `WARMODE` travels, and the note in `view.rs` is the only thing saying
  which one wins.

### Backlog: the shove — a rested player pushes past a body rather than stopping at it

**A good mechanic, and the reason to write it down is not only that it is
good.** A body in the way is not a wall in UO: a player at full stamina walks
*through* the crowd, spends ten stamina and is told they shoved somebody. It is
a small rule that does a lot of work — it keeps a doorway from being griefable by
standing in it, it makes a busy bank an obstacle course rather than a wall, and
it prices moving through people in the one currency a fight already cares about.
Stamina is the throttle: shove your way across a market and you arrive with
nothing left to run with.

**And this engine currently contradicts the client about it.** ClassicUO applies
the mirror of the same rule to what it *predicts* — see
[`findings.md`](findings.md) for the line and its reading — so the stock client
walks a rested player's body into a crowd and the shard snaps it back. That is
today's behaviour on every facet, not a hypothetical.

#### The rule, as the two references state it

`Mobile.CheckShove` (`Server/Mobile.cs:3517`), called on the **mover** through
`OnMoveOver` from `Mobile.Move` (`:3216` and `:3243`, at the same
`(other.Z + 15) > z` overlap the rest of the step uses):

| when | what happens |
|---|---|
| The facet has `MapRules.FreeMovement` (everything but Felucca — `Server/Map.cs:129`) | The rule does not run at all. Everyone walks through everyone |
| The mover has `IgnoreMobiles` | Same: no rule, no cost, no message |
| Either party is dead, or is a dead bonded pet | Allowed, silently and free — this is `CanMoveOver` again, and it is already built |
| The one being walked over is hidden **and** staff | Allowed, silently and free — also already built |
| Already shoved once this step (`m_Pushing`) | Allowed, silently and free. The flag is cleared once per `Mobile.Move` (`:3180`), so walking over two overlapping bodies costs one shove, not two |
| The mover is staff | A message, and nothing else — no stamina, no reveal |
| The mover is at **exactly** full stamina | **10 stamina, a reveal of the mover, a message — and the step goes through** |
| Anything else — a mover one point below full | **Refused.** This is the only branch that stops anybody |

The four messages are clilocs, and which one depends on whether the mover is
staff and whether the body being shoved is hidden:

| | visible | hidden |
|---|---|---|
| staff | `1019040` | `1019041` |
| player | `1019042` | `1019043` |

`1019042` reads *"Being perfectly rested, you shove them out of the way"* and
`1019043` *"Being perfectly rested, you shove something invisible out of the
way"* — read off the client's own table, so the mechanic's in-game name is
*shove* and the hidden form deliberately does not say who. The staff pair's text
has not been read. Note what the wording settles: the line goes to the **mover**,
and the reveal is the mover's too — shoving a hidden player does not reveal
*them*.

#### What is ours to decide, and it is one thing

**This engine has no facet rulesets.** `Facet` is an id and nothing else; there
is no `MapRules`, no Trammel/Felucca split, and nothing anywhere that says a
facet is a ruleset. So the first row of that table has nowhere to come from, and
the choice is:

- **Shove everywhere.** One rule, no new concept, and the client's own
  `_world.Map.Index == 0` clause then disagrees on every facet but the first —
  the client would predict a free walk-through where the shard charges stamina.
  Cheap, and wrong in a way a player feels as a stutter.
- **Grow a facet ruleset.** Honest, and it is a thing this engine will want
  anyway: `BeneficialRestrictions` and `HarmfulRestrictions` live in the same
  ServUO enum and are the same question asked about spells. It is a component or
  a field on `FacetState`, not a plan.

The second is almost certainly right, and it is worth deciding **before** the
shove rather than during it — a shove built on "everywhere" has the facet test
missing rather than wrong, which is the kind of gap nothing fails on.

#### The seams it would use

All of them exist:

- `Stamina { current, max }` (`state/src/components.rs:2076`), and
  `combat::spend_step_stamina`, which is already the one place a step charges
  stamina.
- `WorldState::reveal` (`state/src/runtime.rs:3376`) — ServUO's
  `RevealingAction`.
- `WorldState::localized_message`, plus four entries in
  `protocol/src/localized.rs`'s catalogue, which `localized::contains`
  debug-asserts against.
- `WorldState::crowd_near` already finds who is in the way; what the shove needs
  and it does not have is *which entity*, since `Bodies` deliberately carries
  feet and no identity. The shove wants the shoved mobile (is it hidden? is it
  staff?), so it is a second, server-side lookup at the moment a step is
  refused — not a change to `Bodies`.
- And the wire is already right: `StatusFlags::IGNORE_MOBILES`.

**Done when** a rested player walks through a standing NPC and arrives ten
stamina poorer, a tired one is stopped, neither is a rubber-band on a stock
client, and a staff member does it for free. Its natural companion is the
client's own end — `steer.rs` still plans through a crowd, which is the entry
above.

### ~~Backlog: `can_step` does not check the corner, and two obstruct tests are red~~ — closed

**Both are green**, and were closed by the corner-rule repair recorded in
[`navigation_spans.md`](map/navigation_spans.md#out-of-scope-named) — *"`can_step`
has no corner rule, and the shard walked a creature with it"*. The two tests in
`state/src/obstruct.rs` were what found it: they had been asking `can_step` for a
rule that moved into `steps_out_of` in N3, and the answer taken was that
**`step_allowed` owns the corner** — which is the "same one for both callers" this
entry asked for. `a_diagonal_is_refused_when_either_flank_is_blocked` and
`a_live_terrain_with_no_map_reports_no_water` both ask it now.

Kept as a stub rather than deleted because the entry names the question — *which
layer owns the corner rule* — and the answer is a deliberate divergence from
ServUO, which keeps two rules and gives the lax one to creatures. That
divergence is recorded where it was taken, not here.

### Backlog from R2, the live layer joining the map

Found while moving `Overlay` and its friends into `openshard-map`
([`realtime_map.md`](map/realtime_map.md)'s R2). None of them blocks R3, R4 or
R5.

- **`Resources::map()` borrows the whole struct where a field borrowed itself.**
  The client's map is behind a method now, because `World`'s base is optional
  for a shard's sake and a client can never be that shard. A `&self` method is
  opaque to the borrow checker's field disjointness, so a caller wanting
  `&mut resources.<anything>` beside the map has to hoist — `window.rs`'s atlas
  rebuild already did. If a second one appears, the answer is a free function
  over `&Resources::ground` rather than another hoist: that borrows one field,
  exactly as the old `resources.map` did. (The field was `world` when this was
  written; the ground and its span bake are one value now — see
  [`Ground`](../crates/common/movement/src/ground.rs).)
- **`World` has no way to publish a patch.** `MapSnapshot::publish` wants
  `&mut self` and `World::snapshot` hands out a `&` — as does `Ground`, which
  wraps it and forwards that accessor. Nothing in production
  publishes yet — only `openshard-map`'s own tests do — so the accessor was left
  unwritten rather than guessed at. Era S is what needs it, and what it should
  look like is a question about who is allowed to move a facet's revision, not
  about the borrow.
- **`openshard-movement`'s `lib.rs` is still thirty `pub use` lines.**
  [`style.md`](style.md) asks that a type be imported from the module that
  declares it; the crate's root re-exports its own private modules wholesale,
  which is how `Tile` and `Overlay` came to look like movement's types from the
  outside for as long as they did. It is not R2's to fix — R2 removed the five
  that were lying — but the same reading applies to the rest.

  > **Eight now, not thirty**, worn down by the nodes that came after — era P
  > moved the search's own types to the modules that declare them. The reading
  > still applies to the eight; the number in this entry does not.

### Backlog from R3, a house having floors

Found while giving `Cover::of_static` its platform arm and teaching `can_step`
to read the live layer's surfaces
([`realtime_map.md`](map/realtime_map.md)'s R3). None of them blocks R4 or R5.

- **~~`aboard` has no reach filter, and now it lets a house in.~~ ✅ Fixed.**
  Where the map refuses a tile outright, `walk.rs`'s `aboard` took the *nearest*
  live surface at any distance — a deck's rule, written when a deck was the only
  thing that could be one. A house built over open water lays surfaces on those
  tiles too, so a body on the shore could step onto whichever storey happened to
  be nearest its own z rather than onto the one it could climb to.

  **The question this entry asked is answered: one rule, two entrances.**
  `aboard` and `climbed` are reached according to whether the *map* had anything
  to say about the tile, so a climb limit on one and not the other made the
  reachability of a storey depend on whether there was water under it. There is
  one limit now, and `Overlay::surface_at` takes the reach as an argument — the
  caller's, because how far a body may climb is a *movement* rule and this is the
  same layering argument that keeps `SpanIndex` out of `openshard-map`.

  **And this entry's own objection to a reach filter does not hold.** It says the
  filter cannot be the fix because `aboard` exists for a body stepping *down*
  onto a deck from a mast. `Cover::reach` of a flat surface is its own height, so
  everything below the body passes the filter at any value: the climb is bounded
  and the descent is untouched. Asserted both ways in
  `boarding_from_open_water_obeys_the_climb_limit`, whose control is the limit
  removed by hand — it then fails at exactly the first assertion.

  **What it cost is two fixtures, and both were asserting a boarding the step
  rule does not permit.** `boats`'s deck stood three above its shore and
  `obstruct`'s five, and both passed only because `aboard` applied no limit; a
  walk climbs at most `MAX_STEP_UP`, which is two. Both now put the deck within a
  step, which leaves what those tests are *about* — the map refusing water, the
  deck overturning that, the hull refusing again — unchanged.

  **What is now visible, and is `boats.md`'s:** this shard has no plank. A UO
  player does not walk aboard over the gunwale — they step on the plank, whose
  `OnMoveOver` sets `from.Location` and teleports them
  (`ServUO/Scripts/Multis/Boats/Plank.cs:136`). So "can a body board a real sloop
  from a real shore" is a question about real deck heights that no test here
  answers, and the honest answer is that boarding is the plank's job and the
  plank is not built.
- **`standing_on` walks the map's start surface a second time.** `map.can_step`
  computes `start_surface(from)` internally and throws it away;
  `climbed` needs the same number to measure reach from, so on any tile with a
  live surface on it the walk happens twice. It is one static loop over one
  tile, and it only runs where the overlay has a surface at the destination —
  but the honest fix is for the map's step check to hand back what it already
  knew, which is a signature change `can_step`'s three callers would all see.
- **`Obstructions` is not obstructions any more.** It holds a house's floors,
  which are the opposite of an obstruction — `is_blocked` had to become
  `holds_anything` for exactly that reason. The type is the *identity* half of
  the overlay (who put this here), and that is what it should be called. Not
  renamed in R3 because the rename touches every server crate and none of R3's
  behaviour depends on it.
- **A house's placement checks got stricter, and nothing measured by how much.**
  `footprint_of` now returns an entry for every component that lays a cover, so
  the road test and the flat-ground test see a house's *interior* tiles for the
  first time — they only ever saw its walls. Both are ServUO's rules over the
  whole plot and both are more correct this way, but a plot that was legal
  before and is refused now would look to a player like a regression. Worth a
  pass over the shipped decoration data with a placement of each classic multi
  before anyone is told housing is finished.

### Backlog from R4, the statics becoming one run

Found while making a facet's statics one run with a per-block offset array
([`realtime_map.md`](map/realtime_map.md)'s R4). Nothing here blocks era P.

- **A patch of many ops is now quadratic in the facet.** `place_static` and
  `remove_static` move the tail of the run and every offset past it, where they
  used to move the tail of one block — which is right for the one op a published
  patch usually is, and wrong for a thousand. Nothing publishes at that size
  today; [direction F](map/new_map_representation/plan.md#f--the-editor)'s editor
  is what will, and the fix it wants is a publish that groups its ops by block
  and rebuilds each touched block once, rather than an op at a time. Worth
  measuring before designing: the whole run is 29.5 MiB, so an op is a ~30 MiB
  move, and the crossover with "just rebuild the facet" is not far away.
- **`WorldMap::from_parts`' grouping is a contract with no oracle.** It asserts
  that the counts are one per block and that they sum to the run's length —
  neither of which catches a caller that put the *right number* of items in the
  *wrong* block. That sorts them into the wrong span and every lookup after it is
  silently wrong, which is the failure mode this crate's block order has always
  had. Both callers are in-tree and both are tested end to end (the base-set
  round trip and the client-files import), so this is about the third one: a
  debug-only check that every item's coordinates fall in the block its count
  claims would cost one pass over the run at load.

### Backlog: the land's fourth byte is 29.4 MB of alignment

**Bigger than everything R4 saved, and nobody had written it down.** A
[`LandCell`](../crates/common/map/src/map.rs) is a `LandTileId` (`u16`) and a
`z` (`i8`) — three bytes of fields in four of storage. Felucca is 29,360,128
cells, so the land is **117.4 MiB of which 29.4 MB is the padding byte**; the
statics layer, after R4, is 29.5 MiB in total. The arithmetic is corroborated by
the measured peak of a facet load: 257 MiB is land 117.4 + statics 29.5 + the
file buffers it was read from, which only adds up with a four-byte cell.

**It is gated on the access staying cheap, and that gate is the point rather
than a caveat.** The land is read as a slice —
[`WorldMap::land_in_block`](../crates/common/map/src/map.rs) hands back
`&[LandCell]` and `land_in_row` steps one cell east at a time — and a three-byte
cell cannot be a slice of anything. Every read becomes an unaligned load and a
shift, on the path that draws every frame: the ground walk is the *one* part of
this map whose cache behaviour [`client_today.md`](map/new_map_representation/client_today.md)
measured as already good ("a block is 64 cells × 4 B = exactly four cache lines
… the 1997 tiling picked the cache line's size"). **If the unpack costs more
than the 25% of footprint it saves, the answer is no** — the size is worth
having only at unchanged read speed.

So what this finding asks for is a *measurement*, not a change: the ground walk
of a widest-zoom frame over a packed cell against the same walk over the cell we
have. The same gate governs the packed four-byte static record in
[R4](map/realtime_map.md#r4--statics-become-one-run), which until now was gated
only on whether the statics are still hot.

### ✅ A sector lookup was linear in a bucket, and a house made the bucket fat

`Sectors` (`state/src/sectors.rs`) was right where it was measured. Buckets are
64 tiles square, `located` maps an entity to **its bucket and its row in it**,
so insert, move and remove are all O(1) — the row half was already the lesson
learned from this exact case, and its own doc says so: "in a decorated town
that is thousands of entries, and finding an entity's own row in it by scanning
was paid on *every step by anyone*".

What was still linear was the read. `Sectors::nearby` walked **every entry** in
up to four buckets and filtered by Chebyshev distance. That is correct and was
cheap while a bucket held mobiles; a decorated house made it not cheap.
Housing's own caps say how not: `LOCKDOWNS_PER_TILE` is 4, so a castle's 992
tiles are worth about **4,000 locked-down items**, and at 64 tiles a side that
castle sits in one or two buckets. Every `nearby` touching it compared four
thousand rows — asked per NPC per tick by AI sight, and again by guards, pets,
chat, area spells, quest listeners, the broadcast audience, and, since "a mobile
is not an obstacle" closed, by `crowd_near` on **every step by anyone**. The
cost landed on the NPC that happened to share a sector with somebody's
decorated house, not on the house.

**Closed the way it says: a bucket is two lists, and the caller says which it
means.** `nearby` is gone as a name, which is the point — every call site had to
be revisited rather than keeping the old cost by inheritance.
[`mobiles_near`](../crates/server/state/src/sectors.rs), `items_near`,
`everything_near`, `mobiles_in_block`. Of the nineteen readers, **seventeen
wanted mobiles**, one wanted items (the crafting workshop scan) and one wanted
both (`refresh_around`, which fills a screen and so is about the furniture as
much as the people). **Six** of the seventeen also re-filtered by Chebyshev
distance after a lookup whose doc already promised exactness — chat, both
stealth sweeps, the bard's audience, a guard's call and the AI's sight; those
went with the rename.

The count this entry got wrong: it named `tick/fields.rs` as an item reader. A
field damages whoever *stands on it* and filtered its sweep by `Body` — it was a
mobile reader all along, and one of the several that had been paying for the
furniture twice, once to walk it and once to reject it.

**The kind is declared at the insert and never derived.** `Occupant::Mobile` /
`Occupant::Item`, named at each of the twenty-five places the shard puts
something on the grid, and seven more in tests. The alternative — reading `Body` off the registry inside the index — makes
the answer depend on whether the component went on before the index did, which
is a bug that only appears in whichever spawn path someone reorders later. The
cost of declaring it is the one thing no compiler catches, a caller naming the
wrong list, so `tick/tests.rs`'s
`the_shard_files_what_it_spawns_as_what_it_is` runs the real spawn paths — a
player entering, a creature spawned, an item and a container placed, a corpse
left by a death — and holds every row of both lists against the registry's
`Body`. Its controls: filing the corpse as a mobile fails it on the corpse
assertion; filing an entering player as an item fails **fifty** tests across
sight, chat, guards and the chase, which is the asymmetry to remember — a body
in the item list is invisible, an item in the mobile list is merely wasteful.

#### Found while closing it

- 🚩 **`FacetState::sectors` is public, and forty-five places across six crates
  write to it** — thirty-two inserts and thirteen removals. Its two neighbours in
  the same struct are private on an argument
  that applies here word for word: "every write here has to be followed by …, and
  a public field is a way to forget". The sector grid's forgettable half is
  `remove` — a despawn that misses it leaves a row pointing at an entity that no
  longer exists, which is the "ghost that never leaves" `despawn_mobile` already
  has a written-down order for, and nothing makes anyone follow it. A
  `WorldState::place(entity, facet, at, occupant)` / `unplace(entity)` pair would
  be the seam, and would give `Occupant` one place to be named per *kind of
  thing* rather than per call site. Not done here because it is a second
  refactor over the same sites and this one already had to touch them all; doing
  both at once would have hidden which change broke what.
- **`WorldState::move_to` files its traveller as a mobile, and its callers make
  that true rather than its signature.** Every one of the six is a body — a gate,
  a recall, a `.go`, a ship relocating who is standing on it. An item put through
  it would land in the mobile list and be invisible to the crafting scan, which
  is the one reader of the item list. The doc says so now; the seam above is what
  would make it structural.
- **`openshard_boats::aboard` sweeps a square around the ship's *first* covered
  tile.** The reach is the greatest Chebyshev distance from that tile to any
  other, so a galleon moored east-west sweeps a box as wide as it is long in both
  axes. It is mobiles-only now, which is most of the fix by accident; the shape
  is still wrong and the deck test would not notice.
- **One full-suite run reported a single failure with no name captured**, and
  three consecutive full runs since have been clean. Nothing to chase without the
  panic line — recorded so the next person who sees one knows it is not the
  first.

### The tick

`World::tick` is the deterministic half of the boundary the gateway's channel
draws. Commands queue from network tasks and are applied in a fixed order at a
fixed rate; nothing inside a tick awaits, reads a clock or touches a socket.

That is what makes anything that happens *without* a client asking possible at
all — decay, regeneration, an NPC deciding to move. It is also what makes replay
possible: the same commands produce the same world.

Two things worth knowing:

- **`select!` is `biased`** so the tick cannot be starved. Without it a flood of
  packets keeps `recv` ready forever and the world stops simulating under
  exactly the load that needs it most.
- **A late tick does not catch up.** `MissedTickBehavior::Delay`, because running
  several ticks back-to-back turns a hiccup into a stall and a fixed timestep
  into a variable one.

**What is still missing:** persistence. The world is built at start and lost
at stop.

Two players do now see each other. Verified over real TCP, on the real map:
each is drawn on the other's screen exactly once, steps arrive as `0x77`,
walking past 18 tiles sends `0x1D` and walking back re-draws, and a dropped
connection takes the mobile off every screen that had it.

## 4. Persistence

- [x] Persistence queue, drained outside the tick
- [x] SQLite backend — `SqliteStore`, tested
- [x] Save and load accounts and characters
- [x] Serial reservation on load — `Registry::reserve_serial`, for load-on-play
- [x] Crash recovery — the boot load restores the world; a played character
  returns on its saved serial and spot
- [x] PostgreSQL backend — `PgStore`, the same `Store` trait, tested against a
  live server
- [x] **Item persistence** — a character's carried inventory (worn gear, the
  backpack and everything nested inside it) and loose ground clutter survive a
  restart. `ItemRecord` is the saved shape; `SCHEMA_VERSION` moved to 2. An
  inventory is saved as a unit — the store replaces everything under an owner
  rather than diffing item by item, walked live for an online character and kept
  at logout like the character record; the ground is a full sweep, decoration
  excluded (a pack re-lays that). On boot the item serials are reserved and ground
  items placed; a returning character re-equips its saved inventory instead of a
  starter backpack. Items keep their serials across a restart so a container's
  contents still point at it.
- [x] **A save is complete, and shutdown flushes it.** Consistency, because it is
  gold and gear: every save writes *every online character* in full — record and
  whole inventory — not only the ones that moved, because picking an item up takes
  no step and so never marks a character dirty; the ground is swept every save, not
  only when someone was active; and a logout re-fills the in-memory
  pending-inventory cache so a **re-login in the same run** re-equips what it
  carried (before the fix it lost the backpack). And the shard **saves on the way
  out**: Ctrl-C, or the gateway stopping, takes one last full snapshot, closes the
  save channel and *awaits* the writer so every queued transaction lands before the
  process exits — unlike the per-tick handoff, because the one moment a lost write
  costs a player real value is the last one.
- [x] **`Stackable` persists, the save interval is a config line, and `.save`
  forces one.** An item's `Stackable` flag is saved (`ItemRecord`, schema v3), so a
  restored gold pile still merges with more rather than losing the flag until
  re-lifted. `persistence.save_seconds` sets the periodic cadence (0 = only shutdown
  and `.save`; a save never stops the world, so this is only how much a crash could
  cost). And a staff **`.save`** (GM+) takes an immediate snapshot and tells every
  player "the world is being saved" — the old shards' announce **without** their
  pause, because OpenShard's snapshot is an instant memcpy, not a synchronous walk
  of the world.
- [x] **Spawn regions persist, timers and all.** A populated area stays populated
  across a restart without re-running `.admin`, and — the point — a rare spawn keeps
  its remaining wait: killed with hours to go, it comes back with those hours ahead
  of it, not popping again the moment the shard is up. `SpawnerRecord` (schema v4)
  saves the region, its creatures and the timer as the **seconds still to wait**,
  not a tick count (which resets at boot) or a wall-clock time (the tick reads no
  clock) — so downtime pauses the timer rather than eating it, the semantics chosen
  for a rare spawn. Registering the *same* region twice lays one rather than
  stacking a second, and after a restart the regions come from the store rather
  than being re-laid, so a re-populate is not needed and the timers hold. "The same
  region" is the whole region — box, creatures, ceiling and pace
  (`Spawner::is_the_same_region`), not the box alone: Britannia's regions overlap,
  and matching on the box read 120 of the 1,430 shipped regions as re-registrations
  of the region already there and dropped them, which is how the forest north-east
  of Britain came to hold orcs and no skeletons.
### A spawn region's id is its slot — and now it is that by construction

`maintain_spawners` tags each creature `SpawnedBy(id)`; the tag is saved with the
creature and read back against a list a later boot rebuilt. So the only id that
survives that trip is one the *list* defines. It used to be a counter beside the
list (`World::next_spawner_id`, starting at one), and the tag was the creature's
region's **index** — two numberings that agreed only by luck, and by luck of a
particular kind: a world laid once from empty has `id == index + 1`, so the tag and
the ceiling it was counted against lined up all the way through a restart. Nothing
enforced either half. `clear_spawners` emptied the list without rewinding the
counter, and neither store's `spawners()` had an `ORDER BY id`. Either one drifting
re-points every live creature at a neighbouring region: one region permanently at
its ceiling and never spawning again, its neighbour over its ceiling, and no error
anywhere — the same silence the box-shaped de-duplication had.

The counter is gone. `register_spawner` gives a region the slot it is about to
take, `restore_spawners` gives it the slot it lands in rather than the number in
the row, and `spawner_records` writes that number out — `a_regions_id_is_its_slot_
however_the_list_was_built` walks all three paths plus a Clear. There is no
migration and none is needed: the tags on disk were always indices, which is what
the ids now are. Clear stays safe because it takes the creatures with the regions,
so no tag outlives the numbering it was written against.

What this rules out, and it is worth naming because nothing in the type system
does: **a region may not be removed on its own.** The list is laid whole or cleared
whole. A future "delete this one region" renumbers every region after it and
re-points their creatures — that feature needs a real id and the migration this
did not.

### The playground lays the shipped content when asked — `--seed`

`e2e/shard`'s `in_process::spawn` took no verbs and handed `run_shard` an empty
slice, so `openshard-playground` opened exactly what its database held and had no
way to lay anything else; the module doc pointed at a `--seed` that was the server
binary's. It takes one now (`OPENSHARD_SEED`, comma-separated, the same verbs), and
the in-process shard passes it through. This is the difference between a restart
and a re-populate: content that has grown since a world was laid — a fixed dataset,
a region the engine used to drop — arrives on a seed or on the staff menu's
Populate, never on a boot, because a boot restores and lays nothing new.

- [x] **The save is the whole world (schema v5), the Sphere/ServUO model.** Every
  live NPC mobile — townsfolk, vendors with their priced stock, spawner creatures
  with their current wounds and `SpawnedBy` link (`MobileRecord`) — and every
  placed decoration, door open/shut state included (`DecorationRecord`), is swept
  into each snapshot and restored at boot exactly as it stood. A killed creature is
  simply absent from the sweep and stays dead, its region's saved timer counting
  down; nothing re-populates at boot, so a staff `.admin` Populate/Decorate seeds a
  fresh world **once** and the save is the truth thereafter. Both references walk
  every mobile and item to save (ServUO's `World.Save`, Sphere's `CWorld::SaveStage`)
  and never regenerate the world — this reaches the same end without stopping the
  world to do it. A ridden mount in limbo is the one mobile not swept: its ride
  persists through the saddle item on the rider, and `dismount` reconstitutes the
  creature whole.
- [x] **Stats and trained skills persist (schema v6).** A `CharacterRecord` carries
  str/dex/int and every trained skill with its lock arrow; character creation finally
  *applies* the stats and skills the player picked, threaded through
  `Command::Enter` as a `CharacterSheet` — for a new character from the create packet
  and for a played one from the save. The `0x3A` skills window follows a live gain.
- [x] **Regions and the world clock persist (schema v12).** A facet's named areas
  (`RegionRecord`) and the hour of the day ride in the same snapshot sweep as
  decoration and spawners. Both are things a player never changes and a restart
  would silently lose: no guards, no town music, daylight in every dungeon, and
  every night starting over at boot. The clock cannot ride the tick counter,
  which resets to zero by design — every restored timer is an offset from it.
- [x] **Active effects persist (schema v7).** Poison and the timed stat buffs are
  saved with their mobile as an `EffectRecord` list on the character or mobile row,
  so a relog cannot wash a debuff off — see the `magic` effects work in §6 for the
  shape (`World::effects_of`/`apply_effects`, the ledger-only restore for buffs).
- [x] **A container's trap persists (schema v19).** A restart that quietly disarms
  every chest on the shard is the same class of silent loss as one that forgets a
  lock — and the disarm is a skill somebody spent points on.
- [x] **The poison on an item persists (schema v18).** A bottled dose or the
  coating the Poisoning skill put on a blade. The same lesson as the spellbook mask:
  all four poison potions are one graphic, so an unsaved bottle comes back empty and
  a blade somebody spent a potion on comes back clean.
- [x] **A corpse's story persists (schema v17).** Who it was, who killed it, who
  has read it with Forensic Evaluation and who has rifled it, as one nullable JSON
  column on the item row. A corpse lies for seven minutes and a shard restarts
  inside that window, so without it the body a player was investigating comes back
  anonymous, killed by nobody and disturbed by no one. See the Forensics entry in
  §6 `skills`.

Two backends, one choice. A shard runs on SQLite or on PostgreSQL, and which is
the operator's to make: neither is "the production one", and SQLite runs a real
shard perfectly well. Some will want a text file or a Postgres cluster; the
`Store` trait is the seam that lets any of them sit behind the same simulation.

`persistence.database` picks the backend by what it looks like: a `postgres://`
URL connects to PostgreSQL, anything else is a SQLite file path, and empty keeps
the world in memory — the same bargain as running with no map, and the shard says
so. A logged-out character lives as a row, not an entity: its serial is reserved
at boot so nothing new can take it, and playing it (`0x5D`) spawns it back on that
serial, at its saved position, looking as it did. Characters save as they change
and on logout, through the same journal the tick already feeds.

**Three things it is worth knowing before touching this:**

- **The dirty marks come from the event bus.** Nothing calls `journal.touch()`
  by hand. A system that moves a mobile already emits `MobileMoved`, because
  that is how the client hears about it; persistence reads the same event. There
  is no line to forget.
- **Logout uses `Journal::keep`, not `touch`.** A touch is a promise to read the
  entity at the next save, and the entity is about to be despawned. Logout is
  when a save matters most, so the record is taken before the despawn. There is
  a test with that name.
- **A failed write costs a full sweep, not a rollback.** Re-writing the failed
  snapshot would put everyone back where they were when the write started. The
  world is marked dirty instead and the next save reads it fresh.

**Two things specific to the PostgreSQL backend:**

- **It connects with `NoTls`.** Enough for a database on the same host or a
  trusted network, which is where a first backend earns its keep. An encryptor is
  a later, additive change and does not touch the shape — `PgStore` is one
  connection behind an async mutex, the same shape as SQLite's, because a
  transaction borrows the client and saves are off the tick either way.
- **`tokio-postgres` used to be pinned, and no longer is.** From 0.7.13 it pulls
  a crypto stack (RustCrypto 0.11, `rand` 0.10) that wanted Rust 1.85 — above the
  1.82 MSRV of the time — so the lock held it at 0.7.12. The scripting spike (§5)
  raised the MSRV to 1.88, which cleared the constraint, and the pin was dropped;
  the crate floats on `"0.7"` again. See the `Cargo.lock` note in
  [`development.md`](development.md).

## 5. Scripting — spiked, proven, and deleted

It was the largest open technical risk, and it was answered twice: first that an
embedded V8 *works*, then that this project does not want one.

Issues [#7](https://github.com/youhide/OpenShard/issues/7) and
[#17](https://github.com/youhide/OpenShard/issues/17) settled on pure Rust.
Everything the pack held is in the tree —
`crates/server/state/data/{quests,speech,regions}.json` and
`crates/server/world/data/{spawns,deco,townsfolk,loot}.json`, laid by
`server::content` (the first two at boot, the rest on their admin verbs), plus
the two item behaviours as `world::tick::shipped_items` — and each dataset moved
under a test that compared its `Command`s against the pack's. When the last one
agreed, `crates/server/scripting`, the bridge beside it, `deno_core` and the
`[scripting]` config section were deleted, and `cargo test --workspace` began
running the whole workspace with nothing excluded.

Removing `deno_core` did **not** lower the MSRV. It was measured rather than
assumed: the highest demand in the tree is now `wesl` 0.4.2 at 1.96, a
build-dependency of the client's renderer, so the declared 1.88 had already
stopped being true. See [`development.md`](development.md) § What holds the MSRV.

The checklist below is what the spike delivered, kept as the record of what was
built and thrown away; the decision is in
[`architecture.md`](architecture.md) § Scripting.

- [x] `deno_core` embedded, one V8 isolate — `DenoEngine`, one `JsRuntime`
- [x] `ScriptEngine` trait — four methods, nothing V8-shaped in a signature, so
  the runtime stays replaceable
- [x] Entity and event bindings exposed to TypeScript — domain events in through
  `deliver`, a read model a hook reads through `op_position`, commands out
  through `op_move`; ops declared with `extension!` and `#[op2]`, all synchronous
- [x] Hot reload without a restart — `load` rebinds the hooks in the live
  isolate; `reload_if_changed` polls a watched file's mtime
- [x] **Benchmark** — `examples/benchmark.rs`, numbers below

### The numbers

The question was whether a per-entity hook fits the tick. The budget is
`TICK_INTERVAL`: **50ms at 20Hz**. Measured on an Apple-silicon dev machine, V8
hosted in a Tokio runtime, release build, warmed up so the JIT has tiered the
hook. `cargo run -p openshard-scripting --example benchmark --release`.

| Hook | per call | 10k mobiles/tick | share of a 50ms tick |
|---|---|---|---|
| empty (`onTick(){}`) — pure Rust↔V8 crossing | ~170 ns | ~1.7 ms | ~3% |
| read + maybe move — `op_position`, then conditionally `op_move` | ~490 ns | ~4.9 ms | ~10% |

The realistic hook — the one a gameplay rule looks like: read the mobile's tile
through an op, decide, and on a condition enqueue a step — costs about half a
microsecond a call. Ten thousand mobiles each firing it every tick spend roughly
a tenth of the budget. **It fits, with room.**

Two honest caveats. The ceiling is *script* time only; a real tick also moves
mobiles, runs interest management and writes packets, so the script share is a
slice of the 50ms, not all of it — the per-call nanoseconds are the number that
travels, not the "calls per tick" ceiling. And the crossing cost is per call, so
a design that calls one hook over a batch of entities will always beat one that
crosses per entity; that is a knob for §6, not a problem for the spike.

The design does not have to change. Gameplay can depend on it.

## 6. Gameplay

Roughly in dependency order, each script-first:

- [x] **The script is wired into the tick.** The bridge §5 deferred: the server
  owns a `DenoEngine`, delivers each tick's domain events to it, and queues the
  commands it emits for the next tick. `scripting.main` in the config names the
  script; empty runs scriptless, the same bargain as an empty map. A script acts
  through `Command::Step` — server-authoritative movement, no client sequence or
  pace, terrain the only judge — which is the first thing a script command lands
  on. `crates/server/server/src/scripting.rs` is the whole seam.
- [x] `items` — containers, stacking, equipment layers, decay
  - [x] **On the ground and visible.** A script drops an item
    (`op_spawn_item` → `Command::SpawnItem`) and every client in range is sent
    the `0x1A` that draws it; walking up to one draws it, walking away sends the
    `0x1D`, exactly as for a mobile. Items are entities like anything else — a
    `Graphic` and a `Position`, drawn through the same `seen`/interest machinery
    as bodies. A stack carries an `Amount`. The `WorldItem` (`0x1A`) encoder is
    ported from Sphere's `PacketItemWorld`, flag bits and all.
  - [x] **Pick up and drop** (`0x07`/`0x08`). The client's own item loop: lift
    an item onto the cursor and set it back on the ground. The world holds it in
    limbo — off the sector grid, off every screen but the picker's — and
    remembers where it came from, so a drop out of reach or a logout mid-drag
    bounces it back rather than losing it. A refused lift or drop is a `0x27`
    drag-cancel with a reason. Server-authoritative reach (`ITEM_REACH`), no
    trust in the client's claim. Ground-to-ground only; dropping *into* a
    container is the next slice, and it bounces for now.
  - [x] **Containers** (`0x06` open, `0x24`/`0x3C`/`0x25`). A container is an
    item that also carries a `Container` (its gump); items inside carry a
    `Contained` and no `Position` — the two are exclusive, on the ground *or* in
    a container, never both. Double-click opens it (`0x24` + the `0x3C` contents
    list); dropping onto its serial puts the item inside (`Contained` + a `0x25`
    to the open gump); lifting a contained item drops the containment. A drop
    onto a non-container, or out of reach, bounces to origin — and origin is now
    "the ground *or* the container it was in", so a cancelled drag always undoes
    cleanly. Live updates go to the acting client only; a second viewer re-opens
    to refresh (a noted limitation, not a bug). The `0x24`/`0x25`/`0x3C` version
    seams (High Seas type word, `ItemGrid` grid byte) are gated on `Feature`, not
    era.
  - [x] **Equipment layers** (`0x13` wear, `0x2E` equipped). A worn item carries
    an `Equipped { mobile, layer }` and no `Position`/`Contained` — the third and
    last place an item can be, all three exclusive. Dragging an item onto a
    paperdoll (`0x13`) wears it: the layer is checked free, the wearer reachable,
    and a `0x2E` goes to everyone who can see the mobile. A newcomer sees a
    dressed mobile because the `0x78` now lists what it wears (it sent an empty
    list before). Lifting a worn item takes it off. A held item's origin is now
    "ground, container, *or* mobile", so every cancelled drag still undoes to
    exactly where it came from.
  - [x] **Stacking, split and decay.** A `Stackable` item merges with an
    identical pile (same graphic and hue) dropped onto it — amounts sum, clamped,
    the dragged one despawns, the survivor is redrawn past the `seen` set.
    Picking up part of a pile splits it: the `0x07` amount is honoured, and —
    read out of Sphere's `CItem::UnStackSplit` rather than guessed — the original
    keeps its serial and holds the taken amount on the cursor while a new dupe is
    left on the ground with the remainder, so the client's cursor and its drop
    still name the same object. Ground items carry a `Decays { at_tick }` and rot
    when the tick counter reaches it; lifting, containing or wearing takes the
    clock off, and `decay()` reads only its own counter, no wall clock.
    Containers do not decay with their contents inside.
  - [x] **Stack merge inside a container.** `merge_onto` (`items/stack.rs`) no
    longer bounces on a target with no `Position`: it branches on where the target
    lives, and a `Contained` target is reach-checked through its container
    (`container_in_reach`, the same gate `drop_into_container` uses), the amounts
    summed as on the ground path, and every open gump told the new total with a
    `0x25` (`tell_watchers_updated`, mirroring `give`). The drop already routed
    here — `drop_onto_item`'s `can_stack` arm fires regardless of location.
  - [x] **A pile has a ceiling, and nothing falls off it.** Both merge paths used
    a `saturating_add` on the `u16` an `Amount` is stored in, so dropping 50,000
    gold onto 50,000 left one pile of 65,535 and destroyed the other 34,465 — the
    engine's first item-loss bug, found in play. The cap is now an explicit
    `items::MAX_STACK` (60,000, ServUO's `Item.WillStack` number, kept clear of the
    `u16` edge) and the overflow goes back to the player, not to nowhere: Sphere's
    `CItem::Stack` fills the destination to its maximum and leaves the remainder on
    the source, which is the kinder of the two references (ServUO refuses the merge
    outright). A drag whose remainder will not fit bounces it home. Where the
    *world* hands goods over, `items::give` spreads a payout across as many piles
    as it needs — a container ends up with two gold piles, as in UO — and takes a
    `u32` now, because a large sale earns more than one pile holds and the old
    `u16` made the vendor clamp the payout to 65,535 and say nothing.
  - [x] **Partial lift honours the amount everywhere.** `pick_up`'s container
    branch (`items/drag.rs`) reads the `0x07` amount now: a partial lift of a
    `Stackable` contained pile leaves the remainder behind *in the same grid slot*
    as a new dupe (`items::spawn_contained_leftover`, the container sibling of
    `spawn_leftover`) and lifts the original reduced to what was taken — Sphere's
    `UnStackSplit`, the original keeping its serial for the cursor, the remainder a
    new serial drawn into the open gump. A whole lift is unchanged.
  - [x] **The item-trigger seam (Sphere's `@DClick`).** The engine handles the
    double-clicks it knows — door, container, spellbook, mount, mobile — and hands
    every *other* item to the pack as an `ItemUsed { item, graphic, by }` event
    (defined in `items`, re-exported by `world`, delivered to scripts like every
    domain event), with reach already checked server-side (`container_in_reach`).
    The engine keeps *no* default behaviour for a bare item: the meaning lives in
    the pack, which registers a handler per graphic and answers with ops. This is
    the "default in core, customise in the pack" split — except the core default
    here is nothing, because a graphic has no behaviour until a shard gives it one.
    The Community Pack ships a readable book as the example.
  - [x] **A consume op for one-shot items.** `op_consume_item` (→
    `Command::ConsumeItem` → `items::consume`) removes an item wherever it lives,
    behind one op with the three location-specific client updates: on the ground
    the decay path (off the sector grid, a `0x1D` to every screen, shared with
    `decay` through `remove_ground_item`); in a container the reagent-burn path (a
    `0x1D` to whoever has the gump open, `tell_watchers_removed`); worn it forgets
    the item on the wearer *and* every onlooker (`broadcast_unequip`, the mirror of
    `broadcast_equip` — no "remove from paperdoll" packet exists, so the client
    drops it by serial, and unlike a lift the wearer's own client is told too).
    `amount` 0 removes the whole item; a smaller amount decrements a stackable pile
    (one potion out of a lot) via `remove_from_stack`. Consuming a container
    cascades into its contents (`despawn_contents`, shared with decay), and a stray
    serial removes nothing — the `add_loot` guard. The Community Pack's `items.js`
    ships a heal potion: `op_heal` the drinker, then `op_consume_item(e.item, 1)`.
- [x] `combat` — swing timers, damage, resistances, notoriety
  - [x] **Hit points, damage and death.** Mobiles carry `Hitpoints`; scripts
    spawn creatures (`op_spawn_mobile` → `Command::SpawnMobile`, an entity with a
    body and no client, drawn through the same interest machinery as a player)
    and damage them (`op_damage` → `Command::Damage`). A blow lowers hits and
    redraws the `0xA1` bar — the mobile itself sees the real numbers, everyone
    else a percentage, so a stranger's exact health never crosses the wire. At
    zero it emits `MobileDied`, which the server delivers to scripts, so loot,
    notoriety and quests hang off death without combat knowing they exist — the
    "systems emit, they do not call" rule made concrete. A creature is removed on
    death; a player stays (ghosts and corpses are a later slice).
  - [x] **The interactive layer.** A player toggles war mode (`0x72`, echoed
    back settled) and picks a target (`0x05` → `0xAA`); a `Combat` component
    holds the stance, the target and the next-swing tick. `swings()` runs each
    tick: a combatant in war mode with a target within `MELEE_RANGE` on the same
    facet strikes when its timer is up, out of reach it waits with its timer
    unspent, and a killed target ends the attack. The timer is a tick count, like
    decay — no clock in the tick. A `SwingSpeed` component sets the cadence per
    mobile as an explicit override, but with no override the pace is now *derived*
    from the wielder's dexterity through Sphere's pre-AoS formula
    (`CResourceCalc.cpp`, era 1): swing tenths = `(15000 · 10) / ((dex + 100) ·
    base)`, wrestling base 50, so a `dex 100` mobile swings every 1.5s and a
    nimbler one sooner. The weapon `base` is now the wielded weapon's, not always
    wrestling — see **Weapon properties** below.
  - [x] **Resistances and the damage formula.** A swing's damage is no longer
    flat: `melee_blow` takes the attacker's `MeleeDamage` and cuts it by the
    target's `Resistance { physical }`. Both are components a script sets when it
    spawns a mobile (`op_spawn_mobile` grew `damage` and `resistance`), so a
    hard-hitting ogre or an armoured knight is a data change, not a code one — the
    script-first part. Melee is physical; the other damage types arrived with
    `magic`.
  - [x] **Notoriety and criminal flagging.** Mobiles carry a `Notoriety` (the
    enum already in the protocol), drawn as the health-bar colour in every
    `0x78`/`0x77` — the world stopped hardcoding "innocent". A script sets it at
    spawn; an invulnerable (yellow) mobile cannot be attacked. Raising a hand
    against someone blue or green turns the attacker grey — a `CriminalUntil`
    flag, its expiry a tick count like decay, broadcast to every watcher with a
    `0x77`. **And murderer flagging is real** — the red a repeat killer earns. A
    `Murders` count tallies innocents killed (attributed in `swings`, where the
    killer and the blue victim are both known); the fifth turns the killer red for
    good. Unlike the lapsing grey flag it is persistent, so `expire_criminality`
    now restores a mobile's *base* standing — murderer if the tally stands, else
    innocent — rather than always washing it blue. Attribution is *not*
    melee-only: `damage` takes an `attacker`, and a script's `op_damage`/spell
    carries a `by` serial, so a fireball that kills a blue is a murder the same as
    a sword; unattributed damage kills without blame. And old kills fade — a
    `MurderDecay` clock ages one count off at a time, washing a reformed killer
    back to blue once it drops below the threshold. (Sphere's separate short- and
    long-term counts are a finer model this stands in for.)
  - The **typed damage** this once deferred landed with `magic` below: `damage`
    takes a `DamageType` (physical, fire, cold, poison, energy) and cuts it by the
    target's `Resistance` for that type, in the one place all damage passes — melee,
    spell, poison pulse and script alike.
  - [x] **Weapon properties — swing speed and damage from the wielded weapon.**
    The weapon a mobile holds now drives its swing pace and its damage roll, so a
    katana strikes faster than a maul and a longsword hits harder than a dagger.
    **Not from tiledata**, despite the old heading: weapon speed/damage genuinely
    are *not* in `tiledata.mul` (the reader drops the layer/quality byte, and the
    numbers were never there) — in classic UO they are per-weapon-*class* constants.
    So they live in a **core table keyed by graphic** (`combat::weapons`, ~40
    classic weapons ported from ServUO's `BaseWeapon` subclasses, both the pre-AoS
    `Old*` and the AoS `Aos*` sets, `by_era` picking between them), the same
    "data keyed by graphic, default in core" shape as `creature_name`. The seam was
    already right and stayed put: `swing_speed`/`melee_blow` are **read-site
    derivations**, recomputed fresh each swing, so they consult the item on the
    wielder's weapon layer (`equipped_weapon`) with no mirror stamped on the mobile
    — a weapon coming *off* reverts to wrestling with nothing to undo, none of the
    per-mutation bookkeeping the persistence rule warns against. Precedence is
    **explicit override → weapon → default**: a creature's `MeleeDamage`/`SwingSpeed`
    (its natural blow, a script's pin) still wins, a player (who no longer carries a
    fixed `MeleeDamage`) derives from the weapon, and bare hands stay wrestling and
    `SWING_DAMAGE`. The damage roll uses the world's seeded `rng`, so a fight
    replays.
  - [x] **Hit chance, skill gain, damage scaling, and a pack override.** The follow-ups
    the weapon table set up, all ServUO-faithful:
    - **To-hit.** A swing now rolls to land — pre-AoS `CheckHit`, `chance = (atk + 50)
      / ((def + 50)·2)`, the two mobiles' weapon-skill standings (the defender's own
      weapon skill, Wrestling unarmed, its guard). A miss whistles past (`MELEE_MISS_SOUND`,
      the swing still animates) and does no damage; the timer resets either way. The
      roll **is** a `CheckSkill`, so the same call trains the weapon skill — a new
      `skills::roll_skill_chance` (ServUO's `CheckSkill(skill, chance)`) shares the
      gain half with `roll_skill_band`, and a player's Swords/Archery/… creeps up with use,
      surfacing in the `0x3A` window with no extra wiring.
    - **Damage scaling.** A landed blow scales by the attacker's Tactics, Strength and
      Anatomy — ServUO's `ScaleDamageOld` (era 1: Tactics its own ±50% about parity,
      then Str 1%/5 and Anatomy 1%/5 +10% at GM) and `ScaleDamageAOS` (era 2 the
      `GetBonus` coefficients).
    - **Gated on a `Skills` sheet.** Applying these to a mobile with no skills (an
      untrained player) would make it miss half the time and deal half damage. So both
      the to-hit roll and the scaling engage only when the *attacker* carries a
      `Skills` component; without one it keeps the pre-feature certainty — its natural
      blow always lands, unscaled. A clean, forward-compatible boundary: the moment a
      creature is given skills it starts rolling.
    - **Archery** rolls the wielded bow's damage band now (through `scaled_blow`, the
      same path as melee), not the flat default.
    - **Pack per-item override** — a `Weapon { speed, min, max }` component (set by
      `op_set_weapon` → `Command::SetWeapon` → `items::set_weapon`) on a weapon item
      replaces the core table's stats for a magic sword, `equipped_weapon` reading it
      first while keeping the graphic's skill; era-independent.
  - [x] **The rest of the combat deferrals landed too.**
    - **Pre-AoS PvE damage-halving.** ServUO's `ComputeDamage`: outside AoS, full
      damage lands only when a player strikes a non-player — every other pairing (a
      monster's blow, PvP) is halved. In `scaled_blow`, past the skill gate, keyed on
      a `Client` component, era `< 2` only.
    - **Per-weapon miss sounds and the Axe/Lumberjacking bonus.** The weapon table
      grew `miss_sound` (ServUO's `DefMissSound`, resolved through the base classes)
      and an `is_axe` flag; a whiff plays the wielded weapon's own swish, and an axe
      in a lumberjack's hands hits harder (era 1 capped 20%, era 2 the AoS `GetBonus`).
    - **Creatures carry combat skills.** The spawn path — `op_spawn_mobile` /
      `Command::SpawnMobile` / the spawner's `CreatureTemplate` — grew a `skills` list
      that `npc::spawn` turns into a `Skills` sheet, and it **persists** (a `skills`
      field on `MobileRecord` and the spawner's `CreatureData`, both JSON records, so
      no schema bump). A monster given Wrestling/Tactics rolls to hit and scales
      damage exactly as a player does. The remaining half is data: the converter
      scraping ServUO's `SetSkill` per creature, a Community-Pack follow-up — the
      engine is ready for it.
  - [x] **Combat eras 3–5** — Sphere's `m_iCombatSpeedEra` `0` (custom), `3` (SE) and
    `4` (ML) join the implemented `1`/`2`, ported from `CResourceCalc.cpp` into
    `swing_ticks`: SE `scale/((dex+100)·aos_speed) - 2`, ML `ml_speed·4 - dex/30` (a
    third speed column on the weapon table, from ServUO's `MlSpeed`), era 0 pre-AoS
    with a 0.5s floor. `by_era` and the damage scaling follow the family split (0/1
    pre-AoS, 2/3/4 AoS), and `config` accepts `0..=4`. Set `speed_scale_factor` to
    match (15000 pre-AoS, 40000 AoS, 80000 SE; ML ignores it).
  - [x] **Creature corpses and loot.** A slain creature no longer vanishes: the
    tick's `reap` (reading `MobileDied` — combat emits the death, the world
    disposes of the body) lays a corpse where it fell — item `0x2006` with
    `CorpseBody = body` (the protocol special case that draws the right corpse), a
    container on gump `0x0009` holding the creature's worn gear and a core gold
    drop scaled from its toughness. It decays after seven minutes and takes its
    loot down with it (`items::decay` now cascades into a container's contents, so
    nothing is orphaned). `combat::die` stopped despawning — it announces, `reap`
    disposes. The corpse persists as a ground container; a restored one gets a
    fresh decay timer (the tick is not saved).
  - [x] **A corpse lies the way it fell.** The corpse's picture is a pair — which
    body, and facing where — so `CorpseBody` carries both and `0x1A` sends the
    facing in its direction/light byte (announced by the top bit of `x`, written
    between `y` and `z`; see `docs/findings.md`). Before this the client drew every
    corpse southeast: the death *animation* was right, because it is the mobile's
    own, and the body then turned as it settled and again on every later fold of
    the world. The facing is the heading the mobile died with, and it is saved
    beside the corpse's story rather than in a column of its own — the item row's
    `amount` already carries the body — so a corpse restored from a save written
    before this comes back lying north.
  - [x] **The shard says which corpse a body became (`0xAF`).** The premise of
    the entry this replaces — that the wire has no field pairing a corpse with the
    mobile it was — was wrong: `0xAF` is exactly that packet, thirteen bytes of
    killed serial, corpse serial and a run flag, and it is what ClassicUO's
    `CorpseManager` is built on. `WorldState::announce_death` sends it to everyone
    watching except the dying player's own client (ServUO excludes it too — that
    client has `0x2C` and a ghost, not a corpse to pair), and `Crowd::died` lifts
    the falling body out of the crowd and holds it under the corpse's serial for
    `Crowd::corpse` to finish. The tile-and-body search is gone, and with it the
    case where two of the same creature dying together swapped falls. Holding the
    fall by serial also means the removal and the corpse no longer have to arrive
    in one batch for the hand-off to work.
  - [x] **`0x1A`'s light and flags bytes are read instead of refused.** Both used
    to make the decoder reject the packet, which lost the whole item to save a
    hint — and the flags byte is not rare: ServUO sets `0x20` on everything a
    player may pick up, so the rule refused most of a real shard's ground.
    `WorldItem` now carries `light: Option<LightId>` and `flags: ItemFlags`; this
    shard sends neither (an item's light comes from its graphic's tiledata, and
    what may be lifted is decided when the player tries), so they exist to keep a
    foreign shard's item readable and to be there the day `light.mul` is.
  - [x] **Ghosts and resurrection.** A player who dies no longer stands at zero
    hits: `reap` now lays a **player corpse** holding their worn armour (the
    backpack and bank box stay on them — worn containers, not loot) and puts them
    into a **ghost state**. The ghost wears the grey ghost body (`0x0192` male /
    `0x0193` female, ServUO's `Race.GhostBody`) and a death shroud (`0x204E`), and
    the client is told it is dead (`0x2C`), which greys the world and gives the
    gliding ghost walk. **The living cannot see the dead:** the interest gate
    `WorldState::can_see_mobile` draws a ghost only to another ghost or to staff —
    ServUO's `CanSee(Mobile)` clause — so a living watcher is told to forget it
    (`0x1D`) the moment it dies. The AI already ignores it (a 0-hit mobile is
    neither acquired nor pursued). **Resurrection** lifts the `Ghost` marker,
    restores the living body it remembered, strips the shroud, and hands back a
    tenth of the hit points; the corpse stays where it fell to be walked back to
    and looted. Two paths reach it: the **Resurrection spell** (moved out of
    `Scripted` into a core `SpellEffect::Resurrect`, the effect the table was
    "waiting on") and a staff `.res`. **It persists (schema v9):** a `dead` flag
    rides the character row while the `body`/`hue` stay the *living* ones, so a
    ghost that logs out logs back a ghost — the grey body re-derived, the corpse
    already a saved ground item, no duplicate laid.
  - [x] **Pack loot tables.** The corpse gold stays a flat core baseline (so a
    bare shard still loots); the real per-creature loot is the pack's, off a
    `CorpseCreated` event `reap` emits when a creature's corpse is laid — carrying
    the corpse serial and the body, the pack's loot-table key. A script fills the
    corpse by serial through a new `op_add_loot` (→ `Command::AddLoot` →
    `items::give` for a stackable, `items::place_one` for a discrete piece),
    guarded so a stray serial adds nothing. The "default in core, customise in the
    pack" split, same as spell and skill effects. The Community Pack ships
    `loot.js`: a `Pack.loot[body]` table of `{ graphic, amount, stackable, chance }`
    drops (an orc, a spectre), rolled in `index.js` — pack loot may use
    `Math.random`, since determinism is the core's seeded rng and a script is an
    external input to it.
  - [x] **Stamina as a real pool.** The status bar sent `stamina = dexterity`
    outright, a placeholder that only existed so the client would run at all; now a
    real `Stamina { current, max }` component (`state`, the sibling of `Mana` and
    `Hitpoints`) carries the pool, `max` = dexterity — the UO identity where the
    bar *is* the stat — full at enter, and `send_status` reads it. A dexterity
    change re-caps it the way strength re-caps hit points, both from
    `skills::set_stats` and from an Agility/Clumsy buff (`magic::shift_stats`, whose
    "dexterity's stamina pool has no component yet" comment this retires).
    `combat::regen_stamina` trickles it back from the tick like `magic::regen_mana`,
    a touch faster (`STAMINA_REGEN_TICKS`). The first real consumer — the
    overweight drain — landed with the status-bar slice below; the war-mode
    push-through cost is still a follow-up.
  - [x] **Hit points regenerate.** Mana and stamina trickled back and hits never
    did, so a wounded character could only ever be mended by someone else — a gap
    nothing in the roadmap had recorded. `combat::regen_hits` is the third of the
    trio, the same tick-counter shape: a point every eleven seconds (ServUO's
    pre-AoS `Mobile.DefaultHitsRate`), and none at all for the dead or the
    poisoned, which is literally ServUO's `CanRegenHits` (`Alive && !Poisoned`).
  - [x] **Armour, worn and felt.** Armour rating is not in `tiledata.mul` either —
    the same finding the weapon table recorded — so it is a core table keyed by
    graphic (`combat::armor`, the classic leather/studded/ring/chain/plate/bone
    suits, helms and shields from ServUO's `BaseArmor` subclasses), with an
    `Armor { rating }` component the pack can lay over one item. Where a piece
    sits comes from the layer it is *worn* on, not from the table, because the
    wearer already carries that fact — ServUO derives its own `BodyPosition` the
    same way. `worn_armor_rating` is the read-site derivation (each piece scaled by
    how much of a body it covers, ServUO's `ArmorScalars`), shown on the status bar
    and spent in combat: pre-AoS a swing gives up a share of it through
    `absorb_physical`, ServUO's `BaseWeapon.AbsorbDamage` — a rolled hit location,
    that piece and any shield eating their own bite, then a cut of the wearer's
    total. Rolled on the seeded `rng`, so a fight still replays; a mobile wearing
    nothing rates zero, which is why no existing combat test moved. One tidy-up is
    recorded in the code: ServUO's two ladders disagree by a swap (its
    piece-selection gives the arms the 14%-wide band while its scalar array gives
    the helm 0.14), and this port uses the scalars array for both.
  - [x] **The status bar stops lying.** Four of its numbers were constants — gold
    `0`, armour `0`, weight a flat body weight, followers `0` — read by a player
    every session with no way to check them. They are read-site derivations now,
    the shape `equipped_weapon` established: gold and weight from `items::carried`
    walking the worn tree (a pouch inside the pack counts), armour from the table
    above, followers from whether a mount is under the rider. Gold weighs ServUO's
    `0.02` stones a coin, not the tile's weight, or a bank run would pin a
    character to the floor. Two edges are ServUO's `Mobile.UpdateTotals` exactly,
    and both were wrong in the first cut: **the bank box is not carried** (it is
    `IsVirtualItem`, so neither its weight nor its gold reaches its owner — which
    is why the banker has to *tell* you your balance), and **a held item is** (it
    adds `m_Holding` explicitly, so lifting the anvil onto the cursor is not how
    you carry it home). The re-send is a **diffing pass** (`tick/status.rs`,
    twice a second over the online players) rather than a call beside every item
    mutation — the pattern that decays, the same argument the persistence rule
    makes. And the female flag now follows the body rather than calling every
    character male.
  - [x] **A staff account can put the staff down (`.gm`).** Sphere's split, which
    the fatigue rule above made necessary: its `PLEVEL` says who may command and
    its `PRIV_GM` flag says who is currently exempt from the game's rules, and
    `.GM` toggles the second without touching the first
    (`Source-X/src/game/clients/CClient.cpp:836`). Here that is `Access` (the
    account's authority, re-derived each login) and a new `Staff` marker (the
    mode, given at login to a game master and taken off by `.gm`).
    `WorldState::is_staff` reads the marker and is the one choke point every
    exemption already went through, so the whole behaviour change is two rules:
    with the mode off a game master **tires under its load** and **cannot see or
    hear the dead**. The command gate stays on `staff_authority` — the `.`-prefix
    split and the admin gump's re-check both — or `.gm off` would lock a game
    master out of `.gm on`. The screen is rebuilt on the spot (`refresh_around`,
    the call death and resurrection make), so ghosts appear or are forgotten as
    the mode flips. Without this there is no way to *test* a player-facing rule
    from the only kind of account that can set one up.
  - [x] **The bank is a wallet, on two `[gameplay]` flags.** Taking the bank box
    out of what a character carries (see above) left banked gold buying nothing.
    ServUO does the other half: `BaseVendor` tries the pack, then the bank, and
    says which paid. So `vendor_bank_payment` (default **on**, UO and ServUO) lets
    a purchase fall back to the bank whole — never split across the two, the
    reference's rule and the honest one — and `bank_gold_in_status` (default
    **off**, ServUO's truth) decides whether the status bar's gold adds the box.
    Weight is on neither switch: banked goods are never carried, or banking a pile
    would make you overweight. One rule counts the coins for all three readers
    (`items::banked_gold` — the banker's "balance", the bar, the vendor), and it
    walks the box's whole tree, so a purse *inside* the bank counts where the old
    one-level scan missed it. `take_from_container` widened to `u32` with it: a
    bank purchase runs past what one `u16` stack can hold, and the old cap refused
    it with "thou canst not afford that", which was a lie.
  - [x] **Carrying too much costs something.** ServUO's
    `WeightOverloading.EventSink_Movement`, ported into `combat::spend_step_stamina`
    and consulted by the player walk: over the cap (plus a four-stone allowance) a
    step costs `5 + over/25` stamina — a third of that mounted, double at a run —
    a pool under a tenth full costs an extra point, and every sixteenth step on
    foot (forty-eighth mounted) costs one anyway. A pool at zero refuses the step
    with the reason, through the same `0x21` reject path paralysis uses. Staff never
    tire. This corrects the earlier claim that unencumbered running is free: ServUO
    charges the baseline, and against the regen it is very nearly a wash, which is
    why classic running feels endless without being free.
  - [x] **Combat sounds, per creature, and the projectile.** A landed blow plays
    the *attacker's own* sound — a creature's ServUO `BaseSoundID + 2`, a human's
    fists thwack — so an orc growls its attack instead of punching like a man; a
    death plays the victim's cry (`BaseSoundID + 4`, or a gendered human gasp), and
    a creature growls (`+ 0`) the moment the `ai` aggros it. The sounds derive from
    the body id via `creature_base_sound(body)` (keyed like `creature_name`,
    ServUO's per-creature `BaseSoundID`), so the pack needs no sound data — it
    falls out of the bodies it already spawns. A ranged volley also flies a real
    arrow: a `0x70` moving graphical effect from shooter to mark. The `protocol`
    feedback encoders (`0x54` sound, `0x70`/`0xC0` effect, `0x6E`/`0xE2` animation)
    and `WorldState::broadcast_from`/`play_sound`/`animate` are the seam; every
    source broadcasts to the players in view range.
  - [x] **Swing, death and cast animation.** `WorldState::animate(mobile, Action)`
    animates a swing, a death throe or a cast gesture for everyone in view range.
    The modern client (7.0.0.0+, gated on `Feature::NewMobileAnimation`) gets the
    `0xE2` new-animation packet, where the server names a body-agnostic
    `AnimationType` (Attack/Die/Spell) and the *client* picks the frames — so no
    body table is needed there, the path the test clients take. An older client
    gets the `0x6E` classic packet, whose action id is body-specific, chosen off a
    coarse humanoid-vs-creature split (`body_opens_doors`). Wired into the melee
    and ranged swing, death (`combat::die`), and the cast. ServUO's ids: Wrestle
    31, human die 21, human cast 16; monster attack 4, die 2, cast 12.
  - [ ] **Exact per-weapon and per-body `0x6E` actions** — the classic-packet
    action is a coarse humanoid/creature split, not the per-weapon (slash vs bash
    vs pierce), mounted, or per-monster action ServUO computes from the body
    tables. The modern `0xE2` path is exact; this only refines the old 2D client,
    the minority path, and wants the body-animation tables.
- [x] `skills` — the table, the check, the gain
  - [x] **The fifty-eight skills are data now** (`state::skill`, ported whole from
    ServUO's `Server/Skills.cs`): each skill's client id, its name and title, the
    stats it leans on and the weight it lends each of them, its gain factor, and
    whether it can be used from the window at all. Fixed point, not floats —
    scales in hundredths, gains in thousandths, factors per-mille — because the
    tick replays. **This turned up a real bug:** five of the eight skill ids
    combat used were wrong (Fencing on Cooking's, Macing on Discordance's,
    Tactics on Poisoning's, Wrestling on Tailoring's, Swords on Mace Fighting's).
    They are the client's own `skills.mul` indices and they ride the `0x3A` both
    ways, so a swordsman's gains showed on the Mace Fighting bar. Nothing noticed:
    a roll trains whatever id it is handed.
  - [x] **The check and the gain are ServUO's.** Sphere's `Calc_GetSCurve` against
    a single difficulty is gone, and so is the flat linear gain that stood in for a
    curve. In its place: `CheckSkill` over a difficulty **band** — under it you
    cannot, at it you learn nothing — and `GetGainChance`, which averages the
    headroom under the skill's own cap and under the **total** one. That total cap
    (700.0) is the point: it is what makes a character a build rather than a list,
    and the engine had no notion of it. With it come the rules that hang off it —
    a `Locked` skill holds, a `Down` skill gives ground so another can rise past
    the cap, and a creature is exempt as ServUO exempts it.
  - [x] **Stat gain**, in both of ServUO's mechanics: before ML each stat rolls its
    own weight from the skill's row (`StrGain / 33.3`), from ML one flat chance
    picks the skill's primary stat three times in four. Per-stat and total caps
    bind, a stat at the total cap takes its point from one set to fall, and a
    per-stat cooldown (a tick count, so it replays) stops a flurry of uses pouring
    into one stat. Three `StatLocks` of their own, on the wire in both directions.
  - [x] **A skill is worth more than it is trained.** `skill_value` is ServUO's
    `Skill.NonRacialValue`: the base plus what the mobile's stats lend it, fading
    as the base rises and capped at the row's own ceiling. A **read-site
    derivation**, so a Strength spell raises a smith's effective skill with no
    bookkeeping and nothing to undo. Gone from AoS on, as
    `AOS.DisableStatInfluences` makes it. The `0x3A`'s `value` and `base` are two
    different numbers at last — they had carried the same one since the beginning.
  - [x] **A seeded generator in the world.** A roll is randomness inside a tick,
    and the tick must replay. So `Rng` (xorshift64\*) is a plain field the world
    owns, seeded once from a fixed default and advanced only by the tick — two
    identical runs reach the same skill, roll for roll (there is a test that
    asserts exactly this).
  - [x] **stats** (str/dex/int). A mobile carries `Stats { strength, dexterity,
    intelligence }`; `enter` gives a character the classic 100/100/100 and derives
    its `Hitpoints.max` from strength, `Mana.max` from intelligence and
    `Stamina.max` from dexterity. `skills::apply_stats` is the one door they change
    through, so the three pools can never drift from them.
  - [x] **The skills window on the client** (`0x3A`, both ways from ServUO's
    `SkillUpdate`/`SkillChange`), with per-skill caps and the lock arrows, and the
    status bar's three stat arrows beside it (`0xBF 0x1A` in, `0xBF 0x19` type 2
    out — relayed, unlike a skill arrow, because nothing else sends the stat bits
    and a client that never gets them draws all three pointing up).
  - [x] **The window's buttons work** (`0x12` type `0x24`). It was decoded, tested
    and routed nowhere, so pressing a skill did nothing at all — no message, no
    error, nothing in a log. Now it runs ServUO's `Skills.UseSkill`: a ghost is
    silent, a use inside another's cooldown is refused out loud (cliloc 500118),
    and the thirty-five skills that cannot be used this way get the client's own
    line for it (**cliloc 500014**), which is the right core default and not a gap.
    The twenty-three that can emit a `SkillRequested` for the pack *and* run the
    core's own handler — the "default in core, customise in the pack" split spells
    and loot have.
  - [x] **The cursor seam**, and the first two skills through it. An object cursor
    (`0x6C` type 0) goes up, the world remembers which skill asked
    (`TargetPurpose::Skill`), and the answer reaches the skill a packet later, its
    reach re-checked server-side. **Anatomy** and **Evaluating Intelligence** are
    done and set the shape: a margin of error narrowing with skill, a roll that
    both decides and trains, and an answer chosen by arithmetic on a base cliloc
    (`1038045 + strength*11 + dexterity`), drawn over the thing looked at and sent
    to one connection. Adds `encode_localized_message` (`0xC1`) — whose arguments
    are UTF-16 **little-endian**, the opposite of the `0xAE` a few lines above it
    in the same file.
  - [x] **The gear tables are data, in `state`.** Arms Lore needs a weapon's kind
    and damage and an armour piece's rating — the same rows `combat` reads to swing
    and to absorb — so the tables moved down to `state::weapon` and `state::armor`,
    the `state::title`/`combat::titles` split already in the tree: data below,
    rules in the crate that owns them. `equipped_weapon`, `swing_ticks` and
    `absorb_physical` did not move. The weapon table grew ServUO's `WeaponType`
    (Slashing/Piercing/Bashing/Axe/Polearm/Staff/Ranged), which is *not* derivable
    from the skill column — a war axe is an axe that bashes, a dagger a knife that
    pierces, and Arms Lore reads five different cliloc blocks off exactly that.
  - [x] **And the tiledata layer byte is read at last.** Whether a weapon takes
    both hands is in `tiledata.mul` — the *quality* field, which ServUO reads
    straight into `Layer` (`BaseWeapon`: `Layer = (Layer)ItemData.Quality`) — and
    this reader dropped it. It is `StaticTile::layer` and `Terrain::item_layer`
    now, pinned against a real file. Six weapon classes override it in code and
    only those six carry a `WeaponData::hands`, because measured against a real
    `tiledata.mul` the file is simply **wrong** about them: it files the bow, the
    crossbow, the heavy crossbow, the battle axe and the war hammer as one-handed.
    That is why the fact is read from the client *and* overridable, rather than
    either alone.
  - [ ] **The other twenty-one usable skills.** In rough order of what they cost:
    - [x] **Arms Lore, Item Identification and Forensic Evaluation.** The same
      shape as Anatomy and Eval Int, over three different subjects, so the handlers
      split by what they read: `handlers/lore.rs` (a living body),
      `handlers/appraise.rs` (an object), `handlers/forensics.rs` (a crime). The
      cursor's **prompt and reach are per skill** now (a table, not one shared
      range): Arms Lore reaches 2 tiles, Item ID 8, Forensics 10, each with
      ServUO's own prompt cliloc, which the two skills that were already done had
      been sending none of.
      **Forensics needed the world to keep notes**, and that is the interesting
      part: a `Corpse` component (owner, killer, forensicist, looters) is written
      where a corpse is *laid* and a looter is recorded where an item is *lifted*,
      so the skill only reads what somebody else's rule already recorded — and it
      **persists** (schema v17), because a body lies for seven minutes and a shard
      restarts inside that window. The killer is kept as a **name**, not a serial:
      ServUO holds a live `Mobile` and reads `.Name` at examination time, which
      cannot answer once the killer has logged out, and a corpse outliving its
      killer's session is the ordinary case. Arms Lore's durability lines are
      deliberately absent (an item here has no hit points) and Item ID prices only
      what the pack priced — a guessed value would read as authoritative.
    - [ ] **Taste Identification** — lands with Poisoning below, because what it
      tastes *for* is the poison that slice adds.
    - [x] **Animal Lore**, once pets existed — which is exactly why it waited. Its
      three gates *are* the skill (under 100.0 only a tamed creature, under 110.0
      that or a tameable one, above it anything), and every one of them asks a
      question only the pet slice can answer. The window is ServUO's
      `AnimalLoreGump` in its ML frame through the typed `GumpLayout` builder, in
      **two pages rather than five**: this engine has the attributes and the combat
      ratings, and the three pages it drops are numbers nothing in the world sets
      yet — a column of dashes is worse than a page that is not there.
    - [x] **Meditation and Spirit Speak** — the two skills a mobile turns on itself,
      so pressing the button *is* the whole use and no cursor goes up.
      **Meditation** is one `Meditating` marker and no timer: what ends a trance is
      somebody doing something, and that is now a real seam — `WorldState::disrupt`
      (ServUO's `DisruptiveAction`) called from the step, the blow, the word and the
      lift, which is the same call list the stealth slice will reveal on. Its gates
      are ServUO's in order (busy 501845, body under a tenth 501849, at peace
      501846, hands not free 502626 — a spellbook allowed, a shield not).
      **And the trance had to be worth something**, so mana regen stopped being a
      flat sixty ticks for everybody and became ServUO's pre-AoS curve:
      `medPoints = (Int + Meditation)/2` from seven seconds a point down to three
      quarters of one, plus an **armour offset in seconds** — which is what makes a
      mage in plate regenerate like a warrior and the free-hands rule mean anything.
      The offset needed one more column of ServUO's armour data
      (`MedAllowance`: leather `All`, studded `Half`, metal `None`), and the per-mobile
      rate is **stateless** — a mobile gets its point when the tick counter divides
      its *own* rate, so nothing is stored and nothing is saved.
      **Spirit Speak** is the pre-AoS form: `HearsGhosts { until }` for
      `base/50*90` seconds (floor fifteen), and the gate it feeds is a *second*
      predicate — `can_hear_mobile`, not a relaxed `can_see_mobile`, because a ghost
      must stay invisible to the listener or contacting the netherworld would make
      the dead walk visibly among the living. It does not persist, being seconds long,
      like a cast in flight.
    - [x] **Poisoning and Taste Identification**, the two ends of one fact. A
      `PoisonCharges { level, charges }` on an *item* is both a bottled dose and a
      coating on a blade — ServUO tells them apart by what the item is, and so does
      this. Poisoning is the engine's only **two-cursor** skill (the potion, then the
      blade), which added `TargetPurpose::SkillSecond`; the potion is spent either
      way and leaves the empty bottle; a coated blade holds `18 - level*2` doses and
      `combat` spends one into whatever it cuts, through the one `apply_poison` door.
      A fumble under grandmaster can poison the poisoner — decided in `skills`,
      *emitted*, and applied by the tick through combat, because applying poison is
      combat's door and `skills` sits below it. Taste ID reads the same component; so
      does Arms Lore, which is ServUO's behaviour (a weapon master does not have to
      lick a sword). The four potions **share a graphic** (`0x0F0A`), so which poison
      a bottle holds cannot come from a core table: it is on the item, put there by
      the pack (`op_set_poison`) or a staff `.poison <level>`, and **persisted**
      (schema v18) for exactly the reason a spellbook's mask is.
      **Awarding fame and karma moved out of `combat` into `state::title`** with
      this: Poisoning costs twenty karma, and `skills` cannot depend on `combat`
      because `combat` already depends on `skills`. The file's own note had said a
      crate of its own "would depend on combat for its only input" — a kill stopped
      being the only input, so standing now lives beside the table it feeds.
    - [x] **Begging and Remove Trap.** Begging is ServUO's, with one deliberate
      change: its beggar takes a tenth of what is actually in the target's pack,
      because its NPCs carry pack gold — ours carry none and a corpse's gold is
      already invented at death, so a townsperson gives from a notional purse and a
      *vendor* refuses (its till is a stock crate, not a purse). The karma cost is
      exact: up to forty, down to a floor of −3000, which is what stops the loss
      running away and a career beggar being free. It also added the two small
      substrate pieces it needed — `WorldState::face_toward` (two people talking
      face each other, ServUO's `GetDirectionTo`, which moved `direction_toward`
      down into `movement` beside its inverse `step_from`) and an `Action::Bow`.
      **Remove Trap** brought traps with it: a `Trap { kind, power, level }` on a
      container, ServUO's four kinds and their damage, sprung when the chest is
      opened by anyone but staff (a sprung trap hurts, it does not bar the lid) and
      taken off by the skill. The trigger lives in `tick/traps.rs` rather than in
      `items`, because the damage has to go through `combat::damage` and `items`
      cannot depend on `combat` without closing the `skills → items → combat →
      skills` loop. Neither reference traps anything in Britannia's own data, so —
      exactly like the `Lock` slice before it — it ships with a staff `.trap` and a
      path to pack data rather than as a rule nothing can reach. It **persists**
      (schema v19): a restart that quietly disarms every chest on the shard is the
      same silent loss as one that forgets a lock.
    - [ ] **Inscribe** — the last of the six, and the one that wants a writable book
      to copy.
    - [x] **Stealth is a subsystem, not a skill** — and it landed as one. `Hidden`
      and `Stealthing { steps_left }` live in `state`, read by the *one* gate
      `WorldState::can_see_mobile` (where `Ghost` already lives) and broken by the
      *one* call `WorldState::break_cover` (ServUO's `RevealingAction`, whose last
      line is `DisruptiveAction` — so it disrupts a trance too, and the two are one
      call here as they are there). That is what lets attacking, speaking and
      lifting each give a hider away without a single one of them knowing what
      hiding is: `combat::swings`, `combat::damage`, `chat::speak` and
      `items::pick_up` call `break_cover`, and the two movement paths call
      `step_while_hidden`, which spends a stealth step or gives you away.
      **Hiding** is ServUO's, including the gate that matters: you cannot hide from
      somebody who is *fighting* you within `(100-skill)/2 + 8` tiles, checked both
      ways, which is what stops hiding being a combat escape. **Stealth** wants
      80.0 Hiding and armour under 26 (the plain worn rating pre-AoS — which moved
      `worn_armor_rating` down to `state::armor` beside its data, three readers
      now), and buys `value/10` steps. **Detect Hidden** is a contest
      (`detect/1.5` against each hider's Hiding), not a flat roll, over
      `1 + value/10` tiles. **Stealing** is weight-gated (`10 + value/10` stones)
      and tells the victim *by name* when it fails; the theft itself is returned as
      an intent, because moving an item is `items`' door and flagging a criminal is
      `combat`'s. **Snooping** has no button at all — the action that uses it is an
      ordinary double-click on a container in somebody else's pack, so it is called
      from the tick where the click is dispatched, costs karma every time, and a
      clumsy peek is noticed by name.
      Deferred: **Tracking** (two gumps and the `0x9A` quest-arrow packet) and the
      AoS per-material stealth-armour table.
    - [x] **Bard is a subsystem too**, and it landed as one. `state::instrument` is
      the core table (six classic instruments, each with the pair of sounds its
      ServUO class passes to `base(graphic, well, badly)`), an `Instrument
      { uses_left }` on the item is spent by every attempt, and the three skills
      share a **bard range** (`8 + value/15`), a **Musicianship check before the
      skill's own roll** — which is what makes Musicianship worth training on its
      own — and one `base_difficulty` computed from the target's pools and skills
      rather than a fixed band. A bard with no instrument in the pack gets no cursor
      at all.
      The two lasting effects are components with a tick expiry and **neither is
      folded into anything**. `Pacified` is read where a blow would land
      (`combat::swings`) and where the AI decides (`ai::think_one`), so a calmed
      creature neither swings nor hunts. `Discorded` is read in **`skill_value`** —
      the one question every other system already asks about how good somebody is —
      so a discorded creature hits worse, resists worse and casts worse without
      combat, magic or the AI knowing what a lute is. Provocation reuses the
      `Combat` component the AI already drives, so there is no second fight loop.
      **Musicianship** is the one bard skill with no target: it comes through the
      double-click seam (`tick/skills_wire.rs`'s `use_item_skill`), run *after* the
      `ItemUsed` the pack sees — default in core, customise in the pack, in that
      order. Deferred: the per-target duration scaling (a flat thirty seconds here),
      and the AoS/SE resistance-mod form of Discordance.
    - [x] **Taming, and the pets it wanted.** A `Pet { owner, slots, order,
      order_target }` on the creature and a `Tamable { min_skill, slots }` for the
      kind, with a core table keyed by body (`state::tame`) that a spawn may
      override — and **every rideable body is tamable**, derived from the mount
      table rather than listed twice, because a horse you cannot tame is a horse
      nobody can have (the `mount_body_for` lesson, applied before it could bite
      again).
      **Animal Taming** keeps every gate in ServUO's order — not tamable, already
      tame, too many followers, no chance — and the anger roll, which is what makes
      taming a bear a decision rather than a formality; its timer is dropped the way
      Poisoning's is. The taming itself is an intent: `npc::tame` makes the pet,
      because `npc` owns what a creature *is*, and it gives a brainless prop animal
      a brain, without which a pet would never beat and so never follow.
      **A pet does not decide anything**: `ai::pet_beat` carries out its last order
      and returns a direction, so a pet moves through the same `step` a wild
      creature and a townsperson use, and an attack order simply points the `Combat`
      the AI already drives. **Orders come through speech** (`npc::pets`) — "all
      kill", "<name> stay" — matched on the words, because the `0xAD` keyword block
      is skipped by the parser; ServUO's keyword ids are recorded beside the table
      for the day it is decoded. **Follower slots** are a read-site derivation
      (`skills::followers_of`, pets plus the mount), so the bar and the taming
      refusal can never disagree, and the pet **persists** on the mobile's JSON
      record — a restart that quietly released every pet on the shard would be the
      `Murders` lesson again, over property somebody spent an hour earning.
      Deferred: **stabling** (which wants a pet saved with no position, the
      logged-out-character shape), **loyalty** (which is pointless without feeding),
      and **Herding**.
  - [x] **Item-triggered skills** — Healing, Veterinary and Lockpicking, through
    the double-click seam rather than the window, because the action that uses them
    *is* a double-click on the bandage or the pick. They come in through
    `tick/skills_wire.rs`'s `use_item_skill`, run after the `ItemUsed` the pack
    sees, and each raises its own cursor by reusing `TargetPurpose::SkillSecond` —
    the item is the first answer, the patient or the lock the second.
    **A bandage is the one skill whose duration is the mechanic**, so unlike
    Poisoning (whose two-second beat is flavour and resolves at once) it really does
    keep a `Bandaging { patient, done_at }` and finish on the tick counter: ServUO's
    pre-AoS timing off dexterity (about ten seconds on yourself, three on somebody
    else, five more for a resurrection), the bandage spent when the work *begins*,
    and the three outcomes — mend, cure, resurrect — with their own thresholds and
    chances. Each is returned as an intent and applied by the tick through the crate
    that owns the door. **Lockpicking** gave `Lock` the two levels ServUO has
    (`required_skill`/`max_skill`): without them every lock is either free or
    impossible, and a failed pick snaps. Deferred: **Camping**, which wants a reason
    to light a fire (logging out safely in the wild) more than it wants the fire.
  - [x] **And the shops already sell what the new skills need.** The converter reads
    ServUO's own `SB*.cs`, so the Community Pack's vendors were already stocking
    bandages (37 of them), lockpicks (19), instruments (15) and poison potions (26)
    — they were simply inert. An item's core state now lands where the item is
    *made* (`items::apply_core_defaults`, called from the shelf, the spawn and the
    staff `.add`), because a graphic alone cannot say how many tunes are left in a
    lute or which of the four poisons is in a bottle. The poison is read off the
    **label**, which the converter carries through from ServUO: "a greater poison
    potion" is level two, and an unlabelled bottle is the middling one.
  - [x] **Mining, Lumberjacking and Fishing — the harvest system.** ServUO's
    `Scripts/Services/Harvest/`, and the pillar Crafting was waiting on: nothing
    in the engine could produce a raw material. The four definitions (ore, sand,
    lumber, fishing) are core data in `state::harvest` with their real numbers —
    ore a bank of 8×8 holding 10–34, respawning in 10–20 minutes at reach 2, nine
    veins from iron at 49.6% down to valorite at 1.4%, each richer vein
    disappointing into iron one swing in two hundred; lumber a bank of 4×3 holding
    20–45 over 20–30 minutes, ten logs a swing and twenty in Felucca; sand six
    beats to a swing; fishing a single eight-second cast at reach 4. Skills in
    tenths, chances in hundredths of a percent, every duration a tick count.

    **A bank belongs to the ground, not to an entity**, so `Banks` sits on
    `FacetState` beside the sector grid and the obstruction index, keyed by kind
    and block. It is **deliberately not persisted**, as ServUO does not persist
    it — a restart repays every vein, which is written beside the struct so it is
    not filed as a bug later. What *is* saved is the vein's *position*: where
    ServUO seeds a `Random` with `(x*17)+(y*11)+(map*3)`, this hashes the same
    three inputs, because a bank that is not saved must still find the same ore
    under the same block after a reboot or a valorite vein wanders.

    **The load-bearing half is reading the tile.** A `0x6C` location reply carries
    a graphic only when a *static* was clicked; a click on bare land arrives with
    a graphic of **zero** and the land tile id is never on the wire, so the server
    looks it up — a new `Terrain::land_tile`, beside `statics_at`. And a claimed
    static is verified against the map at that exact id *and* z before it is
    believed (ServUO's `PacketHandlers.cs` cancels the target otherwise): without
    that a client names a tree at its feet and mines the middle of Britain. A
    static is matched as `(id & 0x3FFF) | 0x4000` and land raw, which is why the
    mountain *ground* and the mountain *wall* both reach the ore definition.

    The rest is the shape the bandage slice set: a double-click on the tool
    (`use_item_skill`, so an axe is a lumberjack's tool and a weapon at once —
    derived from `state::weapon`'s `is_axe`, not listed twice), a **location**
    cursor under `TargetPurpose::Harvest`, a `Harvesting` component beaten down on
    the tick counter with its ServUO gesture and sound each time, and on the last
    beat `CheckHarvestSkill` — the flat `req_skill` *and* `roll_skill_band`, the
    same call combat's to-hit makes, so a miner trains from the attempt. Every
    gate is re-checked on every beat, because all of them change under a swing
    that takes seconds; walking away mid-swing gets a **different** line from
    clicking too far off, which is ServUO's distinction and the whole feedback.
    A tool spends a use per swing and breaks, which needed schema **v20**: one
    nullable `uses` column serving both the new `Tool` and the existing
    `Instrument` — the latter a bug this fixes, since a half-played lute came back
    full at every reboot. The seven woods are gated on `[gameplay] expansion`
    (ML by default), which threaded `expansion` into `Gameplay` as an ordinal so
    the `0xB9` mask and the content tables read one setting.

    The vendors already stocked the tools — 46 pickaxes, 40 hatchets, 21 fishing
    poles — and were inert, exactly as the bandages and lutes were before their
    slice. Deferred: ML **bonus resources** (gems, bark fragments, pearls), whose
    items do not exist yet; **granite** and the special deep-water catches;
    `BaseOre`'s pile-size art swap, without which rolling ServUO's four ore
    graphics would leave four piles that refuse to merge; and High Seas' lava
    tiles. The **pack-capacity** refusal this list also carried has landed — see
    `items::capacity` under the staff-command entry in §7.
  - [ ] Sphere's per-skill `AdvRate` tables and its "learn only from a challenge"
    `GainRadius` — **dropped, not deferred**: ServUO's band *is* the
    learn-from-a-challenge rule, and its `gain_factor` column is the per-skill
    rate. Kept here only so nobody re-adds it from the old plan.
- [x] `crafting` — **making things, and the 485 recipes to make.** The pillar the
  harvest slice existed for: mining paid a player in ore and nothing in the
  engine consumed a raw material. A port of ServUO's `Scripts/Services/Craft/` as
  a system in the usual shape — `fn(&mut WorldState)` over `state`, its own
  `ItemCrafted`, no peer calls — with five trades wired: **Blacksmithy**,
  **Tailoring**, **Carpentry**, **Tinkering** and **Alchemy**.
  - **The recipes are core data**, like `magic::spells` and `state::weapon`: a
    bare shard has to be able to forge. `tools/gen-craft-tables` reads ServUO's
    own `Def*.cs` once, its output is committed under `crafting/src/defs/`, and
    those files are ordinary source from then on. The generator's hard half is
    that ServUO names a crafted item by its **C# type** and this engine needs a
    **graphic**, so it indexes every class under `Scripts/` and walks the
    inheritance chain to whichever constructor finally passes a literal id.
    **A type that will not resolve is dropped and printed**, never guessed — the
    `resolveBody` lesson. Of 624 recipes parsed, **485 ship**; the 139 dropped
    are counted in the run's own summary (86 recipe-scroll gated, 37 theme pack,
    7 custom-craft, 5 on the scales axis, 4 whose art will not resolve). A
    further 211 of ServUO's 835 sit behind `Core.SA`/`HS`/`TOL`/`EJ` guards the
    parser removes whole, because `[gameplay] expansion` tops out at ML.
  - **The material axis is a hue swap.** ServUO needs nine `IronIngot` subclasses
    because a C# item *is* its class; an item here is a graphic and a hue, so the
    nine rows of `AddSubRes` collapse to nine hues against one graphic — the same
    nine `state::harvest::ORES` already pays a miner in, asserted equal in a test
    so a hue can never mean valorite on the ground and copper at the forge. That
    made **`items::take_from_backpack` hue-aware** (`take_from_backpack_of_hue`):
    hue *is* identity for a material, and a hue-blind take quietly pays a
    valorite order in iron.
  - **The chance is ServUO's, and its three corners are each a place a plausible
    simplification is wrong.** `chance_at_min + (val - min)/(max - min) *
    (1 - chance_at_min)`, in per-mille. Failing the *band* and failing the *roll*
    are different refusals — one costs nothing and gets cliloc 1044153, the other
    costs the materials — and folding them together eats the ingots of every
    player who clicked a recipe they were not yet good enough for. The
    exceptional draw is **independent of the success draw and made first**, so
    what follows a craft does not depend on how the craft went and the tick still
    replays. And a chance can be *negative*, which is not clamped up: a recipe's
    `min_skill_offset` licenses the attempt, it does not discount the odds.
  - **Every gate is checked twice**, which is design and not redundancy: ServUO
    dry-runs the whole of `ConsumeRes` before starting its timer and again when
    it ends. A craft takes seconds, and in those seconds a player can step away
    from the forge, hand the ingots to a friend, or wear the tongs out.
  - **The workshop scan reads statics as well as items.** A forge is sometimes
    decoration the converter placed and sometimes a tile baked into the map, and
    Britannia has both kinds in the same buildings — `DefBlacksmithy` scans the
    two separately for exactly that reason. Reading only the entities refuses a
    craft at half the forges in the game, and the refusal reads as a broken
    recipe rather than a missing scan. ServUO's per-candidate line-of-sight ray
    is deliberately *not* copied; the ±16 z band already throws out the forge on
    the floor above.
  - **Smelting had to land with it**, or Blacksmithy is unreachable from Mining:
    a miner is paid in ore and every smith recipe eats ingots. ServUO's
    `BaseOre.OnDoubleClick`, with one deliberate difference — its target cursor
    exists to pick which forge and to combine piles, and neither applies here
    (one predicate answers "is there a forge", and identical piles merge on their
    own).
  - **The window** is `CraftGump`/`CraftGumpItem` through the typed `GumpLayout`,
    the path `MondainQuestGump` took, with ServUO's `1 + kind + index * 7` button
    encoding kept verbatim — the decode has to agree exactly and a scheme of
    one's own is a second thing to get wrong. The reply is matched against
    **what the server remembers drawing** (`open_craft_gumps` beside
    `open_quest_gumps`), which carries more weight here than it does for a quest
    log: the tool, the category and the chosen metal all live in the context and
    never in the packet. One layout detail is load-bearing and was got wrong
    first: the **categories are drawn on page zero**, which is what puts them on
    every page of a paginated list — inside the pagination the whole left column
    vanishes the moment a category runs past ten rows, which most of them do.
  - **The way in is the tool's double-click**, through the same `use_item_skill`
    seam the bandage, the lockpick and the pickaxe come through. There is no
    craft packet at all. The tool table is `state::craft`, in `state` for the
    reason `state::weapon` is — two crates read it: `items` to give a fresh
    sewing kit its uses, `crafting` to know which of the five windows to open.
    The vendors already stocked all of it (26 tongs, 28 sewing kits, 15 saws, 41
    scribe's pens) and every one was an inert prop, exactly as the bandages,
    lutes and pickaxes were before their slices.
  - **Quality and the maker's mark persist (schema v21).** `Quality` and
    `CraftedBy` are components on the item, and both are **read at the read
    site** — `state::armor::piece_rating` adds ServUO's `-8 + 8 * quality` and a
    material bonus derived from the hue (valorite +16 over iron, barbed +16 over
    plain leather), so nothing is folded into the wearer and a fine breastplate
    coming off leaves nothing to undo. That material ladder is what makes the
    metal axis worth offering at all. The maker is a **name and not a serial**,
    for the reason a corpse's killer is one: the smith logs out and the sword
    outlives the session. Without the two columns every masterpiece on the shard
    quietly becomes ordinary at the next boot — the `Murders` bug, over property
    somebody spent an hour earning.
  - Deferred, each its own system hanging off crafting: **Repair**, **Enhance**,
    **AlterItem**, **Resmelt** (item back to ingots; *ore* smelting is in),
    **recipe scrolls**, **make-number / make-max** and the **last-ten list**
    (per-player UI state ServUO serializes, so it wants a decision about saving
    UI). The six remaining tables — Cooking, Inscription, Bowcraft,
    Glassblowing, Masonry, Cartography — are data the generator can emit when
    they are wanted; Inscription waits on the writable book it is already tied
    to. And two material chains stay unbuilt rather than implied: **hides →
    leather** (scissors on a hide) and **cotton → thread → cloth** (a spinning
    wheel and a loom), both of which are addon interactions in ServUO and not
    crafts at all — until they exist a tailor buys cloth and leather from the
    vendors that already stock them.
- [x] `magic` — spells, reagents, casting
  - [x] **Mana, casting, and the effect seam.** A mobile carries `Mana` (spent by
    casting, trickling back on a tick-counter regen). `Command::CastSpell` is the
    gate every spell passes: it checks the mana, rolls the casting skill (through
    the same band roll a mined ore uses, so casting trains Magery), spends the
    mana, and emits `SpellCast { caster, spell, target, success }`. What the spell
    *does* — a fireball's damage, a heal, a summon — is not here: a script reads
    `SpellCast` and gives it its effect, `MobileDied`'s decoupling a third time.
    `Command::Heal` mends toward the maximum; `op_cast_spell`/`op_heal`/typed
    `op_damage` are the script's hands.
  - [x] **Typed damage and resistances** (the piece combat deferred). `damage`
    now takes a `DamageType` — physical, fire, cold, poison, energy — and cuts it
    by the target's `Resistance` *for that type*, in the one place all damage
    passes through, so a fireball and a sword swing share the door. Melee is
    physical; a spell picks its element.
  - [x] **reagents** — a spell consumes items from a pack. `items` grew the
    container search the deferral named — `count_in_container` and an
    all-or-nothing `take_from_container` — and `cast_spell` grew a second gate
    beside mana: a `Cast` now carries a `pack` and a `(graphic, count)` reagent
    list, and the spell fizzles spending *nothing* unless the pack holds every
    reagent, then consumes them. Reagents-as-data: the script names them per
    spell, the world enforces them. A pack open on a client redraws live too:
    `WorldState` remembers who has each container open (`double_click` records it,
    logout clears it), and a consumed reagent is pushed to those watchers — a
    `0x1D` for an item burned whole, a re-sent `0x25` for a dipped stack.
  - [x] **the client cast path** — a spellbook cast (`0xBF.0x1C`, read from
    ServUO's `PacketHandlers.CastSpell`) decodes to a `RequestCast`. It once
    became a `SpellRequested` event for the script to own the mana and reagents
    (Sphere-scriptpack style); the core spell table below took that over, running
    the whole cast itself, and `SpellRequested` is left dormant behind it. The
    older `0x12` text-command form is a fill-in; a modern client sends the `0xBF`.
  - [x] **The 64-spell core table and the full cast sequence.** All eight circles
    of Magery live in a core table (`magic::spells`, ported from ServUO's
    `SpellInfo` + the classic reagent lists): each spell's circle — which sets its
    mana, cast delay and difficulty — its reagents, what it targets, and its
    *default effect*. `RequestCast` → `World::begin_cast` runs the sequence in the
    core: mana and reagents from the pack, the Magery roll (the same band roll
    a mined ore uses), the target cursor, and the effect. The core runs the
    archetypes the engine can do — direct and area typed damage, heal, teleport —
    and tags the rest `SpellEffect::Scripted`: they still *cast* fully and emit
    `SpellCast` for the pack to give an effect, the "default in core, customise in
    the pack" split skills has. A pack overrides any spell the same way, off
    `SpellCast`.
  - [x] **Cast style, a `[gameplay]` flag** (the Sphere-vs-ServUO knob asked for).
    `cast_style = "servuo"` roots the caster over a cast delay held in a `Casting`
    component and only then raises the target — moving breaks it, and `spell_disturb`
    decides whether a blow mid-cast fizzles it; `cast_style = "sphere"` resolves
    the cast as it is made, walking. Both threaded from `openshard.toml` into
    `WorldState.gameplay`, never branched on `Era`.
  - [x] **Spell cost, three more `[gameplay]` flags** (Sphere's magic model,
    confirmed in `Source-X`). Sphere spends mana and reagents at *resolution* —
    once the cast has succeeded or fizzled, not up front — so `pay_and_roll` now
    checks availability, rolls, and only then spends: `reagents` (require and
    consume reagents at all, off for a mana-only shard), `mana_loss_on_fail`
    (Sphere's `ManaLossFail` — does a fizzle still burn the mana) and
    `reagent_loss_on_fail` (`ReagentLossFail`). A successful cast always spends;
    the UO/ServUO original is all three on. Orthogonal to `cast_style`, which is
    the rooting/precast axis (`MAGICF_PRECAST`/`FREEZEONCAST`).
  - [x] **Poison, in the core.** Poison, Cure and Arch Cure run a `Poisoned`
    component that `combat::poison_tick` pulses like decay — typed poison damage
    cut by poison resistance, in the one place all damage passes — with a dose
    scaled from the caster's Magery. This is the first spell effect that is *stateful
    over time* rather than instantaneous, the shape the timed buffs then reuse.
  - [x] **Timed stat buffs — the Bless/Curse family.** Strength, Agility, Cunning
    and Bless, and their opposites Weaken, Clumsy, Feeblemind and Curse, all moved
    from `Scripted` into the core. `magic::apply_stat_buff` folds a Magery-scaled
    offset into the target's `Stats` and the caps that hang off them (str→hits,
    int→mana), a `StatMods` ledger remembers exactly how to give it back, and
    `magic::expire_buffs` lifts it on the tick counter; a recast refreshes its kind
    rather than stacking a second. The stat change redraws the player's status bar
    (`0x11`), the one thing that re-sends str/dex/int. The effect kinds are
    canonical in `state::effect`.
  - [x] **Effects persist (schema v7).** A live effect is saved with its mobile —
    a `Poisoned` or a `StatMods` entry becomes an `EffectRecord { kind, amount,
    remaining }` on the character or mobile row — and restored on login and boot
    alike, so a relog cannot wash a debuff off. The ServUO/Sphere model reached the
    way this engine saves anything: a record, swept whole, not a stopped world.
    Poison restores as the whole component; a buff as its ledger *only* (its shift
    is already folded into the saved stats, so re-applying would double it).
    `World::effects_of`/`apply_effects` are the one seam; every future buff and
    debuff slots into the same `effects` list with no schema change.
  - [x] **The spellbook gates the cast (schema v8).** A `Spellbook` is a `u64`
    mask (bit _n_ = spell _n_) on the book item; `begin_cast` refuses a spell the
    caster's book does not hold — classic UO, cast only what you scribed. A mage
    sells an empty book (`0x0EFA`) and Magery scrolls (`0x1F2D + spell`); dragging
    a scroll onto the book learns the spell and consumes it (Sphere's scribe
    flow), and double-click opens it (`0x24` gump `0xFFFF` + the `0xBF 0x1B`
    content packet). The mask persists on a nullable `spellbook` item column (a
    `u64` bit-cast to the signed SQL integer so the full book's top bit survives),
    swept in `item_record` and restored in `place_ground_item`/`restore_inventory`
    — without it a relogged book silently refused to open. `.spellbook` is the
    staff shortcut; the Community Pack's Britain mage stocks reagents, book and all
    64 scrolls.
  - [x] **Cast sound and visual.** A resolved spell now plays a sound and throws a
    visual: a fire bolt for fire damage (0x36D4, sound 0x15E), an energy bolt for
    energy (0x379F/0x20A), a magic-arrow bolt for the rest (0x36E4/0x1E5), a
    sparkle on the mark for a heal, cure or buff (0x376A/0x374A/0x373A), an
    explosion at the aimed spot for an area blast — ServUO's per-spell sound and
    particle ids, resolved in `apply_spell_effect` where the core runs the effect.
    Coarse (keyed on the effect variant, not exact per-spell art) but the cast is
    no longer silent and invisible, which was the single most visible gap against a
    real client. A `Scripted` spell voices itself in the pack, off `SpellCast`.
  - [ ] **Per-spell exact art, power words, and the cast gesture** — the visual is
    keyed on the coarse `SpellEffect` today (every fire spell throws the same
    bolt); exact per-spell art wants the spell table to carry its own graphic/sound,
    the power words want the `0x54`-adjacent overhead speech, and the cast gesture
    is the same per-body animation the swing waits on. ServUO's `SpellInfo` carries
    all of it, so this is data the table can grow.
  - [x] **The non-stat magical buffs — Protection, Reactive Armor, Night Sight,
    Magic Reflection.** The family that modifies a *behaviour*, not a number, moved
    from `Scripted` into the core. All four ride one `BehaviourBuffs` component (the
    sibling of `StatMods`: a ledger, at most one entry per kind, a recast refreshes;
    kinds `9..12` in `state::effect`), timed on the tick counter and swept into the
    same saved `effects` list with **no schema change** — a relog keeps them. Each
    is read where its behaviour is decided, pre-AoS (era 1) classic, Magery-scaled:
    **Reactive Armor** bounces a percent of a melee physical blow back at the
    attacker in `combat::damage` (the one damage door; the reflected hit is
    unattributed, which both breaks the recursion and keeps a reflect kill
    blameless); **Protection** rolls its chance to hold concentration in
    `advance_casts` where a blow would else break a cast; **Magic Reflection** bounces
    the next offensive spell back at its caster at the top of `apply_spell_effect`
    and is spent doing it; **Night Sight** sends the caster its personal light
    (`0x4F`, brightest) — a visual no-op until a day/night cycle exists, but sent and
    restored correctly for the moment one does. Each plays ServUO's per-spell sound
    and sparkle. Deferred: the AoS (era 2) resist-swap variants, and a day/night
    cycle for Night Sight to fight.
  - [x] **Persistent fields — Fire, Poison, Energy, Paralyze Field and Wall of
    Stone.** A spell lays a row of ground-tile entities (each a `Graphic`+`Position`
    drawn like a dropped item, carrying a `Field` component), perpendicular to the
    line of fire (ServUO's `eastToWest` from the caster→target vector), Magery-scaled
    in duration. A new `World::field_tick` (`tick/fields.rs`) pulses and expires them
    on the tick counter — the `combat::poison_tick` shape, so a field replays like
    decay, and it runs before `reap` so a field kill lays its corpse the same tick.
    **Fire** pulses fire damage (through the one `combat::damage` door, credited to
    the caster), **Poison** applies poison, and **Paralyze** freezes (see below) to
    whoever stands on a tile (`sectors.nearby(pos, 0)`); **Energy** and **Wall of
    Stone** register each tile in the per-facet obstruction index
    (`obstructions.block`, `door: false`) so they bar players and A\* alike, and free
    it on expiry — the door-toggle/decoration pattern. Fields are transient
    (10–54 s), so like a cast in flight they are **excluded from the save sweep**
    rather than restored as eternal statics. The cast voices only its ServUO sound
    and gesture; the tiles are the visual. Deferred: Dispel Field (Dispel is still
    `Scripted`), the 300 ms row stagger, and per-tile `stand_z` on slopes.
  - [x] **Paralyze, and the freeze mechanic.** A `Frozen { until }` component (tick
    count, like `CriminalUntil`) that the two movement paths consult and refuse
    while it holds — the player walk (`walk`, a `0x21` reject) and the creature/NPC/
    decree step (`step`), the smallest set of edits since there is no single shared
    step. The **Paralyze** spell (Mobile-targeted, moved out of `Scripted`) freezes
    the aimed mobile for `7 + Magery*0.2` s (`magic::apply_paralyze`, a no-op if
    already frozen — ServUO's `Paralyze()`), and the **Paralyze Field** applies the
    same to whoever a tile catches. Classic pre-AoS: paralysis is *move-only*
    (casting and swinging are left to the client, as ServUO's engine does), and **any
    blow lifts it** — `combat::damage` clears `Frozen` inline the moment real damage
    lands, so a reflected hit wakes too. It expires on the tick counter
    (`magic::expire_frozen`, thawing with a "you can move again" line) and **persists**
    on the same `effects` list (kind `13`), so a relog does not thaw it. Deferred: the
    Resisting-Spells duration cut (×0.75), and barring a cast while paralyzed.
  - [ ] **Summons with a lifetime** — Blade Spirits, Energy Vortex, Summon
    Creature/Daemon: a spawned creature that despawns on its own timer and counts
    against the follower cap the status bar already carries.
  - [x] **Travel — Recall, Mark, Gate Travel, and the moongates.** The last big
    Magery family out of `Scripted`, and the first reader of `no_recall`, which
    had been carried through persistence, the converter and the script bridge
    since regions landed with nothing to consume it.
    - **`SpellTarget::Item`** is the fourth target kind: all three spells aim at
      a rune or a runebook, so they raise the *object* cursor (`0x6C` type 0) and
      the client itself refuses bare ground. A recall rune is a graphic plus a
      `RuneMark { facet, destination }`, and a blank one is a rune *without* the
      component — there is no `marked` flag to disagree with a destination that
      would mean nothing when false. (Gate Travel's reagent row was wrong while
      we were in it: blood moss where ServUO and the classic list both have black
      pearl.)
    - **The permission model is one end and one kind.** ServUO's
      `SpellHelper.CheckTravel` is a `bool[7,24]` matrix over twenty-four corners
      of Britannia; almost none exist here and the rest are region flags, so it
      collapses onto `no_teleport` and `no_recall`. What survives is the shape:
      the kinds are *directional*, and `RecallFrom` is the only permissive row,
      so a dungeon nobody may recall **into** is still one you may recall **out**
      of. Folding both ends into one call and testing them against a single kind
      reads tidier and makes every such region a one-way trap; a test caught
      exactly that, and the doc comment says why the tidy version is wrong.
      Sphere's four separate antimagic bits (`RECALL_IN`/`RECALL_OUT`/`GATE`/
      `TELEPORT`) are what the single bool collapses.
    - **Recall's refusals cost nothing**, in `begin_cast` before a point of mana:
      criminal, mid-fight, overloaded, holding something. The carry cap moved
      into `items` beside the walk that sums what is under it — three rules read
      `40 + 3.5 * str` now, and two copies is a shard where a mule can walk but
      cannot recall.
    - **Mark wants the rune in your own pack** (cliloc 1062422); **Recall does
      not**, because ServUO's target does not — a rune held out by a friend is a
      classic way to be fetched. The asymmetry is deliberate on both sides.
    - **Gate Travel lays a pair with no link field**: each gate points at the
      other's *tile*, so the link is the destination and there are not two halves
      to keep honest. Spawned the `spawn_field_tile` way and never through
      `items::spawn_item`, which would stamp a second, contradictory clock and
      announce an `ItemSpawned` the pack reads as a drop. Excluded from the save
      sweep beside `Field`, as ServUO deletes its own on deserialise: restored, a
      half-minute portal is a permanent one whose caster no longer exists.
    - **Walking in is found, not announced** (`tick/gates.rs`), off this tick's
      `MobileMoved` — there are two movement paths and a call beside each is one
      to forget, and unlike a position scan it cannot miss somebody who steps on
      and off inside one batch of commands.
    - **The nine city moongates carry no component at all.** Their destination is
      derived from where they stand, so they are saved and restored as ordinary
      decoration with no schema and no restore hook. They are placed *without* an
      obstruction, which is the thing here that would have been silently wrong:
      `place_decoration` seals a tile whose tiledata calls the graphic
      impassable, and a blocked gate is not a worse gate but one whose walk-in
      trigger is dead code that reads as a broken step check. Their list window
      is the first engine code to read a gump's **switches**.
    - **The runebook** binds sixteen destinations (the rune is consumed), spends
      charges for free travel, and pays the ordinary price on its Recall and Gate
      buttons through the one `magic::pay_and_roll`. Recharging leaves the
      surplus on the cursor rather than eating it. ServUO's button ids verbatim,
      decoded highest-range-first (`BOOK_USE_CHARGE + 40` would else read as a
      Recall), with a row the book does not hold refused rather than clamped.
    - **The facet change underneath** is `WorldState::move_to`, the one door every
      relocation now goes through. Five caches remember where a mobile is and none
      is compiler-checked: the traveller's own screen, every watcher's, the old
      facet's sector grid, `InRegion` (which gained the facet its id belongs to —
      region 3 on two facets compared equal, so a crossing between them fired no
      event, no music and no guards) and the walk sequence, whose reset was a
      latent bug in plain teleports too. The client is told with `0xBF 0x08` and
      the new `0x76`, never `0x1B`; the size in it comes from new `FacetState`
      dimensions, which also fixed login handing every facet Britannia's
      hardcoded 7168×4096. `[gameplay] cross_facet_travel` (off) is the classic
      pre-AoS refusal on top — a rule, not a missing feature.
    - **Schema v22** carries the rune and the book; one bump and not two, because
      there are no migrations and two inside one slice means throwing a test
      database away twice.
    - Deferred: Sacred Journey (decoded and ignored), the moon-phase gates,
      red/young travel restrictions, ship-mark runes, an `op_place_moongate` for
      the pack, and a tooltip that refreshes mid-life — a marked rune is the
      first thing in the world whose *name* changes.
  - [x] **Resurrection** — landed with the ghost slice: `SpellEffect::Resurrect`
    raises the aimed ghost through the core `resurrect` path (a no-op on the
    living).
  - [ ] **Dispel, polymorph** — each waits on a subsystem of its own (summon
    lifetimes, a body-swap that restores cleanly).
  - [ ] **The Poisoning skill for the deadlier doses** — the Magery-cast dose caps
    at greater; the higher poison levels (deadly, lethal) want the Poisoning skill
    to set them.
- [x] `ai` — brains, aggro, wandering
  - [x] **A built-in brain, and room for scripted ones.** A creature spawned with
    a `sight` or `wander` gets a `Brain`, and `think()` gives it a beat every so
    often (not every tick): it notices the nearest player within sight and takes
    a `Combat` aimed at them — so `swings()` attacks it with exactly the machinery
    a player fights with — chases when out of reach, drops a target that dies or
    flees, and drifts when idle. The decision uses the world's `Rng`, so a fight
    replays. Aggro range and wandering are spawn data (`op_spawn_mobile` grew
    `sight`/`wander`), the script-first knobs.
  - [x] **The fully script-driven brain** — the per-mobile `onTick` the scripting
    benchmark sized. A mobile carries a `Scripted` marker; the built-in `think`
    skips anything wearing it, and the server calls that mobile's `onTick` every
    tick instead. A script takes control with `op_control` — which it can only do
    once it knows a serial, so spawning a mobile emits `MobileSpawned`, delivered
    like every other domain event. The built-in `ai` and a scripted brain are the
    two paths, and a mobile is on one or the other, never fought over by both.
  - [x] **Creatures behave like the references say they should.** Movement sees
    the live world: each facet carries an obstruction index of shut doors and
    impassable decoration, and `LiveTerrain` lays it over the map for every walk,
    step and A\* plan — a closed door blocks players and NPCs alike. Aggro needs
    **line of sight** (`Terrain::sight_clear`, a Bresenham ray; windows pass,
    walls and NO_SHOOT statics do not, shut doors are opaque). A chase walks
    naive-step-first, plans once when blocked, follows a **cached `ChasePath`**
    with a 2s repath, and on an impossible route **gives up** — target dropped,
    ~10s standing guard, then back to its life; never the fence-shuffle.
    Humanoids (`body_opens_doors`) open unlocked doors in their way; so do
    townsfolk heading home. Creatures carry an `Aggression` posture (passive
    fauna flee when struck; defensive ones answer the first blow via
    `ai::retaliate`; aggressive ones hunt on sight), break off badly hurt unless
    too big to scare, and step at `gameplay.creature_step_ms` (400 classic — a
    running player outruns a base monster on purpose), each spawn able to
    override its beat.
  - [x] **Ranged creatures volley and kite.** A spawn with `ranged` reach fires
    through `combat::volleys` — typed damage, LOS-gated, sharing the swing timer —
    and keeps its distance at `KITE_GAP` instead of walking into melee.
  - [x] **Level of detail — the AI dozes where no one is watching.** `think` is
    the tick's most expensive per-mobile work: for every `Brain` it runs
    `ai::think_one`, which scans sectors, casts a Bresenham line of sight and
    plans a path. In a populated world most creatures are nowhere near a player,
    and no one sees what they do — so an opt-in `[gameplay]` flag skips that cost
    for them. When `lod` is on, a creature with no player within `lod_radius`
    tiles (and not already in a fight — a fight must not freeze because the target
    stepped a tile away) does not think this beat; its next think is pushed out by
    `lod_idle_factor`, and it wakes the instant a player comes within range. The
    gate leans on a new `WorldState::any_player_near`, cheap because players are
    few (it walks the player table, not the sector grid). `lod_radius` sits above
    the view range and the largest sight, so a creature a player can see is never
    dozed — "no player near" implies "no player in sight", so nothing is missed by
    skipping. Off by default; a shard turns it on to trade a little off-screen
    liveliness for tick budget. Determinism holds — the gate reads only
    `state.ticks` and positions, never a clock.

    The numbers (`cargo run -p openshard-world --example lod_bench --release`,
    Apple-silicon dev machine, release, 5 players clustered in one corner and
    creatures spread across a wide square — the lopsided load LOD is for; 81
    creatures fall within the radius and stay awake):

    | creatures | LOD off | LOD on | speedup |
    |---|---|---|---|
    | 2,000 | 0.44 ms/tick | 0.04 ms/tick | ~12× |
    | 10,000 | 2.23 ms/tick | 0.09 ms/tick | ~25× |

    The gain scales with how much of the world is idle: the awake set is fixed by
    the players, so ten thousand creatures cost barely more than two thousand once
    the frontier dozes. The benchmark is also the project's first whole-`tick`
    timing harness — the scripting one measured a script call in isolation.
  - [x] **A whole Felucca to run it against.** The `.admin` menu grew a
    **Populate Felucca** and **Decorate Felucca** button (verbs `populate:felucca`
    / `decorate:felucca`), and the Community Pack answers them from
    `felucca/_generated/` — ~1,400 monster spawn regions and ~18,400 statics /
    ~640 doors / ~5,600 containers laying the whole facet in one click. The data
    is not hand-entered: a one-shot converter (`tools/convert-servuo.cjs`, the
    "build tool, not an engine feature" the scriptpack note calls for) reads a
    ServUO checkout — `Spawns/felucca.xml` for the spawns, resolving creature
    class names to body ids by scraping `Body`/`SetHits`/`Karma` out of
    `Scripts/Mobiles`, and `Data/Decoration/**.cfg` for the deco, classifying each
    entry by class name (door offsets from ServUO's `BaseDoor` facing table). It
    also generates the town **vendors** — the `Vendors`/`TownsPeople` regions the
    spawn pass skips, placed with a body, dress and shop stock curated per
    profession in `tools/vendor-data.cjs` — and the shop **signs** (`signs.cfg`,
    its own flat format). At full population that is on the order of ten thousand
    creatures across the map — exactly the load the LOD numbers above are drawn
    from, and the reason it was built first.
  - [x] **And a full facet no longer freezes the tick to populate.** Laying ~1,400
    spawn regions at once exposed two costs a small world hid. First, every region
    started due the same tick — a thundering herd — so `register_spawner` now
    **jitters** each fresh region's first spawn across its respawn window (a
    restored region keeps its saved timer). Second, and worse, `maintain_spawners`
    counted each region's live members by scanning *all* creatures, O(regions ×
    creatures) — millions of comparisons a tick, the freeze itself. It now tallies
    every region in **one sweep** (a `HashMap<id, count>`), O(regions + creatures).
    And LOD reaches spawners too: with `lod` on, a region **no player is near is
    left dormant** — its timer held, nothing spawned — until someone approaches,
    the standard "smart spawning". The three together turn a whole-facet Populate
    from a stall into a shrug.
  - [x] **Body-type tables** — ServUO's `Data/bodyTable.cfg` is ported
    (`state::components::body_type`), so `body_opens_doors` is its rule verbatim
    (`!Body.IsAnimal && !Body.IsSea`) rather than a list of eight human ids, and
    rideability is derived from the `BaseMount` subclasses — thirty bodies, with
    `mount_body_for` derived from the same table rather than kept as a second
    hand-written half.
  - [ ] **Path to a tile *adjacent* to the quarry** rather than onto it — the
    remaining refinement from the A\* work; today a chase plans onto the target's
    own tile and stops one short by the reach check.
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
- [x] `housing` — **built, H1–H5.** A multi placed from a deed, walls
  that stop you, a door and secures that know you, a sign that says who owns it
  and how it is wearing, and a house nobody visits collapsing into a crate that
  keeps what was inside. **See [`housing.md`](housing.md)**, which takes the
  eight decisions, records what each phase came out differently on, and names
  what stays deferred (customisation, boats).
  - [x] **The multis are read.** `openshard_uofiles::multi`, both formats. The
    picture was never the problem — a multi is one item that draws as many, and
    every client already owns every house, so the shard sends no component of
    one. What it has to read the same file *for* is the half the picture does not
    carry: where a wall is for the purpose of stopping somebody.

    Three things about the format are in [`findings.md`](findings.md) and cost
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
- [ ] `boats` — a multi that moves: a hull that blocks, a deck you can stand on,
  and everyone aboard arriving with it. **B1 and B2 built; B3–B4 and the tiller
  planned — see
  [`boats.md`](boats.md)**, which refuses a parent transform on the engine's own
  evidence (mounting *deletes* the mount rather than carrying it), keeps the hull
  out of `Obstructions` because that index only ever subtracts and a deck has to
  *add* a surface, and finds that `Feature::SmoothShip` already names `0xF6` and
  its 7.0.9.0 boundary with no packet behind it. It also supplies the repro the
  open pier/bridge defect below has been waiting for.
  - [x] **B1 — a ship on the water, moored.** `openshard-boats`, `.boat <multi
    id>`, `Terrain::land_is_water`, and the `Boats` index on `FacetState` that
    `LiveTerrain` consults as a third source. Saved at schema v32 and the berth
    recomputed at boot. Walking onto a deck lands you on it; walking into a hull
    does not. What the phase found is that `LiveTerrain` forwarded seven methods
    and answered the trait's no-client-files default for every other — a hole
    nothing had asked through until a boat did.
  - [x] **B2 — it moves.** `boats::step` decides then applies, the manifest is
    derived per move, each occupant is relocated absolutely through `move_to`,
    and the hull is redrawn by forget-then-reveal because no packet relocates a
    drawn item. `Sailing` holds the course, the tick's `sail_boats` steps every
    ship whose cadence is up on the reference's own intervals, and a ship whose
    way is blocked furls and its owner is told. `.sail <direction|stop> [fast]`
    steers. `two_boats_do_not_occupy_one_tile_when_one_is_under_way` is built,
    and what it caught is that the *berth* check would have refused a ship the
    right to move at all — every step overlaps the tiles it is leaving. **A move
    costs six packets** with one player aboard and one watching: two for the
    hull, and a `0x20` and a `0x77` for the passenger. B6's tiller is not built;
    `.sail` stands in for it.
  - [ ] **B3 — `0xF6`**, for the clients that can. Strictly additive.
  - [ ] **B4 — the boat as property**: the hold, the plank, the deed, decay.
    Housing's H2–H5 with a different noun.
- [ ] `customisation` — the `0xD7` house design system. **C1 and C2 built; C3–C4
  planned — see [`customisation.md`](customisation.md)**, which reverts housing's
  D7 in full. The decision it turned on was where a per-house component list
  lives: `Terrain::multi_components` cannot hold one — its only key is a `u16`,
  it returns a borrow out of `&self`, its store is fixed at boot, it is
  documented as deliberately not world state, and a synthetic multi id has no
  picture on any client. So a design is a `HouseDesign` component, saved as its
  own table at schema v31.
  - [x] **C1 — designs exist, and staff make them.** The seam and no editor: a
    house can be any shape, saved and restored, with `.hdesign <multi id>`
    copying an existing multi's components onto it. `0xBF 0x1D` and `0xD8` are
    written on both ends — `openshard-protocol`'s `design` module, the layout
    read out of `HouseFoundation.cs` — though nothing sends either yet. What the
    phase found is that a house's shape is read by four things holding a *house*
    rather than a multi id (the sign, the doors, the lockdown area, the walls the
    fall-down path removes), and two of them were already wrong for a designed
    house before one could exist.
  - [x] **C2 — a foundation is placeable.** Not by deleting the refusal: a
    foundation's own component list has no stairs, so one is placed *with* the
    initial design ServUO's `GetEmptyFoundation` derives — the platform, a floor
    around the perimeter, and a stair strip along a row the box is grown by. The
    refusal still stands where that design cannot be built, which is a shard with
    no client files or an id whose platform this install does not hold. The
    question it settled: the stair block is a **derivation**, not a per-house-type
    table. A player can own a foundation; reshaping it is C3's.
  - [ ] **C3 — the session**: enter and leave, build and erase, floor selection,
    commit and revert. The editor itself, on the `0xD7` subcommand set.
  - [ ] **C4 — roofs, backup and restore, and the validation.** ServUO's
    `HouseFoundation.Check*`, whose support-and-reachability half is deferred by
    name: a floating tower is cosmetic, not a hole in the shard.
- [x] `guilds` — **built, with ServUO's five ranks.** Founding, invitations,
  leaving, dismissal, titles, promotion, leadership, disbanding, and the war and
  alliance handshake, reached from the paperdoll's Guild button (`0xD7`/`0x28`).
  - **Notoriety became relative, which is the architectural half.** A `0x78`'s
    notoriety byte is not a property of the mobile — it is the answer to "what
    colour does *this client* draw it in". `notoriety_of` stays the mobile's own
    standing (combat, guards, shopkeepers); `notoriety_toward(viewer, target)` is
    the wire answer, and `broadcast_move` builds one `0x77` per watcher. ServUO's
    order is kept: murderer and criminal resolve **before** guild, so a red
    cannot hide inside a tabard.
  - **A war takes two declarations** — the guildstone's rule. A guild that
    declared and was ignored is *not* at war, which is why `war_offers` is a set
    separate from `wars`: its members must not turn orange on the strength of
    their own guild's opinion. Peace, though, is one guild's decision, because
    the alternative is a guild that cannot stop being attacked by one that will
    not agree to stop.
  - **An invitation is a consent**: a guild may not conscript, so `invite` leaves
    a `GuildCandidate` the player answers.
  - Every operation that can move a colour re-announces the mobiles it moved.
    Nothing on a client asks again on its own.
  - **Saved, schema v26.** The guilds replace-all like the regions; membership is
    a character column, so the roster is derived from who names the guild; and the
    id counter is in the world row rather than re-derived, because a disbanded
    guild leaves no row and the maximum id in the table is not the maximum ever
    issued. The alliances are a second table on the same terms, with a second
    counter, and the guild's `alliance` column is only a back-pointer into it.
  - **Ranks, and the trap in them.** Ronin, Member, Emissary, Warlord, Leader,
    with ServUO's flag set per rank (`Scripts/Misc/Guild.cs`). The ranks are
    ordered and the permissions are **not nested**: an Emissary recruits,
    dismisses, promotes and titles; a Warlord sits above it and does none of
    those, and declares wars the Emissary cannot. So authority is three separate
    questions and each has its own function — `may` for the flag, `outranks` for
    whether the *target* is reachable, and `may_lead` for the two things no flag
    grants (disbanding, and handing the guild over). Any of them written as a
    plain rank comparison gets the Emissary or the Warlord wrong.

    A newcomer joins as a **Ronin**, which holds nothing at all — not the vote,
    not guild items. That is ServUO's `AddMember`, and it is what a promotion is
    for. Promoting stops **two** rungs below the promoter (only the Leader may
    reach the rank below their own), because promoting into the rank directly
    under you would hand out a flag you may not hold yourself; demoting needs
    only that you outrank them, and stops at Ronin. Saved as a number, schema
    v25 — which refuses an older database rather than opening it into a shard
    where every existing member, leaders included, reads as a Ronin and no guild
    has a way back out of that.
  - **Named alliances, replacing a pairwise `Relation::Ally`.** An alliance is a
    named object — several guilds, a leader guild, a member list and a pending
    list — a guild is invited *into* by a guild already in it, and answered by
    that guild's own leader: the shape a player's own membership has, one level
    up. It replaced this engine's own simplification, in which being allied was a
    fact about a *pair*, and A allied with B and with C left B and C strangers.
    That model had no answer to "who is in my alliance", so alliance chat reached
    a set that depended on who was speaking.

    Four rules came with it, and each is a thing the pairwise model could not
    state. The name is claimed once and belongs to the alliance, so extending one
    does not rename it. War and alliance refuse each other in **all three**
    directions — declaring on an ally, inviting somebody you are at war with, and
    joining an alliance that holds a guild you are at war with — because green
    and orange cannot both be true and the notoriety answer would otherwise
    depend on which question was asked first. The leader guild leaving hands the
    alliance on rather than dissolving it (ServUO's `CalculateAllianceLeader`),
    which is why an alliance's id is its own and not that guild's. And an
    alliance that cannot field two members disbands, handing back its whole
    membership — pending guilds included — because each has a link to unhook and
    the alliance is no longer there to be asked.

    Splitting `propose` in half was the point of it: a war is a thing two guilds
    declare at each other, an alliance is a body one is admitted to, and keeping
    them one function is what made an alliance pairwise in the first place. The
    permissions split with it — `CONTROL_WAR_STATUS` for the war, which is the
    Warlord's, and `ALLIANCE_CONTROL` for the alliance, which is the Leader's.
  - **Guild chat, and alliance chat.** A guild line is
    not a command or a prefix — it is ordinary `0xAD` speech with the mode byte
    set to `0x0D` (`0x0E` for the alliance), and it goes back out as an ordinary
    `0xAE` with the same mode so the client draws it in its own colour. What
    matters is that `World::say` branches on the mode **before** anything
    measures a distance: these two pick listeners by membership, and a line that
    fell through to the broadcast would be a private one said out loud in the
    street. `speech_range` answers zero for both, so even a routing failure is
    silence rather than that.

    Our own client can now speak on either: `chat::Channel` is a selector cycled
    with Tab and drawn in the prompt, rather than a `/` prefix — a channel is a
    property of the line, not of its first character, and a prefix hides the
    state it sets. See [`client.md`](client.md).

    An alliance line reaches the alliance's members, which is now one set rather
    than one per speaker — see the entry above for what it used to be.
  - Deferred: the guildstone as a placeable item.
  - Client-side: the window renders, the health bars take their hue from the
    byte, and the **tooltip** now shows here too — the `[ABBR]` suffix and the
    "Warlord, The Silver Serpent" line both. The `0xD6`/`0xDC` half this client
    had never had landed with the guild work rather than after it; see
    "Tooltips, and the half that was never written" in [`client.md`](client.md).
- [x] `parties` — **built.** Inviting, accepting, leaving, kicking, the chat and
  the loot flag, all on `0xBF` subcommand `0x06`. Ported from ServUO's
  `PartyCommands` and `Scripts/Services/Party/`.
  - **Two numberings under one subcommand.** The byte *after* `0x0006` says which
    of the seven a packet is, and inbound and outbound do not agree about it:
    `0x01` is "raise the add cursor" from a client and "here is the whole roster"
    from the shard, `0x08` is "I accept" and is not an outbound number at all.
    Only `0x03`/`0x04` — the two chat lines — mean the same thing both ways. A
    decoder written from one side reads the other's acceptance as a member list.
  - **The empty list is a removal.** There is no "you are in no party" packet:
    ServUO's `PartyEmptyList` is a `0x02` with a member count of zero and the
    recipient's own serial in the removed slot, which is `PartyRemoveMember`'s
    layout with the list empty. One type serves both.
  - **The leader is the id.** A leader who leaves disbands the party rather than
    handing it on, so the leader's serial is fixed for the party's whole life and
    is the key — no counter, and no high-water mark to save. This is the sharpest
    difference from a guild: a guild outlives its founder because it is a thing in
    the world, and a party is only the people in it.
  - **Asking is what creates one**, so a leader who has asked one person and been
    ignored is leading a party of one. `decline` closes it again — otherwise the
    next invitation silently reuses a group with a phantom member in the cap.
    The cap (10) counts members *and* outstanding invitations, the leader
    included.
  - **`tell_party` is the router the whole thing is for.** "A line goes to a set
    of people who are not the ones standing nearby" is one mechanism, and guild
    chat is its second tenant — which is why party was built first rather than
    beside it.
  - Not saved, and that is the reference's behaviour rather than an omission:
    ServUO's `Party` has no serialization, and a party of people who are all
    offline is not a party.
  - **Logging out leaves the party**, which the reference does not need to do:
    ServUO's logged-out `PlayerMobile` stays in the world and stays in the group,
    and this engine despawns the entity. Without `on_logout` a party would hold a
    serial naming nobody — counted against the cap, drawn on everyone else's
    roster as a member they cannot see, and keeping the party alive after the
    last person in it had gone. It follows from what a party is, which is also
    why none of it is saved.
  - **The loot flag has no consumer yet.** `WorldState::party_may_loot` answers,
    and nothing asks: corpses on this shard are open to anybody, because there is
    no criminal-act rule on looting one to exempt a party from. That rule is the
    missing half, and it belongs with the criminal system rather than here.
  - Client-side: **built**. Our own client decodes the four outbound packets,
    holds the roster and the invitation on its `WorldView`, sends five of the
    seven requests, and draws an invitation prompt and a roster window. It also
    turned up that `0xBF` had **no decoder at all** on that end — nine variants
    share the id byte and there was no arm — so the whole family was arriving as
    `Undecoded`. See "The channel selector, and the whole of `0xBF`" in
    [`client.md`](client.md).
- [x] `quests` — **a core system now, ServUO's Mondain's Legacy model, with the
  content left to the pack.** It was built pack-first (five thin seams and an
  opaque JSON blob the engine only stored) and that did not survive a client.
  Three things were wrong:
  - **No quest log.** The paperdoll's Quest button sends `0xD7` subcommand
    `0x32` — a packet, not a gump reply, so nothing pack-side could answer it.
    The id sat in the length table with nothing routing it. A player could accept
    a quest and then had no way to see it, track it or resign it.
  - **Givers went inert at the first restart.** `restore_mobiles` emits no
    `MobileSpawned` (it would re-stock every vendor and duplicate its crate) and
    the pack bound a giver only on that event, so the shard's quests worked
    exactly once — on the boot where `.admin` Populate ran — and never again,
    silently.
  - **The right window was not writable pack-side.** The script `GumpAnswered`
    dropped `switches` (no radio dialog), and there was no server-side gump close,
    no private message and no per-player sound.

  What landed: `crates/server/quests` owns the model (`QuestDef`, objectives Slay /
  Obtain / Deliver / Escort, rewards, `all_objectives`, `done_once`, restart
  delays), the progress passes, the turn-in and the window; the pack owns the
  quests, registered as data through `op_register_quests` and bound to an NPC
  with `op_bind_quest_giver` / `op_make_escortable`. Progress is **found, not
  announced**: kills off `combat::MobileDied`, escorts a point query against
  `Regions`, timers off the tick counter, and Obtain a diffing pass over the
  backpack twice a second — because nothing in the engine says an item moved, and
  a call beside every insert is the pattern the persistence rule warns decays.
  The gump is a port of `MondainQuestGump` (same frame art, same eight sections,
  same button ids, same four sounds) built through a new typed `GumpLayout` in
  `protocol` whose keywords come from ServUO's `Gump*.cs`; a reply is matched
  against what the server remembers drawing, so a `0xB1` for a window this side
  never opened does nothing. Underneath: `MobileUsed` fires for **every**
  double-clicked mobile (a shop no longer swallows it — in ServUO a
  `MondainQuester` *is* a `BaseVendor`), `restore_mobiles` announces a distinct
  `MobileRestored`, and the bindings are saved components (schema v13, replacing
  the v11 blob with structured `quests`/`done_quests`). The `0xB9` mask is what
  makes the client *draw* the button at all, so `[gameplay] expansion`
  (`"aos"`/`"se"`/`"ml"`, ML by default) sends ServUO's `ExpansionML` bits; a
  staff `.quests` and a "Quest Log" context entry reach the same window either
  way. Deferred: quest chains, `ApprenticeObjective`, the question-and-answer
  objective, reward *choice*, the staff force-complete button, and a converter
  pass over ServUO's own `BaseQuest` subclasses now that the model matches theirs.
  Filed and closed as a non-bug: on an already-accepted quest's Description /
  Objectives / Rewards page, the button drawn with the close-box art
  (`0x2EEC`/`0x2EEE`) is `CLOSE_QUEST`, not `CLOSE` — it redraws the Main
  section rather than closing the window, same as `MondainQuestGump.OnResponse`.
  The window is `no_close()` throughout (a right click never dismisses it), so
  a real close is reachable only from Main. Confusing, but retail-accurate;
  kept for parity rather than "fixed".

### Not built, and until now not written down

A sweep of this file against the code turned up a set of gaps that were not
missing on purpose — they were simply never recorded, which is the difference
between a decision and an oversight. Listed here so they are visible; none is
started.

- ~~**Regions.**~~ and ~~**Day and night.**~~ Both landed together; see
  **Regions, guards and the world clock** in §6 below. What is still open from
  that entry: `0x65` weather, a calendar that turns the season, and the `safe`
  flag, which is carried in the data and has no consumer until PvP rules exist.
  `no_recall` got its first reader with travel.
- ~~**Fame, karma and titles.**~~ Landed; see **A character has a standing** in
  §6. The Felucca converter still falls back to a karma-sign heuristic for
  *notoriety*, which is a converter gap and is listed as one below.
- ~~**Resource gathering.**~~ Landed; see **Mining, Lumberjacking and Fishing**
  in §6 below.
- ~~**Crafting.**~~ Landed; see **Crafting** in §6 `crafting` below. Still open
  from that entry: the six remaining `Def*` tables, Repair/Enhance/AlterItem/
  Resmelt, recipe scrolls, make-number/make-max and the last-ten list, and the
  two material chains (hides → leather, cotton → cloth) that are addon
  interactions in ServUO rather than crafts.
- ~~**Travel.**~~ Landed; see **Travel** in §6 `magic`. Still open from that
  entry: Sacred Journey, the moon-phase gates, red/young restrictions, ship-mark
  runes, and a tooltip that refreshes when a property changes — which travel gave
  its first real consumer, since a marked rune's name changes under the player.
- ~~**Party (`0xBF 0x06`).**~~ Landed; see **Parties** in §6 below, and guild
  chat landed on the router it built. Still open from that entry: the loot flag
  has no consumer. **Chat channels (`0xB3`/`0xB5`)** are untouched and are a
  separate thing — the channel window, not the group.
- ~~**Pets and taming.**~~ Landed with Animal Taming; see **Taming, and the pets
  it wanted** in §6 `skills`. Still open from that entry: **stabling** (which
  wants a pet saved with no position, the logged-out-character shape),
  **loyalty** (pointless without feeding) and **Herding**.
- ~~**CI.**~~ **Closed, and it had been for a while.** This entry said
  `.github/workflows` held a release workflow and nothing that ran `cargo test` /
  `clippy` / `fmt`. There is a `ci.yml`, on every pull request and every push to
  `main`, running all three with `-D warnings` and `--locked` — so the project's
  "all three silent" rule is enforced rather than asked for. Recorded as a
  correction rather than deleted, for the reason the `Text::Cliloc(0)` entry
  below is: **check a backlog claim against the code before planning around it.**
- Smaller, and each a slice of an hour or two: dyes and hues on crafted and
  looted items, writable books, the localized text on the signs the converter
  already places, and rate limiting beyond the walk-pace bucket.

### Backlog from the data-table sweep

The craft, body-type, mount, skill, creature-name, creature-sound, harvest-tile
and NPC-name tables moved out of Rust source and into `data/*.json` behind a
`build.rs` (18,155 lines of source became 5,521 of data; the rule is now in
[`architecture.md`](architecture.md#a-big-table-is-data-and-lives-in-datajson)).
Found while doing it, none started:

- **Three tables share the `body` key and are three files.** `body_types.json`
  answers what *type* a body is, `creature_names.json` what it is *called*, and
  `creature_sounds.json` what it *sounds* like — and `creature_base_sound`'s own
  doc already says "grow it alongside `creature_name`", which is an invariant
  stated in prose because nothing enforces it. They were left separate on
  purpose: the three disagree about which bodies share a row (the dire, grey and
  timber wolves are three names and one howl) and the sound rows carry trailing
  notes the other two have no column for. One file keyed by body, with three
  optional columns, would end the drift — at the cost of a format that has to
  express "these four bodies share a sound but not a name".

- ~~**The recipe invariants are tested, not enforced.**~~ **Done**
  ([`unenforced.md`](unenforced.md) S2). The five headers joined the data as
  `crafting/data/craft_systems.json`, so `build.rs` has both halves and checks
  them: a recipe whose group index is out of range, or that does not lead with
  its system's main skill, is now a build failure naming the row. The two
  assertions in `defs/mod.rs` are gone rather than kept beside it — a check in
  two places drifts. Two coverage checks came with them, because "no bad rows"
  is worth nothing if the rows were never opened: a table no header claims, and
  a header whose table is empty, both fail the build too.
- ~~**`Text::Cliloc(0)` is a null.**~~ **Not true, checked:** of the 11,448
  clilocs the craft tables generate, none is `0` — whatever `generate.cjs` did
  when this was written, the data it produces today has no missing
  `TextDefinition` in it. The other half of the entry was real and is now fixed:
  `CraftSystemDef::needs_message` is an `Option<ClilocId>`, `None` on the four
  systems that need no workshop. Recorded rather than deleted because the entry
  sent a session looking for something that was not there: **check a backlog
  claim against the code before planning around it.**
- ~~**`Recipe::amount` has a column and no data.**~~ **Decided: the column
  stays**, with the reason in its doc. Every one of the 485 rows is 1, but
  `craft::complete` already multiplies by it and the recipes that would use it
  are `DefBowFletching`'s arrows and bolts — porting that table is adding data,
  whereas dropping the field would mean the port had to put it back *and* touch
  the craft path to do it.
- **Three files are still over the 2k line.** `world/src/tick/tests.rs` is
  12,964 — by a wide margin the largest file in the repository, and the split
  mechanics in `architecture.md` are written for exactly this;
  `state/src/runtime.rs` is 2,169 and `state/src/components.rs` 2,108, and
  either is the easier warm-up. Deliberately left out of
  [`unenforced.md`](unenforced.md) — see that file's last section for why a
  13,000-line mechanical move wants a session that owns the tree outright.

### Deferred / not yet ported (the Felucca converter)

The one-shot converter (`OpenShard-Community-Pack/tools/convert-servuo.cjs`) lays
the whole facet, but it skips or approximates a few things by design. Recorded
here so the gaps are visible, not silent:

- **Creatures with no literal body** are dropped from the spawns. `resolveBody`
  reads only a literal `Body =`, `Utility.RandomList(first, …)`, `SetBody(n)` or
  the first element of an `int[]` mount table. So `WanderingHealer`/`evilhealer`
  (body set indirectly), the **camp meta-spawners** `Orccamp`/`Ratcamp`/
  `LizardmenCamp` (a `BaseCamp` spawns creatures and tents but has no body of its
  own, so *its* creatures are lost with it), `Ridablellama`/`Forestostard` (mount
  tables / odd casing) and `Shadowfiend` fall through. `TreasureLevel1-4` are the
  loudest "unresolved" names but are not creatures at all — XmlSpawner sub-tier
  tokens. Where a body *does* resolve, `RandomList` keeps only the first, and
  `SetHits`/`SetDamage` are averaged.
- **Decoration whose point is a function, not art**, is dropped (`SKIP_DECO`):
  teleporters, blockers, warning/hint items, traps, levers, obelisks, serpent
  pillars. Placing the graphic as scenery would show a tile the client draws as
  nothing; the teleport destination, blocking volume and trap trigger are lost,
  not just the art.
- **Containers** are placed **empty** (no loot), and a container graphic not in
  the seeded gump table falls back to the plain wooden-box gump `0x3C`.
- **Signs** place the board art; the localized **cliloc text** is read past and
  discarded (a later slice).
- **Vendors**: town NPC types with no vendor class and no shop are skipped — which
  is where the quest NPCs (escortables, the Bard-Mastery knights) land today until
  `quests` claims them. Expansion-gated (`Core.AOS`/SE/SA) shop items are dropped
  (this is a pre-AoS shard), and `SBMage`'s scroll stock is circles 1–3 only, as
  ServUO ships it.
- **Notoriety** is a karma-sign heuristic (`Karma < 0` → enemy-orange, else grey),
  not ServUO's full alignment/fame computation.
- **Door generation** skips a town whose decoration bbox exceeds `MAX_DOOR_REGION`
  (350k tiles), so a stray far-flung entry can cost that town its generated shop
  doors rather than make `op_generate_doors` sweep millions of tiles.

The bridge is both event- and tick-driven now: the server calls the script's
`onEvent` with each tick's domain events, and the per-mobile `onTick` for every
mobile a script controls (`op_control`, the `Scripted` marker) — the hook the
benchmark priced. The script vocabulary — the events in, the commands out — grows
one gameplay area at a time, each new command mapped in `into_world`.

The balance data comes from the SphereServer scriptpack (`Scripts-X`): `items/`,
`skills/`, `spells/`, `npcs/`, `crafting/`. Numbers taken, arithmetic audited —
the same bargain as everywhere else Sphere is read.

## 7. Scriptpack conversion — dropped with §5

It was a one-shot `.scp` → TS/TOML converter: read a SphereServer scriptpack once,
emit content a shard could edit as normal source. It made sense while the
destination was TypeScript on an embedded V8.

There is no TypeScript now. The destination for ported content is `data/*.json`
compiled by a `build.rs`, and the one conversion this project actually did —
ServUO's tables into `crates/*/data/` — was done with throwaway scripts whose
output was reviewed by hand and committed, which is what this section was really
asking for.

If a `.scp` pack is ever converted, the shape to copy is the migration's:
convert into JSON, put it behind a `build.rs` that rejects what the data cannot
say about itself, and prove it against the source with a test that compares
`Command`s. **`crates/server/server/src/content.rs` was that test's home**, and
`git log` has all eight of them.

## 8. Operations

- [x] `config` — TOML, validated at load
- [ ] `metrics` — tracing, Prometheus, health endpoints
- [ ] `plugins` — manifests, lifecycle, enable/disable
- [ ] REST API + JWT
- [ ] `tools/dashboard` — Next.js admin panel
- [ ] `tools/launcher`, `tools/map-editor`

### Licensing — backlog

The repository shipped a GPL-3.0 `LICENSE` while `Cargo.toml` declared
`MIT OR Apache-2.0`, so every crate's metadata contradicted the file for as
long as both existed. Resolved in favour of the metadata; the reasoning is the
`## Licence` section of the README. Two things it left open:

- **A licence gate in CI.** Nothing currently notices when a dependency
  arrives under terms the workspace cannot take. `cargo-deny` with a `[licenses]
  allow` list is the usual answer, and it belongs beside the three commands CI
  already runs. Today's audit of the tree, for the record: no dependency is
  copyleft-only except `cooked-waker` (MPL-2.0, pulled in by `deno_core`);
  `self_cell` offers `Apache-2.0 OR GPL-2.0-only` and `r-efi` offers an MIT
  option, so both are takeable, and no package is missing a licence field.
- **The MPL notice on a binary release.** MPL-2.0 is file-level copyleft and
  §3.3 explicitly allows a Larger Work under other terms, so `cooked-waker`
  constrains nothing about our own licence — but a distributed binary still owes
  its recipients the notice and an offer of that crate's source. Whatever builds
  the release artefacts should generate a third-party notices file rather than
  leaving this to be remembered.

### Stopping a shard — the mechanism and the manners are done

A shard stops on one `gateway::Shutdown`, cloned into the accept loop, every
connection task and the tick; `run_shard` returns only once the last snapshot is
written. The design and the order of events are in
[`docs/client.md`](client.md), under "Stopping is one word".

**The manners are a plan of its own: [`docs/shutdown.md`](shutdown.md), S1–S6,
all in.** `SIGTERM` asks rather than kills, so a shard under systemd saves; a
second signal is a force-exit for an operator whose store has wedged; bytes
already queued reach the wire before the hang-up; the player is told why; a gate
that has been asked to stop serves nobody; a shard thread that dies in a test
fails that test; and the claim the whole tail exists for is asserted against a
real SQLite file rather than believed.

What is left there is S7 — an operator's stop from inside the world, a GM command
with a countdown — and the plan's own backlog, which is where the next session in
this area starts.

## 9. The client — planned, see [`docs/client.md`](client.md)

Our own client, starting with the only part that has to exist either way: the
protocol in the direction a client reads it, and a `crates/client/net` that
connects, logs in and walks into the world. The milestones, and what is already
missing for each, are in [`docs/client.md`](client.md).

- [x] M0 — `server_packet_length`, `frame_server_packet`, incremental Huffman,
      and `ServerPacket::decode` for the login set. `ClientPacket::encode` and
      the rest of the decoders land as a milestone needs them.
- [x] M1 — `crates/client/net`: sans-io connection, login state machine,
      `WorldView`, and `crates/e2e` proving a client reaches the world against
      the real shard
- [x] M1a — walking
  - [x] The decoders that fill a `WorldView`: `0x20`, `0x11`, `0x77`, `0x78`,
        `0x1A`, `0x1D`. `WorldView` now holds every other mobile and every
        ground item, not just the player; `0x11` decodes but is not folded in
        — see `docs/client.md`.
  - [x] `0x02` with its sequence and fastwalk key, `0x22`/`0x21`.
        `client_net::walk::Walk` sends the steps and predicts where they land,
        because a `0x22` carries no position and only this end knows what the
        acked step was asking for. Two rules are shared with the server rather
        than written twice, which is the part that would have desynchronised
        silently: `movement::intend` (a turn is a whole step, and the world
        edge is not a tile) and `movement::StepCounter`, the client half of
        the sequence rule `WalkSequence` enforces — open at zero, skip zero on
        the wrap, back to zero on a `0x21`. `crates/e2e` walks a burst past
        the pace budget on purpose and compares the position the resulting
        `0x21` carries against the one the client derived on its own; the
        refusal is the only packet that ever states the server's own answer.
- [ ] M2 — `crates/common/uofiles`: move the format readers out of `world`, add
      the ones a renderer needs
- [ ] M3 — the first picture
- [ ] M4 — the gump layer
- [ ] M5 — interaction

### Backlog from the shop that disconnected the player

Found by playing: saying "buy" to a shopkeeper drew no trade window and left a
session in which nothing further worked — the paperdoll would not open. Two
defects, one of each kind the seam has, both now fixed
(`a_shop_says_nothing_the_client_cannot_read` in `world`'s tick tests is the
oracle):

- **The framing table is the authority for every byte a shard writes, and one
  packet was not in it.** `0xD6`, the property list, is written as raw bytes by
  `PropertyList::finish` and named by no `ServerPacket` variant, so nothing in
  the enum would ever have added it — and `open_shop` sends one per stocked
  item. A length the client does not know is not a packet skipped but a
  connection ended (`Connection::poll`), which is why the *paperdoll* looked
  broken afterwards: there was no shard left to answer it. The client's own
  test used `0xD6` as its example of "an id the shard never sends", so the
  assumption was written down twice and true nowhere.
- **A decoder missing where an encoder exists is silent.** `0x2E`, `0x74`,
  `0x9E`, `0x27` and `0x6C` had `EncodePacket` and a row in the table but no
  arm in `ServerPacket::decode`, so `WorldView`'s vendor fold — `vendor_stock`,
  `pending_vendor_buys`, `vendor_buys` — could never run outside its own unit
  tests. The window opened over an empty shelf while every byte of the
  catalogue had arrived. **Worth a sweep**: nothing today asserts that a
  variant this engine *sends* is a variant the client can *read*, and the
  remaining unread ones should each be a decision rather than an omission.
  `0xDC` and `0xD6` were two of the four named here, and both were exactly this
  shape — an encoder, a table row, and no arm — for as long as the entry stood;
  they were read in full on 2026-08-15 (see [`client.md`](client.md)'s
  "Tooltips, and the half that was never written"), and finding them again by
  hand rather than by a failing test is the argument for the sweep. `0x14` and
  `0xBF`'s subcommands are still open.

- **A lost shard is indistinguishable from never having had one, and the one
  thing that hides it is the one thing implemented twice.** `Update::Lost`
  writes `world.link = None` and an `eprintln!`, and nothing else
  (`net_command.rs`). `App::walk` then takes its *offline* arm — the map
  viewer's, gated on `link.is_none()` alone — and moves the body locally, so
  the client keeps walking over a dead connection while `open_own_paperdoll`
  returns silently, `say`/`use_object` log to tracing, and `authoritative.view`
  keeps drawing the world as it stood at the moment of the drop. That is
  exactly what made this bug read as "the state changed" rather than as a
  disconnect. The offline fallback wants a reason — *never connected* is a map
  viewer, *lost the shard* is an error — and the loss wants to reach the
  screen, not stderr. **Fixed**, in the shape the three answers asked for:
  `world::Shard` is one field with three states (`Viewer`, `Live`, `Lost`) in
  place of the `Option<Link>` whose `None` meant both of the two that matter,
  so `App::walk`'s offline arm, `start_replay`'s guard and the scenario panel
  all ask *is this the viewer* rather than *is there a link*;
  `WorldView::shard_lost` puts out every table the shard authored and writes
  the reason into the journal, which is the one thing it keeps; and the status
  strip reads the loss off `Shard` instead of going on saying "in world".
  Left open: nothing reconnects, so the only way out is a restart.

Still not built, and not a defect: the shop *interface*. What draws now is an
ordinary container window over gump `0x0030` with the stock icons in it — no
price column, no quantity, no Buy button, and `link::Link::buy`/`sell` have no
caller (the compiler says so). `0x0030` is a marker in the reference client
rather than container art; drawing the real shop gump is its own piece of work.

### Backlog from the client newtype sweep

A pass over `crates/client/{app,artscan,net,pathtrace}` (`render` excluded on
purpose — its own newtype pass is separate work) for bare numeric fields that
carry domain meaning. The strongest cases are places where a newtype the
protocol already defines (`Serial`, `Graphic`, `RawGumpId`, `RawSwitchId`) gets
unpacked back to a primitive just to cross a struct boundary — fixed below.
What is left is lower-priority: no existing type to reuse, or the fix reaches
into a struct with enough call sites that it deserves its own pass rather than
riding along with this one.

- ~~**`app::shell::Hud` re-flattens `Serial`/`Graphic` into tuples of
  primitives.**~~ Fixed: `mobiles`/`items`/`serial` carry `Serial` and
  `Graphic` directly; `lib.rs` no longer calls `.raw()`/`.0` just to build the
  HUD snapshot.
- ~~**`app::gump::Windows` keys its maps on bare `u32`.**~~ Fixed:
  `by_dialog` and `placement`'s parameter use `GumpId` — the type
  `OpenGump::gump_id` already is, one field over — and `switches` uses
  `RawSwitchId`, what a layout's own `Switch::id` already is.
- ~~**Three copies of an implicit `Axis` enum in `pathtrace`.**~~ Fixed:
  `aabb.rs`, `vector.rs` and `camera.rs` each had their own `usize` 0/1/2 with
  a `match`-and-`panic!` on anything else. `pathtrace::Axis` replaces all
  three.
- ~~**`net`'s undecoded-packet id is a bare `u8`.**~~ Fixed: `PacketId` in
  `connection.rs`, used by `Event::Undecoded` and `LoginError::OutOfTurn`.
- ~~**`net::view::Item::amount` is the one untyped field next to
  `graphic`/`position`/`hue`.**~~ Fixed: `StackAmount`.
- ~~**`app::shell::PickedTile`, the coordinate half** — `x`/`y: u16`.~~ Fixed:
  the two fields are one `at: openshard_movement::Tile` now — "a tile's column
  and row, with no height", already the argument type of `Terrain::{ground_z,
  land_tile, statics_at, stand_z, spawn_z, can_fit}`, so this was class A and
  not a new type. Every reader (`tile_ring`, the two HUD panels,
  `draw_tile_highlight`, `App::{pick_tile, tile_info, walk_toward_cursor}` and
  the click handler) reads `.at.x`/`.at.y` now instead of two loose fields.
- ~~**`app::shell::PickedTile`, the graphic half.**~~ Fixed: `land` is
  `Option<Graphic>` and `statics` is `Vec<(Graphic, i8, Hue)>` — the types the
  neighbouring `Hud::mobiles`/`items` already carry, and no new type needed.
  The values come out of `openshard_map::map::{LandCell, StaticItem}`,
  which hold bare `u16`s of their own; `uofiles` is `common/`, so typing the
  format reader stays a separate decision and `App::tile_info` is the boundary
  the wrap happens at. The two HUD formatters destructure (`Some(Graphic(id))`,
  `for &(Graphic(id), Height(z), Hue(hue), PriorityZ(priority_z))`) rather than
  reading `.0` inline: a panel printing an id in decimal *and* hex is the
  presentation seam, the same licence the wire and SQL get. `statics` since
  gained a fourth element, `PriorityZ` (below), and `PickedTile` gained
  `tile_depth: TileDepth` and `mobile_order: Option<Order>` — the pair the Tile
  panel already read against each other in words, now typed the same way.
- ~~**`app::shell::PickedTile`, the Z half** — `land_z` / `stand_z` /
  `corners` / `levels` / `ceiling: i8`.~~ Fixed: `shell::Height(pub i8)`. The
  narrowing (`z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8`) that used
  to run four times with its own copy of the "a corrupt block must not panic a
  HUD" comment still runs once per site — `Height` did not collapse that
  duplication, it named what the clamp was producing. `Point`, `Terrain` and
  every wire value keep their bare `i8`/`i32`: `Height` is unwrapped (`.0`) at
  exactly the two seams that meet them — `App::tile_info` building the struct,
  and `draw_tile_highlight`'s `at` closure building a `Point`. This contradicts
  nothing in `protocol_newtypes.md`: N1 amendment 2 allowlists `Point`'s
  components because *nothing reaches them except through a `Point`*, and
  `PickedTile`'s height fields are exactly the free-floating case the note
  there flagged — read by two panels and a painter, independently of any
  `Point`.
  Two more depth-sort newtypes came out of the same pass, both local to
  `shell.rs` rather than reused from `client_render::depth`: `TileDepth(pub
  i32)` for `PickedTile::tile_depth` (the `x + y` half of a draw-order key,
  alone) and `PriorityZ(pub i32)` for a static's own sort key inside
  `PickedTile::statics`. `mobile_order` reuses `depth::Order` itself rather
  than getting a third — its two fields are exactly `Order`'s `tile` and
  `priority_z`. `depth::Order`'s own fields stay bare `i32`, which is now the
  one visible seam: a mobile's sort key crosses into `PickedTile` typed, a
  static's does not, because nothing paired the two at the source. Worth
  closing if `Order` itself ever takes `TileDepth`/`PriorityZ` fields — not
  attempted here, since that reaches every caller of `Order` across the render
  pipeline, not just the HUD.
- ~~**`(u16, u16)` is the client's ad-hoc `Tile`, in ten remaining places.**~~
  Fixed: the tuple is `openshard_movement::Tile` now in
  `app::steer::{Steering::goal, go_to, plan}`, `Opening::at`/the command-line
  parser, `App::{in_bounds, tile_info, route_shown, hud}`, `app::dst`'s test
  walls, and `net::walk::Walk::step`'s height callback (`Fn(Point, Tile)`).
  `Point` still names tile-plus-height, and the new `.x`/`.y` reads sit at the
  existing seams: `WorldMap`/`MapTerrain` APIs, `Point::new`, and HUD
  presentation.

  `app::clutter` was the sharpest of them and is **fixed**: it *imported*
  `Tile` on line 41, used it in six trait methods, and then unpacked the `Tile`
  it was handed into `self.clutter.blocked_at(tile.x, tile.y, z)` to feed its
  own `HashMap<(u16, u16), _>`. `Clutter::tiles` is keyed on `Tile` now and
  `blocked_at` takes one, which deleted that unpacking outright — the protocol
  sweep's N2 amendment 1 result ("wrapping deleted `.raw()` calls"), arrived at
  from the other direction. Note what it did *not* need: no `Point` → `Tile`
  helper was added, because `Tile::new(p.x, p.y)` is already the idiom
  `movement::path` and `movement::terrain` use, and a second spelling of it
  would be the thing to avoid.
- ~~**`App::hud` takes two `Option<usize>` indices into different lists,
  positionally.**~~ Fixed: `ItemIndex` and `MobileIndex` travel separately
  through the picked-frame facts and `assemble_geometry`; `App::hud` now takes
  the named `Pick` snapshot rather than either positional index. Swapping an
  item and a mobile no longer compiles.
- ~~**`render::mobiles::Mobile::body` / `app::crowd::Tracked::body` were
  `u16`.**~~ Fixed: both are `Graphic` now, and the `Graphic` → `u16` → `Graphic`
  round trip through `crowd::Crowd::see`/`snap` is gone — `app` carries
  `Graphic` straight through. Also fixed as part of the same pass:
  `EquipConv::resolve(body: u16, item_anim_id: u16)` — the exact
  same-width-adjacent-params shape `docs/style.md`'s newtype section uses as
  its worked example — now takes `Graphic` and a new `AnimId` (below);
  `mobiles::EquipmentLayer::graphic` and `paperdoll::{Wearer::body,
  gump_of, body_gump}` followed the same body/anim-id split.
  ~~`Wanted::animations: BTreeSet<(u16, u8, u8)>` (lib.rs:792)~~ Fixed:
  animation requests now use the shared `AnimationKey`, whose body and group
  are typed; only the stored direction remains a file-format byte.
- ~~**`app::crowd::Tracked::group: u8`** — an animation group with no named
  type yet. `BodyKind::{standing, walking, running, ...}` all return bare
  `u8` from the same three-numbering table `docs/style.md`'s "three
  enumerations, same number means three different actions" comment already
  warns about — a `Group` newtype here would be a `BodyKind`-scoped one, not
  a global animation-group id.~~ Fixed: `AnimationGroup` now names the
  body-specific value throughout `BodyKind`, `Crowd::Tracked`, `Mobile` and
  `AnimationKey`; raw bytes remain only at protocol/file boundaries.
- ~~**`openshard_uofiles::tiledata::AnimId(pub u16)`** — new, this pass: the
  worn-item picture in the body-animation index space
  (`StaticTile::anim_id`, `EquipConvEntry::graphic`,
  `EquipmentLayer::graphic`), split out from `Graphic` because
  `paperdoll.rs`'s own module doc already named it a third, unrelated index
  space that `Graphic` was being reused for.~~ Fixed, and followed all the
  way into the atlas: `FrameKey::body`, `AnimAtlas`'s `asked` set and
  `build`/`add`'s `wanted` iterator, `AnimAtlas::frame_count`, and
  `Anim::{frames, has_frames}` (`common/uofiles`) all take `Graphic` now
  too, and `Wanted::animations` (lib.rs:926) followed since it feeds the
  same atlas. `animation_body` no longer opens back to `u16` at any of its
  call sites — `mobiles::place`, `needed_animations` and
  `App::advance_groups`'s `frame_count` lookup all carry `Graphic` straight
  through. What is left raw on purpose: the file-format bounds check inside
  `Anim`; `AnimationKey` now carries named `AnimationGroup` and
  `AnimationDirection` values, and `FrameKey` adds `AnimationFrameIndex`.
- **`app::desk`** — `Frame`'s `x`/`y`/`width`/`height` are physical window
  pixels, `Panel`'s are logical egui points; same shape, different unit, no
  type keeps them apart. Low priority — it's window-chrome geometry, not game
  state, but it is exactly the space-mixing `docs/style.md` warns about.
- ~~**`app::gump` held page and text-field identities as bare integers.**~~
  Fixed: `GumpPage` and `TextEntryId` carry them through the dialog state;
  `.raw()` occurs only at the renderer-layout and reply-packet seams. They
  remain local because neither name describes a protocol-wide domain yet.
- ~~**`net::walk`'s unanswered-step tally was a `usize`.**~~ Fixed:
  `InFlightSteps` names `Walk::in_flight`, `MAX_IN_FLIGHT`, and the
  `NotSent::Backlogged` diagnostic together. The internal `draining` count
  remains a separate implementation detail: it counts stale responses after a
  rollback, not the live pending queue.
- **`app::gump::text_color(hues: &Hues, hue: u32)` narrows with `as`.** Its
  body is `hues.get(Hue(hue as u16))` — a wire hue that arrived as a `u32`
  because `GumpLayout`'s builder methods (`label`, `croppedtext`, …) declare
  their hue parameter as `u32`, matching the layout language's decimal
  arguments. The `as` silently keeps the low sixteen bits of anything larger.
  Class A on this end (`Hue` exists); on the `protocol` end it is the same
  shape as the four `u32` cliloc parameters on `GumpLayout` that
  `protocol_newtypes.md`'s N-gump backlog already names, and probably wants
  fixing there rather than here.
- ~~**`pathtrace::Image::visibility(x: u32, y: u32, light: usize)`.**~~ Fixed:
  `ImagePixel` now names an image-grid coordinate and `LightIdx` names the
  light-list index, so the image owns the only bounds check over both. The
  tracer tests and renderer oracle carry both types instead of positional
  integers.
- ~~**`pathtrace`'s `width`/`height` travel as two loose `u32`s.**~~ Fixed:
  `trace::ImageSize` now crosses the tracer's public `render` API, lives in
  `Image`, and follows the renderer-side `Mirror`/oracle `Frame` all the way
  through comparison. The raw pair stops at the GPU and PNG seams, where those
  APIs require it. Not per-axis newtypes: the precedent is `MapSize` (N1
  amendment 3 of `protocol_newtypes.md`) — one named pair, because half a
  resolution is not a smaller number, it is a frame of the wrong shape.
- ~~**`app::desk::Desk::fits` throws away the struct it is about.**~~ Fixed:
  `desk::Monitor` names a screen's physical rectangle from winit through the
  saved-frame visibility check. It is deliberately distinct from `Frame`:
  monitor bounds are an outer physical rectangle; a saved frame is an outer
  origin plus an inner window size. Same low priority as the `Frame`/`Panel`
  unit-mixing item above, and the same pass.
- `crates/client/artscan` had no candidates — its public API is already fully
  typed. Re-checked in this pass: still true.

### Backlog from the server/common/render newtype hunt

A pass over `crates/server/*` and `crates/common/{entities,movement,config,metrics,uofiles}`
plus `crates/client/render` (the crate the client sweep above excluded on
purpose) for the same class of gap: an id or index that already has a name
somewhere and is bare where it crosses a boundary. `entities`, `config` and
`metrics` came back clean — `entities`'s newtypes already follow house style
throughout, `config`'s bare integers are gameplay quantities the protocol
sweep's own ALLOWLIST precedent already excludes, and `metrics` is an
unimplemented stub.

The single largest finding is out of scope for one pass and now has its own
living plan, [`facet_newtype.md`](facet_newtype.md): **`Facet` —
`protocol::world::Facet(pub u8)` — is typed correctly in exactly the places
`world::tick::command` already uses it, and a bare `facet: u8` everywhere
else**, which by grep is upward of eighty signatures across `ai`, `npc`,
`items`, `world`, `magic`, `scripting`, `skills`, `state` and their tests.
This is the same shape and the same scale as the `protocol` crate's own
N1–N10 sweep, and wants the same treatment: a dedicated multi-session pass
with its own machine-checked coverage, not a slice riding along with
something else. `persistence::record`'s bare `facet: u8` fields are not part
of that count — they are the disk boundary, where `.0` is expected to surface
once the fields above it carry the type. **Pilot landed:** `ai::lib.rs` (7
occurrences) plus its callers in `npc::live.rs` and `quests::progress.rs` — see
`facet_newtype.md`'s "Amendments forced by the pilot" for what a
single-crate occurrence count misses.

Fixed in this pass, each contained to one or two files and verified with a
full `cargo check`/`test`/`clippy`/`fmt` of the crates touched:

- ~~**`state::harvest`'s two index spaces shared one `usize`.**~~ Fixed:
  `HarvestVein::primary`/`fallback` (index into a definition's `resources`)
  and `Bank::vein` (index into its `veins`) are `ResourceIdx`/`VeinIdx` now —
  two different lists of different lengths, previously indistinguishable at
  a glance and only bounds-checked by two tests that happened to be right.
  `skills::handlers::harvest::{bank_vein, choose_resource}` carry the type
  through instead of re-losing it one file over.
- ~~**`items::trade`'s active-trade index was a bare `usize` in eleven
  functions.**~~ Fixed: `TradeIndex`, local to `trade.rs` — every external
  caller already goes through `cancel_for`/`cancel_all_trades`/
  `validate_trades`, none of which took a raw index, so the type stops at
  the crate's own door with nothing to convert at a boundary.
- ~~**`quests::events::QuestObjectiveUpdated::objective` was a bare `usize`
  crossing the event bus into scripting.**~~ Fixed: `ObjectiveIndex`, next to
  the event in `events.rs`; `progress.rs`'s three near-identical
  advance/refresh/deliver blocks all build and pass it the same way now.
- ~~**`client_render::light::Reach::light` was a bare `usize`** — the same
  open shape `pathtrace::Image::visibility`'s `light: usize` still is,
  below.~~ Fixed: `LightIdx`. The sun's own `Reach` deliberately carries one
  past the end of `Lighting::lights`, which is exactly the kind of fact a
  bare integer does not say and a named type's doc comment does.
- ~~**Scripting discarded `GumpId` at the JS seam.**~~ Fixed: the scripting
  event, `ShowGump`/`CloseGump` commands and serde `GumpSpec` carry the
  protocol `GumpId`; its transparent serde representation stays the same
  JSON number, while the direct `op_close_gump` fast argument remains raw and
  wraps at the operation boundary.

Still open, ranked by how strong the case is:

- ~~**`Skill` (`state::skill`, with `.id()`/`from_id()`) is unwrapped at its own
  component.**~~ Fixed: `state::components::Skills`'s three maps are keyed by
  `Skill` now (it gained `PartialOrd`/`Ord`, matching `id()` order, so
  `ids()`'s `BTreeSet` needed no other change), and `get`/`set`/`lock`/
  `set_lock`/`cap`/`set_cap`/`entries` all take or return `Skill` in place of
  the byte. The wrap the entry named — discarded at the first call in nearly
  every reader — is gone from `skills::{lib.rs, check.rs, button.rs, stats.rs,
  handlers/*.rs}`, `combat::{weapons.rs, lib.rs}`, `crafting::{chance, consume,
  craft, smelt}.rs` and `magic::{lib.rs, spells.rs}`. `state::runtime::
  TargetPurpose::{Skill, SkillSecond}` carries it too, which is what let
  `skills::handlers::mod.rs`'s dispatch chain (`start`/`on_target`/
  `on_second_target`/`on_item_target`/`raise_cursor`) stop re-deriving a
  `Skill` from `Skill::from_id` and immediately discarding it back to the byte
  it was — the sharpest instance the original finding described. What stays
  bare, each promoting `Skill::from_id` at the one seam that first reads it
  (`skills::set_skill`/`set_skill_cap`/`use_skill`, `magic::cast_spell`, and
  the `world`/`scripting` command-queue fields those read from): the
  `Command`/`ScriptEvent` boundary, same shape as N3's "the queue is a
  delivery, not a checkpoint" — asserted by
  `crates/server/state/tests/skill_bare_fields.rs`.
- ~~**`Direction` (`protocol::direction`, with `from_bits`/`to_bits`) is
  unwrapped through `ai`'s pathing core.**~~ Fixed: `step_toward`, creature
  and pet beats, NPC routines, escort progress, `World::step`, and
  `ChasePath::steps` now carry `Direction`; only the external `Command::Step`
  boundary promotes its wire byte.
- ~~**`Notoriety` (`protocol::mobile`) is unwrapped in `npc::spawn::SpawnSpec`.**~~
  Fixed: the spawn, scripting, persistence and component paths carry
  `Notoriety` to their protocol or JSON boundaries.
- ~~**`DamageType` (`state::components`) is unwrapped in the component that
  names it.**~~ Fixed: `DamageType` lives in `protocol::world`, and ranged
  spawns, attacks, scripting and persistence carry it directly. A ranged
  reach is likewise `Option<RangedRange>`, preserving saved numeric `0` as no
  ranged attack.
- ~~**No `SpellId` exists anywhere in the codebase.**~~ Fixed:
  `protocol::casting::SpellId(pub u16)` is the zero-based identity on the far
  side of `RawSpellId`'s one-based wire number. It deliberately does not know
  Magery's 64-row limit — `magic::info` owns that separate, fallible lookup —
  so the dependency-free protocol type can name a later spellbook family too.
  `SpellRequested`, `RequestCast`, `Casting`, `TargetPurpose::Spell`,
  `Cast`/`SpellCast`, the Magery lookup and the spellbook/scroll paths now
  carry it. The scripting event and command stay `u16` only at the JSON
  serialization seam; `server::scripting` unwraps or wraps there, exactly as
  it does for serials and other typed world values.
- ~~**The animation triple `(body: u16, group: u8, direction: u8)` is
  duplicated four ways with no shared name.**~~ Fixed:
  `uofiles::anim::AnimationKey` owns the file-addressing triple, and the
  renderer re-exports that same type instead of maintaining its own copy.
  `Anim::{has_frames,frames}`, `AnimAtlas` and `needed_animations` now pass it
  whole; `FrameKey` embeds it rather than exposing three public bare fields.
  `Mobile` still keeps its wire body, action group and a typed *facing*: it is
  not a stored-file triple until `facing()` resolves the mirror, which is the
  one point that builds `AnimationKey`. The alleged root calls already use
  `Graphic` and `AnimId`, so they needed no sibling wrapper.
- ~~**`uofiles::map::StaticItem{tile: u16, hue: u16}` unwraps `Graphic`/`Hue`
  at the one struct every static on the map is read into.**~~ Fixed:
  `StaticItem` now carries `Graphic` and `Hue`, while `LandCell` carries the
  distinct `LandTile` newtype for the other id space. `movement::Terrain` and
  its map-backed implementations carry those names through their land/static
  queries; `.0` remains only at tiledata, map-file and deliberately raw
  compatibility boundaries. This makes a land id and a static graphic
  unassignable to each other at an ordinary call site.
- **`state::harvest`'s sibling gap, `items::trade`'s sibling gap and
  `quests`'s sibling gap all had one thing in common that a fourth case does
  not yet: `client_render`'s `Option<usize>` "index into `items`"**, repeated
  identically across `frame.rs`, `items.rs` (twice) and `mobiles.rs` (twice).
  Fixed: `ItemIndex` and `MobileIndex` now travel through render and app APIs,
  with `.raw()` only at list/serialisation boundaries. The separate
  picture-index half is also fixed by `PictureIndex` across
  gump/paperdoll/skills.
- ~~**`(u16, u16)` was `render`'s ad-hoc `Tile` in five places** —
  `debug::around`, `scene::{room_wall_tiles, DOORWAY}`, `select::{Selection,
  Selection::on}`.~~ Fixed: render now uses the shared `movement::Tile` across
  its scene and selection APIs; the old `SceneTile` wrapper and its tuple
  constructors are gone.
- ~~**`occlusion::bvh::Leaf::first: u32`** indexes `Bvh::order`, right beside a
  `NodeIdx` whose own doc comment already argues "a place in the primitives
  ... is a different list" from a node index.~~ Fixed: `OrderIndex` names the
  position in the permutation, and `.raw()` appears only at slice indexing and
  the packed GPU seam.
- ~~**`pathtrace::Image::visibility(x: u32, y: u32, light: usize)`**~~ Fixed:
  `pathtrace::trace::ImagePixel` now names the image-grid coordinate and
  `pathtrace::light::LightIdx` names the light-list index; `.raw()` appears
  only at the image buffer seam. The pathtrace oracle and its render tests
  carry both types instead of positional integers.
- ~~**`uofiles::animdata::sequence(graphic: u16)`**~~ Fixed: the parser now
  accepts `Graphic`, matching the static animation API around it; `.0` is only
  used for the file-table offset.
- ~~**`impostor::Volume::of(..., solid: u32)`**~~ Fixed: `Volume::solid` stays
  `Option<SolidId>` until the GPU-byte boundary; three
  `opaque_at(&self, ..., x: u16, y: u16)` picture-local pixel coordinates sit
  bare next to a crate that otherwise names every other pixel space
  (`WorldPixel`, `ViewPixel`, `RealPixel`, `GumpPixel`).

### Backlog: a gump dialog's own captions still can't draw Cyrillic

`--ttf-font`/`OPENSHARD_TTF_FONT` (`fonts.mul` has no glyph past `0xFF`, so no
Cyrillic) now covers the speech line and the journal — `Screen::ttf_gump_pass`
in `crates/client/app/src/lib.rs`, drawing through
`openshard_client_render::text::collect_gump_ttf` — and overhead speech
already went through `Screen::ttf_pass`/`text::collect_ttf` before that. A
server-opened gump's own `{ text }`/`{ croppedtext }` captions did not move:
they still draw through `Screen::gump_text_pass`/`text::collect_gump` and
`App::font_atlas` unconditionally, so a shard whose gump layouts carry
Cyrillic (an NPC's name over its head is one thing; a vendor's whole buy
window is another) would still lose that text to the same silent
byte-outside-the-table skip `text::collect`'s doc already names. Lower
priority than the chat box was — a layout is usually authored by whoever
scripts the shard, in whatever script the client already draws, where a typed
chat line is the one text box a *player* fills in and expects to read back —
but the same switch (`atlas.add` the layout's own strings, draw through
`ttf_gump_pass` instead of `gump_text_pass` when `ttf_font` is set) is what
closes it.

`collect_gump_ttf`'s baseline is also an approximation, not a measured face
metric — see `BASELINE_SHARE`'s doc in `text.rs` for why (this crate never
reads an `hhea` table) — worth revisiting if a real TrueType face ever reads
visibly off its line.

### ~~Backlog: the Chat tab's size knob only reaches the classic face~~ — built

The HUD chat box (journal + compose line) has a Chat tab in the dev window
(`desk::Chat`, `desk::ChatScale`, `shell::chat_panel`): an integer upscale on
`fonts.mul`'s own glyph quads (default 2×, `App::draw`'s `scaled_gump_quads`,
nearest-sampled the same way a camera zoom step grows a world sprite), and a
hue that tints the player's own compose line and caret without touching a
journal row's own server-sent hue.

That knob only ever reached the classic path, and this entry said a real one
for the TrueType path "would have to grow the atlas's own rasterization
height instead of the finished quad — a second, differently-shaped feature".
That is what `docs/text_sizes.md` built: `TtfAtlas` is keyed by
`(char, TextSize)` rather than baked at one height, so every kind of text has
a **real pixel size** of its own (`desk::FontSizes` — speech, window, tooltip,
stack count), fractional, rasterized at that size and never stretched. The
Chat tab's TrueType half is four pixel sliders now; `ChatScale` stays an
integer and stays `fonts.mul`-only, because a bitmap face has no continuous
size to ask for.

### The reopening window, and the overlay that replaced the patch — see `client_window_state.md`

A locally-closed container, paperdoll or dialog reopened itself a beat
later (2026-08-11): `App`'s own copy of `WorldView` learned of the close,
the shard thread's copy did not, and the next packet that changed anything
nearby cloned the still-open copy over it. First patched with
`link::Command::CloseWindow` alone — the shard thread's copy told too, but
two mutable copies of "what is open" kept in step by remembering to write
both, not by construction. Built out the same day into the honest fix:
`App::locally_closed`, a prediction-and-reconciliation overlay mirroring
`link::Body::predicted`/`corrected` one layer down — `App` no longer writes
its own `view` locally at all, closing sets the overlay and sends the
command, and `reconcile_own_windows` (pulled out of `sync_own_windows` so it
is testable without a real `App`) clears an entry only once a fresh
snapshot agrees the subject is gone.
[`client_window_state.md`](client_window_state.md) has the decision record
and the test that reproduces the original bug.

### The route a Ctrl-drag draws, and what is left around it

Built: `steer::plan` reads the ground twice (`steer::Readings` — the map with the
shard's items over it, and the map alone), so a destination sealed off by
something placed is planned *up to* that thing rather than answered "no route";
and where neither reading has a way through, `movement::find_path_toward` plans
as far toward it as the ground goes. **A destination now never asks for a step
this end can already see refused** — the straight-line fallback that used to
shove at a wall until a patience ran out is gone, and every one of those steps
was a `0x21` and a rollback. The walk takes the open half and stands at the
obstacle; the client draws the whole plan green up to it and red past it,
whether or not the terrain overlay is switched on (`App::route_shown`,
`shell::draw_route`). What is left:

- ~~**This end cannot tell a door from a crate.**~~ It can, and now does. The
  fact was already in the tree: `client/render/src/doors.rs` carries ServUO's own
  door families (`data/doors.json`), which is what `clutter.rs` now asks — so a
  blocker is marked `door`, the tiles the shut ones stand on are the list of
  "potentially passable, currently closed", and `Cluttered` reads either as the
  world stands or with every door open. The two readings differ by exactly that
  list, which is what makes the red half of a drawn route mean *a shut door*
  rather than "something the shard placed". The wire needs no new flag.

  **It found a bug on the way in.** `clutter.rs` used to argue that no door state
  had to be tracked, because a door's graphic changes when it swings and only the
  shut leaf is impassable. Measured against the real `tiledata.mul`: all 164 shut
  leaves in the table are impassable, and **so are 132 of the open ones** — so
  this end was refusing to walk through open doors, steps the shard allows. An
  open leaf is now left out of the index entirely.
- **Nothing opens the door.** The classic client's answer to arriving at one is
  the player's double-click, and that is still the whole of ours. A walk that
  ended in front of a shut door could reasonably send the `Use` it already knows
  how to send (`link::Command::Use`) — deliberately not done here: it is a
  gameplay decision (a locked door, a house that is not yours) and not a
  rendering one.
- **The patience is the ordinary one.** A body standing at a shut door is given
  up on after `STUCK_STEPS` beats like any other stalled destination, so a door
  opened more than about a second and a half later needs a fresh click. Holding
  the order longer would want a reason to believe the door is *about* to open,
  which nothing on this end has.
- **A goal that cannot be *stood on*, in a room whose door is shut, walks to the
  wrong side of the building.** `plan`'s middle step needs a full route over the
  bare map to have something to cut, and a tile nothing can stand on (a table, a
  chest, the wall itself) has none — so it falls through to "as close as the real
  ground gets", which is the outside wall nearest the goal rather than the door.
  Clicking the *door* is fine (the doorway is standable with the leaf gone), and
  so is clicking furniture in a room that is open. Fixing it means cutting an
  approach rather than a route — `find_path_toward` over the bare map, cut by the
  real one — which is a third case in `plan` and was not worth the branch until
  somebody hits it.
- **The preview replans per frame while a destination is live** — the walk plans
  at most once a step, and drawing from its stored route would blink the line out
  on every mouse-move (see `App::route_shown`). Bounded by `PLAN_BUDGET` and paid
  only while there is something to draw, but an unreachable destination pays up
  to three full-budget searches a frame for the second and a half before the
  order ends. If that ever shows up in a frame time, the fix is to cache the plan
  against (body tile, goal, view generation) — not to give the picture a cheaper
  rule of its own, which is how the two would start disagreeing.

### Backlog: the cache guard at a house never fires on the packet path

`App::entered` (`client/app/src/net_command.rs:452`) decides whether to throw
away the route, plan, terrain and occluder caches by comparing the incoming view
against the one it already holds:

```rust
let items_changed = self.world.authoritative.view
    .as_ref()
    .is_none_or(|old| old.items != view.items);
```

Its comment says why it exists — "invalidating it unconditionally made the same
expensive plan run on every update (and therefore effectively every frame) at a
house" — and on the path it was written for it cannot work. `apply_packet`
(`:327`) does `self.world.authoritative.view.take()`, mutates the view it now
owns, and calls `entered(*view, …)` at `:394`. Inside, the field is `None`, so
`is_none_or` answers **true** unconditionally; the view is only put back at
`:727`, after the check. Every ordinary packet — a stranger's `0x77`, a line of
speech — therefore clears `terrain_cache`, `occluder_cache`, `route_cache`,
`steer.clear_plan_cache()` and `steer.clear_route()`.

Of the three callers only `reproject_item_drag` (`:71`, which clones rather than
takes) and the `0x1B` path (`:154`) compare against anything.

The comparison is also `O(n)` over the item map, which at a castle's roughly
four thousand locked-down items is not free even when it does run — and it
cannot see an item that moved and came back to the same place. Both go away
together if `items_changed` is derived from **what the packet was** rather than
from a map diff: the mutation path already knows it applied a `WorldItem`,
a `Remove` or an `AddToContainer`, and that answer is O(1) and exact.

### Backlog from the frame-cost instrumentation

The `frames` panel measured a frame with a clock on the event-loop thread, which
can only see half of one. `queue.submit` returns without waiting, so
`Frame::scene` stopped when the *encoding* did and every pass was still ahead;
the device's work reappeared a frame later inside `get_current_texture`, where
`Frame::wait` recorded it under a comment calling it "the pacer working". Under
`PresentMode::Fifo` a saturated GPU and a client asleep on vsync were therefore
the same reading, and the panel could not tell them apart.
`crates/client/app/src/profile.rs` closes that: a timestamp query around each
pass in `App::draw`, a `gpu` row and curve beside `ui`/`world`/`waited`, and a
`puffin` sink on `OPENSHARD_PUFFIN` for the CPU flamegraph. What is left:

- **`Frame::wait` is still one number for two facts.** The `gpu` row now says
  which fact it is, but it says so *beside* the field rather than in it — a
  reader has to do the comparison, and the panel does it for them in a sentence.
  Splitting the acquire stall into "the display held the last frame" and "the
  swapchain had no image because we did" would need something `wgpu` does not
  expose today, which is why it is a sentence and not a field.
- **The GPU number is two or three frames old and is recorded against the
  current one.** Right for a standing cost and wrong for a spike: a repack's own
  frame and the `gpu` reading beside it are not the same frame. The ring would
  have to carry a frame index for the two to be joined up, and nothing yet needs
  it.
- **The scopes are closed by hand.** `profile::begin`/`profile::end` rather than
  the RAII scope `wgpu-profiler` offers, because the guard borrows the encoder
  and every pass in `App::draw` would gain a block. A forgotten `end` is caught
  by `end_frame` and logged, so it is loud — but a scope guard would make it
  impossible, and `App::draw` is overdue a split into per-pass functions that
  would make the block free.
- **Nothing times the CPU below `draw`.** The `puffin` scopes are one span for
  the whole draw. The interesting divisions — `frame::assemble`, the atlas
  growth, `light::collect`, `occlusion::bake` — each want a `profile_scope!`,
  and that is the change that makes the flamegraph worth opening at all.
- **`PresentMode::Fifo` is not switchable at runtime.** Unmasking the true frame
  ceiling means editing `App::create_window` and rebuilding. A flag would make
  "is this vsync or is this cost" a ten-second question; it is currently a
  recompile.
- **The lighting pass is measured twice, by two harnesses that cannot be
  compared.** `crates/client/render/tests/cost.rs` batches it offline with
  `poll(Wait)` and divides down; this measures it in the frame as played. Both
  are right and neither validates the other — the offline one runs the pass
  `REPEATS` times back to back, which is a different cache state from one pass
  among a dozen others.

### The party left egui: a yes/no plate and a manifest — backlog

The two party windows were the last of this client's own interface drawn as
`egui::Window`s over the gump layer, and both are gump windows now: the
invitation is `crates/client/render/src/confirm.rs` on the reference's own
`0x0816` question plate (`panes::confirm`), and the roster is
`crates/client/render/src/party.rs` on the `0x0A28` manifest (`panes::party`).
Both are reconciled from the view — `party.invited_by` and `party.members` — the
way a `0xB0` dialog is, so neither has an openness kept anywhere but in
`Windows::own_windows`. `Link::accept_party`, `decline_party`, `add_to_party`
and `remove_from_party` are gone with them: a pane names `Effect::Net` and never
holds a `Link`. What is left:

- ~~**Three window kinds now carry the same `hit()`.**~~ **Closed.**
  `gump::pick_hit` is that one function, generic over the `Hit` type and over
  whether the caller's index-to-meaning table is a `BTreeMap` or a `Vec` —
  `gump::Window::hit`, `confirm::Window::hit` and `party::Window::hit` are all
  now one line each, calling it.
- **A party member is named by serial, in both windows.** No packet in this
  path carries a name — a `0x78` invitation does not, and the `0xBF 0x06`
  roster does not — so both draw `0x0000002A`. The names this client *does*
  have arrive by single click and by tooltip (`view.paperdolls`, the `0xD6`
  cache), and neither is consulted: a lookup that answered "not yet" for most
  rows would be worse than a number that is always right. Worth revisiting
  when the tooltip cache is keyed for this.
- **Two controls on the reference's manifest have no packet here.** The
  per-member *Tell* buttons address one member and `Outgoing::PartySay` only
  addresses the whole party; the loot-type toggle needs a party-loot request
  `Outgoing` has no arm for. Both are left off the plate rather than drawn dead
  — see the module docs.
- **A question is not modal, and the reference's is.** `QuestionGump` is
  `IsModal = true`; this one is an ordinary window because z-order is the
  manager's (decision 2 in `window_components.md`) and "nothing under me may be
  clicked" would be a second z-order policy living in a pane. If a question ever
  needs to be answered before anything else, that is a manager-level rule and a
  field on `Windows`, not a pane's.
- **Both windows cascade like a bag.** The reference centres its question plate
  on the screen; `reconcile_own_windows` has never been told the surface size
  and deliberately is not. This is the backlog entry every window kind already
  shares — nothing remembers where it was left — and the question plate is the
  one kind where the reference's own answer is *not* "wherever you last put it".

### The keyboard has an owner now — backlog

`Tab` used to enter war mode exactly once per launch. egui's
`egui_wants_keyboard_input` is literally "some widget has the focus", `Tab` is
what hands out that focus, so the first press entered war mode *and* focused a
button in the dev desk — and from the next frame egui claimed every key, war
mode, `Enter` and the arrows included. A self-arming trap: the key that broke
the keyboard was the key that could no longer be pressed.

`crates/client/app/src/keyboard.rs` is the layer that replaced the implicit
ladder of early `return`s inside `App::window_event`: `Owner` names who a
keystroke belongs to (speech line, pane field, world), `Edit` is the binding
table for a line being typed and `Hotkey` the world's own, all with tests that
need no window. egui is handed no `Tab` at all (`egui_may_see`) and may claim the
keyboard only while a text field inside it has the focus
(`Shell::holds_keyboard`) — of which this client has none, every box a player
types into being drawn by `chat.rs` or `panes.rs`.

The speech line completes staff commands as they are typed, from
`openshard-commands` — one table the world dispatches on *and* the client
offers, so a command that runs is a command that is offered and the two cannot
drift: `gm::run` matches `StaffCommand` exhaustively. `Tab` takes the highlight,
arrows move it, `Escape` puts the popup away before the line, and past the
command word the popup becomes the usage hint — and it offers only what this
character's authority lets it run, which the shard says once on world entry.
The channel is a button on the input line, with `Shift+Tab` beside it.

The five entries this backlog was left with are all closed, and what each of
them turned into is worth keeping, because the next thing here will be built on
one of them:

- [x] **The channel is a button, not a chord.** `chat::channel_button` draws it
      at the left end of the input line, on a plate, whether or not the line is
      open; a left click cycles it, ahead of the window layer and the world
      because the chat is drawn over both (`App::press_channel_button`). Its box
      comes out of two functions — `channel_button` and `channel_width` — that
      the frame and the pointer both call, which is `docs/parity.md`'s rule in
      the one place a player can feel it being broken. `Shift+Tab` stays, beside
      it rather than instead of it: a hand already typing should not have to
      leave the keyboard.
- [x] **The world's own hotkeys are a table.** `keyboard::Hotkey` names each of
      the nineteen and `Hotkey::key` says which key it is on; `Hotkey::of` is
      answered *out of* that one table rather than by a second `match`, so a
      forward and a backward reading cannot disagree. `event_loop.rs` is left
      with the doing. The arrows, `Tab` and `Escape` are deliberately not in it
      — two are held rather than pressed and one belongs to the window layer —
      and that is written down on the type.
- [x] **The completer offers only what the shard would run.**
      `openshard_protocol::access::AuthorityNotice` is this engine's own `0xBF`
      subcommand (`0xE001`, in a reserved range no client version and no
      ClassicUO uses), sent once on world entry, and it carries the account's
      `AccessLevel`. The client keeps it on the view and hands it to
      `StaffCommand::matching`, which offers a player nothing — the usage hint
      past the command word included. The threshold itself is
      `StaffCommand::AUTHORITY`, and `WorldState::staff_authority` compares
      against the same constant, so the gate and the completer cannot drift.
      `crates/e2e/shard/tests/staff_authority.rs` is both ends on one wire.
- [x] **The popup's highlight is a plate.** `gump::plate` is the rectangle
      primitive the pass had none of: a quad with no region at all — `du` and
      `dv` zero, which no packed sprite can be — whose `u` carries a `Shade` the
      shader paints through the hue's own ramp. No atlas entry, so it works in
      all three of this pass's uses (gump art, `fonts.mul`, a TrueType face),
      and the chat's furniture is drawn with it.
- [x] **The chat block is cut to the window.** `chat::room_above` answers how
      many rows fit between the input line and the top of the surface, and the
      popup is served first because it is the one a keystroke is moving; the
      journal takes what is left. `Offer::rows` takes that number as a hard cap
      and spends one of its rows on the "… n more" count rather than adding a row
      to it.

What this left behind:

- **The caret is still a glyph.** `gump::plate` now exists, so the `|` the chat
  draws could be a one-pixel bar — which is what a caret is. Not done with the
  rest because it is a *look* rather than a defect, and the width of a caret is a
  decision nobody has argued yet.
- **Nothing draws the bindings.** `Hotkey::key` is the half a key-bindings window
  reads, and there is no such window; the table is rebindable-*ready* and not
  rebindable. What is missing is a place to put it and a file to keep it in
  (`desk::Desk` is the obvious home, `client_ui.toml` the obvious file).
- **The authority notice is sent once and never again.** Right today — an
  account's level does not move while a character is in the world, and `.gm`
  moves the staff *mode* rather than the authority — but a shard that ever grows
  a `.setaccess` would have to send it again, and nothing would notice that it
  had not.
- **A plate is opaque.** The gump pass does no blending, so the chat's furniture
  covers the world under it rather than tinting it. That is the right first
  answer (a highlight has to be readable) and the wrong final one for a chat
  backdrop, which wants to be a wash. Blending is a pipeline decision for the
  whole pass, not a plate's.

Two defects were found on the way and fixed rather than filed, both in code the
work had to touch anyway:

- **The gump pass ran an untinted translucent picture through the hue ramp.**
  `SpriteQuad::hue` carries more than the wire hue — `with_opacity` writes a byte
  into bits 16-23 — and `gump.wgsl` asked whether the whole word was nonzero, so
  a picture with an opacity and no tint took the lookup at index zero, whose row
  is `-1`. An out-of-bounds `textureLoad` answers with zeros: the paperdoll's
  pending-equipment preview drew black. The shader now tests the index bits, and
  `crates/client/render/tests/gump.rs` pins it on a ramp built to fail if the
  lookup runs at all.
- **The chat's caret ignored `desk::ChatScale` on the `fonts.mul` path.**
  `text::gump_width` measures the font's own pixels and `scaled_gump_quads` draws
  them magnified, so an anchor placed at the unmagnified width put the caret a
  fraction of the way along the line it was measuring — at the default scale of
  two, halfway back through what had been typed.

### Found while closing the radar plan's section 9

- **`cargo clippy --workspace --all-targets` is not silent**, though this
  repo's own `CLAUDE.md` says all three commands are. Ten sites, none of them
  radar's: `interiors.rs` (a redundant guard, two `if .. else` chains, a loop
  variable used to index), `items.rs` and `statics.rs` (two functions past the
  argument limit), `world.rs`, `presentation.rs`'s composite-LOD arm and
  `examples/interior_census.rs` (a useless `i32` conversion). They are warnings
  rather than denials, so CI is green and the claim is stale — either the
  warnings go or the claim does.

  > **Those ten are gone**, swept since. What stands in their place is a
  > different, shorter list, and it is the one below under *the last of
  > `navigation_spans.md`'s filed observations* — three files, five findings.
  > The *claim* is still stale, which is the half of this entry that has never
  > been closed.

### Found while taking the last of `navigation_spans.md`'s filed observations

- **Nothing checks an intra-doc link.** `steer.rs` carried `[`Ground::real`]`
  and `[`Ground::through_doors`]` — two fields that stopped existing when the
  pair of terrains became one `Footing` — and both survived every `cargo test`,
  `cargo clippy` and CI run since. `cargo doc` is not one of the three commands
  `CLAUDE.md` names, and `rustdoc::broken_intra_doc_links` is a *rustdoc* lint,
  so nothing fires it. Either the workspace lint table gains it and `cargo doc`
  joins CI, or every `[`Type::member`]` in this repo is prose that happens to
  have brackets round it. They were found by reading, which does not scale.

  > **Counted since, and it is not two.** `cargo doc --no-deps` over
  > `openshard-movement` alone reports **15** — a mix of unresolved links and
  > public docs pointing at private items (`can_step` → `climbed`,
  > `Overlay::blocker_anywhere` from a path that does not resolve). `-state`,
  > `-protocol` and `-ai` each have their own. So the gate is not a tidy-up
  > before it is turned on: whoever adds the lint spends a session on the
  > backlog first, and should decide separately whether
  > `rustdoc::private_intra_doc_links` is wanted at all — a public doc naming
  > the private function it delegates to is often the *right* thing to write,
  > and half of these are that.
- **`cargo clippy --workspace --all-targets` is still not silent**, and none of
  what is left is that session's: `common/uofiles/src/map.rs` (a needless
  borrow), `client/render/tests/traced.rs` (three borrowed expressions that
  implement the trait already), and `client/app/src/link.rs` (a 640-byte
  difference between enum variants). The first three are a parallel session's
  open files, which is why they were left rather than swept.

### Found while pricing `navigation_spans.md`'s baked adjacency

- **The instrument carries its own copy of the rule it measures.**
  `examples/step_cost.rs`'s `expand` helper reimplements the diagonal flank rule
  — the two flanking cardinals of a diagonal, refused together — and **four** of
  its rows now go through it, including the *floor* and *all eight on one column*
  rows the baked-adjacency decision rests on. `steps_out_of` owns that rule, and
  the example cannot call it for the rows whose whole point is to swap one half
  of the expansion out. So a change to the flank rule leaves the example
  measuring the *old* rule and passing: no test fails, and the plan's next number
  is quietly about something else. Same class as
  [`parity.md`](parity.md)'s frame assembled by hand in seven places, one layer
  down. What would close it is `steps_out_of` growing a seam the example can
  substitute into, so there is one flank rule and the harness borrows it.
- **A bench's default is a claim about the machine it was written on.**
  `step_cost --repeat` defaulted to five passes, which is enough on a quiet
  machine; at load average 33 on 24 cores it moved rows by 30% run to run and
  produced a stable-*looking* reading that twenty-five passes do not reproduce —
  and that reading reached `navigation_spans.md` before it was caught. The
  discipline is now a section in the example's own module doc. **The other
  measuring examples were not audited for the same thing.** `coarse_bench` is
  the one that already does the right thing and is worth copying — it prints
  `repeat={}` in its own header, so a number quoted from it carries how it was
  taken. `map_path_probe` has the flag and does not print it; `span_index` and
  `span_census` quote a bake time with no repeat at all, and it is a bake time
  the plans keep.

## Later

LLM NPCs, quest generation, GM assistant, Discord integration. All optional, all
after the engine stands on its own.

## A note on client files

None are in this repository and none will be: they are copyrighted and not ours
to redistribute. `world.client_files` points at an install the operator already
has. Tests that need one read `OPENSHARD_CLIENT` and skip when it is unset.

What this project contains is readers for the *formats*. Nothing is derived from
any particular shard's data, and nothing should be documented as if it were.

### Which client versions to support — see [`client_versions.md`](client_versions.md)

That document holds the evidence: which clients people actually play (7.0.x on
the big shards, 5.0.8.3 on the T2A/Renaissance ones), what changes between
versions in the files and on the wire, and how to obtain a set of files legally.

The backlog it leaves us, in order of size:

- [ ] **`verdata.mul` support.** Mandatory below 5.0.0a and entirely absent:
      `grep -rn verdata --include='*.rs' crates` finds nothing. `uo-rust-libs`
      `src/map/diff.rs` (MIT) is worth reading first for the sibling
      `mapdif`/`stadif` format, whose `*difl` lookup does not announce itself.
- [x] **A version-driven map width.** `MapSize::for_client` (`crates/common/protocol/src/world.rs`)
      clamps Felucca and Trammel to 6144 wide for a client below
      `ClientVersion::WIDE_MAP` (4.0.11d, sourced from ClassicUO's `CV_4011D` —
      Sphere's own `grayproto.h` has no MINCLIVER constant for map width at all,
      so this is not a `Feature`, which every entry of that table is pinned to
      one for). Wired at both places a map size reaches the wire: world entry
      (`0x1B`) and a mid-session facet change (`0x76`), the latter reading the
      traveller's version off the connection row.
- [ ] **The lower half of two protocol boundaries.** `Feature::NewContextMenu`
      (6.0.0.0) gates the *new* `0xBF.0x14.0x02` form, so nothing stops us
      sending the old form to a client with no popup menus at all. Same gap for
      cliloc: `Feature::Tooltips` (4.0.0a) covers OPL, the plain localized
      message `0xC1` has no entry.
- [ ] **The AoS boundary is Sphere's, not the client's.** `MINCLIVER_AOS` is
      4.0.0.0 while the client gained AoS features at 3.0.8z, so every client in
      `[3.0.8z, 4.0.0)` is told it has no AoS support when it does.
