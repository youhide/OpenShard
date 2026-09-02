# Phase 1: the protocol, as the roadmap recorded it

The roadmap's phase record for the protocol, moved here unchanged when the
domain took its documents. What is open now is ranked in
[the domain README](../README.md); this is what was built and the context that
came with it.

- [x] `PacketReader` / `PacketWriter` — std only, every read fallible
- [x] Client packet length table ported from Sphere's `receive.h` (70 packets)
- [x] `frame_client_packet` — split a TCP stream into packets
- [x] Seed handshake state: old 4-byte form, new `0xEF` form, lone-`0xEF` segment
- [x] Login sequence: `0x80`, `0x82`, `0xA8`, `0xA0`, `0x8C`, `0x91`, `0xA9`
- [x] `0xBD` client version report → `ClientVersion` → `FeatureSet`
- [x] Server→client Huffman compression (Sphere's "golden key" table)

Version-gate everything from the first packet. Retrofitting is the thing this
crate exists to avoid.

The codec deliberately has no dependencies — not even `bytes`. Keeping the
foundation crates dependency-free is what lets them build in environments where
crates.io is unreachable.

## Newtype sweep (`docs/protocol_newtypes.md`) — completed findings

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
