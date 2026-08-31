//! N8 — the sweep's last stage: `docs/protocol_newtypes.md`'s N10 says every
//! bare integer field remaining in the crate is either wrapped or on an
//! explicit, reasoned allowlist, and that the count is asserted, not assumed.
//! This test is the assertion — "no violations found" from a detector that
//! examined nothing has been green here before, so this one records what it
//! looked at and fails loudly if the allowlist and the crate disagree, in
//! either direction.
//!
//! The scan is a plain text walk, not a syntax tree: `syn` is not a
//! dependency anywhere in this workspace, and every existing self-check in
//! this crate (`feature.rs`'s `all_lists_every_feature_exactly_once`,
//! `direction.rs`'s `every_byte_the_client_can_send_names_a_direction`) is an
//! in-process assertion over program data, not a source-file reader — this is
//! the first of that kind, so it stays as small as the job allows.
//!
//! Two shapes a naive `pub name: u16` grep would miss are the reason N7's own
//! backlog forced this stage: a tuple (`StartLocation::position` was a bare
//! `(i32, i32, i32)`, fixed in N8) and a `Vec` of one
//! (`PropertyQueryRequest::serials` was a bare `Vec<u32>`, fixed in N7). Both
//! are gone now, but the *type* of gap is not — a later field could
//! reintroduce either — so the scan matches on whether the type *expression*
//! names a primitive integer anywhere in it, not on the field being one on
//! its own; that is what catches `GumpResponse::text_entries: Vec<(u16,
//! String)>` below without a second rule.
//!
//! A third shape is what N8's own backlog left open and this file now closes:
//! **an enum variant's fields carry no `pub`**, so the struct scan walked past
//! every one of them. `DecodeError::UnknownValue { value: u32 }` and
//! `Text::Cliloc(u32)` are the same bare integer a struct field would be, and
//! the sweep that typed `Command`'s twenty-seven serials found them by reading,
//! not by being told. The enum walk below is a second, separate pass with its
//! own bracket state machine, because the two shapes have nothing in common
//! textually: one is recognised by `pub`, the other by where it sits.
//!
//! What is still out of reach: **a function's parameters**. `fn encode(serial:
//! u32)` is a bare integer in the same sense and no field scan can see it —
//! that one wants `syn`, and the argument against the dependency has not
//! changed. The scan therefore reports what it examined (files, enums,
//! variants) and asserts those counts are non-trivial, so "no violations" can
//! never again mean "nothing was read".

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Every bare-integer field the sweep leaves in place, and why — `(file,
/// field, reason)`. This is what the sweep actually enforces; the table in
/// `docs/protocol_newtypes.md`'s N10 section is the narrative for the same
/// list, and the two must agree by hand. A field appears here once per
/// struct that has it: `vendor.rs`'s `amount` is on three different structs
/// and is on this list three times, one per struct, because the scan below
/// counts occurrences of `(file, field)`, not distinct names.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    // -- geometric components: N1 amendment 2, N2 amendment 2 --------------
    (
        "target.rs",
        "x",
        "MultiOffset: one signed displacement's own east-west axis; the enclosing type keeps \
         it distinct from an absolute Point and keeps all three wire fields together",
    ),
    ("target.rs", "y", "MultiOffset: same"),
    ("target.rs", "z", "MultiOffset: same"),
    (
        "world.rs",
        "x",
        "Point: one geometric quantity's own axis, added to and compared on every step",
    ),
    ("world.rs", "y", "Point: same"),
    ("world.rs", "z", "Point: same"),
    ("world.rs", "width", "MapSize: same argument as Point"),
    ("world.rs", "height", "MapSize: same"),
    (
        "design.rs",
        "dx",
        "DesignTile: a signed tile displacement from a house's origin — the same geometry \
         target.rs's offset is, and the same reason it is not a Point. Narrower than that \
         one at i8, because the wire's stair buffer writes each offset as a single byte and \
         nothing wider could survive the round trip",
    ),
    ("design.rs", "dy", "DesignTile: same"),
    (
        "design.rs",
        "dz",
        "DesignTile: same, and it is compared against the five storey heights to pick a plane",
    ),
    (
        "design.rs",
        "x_min",
        "DesignBounds: the corner the grid planes are indexed from, in the same displacement \
         space as DesignTile's dx — it is subtracted from one and added back to the other",
    ),
    ("design.rs", "y_min", "DesignBounds: same"),
    (
        "gump.rs",
        "x",
        "GumpPoint: the same argument in gump-space pixels, signed for negative layout offsets",
    ),
    ("gump.rs", "y", "GumpPoint: same"),
    (
        "chunks.rs",
        "x",
        "ChunkAt: Point's argument in a third grid — one geometric quantity's own axis, and the \
         pair is what a chunk is addressed by. openshard_map::chunk::ChunkCoord is the same place \
         in the crate that owns the world, and that crate is above this one",
    ),
    ("chunks.rs", "y", "ChunkAt: same"),
    (
        "chunks.rs",
        "wide",
        "FacetBlocks: MapSize's argument, counted in map blocks rather than tiles because that \
         is what openshard_map::chunk::assemble refuses a short set of chunks against",
    ),
    ("chunks.rs", "down", "FacetBlocks: same"),
    // -- current/max bars: N2 amendment 2 -----------------------------------
    (
        "mobile.rs",
        "current",
        "Vitals: half a bar is not a smaller number, it is the wrong ratio",
    ),
    ("mobile.rs", "max", "Vitals: same"),
    // -- status-bar quantities: N2 amendment 3 ------------------------------
    (
        "mobile.rs",
        "strength",
        "MobileStatus: a quantity clamped by [gameplay], not a protocol rule",
    ),
    ("mobile.rs", "dexterity", "MobileStatus: same"),
    ("mobile.rs", "intelligence", "MobileStatus: same"),
    ("mobile.rs", "gold", "MobileStatus: same"),
    ("mobile.rs", "armor", "MobileStatus: same"),
    ("mobile.rs", "weight", "MobileStatus: same"),
    ("mobile.rs", "max_weight", "MobileStatus: same"),
    ("mobile.rs", "stat_cap", "MobileStatus: same"),
    ("mobile.rs", "followers", "MobileStatus: same"),
    ("mobile.rs", "followers_max", "MobileStatus: same"),
    // -- gold: N5 amendment 1 ----------------------------------------------
    (
        "vendor.rs",
        "price",
        "BuyLine: gold, the MobileStatus::gold argument",
    ),
    ("vendor.rs", "price", "SellLine: gold, same as BuyLine::price"),
    // -- craft catalogue presentation values --------------------------------
    (
        "craft.rs",
        "button",
        "CraftCatalogueRow: the normal gump reply id; wrapping it would duplicate the raw button type at this private wire seam",
    ),
    (
        "craft.rs",
        "skill_min",
        "CraftCatalogueRow: displayed primary-skill threshold in the game's tenths-of-a-percent unit",
    ),
    (
        "craft.rs",
        "amount",
        "CraftCatalogueComponent: a stack count consumed by one recipe, not an item serial or identifier",
    ),
    (
        "craft.rs",
        "damage_min",
        "CraftWeaponProperties: lower bound of one displayed damage range; both endpoints belong together",
    ),
    (
        "craft.rs",
        "damage_max",
        "CraftWeaponProperties: same displayed damage range",
    ),
    (
        "craft.rs",
        "speed_centis",
        "CraftWeaponProperties: ML swing duration in the authoritative centisecond unit",
    ),
    (
        "craft.rs",
        "range",
        "CraftWeaponProperties: optional tile count for a ranged weapon, zero remains the wire sentinel for melee",
    ),
    (
        "craft.rs",
        "skill",
        "CraftSkillRequirement: openshard_state::Skill lives above protocol; the generated catalogue validates the id",
    ),
    (
        "craft.rs",
        "minimum",
        "CraftSkillRequirement: a displayed skill quantity in tenths of a percent",
    ),
    (
        "craft.rs",
        "needs",
        "CraftCatalogueDefinitionRow: generated facility-presence bitmask; the facility domain lives in crafting above protocol",
    ),
    (
        "craft.rs",
        "request_id",
        "CraftCatalogue: connection-local correlation token chosen by the client and compared opaquely",
    ),
    (
        "craft.rs",
        "catalogue_revision",
        "CraftCatalogue: generated content hash compared opaquely by both ends",
    ),
    (
        "craft.rs",
        "craft_projection_revision",
        "CraftCatalogue: server-computed stock generation used only as an opaque cache diagnostic",
    ),
    (
        "craft.rs",
        "backpack_revision",
        "CraftCatalogue: same opaque stock-generation argument",
    ),
    (
        "craft.rs",
        "facilities",
        "CraftCatalogue: facility-presence bitmask whose domain lives in crafting above protocol",
    ),
    (
        "craft.rs",
        "skills",
        "CraftCatalogue: pairs of state-owned skill ids and displayed tenths-of-a-percent quantities",
    ),
    (
        "craft.rs",
        "amounts",
        "CraftCatalogue: dense CraftKey-indexed stock quantities, not identities",
    ),
    (
        "craft.rs",
        "amount",
        "CraftWorkbenchComponent: a displayed result/input stack quantity",
    ),
    (
        "craft.rs",
        "carried",
        "CraftWorkbenchComponent: optional displayed inventory quantity",
    ),
    (
        "craft.rs",
        "button",
        "CraftWorkbenchGroup: the normal gump reply id at this private presentation seam",
    ),
    (
        "craft.rs",
        "button",
        "CraftWorkbenchMaterial: the normal gump reply id at this private presentation seam",
    ),
    (
        "craft.rs",
        "carried",
        "CraftWorkbenchMaterial: displayed inventory quantity",
    ),
    (
        "craft.rs",
        "make_button",
        "CraftWorkbenchRecipe: optional gump reply id at this private presentation seam",
    ),
    (
        "craft.rs",
        "details_button",
        "CraftWorkbenchRecipe: optional gump reply id at this private presentation seam",
    ),
    (
        "craft.rs",
        "skills",
        "CraftWorkbenchRecipe: displayed skill thresholds in tenths of a percent",
    ),
    (
        "craft.rs",
        "Details.success_per_mille",
        "CraftWorkbenchPage: displayed probability quantity in per-mille units",
    ),
    (
        "craft.rs",
        "Details.exceptional_per_mille",
        "CraftWorkbenchPage: optional displayed probability quantity in per-mille units",
    ),
    (
        "craft.rs",
        "tool_uses",
        "CraftWorkbench: displayed remaining-use quantity",
    ),
    (
        "craft.rs",
        "required_facilities",
        "CraftWorkbench: facility-presence bitmask whose domain lives in crafting above protocol",
    ),
    (
        "craft.rs",
        "present_facilities",
        "CraftWorkbench: same generated facility-presence bitmask",
    ),
    (
        "craft.rs",
        "materials_button",
        "CraftWorkbench: optional gump reply id at this private presentation seam",
    ),
    (
        "craft.rs",
        "refresh_button",
        "CraftWorkbench: gump reply id at this private presentation seam",
    ),
    (
        "craft.rs",
        "cancel_button",
        "CraftWorkbench: gump reply id at this private presentation seam",
    ),
    // -- bounded read-only house inventory ---------------------------------
    (
        "house_inventory.rs",
        "aggregate_total",
        "HouseInventoryRow: permission-filtered item quantity across roots",
    ),
    (
        "house_inventory.rs",
        "root_total",
        "HouseInventoryRow: item quantity in one root",
    ),
    (
        "house_inventory.rs",
        "pile_count",
        "HouseInventoryRow: diagnostic count of piles in one root",
    ),
    (
        "house_inventory.rs",
        "Search.expected_epoch",
        "HouseInventoryRequest: optional projection generation compared opaquely for pagination continuity",
    ),
    (
        "house_inventory.rs",
        "Search.limit",
        "HouseInventoryRequest: page-size quantity validated against MAX_HOUSE_INVENTORY_PAGE while decoding",
    ),
    (
        "house_inventory.rs",
        "Resolve.epoch",
        "HouseInventoryRequest: projection generation compared opaquely before canonical revalidation",
    ),
    (
        "house_inventory.rs",
        "Page.epoch",
        "HouseInventoryReply: server projection generation echoed for the next bounded request",
    ),
    (
        "house_inventory.rs",
        "Resolved.epoch",
        "HouseInventoryReply: same server projection generation",
    ),
    (
        "house_inventory.rs",
        "Refused.current_epoch",
        "HouseInventoryReply: same generation returned to replace a stale client token",
    ),
    (
        "item_kind.rs",
        "SameAsInput.0",
        "MaterialRule: build-validated index into one recipe's bounded resource lines",
    ),
    // -- login's own quantities, and a type that is not a packet struct -----
    // N6 amendments 7/8
    // `percent_full` was on this list and is not any more: 100 is a ceiling
    // the client imposes, so it became `PercentFull` with the clamp in its
    // constructor. `timezone` has no such rule and stays.
    (
        "login.rs",
        "timezone",
        "ShardEntry: a quantity, the MobileStatus argument",
    ),
    (
        "version.rs",
        "major",
        "ClientVersion is not a packet struct; narrowed from a seed dword or a 0xBD string",
    ),
    ("version.rs", "minor", "ClientVersion: same"),
    ("version.rs", "revision", "ClientVersion: same"),
    ("version.rs", "patch", "ClientVersion: same"),
    // -- animation/effect numbers whose domain lives above protocol ---------
    // module doc in feedback.rs
    (
        "feedback.rs",
        "action",
        "Animation: body-specific index; openshard_state::Action lives above protocol",
    ),
    ("feedback.rs", "repeat_count", "Animation: same"),
    ("feedback.rs", "delay", "Animation: same"),
    (
        "feedback.rs",
        "animation_type",
        "NewAnimation: category index; domain above protocol, same as Animation::action",
    ),
    ("feedback.rs", "action", "NewAnimation: same"),
    ("feedback.rs", "delay", "NewAnimation: a quantity"),
    (
        "feedback.rs",
        "speed",
        "GraphicalEffect: a quantity, a per-effect literal",
    ),
    ("feedback.rs", "duration", "GraphicalEffect: same"),
    (
        "feedback.rs",
        "action",
        "HarvestPreview: body-specific animation index; openshard_state::Action lives above protocol",
    ),
    (
        "feedback.rs",
        "cycles",
        "HarvestPreview: a presentation-only count, with no protocol-level domain",
    ),
    (
        "feedback.rs",
        "render_mode",
        "HuedEffect: no non-test caller constructs one, nothing to classify against",
    ),
    // -- weather presentation quantities -----------------------------------
    (
        "world.rs",
        "intensity",
        "WeatherChange: the classic client's particle-count byte, a presentation quantity",
    ),
    (
        "world.rs",
        "temperature",
        "WeatherChange: a client presentation byte; weather rules live above protocol",
    ),
    // -- skill numbers whose domain lives above protocol: N7 amendment ------
    (
        "skill.rs",
        "id",
        "SkillEntry: openshard_state::Skill lives above protocol, same argument as feedback.rs",
    ),
    ("skill.rs", "value", "SkillEntry: a quantity"),
    ("skill.rs", "base", "SkillEntry: same"),
    ("skill.rs", "cap", "SkillEntry: same"),
    // -- one spell school ever wired, and its bitmask: N7 amendment 8, N8 ---
    (
        "spellbook.rs",
        "offset",
        "SpellbookContent: nothing branches on the byte while no second school exists",
    ),
    (
        "spellbook.rs",
        "content",
        "SpellbookContent: a membership bitmask over spell ids; which ids exist is Community \
         Pack content, the feedback.rs argument again",
    ),
    // -- computed by the server, read only by the client: N7 amendment 6 ----
    (
        "properties.rs",
        "hash",
        "TooltipRevision: server-computed, client-only reader; none of N3's four classes fit",
    ),
    (
        "properties.rs",
        "hash",
        "PropertyListReply: the same accumulator arriving on the other side — the client only \
         ever compares it against the one a TooltipRevision carried, and never recomputes it, so \
         a type that promised meaning would be promising more than the value has",
    ),
    // -- text field ids the pack script owns, above the engine: N5 ----------
    (
        "gump.rs",
        "text_entries",
        "GumpResponse: a Vec of (id, text); which id a pack drew is the pack's business",
    ),
    // -- diagnostic fields on typed errors, not wire data: N8 ---------------
    (
        "error.rs",
        "expected",
        "WrongPacket: the id the decoder wanted, chosen by the dispatcher, not client input",
    ),
    (
        "error.rs",
        "found",
        "WrongPacket: the packet's own header id, already matched before this fires",
    ),
    (
        "gump.rs",
        "id",
        "InvalidSwitchId: the rejected value, carried for Display, not a second wire field",
    ),
    (
        "context.rs",
        "tag",
        "InvalidContextMenuIndex: same — the rejected value, carried for Display",
    ),
    ("wire.rs", "slot", "InvalidCharacterSlot: same"),
    // -- enum variants, first scanned here; keyed `Variant.field` -------------
    //
    // Every one of the fifteen the enum walk turned up falls in a class the
    // sweep had already argued out on a struct field. That is the finding, and
    // it is worth writing down: the blind spot hid no untyped wire value, it
    // hid the *evidence* that nothing was hiding.
    //
    // Class 1 — the leftover arm of a `Raw*::interpret`. The byte is carried
    // through precisely because this engine has no name for it; wrapping it in
    // a type would assert a meaning the arm exists to deny.
    (
        "speech.rs",
        "Other.0",
        "TalkMode: interpret's leftover arm, the unnamed mode as the byte it was",
    ),
    (
        "encoded.rs",
        "Other.0",
        "EncodedSubcommand: the same leftover arm, a word this engine does not name",
    ),
    (
        "extended.rs",
        "Unknown.0",
        "ExtendedRequest: the same, a 0xBF subcommand with no handler",
    ),
    (
        "party.rs",
        "Unknown.0",
        "PartyRequest: the same again, one level down — a party sub-subcommand with no handler, \
         which is a byte inside 0xBF 0x0006's body rather than a subcommand of its own",
    ),
    (
        "login.rs",
        "Unknown.0",
        "ClientLoginPacket: the same, an id the login conversation does not act on",
    ),
    (
        "world.rs",
        "Predefined.0",
        "Profession: which template a non-zero id names is Community Pack content, not this crate's",
    ),
    (
        "client_packet.rs",
        "Unknown.id",
        "ClientPacket: an id with no handler, logged as a fact — the leftover arm again",
    ),
    (
        "client_packet.rs",
        "Unknown.body",
        "ClientPacket: the undecoded packet's own bytes; Vec<u8> is a buffer, not a number, and \
         the scan's deliberately broad type match cannot tell the two apart",
    ),
    (
        "chunks.rs",
        "blob",
        "ChunkData: one fragment of a deflated chunk record — the same buffer-not-a-number as \
         ClientPacket::Unknown.body, and this crate never looks inside it",
    ),
    // Class 2 — a diagnostic field on a typed error, the `WrongPacket::expected`
    // argument: the value is carried for `Display` after it was already
    // rejected, and is never read as wire data again.
    (
        "error.rs",
        "UnknownValue.value",
        "DecodeError: the rejected value, carried for Display — WrongPacket::expected's argument",
    ),
    (
        "error.rs",
        "Unsupported.packet",
        "DecodeError: the id whose form is not decoded, for the log line",
    ),
    (
        "login.rs",
        "PastEnd.index",
        "InvalidShardIndex: the out-of-range index, carried for Display",
    ),
    (
        "packet.rs",
        "BadLength.id",
        "FrameError: same — the id that framed wrong, for the log line",
    ),
    (
        "packet.rs",
        "UnknownPacket.0",
        "FrameError: same — the id this server cannot size",
    ),
    (
        "chunks.rs",
        "Incomplete.wanted",
        "JoinError: how many fragments the set said there were, carried for Display after the \
         set was already rejected — WrongPacket::expected's argument",
    ),
    // Class 3 — quantities, on the MobileStatus argument.
    (
        "packet.rs",
        "Fixed.0",
        "PacketLength: a byte count, arithmetic on it is the point",
    ),
    (
        "trade.rs",
        "UpdateGold.gold",
        "SecureTradeAction: gold, the MobileStatus::gold argument; decoded and ignored besides",
    ),
    (
        "trade.rs",
        "UpdateGold.platinum",
        "SecureTradeAction: same currency, same argument",
    ),
];

