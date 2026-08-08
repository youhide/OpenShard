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

### Backlog: a pier or bridge over low ground can drop a walker under it

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

### Backlog: a mobile is not an obstacle

The step check asks two things — `MapTerrain::check` for the client's files, and
`Obstructions` for what the world has put on top (`state/src/obstruct.rs`,
composed in `LiveTerrain::can_step`). Nothing registers a *mobile* in the second.
Every `Obstructions::block` call is a door (`tick/decor.rs`), a placed impassable
decoration, a restored item (`tick/persist.rs`) or a field spell
(`tick/fields.rs`); no mobile is ever entered or removed. So a player walks
through a standing NPC, a guard does not hold a doorway, and `find_path` plans
straight through a crowd. ServUO blocks here (`Movement.CheckMovement` with
`checkMobiles`), and so does the 2D client's expectation — bodies do not overlap.

Two ways to close it, and the choice is the point:

- **Register mobiles in `Obstructions`.** Cheap to read — the index is already on
  the hot path — but it is a second copy of `Position`, updated on every step,
  every spawn, every despawn and every teleport. That bargain is fine for a door
  that flips twice an hour and much worse for a body that moves three times a
  second; one missed `unblock` is a permanent invisible wall.
- **Ask the sector grid.** `FacetState::sectors` is already the authoritative
  index from tile to entity and is already kept honest by the step itself
  (`tick/motion.rs` writes it beside `Position`). No second copy, and the cost is
  one lookup per step. This is the one to take.

Either way three rules come with it and none are in the code yet: the dead do not
block (a corpse is an item, a ghost walks through), a mobile may always step *off*
the tile it is standing on, and staff walking through bodies is the same
permission as walking through walls — see `gm.rs`, which has no such bypass
either.

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
  for a rare spawn. Registering a region twice replaces it rather than stacking a
  second, and after a restart the regions come from the store, not the pack, so a
  re-populate is not needed and the timers hold.
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

## 5. Scripting — spike done

The largest open technical risk. Proven before building gameplay on top, and it
holds. The engine is `crates/server/scripting`; `engine.rs` explains the seam.

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
    `Amount = body` (the protocol special case that draws the right corpse), a
    container on gump `0x0009` holding the creature's worn gear and a core gold
    drop scaled from its toughness. It decays after seven minutes and takes its
    loot down with it (`items::decay` now cascades into a container's contents, so
    nothing is orphaned). `combat::die` stopped despawning — it announces, `reap`
    disposes. The corpse persists as a ground container; a restored one gets a
    fresh decay timer (the tick is not saved).
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
    graphics would leave four piles that refuse to merge; High Seas' lava tiles;
    and a real **pack-capacity** refusal, since nothing in `items` caps what a
    backpack holds — "your pack is full" fires only when there is no pack at all.
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
    — `.where`, `.go`, `.tele`, `.add`, `.set`, `.admin` — lean on the systems
    that own their rules (`items` spawns, `skills` re-caps the stat) rather than
    reaching into the registry, and answer the actor privately with a `0x1C`
    system line. `.go <x> <y>` jumps to coordinates; `.tele` raises a targeting
    cursor (`0x6C`) and jumps to the tile clicked — Sphere's split, and the
    teleport pushes a `0x20` to the mover's own client so the screen refreshes on
    the spot rather than a step late. The gate lives in the world, not the `gm`
    module, so a command function may assume its caller cleared it. The vocabulary
    grows one verb at a time in `world::gm`.
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
      at the open leaves the deal reachable. The **lines are the
      pack's**, registered per trade by `op_register_npc_speech` — and are
      themselves ServUO-derived rather than invented: the greeting is cliloc 500186,
      the "what is thy trade" answer is built from the title, and "what dost thou
      sell" lists the trade's actual `SB*.cs` stock. The core default is a plain
      greeting, so a bare shard still speaks.
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
- [ ] `housing` — player houses: a multi placed on the map, a door with a real
  lock, decay unless refreshed, friends/co-owners. Wants multis (the client's
  `multi.mul`/UOP format, unread yet), a region concept and the door locks above.
- [ ] `guilds` — membership, titles, the guild notoriety rules (green/orange),
  war declarations. Mostly data plus a notoriety hook; the abstract stub exists
  so the dependency graph already names it.
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
- **Party (`0xBF 0x06`) and chat channels (`0xB3`/`0xB5`).** Group play has no
  protocol surface at all.
- ~~**Pets and taming.**~~ Landed with Animal Taming; see **Taming, and the pets
  it wanted** in §6 `skills`. Still open from that entry: **stabling** (which
  wants a pet saved with no position, the logged-out-character shape),
  **loyalty** (pointless without feeding) and **Herding**.
- **CI.** `.github/workflows` holds a release workflow and nothing that runs
  `cargo test` / `clippy` / `fmt` on a push, though "all three silent" is a stated
  rule of the project. The one gap here that is about the project rather than the
  game.
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

## 7. Scriptpack conversion

- [ ] `tools/cli`: one-shot `.scp` → TS/TOML converter
- [ ] Run it over a scriptpack, review the output by hand

A build-time tool that runs once, not an engine feature. The output is committed
and edited as normal source afterwards — there is no ongoing `.scp` dependency.

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
- [ ] **A version-driven map width.** Felucca and Trammel are 6144 wide below
      4.0.11d. We derive the width from the file, which is right about the file
      and wrong about the client: a modern `map0.mul` served to a 3.0.8 client
      gives a world 1024 tiles wider than the one being drawn. ClassicUO clamps
      by version. Wants a `Feature`-shaped rule and a test.
- [ ] **The lower half of two protocol boundaries.** `Feature::NewContextMenu`
      (6.0.0.0) gates the *new* `0xBF.0x14.0x02` form, so nothing stops us
      sending the old form to a client with no popup menus at all. Same gap for
      cliloc: `Feature::Tooltips` (4.0.0a) covers OPL, the plain localized
      message `0xC1` has no entry.
- [ ] **The AoS boundary is Sphere's, not the client's.** `MINCLIVER_AOS` is
      4.0.0.0 while the client gained AoS features at 3.0.8z, so every client in
      `[3.0.8z, 4.0.0)` is told it has no AoS support when it does.
