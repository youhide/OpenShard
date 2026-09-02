# The packet rewrite: from free functions to packet enums

Record of the multi-session rewrite of `crates/common/protocol` that landed the
two root enums, closed 2026-07-29 with Stage 7. The decisions it settled — D1
through D10 and the survey behind them — are
[`design_packet_enums.md`](../design_packet_enums.md); below is what the crate
looked like before, the amendment each stage forced, and the ordering that was
followed.

## What was there before

The crate had two shapes bolted together:

- **Client → server** was a set of unrelated structs, each with its own
  `const ID` and its own `decode`. Nothing tied them together, so
  `server/server/src/dispatch.rs` was a 719-line hand-written `match` over raw
  bytes — including `packet.get(5) == Some(&0x05)` reaching into an undecoded
  packet, and three different `0xBF` types (`context`, `casting`, `mobile`)
  that each re-read the same envelope and each decided independently whether the
  packet was "theirs".
- **Server → client** was 47 free functions named `encode_*`, each returning a
  fresh `Vec<u8>`, each writing the id byte by hand and each patching its own
  length field by hand.

Neither shape was checkable. Nothing told you a packet id was handled twice, or
not at all; nothing stopped a new encoder from forgetting its length patch; the
`match` in `dispatch` had no exhaustiveness to lean on because it matched on
`Option<&u8>`.

## Amendments forced by the Stage 1 pilot

1. **D2 loses its inline case.** "At most four flat scalars go inline in the
   variant" cannot hold: `EncodePacket` has to be implemented *on a type*, so a
   variant with inline fields has no payload to implement it on and would need
   a second body-writing path inside `ServerPacket`. Every payload is now a
   named struct and every variant is a newtype around one.
2. **`EncodePacket` gained `const LENGTH: PacketLength`.** D3 wants the
   framing layer to write the length field, and the framer cannot know
   whether there is one without asking the payload. It pays twice:
   `frame_body` debug-asserts a fixed packet's body size, catching a field
   added to a struct and forgotten in its encoder.
3. **Stage 1 does not introduce `BodyId`,** and proves neither a
   variable-length packet nor a list-carrying one — none of
   `target`/`combat`/`feedback` has any of the three. The variable-length path
   is exercised by a unit test in `packet.rs` instead; the real proof moves to
   Stage 2 (`login`: shard list, character list). D6's own rule — a newtype
   arrives with the packet that needs it — beats the stage bullet that
   promised `BodyId`.
4. **`Option<Serial>` is the shape of an empty object field.**
   `TargetResponse.object`, `AttackRequest.target`, `AttackTarget.target`,
   `GraphicalEffect.from`/`to`.
5. **Sound ids stop at the packet boundary.** `WorldState::play_sound` still
   takes a bare `u16` and wraps it in `SoundId` where the packet is built.
   Converting the sound *tables* (spell definitions, creature voices,
   instrument notes, the scripting op) is its own sweep and would drag serde
   into the protocol newtypes. Same for `Graphic` at the spell-visual sites.
6. **`EffectPoint` is gone** — it was `world::Point` field for field, and the
   effect packets now use `Point`.

## Amendments forced by the Stage 2 pilot (`login`)

