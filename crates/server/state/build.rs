//! Turns `data/*.json` into the lookup tables this crate keeps in `const`s.
//!
//! Every one is ported reference data — ServUO's `Data/bodyTable.cfg`, its
//! `BaseMount` subclasses, `SkillInfo.Table`, the names and `BaseSoundID`s off
//! its `BaseCreature`s, and the tile sets its harvest definitions scan. Between
//! them they were 1,799 lines of Rust that no one has ever read as code: 469
//! body ids one per line, 58 skills of thirteen columns apiece, 271 lines of
//! `match` arms keyed by body, and a hundred lines of bare tile ids. They are
//! 557 lines of JSON now.
//!
//! Three shapes come out of here, and which one a table gets is not a style
//! choice — it is whatever shape the caller already read:
//!
//! - **A `const` slice**, for the tables that are searched. `body_type` and
//!   `mount_item_for` binary-search theirs on the tick path; `SKILLS` is indexed
//!   by a `Skill` discriminant.
//! - **A `const fn` over a `match`**, for `creature_name` and
//!   `creature_base_sound`. The compiler turns a dense integer `match` into a
//!   jump, and a search over a slice could not be `const fn` at all — so the
//!   generated code keeps the shape the hand-written code had.
//! - **A constructor returning owned values**, for `quest::shipped` and
//!   `dialogue::shipped`. Their destinations are not tables read where they lie:
//!   `QuestDefs::set` and `Dialogue::set_tables` take ownership and replace
//!   everything before them, so a `const` here would be a second spelling of the
//!   same types, cloned once at the only call site. The build-time checks are the
//!   part that carries over, and they are the part that mattered.
//!
//! **Invariants are this script's job, not the data's.** It sorts what is
//! binary-searched, because a table sorted by hand decays the first time
//! somebody appends a row; it rejects a duplicate id, which a binary search
//! would answer arbitrarily and a `match` would answer with whichever arm came
//! first. The doc comments for the generated items live here rather than in the
//! JSON — a data file is a poor place for prose, and this is the file that
//! decides what the item means.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

/// The doc over the generated `BODY_TYPES`.
const BODY_TYPES_DOC: &str = "\
/// Every body ServUO's `Data/bodyTable.cfg` gives a type, sorted by id.
///
/// `Equipment` entries are dropped: they are item art, never a mobile. What is left
/// is what a creature can be.
///
/// The source is `data/body_types.json`, which groups the ids by type because
/// that is how a person reads them; the sort into id order happens here, in
/// `build.rs`, because that is what [`body_type`]'s binary search needs and a
/// hand-sorted table decays the first time a row is appended.";

/// The doc over the generated `MOUNTS`.
const MOUNTS_DOC: &str = "\
/// The mount-item graphic each rideable body is drawn as, sorted by body id.
///
/// Ported from ServUO's `BaseMount` subclasses — the `base(name, bodyID, itemID, …)`
/// each one passes, plus the alternating body/item arrays a class that rolls between
/// several looks keeps (`Horse` is one of four).
///
/// The source is `data/mounts.json`. Both directions of the mapping are derived
/// from this one table: two hand-kept halves is how a saved ride comes back as
/// the wrong animal.";

/// The doc over the generated `SKILLS`.
const SKILLS_DOC: &str = "\
/// ServUO's `SkillInfo.Table`, verbatim, indexed by skill id.
///
/// The source is `data/skills.json`. The length is checked by the type: a row
/// added there without a matching [`Skill`] variant will not compile.";

/// One row of `data/skills.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillRow {
    /// What it is called: "Alchemy", "Item Identification".
    name: String,
    /// The title a grandmaster earns.
    title: String,
    /// The stat the ML gain mechanic tries first — a `StatCode` variant's name.
    primary: String,
    /// The stat it falls back to.
    secondary: String,
    /// How much strength lends to the effective value, in hundredths.
    str_scale: u32,
    /// How much dexterity lends.
    dex_scale: u32,
    /// How much intelligence lends.
    int_scale: u32,
    /// The ceiling on the whole stat bonus. ServUO sums the *undivided* scales
    /// here; see `SkillInfo::stat_total` for why that is not a slip.
    stat_total: u32,
    /// The chance weight that training nudges strength, in thousandths.
    str_gain: u32,
    /// The same for dexterity.
    dex_gain: u32,
    /// The same for intelligence.
    int_gain: u32,
    /// A multiplier on how readily the skill trains, in per-mille.
    gain_factor: u32,
    /// Whether the skill can be used straight from the window's button. False on
    /// thirty-five of the fifty-eight, so it is left out of the data there.
    #[serde(default)]
    usable: bool,
    /// Whether it may be used with a spell in flight. Spirit Speak alone.
    #[serde(default)]
    use_while_casting: bool,
}

/// A hex id from the data. Parsed to sort by, and re-emitted verbatim so the
/// generated source reads the way the table does.
fn id(raw: &str) -> u16 {
    let digits = raw
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("{raw} is not 0x-prefixed"));
    u16::from_str_radix(digits, 16).unwrap_or_else(|e| panic!("{raw} is not a u16 ({e})"))
}

