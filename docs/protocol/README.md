# The protocol: where it stands

The canon of the `protocol` domain — `crates/common/protocol`, the crate both
ends of the wire agree on and neither owns. Everything a client and a shard say
to each other is defined here; nothing above it is. `crates/common/movement`
holds the *rule* a walk packet is judged against rather than the packet, so it
is documented where the walk is: [`client/design_walk.md`](../client/design_walk.md)
and [`world/`](../world/README.md).

**One entry point.** This page answers "what does the wire do today" and says
which document holds the reasoning for each line. Where this page and a design
document disagree, the design document is right and this page is stale.

## The one-line answer

**Two sum types, one framing layer, and no bare integer left in either.** Every
message the client can send is a variant of `ClientPacket`, every message the
server can send is a variant of `ServerPacket`, each variant is a newtype around
a named payload implementing `DecodePacket` or `EncodePacket`, and one framing
layer writes the id byte and back-patches the length so no encoder can forget
it. Every field of every payload carries a named type: a client → server field
is a `Raw*` that becomes a domain value only through a named check, a
server → client field is the validated type itself, and what stays a bare
integer is on an allowlist with a reason that a test enforces in both
directions.

**What the wire is not yet:** nothing here has ever been tested against a byte a
real client actually sent, and three of the checks the raw types exist to force
are still missing, because each of them needs a gameplay number this repository
does not have.

## What the crate is, by part

| Part | State | What is left | Held by |
|---|---|---|---|
| `ClientPacket`/`ServerPacket`, the two traits, the framing layer that writes the header once | ✅ shipping | — | [`design_packet_enums.md`](design_packet_enums.md) |
| Version gating: `ClientVersion`, `Feature::since` ported from Sphere's `MINCLIVER_*`, `FeatureSet` | ✅ shipping | boundaries not in the table yet — [`client_versions.md`](../client_versions.md) is the evidence behind the ones that are | the crate's own docs; `feature.rs` asserts every feature is listed exactly once |
| Framing and the codec: `PacketReader`/`PacketWriter` with every read fallible, the client length table ported from Sphere's `receive.h`, `frame_client_packet` | ✅ shipping | — | [`evidence/2026-08-24-the-protocol-phase.md`](evidence/2026-08-24-the-protocol-phase.md) |
| The login sequence and the seed handshake, both forms | ✅ shipping | — | the same |
| Server → client Huffman compression | ✅ shipping | — | the same |
| Login-stream and game-stream encryption | ⬜ deliberately absent | revisit only for a client that cannot be configured without it | [`research_login_encryption.md`](research_login_encryption.md) |
| Named types on every field, `Raw*` for client input, the four classes | ✅ shipping | three promotions the pilot deferred — row 1 | [`design_wire_types.md`](design_wire_types.md) |
| `Facet` carried rather than unwrapped and rebuilt across eight crates | ✅ shipping | — | [`design_facet.md`](design_facet.md) |
| The gump layout DSL as its own type and its own encoder | ✅ shipping | — | [`design_packet_enums.md`](design_packet_enums.md) |
| OpenShard's own packets, in a reserved `0xBF` subcommand range from `0xE000` — the world's chunks, the map editor's commits, house inventory, the craft catalogue, staff authority (the list is the `SUBCOMMAND` constants written off `OPENSHARD_SUBCOMMANDS`) | ✅ shipping | — | `access.rs`'s module docs, and the design of each in its own domain |
| Decoding a `ServerPacket` — the direction only this project's own client needs | 🟡 partial | the decode list is shorter than the encode list and nothing compares them — row 2 | [`client/README.md`](../client/README.md) rows 5 and 6 |

## What is enforced, and by what

The crate carries its own gates, and they are gates rather than habits — each
one reports what it examined, because "no violations found" from a detector that
read nothing has been green here before.

- `tests/bare_integer_fields.rs` — every bare integer field in `src/` is on the
  allowlist with a reason, and every allowlist row still matches something.
  Walks struct fields *and* enum variant bodies, and asserts a floor on the
  files, enums and variants it read.
- `tests/facet_bare_fields.rs` — the same shape for `facet: u8`, workspace-wide,
  with seven allowlisted files.
- `feature.rs` — every `Feature` appears in the lists exactly once.
- `direction.rs` — every byte the client can send names a direction.
- The framer debug-asserts that a fixed-length payload encodes to its declared
  size, which catches a field added to a struct and forgotten in its encoder.

Two crate-wide invariants sit above all of that:

- **Never branch on `Era` for a protocol decision** — ask
  `version.supports(Feature::X)`. Features did not land in era-sized batches, so
  an era check is wrong for most of the clients it covers, and wrong silently.
- **Our own inventions live in one reserved subcommand range** (`0xE000`, under
  `0xBF`), so a stock client ignores them instead of dropping the connection.

