//! Turns `data/*.json` into the `&'static` tables `defs` publishes: one recipe
//! list per trade, and the `SYSTEMS` header table that names them.
//!
//! The six trades are 492 recipes of pure data, generated once from ServUO and
//! edited as data ever since. Written out as Rust source they were sixteen
//! thousand lines — a `Recipe` literal is thirteen fields, eleven of which are
//! the default in almost every row, and each one dragged two named `const`s
//! beside it for its skills and its ingredients. As JSON with the defaults left
//! out they are five thousand, and a row is one line per ingredient.
//!
//! It is a build script rather than a runtime load on purpose: the tables stay
//! `const`, so nothing is parsed or allocated when a shard starts and the gump
//! still reads `&'static [Recipe]`. What a runtime load would report as an error
//! on the first craft of the day, this reports before the crate compiles.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

/// One trade's file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    /// The gump's left-hand column.
    groups: Vec<Text>,
    /// The material axis, for the four trades that have one.
    #[serde(default)]
    sub_res: Option<Axis>,
    /// Everything the trade can make.
    recipes: Vec<Row>,
}

/// `system::Text`: a number is a cliloc, a string is a literal.
#[derive(Deserialize)]
#[serde(untagged)]
enum Text {
    /// A cliloc number the client localizes itself.
    Cliloc(u32),
    /// A literal, for the rows ServUO has no number for.
    Str(String),
}

impl Text {
    /// The Rust expression for this text.
    fn expr(&self) -> String {
        match self {
            // Fully qualified: the generated file is `include!`d into modules
            // whose imports it cannot see, and a bare `ClilocId` would make the
            // generator depend on every one of them importing it.
            Self::Cliloc(n) => format!("Text::Cliloc(openshard_protocol::wire::ClilocId({n}))"),
            Self::Str(s) => format!("Text::Str({s:?})"),
        }
    }
}

/// `recipe::SubResAxis`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Axis {
    /// The resource graphic the axis substitutes a hue into.
    graphic: String,
    /// The heading over the material row.
    name: Text,
    /// The grades, plain one first.
    entries: Vec<AxisEntry>,
}

/// `recipe::SubRes`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AxisEntry {
    /// The hue that *is* this material.
    hue: String,
    /// Its name, for the material row.
    name: Text,
    /// The base skill needed to work it, in tenths.
    req_skill: i32,
    /// What is said when the crafter is not good enough.
    message: Text,
}

/// `recipe::Recipe`. Every field with a `default` is one the overwhelming
/// majority of rows leave alone, and leaving it out of the JSON is what makes a
/// recipe readable at a glance.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    /// The item art produced.
    graphic: String,
    /// Its name, for the gump.
    name: Text,
    /// Which group it files under.
    group: u16,
    /// How many come out.
    #[serde(default = "one")]
    amount: u16,
    /// A fixed hue for the result; zero defers to `retain_color`.
    #[serde(default = "zero")]
    hue: String,
    /// Whether a zero-hued result inherits the chosen material's hue.
    #[serde(default = "yes")]
    retain_color: bool,
    /// Consume every material in the pack and make as many as it can.
    #[serde(default)]
    use_all_res: bool,
    /// Tenths knocked off the "can you attempt this at all" gate.
    #[serde(default)]
    min_skill_offset: i32,
    /// Whether an exceptional one carries its maker's name.
    #[serde(default)]
    markable: bool,
    /// Never exceptional, whatever the roll.
    #[serde(default)]
    never_exceptional: bool,
    /// Always exceptional.
    #[serde(default)]
    always_exceptional: bool,
    /// What has to be standing nearby, on top of the system's own.
    #[serde(default)]
    needs: NeedsRow,
    /// The skills wanted; the system's own leads.
    skills: Vec<SkillRow>,
    /// What it eats.
    resources: Vec<ResRow>,
}

/// `recipe::CraftSkillReq`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillRow {
    /// The `Skill` variant's name, spelled as the enum spells it.
    skill: String,
    /// The bottom of the band, in tenths.
    min: i32,
    /// The top, at which the craft always succeeds.
    max: i32,
}

