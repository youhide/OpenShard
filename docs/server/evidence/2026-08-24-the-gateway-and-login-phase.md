# 2. Gateway and login — done

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

## Entering the world says which character, once

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
      [connection_state.md](2026-07-30-the-connection-state-machine.md). `restore_characters` hands the
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
      [connection_state.md](2026-07-30-the-connection-state-machine.md). The roster stopped being "where
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
## Still to do: the character screen is one conversation, split across two files

The design this works toward is settled and written down — see "Sessions and the
character registry" in [architecture.md](../../architecture.md). Done so far:
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
## A connection's state is kept in two tables that must agree

Read while asking why `world_handle_network` has to hold `Sessions`, `LoginServer`,
`World` and `Roster` at once. None of these was a bug on a working shard; all of
them were the same seam being unnamed. The plan that acted on them, including the
steps above, is [`connection_state.md`](2026-07-30-the-connection-state-machine.md).

**All five are closed — S1 through S7 have landed.** What is left of that plan is
its backlog, and that backlog is the live one: a dozen findings, each with the
file it is in and what it would cost to act on, at
[`connection_state.md` → "Backlog, found while doing S1 through S7"](2026-07-30-the-connection-state-machine.md).
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
