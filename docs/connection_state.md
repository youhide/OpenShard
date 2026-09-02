# The connection state machine: one owner per phase

Living plan for a multi-session refactor of what a *connection* is. It is the
sequel to the design already written down in
[`architecture.md` → "Sessions and the character registry"](architecture.md), and
it does the one thing that design left unresolved: it says **where the
connection's state lives**, rather than which of its questions each existing
table answers.

As with [`protocol_newtypes.md`](protocol_newtypes.md): when reality contradicts
a decision here, change this file in the same commit that changes the code.

## Why

A connection's state is kept in two tables that have to agree, and nothing
checks that they do.

| | `server/src/session.rs` | `openshard-state` |
|---|---|---|
| which socket, which client version | `Session::login` | `Client { version }` on the entity |
| is it playing, and what | `Session::playing` | `WorldState::players` |
| what it is dragging, watching, has open | — | `held`, `open_containers`, `open_quest_gumps`, `open_craft_gumps`, `pending_targets`, `last_status`, `last_light`, `last_music` — the row's own fields since S7, bar `open_containers` |

Five things follow from the split, and each is a reason on its own:

1. **Presence is a bool, and it is set optimistically.** `Session::playing` is
   set as `Command::Enter` is *queued* (`server/src/dispatch.rs`), and
   `World::enter` refuses silently in three places — already in the world, a
   saved serial that will not bind, an exhausted mobile serial pool
   (`world/src/tick/enter.rs`). After any of them the session says it is playing,
   the world has no entity, and every world packet the client sends is queued
   into a tick that drops it on a `players.get` miss. The client is told nothing
   and waits on "logging into shard". `Option<PlayedCharacter>` cannot spell
   *asked to enter, not yet in* — the state that genuinely exists between the
   queue and the tick — so the lie is not a slip, it is the only thing the type
   can say.
2. **`in_world()` is a second copy of `players.contains_key`.** Thirty arms of
   `dispatch_world_packet` open by consulting the copy rather than the world.
   *(S3: it is asked once now. It stays a projection — D4 says why it must be —
   but there is one place that reads it, and one place a new packet can forget
   to.)*
3. **The world cannot answer a connection that has no entity.**
   `WorldState::send_packet` resolves the client version through
   `players → Client`, so a connection on the character screen is unreachable
   from inside a tick, and unreachable *silently*. This is the structural reason
   the character screen cannot become world commands as
   [`roadmap/02-gateway-login.md`](roadmap/02-gateway-login.md) plans: the version has to live on the
   connection, not on the entity. *(S1 moved it there; S5 is what it was moved
   for, and the row carries the account and access level too.)*
4. **The login crate already made this argument about itself.**
   `LoginSession`'s own doc: *"a state machine with facts kept outside it is a
   state machine that can disagree with itself"* — which is what `playing` is.
5. **Teardown is hand-written.** `World::disconnect` clears eight maps by name.
   The ninth one added without a line there leaks, and nothing catches it.
   *(S7: four of them already had — the gump tables, each promising in its own
   doc to be cleared on logout. All eight are fields on the connection's row now
   and go with it; `open_containers` is the one exception and the backlog says
   why.)*

## The shape this works toward

The seam is **authentication**. Everything before it is the login conversation;
everything after it is the world's, character screen included.

| | owner today | owner after |
|---|---|---|
| credentials, auth keys, `0x80`/`0xA0`/`0x8C` | `openshard-login` | unchanged |
| a character *exists* | `login.accounts` | the world, with the roster ✅ S5 |
| a character is *present* | `Sessions::playing` (binary) | the world, as a phase ✅ S5 — asked of the entity, and the binary's phase no longer names a character |
| client version | `Client` on the entity | the connection record ✅ S1 |
| the socket, and whether it is compressed | `Session` (binary) | unchanged — it is transport |

The login crate ends its life at `Authenticated { account, version, access }` and
hands the connection over with one command. From there the connection is a row in
the world with a phase:

```text
   Authenticated ──> Entering ──> Playing { entity } ──> LoggingOut
        │                │
        │                └─ Command::Enter queued, the tick has not applied it
        └─ character screen: list, create, select, delete
```

## Decisions

Numbered so a later session can argue with one without reopening all of them.

