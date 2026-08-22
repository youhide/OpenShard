# Invariants nothing enforces

Living plan for the backlog that three finished sweeps left behind. Unlike
[`protocol_newtypes.md`](protocol_newtypes.md) and
[`connection_state.md`](connection_state.md), which each took one subject
through one crate, this one is a shape found in four places at once — a fifth
came out of the first while it was being done — so the stages share a reason
rather than a module.

As with both of those: when reality contradicts a decision here, change this
file in the same commit that changes the code.

## Why

Every item below is a rule the code obeys and cannot state. The rule is real,
it is written down, and the only thing keeping it true is that whoever last
touched the area had read the paragraph.

That is a specific failure mode and not a general complaint about
documentation. A rule in prose has three properties a rule in a type does not:
it is invisible to the person who never opens the file, it stays green when the
code stops obeying it, and it cannot be found by the search that would prove it
was broken. The newtype sweep spent thirteen stages on the first of those —
`Serial` instead of `u32` — and the sweep's own gate then found the second: a
coverage check that examined nothing had been green here before, and a check
that cannot see a function's parameters was green over four `cliloc: u32`
signatures for as long as they existed.

So the question this file asks about each item is not "is it correct today"
(all of them are) but **what would notice if it stopped being**.

| | the rule | what holds it today | what would notice |
|---|---|---|---|
| S1 | `restore_characters` runs before `restore_items` | ~~two doc comments and the order of two lines in `run_shard`~~ the signature: `restore_items` takes what only `restore_characters` returns | the compiler |
| S2 | every recipe names a group that exists and leads with its system's skill | ~~an assertion in a test~~ **`crafting/build.rs`** | `cargo check`, before the crate compiles |
| S3 | closing a refused connection tears down six things in order | ~~six doc comments, one per link~~ **`e2e/shard/tests/refused_teardown.rs`**, which walks all six | `cargo test`, on any machine — and it found link 4 half-missing |
| S4 | the ground renderer's projection and visible set are correct | ~~`tests/frame.rs`, behind `OPENSHARD_CLIENT` **and** a GPU~~ **`ground.rs`'s own tests**, on a map built in memory | `cargo test`, on any machine |
| S5 | `restore_items` runs before `restore_mobiles` | ~~a doc comment on each and the order of two lines in `boot::restore`~~ the signature: `restore_mobiles` takes what only `restore_items` returns | the compiler |
| S6 | ~~the config's characters are seeded after the store's~~ *there was no such rule* — but a character keeps its slot across a boot | ~~nothing, on two of three backends~~ `Store::characters` promising ascending serial, and three tests | `cargo test` — and a `MemoryStore` shard stops reshuffling every boot |

## Decisions

Nailed down here so a session does not stop to ask.

**D1. A build-time check beats a test, and a type beats both.** In that order,
and the order is about *when the wrongness is visible*: a type is wrong while
the author is still typing, a build script is wrong before the artefact exists,
a test is wrong after a commit that may already have been pushed. Where an item
below can be moved a whole step, it is moved a whole step. Where it cannot, the
stage says why rather than settling quietly.

**D2. `Option`, never a zero.** Already settled in
[`style.md`](style.md) and applied twice in the cliloc work (`CraftGumpContext::
notice`); S2 finishes it. A cliloc `0` is a number the client would look up, so
"absent" and "message zero" are the same bits and only a comparison somewhere
tells them apart.

**D3. An e2e test is allowed to be slow and is not allowed to be flaky.**
`crates/e2e` exists precisely so that a test may stand up a real gateway and a
real shard in one process. S3 is the second thing to live there. A retry loop
with a deadline is fine; a `sleep` that happens to be long enough is not — that
is a test that passes on this machine.

**D4. The renderer's fixture is a map, not a mock.** S4 does not introduce a
`trait WorldMap`. The renderer reads a concrete `WorldMap` and should keep
reading one; what is missing is a way to *construct* one from cells in memory. A
trait would make the tests pass against something that is not what ships.

**D5. Nothing here touches a file another session is in.** This is a working
constraint, not a design one, and it is written down because it has already
cost a day: the tree is shared, and two of these stages sit in the hottest files
in the repository. See "Order" below.