/// `recipe::CraftRes`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResRow {
    /// The item art consumed.
    graphic: String,
    /// And its hue; absent is the plain grade.
    #[serde(default = "zero")]
    hue: String,
    /// How many, per craft.
    amount: u16,
    /// What it is called, for the detail page.
    name: Text,
    /// What is said when there are not enough.
    message: Text,
    /// Whether the material axis substitutes into this line.
    #[serde(default)]
    from_axis: bool,
}

/// `system::Needs`, with every requirement absent by default.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct NeedsRow {
    /// A forge.
    #[serde(default)]
    forge: bool,
    /// An anvil.
    #[serde(default)]
    anvil: bool,
    /// Any fire at all.
    #[serde(default)]
    heat: bool,
    /// An oven.
    #[serde(default)]
    oven: bool,
    /// A flour mill.
    #[serde(default)]
    mill: bool,
    /// Water.
    #[serde(default)]
    water: bool,
}

/// `data/craft_systems.json`: the trade headers, which used to be a
/// hand-written `SYSTEMS` in `defs/mod.rs`. They are here because the two
/// invariants below are properties of a *recipe row* checked against its
/// *system's* header, and a build script that can only see one half can check
/// neither.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Systems {
    /// Prose at the top of the file, for whoever opens it. Read by nobody.
    #[serde(default)]
    _comment: Vec<String>,
    /// In order: the index is the `SystemId`.
    systems: Vec<SystemRow>,
}

/// `system::CraftSystemDef`, minus the three fields that come from the trade's
/// own table (`groups`, `recipes`, `sub_res`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemRow {
    /// Which `data/<trade>.json` supplies this system's recipes.
    trade: String,
    /// Why this system's numbers are what they are; emitted as a comment above
    /// the row, because that is where it was worth reading before the move.
    #[serde(default)]
    note: String,
    /// The `Skill` variant's name, spelled as the enum spells it — and the skill
    /// every one of the trade's recipes has to lead with.
    skill: String,
    /// The title over the gump.
    title: Text,
    /// Success chance at the bottom of a recipe's band, in per-mille.
    chance_at_min: u32,
    /// The `Eca` variant's name.
    eca: String,
    /// ServUO's `Delay`, in milliseconds rather than ticks: the tick rate is the
    /// engine's business and this file is ServUO's numbers.
    #[serde(default = "default_delay_ms")]
    delay_ms: u64,
    /// The fewest beats one craft takes.
    #[serde(default = "one_beat")]
    min_beats: u8,
    /// And the most.
    #[serde(default = "one_beat")]
    max_beats: u8,
    /// What the tool makes on each beat.
    craft_sound: String,
    /// What has to be standing nearby for *any* of this system's recipes.
    #[serde(default)]
    needs: NeedsRow,
    /// The cliloc said when `needs` is not met. Absent — not zero — for the four
    /// systems that need no workshop, so "no message" and "message 0" are not
    /// the same bits.
    #[serde(default)]
    needs_message: Option<u32>,
}

/// serde's `default` wants a function, and one is the only sane stack size.
fn one() -> u16 {
    1
}

/// `base(1, 1, 1.25)` — the delay all shipped systems pass up.
fn default_delay_ms() -> u64 {
    1250
}

/// Likewise for the beat count, which is 1 everywhere so far.
fn one_beat() -> u8 {
    1
}

/// Most craftable items retain the hue of their material.
fn yes() -> bool {
    true
}

/// Likewise for a hue nobody wrote down.
fn zero() -> String {
    "0x0000".to_owned()
}

/// A hex literal from the data, passed through verbatim so the generated source
/// reads the way the table does — but parsed first, because `Graphic(0xZZZZ)`
/// would otherwise be a compile error in a file nobody has open.
fn hex(field: &str, raw: &str) -> String {
    let digits = raw
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("{field}: {raw} is not 0x-prefixed"));
    u16::from_str_radix(digits, 16).unwrap_or_else(|e| panic!("{field}: {raw} is not a u16 ({e})"));
    raw.to_owned()
}