**D1. Login does not move into the world.** Accounts, argon2, auth keys, the
`0x8C` relay and the shard list are not simulation. `Argon2::default()` is 19 MiB
and two passes against a 25 ms `TICK_INTERVAL`; a password check inside a tick
stalls the whole shard for one client's benefit. What moves is everything *after*
the `0x91`.

*Refined by S6.* The hash does not belong on the shard's **task** either, which
is not the same statement: it stays in `openshard-login`, where the credentials
are, but the crate hands it back as work rather than doing it. The loop is what
must not wait.

**D2. The world's record of a connection is `openshard_state::connection::Connection`.**
What the world knows about a client that is not its character: its version
today, the per-connection state of S7 later. It is deliberately *not* called a
session — the session is the binary's, see D4 — and the two must not share a
name, or a reader has to work out each time which of them is authoritative.

**D3. The phase is an enum, and `Entering` is a state of its own.** It is the
distinction `Option<PlayedCharacter>` could not carry, and the one that makes
point 1 above unspellable rather than merely fixed.

**D4. The phase lives in the binary and is moved only by the world.**

The first half is forced: the packet router has to decide *now* whether a packet
may reach the world, and the world answers no synchronous question — only
`queue(Command)` in, `drain_*` and the bus out. That rule is why `World::is_online`
was deleted (see `architecture.md`), and a phase held in `WorldState` would put it
straight back by making the router read the world on every packet.

The second half is what keeps it honest: `Session::enter_world` may only move
`Outside → Entering`, and nothing but a world event moves it further.
`PlayerEntered`, `PlayerLeft` and the new `PlayerRefused` carry the connection,
and the shard loop drains them beside `drain_departed`. The binary's phase is a
*projection* of the world's fact, not a second copy of it: the one direction it
can be wrong in is being one tick behind, and `Entering` is the name of exactly
that gap.

Commands queued while `Entering` are still queued, not dropped — the queue is
ordered, so they apply after the `Enter` that precedes them. This is why the
phase must exist rather than the gate simply being moved later.

*Changed after S1, which had this backwards.* The first draft put the phase in
`WorldState` and had the binary read it. It cannot: see above.