## Steps

Each is a pull request. S1 and S2 are worth doing whether or not the rest
follows.

- [x] **S1. The restore order stops being a comment.**
      `restore_characters` reserves the serials that `restore_items`' records
      point at as owners. Run them the other way round and a character's pack is
      filed under a serial the allocator is free to hand to something else — and
      nothing fails, then or later: the pack is simply somewhere else, and the
      first person to notice is a player. Both `restore_*` docs state the order
      and `run_shard` obeys it.

      The fix is to make the second unable to run first. The cheapest honest
      shape is a token: `restore_characters` returns a `RestoredCharacters`
      (whatever it already computes, plus the fact that it ran), and
      `restore_items` takes one. It cannot be constructed elsewhere, so the
      order is the signature. Prefer this to an assertion inside `restore_items`
      — an assertion is a fourth thing to keep true, and it fires at boot on a
      live shard rather than at compile time on a developer's machine.

      **DoD:** the two functions cannot be called out of order without a type
      error; the token carries something real rather than being a marker
      (whatever `restore_characters` already hands over, or the reserved-serial
      set itself); `run_shard` reads the same or shorter; the doc comments say
      what the type now says, or are deleted for repeating it; all four gates
      silent.

      **Done.** `World::restore_characters` returns `RestoredCharacters` — the
      `HashSet<Serial>` it reserved, with a private field, so nothing else can
      build one — and `restore_items` takes it by reference. The set is read,
      not carried: `restore_items` uses it to tell a player's pack from an NPC's
      gear for one `debug!` line, which is the only thing at boot that says the
      packs found their owners.

      Two things fell out that the stage did not name. `World::stored_characters`
      existed for the boot log alone and the token now answers that better (it
      counts what *this* restore brought back, not what the roster holds), so it
      is gone and `Roster::saved` under it is `#[cfg(test)]` — its remaining
      callers are three of the roster's own tests. And `boot::restore_characters`
      returns the token even when the store cannot be read, restoring nothing:
      an `Option` there would have put the ordering rule straight back into prose
      by making the caller decide what "no characters" permits.

      Tests: the four call sites that restore items without a store now say what
      they mean — `tests::on_file` hands the token on, and the two that have no
      characters at all call `restore_characters(Vec::new())` and say why in a
      comment. No test-only constructor was added: an escape hatch inside the
      crate that defines the order is the order back as a convention.

      **The mobiles link is still prose.** `restore_items` must run before
      `restore_mobiles`, which equips out of the inventories the items filed, and
      `boot::restore`'s doc says so. It is the same shape one step further on and
      the same fix would work (`restore_items` returning a token
      `restore_mobiles` takes), at a cost this stage did not carry: eight test
      sites restore mobiles alone. Worth doing, cheaper than it looks, and left
      out of S1 because S1 said two functions and the shape is worth judging on
      its own once. *Judged, and done: S5.*