/// `data/body_types.json`, grouped by type name, into a table sorted by id.
fn body_types(text: &str) -> String {
    let grouped: BTreeMap<String, Vec<String>> = serde_json::from_str(text).expect("body_types.json");

    // Sorted here rather than in the data, and checked for the duplicate that a
    // binary search would answer arbitrarily.
    let mut rows: Vec<(u16, String, String)> = grouped
        .iter()
        .flat_map(|(kind, ids)| ids.iter().map(move |raw| (id(raw), raw.clone(), kind.clone())))
        .collect();
    rows.sort_by_key(|(id, _, _)| *id);
    for pair in rows.windows(2) {
        assert_ne!(
            pair[0].0, pair[1].0,
            "body {:#06x} is listed as both {} and {}",
            pair[0].0, pair[0].2, pair[1].2
        );
    }

    let mut out = String::from("// @generated by build.rs from data/body_types.json.\n\n");
    out.push_str(BODY_TYPES_DOC);
    out.push_str("\nconst BODY_TYPES: &[(u16, BodyType)] = &[\n");
    for (_, raw, kind) in &rows {
        writeln!(out, "    ({raw}, BodyType::{kind}),").unwrap();
    }
    out.push_str("];\n");
    out
}

/// `data/mounts.json`, a body/item pair per row, into a table sorted by body.
fn mounts(text: &str) -> String {
    let pairs: Vec<(String, String)> = serde_json::from_str(text).expect("mounts.json");

    let mut rows: Vec<(u16, String, String)> = pairs
        .into_iter()
        .map(|(body, item)| (id(&body), body, item))
        .collect();
    rows.sort_by_key(|(body, _, _)| *body);
    for pair in rows.windows(2) {
        assert_ne!(pair[0].0, pair[1].0, "body {:#06x} is ridden twice", pair[0].0);
    }

    let mut out = String::from("// @generated by build.rs from data/mounts.json.\n\n");
    out.push_str(MOUNTS_DOC);
    out.push_str("\nconst MOUNTS: &[(u16, u16)] = &[\n");
    for (_, body, item) in &rows {
        let _ = id(item);
        writeln!(out, "    ({body}, {item}),").unwrap();
    }
    out.push_str("];\n");
    out
}

/// `data/skills.json`, in the order the client numbers the skills.
fn skills(text: &str) -> String {
    let rows: Vec<SkillRow> = serde_json::from_str(text).expect("skills.json");

    let mut out = String::from("// @generated by build.rs from data/skills.json.\n\n");
    out.push_str(SKILLS_DOC);
    out.push_str("\npub const SKILLS: [SkillInfo; SKILL_COUNT] = [\n");
    for row in &rows {
        writeln!(out, "    SkillInfo {{").unwrap();
        writeln!(out, "        name: {:?},", row.name).unwrap();
        writeln!(out, "        title: {:?},", row.title).unwrap();
        writeln!(out, "        str_scale: {},", row.str_scale).unwrap();
        writeln!(out, "        dex_scale: {},", row.dex_scale).unwrap();
        writeln!(out, "        int_scale: {},", row.int_scale).unwrap();
        writeln!(out, "        stat_total: {},", row.stat_total).unwrap();
        writeln!(out, "        str_gain: {},", row.str_gain).unwrap();
        writeln!(out, "        dex_gain: {},", row.dex_gain).unwrap();
        writeln!(out, "        int_gain: {},", row.int_gain).unwrap();
        writeln!(out, "        gain_factor: {},", row.gain_factor).unwrap();
        writeln!(out, "        primary: StatCode::{},", row.primary).unwrap();
        writeln!(out, "        secondary: StatCode::{},", row.secondary).unwrap();
        writeln!(out, "        usable: {},", row.usable).unwrap();
        writeln!(out, "        use_while_casting: {},", row.use_while_casting).unwrap();
        out.push_str("    },\n");
    }
    out.push_str("];\n");
    out
}

/// One `group:` of the creature files — a heading and the rows under it. The
/// heading becomes the comment it already was, so the generated `match` reads
/// the way the hand-written one did.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatureGroup {
    /// The heading: "Farm and forest animals", "Undead".
    group: String,
    /// The rows under it, in the order they are matched.
    rows: Vec<CreatureRow>,
}

/// One row: the bodies that share a value, and the value.
///
/// `ids` is a list because several bodies are the same creature — four horse
/// bodies, two cow bodies — and the arm they generate is the `|` pattern that
/// was there before. Which bodies share a row differs between the two files:
/// the dire, grey and timber wolves have three names and one howl.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatureRow {
    /// The body ids this row answers for.
    ids: Vec<String>,
    /// The default name, in `creature_names.json`.
    #[serde(default)]
    name: Option<String>,
    /// The base sound id, in `creature_sounds.json`.
    #[serde(default)]
    sound: Option<String>,
    /// What the sound belongs to, kept as the trailing comment it was: a sound
    /// id says nothing on its own, and `0x00E5` is a wolf only if it says so.
    #[serde(default)]
    note: Option<String>,
}