**D5. The character screen becomes world commands only after the roster moves
in.** Ordering, not preference: until the world owns the saved records it cannot
answer `0x5D`, and until the connection record exists it cannot answer anybody
who has no entity. See ["The roster belongs in the
world"](roadmap/02-gateway-login.md).

**D6. This does not reopen the decision in `architecture.md`.** That decision —
create, select and delete stay out of `openshard-login` — was about not dragging
`openshard-persistence` (and with it bundled SQLite and `tokio-postgres`) into a
crate whose whole value is that it has neither. Moving them *into the world*,
which already depends on persistence, is the other direction and the objection
does not apply. What it does retire is "all three live in the binary": they live
in the binary today because the world could not be asked, and D2 is what changes
that.

**D7. No new crate.** The state goes into `openshard-state` beside the rest of
the runtime; the rules go into `openshard-world`. A `crates/server/session` crate
was considered and rejected: it would sit between login and world with a
dependency on both, which is the same seam this file is trying to delete, only
with a `Cargo.toml` around it.

## Steps

Each is a pull request. The first three are worth doing whether or not the rest
follows.

- [x] **S1. The world can address a connection that has no entity.** Done.
      `WorldState::sessions: HashMap<ConnectionId, Session>` carries the client
      version and `version_of` reads it; the binary sends
      `Command::Authenticated { connection, version }` at the one-way transition
      into `LoginSession`'s game state, `World::attach` writes the row and
      `World::disconnect` removes it. `Client` is down to its connection —
      `WorldState::client_of` returns the (connection, version) pair every packet
      path wanted, and `tick/context.rs` lost a second private copy of
      `version_of` that had been written beside it.
      Guarded by `a_connection_can_be_answered_before_it_has_a_character` and
      `a_disconnect_forgets_the_connection_itself` in `world/src/tick/tests.rs`.

- [x] **S2. The phase replaces the bool.** Done. `WorldPhase` — `Outside →
      Entering → Playing → Left` — on the binary's `Session`, in place of
      `playing: Option<PlayedCharacter>`. `PlayerEntered`/`PlayerLeft` carry the
      connection, the new `PlayerRefused` carries why an entry did not happen,
      and `PhaseSync` (cursors, drained at the top of `world_tick`) is the only
      thing that moves a phase past `Entering` — see D4. A refusal comes back
      from `PhaseSync::apply` as a connection to close, because the protocol has
      no way to tell a client its `0x5D` failed: the socket dropping is what
      turns an indefinite hang into a reconnect.
      Guarded by `the_phase_moves_on_only_when_the_world_says_it_did`,
      `an_entry_the_world_refuses_takes_the_connection_down` and
      `a_character_the_world_let_go_of_is_no_longer_being_played` in
      `server/src/session.rs`.

- [x] **S3. One gate instead of thirty.** Done. `dispatch_world_packet` is
      `fn(ClientPacket, ConnectionId) -> Option<Command>`: it takes neither the
      session nor the world, so an arm cannot reach past the packet it was
      handed, and `None` is a packet the world has nothing to do about. The
      phase is matched once, in `handle_world_packet`, which queues what comes
      back — the rule [`roadmap/02-gateway-login.md`](roadmap/02-gateway-login.md)
      asks for on the character screen, applied
      to the whole dispatcher.
      `0x5D` moved out of the dispatcher and was routed *before* the gate: it is
      the one packet a connection outside the world may send, and the only one
      that needed the roster and the account's access. That is what leaves the
      gate a single `if` and the dispatcher total. *(It is not a world packet at
      all any more — the backlog entry on `ClientPacket` below says where it
      went.)*
      Guarded by `a_world_packet_from_outside_the_world_becomes_no_command`,
      `the_same_packet_becomes_a_command_once_the_connection_is_in` and
      `the_character_screen_packet_never_meets_the_gate` in
      `server/src/shard.rs`. They assert on `World::queued`, added for them: a
      refused packet's whole story is that no work was created, and every other
      observation of the world is downstream of a tick that would have had
      nothing to apply either way.

- [x] **S4. The roster moves into the world.** Done. `Roster` is
      `world/src/tick/roster.rs` and a field on `World`; `departed` and
      `drain_departed` are gone, because the logout that filled the vector now
      writes the roster directly, at the same instant it hands the journal its
      copy. Boot hands the store's rows to `World::restore_characters` instead of
      keeping them, and the two commands that used to carry what the shard had
      looked up carry a name instead: `Character::Saved` asks the world to play
      whatever is on file, and `Command::DeleteCharacter { account, name }` tells
      it to forget everything under that name — its roster row, its store row and
      its saved inventory. `world_tick` is down from seven parameters to six.
      Guarded by the relogin tests in `world/src/tick/tests.rs`, which now enter
      with `Character::Saved` and nothing else: the row they come back on is the
      one the logout wrote, so a roster the world failed to fill would put the
      character back at the start city with default stats rather than pass.

- [x] **S5. The character screen is world commands.** Done, in two commits.

      *The roster is the account's list.* It held a record per character
      something had *saved*, while which characters *existed* lived on `Accounts`
      in the login crate — two lists that had to agree, with the world's half
      unable to see the other. Now it is per account, in the slot order `0xA9`
      shows and `0x83` indexes, and each entry carries `Option<CharacterRecord>`:
      `None` is a character that exists and that nothing has described yet, which
      is the state the old shape could not spell. `enter` enrols what it enters,
      and `forget` takes the entry off the list whether or not a record came with
      it — the early return that skipped the cleanup used to take the removal with
      it. Boot fills both halves: `restore_characters` for the store's rows,
      `seed_configured_characters` for `[[accounts]] characters`.

      *The screen moved.* `world/src/tick/screen.rs` answers `0xA9` on
      `Command::Authenticated`, and `0x00`/`0xF8`, `0x83` and `0x5D` are commands
      — `CreateCharacter`, `DeleteCharacter { connection, slot }`,
      `PlayCharacter`. The connection's row carries the account and the access
      level, so no character-screen packet has to name whose account it is. Name
      validation moved with creation; `Accounts` lost `characters`,
      `create_character` and `delete_character`, and `LoginServer` lost `starts`,
      `character_list_flags` and `supported_features` — they are the world's
      `CharacterScreen`, handed over at boot the way `Gameplay` is. The login
      crate now ends at `Response::Idle`.

      Three things fell out rather than being built. `0x83` indexes the list
      `0xA9` was built from, out of one value in one process, so the two agree by
      construction instead of by two lists happening to be in the same order.
      `0x5D`'s echoed name is checked against the account's list rather than
      trusted. And `WorldPhase` lost its payload: it carried the account and name
      for one question — "is anybody playing this character" — which the world now
      answers off the entity that *is* the fact. `Sessions::is_playing` is gone
      with it.

      Guarded by `world/src/tick/screen.rs`'s own tests — the list comes out of
      the tick, a duplicate/empty/sixth name is refused and keeps the connection,
      a played character cannot be deleted from a second connection on the same
      account, an unplayed one is and the screen is redrawn, and a `0x5D` naming a
      character the account does not have enters nothing.

- [x] **S6. The binary is glue again.** Done, in two commits.

      *What comes off a disk is boot's.* The accounts and the seven `restore_*`
      calls are `boot::restore`, beside `load_world` and `open_store`. They run
      once, before the first tick, and what holds them together is an order —
      characters before items because the serials one reserves are the owners the
      other points at — which now lives in one function with the reason above it
      rather than in eight lines of the `select!` loop's own function. The loop's
      state is one `Shard` value: the tick took seven parameters and had taken one
      more per step of this plan. And `keys.expire` has its own arm on its own
      interval, where memory upkeep for abandoned relay keys belongs; at the
      tick's rate it ran twenty times a second to find nothing 599 times out of
      600.

      *argon2 left the loop.* This was the part with a design in it. The hash
      cannot simply move to a blocking task, because the login conversation is
      synchronous and the loop cannot wait for it: so the conversation *suspends*.
      `LoginServer::handle` returns an `Outcome` — bytes to send, or a
      `CredentialCheck` to run — the session waits in a state named for exactly
      that (`VerifyingAccount` on the login socket, `GameState::Verifying` on the
      game one), and the verdict comes back through `LoginServer::resume` on a
      `select!` arm of its own. It is the same shape as S2: the state that
      genuinely exists between asking and knowing, given a name instead of being
      hidden inside a call.

      Two things fall out of where the identity lives. The account stays in the
      state machine and the `CredentialCheck` carries none, so a verdict is
      yes-or-no about a credential and nothing else — one delivered to the wrong
      connection closes it rather than authenticating it, and there is a test that
      says so. And `Command::Authenticated` is now queued in one place,
      `Shard::resume_login`, because a matching verdict on a game login is the
      *only* transition into `CharacterListSent`: no "was it already
      authenticated?" comparison, because the state machine cannot be asked twice.

      The blocking pool is bounded by a semaphore, one permit per core. Every
      argon2 in flight holds 19 MiB and `spawn_blocking` will start 512 of them —
      ten gigabytes, from clients that have proved nothing yet. The old loop
      bounded that by having no choice but to run one at a time; taking the
      serialisation away without putting a bound back would have been a new door.

- [x] **S7. The rest of the per-connection state joins the row.** Done, in two parts,
      because the eight maps turned out to be two different problems wearing one
      name.

      - [x] **S7a. What is keyed by a connection.** Done. `held`, `last_status`,
            `last_light` and `last_music` are fields on
            `openshard_state::connection::Connection` — the first from
            `WorldState`, the other three from `World` itself, where they had
            been since before there was a row to put them on. `held` is an
            `Option<HeldItem>` rather than a map entry, which is what the map's
            missing key always meant: a cursor holds one thing.

            Teardown is `WorldState::forget_connection`, and `disconnect` reads
            one field off the row it removed — the cursor, because an item on it
            is off the ground and in no container and would be deleted by the
            row simply ceasing to exist. Nothing else needs an answer, which is
            the point. Two hand-written sweeps went with it: `disconnect`'s list
            of maps by name, and a second one in `refresh_statuses` that
            `retain`ed the same state on a different condition.

            `attach` writes the identity *into* the row now instead of replacing
            it. Both callers pass the same identity — that is why replacing was
            safe — but the row carries what the client is in the middle of, and
            a second hand-off must not take an item off a cursor.

            Guarded by `a_disconnect_takes_everything_the_connection_was_in_the_middle_of`
            and `a_second_hand_off_keeps_what_the_connection_was_doing` in
            `world/src/tick/tests.rs`. The first asserts the row's *absence*
            rather than each field's, on purpose: a field added later is covered
            by it without anybody remembering to extend it, which is the whole
            property the shape was changed for.

      - [x] **S7b. What is keyed by an entity.** Done. `pending_targets`,
            `open_quest_gumps`, `open_craft_gumps`, `open_runebook_gumps` and
            `open_gate_gumps` were keyed by the *player's entity*, so `disconnect`
            could not sweep them by connection — and four of the five it did not
            sweep at all, though each one's own doc said "Cleared on logout". They
            are fields on the row now, reached through `WorldState::row_of` and
            `row_of_mut`, which resolve the entity's `Client` component in one
            lookup rather than walking `players`.

            The re-keying is not a convenience: every one of the six sites that
            raises a targeting cursor already began by refusing to raise one
            without a `Client` — "a creature has no cursor to raise" — so the
            connection was what the state was about, and the entity key made the
            invariant something to restate at each site instead of something the
            type holds. `raise_target`/`take_target`/`has_target` are the seam,
            and they are total: a mobile with no client gets no cursor and is
            asked no questions.

            Guarded by `a_targeting_cursor_belongs_to_the_screen_it_is_drawn_on`
            in `world/src/tick/tests.rs` and by the disconnect half added to
            `double_clicking_the_tongs_is_what_opens_the_window`. Both assert the
            state is remembered *before* the logout as well as gone after it: a
            leak test on a window nobody opened is green for the wrong reason.

## Backlog, found while doing S1 through S7

None is a blocker; each is written down where the next step through this area
will read it.

- **A helper written beside the one that already exists.** S1 found two:
  `tick/context.rs` had a private `client_version(connection)` that was
  `WorldState::version_of` reimplemented next to it, and `items/containers.rs`
  walked `players → Client` inline for the same answer. Both predate the version
  moving, and both would have had to be found and changed anyway — the point is
  that neither was findable by grepping for `version_of`. A duplicate helper is
  invisible to the search that would prove it is a duplicate.
- ~~**`world_tick` now takes seven parameters**~~ — fixed in S6: the loop's state
  is a `Shard`. The packet handlers still take their pieces one at a time, and
  that is not an oversight left half-done: a handler holds a `&mut Session` while
  it queues into the world, which the compiler splits across fields and refuses
  across a `&mut self`.
- **Closing a refused connection relies on a chain nothing states.** *(Planned:
  [`unenforced.md`](unenforced.md) S3.)*
  `Sessions::close` drops the session, which drops the outbox, which ends the
  gateway's write task, which closes the socket, which makes the gateway emit
  `Disconnected`, which queues `Command::Disconnect` so the world lets go of
  whatever it had. Every link is real and none is written down in one place;
  there is no test that walks it end to end, because it needs a real gateway —
  which is what `crates/e2e` is for.
- ~~**`Entering` has no timeout.**~~ Half fixed. A session still stays there until
  the world says `PlayerEntered` or `PlayerRefused` — there is no clock — but the
  world can no longer fail to say one of them. `World::enter` is a wrapper around
  `try_enter(…) -> Result<(), RefusedEntry>`: a failure path is a `return
  Err(reason)` and the refusal is emitted in the one place all of them come back
  to, so the fourth failure path cannot be added without one. It used to be true
  by construction and checkable only by reading every early return.
  Guarded by `an_entry_that_does_not_happen_says_so` in `world/src/tick/tests.rs`,
  which was checked against the wrapper swallowing the error. What is left is the
  clock: a `PlayerEntered` that the shard loop never *reads* — a phase sync that
  stops draining — would still strand the session, and nothing bounds the wait.
- ~~**A logout leaves the phase on `Playing`.**~~ Fixed, and it is named
  `LoggingOut` after all. `Command::LogoutRequest` still answers the ack and lets
  go of nothing — the character stands there until the socket closes and
  `Disconnect` runs — but it now emits `PlayerLeaving` beside the ack, and
  `PhaseSync` moves the session on it. The gate does the rest: `may_act` is false
  there, so the window between the ack and the client hanging up stops accepting
  in-world packets from a connection that has said it is going.

  It is the second in-between state and the opposite answer to the first:
  `Entering` may act because the queue is ordered and a command applies after the
  `Enter` it follows, while `LoggingOut` may not, because there is nothing left
  for a client to say after "I am leaving". Worth stating that the phases are now
  applied in lifecycle order — one tick can carry an `Enter` and the `0xD1`
  behind it, and each cursor reads its own event type without saying anything
  about the others' order.
  Guarded by `a_connection_that_announced_a_logout_stops_being_asked_for_more`.

- **`pending_inventories` did not collapse, and cannot.** S4's plan above said it
  would fold into the roster's record along with `departed`. Two of the three
  merged; the third is keyed by *mobile* serial and holds an NPC's gear and a
  vendor's stock crate as well as a character's pack — `restore_items` files
  every record whose owner is non-zero, and `restore_mobiles` equips out of the
  same map. Folding it into a table of characters would leave the NPC half of it
  homeless. What S4 did instead is put the two behind one *deletion*:
  `World::delete_character` forgets the roster row and the inventory under its
  serial together, so the pair can no longer disagree about whether a character
  exists even though they are still two maps.
- **A character that is playing is still on file at its logout position.**
  `enter` reads the roster and never writes it, so while a character is in the
  world its row describes where it was *last* time. Nothing reads that row in the
  meantime, so it is inert rather than wrong — but it is exactly the "exists"
  versus "is played" distinction this plan wanted as two states of one record,
  and S5 did not give it one. What S5 did was move the *question*: `is_playing`
  is asked of the entities now (`tick/screen.rs`), which is the only thing that
  can answer it truthfully, rather than of the shard's session table. The record
  still cannot say it, and the answer is a scan of `players` — cheap because only
  create and delete ask it, and the reason to leave it alone until the record can.
- ~~**`StoredCharacter` is public and nothing outside the world names it any
  more.**~~ Fixed. It was on `Command::Enter`, so it had to be public; since S4
  `enter` is the only caller of `from_record` and the only reader of the type,
  and it was still exported from `openshard_world` — a public API nobody used.
  It is `pub(crate)` now and off both re-export lists.

  The doc links this entry expected to have to unpick were two, and neither
  survived as a link: `CharacterSheet::starting` and `Character` are public and
  now describe a type the crate does not export, so they name it in a code span
  instead. That is the honest rendering — a reader of the public docs cannot
  follow a link to something they cannot use — and it keeps
  `rustdoc::private_intra_doc_links` quiet.
- ~~**`restore_characters` must run before `restore_items`, and only a doc says
  so.**~~ Fixed by [`unenforced.md`](unenforced.md) S1. `restore_characters`
  returns a `RestoredCharacters` — the set of serials it reserved, with a private
  field — and `restore_items` takes one, so the order is the signature and the
  pair cannot be swapped without a type error. The set is read rather than
  carried: it is what lets the items' restore tell a player's pack from an NPC's
  gear.

  The link after it is still prose: `restore_items` must run before
  `restore_mobiles`, which equips out of the inventories the items filed. Same
  shape, same fix, and the reason it is not done is eight test sites that restore
  mobiles alone — recorded in that plan's S1 rather than here.

- ~~**`ClientPacket` mixes the character screen in with the world.**~~ Fixed, at
  the decode seam the entry pointed at. S3 left one `unreachable!` behind: `0x5D`
  was a `ClientPacket` variant that `dispatch_world_packet` could never
  legitimately see, because `handle_world_packet` matched it out first. The
  invariant was real and lived in a `match` arm in another file, which is exactly
  the shape this plan is trying to delete.

  `0x5D` is a `LoginStagePacket::PlayCharacter` now, beside `0x00`/`0xF8` and
  `0x83`, so `RawPacket::parse_packet` routes it to `handle_login_packet` with
  the screen's other two and the world's dispatcher never sees it. The arm cannot
  be written: there is no variant to write it about. `handle_world_packet` is
  down to the gate and the queue, which is what S3 said it was and what it has
  only now become.

  The struct stays in `protocol::world` — that is where the conversation it opens
  is drawn, and `CreateCharacter`'s body lives there too. What moved is which
  half of the split claims the id.

  Guarded by `character_select_is_not_a_world_packet` in `protocol`, which asserts
  the world decoder now answers `Unknown` for `0x5D` rather than a packet the
  dispatcher could be handed, and by
  `the_character_screen_packet_never_meets_the_gate` in `server/src/shard.rs`.
- ~~**`Accounts::verify` is the slow path, still reachable.**~~ Fixed, and not the
  way this entry proposed. S6 split the trait into `credential` (a lookup) and
  `CredentialCheck::run` (the hash), and left `verify` as a provided method that
  did both — argon2 on the caller's thread, which is exactly the call that must
  never appear in the shard again, with nothing but a doc comment saying so. The
  entry asked for a clippy `disallowed_methods` line; what it got instead is the
  method not existing.

  The reason to prefer that is what a grep for the call sites turned up: every
  one of them was in `accounts.rs`'s own test module. The "fixtures and tools"
  the doc comment justified it with are not in this repository, and the three
  test loops of the finding below run `CredentialCheck::run` themselves rather
  than going through the trait. So `verify` is a private helper in that test
  module now, taking the store as an argument the way a free function does. A
  lint would have said "do not call this"; a helper the shard's crates cannot
  name says it without a lint config, and without the `#[allow]` that each of the
  fourteen honest test call sites would have needed.
- **Three test loops resolve a verification by hand.** `login/src/session.rs`'s
  `drive`, `server/src/testing.rs`'s `drive`, and the fake shard in
  `server/tests/login_flow.rs` each write `match handle { Reply => …, Verify =>
  resume(check.run()) }`. They are three lines each and they are honest — a test
  *should* be able to run the check on the spot — but they are three copies of
  the shard's control flow, and a fourth step of this plan that changes the shape
  will find all three.
- **A verifying session has no timeout, and its queue has no bound.** The same
  shape as `Entering` above: a connection waits for exactly one verdict and
  accepts nothing else. The verdict always comes — a check that cannot run is
  answered `Rejected` rather than dropped — but nothing bounds how many
  *unauthenticated* connections may have a check queued behind the semaphore at
  once. Today that bound is whatever the gateway will accept; a shard under a
  login flood queues a task per connection, each holding a password.
- **The gate no longer says which packet it dropped.** Thirty arms could each
  name their own; one gate has only the connection. Naming the packet would mean
  either `ClientPacket`'s `Debug` — which carries bodies, so a `0x03` would put
  the player's typing in the log — or a per-variant name table, a second list to
  keep in step with the enum. Worth revisiting if a real client ever hits this
  path, because today it means a misbehaving client is one indistinguishable
  line per packet.

- **`open_containers` did not collapse, and cannot** — the `pending_inventories`
  finding of S4, in the same shape and for a different reason. It is an *inverted*
  index, `Serial -> {ConnectionId}`, and every read of it asks "who is watching
  this container" as an item inside it changes. On the row it would be
  `HashSet<Serial>` per connection, and each of those reads would become a scan of
  every connection on the shard — a per-item-change cost that grows with the
  player count. So it stays an index, and `WorldState::forget_connection` sweeps
  it: the one piece of per-connection state the row cannot carry, in the one place
  that lets go of a connection.

- ~~**The four gump tables prove the argument this plan opens with.**~~ Fixed by
  S7b. Point 5 of "Why" says the ninth map added without a line in `disconnect`
  leaks and nothing catches it. `open_quest_gumps`, `open_craft_gumps`,
  `open_runebook_gumps` and `open_gate_gumps` were that map, four times over, and
  each one's own doc comment said "Cleared on logout" — which was true of the
  neighbour it was written beside and never true of it. Logging out with a craft
  window open left an entry keyed by an entity that no longer existed, until the
  id was reused.

  They are `quest_gump`, `craft_gump`, `runebook_gump` and `gate_gump` on the
  row now, and each lost its plural with its map: a screen shows one of each, so
  the `Option` says what the map's missing key had been standing in for. The leak
  cannot be reintroduced by forgetting a line, because there is no line — the row
  going away takes them.

- ~~**`connection_of` is a scan, and it is the third copy.**~~ Fixed. Both walks of
  `state.players` are gone: `WorldState::connection_of` is the named lookup beside
  `row_of` and `client_of`, and three of the four call sites did not want the
  connection at all — they went on to read its row, which is `row_of` in one call.
  `items/src/weight.rs`'s `held_by` went the same way and stopped existing.

  The finding it repeated is still worth the words. A duplicate helper is
  invisible to the grep that would prove it is a duplicate, and it is *also* how a
  complexity regression gets in: the two scans were O(players) where the lookup
  they duplicated is O(1), on paths that run per status refresh and per weight
  read.

- **A creation that enters the world does so without the gate ever opening.**
  `0x00` does not move the phase — a refused creation must keep the connection on
  the screen, and moving it optimistically would strand it in `Entering` with no
  character behind it — so the phase goes `Outside → Playing` on the
  `PlayerEntered`. In the window between the two, an in-world packet from that
  connection is dropped by the gate. No client sends one there (it is waiting for
  the `0x1B`), but the window is real and unnamed. The honest fix is a phase the
  world moves *into* on a creation as well — the same shape `LoggingOut` was
  given above, and the one in-between state this plan has left hiding inside a
  transition rather than named by one.

- **`DeleteResult` says less than the world knows.** A slot naming no character
  and a slot outside the list both come back as `CharNotExist`, because that is
  what the protocol has. Fine — but the *log* now collapses them too, and the two
  mean different things about the client.

- **`start_cities` is content in the binary.** It is a list of nine towns with
  coordinates, filtered by facet, living in `server/src/dispatch.rs` next to the
  packet translation. It is handed to the world at boot as configuration, which is
  right; where it is *written* is not, and it is the kind of thing the Community
  Pack should own.

- ~~**A doc comment outlived the function it described.**~~ Fixed where it was
  found, and kept here for the pattern. `World::enter` carried four lines about
  "the facet a mobile is on, or the default if it carries none", which is
  `WorldState::facet_of` — moved out to the state crate long ago, its doc left
  behind to become the first thing anybody reads about world entry. The sentence
  even ended in a comma. Nothing catches this: `missing_docs` sees a documented
  item, and rustdoc renders it. Replaced with `enter`'s own, which states the
  obligation the wrapper exists for. Worth watching for wherever a helper has
  been lifted out of a file: the doc does not move itself.

- **Nothing in this workspace ever runs rustdoc.** Found while demoting
  `StoredCharacter`, which needed to know whether a broken doc link would be
  noticed: `cargo doc --workspace --no-deps` emits 67 warnings across sixteen
  crates today — unresolved links, and public items linking to private ones —
  and none of them is visible to `cargo test`, `cargo clippy` or `cargo fmt`,
  which is the whole of CI. They are the same class as the doc comment above: a
  doc that has quietly stopped describing the code, with no build step that
  disagrees. A fourth command would say so, and `-D warnings` on it would keep it
  said; the cost is that somebody has to clear the 67 first.

## To verify with a real client

Two things only, and both are about timing on the wire rather than about the
shape of the code. Findings that read like work rather than like an observation
belong in the backlog above, however they were discovered.

- **The character screen answers a tick late, and nobody has watched a real
  client do it.** `0xA9` used to go back inside the same call that read the
  `0x91`; since S5 it waits for the next tick, up to one `TICK_INTERVAL` (25 ms).
  The client is already waiting at that point and this should be invisible — but
  it is exactly the kind of thing that is fine in theory and a hang in practice.
- **Compression must not follow the phase.** A game socket is Huffman-compressed
  from the moment its `0x91` is read, refusal included; the flag stays in the
  binary's transport and is set once, irreversibly, at the hand-off. Reading it
  off a phase that lives in the world would put a channel round-trip between the
  socket and the question "is this stream compressed".

## Status

**S1 through S7 have landed, and the backlog above is what is left of this plan.**
It is the live list, not an appendix: each entry names the file it is in and what
acting on it would cost, and entries are struck through in the commit that fixes
them rather than deleted, so the reasoning that was wrong stays readable beside
the reasoning that replaced it.

[`roadmap/02-gateway-login.md`](roadmap/02-gateway-login.md), under "A
connection's state is kept in two tables that must agree", is where this plan was
argued for. Its five findings are all
closed and it points back here for what is open — do not treat the two as two
lists.