impl NeedsRow {
    /// The Rust expression, short where nothing is wanted — which is every one of
    /// the 492 recipe rows. Only a *system* asks for a workshop today
    /// (blacksmithy's forge and anvil); the per-recipe half is carried because
    /// ServUO's ovens, mills and water are per-recipe and the trades that use
    /// them are not ported.
    fn expr(&self) -> String {
        if !(self.forge || self.anvil || self.heat || self.oven || self.mill || self.water) {
            return "Needs::none()".to_owned();
        }
        format!(
            "Needs {{ forge: {}, anvil: {}, heat: {}, oven: {}, mill: {}, water: {} }}",
            self.forge, self.anvil, self.heat, self.oven, self.mill, self.water
        )
    }
}

/// One trade's generated source.
fn generate(table: &Table) -> String {
    let mut out = String::new();
    out.push_str("// @generated by build.rs from the trade's `data/*.json`. Edit the JSON.\n\n");

    out.push_str("/// The gump's left-hand column.\npub const GROUPS: &[Text] = &[\n");
    for group in &table.groups {
        writeln!(out, "    {},", group.expr()).unwrap();
    }
    out.push_str("];\n\n");

    out.push_str("/// Everything this trade can make.\npub const RECIPES: &[Recipe] = &[\n");
    for row in &table.recipes {
        writeln!(out, "    Recipe {{").unwrap();
        writeln!(out, "        graphic: Graphic({}),", hex("graphic", &row.graphic)).unwrap();
        writeln!(out, "        name: {},", row.name.expr()).unwrap();
        writeln!(out, "        group: {},", row.group).unwrap();
        out.push_str("        skills: &[\n");
        for want in &row.skills {
            writeln!(
                out,
                "            CraftSkillReq {{ skill: Skill::{}, min: {}, max: {} }},",
                want.skill, want.min, want.max
            )
            .unwrap();
        }
        out.push_str("        ],\n        resources: &[\n");
        for res in &row.resources {
            writeln!(
                out,
                "            CraftRes {{ graphic: Graphic({}), hue: Hue({}), amount: {}, name: {}, \
                 message: {}, from_axis: {} }},",
                hex("resource graphic", &res.graphic),
                hex("resource hue", &res.hue),
                res.amount,
                res.name.expr(),
                res.message.expr(),
                res.from_axis
            )
            .unwrap();
        }
        out.push_str("        ],\n");
        writeln!(out, "        amount: {},", row.amount).unwrap();
        writeln!(out, "        hue: Hue({}),", hex("hue", &row.hue)).unwrap();
        writeln!(out, "        retain_color: {},", row.retain_color).unwrap();
        writeln!(out, "        use_all_res: {},", row.use_all_res).unwrap();
        writeln!(out, "        min_skill_offset: {},", row.min_skill_offset).unwrap();
        writeln!(out, "        markable: {},", row.markable).unwrap();
        writeln!(out, "        never_exceptional: {},", row.never_exceptional).unwrap();
        writeln!(out, "        always_exceptional: {},", row.always_exceptional).unwrap();
        writeln!(out, "        needs: {},", row.needs.expr()).unwrap();
        out.push_str("    },\n");
    }
    out.push_str("];\n");

    if let Some(axis) = &table.sub_res {
        out.push_str("\n/// The material grades this trade's gump offers.\n");
        out.push_str("pub const SUB_RES: SubResAxis = SubResAxis {\n");
        writeln!(
            out,
            "    graphic: Graphic({}),",
            hex("axis graphic", &axis.graphic)
        )
        .unwrap();
        writeln!(out, "    name: {},", axis.name.expr()).unwrap();
        out.push_str("    entries: &[\n");
        for entry in &axis.entries {
            writeln!(
                out,
                "        SubRes {{ hue: Hue({}), name: {}, req_skill: {}, message: {} }},",
                hex("axis hue", &entry.hue),
                entry.name.expr(),
                entry.req_skill,
                entry.message.expr()
            )
            .unwrap();
        }
        out.push_str("    ],\n};\n");
    }

    out
}