The crate is no longer dependency-free — the phase record's "not even `bytes`"
predates three arrivals, each argued where it landed: `miniz_oxide` because
`0xD8` sends a house design deflated (and `flate2`'s C backend is out under
`unsafe_code = "deny"`), `serde` so a content table can hand over a `ClilocId`
or a `SoundId` already typed instead of every call site wrapping a number, and
`serde_json` in the build script that generates the craft presentation
catalogue.

## What is open, ranked

**1. 🚩 Three values a client picks still reach the world unchecked.** The
newtype pilot landed the types and not the promotions: `validate_stats` for the
three starting stats, the starting skill value, and the allowlists a hue and a
hairstyle are checked against. Each `.0` is unwrapped at
`dispatch::create_character` with a comment naming it as a pass-through, which
is deliberately uglier than the bare `u16` it replaced — the gap is grep-able
now instead of invisible. What blocks the fix is not code: every one of the
three needs a real balance number (a stat total and its floors, a starting skill
budget, the set of hairstyles and hues this shard allows) and none of those
exists anywhere in this repository. Inventing them is a content decision.

**2. 🚩 What this engine sends and what it can read are two lists nobody
compares.** `ServerPacket::decode`'s arms are a subset of the encode side's, and
nothing asserts otherwise — so a packet the reader has no arm for is not a
packet skipped, it is a connection ended. It has been found by hand twice
(`0xD6` hung up on a shopper; `0x2E`, `0x74`, `0x9E`, `0x27`, `0x6C` had an
encoder, a table row and no arm), and `0x14`, `0x70`, `0xC0` and `0xBF`'s
subcommands are still in that state. The symptoms are the client's
([`client/README.md`](../client/README.md) rows 5 and 6); the check that would
end the class belongs here, where both tables live.

**3. No test has ever seen a byte a real client sent.** Every byte-level
assertion in the crate compares this crate's output against this crate's own
expectation, which cannot catch a field this project mis-read the same way
twice. Captures from a stock client and from ClassicUO, replayed against the
decoders, are the missing oracle.

**4. The coverage gate cannot see a function's parameters.** `fn encode(serial:
u32)` is the same bare integer and no field scan can find it; that wants `syn`,
and the argument against the dependency has not changed. This is not
theoretical: `GumpLayout`'s three `cliloc: u32` parameters kept producing bare
numbers at every caller for months and were found by reading, not by the gate.
The gap is documented in the test rather than papered over.

**5. Cliloc and sound numbers are literals in Rust, not content-table rows.**
`ClilocId` and `SoundId` are `#[serde(transparent)]`, so a loader can read one
straight into the newtype — nothing about this is a newtype question any more,
it is a Community Pack one.

**6. Three shard-wide hue defaults are constants, not configuration.**
`Hue::SYSTEM`, `Hue::NPC_SPEECH` and `Hue::LABEL` name the client's one muted
grey for three different speakers. Splitting them already caught a bug the
shared literal was hiding; making them `[gameplay]` fields is the cheap
remainder.

**7. Login encryption stays unimplemented.** Sphere's per-version key table is a
real lift that buys obfuscation, not security, and ClassicUO connects with it
off. Revisit only if a client that cannot be configured without it has to be
supported — the argument is in
[`research_login_encryption.md`](research_login_encryption.md).

**8. `ButtonId::CLOSE_BOX` and `ButtonId::UNUSED` are one value with two
meanings.** A third meaning is the trigger to make the type an enum with a
`Reply(u32)` arm. Re-checked across the workspace and the condition has not
fired; both constants now name the collision and the trigger.

## The documents

**Design** — the model as built, no status in them:

- [`design_packet_enums.md`](design_packet_enums.md) — the two roots, the
  framing layer, D1–D10, and the survey of the wire's shape against ClassicUO.
- [`design_wire_types.md`](design_wire_types.md) — direction decides the shape,
  the four classes, where a type lives, N1–N11, and the allowlist with its
  reasons.
- [`design_facet.md`](design_facet.md) — why a facet is carried rather than
  rewrapped, the two seams that stay bare, and the gate.

**Research** — what was read and what was rejected:

- [`research_login_encryption.md`](research_login_encryption.md).

**Evidence** — measurements and closed records; none of them is a status:

- [`evidence/2026-07-29-the-packet-rewrite.md`](evidence/2026-07-29-the-packet-rewrite.md)
  — what the crate looked like as 47 `encode_*` functions and a 719-line
  `match`, and the amendment each of the seven stages forced.
- [`evidence/2026-08-31-the-newtype-sweep.md`](evidence/2026-08-31-the-newtype-sweep.md)
  — the 193-field census, the pilot, and seventeen stages of amendments, ending
  with the gate that now enforces them.
- [`evidence/2026-08-11-the-facet-sweep.md`](evidence/2026-08-11-the-facet-sweep.md)
  — ~70 call sites across eight crates, crate by crate.
- [`evidence/2026-08-24-the-protocol-phase.md`](evidence/2026-08-24-the-protocol-phase.md)
  — the roadmap's phase record, including the five findings the newtype sweep
  raised and the tick, config and state crates closed.