/// The doc over the generated `creature_name`.
const CREATURE_NAME_DOC: &str = "\
/// The default name a creature's body gives it — \"a chicken\", \"a horse\" —
/// shown on single-click and in the tooltip when a spawn did not name it.
///
/// Creature names are not in any client file the way item names are (those come
/// from tiledata); every emulator holds its own table, ServUO on each
/// `BaseCreature`, Sphere in its chardefs. This is the core default that pack
/// data overrides — the same \"default in core, customise in pack\" split item
/// names and spells have — so the common Britannia wildlife and dungeon monsters
/// read right out of the box and an unlisted body simply stays nameless rather
/// than wearing a wrong label. Body ids are ServUO's.
///
/// The table is `data/creature_names.json`. Expand it there.";

/// The doc over the generated `creature_base_sound`.
const CREATURE_SOUND_DOC: &str = "\
/// A creature's base sound id — ServUO's `BaseSoundID`, keyed by body like
/// [`creature_name`]. Its attack, hurt and death sounds are fixed offsets from
/// it (`+2`, `+3`, `+4`), so an orc growls and a wolf howls instead of every
/// mobile making the human punch sound. `None` for a human body (which uses the
/// gendered death sounds) and for the passive fauna ServUO leaves silent (a
/// rabbit, a deer).
///
/// The table is `data/creature_sounds.json`. Grow it alongside
/// `data/creature_names.json` as bodies are added — the two are keyed the same
/// and neither is complete.";

/// A `match` over body ids, generated from one of the two creature files.
///
/// The shape is deliberately the one that was written by hand: a `match` rather
/// than a sorted slice and a binary search, because the compiler turns a dense
/// integer `match` into a jump and because both functions are `const fn`, which
/// a search over a slice could not be.
fn creatures(text: &str, file: &str, doc: &str, signature: &str, open: &str, close: &str) -> String {
    let groups: Vec<CreatureGroup> = serde_json::from_str(text).unwrap_or_else(|e| panic!("{file}: {e}"));

    let mut out = format!("// @generated by build.rs from data/{file}.\n\n");
    out.push_str(doc);
    out.push('\n');
    out.push_str("#[must_use]\n");
    writeln!(out, "{signature} {{").unwrap();
    writeln!(out, "    {open}").unwrap();

    let mut seen: BTreeMap<u16, String> = BTreeMap::new();
    for group in &groups {
        writeln!(out, "        // {}.", group.group).unwrap();
        for row in &group.rows {
            assert!(!row.ids.is_empty(), "{file}: a row with no body ids");
            // An id in two arms is unreachable in the second — the `match` would
            // compile and quietly answer with the first, which is how a creature
            // ends up wearing another one's name.
            for raw in &row.ids {
                let parsed = id(raw);
                if let Some(first) = seen.insert(parsed, group.group.clone()) {
                    panic!(
                        "{file}: body {raw} appears twice, under {first} and {}",
                        group.group
                    );
                }
            }
            let pattern = row.ids.join(" | ");
            let value = match (&row.name, &row.sound) {
                (Some(name), None) => format!("{name:?}"),
                (None, Some(sound)) => {
                    let _ = id(sound);
                    sound.clone()
                }
                _ => panic!("{file}: a row must carry exactly one of `name` and `sound`"),
            };
            match &row.note {
                Some(note) => writeln!(out, "        {pattern} => {value}, // {note}").unwrap(),
                None => writeln!(out, "        {pattern} => {value},").unwrap(),
            }
        }
    }

    out.push_str("        _ => return None,\n");
    writeln!(out, "    {close}").unwrap();
    out.push_str("}\n");
    out
}

/// `data/harvest_tiles.json` — the four tile sets a harvest definition scans for.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HarvestTiles {
    /// ServUO's `Mining.m_MountainAndCaveTiles`, land ids and Ter Mur statics.
    mountain_and_cave: Vec<u16>,
    /// ServUO's sand tiles.
    sand: Vec<u16>,
    /// ServUO's `Lumberjacking.m_TreeTiles` — all statics.
    tree: Vec<String>,
    /// Water, as inclusive `(from, to)` ranges rather than every id.
    water: Vec<(String, String)>,
}