Stage 2 is where the variable-length path first carries a real payload — the
shard list and the character list are both `Vec<T>` bodies, not the unit test
Stage 1 covered it with — and where a packet finally does not fit
[`D3`](../design_packet_enums.md#decisions)'s
Fixed/Variable split at all.

1. **`decode_packet` now skips a variable packet's length field itself,**
   rather than leaving it to each `decode_body`. `ClientVersionReport` (`0xBD`)
   is the first variable client-to-server packet migrated, and without this
   its body would start two bytes early. The check belongs in exactly one
   place: `frame_client_packet` has already validated the claimed length
   against the buffer and `MAX_PACKET_SIZE` by the time a decoder runs, so
   `decode_body` gets bytes that are already known-good and never re-checks
   the length itself. One consequence worth being explicit about: a
   `decode_packet` call fed raw bytes that skip framing (as a unit test can)
   no longer rejects a length field that lies — that check now lives once, at
   the framing layer, not duplicated in every variable decoder.
2. **`0xB9` (`encode_supported_features`) stays a free function, not an
   `EncodePacket`.** It has no length field at all — unlike every other
   variable packet — and its size (3 or 5 bytes) depends on the client
   version, which `EncodePacket::LENGTH` cannot ask about because it is a
   `const`. Neither `Fixed` nor `Variable` describes it. This is the
   server-to-client mirror of `0x08`'s problem on the decode side
   (`client_packet_length` takes a version for exactly this reason); until the
   framing layer can express "fixed, but which fixed size depends on the
   version" for both directions at once, `0xB9` is written by hand rather than
   forced into a model it does not fit.

## Amendments forced by the Stage 3 pilot (`world`, `mobile`)

Stage 3 is the first to hit a packet whose *id* is shared by two logically
different bodies (`CreateCharacter`), and the first to find `0xBF` packets
that are fixed-size despite the id's own table entry saying `Variable`.

1. **`CreateCharacter` (`0x00` / `0xF8`) is not a `DecodePacket`, for the same
   reason `0xB9` is not an `EncodePacket` (Stage 2).** `DecodePacket` assumes
   one `const ID`; this packet is one logical decode across *two* ids with two
   different fixed lengths (104 bytes/three skills vs. 106 bytes/four). Bending
   the trait to accept an id list, or picking one id arbitrarily, would either
   complicate every other decoder for one packet's sake or silently stop
   accepting the id it didn't pick. `CreateCharacter::decode` stays a plain
   inherent method, exactly as surveyed.
2. **Two more `0xBF` packets turned out fixed, not variable, and both still
   hand-write their own length field.** `world::MapChange` (subcommand `0x08`,
   always 6 bytes) and `mobile::StatLocks` (subcommand `0x19` type `2`, always
   12 bytes) never carry a list or a version branch, so `EncodePacket::LENGTH`
   is `Fixed`, not `Variable` — simpler, and it gets the `frame_body` debug
   assert on total size for free. The one wrinkle: `frame_body` only
   back-patches a length field for `Variable`, so these two bodies still write
   their own constant `u16` length literal by hand, in exactly the spot the
   `0xBF` envelope always puts one. That hand-written literal and
   `EncodePacket::LENGTH` now have to agree by construction rather than by a
   shared mechanism — the same kind of two-places-that-could-disagree gap D3
   exists to close, just not one the trait as designed can close for an id
   whose *table* entry is `Variable` but whose *body* never is. Noted here
   rather than silently declaring `Variable` (which would insert a length field
   the client already gets from the subcommand's fixed shape, doubling it).
3. **`MobileStatus` and `MobileIncoming` matched the plan exactly:** both were
   already self-patching their length by hand at the same offset `frame_body`
   patches for `Variable`; converting them to `EncodePacket` with
   `LENGTH = PacketLength::Variable` let the manual `writer.u16(0)` placeholder
   and the closing `bytes[1..3].copy_from_slice(...)` come out unchanged in
   behaviour, byte for byte.
4. **`StatLockRequest` was left exactly as surveyed:** it already had the
   `0xBF`-envelope shape (`decode(bytes) -> Result<Option<Self>, DecodeError>`)
   that several unrelated logical packets share one id under, and forcing it
   into `DecodePacket` would be Stage 6's unification arriving four stages
   early.

## Amendments forced by the Stage 4 pilot (`items`, `containers`, `vendor`, `properties`, `skill`)

Stage 4 is the first group where a fixed-size packet's *size itself* is
version-conditional in two more places, and the first where a payload is a
streaming builder rather than a value `EncodePacket` can wrap.

1. **`0x24` (`encode_open_container`) and `0x25` (`encode_add_to_container`)
   stay free functions, for the same reason `0xB9` did in Stage 2:** each is
   fixed-length, but which fixed length depends on `version`
   (`Feature::HsPackets`, `Feature::ItemGrid`), and `EncodePacket::LENGTH` is a
   `const` that cannot ask a payload's own `version`. `0x3C`
   (`ContainerContents`) does not have this problem — it is genuinely
   `Variable`, version only changes the per-item record's shape inside the
   body — so it became an `EncodePacket` as planned.
2. **`0x08`'s decode moved from inspecting `bytes.len()` to asking
   `version.supports(Feature::ItemGrid)` directly.** The framer already made
   this choice before `DropItem::decode_body` runs (`client_packet_length`
   picks `Fixed(14)` or `Fixed(15)` on the same feature), so re-deriving it
   from the buffer length a second time was one check the trait's `version`
   parameter makes unnecessary, not a behaviour change.
3. **`PropertyList` (`0xD6` outbound) stays a hand-written builder, not an
   `EncodePacket`.** It accumulates a hash across an unknown number of
   `add`/`add_args` calls and returns that hash alongside the bytes on
   `finish` — `EncodePacket::encode_body` assumes a value that already knows
   its whole body, with nothing to hand back but the bytes. Forcing the
   builder into that shape would mean either a payload struct holding a
   pre-built entry list (losing the streaming hash-as-you-go property that
   keeps the arithmetic auditable) or a second, parallel encoding path.
   `encode_opl_info` (the *other*, stateless half of the pair, `0xDC`) had no
   such obstacle and became `TooltipRevision`.
4. **`UseSkillRequest` (`0x12`) was left exactly as surveyed,** for the same
   reason `StatLockRequest` was in Stage 3: `0x12` is a text-command envelope
   several unrelated commands share, and `decode(bytes) -> Result<Option<Self>,
   DecodeError>` already says "not mine" without an error for the ones this
   crate does not act on.
5. **The skill list split into two payload types sharing one id,
   `SkillsFull` and `SkillUpdate` (`0x3A` both directions out).** They are not
   the same packet at different sizes — one is the absolute, one-based,
   zero-terminated full window; the other a zero-based delta with no
   terminator — so one struct with an `is_update: bool` field would have
   meant an `encode_body` with two unrelated bodies gated on a flag. Two
   structs, same `ID`, is unremarkable: nothing about `EncodePacket` requires
   ids to be unique across variants, only that each variant's own `ID` is
   right.
6. **`caps` stopped being a parameter and became something `encode_body`
   derives from `version` itself,** for `SkillsFull`, `SkillUpdate` and
   (already, since D4) every other payload: `version.supports(Feature::SkillCaps)`
   is exactly the kind of derived, redundant flag the call site used to have
   to compute and pass correctly by hand. Letting the trait's own `version`
   parameter answer it removes a place the caller's flag and the packet's
   actual shape could disagree.
7. **`items`, `containers`, `vendor`, `properties` and `skill` all left the
   `lib.rs` re-export wall (D8), same as `world`/`mobile`/`login` did in
   Stage 3.** Their call sites now import from the defining module
   (`openshard_protocol::items::WorldItem`, not `openshard_protocol::WorldItem`)
   — this happened one stage earlier per module than D8 originally scheduled
   it (Stage 7), because leaving a *rewritten* module in the wall while its
   neighbours were already out was the inconsistency D8 exists to prevent,
   not a reason to defer it.

## Amendments forced by the Stage 5 pilot (`speech`, `gump`, `spellbook`, `context`, `casting`)

Stage 5 is the first group built entirely out of `0xBF` subcommands and their
own root-level messages, with no fresh newtype and no version-conditional
tail — the pattern questions here are all about which packets fit `EncodePacket`
at all, not about wire shapes.

1. **`CastSpellRequest`, `ContextMenuRequest` and `ContextMenuSelect` are left
   exactly as surveyed,** for the same reason `StatLockRequest` and
   `UseSkillRequest` were in Stages 3 and 4: `0xBF` is a whole family of
   subcommands sharing one id, so `decode(bytes) -> Result<Option<Self>,
   DecodeError>` already says "not mine" for the ones a given type does not
   handle. `DecodePacket` assumes one `const ID` maps to one logical packet,
   which none of these three are — merging them into a single decode is
   Stage 6's `ExtendedRequest` unification, not this stage's.
2. **`ContextMenu` (`0xBF` `0x14`, outbound) is the first *variable*-length
   `0xBF` subcommand to become an `EncodePacket`.** Unlike `MapChange` and
   `StatLocks` (Stage 3) or `CloseGump` and `SpellbookContent` below, its entry
   count is the caller's, so it declares `LENGTH = PacketLength::Variable`
   rather than a hand-rolled `Fixed`. No special-casing was needed: the
   `0xBF` envelope's own length field sits at the same offset every packet's
   does, so `frame_body` patches it exactly as it would for any other variable
   payload.
3. **`CloseGump` (`0xBF` `0x04`) and `SpellbookContent` (`0xBF` `0x1B`) are
   `Fixed`, and both still hand-write their own length literal,** for the same
   reason `MapChange` and `StatLocks` did in Stage 3: `frame_body` only
   back-patches a length field for `Variable`, so a fixed-size subcommand
   under the `0xBF` envelope writes its own constant `u16` in exactly the spot
   the envelope always puts one.
4. **`GumpLayout` stays a hand-written builder, not an `EncodePacket`,** for
   the same reason `PropertyList` did in Stage 4: it accumulates elements
   (and interns their text) across an unbounded number of calls, with nothing
   to hand back until `GumpLayout::finish` but the two
   half-built pieces. `GumpDisplay` (`0xB0`), which takes the *finished*
   layout string and line table, has no such obstacle and became the
   `EncodePacket`.
5. **The three outbound speech packets became named payload structs** —
   `SpokenMessage` (`0x1C`), `LocalizedMessage` (`0xC1`), `UnicodeMessage`
   (`0xAE`) — replacing `encode_message`, `encode_localized_message` and
   `encode_unicode_message`. None needed a new wire newtype: every field is
   already the shape Stage 1's `SoundId`/`Graphic`/`Hue` covered or a plain
   scalar, and D6's own rule — a newtype arrives with the packet that first
   needs it — had nothing left to add here.
6. **`GumpResponse` (`0xB1`) becomes a `DecodePacket`, and its manual
   length-skip is gone.** The old `decode` skipped the `u16` length field by
   hand because it read the whole packet itself; `0xB1` is `Variable` in
   `client_packet_length`, so `decode_packet` already skips those two bytes
   before calling `decode_body` (per Stage 2's amendment), and skipping them a
   second time would just be the same check done twice.
7. **All five modules left the `lib.rs` re-export wall (D8)** in this stage
   rather than waiting for Stage 7, the same one-stage-early move Stage 3 and
   4 made for their own groups: leaving a freshly rewritten module in the wall
   while its neighbours were already out would be the inconsistency D8 exists
   to prevent.

## Amendments forced by the Stage 6 pilot (`ClientPacket`, `dispatch.rs`)

Stage 6 is the first to decode a whole packet family in one place rather than
one module at a time — `ClientPacket` covers everything `dispatch.rs` acts on
— and the first to have to decide what a decode failure means once decoding
no longer sits behind the `session.in_world` gate that used to run first.

1. **`ClientPacket::decode` is unconditional, ahead of every `in_world`
   check.** Before this stage, most of `dispatch.rs`'s arms checked
   `session.in_world` *before* decoding, so a malformed packet arriving
   before world entry was never even read — silently accepted, not silently
   dropped. Decoding once at the edge means that check can no longer run
   first, and a client sending unparseable bytes on a recognised id now
   drops the connection regardless of world state. Deliberate, not
   incidental: a client sending bytes that do not decode is not one this
   shard has a reason to keep trusting, at any point in the conversation.
   No existing test exercised the old, more permissive timing.
2. **`ExtendedRequest` collapses `CastSpellRequest`, `ContextMenuRequest`,
   `ContextMenuSelect` and `StatLockRequest` (Stages 1, 5) into one `0xBF`
   decode.** Each used to read the id, length and subcommand for itself and
   decide independently whether a given `0xBF` was its own — the "three
   different 0xBF types … each re-read the same envelope" duplication the
   top of this document calls out. Every one of the four keeps its payload
   struct and its `SUBCOMMAND` constant, but trades its standalone
   `decode(bytes) -> Result<Option<Self>, DecodeError>` for a
   `pub(crate) decode_body(reader: &mut PacketReader<'_>) -> Result<Self,
   DecodeError>` that only `ExtendedRequest::decode` calls, with the reader
   already past the subcommand. An unrecognised subcommand reads as
   `ExtendedRequest::Unknown(subcommand)`, not an error — the same shape as
   `ClientPacket::Unknown` — where the old per-type probing simply did
   nothing and logged nothing if none of the three matched.
3. **`0xD7` does not get its own sub-enum.** The plan named
   `EncodedRequest` as `0xBF`'s sibling collapse, but `EncodedCommand`
   was already exactly one type for exactly one id — nothing was probing the
   same envelope twice the way the three `0xBF` types were. `ClientPacket::
   Encoded(EncodedCommand)` wraps it unchanged; `dispatch` still matches on
   `command.subcommand` by its `*_REQUEST` constants, same as before
   `EncodedCommand` moved out of the `lib.rs` wall (D8) alongside it.
4. **`mobile::StatusQuery` is new** — `0x34` had no payload type at all
   before this stage, just `dispatch` reaching into the raw buffer with
   `packet.get(5) == Some(&0x05)`. `StatusQuery` models only the one bit
   `dispatch` reads, `kind: StatusQueryKind`, not the magic word or the
   queried serial: nothing downstream ever used the serial (every query this
   engine acts on is about the asking connection's own mobile), and D6 does
   not ask for a field nothing reads.
5. **`UseSkillRequest`'s `Ok(None)` (a `0x12` text command that is not "use
   skill") folds into `ClientPacket::Unknown`,** rather than `ClientPacket`
   growing an `Option`-shaped variant. `dispatch` special-cases
   `Unknown { id: UseSkillRequest::ID, .. }` to keep the one debug log the
   old code had for it; every other `Unknown` id logs nothing, as before —
   `dispatch` runs on *every* packet, including the login conversation's own
   ids (`0x80`, `0xA0`, `0x91`, `0xBD`), and logging those as "unhandled"
   would be noise on every normal connection, not a diagnostic.
6. **`ClientPacket` and `ClientDecodeError` are `#[non_exhaustive]`,
   matching `ClientLoginPacket`/`ClientLoginDecodeError`** (the login
   conversation's own version of this pattern, added just ahead of this
   stage). Both `dispatch`'s match on `ClientPacket` and its inner match on
   `ExtendedRequest` (also `#[non_exhaustive]`) end in `_ =>
   unreachable!(...)`, the same "every variant that exists today is matched
   above" comment `LoginServer::handle` uses.
7. **`encoded` and `extended` leave the `lib.rs` re-export wall (D8)
   alongside `client_packet`,** the same one-stage-early move every prior
   stage made for the modules it touched.

## Amendments forced by the Stage 7 cleanup

Stage 7 is the only stage with no packets left to convert — it closes the two
mechanical bullets the plan named for it, and both turned out smaller than
expected.

1. **"Delete the last `encode_*`" was already done.** Every stage from 2
   onward left behind exactly the free functions its own amendments named as
   settled exceptions (`0xB9`'s `encode_supported_features`, `0x24`/`0x25`'s
   `encode_open_container`/`encode_add_to_container`). Stage 7 found no
   forgotten leftovers — those three are the only free `encode_*`/`decode_*`
   functions in the crate, and all three stay for the reasons already on
   record.
2. **The `pub use` wall (D8) was real, not vestigial.** Seven modules —
   `access`, `codec`, `direction`, `error`, `feature`, `seed`, `version` —
   were still private, with `pub use` in `lib.rs` as the only path out; twelve
   files across `crates/server` and `crates/common/movement` reached them
   through the crate root. All seven became `pub mod`, the wall came out, and
   every one of those call sites (plus the crate's own doc-tests and a couple
   of intra-crate `crate::Feature`/`crate::client_packet_length` shortcuts
   that had been leaning on the same wall from the inside) now imports from
   the module the type is defined in. `packet`'s own entry in the wall
   (`client_packet_length`, `frame_client_packet`, `Frame`, `FrameError`,
   `PacketLength`, `MAX_PACKET_SIZE`, `SEED_LENGTH_*`) was already redundant
   before this stage — `packet` has been `pub mod` since Stage 1 — so removing
   it is a pure simplification, not a behaviour change.
3. **`docs/architecture.md` needed no edits.** It never described the old
   47-function/hand-written-`match` shape in the first place; it stayed at a
   level of abstraction ("packet dispatch", "the login and world packets")
   that the rewrite didn't invalidate. The "update the crate docs" bullet
   turned out to mean `crates/common/protocol/src/lib.rs`'s own `# Status`
   section, which still said "individual packet types are not [written]" —
   stale since before Stage 1 — plus five doc comments elsewhere
   (`gump.rs`, `feature.rs`, `tick.rs`, `dispatch.rs`, `session.rs`) still
   naming `encode_gump_display`, `encode_shard_list`, `encode_relay` and
   `encode_logout_ack` — functions Stages 1-6 replaced with `EncodePacket`
   types but whose doc comments elsewhere were never updated to match.

## Stages

Each stage ends with all four silent: `cargo check --workspace --all-targets`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo fmt --all`. Each stage is one or more commits on `main`.

- **Stage 0 — the rename.** `DecodeError`, `WrongPacket` and `expect_id` out of
  `login.rs` into `error.rs` (D7). Nothing else: the traits and the newtypes
  land with the first packets that use them, per D6, rather than as a layer
  written against packets nobody has re-read yet.
- **Stage 1 — pilot: `target`, `combat`, `feedback`.** Smallest groups, fewest
  call sites. Brings in with them: the `Serial` move (D6), the first wire
  newtypes (`SoundId`, `CursorId`, `Graphic`, `Hue`), the two traits (D4) and
  the framing layer (D3), plus the `ServerPacket` root enum with its first
  variants. The variable-length path is covered by a packet unit test; the real
  variable/list packet proof moves to Stage 2's login lists. If D2/D3 are wrong,
  this is where it shows and this document changes before anything else is
  migrated.
- **Stage 2 — `login`.** The most version-conditional group (shard list,
  character list, feature flags) and its own dispatch path in `server/login`.
- **Stage 3 — `world`, `mobile`.** The largest and hottest: movement, status,
  `MobileIncoming`, equipment. Flag-conditional fields land here.
- **Stage 4 — `items`, `containers`, `vendor`, `properties`, `skill`.**
  List-heavy, mostly mechanical after Stage 3.
- **Stage 5 — `speech`, `gump`, `spellbook`, `context`, `casting`.** Includes
  the gump layout DSL as its own type.
- **Stage 6 — `ClientPacket` and `dispatch.rs`.** Decode once at the edge, then
  a single exhaustive `match` over `ClientPacket`. The `0xBF`/`0xD7` envelopes
  collapse into `ExtendedRequest`/`EncodedRequest` sub-enums here — the three
  separate `0xBF` types (`context`, `casting`, `mobile`) merge.
- **Stage 7 — cleanup.** Drop the `pub use` wall (D8), delete the last
  `encode_*`, update `docs/architecture.md` and the crate docs.

## Progress

| Stage | State | Commit |
| --- | --- | --- |
| 0 | done | `153e1f8` |
| 1 | done | `daad3e0` |
| 2 | done | `77ba897` |
| 3 | done | `1c94006` |
| 4 | done | `d483bb3` |
| 5 | done | `0d39525` |
| 6 | done | `ca20428` |
| 7 | done | `5b74452` |

## What this plan left for the next one

[D6](../design_packet_enums.md#decisions) — newtypes on the wire — was scoped to "a newtype arrives with
the packet that first needs it", which is why `Serial`, `Graphic`, `Hue`,
`SoundId`, `CursorId` and `AuthKey` exist and 193 other packet fields are still
bare integers. Finishing that, and adding the raw-versus-validated split those
fields need to say whether anyone checked them, is
[`protocol_newtypes.md`](../design_wire_types.md).
