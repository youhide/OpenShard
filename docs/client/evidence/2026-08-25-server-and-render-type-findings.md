# Server and render type findings

> A record, one of the five the roadmap kept as its client backlog until
> 2026-09-02. What is open is now ranked in [`client/README.md`](../README.md);
> the rows here that belong to a neighbouring domain travel there when that
> domain is migrated.

## Backlog from the server/common/render newtype hunt

A pass over `crates/server/*` and `crates/common/{entities,movement,config,metrics,uofiles}`
plus `crates/client/render` (the crate the client sweep above excluded on
purpose) for the same class of gap: an id or index that already has a name
somewhere and is bare where it crosses a boundary. `entities`, `config` and
`metrics` came back clean — `entities`'s newtypes already follow house style
throughout, `config`'s bare integers are gameplay quantities the protocol
sweep's own ALLOWLIST precedent already excludes, and `metrics` is an
unimplemented stub.

The single largest finding is out of scope for one pass and now has its own
living plan, [`facet_newtype.md`](../../protocol/design_facet.md): **`Facet` —
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
  `opaque_at` families now take `AtlasPixel`, one picture-local coordinate
  shared by static art and animation frames, beside the crate's other named
  pixel spaces (`WorldPixel`, `ViewPixel`, `RealPixel`, `GumpPixel`).

## Backlog: a gump dialog's own captions still can't draw Cyrillic

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

## ~~Backlog: the Chat tab's size knob only reaches the classic face~~ — built

The HUD chat box (journal + compose line) has a Chat tab in the dev window
(`desk::Chat`, `desk::ChatScale`, `shell::chat_panel`): an integer upscale on
`fonts.mul`'s own glyph quads (default 2×, `App::draw`'s `scaled_gump_quads`,
nearest-sampled the same way a camera zoom step grows a world sprite), and a
hue that tints the player's own compose line and caret without touching a
journal row's own server-sent hue.

That knob only ever reached the classic path, and this entry said a real one
for the TrueType path "would have to grow the atlas's own rasterization
height instead of the finished quad — a second, differently-shaped feature".
That is what `docs/render/design_text_sizes.md` built: `TtfAtlas` is keyed by
`(char, TextSize)` rather than baked at one height, so every kind of text has
a **real pixel size** of its own (`desk::FontSizes` — speech, window, tooltip,
stack count), fractional, rasterized at that size and never stretched. The
Chat tab's TrueType half is four pixel sliders now; `ChatScale` stays an
integer and stays `fonts.mul`-only, because a bitmap face has no continuous
size to ask for.