/// The four tile tables, emitted the way they were written: mining's and sand's
/// as decimal (they are land ids, and ServUO lists them decimal), the tree and
/// water statics as hex.
///
/// Nothing here is sorted or deduplicated. These are `contains` tables, so
/// neither would change an answer, and the order is ServUO's — keeping it is
/// what lets the two be diffed against each other.
fn harvest_tiles(text: &str) -> String {
    let tiles: HarvestTiles = serde_json::from_str(text).expect("harvest_tiles.json");

    let mut out = String::from("// @generated by build.rs from data/harvest_tiles.json.\n\n");

    for (ident, doc, values) in [
        (
            "MOUNTAIN_AND_CAVE_TILES",
            "ServUO's `Mining.m_MountainAndCaveTiles`, verbatim. Land ids below 0x4000, and\n\
             /// the Ter Mur cave statics above it.",
            &tiles.mountain_and_cave,
        ),
        ("SAND_TILES", "ServUO's sand tiles, verbatim.", &tiles.sand),
    ] {
        writeln!(out, "/// {doc}").unwrap();
        writeln!(out, "static {ident}: &[HarvestTile] = &[").unwrap();
        for chunk in values.chunks(10) {
            let row: Vec<String> = chunk.iter().map(|tile| format!("HarvestTile({tile})")).collect();
            writeln!(out, "    {},", row.join(", ")).unwrap();
        }
        out.push_str("];\n\n");
    }

    out.push_str(
        "/// ServUO's `Lumberjacking.m_TreeTiles`, verbatim — all statics, so every id is\n\
         /// matched through [`tile_key`]'s `| 0x4000`.\n",
    );
    out.push_str("static TREE_TILES: &[HarvestTile] = &[\n");
    for chunk in tiles.tree.chunks(8) {
        let row: Vec<String> = chunk
            .iter()
            .map(|t| {
                let _ = id(t);
                format!("HarvestTile({t})")
            })
            .collect();
        writeln!(out, "    {},", row.join(", ")).unwrap();
    }
    out.push_str("];\n\n");

    out.push_str(
        "/// Water, as inclusive `(from, to)` ranges: the sets are contiguous runs and\n\
         /// listing every id would be four hundred rows for no gain.\n",
    );
    out.push_str("static WATER_TILES: &[(HarvestTile, HarvestTile)] = &[\n");
    for (from, to) in &tiles.water {
        assert!(id(from) <= id(to), "water range {from}..{to} runs backwards");
        writeln!(out, "    (HarvestTile({from}), HarvestTile({to})),").unwrap();
    }
    out.push_str("];\n");
    out
}

/// The doc over the generated `shipped`.
const QUESTS_DOC: &str = "\
/// The quests this shard ships, built fresh from `data/quests.json`.
///
/// **This is the third shape out of `build.rs`, and the reason is the
/// destination type.** The other tables here are `const` because their callers
/// read `&'static [_]` and nothing is allocated at startup. [`QuestDefs::set`]
/// takes a `Vec<QuestDef>` and owns the strings in it, because a definition is
/// replaced wholesale rather than searched — so a `const` mirror would be a
/// second copy of three types and an extra clone at the one call site, and buy
/// back nothing. What survives from the rule is where the errors are caught: a
/// misspelt objective kind, a graphic that is not a `u16`, a count of zero and a
/// quest defined twice are all build failures, named with the file.
///
/// The order is by key, not the file's. Nothing reads a definition except
/// [`QuestDefs::get`], so the order cannot change an answer — and sorting here
/// means a row appended to the JSON does not move every row after it in a diff
/// of the generated source.
";

/// One quest in `data/quests.json`.
///
/// Mirrors `QuestDef` field for field. The five text fields a player is shown
/// are required — a quest that forgets its `refuse` line answers a refusal with
/// silence, and that is a content bug worth a build failure rather than an empty
/// string. Everything ServUO defaults is `#[serde(default)]` and left out of the
/// data.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Quest {
    key: String,
    title: String,
    description: String,
    refuse: String,
    uncomplete: String,
    complete: String,
    /// Said when a timed objective runs out. Blank for the quests that have no
    /// clock, which is all of them so far.
    #[serde(default)]
    failed: String,
    objectives: Vec<Objective>,
    #[serde(default)]
    rewards: Vec<Reward>,
    /// ServUO's default: a quest asks for everything on its list.
    #[serde(default = "every_objective")]
    all_objectives: bool,
    #[serde(default)]
    done_once: bool,
    #[serde(default)]
    restart_delay_secs: u32,
}

/// `all_objectives`' default. `bool`'s own is `false`, which is the opposite of
/// ServUO's rule, so the field cannot take it.
const fn every_objective() -> bool {
    true
}

/// `quest::ObjectiveDef`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Objective {
    kind: ObjectiveKind,
    count: u16,
    name: String,
    /// How long the player has, in seconds. `0` is untimed.
    #[serde(default)]
    seconds: u32,
}

/// `quest::ObjectiveKind`, externally tagged so the variant name is spelled in
/// the data and checked by serde. A misspelt `slya` fails the build naming the
/// file, which is the whole point of the data living here.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum ObjectiveKind {
    Slay {
        body: String,
    },
    Obtain {
        graphic: String,
    },
    Deliver {
        graphic: String,
        to: String,
    },
    Escort {
        /// The destination region, as `Regions` names it — or **empty**, which
        /// is not an omission: it means "wherever this traveller asked for",
        /// chosen when the quest is accepted (ServUO's `PickRandomDestination`).
        /// One definition covers every escortable traveller that way.
        region: String,
    },
}