/// The two rules a recipe row obeys and could not state, checked here so a bad
/// row stops being a build rather than a test run.
///
/// Both are silent at runtime, which is why they are worth a check at all: an
/// out-of-range group draws an empty category in the gump, and a recipe with no
/// line for its system's main skill reads as chance zero — a thing the trade
/// simply always refuses. Neither raises anything anywhere.
fn check(row: &SystemRow, table: &Table) {
    // A check over nothing is green, and this crate has been caught by that
    // shape before: say how many rows there were to check.
    assert!(!table.recipes.is_empty(), "{} has no recipes at all", row.trade);
    for recipe in &table.recipes {
        assert!(
            usize::from(recipe.group) < table.groups.len(),
            "{}: recipe {} is in group {}, and the trade has {}",
            row.trade,
            recipe.graphic,
            recipe.group,
            table.groups.len()
        );
        // Leads with, not merely names: `chance` finds the main skill wherever it
        // sits, but `Recipe::skills` documents the first as the one the success
        // chance is interpolated over, and all shipped rows obey it today.
        let leads = recipe.skills.first().is_some_and(|want| want.skill == row.skill);
        assert!(
            leads,
            "{}: recipe {} does not lead with {}",
            row.trade, recipe.graphic, row.skill
        );
    }
}

/// The generated `SYSTEMS`.
///
/// `tables` is every trade's parsed file; a header names one by `trade` and the
/// three list fields are that table's, so the header cannot name a trade whose
/// recipes are somewhere else.
fn generate_systems(systems: &Systems, tables: &BTreeMap<String, Table>) -> String {
    let mut out = String::new();
    out.push_str("// @generated by build.rs from `data/craft_systems.json`. Edit the JSON.\n\n");
    out.push_str("/// The trades a shard can practise, in the order their ids are numbered.\n");
    out.push_str("///\n");
    out.push_str("/// The index into this table is a [`SystemId`] — it rides in a `Crafting`\n");
    out.push_str("/// component and, once crafting is saved mid-flight, in a record. **Append,\n");
    out.push_str("/// never reorder.**\n");
    out.push_str("pub const SYSTEMS: &[CraftSystemDef] = &[\n");

    let mut claimed = BTreeMap::new();
    for row in &systems.systems {
        let table = tables
            .get(&row.trade)
            .unwrap_or_else(|| panic!("craft_systems.json names {}, which has no table", row.trade));
        if let Some(first) = claimed.insert(&row.trade, &row.skill) {
            panic!("{} is claimed by both {first} and {}", row.trade, row.skill);
        }
        check(row, table);

        if !row.note.is_empty() {
            writeln!(out, "    // {}", row.note).unwrap();
        }
        out.push_str("    CraftSystemDef {\n");
        writeln!(out, "        skill: Skill::{},", skill(&row.skill)).unwrap();
        writeln!(out, "        title: {},", row.title.expr()).unwrap();
        writeln!(out, "        chance_at_min: {},", row.chance_at_min).unwrap();
        writeln!(out, "        eca: Eca::{},", eca(&row.eca)).unwrap();
        // A tick count rather than the milliseconds, because everything the tick
        // compares it against is a tick count. The assertion below is what keeps
        // the two readings of a delay from drifting apart silently: emitted, not
        // evaluated, because only the crate proper knows `TICKS_PER_SECOND`.
        writeln!(
            out,
            "        delay_ticks: TICKS_PER_SECOND * {} / 1000,",
            row.delay_ms
        )
        .unwrap();
        writeln!(out, "        min_beats: {},", row.min_beats).unwrap();
        writeln!(out, "        max_beats: {},", row.max_beats).unwrap();
        writeln!(
            out,
            "        craft_sound: SoundId({}),",
            hex("craft sound", &row.craft_sound)
        )
        .unwrap();
        writeln!(out, "        needs: {},", row.needs.expr()).unwrap();
        writeln!(
            out,
            "        needs_message: {},",
            match row.needs_message {
                Some(cliloc) => format!("Some(openshard_protocol::wire::ClilocId({cliloc}))"),
                None => "None".to_owned(),
            }
        )
        .unwrap();
        writeln!(out, "        groups: {}::GROUPS,", row.trade).unwrap();
        writeln!(out, "        recipes: {}::RECIPES,", row.trade).unwrap();
        writeln!(
            out,
            "        sub_res: {},",
            match table.sub_res {
                Some(_) => format!("Some({}::SUB_RES)", row.trade),
                None => "None".to_owned(),
            }
        )
        .unwrap();
        out.push_str("    },\n");
    }
    out.push_str("];\n");

    // The other direction, and the one that decides whether "no bad rows" means
    // anything: a table no header names is a trade the gump never offers *and*
    // a table these checks never open.
    for trade in tables.keys() {
        assert!(
            claimed.contains_key(trade),
            "{trade} has a table and no system in {SYSTEMS_FILE}.json"
        );
    }

    // One per *distinct* delay, not per system: copies of the same assertion say
    // nothing several times.
    let delays: BTreeSet<u64> = systems.systems.iter().map(|row| row.delay_ms).collect();
    for ms in delays {
        writeln!(
            out,
            "\n/// A delay that is not a whole number of ticks would be truncated, and the \
             craft\n/// would quietly run at a rate nobody wrote down.\nconst _: () = \
             assert!(\n    (TICKS_PER_SECOND * {ms}).is_multiple_of(1000),\n    \"a craft delay of \
             {ms}ms is \
             not a whole number of ticks\"\n);"
        )
        .unwrap();
    }
    out
}