/// Whether a field's type expression names a primitive wire integer anywhere
/// in it — `u16`, `(i32, i32, i32)`, `Vec<u32>` and `Vec<(u16, String)>` all
/// do; `Point`, `Serial`, `Vec<RawSerial>` do not. Matching the whole type
/// text rather than requiring the field's own type to *be* one of these
/// tokens is what closes N7's tuple and collection blind spots without a
/// second rule for each.
fn names_a_primitive_int(ty: &str) -> bool {
    let mut token = String::new();
    let mut hit = false;
    for c in ty.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            token.push(c);
            continue;
        }
        if matches!(
            token.as_str(),
            "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64"
        ) {
            hit = true;
        }
        token.clear();
    }
    hit
}

/// Every `pub name: <type containing a primitive int>` field declaration in
/// one source file, as `(field name, type text)`.
///
/// A field declaration is recognised as "`pub `, then an identifier with no
/// space or `(` in it, then `:`" — which a struct field satisfies and a
/// method (`pub fn foo(x: u16)`), an associated const (`pub const ID: u8`) or
/// a tuple struct's own definition (`pub struct RawSerial(pub u32);`, no
/// colon at all) do not, so none of those needs a separate exclusion.
fn scan_file(text: &str) -> Vec<(String, String)> {
    let mut hits = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("pub ") else {
            continue;
        };
        let Some(colon) = rest.find(':') else {
            continue;
        };
        let name = &rest[..colon];
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let ty = rest[colon + 1..].trim().trim_end_matches(',').trim();
        if names_a_primitive_int(ty) {
            hits.push((name.to_owned(), ty.to_owned()));
        }
    }
    hits
}