impl ObjectiveKind {
    /// The Rust expression for this kind. `Graphic` is fully qualified: the
    /// generated file is `include!`d and must not depend on what the host module
    /// happens to import.
    fn expr(&self) -> String {
        match self {
            Self::Slay { body } => {
                let _ = id(body);
                format!("ObjectiveKind::Slay {{ body: openshard_protocol::wire::Graphic({body}) }}")
            }
            Self::Obtain { graphic } => {
                let _ = id(graphic);
                format!("ObjectiveKind::Obtain {{ graphic: openshard_protocol::wire::Graphic({graphic}) }}")
            }
            Self::Deliver { graphic, to } => {
                let _ = id(graphic);
                assert!(
                    !to.is_empty(),
                    "a deliver objective with no destination can never be finished"
                );
                format!(
                    "ObjectiveKind::Deliver {{ graphic: openshard_protocol::wire::Graphic({graphic}), to: {} }}",
                    owned(to)
                )
            }
            Self::Escort { region } => {
                format!("ObjectiveKind::Escort {{ region: {} }}", owned(region))
            }
        }
    }
}

/// `quest::RewardDef`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reward {
    kind: RewardKind,
    name: String,
}

/// `quest::RewardKind`.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RewardKind {
    Gold(u32),
    Item {
        graphic: String,
        #[serde(default = "no_hue")]
        hue: String,
        amount: u16,
        #[serde(default)]
        stackable: bool,
    },
}

/// An item reward's default hue: none.
fn no_hue() -> String {
    "0x0000".to_owned()
}

/// The expression for an owned string field. Empty is spelled `String::new()`
/// rather than `"".to_owned()` — the same thing, but the generated file is
/// linted like any other source and clippy is right about which one to write.
fn owned(text: &str) -> String {
    if text.is_empty() {
        "String::new()".to_owned()
    } else {
        format!("{text:?}.to_owned()")
    }
}

impl RewardKind {
    /// The Rust expression for this reward.
    fn expr(&self) -> String {
        match self {
            Self::Gold(amount) => format!("RewardKind::Gold({amount})"),
            Self::Item {
                graphic,
                hue,
                amount,
                stackable,
            } => {
                let _ = (id(graphic), id(hue));
                assert!(*amount >= 1, "an item reward of none is not a reward");
                format!(
                    "RewardKind::Item {{ graphic: openshard_protocol::wire::Graphic({graphic}), \
                     hue: openshard_protocol::wire::Hue({hue}), amount: {amount}, stackable: {stackable} }}"
                )
            }
        }
    }
}

/// The doc over the generated `region::shipped`.
const REGIONS_DOC: &str = "\
/// Every region set the shard ships, built fresh from `data/regions.json`.
///
/// Ported from ServUO's `Data/Regions.xml`, with its nesting flattened the way
/// this module's header describes: a child is a region of its own at a higher
/// priority, so the engine holds a list and a number rather than a tree.
///
/// **Keyed by an admin verb, unlike the other datasets here.** Quests and speech
/// are registered at boot unconditionally; regions are laid by
/// `regions:felucca` — a button in the staff menu and a `--seed` argument —
/// because an operator lays and clears them by hand. The verb travels *with* the
/// data rather than being spelled into the server, so adding a facet is a row in
/// the JSON and a row in `world::admin`'s menu, not a match arm.
///
/// Written in the file's order, not sorted. A region's order is not free the way
/// a quest's is: `Regions::set` numbers them by position, and that number is the
/// id a save and the wire both carry — sorting here would renumber every region
/// in an existing save.
";

/// One set in `data/regions.json`: the verb that lays it, and what it lays.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionSet {
    /// The admin verb — `world::admin`'s `ROWS` is the other half of this.
    verb: String,
    /// Which facet the areas belong to.
    facet: u8,
    /// The areas, in the order they will be numbered.
    regions: Vec<RegionDef>,
}

/// One region in `data/regions.json`.
///
/// No `id`: `Regions::set` assigns one by position, and a number written here
/// would be a second source for it. The five flags and the two overrides are
/// `#[serde(default)]`, so a plain unguarded region is three lines.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionDef {
    /// What it is called — "Britain", "Covetous".
    name: String,
    /// Which region wins where two overlap: the higher number.
    priority: u8,
    /// The boxes it covers.
    rects: Vec<Rect>,
    #[serde(default)]
    guarded: bool,
    #[serde(default)]
    no_teleport: bool,
    #[serde(default)]
    no_recall: bool,
    #[serde(default)]
    no_housing: bool,
    #[serde(default)]
    safe: bool,
    /// A `MusicName` index, ServUO's enum order.
    #[serde(default)]
    music: Option<u16>,
    /// The light level inside, overriding the time of day.
    #[serde(default)]
    light: Option<u8>,
}

/// One box of a region. `z_min`/`z_max` default to the whole column, which is
/// what a ServUO `<rect>` with no `zrange` means.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    #[serde(default = "z_floor")]
    z_min: i8,
    #[serde(default = "z_ceiling")]
    z_max: i8,
}

/// A rect's default lowest height: everything below.
const fn z_floor() -> i8 {
    i8::MIN
}

/// A rect's default highest: everything above.
const fn z_ceiling() -> i8 {
    i8::MAX
}

