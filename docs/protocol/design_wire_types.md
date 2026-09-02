# Wire types: raw off the wire, validated at the seam

Every field of a packet struct in `crates/common/protocol` carries a named type.
A client → server field carries a `Raw*` that becomes a domain value only by
passing through a named check; a server → client field carries the validated
type directly. What stays a bare integer is on an allowlist with a reason, and
[`bare_integer_fields.rs`](../../crates/common/protocol/tests/bare_integer_fields.rs)
fails if the crate and that allowlist disagree in either direction.

This is the second half of [D6](design_packet_enums.md#decisions), which the
packet rewrite left deliberately half-done: a newtype arrived only with the
packet that first needed it, so `Serial`, `Graphic`, `Hue`, `SoundId`,
`CursorId` and `AuthKey` existed and every other field was a bare integer. The
stage-by-stage record of closing that — the census it started from, and the
amendment each stage forced — is
[the newtype sweep](evidence/2026-08-31-the-newtype-sweep.md).

## The two things a name carries

**A bare integer does not say what it is.** A hue and a graphic are both `u16`;
a skill id and a stat value are both `u8`. Nothing but a reader's attention
stops `Hue(create.hair)` from compiling. This is what D6 already argued.

**A bare integer off the wire does not say whether anyone checked it.** This is
the sharper half, and the reason the work was worth a plan rather than a `sed`
run. Before it, `dispatch::create_character` took the three starting stats, each
skill value and five hues straight off the wire and into the world unread: a
client that sent `skill value = 255` got a skill at 2550, and any `u16` was a
legal skin hue, staff-only ones included. CLAUDE.md's rule — *"a packet is not
an invariant, it is a hostile input"* — was stated and not enforced, because the
type system was never asked to carry the distinction. Bare `u8` reads exactly
the same whether it was validated or not, so the absence is invisible at the
call site and stays invisible in review.

So: every client-supplied field gets a `Raw*` type that can only become
something meaningful by passing through a named check. The check is the thing
being added; the newtype is what makes its absence a compile error.

## Decisions

Settled. They are what a new packet field follows; changing one means changing
the crate, not this document.

**N1. Direction decides the shape.** A client → server field carries a `Raw*`
type. A server → client field carries the *validated* domain type directly
(`Hue`, `Graphic`, `Serial`, `Skill`…) — the server does not send itself
hostile input, and a `Raw` on an outbound packet would be a lie about where the
check happened. Packets that go both ways (`0x3A` skills, `0xBF` subcommands)
follow the direction of the *struct*, not the id.

**N2. Validation lives on the seam, never in `decode_body`.** Decoding stays
what it is: byte shape only. Promotion is a named method called by the code
that acts on the packet — `dispatch.rs`, `openshard_login`, a tick system.

Three reasons this is the split and not the other one:

- A value outside its domain is a *gameplay* refusal, not a framing failure. It
  answers with a `0x82`, or is ignored, or is clamped with a log line. Making
  `decode_body` return `Err` would drop the connection instead
  ([Stage 6 amendment 1](evidence/2026-07-29-the-packet-rewrite.md#amendments-forced-by-the-stage-6-pilot-clientpacket-dispatchrs)),
  which is right for bytes that do not parse and wrong for a hue nobody offered.
- Most domains are not the protocol's to know. A skill id's meaning lives in
  `openshard_state::Skill`; a starting-stat cap lives in `[gameplay]` config; the
  set of legal hairstyles is Community Pack content. `common/protocol` is below
  all of them and must not learn any of it.
- It is the shape the crate already uses and likes: `RawCharacterName` →
  `CharacterName` through `Accounts::create_character`, `RawAccessLevel` →
  `AccessLevel` in `openshard_config`. See `identity.rs`'s module docs.

**N3. Four classes of field, and the class fixes the recipe.** A cheap agent
must never have to decide what "meaningful" means. It classifies, and the class
says what to write.

| class | what it is | the type | the promotion |
|---|---|---|---|
| **A — already named** | the value is a `Serial`/`Graphic`/`Hue`/`SoundId`/… and the server chose it | the existing newtype, no `Raw` | none needed |
| **B — total interpretation** | every bit pattern means something, including "something odd" | `RawX(pub u8)` | `fn interpret(self) -> X` — total, no `Result`; the leftover arm is an explicit `X::Unknown(n)` or a documented safe default |
| **C — fallible validation** | out-of-domain values exist and must be refused, clamped, or ignored | `RawX(pub u16)` | `fn validate(self, …) -> Result<X, InvalidX>` — the context argument is whatever the rule needs (a config cap, a list length) |
| **D — opaque, never read** | the client claims it and the server has no use for it | `RawX(pub u32)` with **no** promotion method | none, on purpose; the doc comment says "never trusted / never read", and the type is the record of that decision |

Class D is why the sweep can be mechanical: `client_ip`, client `flags`, echoed
constants all get a named type and *no* second step, so "no bare integer in a
packet struct" stays a rule an agent can satisfy without judgement, and a
reviewer can grep for `Raw` types with no promotion to find every field the
server is choosing to ignore.

**N4. Where a type lives.**

- A `Raw*` used by **one** module lives in that module, next to its packet.
- A `Raw*` used by **two or more** lives in `wire.rs`, next to `Hue`/`Graphic`.
- A **validated** type lives where its *rule* lives: in `protocol` when the
  client's own wire format fixes the domain (`Sex`, `Race`), in the server crate
  that owns the rule when the shard does (`StartingStats` and its cap,
  `openshard_state::Skill`).

**N5. Field visibility follows the invariant.** A `Raw*` carries no invariant,
so its field is `pub`, exactly as `RawCharacterName(pub String)` is. A validated
type that carries one keeps its field private behind a named
constructor/accessor pair, the way `Serial::new`/`Serial::raw` do. Never `From`,
`Into`, or `Deref` on either — CLAUDE.md, non-negotiable.

**N6. Promotion methods have two names and only two.** `interpret` for class B,
`validate` for class C. Uniform on purpose: an agent that invents
`classify`/`check`/`resolve`/`to_domain` per module produces a crate nobody can
grep. A promotion that reads several fields at once (starting stats are one
rule across three bytes) is a method on the *packet*, still named `validate`
with a qualifier: `CreateCharacter::validate_stats`.

**N7. Errors are typed, per promotion.** `InvalidHue`, `InvalidStartingStats`,
`InvalidSkillChoice`. No `String`, no shared `InvalidValue` catch-all, and
**not** `DecodeError` — these are not decode failures and must not be
convertible into one, or N2's split collapses back into a dropped connection.

**N8. The byte-level tests do not change.** D10 from the rewrite still holds:
every existing encode/decode test asserts the same bytes, only the value
constructing them gains a wrapper. A stage that cannot keep the bytes identical
has found a bug — fix it deliberately and say so in the commit.

**N9. Every stage adds the test that proves the split.** For each class C field
introduced, one test that an out-of-domain value **decodes cleanly** and is
**refused at promotion**. That pair is the whole point of the design; a stage
without it has added wrappers and no checks.

**N10. Coverage is counted, not assumed.** Each stage's commit message records
the bare-int-field count in the files it touched, before and after, and the
crate carries a repo-level check that counts them across `src/` and asserts
every remaining one is on an explicit allowlist with a reason. The check also
reports what it examined — files, enums, variants — with a floor asserted on
each: "no violations found" from a detector that examined nothing has been green
here before, and a count that names its own coverage cannot be.

The allowlist, each entry argued where it was decided —
`crates/common/protocol/tests/bare_integer_fields.rs` scans the
crate and fails if a bare integer field appears anywhere in `src/` that is not
one of these rows, or if a row here no longer matches anything in `src/` (see
[N8's amendments](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n8-the-sweep) for how the check
itself works and why a text scan rather than a syntax tree). The test's own
`ALLOWLIST` constant is the enforced copy; this table is the narrative for it
and the two are kept in step by hand.

| field | why it stays a bare integer |
|---|---|
| `world::Point::{x, y, z}` | components of one geometric quantity — [N1 amendment 2](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n1-the-rest-of-worldrs) |
| `world::MapSize::{width, height}` | same |
| `target::MultiOffset::{x, y, z}` | components of one signed displacement; the enclosing type keeps it distinct from an absolute `Point` and keeps the three wire fields together |
| `gump::GumpPoint::{x, y}` | the same argument in gump-space pixels, signed for the layout language's negative offsets — [N8 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n8-the-sweep) |
| `mobile::Vitals::{current, max}` | components of one bar — [N2 amendment 2](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n2-mobilers) |
| `mobile::MobileStatus::{strength, dexterity, intelligence, gold, armor, weight, max_weight, stat_cap, followers, followers_max}` | the status bar's quantities — [N2 amendment 3](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n2-mobilers) |
| `vendor::BuyLine::price`, `vendor::SellLine::price` | gold: the `MobileStatus::gold` argument — [N5 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n5-vendorrs) |
| `login::ShardEntry::timezone` | a quantity, by the `MobileStatus` argument — [N6 amendment 8](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n6-loginrs-seedrs-versionrs) |
| `version::ClientVersion::{major, minor, revision, patch}` | components of one version, and not a packet struct — [N6 amendment 7](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n6-loginrs-seedrs-versionrs) |
| `feedback::Animation::{action, repeat_count, delay}` | a body-specific animation index whose domain (`openshard_state::Action`) lives above `protocol`, plus quantities — [N7 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::NewAnimation::{animation_type, action, delay}` | same, the `0xE2` numbering — [N7 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::GraphicalEffect::{speed, duration}` | quantities, a per-effect literal at every call site — [N7 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::HarvestPreview::{action, cycles}` | a body-specific animation index whose domain (`openshard_state::Action`) lives above `protocol`, plus a presentation-only count — [N7 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::HuedEffect::render_mode` | no non-test code constructs one, so there is no caller to classify against — [N7 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `world::WeatherChange::{intensity, temperature}` | classic-client presentation bytes; weather rules live above `protocol` — [N7 amendment 1](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `skill::SkillEntry::{id, value, base, cap}` | `openshard_state::Skill` lives above `protocol`, plus quantities — the `feedback.rs` argument again — [N7 amendment 11](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `spellbook::SpellbookContent::offset` | nothing branches on the byte while no second spell school is wired up — [N7 amendment 8](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `spellbook::SpellbookContent::content` | a membership bitmask over spell ids; which ids exist is Community Pack content, the `feedback.rs` argument once more — [N8 amendment 2](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n8-the-sweep) |
| `properties::TooltipRevision::hash` | server-computed, client-only reader; none of N3's four classes fit — a fifth shape, documented rather than forced — [N7 amendment 6](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `gump::GumpResponse::text_entries` | a `Vec<(u16, String)>`; which text-field id a pack drew is the pack script's business, above the engine — [N5 amendment 10](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n5-gumprs) |
| `error::WrongPacket::{expected, found}` | diagnostic fields on a typed error (the id the dispatcher wanted, and the packet's own header id) — not client-supplied wire data — [N8 amendment 3](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n8-the-sweep) |
| `gump::InvalidSwitchId::id` | the rejected value, carried on the error for its `Display` impl — [N8 amendment 3](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n8-the-sweep) |
| `context::InvalidContextMenuIndex::tag` | same | 
| `wire::InvalidCharacterSlot::slot` | same |
| `design::DesignTile::{dx, dy, dz}` | a signed tile displacement from a house's origin — `target::MultiOffset`'s geometry, at `i8` because the wire's stair buffer gives each offset one byte |
| `design::DesignBounds::{x_min, y_min}` | the corner the grid planes are indexed from, in that same displacement space: subtracted from one and added back to the other |
| `craft::{CraftCatalogueComponent, CraftWorkbenchComponent}::amount`, workbench/component `carried`, `CraftCatalogue::amounts`, `CraftWorkbench::tool_uses` | displayed or indexed item/use quantities; their gameplay caps live above `protocol` |
| craft catalogue/workbench `button`, `make_button`, `details_button`, `materials_button`, `refresh_button`, `cancel_button` | private presentation-seam gump reply ids, matching the existing `CraftCatalogueRow::button` exception |
| `craft::CraftSkillRequirement::{skill, minimum}`, catalogue/workbench `skills` | state-owned skill ids plus displayed thresholds in tenths of a percent |
| craft `needs`, `facilities`, `required_facilities`, `present_facilities` | generated facility-presence bitmasks whose domain lives in `crafting` above `protocol` |
| `craft::CraftCatalogue::{request_id, catalogue_revision, craft_projection_revision, backpack_revision}` | connection correlation, generated content hash, and stock generations compared opaquely rather than interpreted as gameplay identity |
| `craft::CraftWorkbenchPage::Details::{success_per_mille, exceptional_per_mille}` | displayed probability quantities in their named per-mille unit |
| `house_inventory::HouseInventoryRow::{aggregate_total, root_total, pile_count}` | permission-filtered item quantities and a diagnostic pile count |
| house-inventory request/reply `epoch`, `expected_epoch`, `current_epoch` fields | one server projection generation compared opaquely for pagination/result continuity; result use still canonically revalidates |
| `house_inventory::HouseInventoryRequest::Search::limit` | page-size quantity validated against `MAX_HOUSE_INVENTORY_PAGE` at decode |
| `item_kind::MaterialRule::SameAsInput.0` | build-validated index into one recipe's bounded resource lines |

`containers::ContainedItem::{x, y}` came *off* this list in N5: they are one
`GumpPoint` now, as [N4 amendment 6](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n4-containersrs)
promised — [N5 amendment 6](evidence/2026-08-31-the-newtype-sweep.md#amendments-forced-by-n5-gumprs).

**N11. No compatibility shims.** Same as D9: a stage wraps a group of fields
**and** updates every call site in the same commit.