/// What one file's enum walk saw. The counts are not decoration: a scan that
/// reports zero violations having examined zero variants is the failure mode
/// this whole file exists to prevent, so the caller asserts on them.
struct EnumScan {
    /// Bare-integer variant fields, as `(Variant.field, type text)` for a
    /// struct-like variant and `(Variant.0, type text)` for a tuple one. The
    /// variant name is part of the key because a variant's field carries no
    /// `pub` and would otherwise be indistinguishable from a struct's.
    hits:     Vec<(String, String)>,
    /// How many enum bodies the walk entered.
    enums:    usize,
    /// How many variants it read inside them.
    variants: usize,
}

/// Split a tuple variant's parenthesised body on its top-level commas —
/// `Vec<(u16, String)>, u32` is two elements, not three. Angle brackets and
/// nested parentheses both nest; a `->` in a fn-pointer type would confuse the
/// angle-bracket counter, and no variant in this crate has one.
fn split_top_level(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in inner.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(current.trim().to_owned());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    let last = current.trim();
    if !last.is_empty() {
        parts.push(last.to_owned());
    }
    parts
}

/// Every bare-integer field declared inside an `enum` body in one source file.
///
/// The walk is a brace counter that runs only between `enum Name {` and the
/// brace that closes it, so the string literals and `impl` blocks elsewhere in
/// the file — which a whole-file brace counter would trip over — are never
/// counted. Doc comments are skipped before counting, which is what keeps
/// `gump.rs`'s `{ resizepic … }` layout examples out of the depth.
///
/// At depth 1 a line is a variant: `Name {` opens a struct-like one, `Name(..)`
/// is a tuple one read on the spot, anything else is a unit variant or a
/// discriminant. At depth 2 a line is a struct-like variant's field, which
/// looks exactly like a struct field with the `pub` removed.
///
/// Panics if an enum body does not close before the end of the file: that means
/// the counter lost the depth, and a silently truncated scan is the one outcome
/// worse than no scan.
fn scan_enums(file_name: &str, text: &str) -> EnumScan {
    let mut scan = EnumScan {
        hits:     Vec::new(),
        enums:    0,
        variants: 0,
    };
    let mut depth = 0i32;
    let mut variant: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("#[") {
            continue;
        }

        if depth == 0 {
            // `pub enum Foo {`, `enum Foo {`, `pub enum Foo<T> {` — the body
            // always opens on the same line under this repo's rustfmt.
            let head = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
            if head.starts_with("enum ") && trimmed.ends_with('{') {
                scan.enums += 1;
                depth = 1;
            }
            continue;
        }

        if depth == 1 && !trimmed.starts_with('}') {
            if let Some(open) = trimmed.find('(') {
                // A tuple variant, read whole: `Cliloc(u32),`.
                let name = trimmed[..open].trim();
                if let Some(close) = trimmed.rfind(')') {
                    scan.variants += 1;
                    for (index, element) in split_top_level(&trimmed[open + 1..close]).iter().enumerate() {
                        if names_a_primitive_int(element) {
                            scan.hits.push((format!("{name}.{index}"), element.clone()));
                        }
                    }
                }
            } else if trimmed.ends_with('{') {
                scan.variants += 1;
                variant = Some(trimmed.trim_end_matches('{').trim().to_owned());
            } else if !trimmed.is_empty() {
                scan.variants += 1;
            }
        } else if depth == 2 {
            if let (Some(name), Some(colon)) = (variant.as_deref(), trimmed.find(':')) {
                let field = &trimmed[..colon];
                if !field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    let ty = trimmed[colon + 1..].trim().trim_end_matches(',').trim();
                    if names_a_primitive_int(ty) {
                        scan.hits.push((format!("{name}.{field}"), ty.to_owned()));
                    }
                }
            }
        }

        depth += i32::try_from(trimmed.matches('{').count()).unwrap()
            - i32::try_from(trimmed.matches('}').count()).unwrap();
        if depth <= 1 {
            variant = None;
        }
    }

    assert_eq!(
        depth, 0,
        "the enum walk left {file_name} at brace depth {depth} — it lost track of a body, and \
         everything after that point went unscanned",
    );
    scan
}