/// `data/regions.json` into the `shipped` constructor.
///
/// The checks are the ones `register_regions` makes at runtime with a `warn!`
/// nobody reads, plus the ones it cannot make at all. A light above `0x1F` is
/// the interesting one: the client does not reject it, it *clamps* — so the
/// region goes pitch dark and the only symptom is a player saying a room looks
/// wrong. That belongs in a build failure, not a log line.
fn regions(text: &str) -> String {
    let sets: Vec<RegionSet> = serde_json::from_str(text).expect("regions.json");

    let mut out = String::from("// @generated by build.rs from data/regions.json.\n\n");
    out.push_str(REGIONS_DOC);
    out.push_str("#[must_use]\npub fn shipped() -> Vec<RegionSet> {\n    vec![\n");
    for set in &sets {
        assert!(
            !set.verb.is_empty(),
            "a region set with no verb can never be laid"
        );
        assert!(
            !set.regions.is_empty(),
            "region set {:?} lays nothing, and an empty registration clears the facet",
            set.verb
        );
        for other in &sets {
            assert!(
                std::ptr::eq(set, other) || set.verb != other.verb,
                "two region sets answer to {:?}; the second would replace the first",
                set.verb
            );
        }

        out.push_str("        RegionSet {\n");
        writeln!(out, "            verb: {:?}.to_owned(),", set.verb).unwrap();
        writeln!(
            out,
            "            facet: openshard_protocol::world::Facet({}),",
            set.facet
        )
        .unwrap();
        out.push_str("            regions: vec![\n");
        for region in &set.regions {
            assert!(!region.name.is_empty(), "a region of {:?} has no name", set.verb);
            assert!(
                !region.rects.is_empty(),
                "region {:?} covers nothing, so no point is ever inside it",
                region.name
            );
            if let Some(light) = region.light {
                assert!(
                    light <= 0x1F,
                    "region {:?} asks for light {light}, above the client's 0x1F — \
                     it clamps rather than complaining, so the room goes pitch dark",
                    region.name
                );
            }

            out.push_str("                Region {\n");
            // The world numbers them on registration, by position. A zero here is
            // the same placeholder the script bridge passes, and for the same
            // reason: this side has no id to give.
            out.push_str("                    id: RegionId(0),\n");
            writeln!(out, "                    name: {:?}.to_owned(),", region.name).unwrap();
            writeln!(out, "                    priority: {},", region.priority).unwrap();
            out.push_str("                    rects: vec![\n");
            for rect in &region.rects {
                assert!(
                    rect.width > 0 && rect.height > 0,
                    "a rect of region {:?} is {}x{}, which contains no tile",
                    region.name,
                    rect.width,
                    rect.height
                );
                assert!(
                    rect.z_min <= rect.z_max,
                    "a rect of region {:?} has its z band inverted ({}..{})",
                    region.name,
                    rect.z_min,
                    rect.z_max
                );
                writeln!(
                    out,
                    "                        RegionRect {{ x: {}, y: {}, width: {}, height: {}, \
                     z_min: {}, z_max: {} }},",
                    rect.x, rect.y, rect.width, rect.height, rect.z_min, rect.z_max
                )
                .unwrap();
            }
            out.push_str("                    ],\n");
            out.push_str("                    flags: RegionFlags {\n");
            for (field, value) in [
                ("guarded", region.guarded),
                ("no_teleport", region.no_teleport),
                ("no_recall", region.no_recall),
                ("no_housing", region.no_housing),
                ("safe", region.safe),
            ] {
                writeln!(out, "                        {field}: {value},").unwrap();
            }
            out.push_str("                    },\n");
            // Written as two separate matches rather than a loop over a pair,
            // because `music` is a `u16` and `light` a `u8` — a shared loop would
            // need one of them widened, and a light silently widened is exactly
            // the field with a ceiling worth checking.
            match region.music {
                Some(music) => writeln!(out, "                    music: Some({music}),").unwrap(),
                None => out.push_str("                    music: None,\n"),
            }
            match region.light {
                Some(light) => writeln!(out, "                    light: Some({light}),").unwrap(),
                None => out.push_str("                    light: None,\n"),
            }
            out.push_str("                },\n");
        }
        out.push_str("            ],\n");
        out.push_str("        },\n");
    }
    out.push_str("    ]\n}\n");
    out
}

/// The doc over the generated `shipped`.
const SPEECH_DOC: &str = "\
/// What every trade the shard ships says, built fresh from `data/speech.json`.
///
/// Ported from ServUO — the shop lists its `SB*.cs` vendors carry, and the
/// clilocs a `BaseVendor` answers with — because ServUO has the *mechanism* and
/// almost none of the words: a vendor's entire stock vocabulary is 500186 and
/// 501522, which is two lines for sixty-eight trades. The rest is written to
/// ServUO's voice rather than lifted from it.
///
/// [`QuestDefs::shipped`](crate::quest::shipped)'s shape, for
/// [`QuestDefs::shipped`](crate::quest::shipped)'s reason: the destination owns
/// its strings. `Dialogue::set_tables` takes the map and replaces everything
/// before it, so a `const` here would be a second spelling of two types cloned
/// once at the one call site.
///
/// Pairs rather than a map, because that is what the command carries and a
/// `HashMap` would fix an order the data does not have. The order is by title —
/// nothing reads a table except [`Dialogue::table`], so it cannot change an
/// answer, and it keeps a trade appended to the JSON from moving every trade
/// after it in a diff of the generated source.
";