/// A `Skill` variant name, checked for shape only — the compiler has the list,
/// and a name it does not know is a compile error in the generated file with the
/// row's own text in it.
fn skill(name: &str) -> &str {
    assert!(
        name.chars().all(char::is_alphanumeric) && name.starts_with(char::is_uppercase),
        "{name} is not a Skill variant"
    );
    name
}

/// An `Eca` variant name. Enumerated rather than passed through, because the
/// three are the whole of ServUO's `CraftECA` and a fourth is a decision, not a
/// typo.
fn eca(name: &str) -> &str {
    match name {
        "ChanceMinusSixty" | "FiftyPercentChanceMinusTenPercent" | "ChanceMinusSixtyToFourtyFive" => name,
        other => panic!("{other} is not an Eca"),
    }
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let data = Path::new("data");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", data.display());

    // Sorted, so a rebuild on a different filesystem produces the same bytes.
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(data).expect("crafting/data exists") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let stem = path
                .file_stem()
                .expect("a .json has a stem")
                .to_string_lossy()
                .into_owned();
            files.insert(stem, path);
        }
    }
    // The headers are the one file in here that is not a trade.
    let headers = files
        .remove(SYSTEMS_FILE)
        .unwrap_or_else(|| panic!("crafting/data has no {SYSTEMS_FILE}.json"));
    assert!(!files.is_empty(), "crafting/data has no tables");

    let mut tables = BTreeMap::new();
    for (trade, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let table: Table = read(path);
        let generated = generate(&table);
        std::fs::write(Path::new(&out_dir).join(format!("{trade}.rs")), generated)
            .unwrap_or_else(|e| panic!("writing {trade}.rs: {e}"));
        tables.insert(trade.clone(), table);
    }

    // Last, and with every table in hand: the headers are checked against the
    // rows they own, which is the only place both halves exist at once.
    println!("cargo:rerun-if-changed={}", headers.display());
    let systems: Systems = read(&headers);
    let generated = generate_systems(&systems, &tables);
    std::fs::write(Path::new(&out_dir).join("systems.rs"), generated)
        .unwrap_or_else(|e| panic!("writing systems.rs: {e}"));
}

/// The headers' file, by stem.
const SYSTEMS_FILE: &str = "craft_systems";

/// Parse one data file, naming it in both failures — a build script's panic is
/// all the diagnostic there is.
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}