- [x] **S2. The craft tables fail the build, not the test run.**
      `defs/mod.rs` asserts that every recipe names a group that exists and
      leads with its system's own skill. Both are properties of the *data*, and
      `crafting/build.rs` is where a bad row should stop being a build. What
      blocks it is that the group count and the main skill live in `SYSTEMS`,
      which is hand-written Rust the build script does not read.

      **The decision: the five headers join the data.** `SYSTEMS` becomes
      `data/craft_systems.json` beside the eight tables that already moved,
      generated by the same `build.rs`, which then has both halves in hand and
      can check them. The alternative — teaching the script to parse `SYSTEMS`
      out of Rust source — is a second parser for a language we already have a
      compiler for, and it is the shape `architecture.md` moved 18,155 lines of
      tables away from in the first place.

      Two things ride along, because they are in the same rows and the same
      commit is the cheapest place for them:

      - `CraftSystemDef::needs_message` is `ClilocId(0)` on four systems of five
        (`defs/mod.rs`) — D2's zero. `Option<ClilocId>`.
      - `Recipe::amount` has a column and no data: all 485 rows are 1. Leave the
        column and say so in the doc, or drop it — but decide, and record which,
        rather than carrying a field nothing sets. The stacking recipes that
        would use it (`DefBowFletching`'s arrows and bolts) are not ported.

      **Note before starting:** the roadmap entry for this also claims
      `Text::Cliloc(0)` appears in the generated tables as a null. It does not —
      of 11,448 generated clilocs, zero are `0`. The entry is stale; delete that
      half of it. This is the second stale backlog claim in two sessions, which
      is itself worth remembering: **check a backlog entry against the code
      before planning work around it.**

      **DoD:** `SYSTEMS` is `data/craft_systems.json`; both invariants are
      checked in `build.rs` and fail the build with a message naming the row;
      the two assertions in `defs/mod.rs` are deleted, not kept as belt and
      braces (a check in two places drifts); `needs_message` is an `Option`;
      `Recipe::amount` is decided either way with the reason in its doc; the
      stale roadmap half is gone; all four gates silent.

      **Done.** `SYSTEMS` is generated from `data/craft_systems.json` into
      `OUT_DIR/systems.rs`; the two assertions are `build.rs::check`, verified to
      fire by breaking a row four ways and reading the panic. `amount` stays,
      with the reason in its doc. Four things worth carrying forward:

      - **Two coverage checks came with the two real ones**, and they are the
        reason the pair means anything: a `data/*.json` no header claims is a
        table these checks never open — green, over nothing. So a trade with no
        system, and a system whose table is empty, both fail the build too.
      - **A build script can still emit a compile-time check.** The delay is
        ServUO's 1.25 seconds and the tick rate is the engine's, so the data says
        `delay_ms: 1250` and the generator writes `TICKS_PER_SECOND * 1250 /
        1000` — plus a `const _: () = assert!(… % 1000 == 0)` beside it, because
        a delay that is not a whole number of ticks would truncate silently. The
        value belongs to the crate and the rule belongs to the data; emitting the
        assertion is how both keep their half.
      - **The "note before starting" was itself stale.** The roadmap already
        carried the `Text::Cliloc(0)` correction, marked and kept deliberately.
        Third stale backlog claim in three sessions, and the first one that was
        stale in the direction of *already fixed*.
      - **`NeedsRow::expr` said "484 rows of 485".** No recipe row carries a
        `needs` at all — every workshop requirement in the data today is a
        system's. Corrected in place.

      Left behind: `chance.rs` finds the main skill with `.any()` while
      `Recipe::skills` documents the first line as the one the chance is
      interpolated over, and `build.rs` now enforces the stronger reading. The
      scan could be `skills.first()`, which would delete the `main: Option<…>`
      accumulator and the "no main-skill line is a malformed recipe" arm with it
      — the build script has made that arm unreachable.

- [x] **S3. The teardown chain gets a test that walks it.**
      A refused connection is closed by a chain of six: `Sessions::close` drops
      the session, which drops the outbox, which ends the gateway's write task,
      which closes the socket, which makes the gateway emit `Disconnected`,
      which queues `Command::Disconnect` so the world lets go of what it held.
      Every link is real, each is documented where it lives, and no file states
      the chain. Breaking any link leaves a connection the world still thinks is
      present — the leak the whole of `connection_state.md` was written against.

      This one cannot become a type: the links are a socket, a task and a
      channel, and what joins them is `Drop`. So it becomes the second thing in
      `crates/e2e`: a real gateway, a real shard, a connection refused entry,
      and an assertion that the world has forgotten it — with the *whole* chain
      exercised rather than any single link mocked.

      Write the chain down in one place while doing it. The natural home is a
      module doc on the e2e test itself: the test is the only artefact that
      knows all six links, so the prose belongs beside it rather than in a
      seventh file that can go stale on its own.

      **DoD:** a test in `crates/e2e` that opens a real connection, is refused,
      and observes the world drop it; the test asserts the *state before* as
      well as after, because a leak test on something that was never there is
      green for the wrong reason (`connection_state.md` S7 learned this); no
      fixed `sleep` — a deadline and a poll (D3); the chain named link by link
      in one doc comment; all four gates silent.

      **Done, and the chain was broken.** `crates/e2e/shard/tests/
      refused_teardown.rs`: two clients on the stock account, the second
      provoking `RefusedEntry::AlreadyInWorld` with a second `0x5D` — the one of
      the four refusals a real client can reach over the wire. It failed on
      first run, at the last assertion, and what it found is worth more than the
      test:

      **Link 4 was half a link.** Dropping the outbox ends the gateway's write
      task, and dropping `OwnedWriteHalf` shuts the *write* half of the socket.
      The client therefore reads zero bytes and knows it has been hung up on —
      which is why this looked right for as long as anyone had looked. But the
      socket is only closed when *both* halves are, so `read_loop` went on
      awaiting a read that would never come, no `Disconnected` was emitted, and
      links 5 and 6 never ran. The world kept the refused character: standing
      there, visible to everyone, for as long as the client held its half open.
      A well-behaved client closes and the chain completes, so the bug was
      invisible to every real client and to every test that used one. Fixed by
      racing `read_loop` against the write task in
      `gateway::client_session_serve`.

      Three things worth carrying forward:

      - **The witness is the only honest observer.** A refused client's own
        socket closing proves links 1–4 and says nothing about 6 — a world that
        never let go would close it identically. The second client is what makes
        the last link observable at all, and asserting it *saw the arrival*
        first is what keeps the disappearance from being green on a shard that
        spawned nothing.
      - **Three gateway tests were dropping the outbox by accident**, which now
        means "hang up", so they closed the connection they were about to read
        from. They were not wrong before and are not wrong now — the operation
        changed meaning under them, which is what a test double for a `Drop`
        will always risk. Each now holds it and says why.
      - **Two places already said the outbox drop *is* the close** —
        `connection_state.md`'s chain and `Session::apply`'s doc, which adds
        "there is no separate close to forget". Both were true of the write half
        and read as if they were true of the socket, and neither is edited here:
        they are right today because the code was changed to match them. Prose
        that states an invariant correctly and is believed by nobody's compiler
        is the whole subject of this file.

      **The duplicate is resolved in favour of this file**, and two things came
      across from `refused_entry.rs` before it was deleted:

      - **The witness is a second account, not the stock one played twice.**
        Both clients logging in as `admin` works today, and nothing states that
        it may: no rule refuses a second connection on an account, and none
        promises not to. A fixture resting on that dies the day somebody adds
        the check — in a test that has nothing to do with logging in twice. So
        `config_for` appends one `[[accounts]]` table to the stock config, which
        is a complete table wherever the sections above it move to, and the
        witness plays a character of its own.
      - **A close is a close; any other error is not.** `Err(_) => return` while
        waiting for the far end to hang up reads a stream that stopped making
        sense as a teardown that worked. Only zero bytes and
        `TransportError::Io` count now; anything else panics. This is the same
        shape as the bug above — a thing that looks closed from one angle.

      Left behind, both found while looking for a second character to put in the
      world:

      - **Two connections on one account is neither allowed nor refused.**
        Nothing in login, the world or the shard loop looks at whether an
        account is already playing; the same character can be entered twice and
        stands as two bodies with two serials. That may be the right answer — a
        shard operator with two clients open is a normal thing — but it is
        currently the answer by omission, which means no test would notice it
        changing. Decide it and write it down where it is decided.
      - **`client/net` cannot create a character.** `Login` answers the `0xA9`
        character list with a `0x5D` and has no path for the `0x00` that would
        make a new one, so a test needing a character the config does not ship
        has to add it to the config. `CreateCharacter::encode` exists on the
        protocol side; what is missing is the state machine's arm and the
        `Plan` field that would choose it. It belongs with `docs/client.md`'s
        milestones rather than here.

- [x] **S4. A `WorldMap` can be built without a client install.**
      Every assertion about `ground::collect` lives in `client/render/tests/
      frame.rs`, behind `OPENSHARD_CLIENT` and a GPU, because the only way to
      obtain a `WorldMap` is to load one from a file. So the projection and the
      visible-set logic — which are arithmetic, and wrong in ways a picture does
      not show — are untested on every machine that has neither, which includes
      CI.

      Both atlases already take pictures directly (`LandAtlas::pack`,
      `TexmapAtlas::pack`), so the *art* half of this is done: that a slope is
      drawn from its texture and a level tile from its art is already green with
      no install. What is left is the map. Per D4: a constructor taking cells,
      or a small fixture facet built in memory — not a trait.

      **DoD:** `ground::collect`'s visible set and the projection are asserted
      in a test that runs with neither `OPENSHARD_CLIENT` nor a GPU; the
      existing frame tests keep working against a real install; the constructor
      is honest about what a hand-built `WorldMap` does not have
      (`WorldMap::load`'s facet ambiguity, noted in `client.md`, is not made
      worse); all four gates silent.

      **Done.** `WorldMap::from_blocks(blocks_wide, blocks_down, |x, y| …)` asks
      a closure for every tile by world coordinate and lays the cells out
      itself. Seven tests in `ground.rs` now run with neither the client's files
      nor an adapter: the visible set, the projection, the sort, the edge of the
      map, and the two that were gated behind `OPENSHARD_CLIENT` before.

      Four things worth carrying forward:

      - **The size is in blocks, so the invalid case stopped existing.** A facet
        is a whole number of 8×8 blocks; taking tiles would have meant a
        `Result`, or an assertion, for a caller who cannot express the mistake
        this way. The remaining `assert!` is about a facet too large for a `u16`
        coordinate to reach, which is memory that exists to be unreachable.
      - **The layout is checked by round trip, not by a second copy of the
        formula.** `from_blocks` decides where a cell goes and `cell_index`
        decides where it is read from — two separate walks of the same
        column-major order — so building a map out of the loader's own fixture
        and reading every tile back is what catches a transposition. Verified by
        swapping the two block loops: the test names the first cell that moved.
      - **The visible set has to be asserted as a set.** A count is green for a
        walk that draws the wrong tiles as long as it draws the right number of
        them, and a tile's screen position identifies it — `(x - y, x + y)`
        recovers both coordinates — so the comparison is exact. Both halves of
        the fixture are asserted to be real first: tiles with art and tiles
        without.
      - **Height must not move a quad, and that cannot be checked pairwise.**
        Two frames of the same map at different heights come back in different
        *orders*, because height is half the sort key; comparing the lists index
        by index fails on a correct renderer. Compared by position instead.
        Folding `cell.z` into the projection — the bug the comment there warns
        about — fails four of the new tests.

      Left behind: `WorldMap::from_blocks` builds ground only, so
      `statics::collect` is still install-gated. It is the same shape one step
      on — the constructor would take an `IntoIterator<Item = StaticItem>` and
      file each into its block — and it was left out because nothing needs it
      yet and an unused parameter is a worse thing to carry than a missing one.

- [x] **S5. The other half of the restore order.**
      S1 made `restore_characters` before `restore_items` a signature and left
      the next link alone: `restore_items` must run before `restore_mobiles`,
      which equips each NPC and each vendor out of the inventories the items
      filed under its serial. Run them the other way round and every mobile comes
      back naked, its gear filed under a serial nothing will ever ask for — and,
      as with S1, nothing fails at boot or afterwards. The items are in the world,
      bound, reachable by nobody, and the first to notice is a player buying from
      a vendor with no stock.

      The fix is S1's, one step on: `restore_items` returns a `RestoredItems` and
      `restore_mobiles` takes one. The cost S1 named was eight test sites that
      restore mobiles alone; that is the whole of the work beyond the two
      signatures, and it is the reason this was worth judging separately rather
      than riding along.

      **DoD:** the two functions cannot be called out of order without a type
      error; the token carries something the mobiles' restore actually depends
      on rather than being a marker; no test-only constructor for the token (S1's
      reason: an escape hatch inside the crate that defines the order is the
      order back as a convention); `boot::restore`'s doc says what the types now
      say; all four gates silent.

      **Done.** `RestoredItems` carries the filed owners that are *not* among the
      restored characters — which is to say the mobiles with gear waiting, since
      an item's `owner` is the mobile at the top of whatever it is nested in.
      `restore_mobiles` reads it for the boot log: `restore_inventory` already
      returned whether it found anything, so the two numbers are "mobiles that
      found gear" and "inventories that were filed for one", and their difference
      is gear nobody came for. That is the failure this order exists to prevent,
      and the count is the only thing at boot that would say it had happened.

      Two things worth carrying forward:

      - **The eight test sites became one helper, not one constructor.**
        `tests::nothing_restored_first` walks the real restore with nothing in it
        — `restore_characters(Vec::new())` then `restore_items(Vec::new(), …)` —
        and hands back the token. It goes through the API rather than around it,
        which is the distinction S1 drew: a helper that *runs the order* is not
        an escape hatch from it, and it puts the reason in one doc comment
        instead of eight copies of a two-line comment.
      - **`boot::restore_items` returns the token on a store it could not read**,
        for the reason `restore_characters` does. The alternative is an `Option`,
        and an `Option` there hands the caller the question "does no items still
        permit mobiles?" — which is the ordering rule back in prose, in the one
        function whose whole job is to state it.

      Left behind: `seed_configured_characters` is still prose. It runs after
      `restore_characters` so a name in both the store and the config keeps the
      row that describes it, and nothing but `boot::restore`'s doc says so. It is
      the third link and it is *not* the same shape — the rule is about which of
      two writers wins, not about a value one produces for the other — so a token
      would be a marker here, and the honest fix is probably `enrol_character`
      refusing to overwrite a described row, which is an invariant in the roster
      rather than in the boot order. *Taken, and the guess was right about the
      shape and wrong about the fact: S6.*

- [x] **S6. The third link was never a link.**
      S5 left `seed_configured_characters` as prose: it runs after
      `restore_characters` so that a name in both the store and the config keeps
      the row that describes it, and only `boot::restore`'s doc says so. S5 also
      guessed the honest fix — `enrol_character` refusing to overwrite a described
      row, an invariant in the roster rather than in the boot order.

      **It already refused.** `Roster::enrol` returns early on a name that is on
      the list and does not touch the entry; `Roster::remember` enrols first and
      then describes, so a record arriving after a config seeding lands on the
      entry the seeding made. The order genuinely does not decide which
      description survives, and `enrolling_twice_leaves_one_character` had been
      asserting one of the two directions since S5 of `connection_state.md`.
      Fourth stale backlog claim in four stages, second one stale towards
      *already fixed*, and this time the claim was the *reason* for a call order
      rather than a to-do — a rule the code does not have, cited as why a line
      sits where it sits. Checking the entry against the code is now cheap enough
      to be a habit; S2 said so and this is the one that proves it.

      **What the order did decide was the name's spelling**, which nothing had
      noticed. An entry keeps whatever `CharacterName` first enrolled it, and
      `characters()` hands that to `0xA9`. An operator with `characters =
      ["lord british"]` in the config renamed a character the player created as
      `Lord British` — silently, on the character screen, for as long as the
      config seeding happened to run first. Probed before it was fixed: the test
      failed with `left: "lord british"`. `remember` now takes the spelling off
      the record, which is the one the character was created under, so both
      directions of the order leave the same list. The rule is now true by
      construction rather than by call order, and the boot doc says the order
      decides a slot.

      **And the slot it decides was not stable either.** Ask what the ordering
      is *for* and it is the account's character list: the roster enrols in the
      order the store hands rows over, `0xA9` draws that list, and `0x83` picks by
      position in it. `Store::characters` said "Every character" and nothing about
      order, and the three backends each did something different — SQLite's
      `serial INTEGER PRIMARY KEY` is the rowid alias so a bare select was already
      ascending and the rule *looked* held; PostgreSQL returned heap order, where
      an `UPDATE` writes the tuple at the end, so **one logout moved that
      character to the bottom of its own list on the next boot**; and
      `MemoryStore` returned `HashMap` iteration order, a fresh shuffle every
      process, on the backend a shard with no database actually runs. The trait
      now promises ascending serial — creation order, and the only key all three
      have — and all three obey it.

      Three things worth carrying forward:

      - **The bug was in the backend nobody tests against.** SQLite is what the
        tests open and it was the one that happened to be right, so every gate in
        the repository was green over a rule two of three implementations broke.
        The Postgres test that would have failed only runs for someone with
        `OPENSHARD_POSTGRES` set, which is the honest limit of it; the
        `MemoryStore` one runs everywhere and needed eight characters to make an
        accidental pass one arrangement in `8!`.
      - **The SQLite test is labelled green-before-the-fix.** It would have passed
        without the `ORDER BY`, and it says so in its own comment rather than
        standing beside the other two as if it were evidence. What it pins is a
        schema change — a serial that stops being the rowid — which is exactly the
        drift that would otherwise reach a shard first.
      - **An order-independent rule beats a documented order.** S1 and S5 made an
        ordering into a signature because the second function needed a value the
        first produced. Here there was no value, and the answer was not a weaker
        version of the same fix but the opposite one: make both orders agree, and
        the rule stops needing to be stated at all. A stage that cannot be moved a
        whole step up D1's ladder is sometimes a stage whose rule should not
        exist.

      Left behind: `Store::items`, `mobiles` and the rest are still unordered, and
      correctly so — nothing downstream shows them in a list a player indexes.
      That is the reason the promise is on `characters` alone, and it is written
      there so the next backend does not sort everything out of caution.

## Order

S1 and S2 first, and in either order — they are small, they are in `server` and
`crafting`, and neither has been touched by a parallel session in a week.

S3 next. It is the largest and it is new code in a crate with one occupant, so
it collides with nothing.

**And it collided anyway, with another session doing S3.** Two independent
tests of the same chain landed in `crates/e2e/shard/tests/` in one afternoon —
`refused_teardown.rs` and `refused_entry.rs` — because "new code in an empty
crate" reasons about *files*, and what two sessions actually contend for is the
next unticked box in this list. D5 was written about the working tree and this
is the other half of it: **a stage being started is not visible anywhere**.
`refused_teardown.rs` was kept and `refused_entry.rs` deleted, with the two
things the second one did better folded in — see the end of S3. The fix in the
gateway is the part neither test could have been without.

**S4 last, and only when the tree is quiet.** `client/render` and
`common/uofiles` are where the parallel work has been living — `atlas.rs`,
`ground.rs`, `texmaps.rs`, `frame.rs` all moved under this session's feet. D5
is about this: check `git status` for foreign edits in `crates/client/*` before
starting, and if there are any, take S1 or S2 instead. There is no deadline
here and there is no reason to reproduce a mixed working tree on purpose.

**S5 was added after S1–S4 were done**, out of S1's own "left behind", and it
collided with nobody: it is `world/src/tick/persist.rs` and `server/src/boot.rs`,
which the renderer work does not touch. The one thing it does contend for is the
place every stage here contends for — `tick/tests.rs` — and only by three lines
in it.

**S6 came out of S5's "left behind" the same way**, and it is the first stage
here that ended by deleting a rule rather than enforcing one. It sits in
`world/tick/roster.rs` and the three `persistence` backends, and it touched
`tick/tests.rs` not at all.

## Deliberately not in this plan

- **`world/src/tick/tests.rs` is 12,964 lines** — the largest file in the
  repository against a stated ~2k rule, and `architecture.md` has the split
  mechanics written for exactly it. It is left out because it is a *mechanical*
  change to the single hottest file in the tree: every parallel session touches
  it, and a 13,000-line move conflicts with all of them. It is worth doing in a
  session that owns the tree outright, and it is worth doing then rather than
  never — `state/src/runtime.rs` (2,169) and `state/src/components.rs` (2,108)
  are over the line too and are the easier warm-up.
- **The three body-keyed tables** (`body_types.json`, `creature_names.json`,
  `creature_sounds.json`) drifting apart. Real, and it needs a format decision
  first — "these four bodies share a sound but not a name" has to be expressible
  before the merge is an improvement rather than a second problem.
- **`Uop::open` reads 155MB whole.** Harmless for a shard, wrong for a browser,
  and the browser is the deadline. It belongs to whoever takes the renderer to
  the web, not here.

## Status

| Stage | State |
| --- | --- |
| S1 — restore order in the types | done — `RestoredCharacters` is the signature |
| S2 — craft tables fail the build | done — `build.rs` reads both halves |
| S3 — teardown chain, end to end | done — and it found link 4 half-missing |
| S4 — a `WorldMap` without an install | done — `WorldMap::from_blocks`, and the ground tests left the gate |
| S5 — the items→mobiles link | done — `RestoredItems`, the same signature one link on |
| S6 — the config seeding | done — the rule did not exist; the slot order it hid does |