/// One trade in `data/speech.json`.
///
/// Only `title` is required. A trade that greets and says nothing else omits
/// three fields rather than spelling out three empty lists, and the shape it
/// leaves is the one the reader cares about.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trade {
    /// The [`Title`] its NPCs wear, and the key the table is found by.
    title: String,
    /// What it greets an approaching player with.
    #[serde(default)]
    greetings: Vec<String>,
    /// What it says to itself. Empty is silence, and two trades in three are.
    #[serde(default)]
    barks: Vec<String>,
    /// Keyword groups, in precedence order — the first match wins.
    #[serde(default)]
    entries: Vec<TradeEntry>,
    /// What it answers when nothing matched. Blank stays quiet, which is what
    /// every trade so far does.
    #[serde(default)]
    fallback: String,
}

/// One keyword group of a [`Trade`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TradeEntry {
    /// The words that trigger it.
    keywords: Vec<String>,
    /// The answers, one picked at random.
    lines: Vec<String>,
}

/// The Rust expression for a list of owned strings.
fn owned_list(values: &[String], indent: &str) -> String {
    if values.is_empty() {
        return "Vec::new()".to_owned();
    }
    let mut out = String::from("vec![\n");
    for value in values {
        writeln!(out, "{indent}    {value:?}.to_owned(),").unwrap();
    }
    write!(out, "{indent}]").unwrap();
    out
}

/// `data/speech.json` into the `shipped` constructor.
///
/// The checks here are the ones the running shard cannot make. A keyword that is
/// not lowercase can never match, because [`overhear`](crate::speech) lowercases
/// the sentence before comparing — so it is silence that looks like content, and
/// the script bridge's answer (lowercase it quietly) is the wrong one for data
/// that can simply be corrected. A table with nothing in it is worse than no
/// table: [`Dialogue::table`] answers `Some`, and every field being empty then
/// reads as a trade that has been struck dumb.
fn speech(text: &str) -> String {
    let mut trades: Vec<Trade> = serde_json::from_str(text).expect("speech.json");

    trades.sort_by(|a, b| a.title.cmp(&b.title));
    for pair in trades.windows(2) {
        assert_ne!(
            pair[0].title, pair[1].title,
            "speech.json defines {:?} twice, and the map would keep whichever came last",
            pair[0].title
        );
    }

    let mut out = String::from("// @generated by build.rs from data/speech.json.\n\n");
    out.push_str(SPEECH_DOC);
    out.push_str("#[must_use]\npub fn shipped() -> Vec<(String, SpeechTable)> {\n    vec![\n");
    for trade in &trades {
        assert!(
            !trade.title.is_empty(),
            "a trade with no title is keyed by nothing and can never be found"
        );
        assert!(
            !trade.greetings.is_empty()
                || !trade.barks.is_empty()
                || !trade.entries.is_empty()
                || !trade.fallback.is_empty(),
            "trade {:?} says nothing at all, which is not the same as having no table",
            trade.title
        );

        writeln!(out, "        (").unwrap();
        writeln!(out, "            {:?}.to_owned(),", trade.title).unwrap();
        out.push_str("            SpeechTable {\n");
        writeln!(
            out,
            "                greetings: {},",
            owned_list(&trade.greetings, "                ")
        )
        .unwrap();
        writeln!(
            out,
            "                barks: {},",
            owned_list(&trade.barks, "                ")
        )
        .unwrap();

        if trade.entries.is_empty() {
            out.push_str("                entries: Vec::new(),\n");
        } else {
            out.push_str("                entries: vec![\n");
            for entry in &trade.entries {
                assert!(
                    !entry.keywords.is_empty(),
                    "an entry of trade {:?} has no keywords, so it can never be reached",
                    trade.title
                );
                assert!(
                    !entry.lines.is_empty(),
                    "an entry of trade {:?} has no answers, so matching it is silence",
                    trade.title
                );
                for keyword in &entry.keywords {
                    assert!(
                        !keyword.trim().is_empty(),
                        "a blank keyword of trade {:?} matches nothing and is dead weight",
                        trade.title
                    );
                    assert_eq!(
                        *keyword,
                        keyword.to_lowercase(),
                        "keyword {keyword:?} of trade {:?} is not lowercase, and the \
                         sentence it is matched against always is",
                        trade.title
                    );
                }
                out.push_str("                    SpeechEntry {\n");
                writeln!(
                    out,
                    "                        keywords: {},",
                    owned_list(&entry.keywords, "                        ")
                )
                .unwrap();
                writeln!(
                    out,
                    "                        lines: {},",
                    owned_list(&entry.lines, "                        ")
                )
                .unwrap();
                out.push_str("                    },\n");
            }
            out.push_str("                ],\n");
        }

        match trade.fallback.is_empty() {
            true => out.push_str("                fallback: None,\n"),
            false => writeln!(
                out,
                "                fallback: Some({:?}.to_owned()),",
                trade.fallback
            )
            .unwrap(),
        }
        out.push_str("            },\n");
        out.push_str("        ),\n");
    }
    out.push_str("    ]\n}\n");
    out
}

