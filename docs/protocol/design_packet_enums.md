# Packet enums: two roots and one framing layer

The wire format is a fixed external contract with a closed set of messages, so
`crates/common/protocol` is two sum types. `ClientPacket` is every message the
client can send, decoded once at the edge; `ServerPacket` is every message the
server can send. One framing layer writes the id byte and back-patches the `u16`
length, so a payload encoder writes body only and the length table is the single
source of truth for both directions.

The shape this replaced — a 719-line hand-written `match` over raw bytes on one
side, 47 free `encode_*` functions each patching its own length on the other —
and the seven stages that got here are
[the packet rewrite](evidence/2026-07-29-the-packet-rewrite.md). What a field
*means*, and whether anyone checked it, is
[`design_wire_types.md`](design_wire_types.md).

## The shape of the protocol (surveyed against ClassicUO)

ClassicUO is the reference: `src/ClassicUO.Client/Network/PacketsTable.cs` is
the 256-entry server-packet length table, `PacketHandlers.cs` (7161 lines) is
every server packet parsed, `OutgoingPackets.cs` (4671 lines) every client
packet built.

**There is no recursion.** The nesting is finite and shallow:

1. **id byte** — 256 slots.
2. **subcommand** — a handful of envelopes: `0xBF` general information (`u16`
   subcommand, ~40 defined), `0xD7` encoded command (`u16`), `0xB5`/`0xB3` chat
   (`u16`, `0x03E8..0x03F4`), `0x12` (type byte).
3. **third level, rare** — `0xBF 0x06` party has its own byte subcommand
   (`PartyManager.ParsePacket`); `0xBF 0x16` close-window keys on a window id;
   `0xBF 0x19` extended stats keys on a `version` byte (0/2/5), and inside
   version 5 branches again on `type2 == 0xFF` — effectively a fourth level.

So: an enum of enums, no `Box`, no cycles.

Three things complicate it beyond plain nesting, and the type design has to
carry all three from the start rather than retrofit them:

- **Repeated records** — container contents, skill lists, shard and character
  lists, buy/sell lists. These are `Vec<T>` of a named row type.
- **Fields conditional on bit flags** — `0x1A` world item hides the presence of
  count, hue and flags in the high bits of the serial and graphic; `0x77`/`0x78`
  do the same. Presence is data, so the optional parts are modelled as fields
  that are semantically absent, not as defaulted zeros.
- **Version-conditional tails** — `0x11` status, `0x78`/`0xD3`, `0xA9` change
  shape by era. The branch is on `ClientVersion::supports(Feature::…)`, never on
  `Era` and never on a version comparison (see the crate docs).

One genuine grammar exists: **gump layout** (`0xB0`, compressed `0xDD`) is a
text DSL — `{ gumppic 0 0 100 }{ page 1 }…` — plus a string table. It is not
recursive (pages are flat sections) but it is a language, and it gets its own
type and its own encoder rather than a variant that carries a pre-built string.

## Decisions

Settled. They are what a new packet follows; changing one means changing the
crate, not this document.

**D1. Two root enums.** `ClientPacket` (decoded, client → server) and
`ServerPacket` (encoded, server → client). Both non-exhaustive.

**D2. Variant payloads.** Every variant is a newtype around a named payload
struct (`Status(MobileStatus)`, `WarMode(WarMode)`). The pilot disproved the
inline exception: [`EncodePacket`] is implemented on a payload type, so inline
variant fields would need a second body-writing path inside the root enum.
One shape for every variant keeps the framing layer mechanical.

**D3. The header is written once.** Payload encoders write **body only**. A
single framing layer writes the id and, for variable packets, back-patches the
`u16` length. This deletes the whole class of "forgot to patch the length"
and makes the length table the single source of truth for both directions.

**D4. Traits.**

```rust
pub trait EncodePacket {
    const ID: u8;
    const LENGTH: PacketLength;
    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion);
}

pub trait DecodePacket: Sized {
    const ID: u8;
    fn decode_body(reader: &mut PacketReader, version: ClientVersion)
        -> Result<Self, DecodeError>;
}
```

`ClientVersion` is passed to every encoder and decoder uniformly, even where it
is unused — a packet that grows a version-conditional tail later must not
change its signature and every call site with it.

`EncodePacket::LENGTH` tells the framing layer whether a payload is fixed or
variable length. For fixed packets, the framer debug-asserts that the encoded
body matches the declared size, catching a field added to a struct but forgotten
in its encoder.

**D5. Nothing is silently dropped.** `ClientPacket` has an
`Unknown { id: u8, body: Vec<u8> }` variant. An unhandled id is a logged fact,
not a dropped connection and not a silent `true` return.

**D6. Newtypes on the wire.** `Serial`, `Graphic`, `Hue`, `Layer`, `SoundId`,
`GumpId`, `CursorId`, `CliLocId`, `MusicId`. Bare `u32`/`u16` fields
are gone from packet definitions; `.0` is unwrapped only inside the codec.
`Serial` lives in `common/protocol` and not in `common/entities` — it is a wire
concept first, so `entities` depends on `protocol` for it and never the reverse.

A newtype arrives with the packet that first needs it, in `wire.rs`, rather than
as a layer written up front: a type nothing uses yet is a guess about a packet
nobody has read closely, and it hardens before it is right. What that leaves —
every *other* field — is [`design_wire_types.md`](design_wire_types.md)'s
subject.

`Option<Serial>` is the packet shape for an absent object field. A zero object
serial on the wire decodes to `None`, and an absent object encodes as zero.

**D7. `LoginDecodeError` is renamed `DecodeError`.** It was never login-specific
(56 references across the workspace); the name lies about scope.

**D8. No re-exports.** `lib.rs` is not a wall of `pub use` (CLAUDE.md:
re-exports hide where a type lives). Modules are `pub` and call sites import
from the defining module. It was taken as one mechanical sweep in the cleanup
stage rather than drip-fed, which is also how the `lib.rs` files elsewhere in
the workspace that still carry one should go — [`style.md`](../style.md) says
so at the same rule.

**D9. No compatibility shims.** Each stage rewrites a group of packets **and**
updates every call site of that group in the same commit. No `#[deprecated]`
wrapper layer: a half-migrated crate with two ways to send a packet is worse
than a bigger diff.

**D10. Byte-level tests are the contract.** Every existing encoder test keeps
asserting the same bytes; only the call that produces them changes. A stage
that cannot keep the bytes identical has found a bug — fix it deliberately and
say so in the commit, do not adjust the expectation quietly.

