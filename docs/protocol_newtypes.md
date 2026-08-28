# Protocol newtypes: from bare integers to raw-then-validated fields

Living plan for a multi-session sweep of `crates/common/protocol`. It is the
sequel to [`protocol_rewrite.md`](protocol_rewrite.md), which turned 47 free
`encode_*` functions and a hand-written `match` into two root enums, and left
[D6](protocol_rewrite.md#decisions) — "newtypes on the wire" — deliberately
half-done: a newtype arrived only with the packet that first needed it, so
`Serial`, `Graphic`, `Hue`, `SoundId`, `CursorId` and `AuthKey` exist and
everything else is still a bare integer.

As with its predecessor: when reality contradicts a decision here, change this
file in the same commit that changes the code.

## Why

193 `pub <name>: u8|u16|u32|i8|…` fields remain in the crate's packet structs:

| module | bare int fields | module | bare int fields |
|---|---|---|---|
| `world.rs` | 39 | `login.rs` | 8 |
| `mobile.rs` | 37 | `skill.rs` | 6 |
| `speech.rs` | 22 | `context.rs` | 6 |
| `items.rs` | 16 | `version.rs` | 4 |
| `vendor.rs` | 14 | `spellbook.rs` | 3 |
| `feedback.rs` | 9 | `properties.rs`, `encoded.rs`, `combat.rs` | 2 each |
| `gump.rs` | 9 | `casting.rs`, `seed.rs` | 1 each |
| `containers.rs` | 9 | | |

Two separate problems hide in that number, and this sweep is about both.

**A bare integer does not say what it is.** A hue and a graphic are both `u16`;
a skill id and a stat value are both `u8`. Nothing but a reader's attention
stops `Hue(create.hair)` from compiling. This is what D6 already argued.

**A bare integer off the wire does not say whether anyone checked it.** This is
the sharper half, and the reason the sweep is worth a plan rather than a
`sed` run. `dispatch::create_character` today does:

```rust
strength: u16::from(create.strength),
…
.map(|choice| (choice.skill, u16::from(choice.value) * 10, SkillLock::Up, 0))
…
appearance: Some(Appearance { body: Graphic(create.body()), hue: Hue(create.skin_hue) }),
```

Every one of those values came straight off the wire and none of them was
checked. A client that sends `strength = 255, dexterity = 255, intelligence =
255` gets it. A client that sends `skill value = 255` gets a skill at 2550. A
client that sends any `u16` gets it as a skin hue, staff-only hues included.
CLAUDE.md's rule — *"a packet is not an invariant, it is a hostile input"* — is
stated and then not enforced, because the type system was never asked to carry
the distinction. Bare `u8` reads exactly the same whether it was validated or
not, so the absence is invisible at the call site and stays invisible in review.

So: every client-supplied field gets a `Raw*` type that can only become
something meaningful by passing through a named check. The check is the thing
being added; the newtype is what makes its absence a compile error.

## Decisions

Settled. Do not re-open mid-sweep.

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
  ([Stage 6 amendment 1](protocol_rewrite.md#amendments-forced-by-the-stage-6-pilot-clientpacket-dispatchrs)),
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
the bare-int-field count in the files it touched, before and after. The final
stage adds a repo-level check that counts them across the crate and asserts the
number is zero — or that every remaining one is on an explicit allowlist with a
reason. "No violations found" from a detector that examined nothing has been
green here before; a count cannot be.

The allowlist so far, each entry argued where it was decided. It is exhaustive
as of N8 — `crates/common/protocol/tests/bare_integer_fields.rs` scans the
crate and fails if a bare integer field appears anywhere in `src/` that is not
one of these rows, or if a row here no longer matches anything in `src/` (see
[N8's amendments](#amendments-forced-by-n8-the-sweep) for how the check
itself works and why a text scan rather than a syntax tree). The test's own
`ALLOWLIST` constant is the enforced copy; this table is the narrative for it
and the two are kept in step by hand.

| field | why it stays a bare integer |
|---|---|
| `world::Point::{x, y, z}` | components of one geometric quantity — [N1 amendment 2](#amendments-forced-by-n1-the-rest-of-worldrs) |
| `world::MapSize::{width, height}` | same |
| `target::MultiOffset::{x, y, z}` | components of one signed displacement; the enclosing type keeps it distinct from an absolute `Point` and keeps the three wire fields together |
| `gump::GumpPoint::{x, y}` | the same argument in gump-space pixels, signed for the layout language's negative offsets — [N8 amendment 1](#amendments-forced-by-n8-the-sweep) |
| `mobile::Vitals::{current, max}` | components of one bar — [N2 amendment 2](#amendments-forced-by-n2-mobilers) |
| `mobile::MobileStatus::{strength, dexterity, intelligence, gold, armor, weight, max_weight, stat_cap, followers, followers_max}` | the status bar's quantities — [N2 amendment 3](#amendments-forced-by-n2-mobilers) |
| `vendor::BuyLine::price`, `vendor::SellLine::price` | gold: the `MobileStatus::gold` argument — [N5 amendment 1](#amendments-forced-by-n5-vendorrs) |
| `login::ShardEntry::{percent_full, timezone}` | quantities, by the `MobileStatus` argument — [N6 amendment 8](#amendments-forced-by-n6-loginrs-seedrs-versionrs) |
| `version::ClientVersion::{major, minor, revision, patch}` | components of one version, and not a packet struct — [N6 amendment 7](#amendments-forced-by-n6-loginrs-seedrs-versionrs) |
| `feedback::Animation::{action, repeat_count, delay}` | a body-specific animation index whose domain (`openshard_state::Action`) lives above `protocol`, plus quantities — [N7 amendment 1](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::NewAnimation::{animation_type, action, delay}` | same, the `0xE2` numbering — [N7 amendment 1](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::GraphicalEffect::{speed, duration}` | quantities, a per-effect literal at every call site — [N7 amendment 1](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::HarvestPreview::{action, cycles}` | a body-specific animation index whose domain (`openshard_state::Action`) lives above `protocol`, plus a presentation-only count — [N7 amendment 1](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `feedback::HuedEffect::render_mode` | no non-test code constructs one, so there is no caller to classify against — [N7 amendment 1](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `world::WeatherChange::{intensity, temperature}` | classic-client presentation bytes; weather rules live above `protocol` — [N7 amendment 1](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `skill::SkillEntry::{id, value, base, cap}` | `openshard_state::Skill` lives above `protocol`, plus quantities — the `feedback.rs` argument again — [N7 amendment 11](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `spellbook::SpellbookContent::offset` | nothing branches on the byte while no second spell school is wired up — [N7 amendment 8](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `spellbook::SpellbookContent::content` | a membership bitmask over spell ids; which ids exist is Community Pack content, the `feedback.rs` argument once more — [N8 amendment 2](#amendments-forced-by-n8-the-sweep) |
| `properties::TooltipRevision::hash` | server-computed, client-only reader; none of N3's four classes fit — a fifth shape, documented rather than forced — [N7 amendment 6](#amendments-forced-by-n7-feedbackrs-skillrs-combatrs-propertiesrs-spellbookrs-encodedrs-castingrs) |
| `gump::GumpResponse::text_entries` | a `Vec<(u16, String)>`; which text-field id a pack drew is the pack script's business, above the engine — [N5 amendment 10](#amendments-forced-by-n5-gumprs) |
| `error::WrongPacket::{expected, found}` | diagnostic fields on a typed error (the id the dispatcher wanted, and the packet's own header id) — not client-supplied wire data — [N8 amendment 3](#amendments-forced-by-n8-the-sweep) |
| `gump::InvalidSwitchId::id` | the rejected value, carried on the error for its `Display` impl — [N8 amendment 3](#amendments-forced-by-n8-the-sweep) |
| `context::InvalidContextMenuIndex::tag` | same | 
| `wire::InvalidCharacterSlot::slot` | same |
| `design::DesignTile::{dx, dy, dz}` | a signed tile displacement from a house's origin — `target::MultiOffset`'s geometry, at `i8` because the wire's stair buffer gives each offset one byte |
| `design::DesignBounds::{x_min, y_min}` | the corner the grid planes are indexed from, in that same displacement space: subtracted from one and added back to the other |

`containers::ContainedItem::{x, y}` came *off* this list in N5: they are one
`GumpPoint` now, as [N4 amendment 6](#amendments-forced-by-n4-containersrs)
promised — [N5 amendment 6](#amendments-forced-by-n5-gumprs).

**N11. No compatibility shims.** Same as D9: a stage wraps a group of fields
**and** updates every call site in the same commit.

## The pilot: `0x00`/`0xF8` create character, and `0x5D` character play

`CreateCharacter` is the right first packet: it is entirely client → server, it
has one field of every class, it already carries a `Raw` type
(`RawCharacterName`) so the pattern has a foothold, and its seam
(`dispatch::create_character`) is where three real unchecked values currently
enter the world. `CharacterPlay` joins it — two fields, and it proves a `Raw`
type is shared across packets rather than invented per struct (N4).

Field by field, as decided. `world.rs` unless noted.

| field | wire | class | type | promotion, and where |
|---|---|---|---|---|
| `name` | 30 bytes | C | `RawCharacterName` *(exists)* | `Accounts::create_character` *(exists)* |
| `flags` | `u32` | D | `ClientFlags` | none — never read |
| `profession` | `u8` | B | `RawProfession` | `interpret() -> Profession { Custom, Predefined(u8) }`, in `protocol`. **Do not invent the `prof.txt` table** — the only distinction the wire fixes is "0 means the advanced/custom option"; naming the professions is Community Pack content |
| `sex_race` | `u8` | B | `RawSexRace` | `interpret() -> (Sex, Race)`, in `protocol`. Replaces `is_female()`/`race()`; `body()` takes the interpreted pair. Keep the existing doc note that the SA encoding is assumed |
| `strength`, `dexterity`, `intelligence` | `u8`×3 | C | `RawStatValue` | `CreateCharacter::validate_stats(caps) -> Result<StartingStats, InvalidStartingStats>` — **one rule across three bytes** (per-stat floor/ceiling and the total). Lives in the server crate that reads `[gameplay]`, not in `protocol`. **This check does not exist today.** |
| `skills[].skill` | `u8` | C | `RawSkillId` | `openshard_state::Skill::from_id` *(exists, returns `Option`)*, at the seam |
| `skills[].value` | `u8` | C | `RawSkillValue` | validated against the shard's starting cap at the seam. **Does not exist today** — `value * 10` is applied to whatever arrived |
| `skin_hue`, `hair_hue`, `beard_hue`, `shirt_hue`, `pants_hue` | `u16`×5 | C | `RawHue` → `wire.rs` | `validate(&allowed) -> Result<Hue, InvalidHue>`. The allowed set is content, so it lives above `protocol`. **Does not exist today** |
| `hair`, `beard` | `u16`×2 | C | `RawGraphic` → `wire.rs` | `validate(&allowed) -> Result<Graphic, InvalidGraphic>`; allowed hairstyles are content |
| `start_location` | `u8` | C | `RawStartLocationIndex` | validated against `login.starts.len()`. Behaviour is unchanged — out of range still falls back to the default facet — but the fallback becomes a named branch on a `Result` instead of a `None` from `.get()` |
| `slot` | `u32` | D→C | `RawCharacterSlot` → `wire.rs` | none in the pilot: `create_character` fills the first free slot and ignores the client's pick. Document that as class D with a note; it becomes class C if slot choice is ever honoured |
| `CharacterPlay::name` | 30 bytes | C | `RawCharacterName` | existing lookup path |
| `CharacterPlay::slot` | `u32` | C | `RawCharacterSlot` | validated against the account's character count at the seam |
| `CharacterPlay::client_ip`, `CreateCharacter`'s claimed ip | `u32` | D | `RawClientIp` → `wire.rs` | none — never trusted, never read |

The pilot is done by hand, not by an agent, and it ends by writing an
"Amendments forced by the pilot" section below. Three of its rows are checks
the server is missing today; each one lands with a test that a hostile value
reaches the seam and is refused there.

## Amendments forced by the pilot

The pilot landed classes A, B and D in full — every row of the field table
above now has its named type — and class C's *type* half only: every
class-C field is a `Raw*` newtype wired through decode, encode and the seam,
but none of the three promotion methods the field table calls "does not exist
today" (`validate_stats`, the skill-value check, the hue/graphic allowlist)
were written. Each one needs a real gameplay-balance number — a starting stat
total and per-stat floor/ceiling, a starting skill-point budget, a set of
hairstyles/hues this shard actually allows — and none of those numbers exist
anywhere in this repo yet. Inventing them here would be a content decision, not
a mechanical refactor, so they are left as the concrete next step rather than
guessed at.

1. **Every class-C field's `.0` is unwrapped at the seam
   (`dispatch::create_character`) with a comment naming it as an unchecked
   pass-through.** This is deliberately worse-looking than the old bare `u16`
   it replaces — the point is that it is now *visible* and grep-able
   (`Raw` with no matching `validate`/`interpret` call at its one call site),
   where before the same gap was invisible. N9's test pair (decodes cleanly /
   refused at promotion) has nothing to attach to until a promotion method
   exists, so none were added this stage; the next stage that adds
   `validate_stats`, the skill check, or the hue/graphic check owes N9's pair
   for that field specifically.
2. **`CharacterPlay::name` moved to `RawCharacterName` even though it is not
   one of the three "missing check" rows.** `dispatch_world_packet` was
   building a `CharacterName` straight from a bare `String` with no type
   marking it as unchecked client input — the exact invisibility N3 exists to
   remove — so it got the same treatment as `CreateCharacter::name`, at no
   cost: the promotion is unchanged, `roster.get(&account, &name)` is still
   the check (a name nobody has is not an account's character, and the seam
   already handles that by falling back to a fresh spawn).
3. **`RawSexRace::interpret` and `CreateCharacter::body` are both `pub const
   fn`, matching the methods they replace (`is_female`, `race`, `body`).** No
   behaviour changed; `body` moved from a method reading `self.sex_race` twice
   (once through each of `is_female`/`race`) to an associated function taking
   the already-interpreted `(Sex, Race)` pair once, exactly as the field
   table's pilot row specifies.
4. **`ClientFlags`, `RawStatValue`, `RawSkillId`, `RawSkillValue` and
   `RawStartLocationIndex` all live in `world.rs`, not `wire.rs`** — each is
   used by exactly one packet, so N4's "one module" branch applies. Only
   `RawHue`, `RawGraphic`, `RawCharacterSlot` and `RawClientIp` went to
   `wire.rs`, matching the field table's own `→ wire.rs` column exactly.
5. **Bare-integer field count in `world.rs`: 39 before, 20 after** (N10) — the
   19 fields the pilot's two packets own (15 on `CreateCharacter`, 2 on
   `SkillChoice`, 2 on `CharacterPlay`) all gained a named type. The remaining
   20 belong to packets N1 (the rest of `world.rs`) has not touched yet.

## Amendments forced by N1 (the rest of `world.rs`)

N1 is entirely class A, B and D: the module's remaining packets are the outbound
entry sequence plus `0x02`, and the one inbound packet's two fields are an echo
and a value nobody reads. No class C field appeared, so N9's test pair is not
owed by this stage — what it added instead is a test per class-B promotion that
the promotion is *total*.

1. **Class B can be degenerate, and the walk sequence is.** `RawStepSequence::
   interpret` returns a structurally identical `StepSequence`; every one of the
   256 bytes maps to itself. That is not ceremony to be optimised away: the
   sequence is an **echo tag** — the client owns the number, the server sends it
   back so an ack can be matched to the step that asked for it — so the type
   pair records provenance, which is the only thing that differs between the two
   ends. There *is* a rule (a fresh connection must open at zero, a wrap skips
   it), it lives in `openshard_movement::WalkSequence::accept`, and it refuses
   the **step**, not the value: a `0x21` names the very sequence it is
   rejecting, so the reject echoes a byte the rule declined. N5's gump and
   button ids are the mirror image — server-chosen, echoed *by the client* — and
   are class C ("is this one I offered"), not this.
2. **N10 gains an allowlist, and its first entries are geometric.** `Point`'s
   `x`/`y`/`z` and `MapSize`'s `width`/`height` stay bare integers *by
   decision*: the struct is the named type, nothing reaches a component except
   through it, and the components are the one thing that is genuinely a number —
   they get added to, compared and clamped, in movement, sectors, pathfinding
   and line of sight. Wrapping them buys no confusion that the enclosing type
   does not already prevent, and costs a `.0` on every arithmetic site in the
   server. Reason recorded here so N8's counter can assert five, not zero, for
   this file.
3. **`map_width`/`map_height` became one `MapSize`.** Both call sites read the
   two together and both packets carry both halves; a client told a width
   without its height draws the edge of the world in the wrong place. The old
   `DEFAULT_MAP_WIDTH`/`DEFAULT_MAP_HEIGHT` pair became `MapSize::BRITANNIA`,
   which is what they always meant.
4. **A packet was renamed to free a name for its value: `Season` →
   `SeasonChange`.** The five seasons are a real domain — the client draws
   exactly five and nothing else — so `Season` had to become the enum, and the
   packet took ServUO's own name for it, which its doc comment already cited.
   The `ServerPacket` variant moved with it. `Season::from_bits` is total, with
   the same "fall back to what the client can always draw" argument as
   `Notoriety::from_bits`; `openshard_config` still refuses a sixth season at
   startup, so the fallback is for scripts and foreign saves, not for config.
5. **The stage reached into `mobile.rs` for one shared type: `StatusFlags`.** It
   was `pub type StatusFlags = u8`, an alias — which passes a bare-integer count
   while being exactly the invisibility N3 exists to remove — and `PlayerUpdate`
   needed it. It is now a newtype with a `NONE` constant, and it stayed in
   `mobile.rs` rather than moving to `wire.rs`: N4's `wire.rs` rule is written
   for `Raw*` types, and a *validated* type lives where its rule lives, which for
   a mobile's status bits is with the mobile packets. Same argument kept
   `Notoriety` where it is while `WalkAck` began using it.
6. **`WalkAck::notoriety` now goes out through `Notoriety::for_client`,** as
   `0x77` and `0x78` already did. Bytes are unchanged for every value this shard
   currently sends (`Innocent`, `0x01`); what changed is that a yellow bar can no
   longer reach a pre-4.0.0 client, which would have drawn the player's own
   health bar as nothing at all. `NOTORIETY_INNOCENT`, the loose `u8` constant in
   `tick/defaults.rs`, is gone.
7. **One real bug fell out of `Serial`.** `WorldState::teleport` built its
   `0x20` with `serial_of(entity).map_or(0, |s| s.raw())` — zero is not a
   serial, it is the wire's word for "no object", and `Serial::new` refuses it.
   The serial now joins the body and the facing in the `if let`, so a client
   whose entity has no serial gets no packet instead of a nonsense one.
8. **Bare-integer field count in `world.rs`: 20 before, 5 after** (N10), the
   five being the allowlisted geometric components in amendment 2. `mobile.rs`
   is unchanged at 37 — the `StatusFlags` alias was not one of them.

## Amendments forced by N2 (`mobile.rs`)

The direction rule paid for itself exactly as N2's stage line predicted: seven of
the module's serials, both body graphics and three hues are class A, and wrapping
them **deleted** code — ten `.raw()` calls and four `.0`s vanished from the
server, because the call sites already held a `Serial`, a `Graphic` and a `Hue`
and were unwrapping them to satisfy a `u32`. The stage's real content is the two
inbound packets and one question the class table does not answer.

1. **`RawSerial` is the pattern for every inbound object reference, and it
   returns `Option`, not `Result`.** `LookRequest` is the sweep's first client-
   chosen serial and there will be one in `items.rs`, `target.rs`, `vendor.rs`
   and `context.rs`, so the type went into `serial.rs` beside the rule rather
   than into the packet that first needed it. Its promotion is
   `validate(self) -> Option<Serial>`, wrapping the existing `Serial::new`: N7
   asks for typed errors, but the two values a client actually sends here — `0`
   and `0xFFFF_FFFF` — are the wire's own words for *no object*, which is an
   answer and not a malformed packet. An `InvalidSerial` would make every seam
   handle an error where all but one of them want to do nothing. This is the same
   licence the pilot took with `Skill::from_id`'s `Option`, written down.
2. **A current/max pair is one field: `Vitals`.** `MobileStatus` carried
   `hits`/`hits_max`, `stamina`/`stamina_max`, `mana`/`mana_max` — six bare
   `u16`s of which every source produces both halves at once (`Hitpoints`,
   `Mana`, `Stamina` each hold the pair) and the client draws a *ratio*, so half a
   pair is not a smaller number but a bar of the wrong length. The `MapSize`
   argument of N1 amendment 3, applied three times. `weight`/`max_weight` looks
   like a fourth pair and is not: the two come from different places (what is
   carried, versus a function of strength), so they stay separate fields.
3. **The status bar's ten remaining numbers stay bare, by decision.** `strength`,
   `dexterity`, `intelligence`, `gold`, `armor`, `weight`, `max_weight`,
   `stat_cap`, `followers`, `followers_max` are the case N1 amendment 2 opened
   for `Point`: they are genuinely numbers — added to, compared and clamped on
   every blow, every regeneration tick and every item picked up — and their rules
   (the caps, the carry limit, the training curve) live in `skills`, `items` and
   `[gameplay]` config, all far above `protocol`. Ten newtypes here would be ten
   types that only ever unwrap, and the packet's named fields already prevent the
   confusion a newtype would. They are on N10's allowlist, so the count for this
   file asserts twelve and not zero.
4. **Class C appeared where the plan expected only A, B and D**, and both of its
   fields are on the same packet: `0xBF 0x1A`'s `stat` and `lock`. `RawStat::
   validate -> Result<Stat, InvalidStat>` is a real refusal — the status bar has
   exactly three arrows — and it moved a `_ => return` out of
   `World::set_stat_lock` and into `dispatch.rs`, which now logs the byte it
   dropped. N9's pair is there: a `0xBF 0x1A` naming stat 3 decodes cleanly and
   is refused at promotion.
5. **A decoder that rewrote a value was the stage's one real finding.**
   `StatLockRequest::decode_body` folded `lock > 2` to `0` *while decoding* —
   ServUO's behaviour, in the wrong place: after it, nothing downstream could
   tell the `0` a client sent from the `0x63` it did not, so a log line about a
   nonsense arrow was impossible to write. The fold is now
   `RawStatLock::interpret`, class B and total, with a test that all 256 bytes
   interpret and that the byte survives decoding unchanged.
6. **Three-valued arrows are one type, bridged by name.** `StatLockBits`'s three
   `u8`s became `skill::SkillLock` — its own doc already called it "the mirror of
   `SkillLock`", so a second three-way enum was never needed. `openshard_state`
   keeps its separate `StatLock` (its gain path is not the skill one) and gained
   `to_wire`/`from_wire`, both directions named, no `From`. `StatLock::from_bits`
   stays, now documented as the *saved* byte's reader — a save written by an
   older build may hold anything, which is a different problem from a packet.
7. **`Layer` went to `wire.rs` although only one module uses it today.** N4's
   "two or more modules" rule is written for `Raw*` types; a validated type lives
   where its rule lives, and a layer's rule is the client's alone. Both packet
   modules that carry one — this module's `0x78` outfit list and `items.rs`'s
   `0x2E`/`0x13` — would otherwise have to import it from each other. It is a
   named byte and not an enum, for `StatusFlags`' reason: the twenty-odd layers
   this engine has never sent would be a guess. `openshard_state::Equipped.layer`
   stays a `u8` — that is a component, not a packet field, and N4's stage is
   where it becomes one question rather than two.
8. **`PaperdollFlags` replaced two loose `pub const u8`s** (`PAPERDOLL_WARMODE`,
   `PAPERDOLL_CAN_LIFT`), with a named `with` rather than a `BitOr` impl: an
   operator on a newtype is the same invisible coercion `Deref` is.
9. **One byte-level test changed its input, deliberately (N8).**
   `remove_is_five_bytes` built its `0x1D` with serial `0xDEAD_BEEF`, which is
   past the item pool and refused by `Serial::new` — an unaddressable serial the
   old bare `u32` let through. It now uses `0x4EAD_BEEF` and asserts the same
   shape: five bytes, the serial big-endian. Every other assertion in the crate
   is byte-for-byte what it was.
10. **Bare-integer field count in `mobile.rs`: 37 before, 12 after** (N10), the
    twelve being amendments 2 and 3's allowlisted quantities. `wire.rs` and
    `serial.rs` gained a type each and no bare fields.

## Amendments forced by N3 (`speech.rs`)

The module the plan called five packets is really one *header* sent five times —
mode, hue, font, and (outbound) a speaker — so the stage is where N1's direction
rule met the same field going both ways. It produced the sweep's first genuinely
shared class-B type and its second decoder-rewrites-a-value finding.

1. **`TalkMode` is an enum with a leftover arm, where `Layer` and `StatusFlags`
   are named bytes.** N2 amendment 7 argued a byte with a name beats an enum
   when the unnamed values would be a guess, and the modes look like that case —
   ServUO's `MessageType` has a dozen this engine has never sent. The difference
   is that something already *branches* on this byte: `speech_range` has always
   asked "whisper, yell, or neither", and the answer decides who hears it. A
   named byte cannot be matched exhaustively, so the branch stays a `_ =>` with
   no compiler behind it. Five variants are named — the ones this repo's own doc
   comments already name — and `Other(u8)` carries the rest, which is exactly
   what N3's class-B row prescribes. Nothing was guessed: `Other` is the record
   that the meaning is unknown, not a claim it has none.
2. **Three modules had each named the same domain, and none of them knew.**
   `mobile::LABEL_MODE`, `chat::TALKMODE_WHISPER` and `chat::TALKMODE_YELL` were
   loose `pub const u8`s in two crates, and `chat::DEFAULT_FONT` a third — the
   `PaperdollFlags` situation of N2 amendment 8, spread across a crate boundary.
   They are `TalkMode::Label`/`Whisper`/`Yell` and `Font::DEFAULT` now. A domain
   named in three places is a domain with no type; that a bare `u8` *travels*
   between crates is what let it happen.
3. **`0xAD`'s decoder rewrote the mode byte, and this is the second time.**
   `decode_body` stored `mode & !0xC0` — the keyword bits gone before anything
   downstream could see them, so the `0x00` a client sent and the `0xC0` it did
   not were indistinguishable, exactly `StatLockRequest`'s finding (N2 amendment
   5). The distinction here is sharper than that one, because the bits *are*
   framing: the decoder legitimately reads them to know which of two text shapes
   follows. So the read stayed (`RawTalkMode::has_keywords`, private, framing
   only) and the *fold* moved to `RawTalkMode::interpret`. Two findings of the
   same shape in two stages says this is a pattern to look for and not an
   accident: **wherever a decoder normalises, the raw byte is being destroyed.**
4. **A packet can have its own sentinel, and speech's is not the wire's usual
   one.** `serial`/`graphic` became `Option<Serial>`/`Option<Graphic>`, but the
   absent case encodes as `0xFFFF_FFFF`/`0xFFFF` — ServUO's `Serial.MinusOne` —
   and **not** as `serial::raw_or_none`'s `0`. Reusing the shared helper would
   have compiled, changed the bytes, and told the client the words came from an
   object it does not have; it would draw them nowhere. So `speech.rs` keeps a
   private `serial_or_system`/`graphic_or_none` pair beside its own constants.
   The lesson for later stages: `Option<Serial>` names the *shape* of a field,
   never the value it goes out as — check the packet's own sentinel every time.
5. **The same `map_or(0, …)` bug as N1 amendment 7, fixed the other way.** Both
   `private_overhead_cliloc` and `private_overhead_text` built their serial with
   `serial_of(source).map_or(0, |s| s.raw())`, and zero is not a serial. `0x20`'s
   fix was to send no packet; these send the line as a *system* message instead,
   because the text is feedback the watcher asked for (Item Identification saying
   what an item turned out to be) and a line drawn in the corner beats no line at
   all. Which way a nonsense serial degrades is the packet's question, not a rule
   the sweep can settle once.
6. **`ClilocId` went to `wire.rs` and `Font` stayed in `speech.rs`,** both by
   N4's counting rule as N2 amendment 7 read it for validated types: five modules
   carry a cliloc (`speech`, `context`, `properties`, `gump`, `login`) and one
   carries a font. Only `speech.rs`'s cliloc field was converted — the other four
   are their own stages' work, exactly as `Layer` landed in `wire.rs` for
   `mobile.rs` and left `items.rs` for N4.
7. **`WorldState::localized_message` keeps a bare `u32`, citing `play_sound`.**
   Carrying `ClilocId` up through it would have touched ~190 call sites across
   `skills`, `crafting`, `items` and `world`, every one a ported ServUO message
   *number* out of a table. `play_sound` already made and documented this
   decision for `SoundId` (`runtime.rs`), and the reasoning transfers whole: the
   newtype starts where the packet is built, nothing above unwraps one, and
   converting the tables is its own sweep. Recorded rather than done, so the next
   reader finds a decision and not an oversight.
8. **Class C appeared, and its promotions are the pilot's deferral again.** A
   client's `hue` and `font` are checked against sets that are content — which
   hues this shard allows, which faces the client has — and neither exists in the
   repo. They arrive as `RawHue`/`RawFont` and are unwrapped at
   `World::say` with the comment naming them unchecked, exactly as pilot
   amendment 1 established. N9's test pair is owed by whichever stage writes
   those checks, not by this one. `mode` is class B and *does* promote, so its
   totality test is here.
9. **The raw types reached the world's `Command` enum for the first time.**
   `Command::Say` carries `RawTalkMode`/`RawHue`/`RawFont` from `dispatch.rs`
   through the queue to `World::say`, which is the seam — the command queue is
   not a checkpoint, it is a delivery, and pretending otherwise would have put
   the promotion on whatever thread Tokio picked. `Command::Speak`, which a
   script raises, takes a validated `Hue`: the script bridge is a serialization
   seam like SQL or the wire, and that is where the JSON number becomes a type.
10. **Bare-integer field count in `speech.rs`: 22 before, 0 after** (N10) —
    nothing on the allowlist, the first module in the sweep to reach zero.
    `wire.rs` gained `ClilocId` and `mobile.rs` lost `LABEL_MODE`; neither file's
    count moved.

### Backlog from this stage

- **The cliloc-table sweep** (amendment 7): ~190 call sites pass a bare `u32`
  message id. Worth doing with the `SoundId` table sweep, which has the same
  shape. Both want the number to come out of a content table already typed —
  **decided:** `protocol` takes the dependency. `ClilocId` and `SoundId` are
  `Deserialize`/`Serialize` now (`#[serde(transparent)]`, `wire.rs`), so a
  content loader can read either straight into the newtype instead of every
  call site wrapping a bare number by hand. The sweep across the ~190 call
  sites itself is still open. — **closed by N-gump (below): the remaining bare
  clilocs were not call sites at all but four `u32` *parameters* on
  `GumpLayout`, which is why no field scan ever counted them. Sourcing the
  numbers from a content table stays open, and is a Community Pack question now
  that the type is in place, not a newtype one.**
- ~~**`0x03B2` is written out five times**: `gm::SYSTEM_HUE`, `npc::GREET_HUE`,
  `quests::progress::NPC_HUE`, `runtime::SYSTEM_HUE`,
  `tick::defaults::TEXT_HUE`. They are all "the client's muted grey", all four
  crates deep, and they are now five `Hue` constants rather than five `u16`s —
  which makes the duplication visible but not gone. Where a shard-wide default
  hue *lives* is a `[gameplay]`-config question this stage did not open.~~

  **Fixed: the number lives once, the meanings stay three.** The five sites are
  not one concept. Two are the shard talking (`runtime::SYSTEM_HUE`, a private
  system line; `gm::SYSTEM_HUE`, a staff command's reply) — no serial, no
  graphic, the name field literally `"System"`. Two are a *mobile* talking
  (`npc::GREET_HUE`, `quests::progress::NPC_HUE`) — over the NPC's head, heard by
  everyone in earshot. The fifth is neither: `tick::defaults::TEXT_HUE` colours an
  **item's** single-click name label, the branch a mobile takes
  `Notoriety::name_hue` for instead. Both references write `0x3B2` out separately
  for each of these (ServUO's `AsciiMessage` fallback, its `Item.OnSingleClick`,
  its `Notoriety.Hues[3]`; Sphere's `HUE_TEXT_DEF`), which is the tell: they
  coincide because the client's palette has one grey that reads as "not a person
  talking", not because they are the same rule. Collapsing them to a single
  constant would have made "recolour system messages" silently recolour every
  shopkeeper.

  So `protocol::wire` gained a **private** `Hue::MUTED_GREY` — the only place the
  literal is written — and three public names off it: `Hue::SYSTEM`,
  `Hue::NPC_SPEECH`, `Hue::LABEL`. The five constants stay where they are as the
  local names their crates read best, each now defined as one of the three;
  `protocol` is the home because it already owns `Hue` and already carries this
  kind of client-palette knowledge in `Notoriety::name_hue`. That table keeps its
  own literal on purpose: it is one unit of ported ServUO numbers, and breaking a
  single arm out of it to point at `MUTED_GREY` would make the table harder to
  audit than the duplication costs.

  **Splitting the names immediately caught a bug the coincidence was hiding.**
  `npc::notify` (`crates/server/npc/src/lib.rs`) sends a *private system line* —
  no serial, `name: "System"` — and was drawing it in `GREET_HUE`, the townsfolk
  chatter colour. Identical bytes today, so nothing was visibly wrong; the moment
  a shard recoloured NPC speech the banker's "the bank says" would have followed
  it. It now names `Hue::SYSTEM`. This is the argument for one-value-many-names
  over one-value-one-name, made by the refactor itself.

  The `[gameplay]`-config question stays open and is now cheap: three named
  defaults are three config fields, and the tick would override the constants
  rather than replace a scattered literal.
- **`npc::say` promised "the one door every townsperson's speech goes through" and
  two callers walked past it — closed by routing them through.**
  `guards::execute` and `vendor::vendor_says` built the `openshard_chat::speak`
  call themselves with `crate::GREET_HUE` and `crate::GREET_FONT`. The arguments
  agreed, so nothing was wrong yet; a doc that promises an invariant the code does
  not enforce is the shape that decays, and the `notify` bug above is what that
  decay looks like when it arrives. Both call `say` now, the two calls collapsed to
  one line each, and `say`'s doc names its three callers plus the cases that are
  *supposed* to stay outside it (a private system line uses `notify`; a script's
  `Speak` names its own hue). `quests::progress` still calls `speak` directly with
  its own `NPC_HUE` — a different crate, and per-crate names were the decision.

## Amendments forced by N4 (`containers.rs`)

The stage's two packets are a two-line inbound `0x06` and an outbound item
record sent three ways, and between them they raised the sweep's first
*component* question — how far up a validated type travels once the packet
below it has one — and its first packet whose inbound field is not a value at
all but a flag riding on one.

1. **A `0x06`'s serial is a serial *and* a flag, and the split is the packet's,
   not the tick's.** Bit 31 is the client's paperdoll request — ServUO's
   `UseReq` routes it straight to `OnPaperdollRequest` and never to `Use`, and
   treating both alike is the bug where relogging mounted dismounted you a
   breath later. That knowledge lived in `tick.rs` as `serial & 0x8000_0000`,
   which is a rule in the file [architecture.md](architecture.md) says holds no
   rules. It is now `DoubleClick::interpret -> UseRequest`, class B and total:
   every one of the 2³² values is a paperdoll request or a use, and **both arms
   carry a [`RawSerial`]**, because stripping a flag bit does not make what is
   left address anything. The validation stays where N2 puts it, at the seam.
2. **A packet-level `interpret` may run at the network seam; a `validate` still
   may not.** `dispatch.rs` calls `interpret` and queues a `Command::DoubleClick
   { request: UseRequest }`. This does not contradict N3 amendment 9's "the
   queue is a delivery, not a checkpoint": a total interpretation cannot refuse
   anything, so running it early costs nothing and cannot drop a client's
   request on Tokio's thread. What crossed the queue is still raw, and
   `RawSerial::validate` runs in the tick.
3. **Wrapping deleted three guards.** `items::double_click`,
   `items::paperdoll_request`, `items::mobile_used` and `npc::open_shop` each
   opened with `Serial::new(serial)` and each now takes a `Serial`; the tick's
   arm validates **once** where it used to re-derive the same `Option` five
   times over. N2's amendment 1 result, in the other direction: there the
   outbound types deleted `.raw()` calls, here the inbound one deleted repeated
   checks.
4. **A validated type stops where the packet is built, and the component below
   it keeps its bare integer — for now.** `ContainedItem` gained `Serial`,
   `Graphic`, `Hue` and `GridSlot`, but `openshard_state::Contained.grid`,
   `Container.gump` and `components::Graphic`'s `id`/`hue` are all still bare.
   This is N3 amendment 7's `localized_message` decision applied to components
   rather than to a table: `Contained.grid` alone reaches the persistence
   record, both stores' SQL and a dozen test fixtures, and converting it is a
   sweep with its own shape — the newtype starts where the packet is built and
   nothing above unwraps one. Recorded rather than done. The exception the doc
   already promised, `Equipped.layer`, is N4's `items.rs` half and is decided
   there.
5. **`GridSlot` is a named byte, not an index type.** Same argument as `Layer`
   (N2 amendment 7): the grid's size is the client's, this engine has never had
   a reason to learn it, and a range check would be a guess. What the type buys
   is that the three `u16`s beside it on the record can no longer be handed to
   it.
6. **`x`/`y` on a container record stay bare, and the reason is a stage
   boundary.** They are the item's column and row in the gump — a pair, read
   and written together, which by N1 amendment 3 and N2 amendment 2 asks to
   become one named type. It is not made here: a gump coordinate is exactly
   what `gump.rs` carries, that module is N5's, and a `GumpPoint` invented in
   this stage is a name the next stage would have to either adopt or contradict.
   On N10's allowlist with that reason, and in N5's backlog.
7. **Two magic gump ids became constants.** `0xFFFF` — what makes a `0x24` draw
   a book rather than a bag — is `containers::BOOK_GUMP`, beside the packet
   whose behaviour it changes; `npc`'s `SHOP_GUMP` was already named and is now
   a `Graphic`.
8. **Bare-integer field count in `containers.rs`: 9 before, 3 after** (N10), the
   three being amendment 6's `x`/`y` and the stack `amount`, which is now
   [`items::ItemAmount`](../crates/common/protocol/src/items.rs) across item,
   container, and vendor packets.

## Amendments forced by N4 (`items.rs`)

The module is the sweep's first genuinely two-directional one — the same item
drawn outbound and named inbound — and it is where `Layer`, parked by
[N2 amendment 7](#amendments-forced-by-n2-mobilers), had to become one answer
rather than two.

1. **`Equipped.layer` is a `Layer`, and that is where the component sweep stops
   this stage.** N2 left the question open; the answer is yes, and the reason is
   not symmetry with the packet but that *every* rule reading it is naming a
   slot, never doing arithmetic: what a corpse keeps, what armour covers, what
   may not be lifted, which hand a weapon is in. The type carried outward from
   there through `state::armor`, `state::weapon`, `combat`, `npc` and `world`
   with no `.0` except at the two seams that are supposed to have one — the
   persistence record's `u8` and the script bridge's JSON number. Contrast
   `Contained.grid` and `components::Graphic` in
   [containers amendment 4](#amendments-forced-by-n4-containersrs), which stayed
   bare: those are read as *numbers* nowhere either, but nothing in a packet
   forced the question, and a sweep with no forcing packet is its own stage.
2. **`RawLayer` lives in `wire.rs`, beside its twin, against N4's own counting
   rule.** Only `0x13` carries an inbound layer, so N4 would put it in
   `items.rs`. Every other `Raw*` in the crate sits beside the validated type it
   promotes to — `RawHue` beside `Hue`, `RawSerial` beside `Serial` — and a pair
   split across two modules is a pair the next reader has to be told about. The
   counting rule is for `Raw*` types with **no** twin (`RawStatValue`,
   `RawStartLocationIndex`); where there is one, the twin's home wins.
3. **`RawLayer::interpret` is degenerate, and deliberately so.** The second
   `RawStepSequence` (N1 amendment 1): a layer is a *name*, not a range — N2
   amendment 7 settled that — so every one of the 256 bytes interprets, and what
   the pair records is provenance. The refusal that does exist is a gameplay
   one and stayed where it was: `equip_item` still rejects layer `0` and
   anything past `MAX_WEARABLE_LAYER`, now stated in `Layer`s.
4. **`DROP_TO_GROUND` is a `RawSerial` constant, and `to_ground` compares
   against it rather than asking `validate`.** N3 amendment 4's lesson, met from
   the other side: `RawSerial::validate` answers `None` for `0xFFFFFFFF` *and*
   for `0`, but a `0` container is a confused client and `0xFFFFFFFF` is the
   floor. Folding the two would have compiled and silently turned every
   malformed drop into a ground drop.
5. **`BACKPACK_LAYER` was written out five times in two crates.** `world`'s
   `gm.rs`, `travel.rs`, `spells.rs` and `tick/defaults.rs` each declared their
   own `0x15`, and `npc/vendor.rs` a fifth, while `openshard_items` had the
   canonical one all along. Exactly N3 amendment 2's finding (`TALKMODE_WHISPER`
   in two crates) and N2 amendment 8's (`PAPERDOLL_WARMODE`), for the third
   time: **a bare integer that travels between crates gets re-declared at each
   stop.** The four copies are gone; there is one `Layer`.
6. **The paperdoll layers scattered as loose `pub const u8` are all `Layer`
   now** — `state::armor`'s seven coverage layers, `state::weapon`'s two hands,
   `npc::dress`'s seven garment slots, `items`' backpack/bank/mount/trade, and
   `world`'s corpse robe. `layer_coverage` and `hit_layer` take and return one,
   which is what stopped `hit_layer`'s roll and its layer from being the same
   type.
7. **`Terrain::item_layer` keeps its byte, wrapped at one call site.** The trait
   lives in `openshard_movement`, which is below `protocol`, and it reads the
   quality byte out of `tiledata.mul`. `skills::appraise::tiledata_layer` is the
   single place the byte meets a `Layer`, and it names the wrap. `weapon_layer`
   above it takes and returns `Layer`s.
8. **`WorldItem`'s stack-amount bit still masks a serial it cannot need to.**
   `serial & 0x7FFF_FFFF` in the unstacked branch is now provably a no-op —
   `Serial` cannot be built above the item pool — and it stayed, with a comment
   saying so. Removing it would make the encoder depend on `Serial`'s invariant
   at a distance for no byte saved.
9. **`WorldItem` has a tagged payload.** Its post-graphic word is a stack size
   for ordinary items and a body graphic for the corpse marker, so it is
   `WorldItemPayload`, not an `ItemAmount`.

### Backlog from this stage

- **The component sweep.** `Contained.{x, y, grid}`, `Container.gump` and
  `components::Graphic.{id, hue}` are the bare integers directly under the
  packets this stage typed, and each is one `Layer`-sized job: `grid` and `gump`
  reach the persistence record and both stores' SQL, `Graphic` reaches most of
  the server. Worth doing as its own stage after N8, with the cliloc and
  `SoundId` table sweeps N3 left — they share the blocker, which is that the
  numbers should arrive from config already typed. **Done** — N-tables and
  N-components; the estimate of `Graphic`'s reach was the accurate part.
- **`GumpPoint` for N5.** Three modules now carry an `x`/`y` pair that is a
  *gump* coordinate rather than a world one: `containers::ContainedItem`,
  `gump::GumpDisplay`, and `Command::ShowGump`. N5 owns `gump.rs` and should
  name the type; `containers.rs`'s two fields join it then and come off the
  allowlist.
- **`state::Graphic` and `wire::Graphic` collide by name**, so four files now
  spell one of them out in full (`openshard_protocol::wire::Graphic(id)`) and
  `runtime.rs` imports it `as WireGraphic`. Neither name is wrong — one is the
  component an item is *drawn* by, the other the id on the wire — but three
  spellings of the same conversion across the server is a smell. Renaming the
  component (`Drawn`? `Art`?) is a `state` question, not a protocol one.
  **Done** — `Drawn`, in N-components, and it turned out to be the move that
  made the rest of that stage readable rather than a tidy-up after it.

## Amendments forced by N5 (`vendor.rs`)

The module is four packets in two mirrored pairs, and N1's direction rule sorted
them with nothing left over: the two lists the server draws are class A, the two
replies are `RawSerial`. Its content is the *quantities*, which are the first
fields in the sweep to go on N10's allowlist because of what they are rather
than where their type would live.

1. **A price is a quantity and stays bare; stack amounts use `ItemAmount`.** `BuyLine::
   price`, `SellLine::{amount, price}`, `Purchase::amount` and `Sale::amount`
   are N2 amendment 3's case exactly: multiplied into a total, compared against
   what a purse holds, split off a stack — and their rules (what a vendor
   charges, what half price is, how much is on the shelf) live in
   `openshard_npc` and `openshard_items`, far above `protocol`. Purchase and
   sale amounts are item-stack quantities and share `ItemAmount` with the
   corresponding item packets.
2. **A decoder that reads a byte and drops it is not the N2/N3 finding.**
   `BuyReply::decode_body` branches on `0x02` and keeps nothing. The two earlier
   findings (`StatLockRequest`, `0xAD`) *stored* a folded value, so the client's
   own byte was gone; here the byte is framing — it says whether a list
   follows — and the two answers it separates, "closed" and "bought nothing",
   are the same empty basket to everything downstream. The distinction is
   written in a comment beside it, because the shape looks identical at a
   glance. **What makes a normalising decoder a bug is that something
   downstream can no longer tell two inputs apart**; where nothing downstream
   cares, there is nothing to preserve.
3. **Wrapping deleted four `Serial::new` guards and two `.raw()` calls** —
   N2 amendment 1 and N4 containers amendment 3, in one module and both
   directions at once.
4. **Bare-integer field count in `vendor.rs`: 14 before, 5 after** (N10), the
   five being amendment 1's quantities.

## Amendments forced by N5 (`context.rs`)

1. **The tag is class C and its promotion is a `Result`.** A `0x15` echoes the
   entry's position in the list the `0x14` drew, so the count of entries is the
   whole domain: `RawContextMenuIndex::validate(offered)`. Unlike
   `RawSerial::validate`'s `Option` (N2 amendment 1) there is no wire value here
   that *means* "no entry", so every rejection is a refusal worth logging, and
   the error carries the tag and the count to log. The check itself is not new —
   `entries.get(index)` was doing it — but it was silent, and it could not be
   skipped by accident before only because one call site happened to be careful.
2. **`ContextMenuFlags` is a named byte, not an enum**, for `Layer`'s reason (N2
   amendment 7): ServUO's `CMEFlags` has a dozen bits this engine has never set.
3. **`ClilocId` reached its second module**, as N3 amendment 6 said the four
   remaining carriers would. The cliloc *constants* in `tick/context.rs` are
   typed with it; the ~190-call-site table sweep N3 recorded is still open.
4. **Bare-integer field count in `context.rs`: 6 before, 0 after** (N10).

## Amendments forced by N5 (`gump.rs`)

The stage the plan ordered N5 for. Six windows answer through one packet, and
every number in it is one the server chose — which makes this the module where
"is this one I offered" had to become three different checks rather than one.

1. **`RawGumpId::validate` takes a *list* and answers `Option`.** The list
   because the quest system draws two windows and claims a reply for either; the
   `Option` because the router asks each handler in turn and four of the five
   legitimately answer "not mine". A typed error would be an error nobody could
   act on. This is N2 amendment 1's licence extended from "the wire has a word
   for nothing" to "not-mine is an answer this control flow depends on" — and
   the reply that matches *no* engine dialog is not refused at all: it belongs to
   the script pack and is forwarded.
2. **A button id is class B, which the field table did not predict.**
   `RawButtonId::interpret -> GumpAnswer { Closed, Pressed(ButtonId) }` is
   `DoubleClick::interpret`'s shape (N4 containers amendment 1): one field
   carrying a value *and* an answer. The close box is `0`, and it was being
   compared against by hand in three handlers; `crafting::decode_button`'s own
   `if id == 0 { return None }` guard is gone with them, which is the third time
   in this sweep that wrapping a field deleted a guard.
3. **Two layouts deliberately give a button the close box's id, and that had to
   survive.** ServUO's `Buttons.Close = 0` (the quest window's `X`) and
   `CraftGumpItem`'s Back button both send `0`, so dismissing those windows and
   pressing their own button are the same answer *by construction* — pressing
   Back and closing the craft detail page both return to the list, in ServUO and
   here. The refactor's temptation was to treat `Closed` as "do nothing", which
   would have quietly changed both. They are now `ButtonId::CLOSE_BOX` constants
   with the collision stated, rather than a `0` that reads like a coincidence.
   `ButtonId::UNUSED` is the same value again with a third meaning — what a
   `Page` button writes where a reply button writes its id — and has its own
   name for the same reason.
4. **Whether a button was *offered* stays in each handler's `match`.** There is
   no list to check it against: the craft window's ids are computed
   (`1 + kind + index * 7`), the quest log's are a table plus a row offset, the
   runebook's are five ranges. So the sweep names the encodings instead —
   `quests::gump::{row_button, row_of}` and `travel::book_button`, both
   directions with names, the `to_wire`/`from_wire` shape of N2 amendment 6 —
   and the arithmetic stops being open code at five call sites.
5. **`RawSwitchId::validate` takes a count, because a radio group is its rows
   numbered from zero.** Both groups this engine draws are; the group's length
   is the one thing a handler still has when the reply arrives. The moongate
   list was already checking with `.get`; the resign dialog was not, and its
   `switches.contains(&YES)` would have accepted any id the client invented as
   long as one of them was `1`.
6. **`GumpPoint` closes N4's backlog item, and the wire widths differ.**
   `GumpDisplay`, `Command::ShowGump` and `containers::ContainedItem` all
   carried a loose `x`/`y` pair in *gump* space; they share one type now and
   `ContainedItem`'s two fields come off N10's allowlist. The two are measured
   from different origins (a window from the screen, an icon from the container
   art's corner) and go out four bytes wide and two — neither of which makes
   them different quantities, any more than a `Serial` stops being one where a
   packet writes it short. Signed, because the layout language needs it: the
   quest frame puts an element at `x = -16` and an unsigned type would send
   `4294967280`, which the client answers by dropping the whole layout.
7. **The first field of a `0xB0` is not a serial, and `GumpKey` says so.** The
   engine keys a window on the mobile it drew it for, which is why the field was
   called `serial` — but `0` is legal there and means a standalone dialog, the
   animal-lore window keys on its own dialog id, and ServUO puts `Gump.Serial`
   in it, a per-instance counter that is never an object. So it is `CursorId`'s
   twin: server-chosen, echoed, opaque. This also settles that the two
   `map_or(0, |s| s.raw())` sites here are **not** N1 amendment 7's bug — zero
   is a meaning in this field, not a nonsense serial — which is the answer to a
   pattern this sweep has otherwise found three times.
8. **The inbound key is class D.** `GumpResponse::serial` is echoed and nothing
   reads it: a reply is routed by its gump id, and each handler then matches
   against the context it *remembers* drawing, which is a stronger check than an
   echo can be. `RawGumpKey` therefore has no promotion, and the doc comment
   says why — the class-D record N3 asks for.
9. **The layout builder takes the typed ids, because it is the encoder.**
   `GumpLayout::button`/`radio`/`check` take `ButtonId`/`SwitchId` and unwrap
   inside, so one constant serves both the layout that drew a button and the
   arm that answers it — which is the whole loop N5 exists to close. Its other
   arguments (coordinates, gump art, hues, clilocs) stay bare: they are the
   client's positional format, nothing echoes them, and the cliloc column in
   particular is N3 amendment 7's parked table sweep, not this stage's.
10. **The engine types what the engine reads.** `GumpResponse::text_entries`
    stays `(u16, String)`: no window this engine draws has a text field, so
    every one of them is a *pack* gump, the id is one the pack chose, and "is
    this a field I drew" is a check only the pack can make. Typing it here would
    be a wrapper with no promotion and no reader — a `Raw` type that means
    nothing. This is the rule that decided the whole `Vec` question: `switches`
    got a type because `gates` and `quests` read them, `text_entries` did not.
11. **Raw ids cross the event bus to the pack.** `GumpAnswered` carries
    `RawGumpId`, `RawButtonId` and `Vec<RawSwitchId>`, and
    `openshard_server::scripting` unwraps them into JSON numbers. N3 amendment 9
    put raw types on `Command` going *in*; this is the same argument going out,
    and it is stronger: the engine drew none of these windows, so it is in no
    position to validate ids it never issued. The script bridge is the
    serialization seam, exactly as `Command::Speak` established.
12. **Bare-integer field count in `gump.rs`: 9 before, 0 after** (N10).
    `containers.rs` went 3 to 1 — the stack amount alone — when its `x`/`y`
    became a `GumpPoint`.

### Backlog from this stage

- ~~**The admin menu is the one window still written as a layout string by
  hand**~~ — done. `world/src/admin.rs` now draws through `GumpLayout` from a
  `ROWS` table that carries the id, the art, the label and the verb together;
  the handler looks a reply up in the same table, so a button id is written
  once. A test pins the built layout against the hand-written string byte for
  byte, and a second asserts the one thing the table cannot enforce by
  construction — that no two rows share an id and none is `CLOSE_BOX`.
- **`ButtonId::CLOSE_BOX` and `ButtonId::UNUSED` are the same value with
  different meanings**, and a third would be one too many; if one appears, the
  type wants to be an enum with a `Reply(u32)` arm rather than a newtype with
  named zeroes. *Still two, re-checked:* every use in the workspace is either a
  close box (`crafting`, `quests`, `gates`, `travel`, `admin`, `animal`) or a
  page button's ignored id (`crafting`, `animal`), and the only reader of the
  value is `RawButtonId::interpret`, which splits `0` off once. The condition
  has not fired, so the type is unchanged; what was missing was that only
  `UNUSED` documented the collision, so `CLOSE_BOX` now names the other half and
  the trigger, and a reader landing on either constant meets both.
- **`Command::ShowGump::serial` and its siblings are still bare `u32`s from the
  script bridge.** Roughly a dozen script-raised commands name a mobile that
  way and each re-does `Serial::new` in the tick. That is one sweep of its own,
  and it belongs with the component sweep N4 left rather than with a protocol
  stage.

## Amendments forced by N6 (`login.rs`, `seed.rs`, `version.rs`)

The stage the plan sized at thirteen fields across three files, and the one where
the sweep's value showed up somewhere other than the wire: two of `login.rs`'s
dwords were a *pair* nothing but a reader's attention kept apart, and they were
sitting in adjacent fields of a server struct rather than in a packet at all.

1. **Two capability masks, one type, adjacent fields.** `0xA9`'s character-list
   flags and `0xB9`'s SupportedFeatures were both `u32`, both about whether the
   client behaves like an AoS client, and both stored in
   `openshard_login::LoginServer` one line apart (`character_list_flags`,
   `supported_features`). Swapping them compiles, and the shard it produces has
   clients that draw no tooltips with nothing anywhere logging why — the client
   does not complain about a mask it does not understand, it simply does less.
   They are `CharacterListFlags` and `SupportedFeatures` now, and the five loose
   `pub const u32`s became associated constants on the two, with the bits pinned
   in a test beside them (CLAUDE.md's rule for a ported flag). `with` rather than
   a `BitOr` impl, on N2 amendment 8's argument. The name is `SupportedFeatures`
   and not ServUO's `FeatureFlags` because this crate already has a `Feature`,
   which asks the opposite question — what a *version* can do, rather than what
   the shard claims.
2. **`address` + `port` is one `SocketAddrV4`, and the caller already had one.**
   `Relay`'s two fields were being filled from `LoginServer::game_address`, a
   `SocketAddrV4` taken apart at the call site and put back together in the
   encoder. This is N4 amendment 6's `GumpPoint` move with a std type instead of
   a new one — "look for it before writing it" — and it takes `port` off the
   count without a newtype existing for it. The `0x8C` byte order, which is the
   expensive thing in this file, is untouched and its test still asserts
   `[192, 168, 11, 6]`.
3. **One promotion replaced two checks a hundred lines apart.**
   `SelectShard::index` had `slot() -> Option<usize>` refusing zero in
   `protocol`, and `openshard_login::on_select_shard` separately refusing
   anything past `shards.len()`. `RawShardIndex::validate(offered)` does both and
   says which: `InvalidShardIndex::Zero` (the wire numbers from one, so a naive
   `index - 1` on a `u16` zero wraps to 65535) versus `PastEnd`. First error type
   in the sweep with two variants, and the reason is that the two are genuinely
   different bugs — one is impossible input, the other is a stale or forged list.
4. **`RawCharacterSlot` became class C, as the pilot's row predicted.** The pilot
   filed it class D with "it becomes class C the day slot choice is honoured";
   `0x83` delete honours it, and always did. Three packets carry the type and
   only this one reads it, so the promotion exists and two of its three uses
   still do not call it — which is the honest shape, not an omission.
5. **The store promotes, not the seam — because the list is the store's.**
   `Accounts::delete_character` takes a `RawCharacterSlot` and validates against
   its own character list. `dispatch.rs` validates too, against the list it just
   read to find the name. Two lists, two checks, neither pretending to be the
   other: a slot checked against somebody else's list is a check about the wrong
   thing. This is N2's "promotion lives on the seam" read strictly — the seam is
   wherever the domain is in hand, and here that is two places.
6. **`seed.rs`'s one field is class D and will stay that way.** `Seed::value` is
   the client's claimed IPv4 on old clients and the login cipher's key material
   on all of them. This engine implements no login encryption (the password is
   plaintext inside a cipher that is broken either way — see
   `AccountLogin::password`), and the address is the claim `RawClientIp` already
   refuses to believe with the socket's real address free for the asking. So
   `RawSeedValue` has no promotion, and the type is the record of that.
7. **`version.rs` needed no change at all.** Its four are `ClientVersion`'s own
   components and `ClientVersion` is not a packet struct — what arrives off the
   wire is a seed dword or a `0xBD` string, both narrowed into it. `Point`'s
   argument applies verbatim and the doc comment now says so. A stage whose whole
   answer is "the allowlist already covers this" is worth writing down: the next
   reader would otherwise re-derive it.
8. **`ShardEntry`'s two bytes are allowlisted, against first instinct.** They are
   adjacent `u8`s of unrelated meaning — `percent_full` and `timezone` — which
   looks exactly like the confusion the sweep exists to remove. It is not: N2
   amendment 3 settled this for `strength`/`dexterity`/`intelligence`, three
   adjacent `u8`s with the same shape. They are quantities, compared and clamped,
   and the only place either is written names it. Two newtypes here would be two
   types that only ever unwrap.
9. **Two byte-level writes changed type without changing bytes (N8).**
   `StartLocation::map` and `description_cliloc` went to `MapId` and `ClilocId` —
   reuse, per N4 — and their `out.i32` became `out.u32`. Same four big-endian
   bytes for every value either type can hold, and `create_character` lost a
   lossy `city.map as u8` on the way in.
10. **Bare-integer field count: `login.rs` 8 before, 2 after** (amendment 8's
    pair, allowlisted); **`seed.rs` 1 before, 0 after**; **`version.rs` 4 before,
    4 after**, all four allowlisted by amendment 7. `wire.rs` gained
    `CharacterSlot`/`InvalidCharacterSlot` and no bare fields.

### Backlog from this stage

- **`StartLocation::position` is a bare `(i32, i32, i32)`** and escapes N10's
  count entirely, because the count looks for `pub name: int` and a tuple has no
  names. It is a `Point` in everything but type — `create_character` casts it to
  one, `as u16`/`as i8`, three casts a `Point` would not need. The wire width is
  the only reason it is not one, and N8's "same bytes" would survive the change.
- **N10's counting check must count tuple fields too**, or it will report zero
  while the line above is still there. The same hole hides any
  `pub thing: (u16, u16)` a later packet adds.
- ~~**`ShardEntry::percent_full` is clamped in the encoder** (`.min(100)`)~~ —
  fixed. The field is a `PercentFull` with a private byte,
  `PercentFull::clamped` and `raw()`, plus `EMPTY`/`FULL`, so the encoder writes
  `shard.percent_full.raw()` with nothing left to repair. Clamped rather than
  refused, and the doc comment says why: every source of the number is a
  quantity meant to saturate at "full", where `RawShardIndex` refuses because a
  wrong index names a *different shard*. The decoder clamps too — a client
  reads this list from a server it has not met — so no `ShardEntry` in the
  process can hold a value the client would draw as garbage, in either
  direction. Two tests: one on the type at `0/99/100/101/250`, naming `100` as
  legal so a future clamp cannot quietly become a rescale, and one on the wire
  in both directions. Amendment 8 above allowlisted this field alongside
  `timezone`; that half of the amendment no longer holds, and the difference is
  that `100` is a rule the *client* imposes while a timezone has no range at
  all. `timezone` stays bare, and the allowlist in
  `tests/bare_integer_fields.rs` records the split.

## Amendments forced by N7 (`feedback.rs`, `skill.rs`, `combat.rs`, `properties.rs`, `spellbook.rs`, `encoded.rs`, `casting.rs`)

The tail, and the smallest modules in the sweep, but two of the seven turned up
the two shapes N3's four classes cannot answer alone — a value the *server*
computes and only the *client* reads back, and a third instance of the
decoder-destroys-the-byte finding — plus the collection blind spot N6's own
backlog had already named.

1. **`feedback.rs` originally needed no code change**, `version.rs`'s outcome
   again: its own module doc argued `action`/`animation_type` and the effect
   quantities stay bare. A later animation pass closed the stronger case:
   `Animation::frame_count` now carries `AnimationFrameCount`, shared by the
   client atlas and animation clock; the wire still writes the same `u16`.
   The remaining domain type (`openshard_state::Action`) lives in a server
   crate above `protocol` and cannot be held here. `speed`/`duration` are
   quantities (every caller passes a per-effect literal, nothing branches on
   either), and `HuedEffect::
   render_mode` is untouched for a third reason: no non-test code in this
   workspace constructs a `HuedEffect`, so there is no caller to classify
   against. All three reasons are recorded in the module doc rather than only
   here, so the next reader finds them beside the fields.
2. **`RawSkillId` moved from `world.rs` to `wire.rs`**, N4's plain counting
   rule: `skill.rs` is its second user and, unlike `RawLayer`'s twin (a
   validated `Layer` that lives in this crate), `RawSkillId`'s twin —
   `openshard_state::Skill` — lives in a server crate above `protocol` and has
   no crate-local pairing to follow instead.
3. **`SkillLockRequest::skill` and `UseSkillRequest::skill` were bare `u8`s,
   not even a `Raw` type, until this stage** — the sweep's own blind spot: a
   field can look done because a neighbouring module already named
   `RawSkillId`, when the packet actually holding client input never adopted
   it. Both are `RawSkillId` now, with the promotion pilot amendment 1's
   licence extended one step: `openshard_state::Skill::from_id` is the check,
   called at the seam that owns the domain, and `RawSkillId` itself carries no
   promotion method because that type cannot live in `protocol`.
4. **A hostile skill id has needed no game-balance number, unlike the pilot's
   three deferrals — and one of the two checks was missing entirely.**
   `SkillLockRequest::skill` reached `Skills::set_lock`, a bare `HashMap`
   insert, with nothing on the path checking it against the table; `World::
   set_skill_lock` now refuses (and logs) an id `Skill::from_id` does not
   know, with N9's pair — decodes cleanly, refused at promotion — living in
   `crates/common/protocol/src/skill.rs` for the decode half and in
   `openshard_world`'s test suite for the seam. `UseSkillRequest::skill` was
   already checked, one hop further down (`skill::info` in `skills::
   use_skill_button`, predating this stage) — wrapping it only made the check
   visible, per N3's whole argument.
5. **`combat.rs`'s `HealthBar` gained no new type: it reused `mobile::Vitals`.**
   `max`/`current` are exactly the current/max pair N2 amendment 2 named the
   shape for, and every constructor already built both at once. The wire order
   (max, then current) is the encoder's business and unaffected by the fold.
6. **`properties.rs`'s `TooltipRevision::hash` is a class N3's table cannot
   name**: not client input (B/C do not apply), not a value with an existing
   type (A does not apply), and class D's shape runs backwards here — D is
   "the client claims it, the server never reads it"; here the *server*
   computes it (`PropertyList::add_hash`) and the *client* is the only reader.
   Left as a documented bare `u32` rather than forced into a class that does
   not fit, with the doc comment naming the gap so a real fifth class is a
   deliberate choice if one is ever added, not a drift.
7. **The collection blind spot N6's own backlog predicted, one stage later.**
   `PropertyQueryRequest::serials` was a bare `Vec<u32>` — invisible to N10's
   counter for exactly the reason the N6 backlog gave for `StartLocation::
   position`'s tuple: the counter looks for `pub name: int` and neither a
   tuple nor a `Vec<int>` matches. It is `Vec<RawSerial>` now, and the seam
   that reads it (`World::query_properties`) already had the right shape —
   `Serial::new(serial)` per element — so this was a rename, not new logic.
   **N8's counter needs both holes closed, tuple and collection, or it will
   report a number lower than the truth.**
8. **`spellbook.rs`'s `serial` and `graphic` are class A, and `offset` stays
   bare by the same test N3 amendment 1 used for `TalkMode`'s named variants:**
   something must already branch on the byte before a name pays for itself,
   and nothing does — every call site sends the literal `1` (Magery), because
   no second spell school is wired up. The day one is, `offset` is a real
   class-B candidate; today a name would be a guess with one value ever
   observed.
9. **`encoded.rs` split one struct into three types, the `TalkMode` shape
   applied to a subcommand word for the first time in this sweep.**
   `EncodedCommand::serial` is class D — never read, the same shape as
   `gump.rs`'s `RawGumpKey` (an id, not this file's business, addressed from
   the connection instead) — and got a named `RawEncodedSerial` rather than
   staying a bare `u32`, because N4's rule is that class D gets a type and no
   second step, not that it gets nothing. `subcommand` is `RawEncodedSubcommand`
   → `EncodedSubcommand { SetAbility, GuildGumpRequest, QuestGumpRequest,
   Other(u16) }`, total, and the three `pub const u16`s `EncodedCommand` used
   to carry are gone with the match that used to compare against them by
   number — N11, no compatibility shims, one commit.
10. **`casting.rs` is this stage's `StatLockRequest`/`0xAD` finding a third
    time, and the sharpest of the three.** `CastSpellRequest::decode_body`
    folded the wire's one-based spell id with `saturating_sub(1)` *while
    decoding*, so a client-sent `0` (never legitimate — the wire numbers
    spells from one) and a client-sent `1` (a real spell: Magery's first,
    confirmed present in `magic::info(0)`) both became the stored zero-based
    `0`. Nothing downstream could tell a hostile zero from the first spell in
    the book. The fold is now `RawSpellId::interpret`, class B and total —
    every `u16` has an answer, `0 => None`, `n => Some(n - 1)` — run at the
    network seam (`dispatch.rs`) because a total interpretation cannot refuse
    a connection, `docs/protocol_newtypes.md`'s N4 containers amendment 2
    licence again. Whether the resulting number names a spell in the table is
    unchanged: `magic::info`'s job, downstream, fallible.
11. **Bare-integer field count: `feedback.rs` 10 before, 9 after** (the later
    `AnimationFrameCount` follow-up removes one from amendment 1);
    **`skill.rs` 6 before, 4 after** (`SkillEntry`'s
    `id`/`value`/`base`/`cap`, allowlisted for the same two reasons as
    `feedback.rs` — domain above `protocol`, and quantities); **`combat.rs`** 2
    before, 0 after (folded into `Vitals`, itself already on the allowlist);
    **`properties.rs`** 2 before (plus the invisible `Vec<u32>`, amendment 7),
    1 after (`hash`, amendment 6); **`spellbook.rs`** 3 before, 1 after
    (`offset`, amendment 8); **`encoded.rs`** 2 before, 0 after; **`casting.rs`**
    1 before, 0 after.

## Amendments forced by N8 (the sweep)

The last stage was supposed to be counting and documenting, not finding new
fields — and mostly was. The counter itself is what found the exceptions.

1. **`login::StartLocation::position` was the tuple N6's own backlog named,
   and it is `world::Point` now.** The wire still writes three full
   dwords — unlike every other position on the wire, which is `Point`'s own
   `u16`/`u16`/`i8` — so encode widens with `i32::from` and decode narrows
   back with `as`, the same shape `PlayerStart`'s `0x1B` already uses for its
   `z`. This is server → client data (N1's direction rule), so there is
   nothing to validate: the narrowing is a wire-width fact, not a hostile
   value, and every value this shard ever writes here (nine literal cities,
   one fallback) fits comfortably. Three sites got shorter for it —
   `dispatch::start_cities`'s `city()` helper takes `Point`'s own field types
   now instead of `i32` triples, and `dispatch::create_character` stopped
   rebuilding a `Point` from three casts because `city.position` already is
   one — the class-A "wrapping deletes code" pattern one more time.
2. **The counter is a text scan, not a syntax tree, because nothing in this
   workspace parses Rust today.** `syn`, `walkdir` and an `xtask`-style binary
   are all absent from every `Cargo.toml` in the workspace, and every
   existing self-check in this crate (`feature.rs`'s
   `all_lists_every_feature_exactly_once`, `direction.rs`'s
   `every_byte_the_client_can_send_names_a_direction`) asserts over program
   data already in memory, not over source text. Pulling in a parser for one
   test would be the heavier and less-precedented choice. So
   `tests/bare_integer_fields.rs` reads `src/*.rs` as strings and looks for
   lines starting with `pub name:` — which a struct field satisfies and a
   method, an associated const, or a tuple struct's own definition (no colon
   on that line at all) do not, so none of those needs a separate exclusion
   rule.
3. **The scan matches the *type expression*, not the field's own type — which
   is what closes both of N7's blind spots with one rule instead of two.** A
   tuple (`(i32, i32, i32)`), a `Vec` of one (`Vec<u32>`) and a `Vec` of a
   tuple containing one (`gump::GumpResponse::text_entries: Vec<(u16,
   String)>`) all contain the token `u16`/`i32` somewhere in their text, so
   the same "does this type name a bare integer anywhere" check catches all
   three without a second regex for tuples and a third for collections. A
   type that names no bare integer at all — `Point`, `Vec<RawSerial>`,
   `Vec<CharacterEntry>` — never matches, which is what keeps the newtypes
   the sweep already wrote off the list.
4. **Running the scan surfaced four fields no earlier stage had reasoned
   about, because they are not packet fields at all.** `error::WrongPacket`'s
   `expected`/`found`, `gump::InvalidSwitchId::id`,
   `context::InvalidContextMenuIndex::tag` and `wire::InvalidCharacterSlot::slot`
   are diagnostic data on typed errors (N7's own rule) — the value a
   `validate` call rejected, carried so its `Display` impl can say what was
   wrong. None of N3's four classes is written for this shape: it is not
   wire data at all, inbound or outbound, so direction (N1) does not apply to
   it either. They are allowlisted rather than forced into a class that does
   not fit, the same call N7 amendment 6 made for `TooltipRevision::hash`.
5. **`gump::GumpPoint::{x, y}` and `spellbook::SpellbookContent::content` were
   real gaps, not new shapes.** `GumpPoint` is `Point`'s own argument
   (N1/N2's geometric-component case) and should have joined the allowlist
   when N5 amendment 6 introduced the type; it is there now. `content` is the
   64-bit spell-membership bitmask `SpellbookContent::offset` already sits
   next to — N7 amendment 8 reasoned about `offset` and silently dropped
   `content` from the same paragraph. Same argument extends to it: which bit
   means which spell is a Community Pack table this crate does not hold, the
   `feedback.rs` module doc's argument for `action`/`animation_type` again.
6. **The check is bidirectional on purpose.** It fails on a bare field with no
   allowlist entry, which is the violation N10 was written to catch — and
   also fails on an allowlist entry with no matching field left in `src/`,
   which catches the mirror mistake: a field gets a real type in some later
   change and whoever did it forgets the now-stale allowlist line. A one-way
   check would let the allowlist grow forever and never shrink.
7. **`docs/style.md` gained a short section**, pointing here rather than
   restating N3's table: the newtype rules it already had (no `Deref`, open
   with `.0`, typed errors) apply to `Raw*` types without change, and
   repeating the four-class table in two documents is the kind of copy this
   repository's own `CLAUDE.md` warns goes stale silently.

## Amendments forced by N-components (the component sweep)

The stage N4's two backlogs recorded twice and deferred twice. It is the first
one whose subject is not a protocol module at all — the components sit in
`openshard_state`, above `protocol` — and what it cost was not the components
but everything they are read *against*.

1. **The item graphic component is `Drawn`, and the rename was the enabling
   move rather than a tidy-up.** While both it and `wire::Graphic` were called
   `Graphic`, one conversion had three spellings across the server: a full
   `openshard_protocol::wire::Graphic(..)` path in a dozen files, an
   `as WireGraphic` import in four crates, and a comment in `components.rs` and
   `world::tick::command` explaining the collision to the next reader. All of
   them are gone, and the component is now named for what it does to an item
   rather than for what it is made of — which is also how its neighbours read
   (`Contained`, `Equipped`, `Stackable`, `Hidden`).
2. **`Contained`'s `x`/`y` became the `GumpPoint` N4 amendment 6 parked.** N5
   made the type; this stage was the one allowed to use it. The payoff is at
   `items::contained_record`, which built the packet's `GumpPoint` from two
   loose halves and now copies one field — the conversion that could disagree
   about which space the pair was in no longer exists.
3. **Typing a component types the table it is keyed by, and that is the stage.**
   `Drawn.id` reaches `armor_data`, `weapon_data`, `instrument_data`,
   `tool_data`, `craft_tool`, `mount_item_for`/`mount_body_for`, `tamable`,
   `creature_name`, `creature_base_sound`, `body_type`, `body_opens_doors`,
   `scroll_spell` and the five `Terrain` tiledata methods. Each was a
   `fn(u16) -> ..` over a table of art ids; each takes a `Graphic` now. The
   components were a day; the tables were the rest of it.
4. **A terse table keeps its bare literals, and the wrap goes in the row
   helper.** `ore`, `wood`, `i`, `a`, `t` and the `TOOLS` tuple list stay aligned
   blocks that read as data — this is N3 amendment 7's `instrument.rs` decision
   applied to six more tables. The variant for a *lookup* rather than a table is
   to open the argument once at the top (`let graphic = graphic.0;` in
   `craft_tool`, `match body.0` in `creature_name`), so the arms below stay the
   art table they are.
5. **A base with arithmetic on it stays an integer; the result is named.** A
   creature's attack and death sounds are `BaseSoundID + 2` and `+ 4`, and
   `doorgen`'s facing offsets are `DARK_WOOD_BASE + 2 * index`. The base is
   opened for the arithmetic and the answer wrapped once on the way out — the
   same split N-tables settled on for Arms Lore and Anatomy clilocs, for the
   same reason: a newtype names an identity, not a quantity still being counted.
6. **`MapTerrain`'s five `Terrain` methods are a seam, and they are the only new
   `.0` in a lib.** `tiledata.mul` is indexed by a bare `u16`, and the ids in a
   map's static blocks never went through a packet, so the newtype stops at the
   client-file boundary rather than inside `openshard_uofiles`. The other `.0`s
   this stage added are the ones that were already supposed to exist: the
   persistence record and both stores' SQL, the script bridge's JSON numbers in
   `server/scripting.rs`, and the gump layout language (N5 amendment 9).
7. **The generator was changed with the tables it generates.**
   `tools/gen-craft-tables/generate.cjs` emits `Graphic(..)`/`Hue(..)` and the
   import line, so regenerating `crafting/src/defs/*.rs` — 2,323 typed literals —
   does not silently walk the sweep back. A one-shot generator whose output has
   been hand-edited since is exactly the thing that reverts a sweep a year later.

### Backlog from this stage

- ~~**`PropertyList::add`/`add_args` still take a bare `u32` cliloc.**~~ —
  fixed. Both now take `ClilocId`; `object_properties`
  (`crates/server/state/src/runtime.rs`) already held one everywhere it called
  them, so the change was the signature and its call sites, not a new check.
  The one place a `cliloc` still meets a raw number is the `#{cliloc}` argument
  string for `1_050_039`'s stack-amount template — that number is *text* the
  client's own cliloc parser substitutes into another string, not a value this
  crate reads, so it stays `cliloc.0` there.
- ~~**`items::drop_into_container` takes a `Point` that is holding gump
  coordinates.** The `0x08` reuses the position field for both meanings, so the
  parameter is a world `Point` and the function converts it to a `GumpPoint` at
  the one place the two part company. The honest fix is upstream, in how the
  packet is interpreted — a drop onto the ground and a drop into a container are
  not the same request — and it is a `containers.rs` question, not this stage's.~~
  — **fixed (N-drop, below), and the honest fix is what landed.**
- ~~**`crafting::system::Text::Cliloc(u32)` is still bare**, for N-tables' reason:
  it is ServUO's `TextDefinition` and doubles as gump-label text, so it is a
  wider structure than a message id.~~ — **fixed (N-cliloc, below). The reason
  above described the `Text` enum, not the variant: `Text` is wider than a
  message id, and `Cliloc`'s payload is exactly one.**
- ~~**`Command`'s script-facing serials are still bare `u32`.** `ShowGump::serial`
  and roughly a dozen others take a raw serial where the tick then calls
  `Serial::new`. The bridge in `server/scripting.rs` is the seam that should
  make them, exactly as it now makes every `Graphic` and `Hue`.~~ **Done** —
  N-commands below.

## Amendments forced by N-commands (`Command`'s serials)

The stage the component sweep's backlog named, and the second whose subject is
not a protocol module: `Command` is `openshard_world`'s own enum, above
`protocol`, and its twenty-seven bare `u32` serials were the last place a raw
wire number travelled through the engine untyped. Typing them turned out to be
the smaller half. What the stage actually cost was the `0`-as-sentinel habit
underneath them, and it found a protocol module the sweep had never staged.

1. **The bridge makes the `Serial`, and a refusal drops the command with a log
   line.** `into_world` returns `Option<Command>` now, and `script_serial`
   (`server/scripting.rs`) is the one place a script's JSON number becomes a
   `Serial` — the seam `Command::Speak`'s `Hue` and `PlaySound`'s `SoundId`
   already crossed. Unlike those, the conversion can fail, and the *old*
   behaviour was the reason to move it: a pack naming `0` or a number past the
   item pool travelled to the tick as a bare `u32`, was refused there by the same
   `Serial::new`, and the command did nothing with nothing said — indistinguishable
   from a mobile that had logged out between the event and the reply to it.
2. **A field whose absence is a *value* stays an `Option` and is not logged.**
   `Damage::by` (unattributed damage), `CastSpell::target` and `CastSpell::pack`
   are `Option<Serial>`, made with a plain `Serial::new`: the script's `0` is its
   word for "none", and `Serial::new` already answers `None` to it. The
   distinction from amendment 1 is N2 amendment 1's, applied to a command rather
   than a packet — refuse where the value names something, be silent where it
   names nothing.
3. **The two client-supplied ones are `RawSerial`, by N1's direction rule.**
   `TradeAction::container` and `TradeCancel::container` come off a `0x6F`, so
   they cross the queue raw and `items::trade` promotes them —
   `set_accepted`/`cancel_by_container` take a `RawSerial` and call `validate`,
   which is the same `Serial::new` they were doing by hand. The queue is a
   delivery and not a checkpoint (N3 amendment 9), one more time.
4. **`trade.rs` is a protocol module the sweep never staged, and N8's counter
   cannot see it.** Its three inbound `container` fields are bare `u32`s inside
   *enum variants* — `SecureTradeAction::Cancel { container: u32 }` — and the
   scan looks for lines starting with `pub name:`, which a variant's field is
   not (it carries no `pub`). Its outbound serials were worse: `encode_trade_open
   (partner: u32, mine: u32, theirs: u32)` and its two siblings are *function
   parameters*, which no field scan of any kind would find. **This is the third
   blind spot in N10's counter, after N7's tuple and collection**, and unlike
   those two it hid a whole module rather than a field. The three inbound fields
   are `RawSerial` now and the three encoders take `Serial`s; the bytes are
   unchanged and both `0x6F` byte-level tests still assert what they did.
5. **`0` as "no target" ran twelve branches deep, and it was a *parameter*.**
   `World::apply_spell_effect(target_serial: u32)` used `0` for a self-cast or an
   area spell and re-tested `!= 0` in every arm, twice reaching for
   `by.map_or(0, |s| s.raw())` to fall back to the caster. It takes an
   `Option<Serial>` now and each arm is `target_serial.or(by)` or a plain `if
   let`. This is N1 amendment 7's `map_or(0, …)` finding for the fourth time and
   the first outside a packet field: the same wrong value, spread over a call
   tree instead of a struct. `caster_pack` (`0` for "wears no pack") and
   `spell_feedback` went with it.
6. **Typing the command types the system function, and that is where the code
   went.** `combat::damage`, `combat::apply_poison`, `combat::cure_poison`,
   `magic::heal`, `magic::cast_spell`, `magic::pay_and_roll`, `magic::apply_*`,
   `skills::set_stats`/`set_skill`/`set_skill_cap`/`use_skill`,
   `items::set_weapon`/`set_poison`, `npc::vendor::stock`, `npc::live`,
   `quests::advance_escorts` and eight private `World` methods all take a
   `Serial` now. Every one of them opened with its own `Serial::new(serial)`;
   fourteen of those guards are gone, and so are the `.raw()` calls at the
   callers that already held a `Serial` — `traps.rs` (five), `guards.rs`,
   `gm.rs`, `skills_wire.rs`, `fields.rs`. N2 amendment 1's "wrapping deletes
   code", now in both directions at once across five crates.
7. **`skills::handlers`' three target entry points took `Option<Serial>`,
   which deleted the sweep's last `map_or(0, …)`.** `staff.rs` built
   `object_raw = response.object.map_or(0, |s| s.raw())` from an
   `Option<Serial>` it already had, purely to satisfy `on_target`,
   `on_second_target` and `on_item_target`. All three take the `Option` now and
   the local is gone.
8. **`DecorDoor::key_value` and `DecorContainer::key_value` stay bare, and are
   not serials.** ServUO's lock value: a key opens a door when its own number
   matches, and `0` means unlocked. It is a match token the pack chooses, never
   an object reference — the `MobileStatus` quantity argument (N2 amendment 3)
   in a different shape. They are the only bare integers left on `Command`.
9. **One test's stray serial had to become an addressable one.** The AddLoot
   "nothing exists at this number" test used `0xDEAD_BEEF`, which `Serial::new`
   refuses, so with a `Serial` field the case cannot be expressed at all — the
   command now cannot carry it. It uses `0x4EAD_BEEF`, in the item pool and held
   by nothing, and still asserts nothing is placed. Exactly N2 amendment 9's
   `remove_is_five_bytes` fix, and the second time this sweep has found a test
   that was proving something the type system now proves.
10. **Bare-serial field count on `Command`: 27 before, 0 after**, plus
    amendment 8's two `key_value`s which were never serials; **`trade.rs` 3
    before, 0 after** (the enum-variant fields amendment 4 found) and its three
    encoders' seven `u32` parameters typed with them.

### Backlog from this stage

- ~~**N10's counter still cannot see an enum variant's fields or a function's
  parameters** (amendment 4).~~ — **half fixed (N-gate, below): the variant case
  is closed, the parameter case is argued and left open on purpose.**
- ~~**`CharacterRecord::serial` and the persistence records around it are bare
  `u32`s.**~~ — fixed (N-persistence, below).

## Amendments forced by N-persistence (`record.rs` and its stores)

The last bare serials in the engine, and the one place N1's direction rule does
not apply: a save record is neither client → server nor server → client, it is
server → itself, across a restart. What made the stage more than a find-and-replace
was that `record.rs` already had the pattern to follow, for a different type.

1. **`AccountName`/`CharacterName`'s `serde(with = "...")` modules are the
   template, unchanged.** Both already prove the shape this sweep needs: a typed
   field, a hand-written `Serializer`/`Deserializer` pair beside it, and an
   on-disk shape (a bare JSON string) that does not move when the Rust side gets
   a newtype. `serial`/`optional_serial` are the same two functions for `Serial`,
   writing and reading the same `u32`/`0` a bare field always did — no
   `SCHEMA_VERSION` bump, because nothing on disk changed, only what the type
   system will let past it in memory.
2. **`Serial` gains no `ToSql`/`FromSql` impl, on the same argument N7 amendment
   6 and N8 amendment 4 already settled for a type with no natural home in one of
   N3's four classes.** A trait impl on the newtype would make the SQL boundary
   invisible the way `Deref` would, and CLAUDE.md's ban on both is one rule, not
   two exceptions. `sqlite.rs` opens with `.raw()` on the way in and reads through
   a named `get_serial`/`get_optional_serial` pair on the way out — the same
   `.0` at the serialization seam the wire and the JSON both already use, moved to
   a third seam. A row that fails `Serial::new` fails the read with
   `rusqlite::Error::IntegralValueOutOfRange`, routed through the store's existing
   `database()` — a corrupt column is exactly as fatal as `z` overflowing an
   `i8` already was, not a new kind of error.
3. **`ItemRecord::owner`'s `0` and `MobileRecord::spawned_by`'s `0` looked like
   the same sentinel and are not, and only one of them is a `Serial`.**
   `owner == 0` means "no character owns this" over the *serial* namespace, so it
   became `Option<Serial>` through `optional_serial`. `spawned_by` is a
   `SpawnedBy` index into the world's own spawner list — a namespace that
   legitimately starts at zero, which `Serial::new` would refuse outright. A
   first pass converted it anyway and then reached for the wrong fix (offsetting
   every spawner id by one to dodge the refusal), which would have changed the
   on-disk shape for a false cognate; the field stays `Option<u32>`, with a
   comment now saying why, rather than gaining a type it does not own. Two
   fields shaped alike and ruled differently is exactly N7 amendment 6's
   `TooltipRevision::hash` lesson again: a class table answers most fields, and
   the ones it does not are worth a sentence rather than a guess.
4. **Wrapping deleted the `Serial::new(record.serial)`/`.raw()` round-trip at
   every read and write in `tick/persist.rs`,** and the `record.owner == 0`
   checks folded into plain `Option` matching — N2 amendment 1's "wrapping
   deletes code" one more time, this time on the load/save path rather than a
   packet.
5. **The two round-trip tests in `record.rs` are the N9 pair for this stage,
   read the other way round.** They already existed to prove every field is
   reachable by name; unchanged in intent, they now also prove the on-disk shape
   survives the type change — a `CharacterRecord`/`ItemRecord` built with typed
   serials serialises to the same JSON integers and reads back equal.

## Stages

Each stage ends with all four silent: `cargo check --workspace --all-targets`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`cargo fmt --all`. Each stage is one or more commits, landed through a pull
request (`main` is protected).

- **N-pilot — `CreateCharacter`, `CharacterPlay`.** By hand. Establishes the
  four classes, the two method names, the shared `wire.rs` types
  (`RawHue`, `RawGraphic`, `RawCharacterSlot`, `RawClientIp`), the first typed
  `Invalid*` errors, and the three missing checks.
- **N1 — the rest of `world.rs`** (movement, `0x02`, and the outbound world
  packets). The largest module, and the one whose inbound/outbound mix exercises
  N1's direction rule hardest.
- **N2 — `mobile.rs`.** Almost entirely outbound: mostly class A, the stage that
  proves the direction rule saves work rather than doubling it. It also lands
  `RawSerial` in `serial.rs` and `Layer` in `wire.rs`, both of which every later
  stage uses.
- **N3 — `speech.rs`.** One header sent five times, in both directions. Lands
  `TalkMode`/`RawTalkMode` and `Font`/`RawFont` in `speech.rs` and `ClilocId` in
  `wire.rs`; the four later stages that carry a cliloc use the last one.
- **N4 — `items.rs`, `containers.rs`.** Two directions on the same item, and the
  stage that answers N2's parked question about `Equipped.layer`.
- **N5 — `vendor.rs`, `gump.rs`, `context.rs`.** Gump ids and button ids are the
  interesting inbound case: a `0xB1` response echoes ids the *server* chose, so
  the check is "is this one I offered", not a range.
- **N6 — `login.rs`, `seed.rs`, `version.rs`.** The stage where the confusion the
  sweep exists to remove turned up outside a packet: two capability masks, one
  `u32`, adjacent fields of a server struct.
- **N7 — `feedback.rs`, `skill.rs`, `combat.rs`, `properties.rs`,
  `spellbook.rs`, `encoded.rs`, `casting.rs`.** The tail. Stayed mechanical
  enough for one commit, but turned up a third decoder-destroys-the-byte
  finding (`casting.rs`) and the collection blind spot N6's backlog predicted
  (`properties.rs`).
- **N8 — the sweep.** The counting check from N10, the allowlist with reasons,
  and a pass over `docs/style.md` if the four classes deserve a line in the
  canon.
- **N-tables — the cliloc and `SoundId` content tables.** Not a protocol module:
  the four `openshard_state::WorldState` doors every gameplay crate says a line
  or plays a sound through. Blocked until `protocol` was allowed a `serde`
  dependency, which is what lets a message id or a sound arrive from a content
  table already typed instead of being wrapped at each of ~200 call sites.
- **N-commands — `Command`'s serials.** Not a protocol module either: the world's
  own command enum, whose twenty-seven script- and client-facing `u32` serials
  were the last raw wire numbers travelling through the engine. The script bridge
  makes the `Serial`, and typing the commands typed every system function under
  them — which is where the stage's size was.
- **N-components — the components under the packets.** `openshard_state`'s
  `Drawn` (the item graphic component, renamed out of the way of `wire::Graphic`),
  `Container.gump` and `Contained.{position, grid}` — the bare integers N4's
  backlog recorded and deferred. Typing them forces the graphic- and hue-keyed
  content tables they are read against, which is most of the stage's size.
- **N-persistence — `record.rs` and its stores.** Not a protocol module either,
  and not the wire: `CharacterRecord::serial` and its siblings across
  `ItemRecord`, `MobileRecord`, `DecorationRecord`, `PetData`, `Inventory` and
  `QuestRecord`, the last bare serials in the engine, at the save/load seam
  instead of the network one. Typing them forces a third `.raw()` boundary
  (SQL bind/read) beside the wire and the script bridge.

Stages N1–N7 are agent work. They are ordered by module size rather than
dependency: `wire.rs`'s shared types all land in the pilot, so nothing after it
blocks anything else, and two stages can run in parallel when they touch
disjoint modules and disjoint call sites.

## The agent recipe

A stage is handed to a cheap agent with this, verbatim, plus the module list:

> Read `docs/protocol_newtypes.md` first — N1 through N11 are settled decisions,
> do not re-litigate them, and the four-class table in N3 is the whole recipe.
> Read `docs/style.md` for how code here reads. Use the
> `mcp__rust-code-mcp__*` tools (`search`, `find_definition`,
> `find_references`) rather than grep sweeps; they are deferred tools, so call
> them explicitly.
>
> For each module named: list every `pub` field of a bare integer type in a
> packet struct. For each one, decide the *direction* of the struct (N1) and the
> *class* (N3), then write exactly what the class says — no more. Reuse an
> existing type before adding one (N4); the wire newtypes are in `wire.rs` and
> `serial.rs`.
>
> Update every call site in the same commit (N11). Keep every byte-level test
> asserting the same bytes (N8). Add, for each class C field, the pair of tests
> N9 asks for. Report the bare-int field count for each file before and after
> (N10), and record anything the class table could not answer — that is an
> amendment for the doc, not a decision to make alone.
>
> Done when `cargo check --workspace --all-targets`, `cargo test --workspace`,
> `cargo clippy --workspace --all-targets` and `cargo fmt --all` are all silent.

A finding the class table cannot answer stops the stage and comes back as a
proposed amendment. That is the one thing an agent must not improvise: the
predecessor plan's value was that every stage's surprise got written down
(`0xB9` not fitting `EncodePacket`, `CreateCharacter`'s two ids), and a surprise
resolved silently in one module is a pattern the next module contradicts.

## Amendments forced by N-gump (`GumpLayout`'s cliloc parameters)

The last of the backlog, and the one that proves N-gate's parameter gap is not
theoretical.

**1. The "~190 bare cliloc call sites" were a symptom, not the disease.**
`ClilocId` had existed since N-tables and most of the engine used it; what kept
producing bare numbers was that `GumpLayout::html_localized`,
`html_localized_colored` and `html_localized_args` took `cliloc: u32`. A
parameter, not a field — so N10's counter, which scans `pub name: type`, has
never once looked at them, and every caller had to hand a naked number to a
typed API. Three signatures changed and the bare numbers had nowhere left to
come from. This is exactly the class N-gate documented as out of reach and the
reason to keep re-opening the `syn` argument.

**2. The constants moved with them.** `quests::gump`'s sixteen `pub const X:
u32` cliloc table became sixteen `ClilocId`s, `crafting::gump`'s
`skill_label` now returns one, and `crafting::system::Text::Cliloc` carries one
— which meant `build.rs` had to emit `openshard_protocol::wire::ClilocId(n)`
fully qualified, because the file it generates is `include!`d into modules whose
imports it cannot see.

**3. A zero sentinel fell out on the way.** `CraftGumpContext::notice` was a
`u32` documented "zero for none", read through an `if context.notice != 0`.
Cliloc `0` is a number the client would happily look up, so "no notice" and
"notice number zero" were the same value and only that one comparison told them
apart — the shape `docs/style.md` names as worse than an `Option` because it
reads like a value somebody chose. It is `Option<ClilocId>` now and the
comparison is a `if let`.

**4. What stays open, and is no longer this document's problem.** The original
entry also wanted the numbers to *come from a content table* rather than be
literals in Rust. That is worth doing and it is a Community Pack question:
`ClilocId` is `#[serde(transparent)]`, so a loader can read one straight into
the newtype. Nothing about it is a newtype decision any more.

## Amendments forced by N-drop (the `0x08`'s two coordinate spaces)

The one backlog entry the sweep left that was a *design* bug rather than a
missing type: `0x08` carries one position field with two meanings, and the
struct that decoded it carried both meanings forward as a world `Point`.

**1. A newtype cannot fix a field that means two things.** `Point` was already a
newtype and already correct; what was wrong is that the same field was sometimes
a map tile and sometimes a pixel offset into a container's gump, and the type
system was being asked a question the type could not hold. Every seam downstream
took a `Point` that was sometimes not one, and the conversion happened at
whatever depth first noticed — `drop_into_container`, four calls deep. The rest
of this sweep was "give the number a name"; this one was "the number is two
numbers".

**2. The fix is an interpreted destination, in the crate's own idiom.**
`DropItem::destination` reads the container field and returns a
`DropDestination`: `Ground(Point)`, `Item { item, at: GumpPoint }`,
`Mobile(Serial)`, `Nowhere`. It is the `Raw*::interpret` shape N3 class B already
uses everywhere — total over all 2³², with the ordering of the checks as the
whole rule, because [`DROP_TO_GROUND`] is outside both serial pools and would
otherwise fall in with the values that address nothing. `Command::DropItem` now
carries the destination instead of a `(position, container)` pair, so the choice
is made once, at the packet, and nothing below can re-derive it differently.

**3. `Item`, deliberately not `Container`.** The client sends the identical shape
for a drop into a bag, onto a stack to merge with, onto a spellbook and onto a
runebook; which it is depends on components the wire knows nothing about. The
variant claims only what the wire actually said — the target is in the item pool.
Naming it `Container` would have been the same class of lie the position field
was telling.

**4. `Nowhere` is a variant, not an `Option`.** A destination that addresses
nothing still owes the client a `0x27`: the item is on its cursor, and a seam
that quietly does nothing leaves the server believing the item is held forever.
As an `Option` that obligation is something a caller can drop by writing `if
let`; as a `match` arm it is something the compiler asks about. There is a test
named for exactly that (`a_drop_that_addresses_nothing_bounces_rather_than_
swallowing_the_item`).

**5. What it deleted.** `DropItem::to_ground`, the `RawSerial` → `Serial`
validation inside `drop_onto_item` (the destination already validated it, so the
function takes a `Serial` and is renamed `drop_onto_serial`), and the
`GumpPoint::new(i32::from(position.x), …)` conversion in the middle of
`drop_into_container`. The two `trade.rs` callers that passed `Point::default()`
into a gump-space parameter now say `GumpPoint::new(0, 0)` and read as what they
are. Wrapping deletes code, again.

## Amendments forced by N-gate (the coverage check itself)

Not a module: the check that all the other stages are enforced. N8's own backlog
left it with two blind spots, and by the time N-commands had to find `trade.rs`'s
three variant fields *by reading*, the gate had stopped being a gate — a sweep
that only a person can verify is a sweep that decays the first week nobody looks.

**1. An enum variant's fields carry no `pub`, so the scan walked past all of
them.** Closed. The walk is a second pass with its own bracket state machine,
because the two shapes share nothing textually: a struct field is recognised by
`pub`, a variant's field only by where it sits. It runs between `enum Name {` and
the brace that closes it and nowhere else, which is what keeps the `impl` blocks
and the `{ resizepic … }` layout examples in the doc comments out of the depth
count. Struct-like variants key as `Variant.field`, tuple ones as `Variant.0`.

**2. What the blind spot was hiding was nothing — and that is the finding.** All
fifteen fields it turned up fall in classes N3's table had already decided on a
struct field: five are a `Raw*::interpret` leftover arm (the byte is carried
through *because* this engine has no name for it, and typing it would assert the
meaning the arm exists to deny), five are a diagnostic on a typed error
(`WrongPacket::expected`'s argument — carried for `Display`, already rejected,
never read as wire data again), three are quantities on the `MobileStatus`
argument, and `ClientPacket::Unknown.body` is a `Vec<u8>` buffer that the
deliberately broad type match cannot tell from a number. No new type was needed.
The gap hid no untyped wire value; it hid the *evidence* that none was there.

**3. A function's parameters stay out of reach, on purpose.** `fn encode(serial:
u32)` is the same bare integer and no field scan can see it. That wants `syn`,
and N8's argument against the dependency has not changed — so instead of a
half-measure the scan now *reports what it examined*: files, enum bodies,
variants, with a floor asserted on each. "No violations" can no longer mean
"nothing was read", which is the failure mode this file was written against in
the first place and which had already produced a green, meaningless result here
once. The parameter gap is documented, not papered over.

**4. The detector has a fixture.** `the_enum_walk_sees_what_it_claims_to_see`
runs the walk over synthetic source containing every shape that sits next to a
variant field — a unit variant, a discriminant, a doc comment with braces, a
typed tuple element, a method parameter after the body — and asserts the exact
set of hits. A detector that has never been shown to go red is worth nothing
green, and this one reads source text it does not own, so the fixture is the only
place its behaviour can be pinned.

## Progress

| Stage | State | Commit |
| --- | --- | --- |
| pilot | types landed, promotions deferred (see amendments) | |
| N1 | done — `world.rs` 20 bare int fields → 5 allowlisted | |
| N2 | done — `mobile.rs` 37 bare int fields → 12 allowlisted | |
| N3 | done — `speech.rs` 22 bare int fields → 0 | |
| N4 | done — `containers.rs` 9 → 3, `items.rs` 16 → 3, all allowlisted | |
| N5 | done — `vendor.rs` 14 → 5 allowlisted, `context.rs` 6 → 0, `gump.rs` 9 → 0 | |
| N6 | done — `login.rs` 8 → 2 allowlisted, `seed.rs` 1 → 0, `version.rs` 4 → 4 allowlisted | |
| N7 | done — `feedback.rs` 10 → 9 (one later `AnimationFrameCount` follow-up), `skill.rs` 6 → 4 allowlisted, `combat.rs` 2 → 0, `properties.rs` 2 → 1 allowlisted, `spellbook.rs` 3 → 1 allowlisted, `encoded.rs` 2 → 0, `casting.rs` 1 → 0 | |
| N8 | done — `login.rs`'s `StartLocation::position` tuple became `Point`; the repo-level coverage check landed with a full allowlist; `docs/style.md` gained a short section | |
| N-tables | done — the cliloc and `SoundId` content tables: `protocol` took `serde`, and `State`'s four doors take the types | `a78ee4c`, `4d9561d` |
| N-components | done — `Drawn`, `Container.gump`, `Contained.{position, grid}`, and the graphic-keyed tables under them | `6c01d6e` |
| N-commands | done — `Command` 27 bare serials → 0, `trade.rs` 3 → 0 (the module N8's counter could not see), and the system functions under them | |
| N-persistence | done — `CharacterRecord`, `ItemRecord`, `MobileRecord`, `DecorationRecord`, `PetData`, `Inventory`, `QuestRecord` serials typed via `serde(with = ...)`, on-disk shape unchanged, `SCHEMA_VERSION` still 23; `MobileRecord::spawned_by` stays `Option<u32>` (a spawner-list index, not a serial) | `84a59b1` |
| N-gump | done — `GumpLayout`'s three localized-text methods take a `ClilocId`, `quests::gump`'s sixteen constants and `crafting`'s `Text::Cliloc` with them, and `CraftGumpContext::notice`'s zero sentinel became an `Option` | |
| N-drop | done — the `0x08`'s one position field with two meanings became `DropDestination`; `Command::DropItem` carries it, `drop_into_container` takes a `GumpPoint`, `drop_onto_item` became `drop_onto_serial` | |
| N-gate | done — the coverage check now walks enum bodies (15 fields found, all allowlisted in existing classes), reports and asserts what it examined, and has a fixture; a function's parameters stay out of reach and are documented as such | |