/// `data/quests.json` into the `shipped` constructor.
fn quests(text: &str) -> String {
    let mut quests: Vec<Quest> = serde_json::from_str(text).expect("quests.json");

    // Sorted here rather than in the data, and checked for the duplicate
    // `QuestDefs::set` resolves by keeping whichever came last — a rule that is
    // right for a pack redefining a quest and wrong for one file defining it
    // twice, where the loser is invisible.
    quests.sort_by(|a, b| a.key.cmp(&b.key));
    for pair in quests.windows(2) {
        assert_ne!(
            pair[0].key, pair[1].key,
            "quests.json defines {:?} twice",
            pair[0].key
        );
    }

    let mut out = String::from("// @generated by build.rs from data/quests.json.\n\n");
    out.push_str(QUESTS_DOC);
    out.push_str("#[must_use]\npub fn shipped() -> Vec<QuestDef> {\n    vec![\n");
    for quest in &quests {
        assert!(
            !quest.key.is_empty(),
            "a quest with no key cannot be offered, bound to a giver, or saved"
        );
        // The engine's own rule, moved to the build: the bridge from the script
        // pack drops an objectiveless quest at load, because one shows as a quest
        // that can be taken and never finished.
        assert!(
            !quest.objectives.is_empty(),
            "quest {:?} asks for nothing, so it could be taken and never finished",
            quest.key
        );

        writeln!(out, "        QuestDef {{").unwrap();
        writeln!(out, "            key: QuestKey::from({}),", owned(&quest.key)).unwrap();
        for (field, text) in [
            ("title", &quest.title),
            ("description", &quest.description),
            ("refuse", &quest.refuse),
            ("uncomplete", &quest.uncomplete),
            ("complete", &quest.complete),
            ("failed", &quest.failed),
        ] {
            writeln!(out, "            {field}: {},", owned(text)).unwrap();
        }

        out.push_str("            objectives: vec![\n");
        for objective in &quest.objectives {
            // `count: 0` is complete on sight, which reads in the data as a typo
            // and in the game as a quest that pays for nothing.
            assert!(
                objective.count >= 1,
                "objective {:?} of quest {:?} asks for none",
                objective.name,
                quest.key
            );
            writeln!(
                out,
                "                ObjectiveDef {{ kind: {}, count: {}, name: {}, seconds: {} }},",
                objective.kind.expr(),
                objective.count,
                owned(&objective.name),
                objective.seconds
            )
            .unwrap();
        }
        out.push_str("            ],\n");

        out.push_str("            rewards: vec![\n");
        for reward in &quest.rewards {
            writeln!(
                out,
                "                RewardDef {{ kind: {}, name: {} }},",
                reward.kind.expr(),
                owned(&reward.name)
            )
            .unwrap();
        }
        out.push_str("            ],\n");

        writeln!(out, "            all_objectives: {},", quest.all_objectives).unwrap();
        writeln!(out, "            done_once: {},", quest.done_once).unwrap();
        writeln!(
            out,
            "            restart_delay_secs: {},",
            quest.restart_delay_secs
        )
        .unwrap();
        out.push_str("        },\n");
    }
    out.push_str("    ]\n}\n");
    out
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let out_dir = Path::new(&out_dir);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data");

    let names = std::fs::read_to_string("data/creature_names.json").expect("creature_names.json");
    std::fs::write(
        out_dir.join("creature_names.rs"),
        creatures(
            &names,
            "creature_names.json",
            CREATURE_NAME_DOC,
            "pub const fn creature_name(body: Graphic) -> Option<&'static str>",
            "Some(match body.0 {",
            "})",
        ),
    )
    .expect("writing creature_names.rs");

    let sounds = std::fs::read_to_string("data/creature_sounds.json").expect("creature_sounds.json");
    std::fs::write(
        out_dir.join("creature_sounds.rs"),
        creatures(
            &sounds,
            "creature_sounds.json",
            CREATURE_SOUND_DOC,
            "pub const fn creature_base_sound(body: Graphic) -> Option<SoundId>",
            "Some(SoundId(match body.0 {",
            "}))",
        ),
    )
    .expect("writing creature_sounds.rs");

    for (name, render) in [
        ("body_types", body_types as fn(&str) -> String),
        ("mounts", mounts),
        ("skills", skills),
        ("harvest_tiles", harvest_tiles),
        ("quests", quests),
        ("speech", speech),
        ("regions", regions),
    ] {
        let path = Path::new("data").join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        std::fs::write(out_dir.join(format!("{name}.rs")), render(&text))
            .unwrap_or_else(|e| panic!("writing {name}.rs: {e}"));
    }
}