/// `a` minus `b` as multisets, keyed by `(file, field)` — what is in `a` more
/// times than in `b`. Called both ways: found-minus-allowed is a violation to
/// fix, allowed-minus-found is a stale entry the field's own fix should have
/// deleted.
fn multiset_extra(a: &[(String, String)], b: &[(String, String)]) -> Vec<(String, String)> {
    let mut remaining: HashMap<(String, String), i32> = HashMap::new();
    for key in b {
        *remaining.entry(key.clone()).or_insert(0) += 1;
    }
    let mut extra = Vec::new();
    for key in a {
        let count = remaining.entry(key.clone()).or_insert(0);
        if *count > 0 {
            *count -= 1;
        } else {
            extra.push(key.clone());
        }
    }
    extra.sort();
    extra
}

/// The enum walk finds a planted field, and is not fooled by the shapes that
/// sit next to one.
///
/// A detector is only worth its green when it has been shown to go red, and
/// this one runs against source text it does not own — the fixture is the only
/// place its behaviour can be pinned. Every line below is a shape that appears
/// in `src/`: a unit variant, a discriminant, a doc comment with braces in it
/// (`gump.rs`'s layout examples), a nested type in a tuple, and an `impl` after
/// the body, which must not be counted as part of it.
#[test]
fn the_enum_walk_sees_what_it_claims_to_see() {
    let source = r#"
/// A mode, with a layout example: `{ resizepic 0 0 }{ button 5 5 }`.
pub enum Fixture {
    /// A unit variant.
    Quiet,
    /// A discriminant.
    Loud = 0x09,
    /// A tuple variant with one bare byte and one type.
    Raw(u8, Serial),
    /// A tuple variant that is fully typed.
    Typed(Serial),
    /// A struct variant, half typed.
    Both {
        /// Bare.
        count: u16,
        /// Not bare.
        who: Serial,
        /// Bare, inside a collection.
        pairs: Vec<(u32, String)>,
    },
}

impl Fixture {
    /// A method whose parameter is bare — out of reach, and it must not be
    /// mistaken for a variant field.
    pub fn of(id: u16) -> Self {
        Self::Raw(id as u8, Serial::new(1))
    }
}
"#;

    let scan = scan_enums("fixture.rs", source);
    assert_eq!(scan.enums, 1, "one enum body");
    assert_eq!(scan.variants, 5, "five variants");

    let found: Vec<&str> = scan.hits.iter().map(|(field, _ty)| field.as_str()).collect();
    assert_eq!(
        found,
        vec!["Raw.0", "Both.count", "Both.pairs"],
        "the walk must find the bare tuple element and both bare struct-variant \
         fields, and must skip the typed ones, the unit variants, the doc \
         comment's braces and the method parameter",
    );
}

#[test]
fn every_bare_integer_field_is_wrapped_or_allowlisted() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut entries: Vec<_> = fs::read_dir(&src_dir)
        .expect("protocol crate must have a src directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    entries.sort();
    assert!(
        entries.len() > 20,
        "found only {} .rs files under {src_dir:?} — the scan is reading the wrong directory",
        entries.len(),
    );

    let mut found: Vec<(String, String)> = Vec::new();
    let mut enums = 0;
    let mut variants = 0;
    for path in &entries {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let text = fs::read_to_string(path).expect("protocol source file must be readable");
        for (field, _ty) in scan_file(&text) {
            found.push((file_name.clone(), field));
        }
        let scan = scan_enums(&file_name, &text);
        enums += scan.enums;
        variants += scan.variants;
        for (field, _ty) in scan.hits {
            found.push((file_name.clone(), field));
        }
    }

    // What the scan examined, asserted rather than assumed. These are floors a
    // long way below the real numbers, so they do not need editing when a
    // packet lands; they fire when the walk stops walking — a changed brace
    // convention, a moved directory, a file the reader silently skipped.
    assert!(
        enums > 25 && variants > 100,
        "the enum walk examined {enums} enums and {variants} variants — far too few for this \
         crate, so the walk is broken and any \"no violations\" it reports is meaningless",
    );

    let allowed: Vec<(String, String)> = ALLOWLIST
        .iter()
        .map(|(file, field, _reason)| ((*file).to_owned(), (*field).to_owned()))
        .collect();

    let unallowed = multiset_extra(&found, &allowed);
    let stale = multiset_extra(&allowed, &found);

    assert!(
        unallowed.is_empty() && stale.is_empty(),
        "N10's coverage check disagrees with the allowlist in `tests/bare_integer_fields.rs`.\n\
         \n\
         Bare integer fields found in `src/` but NOT on the allowlist — give the field a real \
         type, or add a reasoned entry:\n{unallowed:#?}\n\
         \n\
         Allowlist entries with no matching field left in `src/` — the field was fixed and the \
         entry should have been deleted with it:\n{stale:#?}",
    );
}
