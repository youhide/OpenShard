//! Generates the client/server craft presentation catalogue from gameplay data.
//!
//! The authoritative crafting crate compiles the same JSON into executable
//! recipes. Protocol owns this presentation-only artifact so both the client
//! and server can name the same dense stock keys and revision without either
//! side depending on the other's runtime crate.

use std::collections::{
    BTreeMap,
    BTreeSet,
};
use std::fmt::Write as _;
use std::fs;
use std::path::{
    Path,
    PathBuf,
};

use serde_json::Value;

#[derive(Clone)]
struct ItemInfo {
    id:       u32,
    name:     String,
    family:   Option<String>,
    tags:     Vec<String>,
    graphics: Vec<u16>,
}

#[derive(Clone)]
struct MaterialInfo {
    id:     u16,
    family: String,
    name:   String,
    hue:    u16,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StockSelector {
    kind:     Option<u32>,
    material: Option<u16>,
    graphic:  u16,
    hue:      u16,
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let crafting = manifest.join("../../server/crafting/data");
    let state = manifest.join("../../server/state/data");
    let systems_path = crafting.join("craft_systems.json");
    let items_path = state.join("items.json");
    let materials_path = state.join("materials.json");

    for path in [&systems_path, &items_path, &materials_path] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let systems = json(&systems_path);
    let items_json = json(&items_path);
    let materials_json = json(&materials_path);
    let items = parse_items(&items_json);
    let material_entries = parse_material_entries(&materials_json);
    let materials = material_entries
        .iter()
        .map(|material| ((material.family.clone(), material.hue), material.id))
        .collect();
    let mut revision_bytes = Vec::new();
    revision_bytes.extend(fs::read(&systems_path).expect("read craft systems"));
    revision_bytes.extend(fs::read(&items_path).expect("read item definitions"));
    revision_bytes.extend(fs::read(&materials_path).expect("read material definitions"));

    let mut selectors = BTreeSet::new();
    let mut skill_ids = BTreeSet::new();
    let mut rows = Vec::new();
    let mut recipe_index = 0usize;
    for (system_index, system) in array(field(&systems, "systems"), "systems").iter().enumerate() {
        let trade = string(field(system, "trade"), "trade");
        let table_path = crafting.join(format!("{trade}.json"));
        println!("cargo:rerun-if-changed={}", table_path.display());
        revision_bytes.extend(fs::read(&table_path).expect("read craft table"));
        let table = json(&table_path);
        let axis = table.get("sub_res");
        let system_needs = needs_mask(system.get("needs"));
        for (recipe_in_system, row) in array(field(&table, "recipes"), "recipes").iter().enumerate() {
            for skill in array(field(row, "skills"), "skills") {
                skill_ids.insert(skill_id(string(field(skill, "skill"), "skill")));
            }
            let mut components = Vec::new();
            for resource in array(field(row, "resources"), "resources") {
                let (selector, name, shown_material) = resource_selector(resource, axis, &items, &materials);
                selectors.insert(selector);
                if bool_field(resource.get("from_axis")) {
                    if let Some(axis) = axis {
                        let kind = integer(field(axis, "item_kind"), "axis item kind") as u32;
                        for entry in array(field(axis, "entries"), "axis entries") {
                            selectors.insert(StockSelector {
                                kind:     Some(kind),
                                material: Some(integer(field(entry, "material"), "axis material") as u16),
                                graphic:  selector.graphic,
                                hue:      hex(string(field(entry, "hue"), "axis hue")),
                            });
                        }
                    }
                }
                components.push((selector, name, shown_material, resource.clone()));
            }
            rows.push((
                recipe_index,
                system_index,
                recipe_in_system,
                system_needs,
                row.clone(),
                components,
            ));
            recipe_index += 1;
        }
    }

    let selector_ids: BTreeMap<_, _> = selectors
        .iter()
        .copied()
        .enumerate()
        .map(|(index, selector)| (selector, index))
        .collect();
    let mut legacy_multiplicity = BTreeMap::<(u16, u16), usize>::new();
    let mut semantic_multiplicity = BTreeMap::<(u32, Option<u16>), usize>::new();
    for selector in &selectors {
        *legacy_multiplicity
            .entry((selector.graphic, selector.hue))
            .or_default() += 1;
        if let Some(kind) = selector.kind {
            *semantic_multiplicity
                .entry((kind, selector.material))
                .or_default() += 1;
        }
    }
    let max_keys_per_pile = legacy_multiplicity
        .values()
        .chain(semantic_multiplicity.values())
        .copied()
        .max()
        .unwrap_or_default();
    assert!(
        max_keys_per_pile <= 4,
        "one pile maps to more than four craft keys"
    );
    let revision = fnv1a(&revision_bytes);
    let mut out = String::new();
    writeln!(out, "pub const CRAFT_CATALOGUE_REVISION: u64 = {revision};").unwrap();
    writeln!(out, "pub const CRAFT_KEY_COUNT: usize = {};", selectors.len()).unwrap();
    writeln!(
        out,
        "pub const MAX_CRAFT_KEYS_PER_PILE: usize = {max_keys_per_pile};"
    )
    .unwrap();
    write!(out, "pub const CRAFT_SKILL_IDS: &[u8] = &[").unwrap();
    for skill in skill_ids {
        write!(out, "{skill},").unwrap();
    }
    out.push_str("];\n");
    out.push_str("pub const CRAFT_RECIPE_LOCATIONS: &[(u8, u16)] = &[\n");
    for (_, system, recipe, _, _, _) in &rows {
        writeln!(out, "    ({}u8, {}u16),", system, recipe).unwrap();
    }
    out.push_str("];\n");
    out.push_str("pub const CRAFT_STOCK_SELECTORS: &[CraftStockSelector] = &[\n");
    for selector in &selectors {
        writeln!(
            out,
            "    CraftStockSelector {{ kind: {}, material: {}, graphic: Graphic({}), hue: Hue({}) }},",
            option_u32(selector.kind, "ItemKindId"),
            option_u16(selector.material, "MaterialId"),
            selector.graphic,
            selector.hue,
        )
        .unwrap();
    }
    out.push_str("];\n\n");
    out.push_str("pub fn craft_catalogue_definitions() -> Vec<CraftCatalogueDefinitionRow> {\n    vec![\n");
    for (index, _, _, system_needs, row, components) in rows {
        let name = cliloc(row.get("name"));
        let graphic = hex(string(field(&row, "graphic"), "graphic"));
        let hue = row.get("hue").map_or(0, |value| hex(string(value, "hue")));
        let kind = row.get("kind").and_then(Value::as_u64).map(|value| value as u32);
        let offset = row.get("min_skill_offset").and_then(Value::as_i64).unwrap_or(0) as i32;
        let skills = array(field(&row, "skills"), "skills");
        let primary = skills.first();
        let primary_id = primary.map_or(0, |skill| skill_id(string(field(skill, "skill"), "skill")));
        let primary_min = primary
            .map_or(0, |skill| {
                integer(field(skill, "min"), "skill min") as i32 - offset
            })
            .clamp(0, i32::from(u16::MAX));
        let row_needs = system_needs | needs_mask(row.get("needs"));
        writeln!(out, "        CraftCatalogueDefinitionRow {{").unwrap();
        writeln!(out, "            row: CraftCatalogueRow {{").unwrap();
        writeln!(out, "                button: {},", 3 + index as u32 * 7).unwrap();
        writeln!(out, "                result: Graphic({graphic}),").unwrap();
        writeln!(out, "                result_hue: Hue({hue}),").unwrap();
        writeln!(
            out,
            "                result_item_kind: {},",
            option_u32(kind, "ItemKindId")
        )
        .unwrap();
        writeln!(out, "                name: ClilocId({name}),").unwrap();
        writeln!(
            out,
            "                skill: ClilocId({}),",
            1_044_060u32 + u32::from(primary_id)
        )
        .unwrap();
        writeln!(out, "                skill_min: {primary_min}u16,").unwrap();
        out.push_str("                ready: false,\n                weapon: None,\n                components: vec![\n");
        for (selector, component_name, shown_material, resource) in components {
            let key = selector_ids[&selector];
            let component_kind = selector.kind;
            let component_hue = selector.hue;
            writeln!(out, "                    CraftCatalogueComponent {{").unwrap();
            writeln!(out, "                        stock_key: CraftKey({key}u16),").unwrap();
            writeln!(
                out,
                "                        item_kind: {},",
                option_u32(component_kind, "ItemKindId")
            )
            .unwrap();
            writeln!(
                out,
                "                        material: {},",
                option_u16(shown_material, "MaterialId")
            )
            .unwrap();
            writeln!(
                out,
                "                        graphic: Graphic({}),",
                selector.graphic
            )
            .unwrap();
            writeln!(out, "                        hue: Hue({component_hue}),").unwrap();
            writeln!(out, "                        name: ClilocId({component_name}),").unwrap();
            writeln!(
                out,
                "                        amount: {}u16,",
                integer(field(&resource, "amount"), "resource amount")
            )
            .unwrap();
            out.push_str("                    },\n");
        }
        out.push_str("                ],\n            },\n            skill_requirements: vec![\n");
        for skill in skills {
            let id = skill_id(string(field(skill, "skill"), "skill"));
            let minimum =
                (integer(field(skill, "min"), "skill min") as i32 - offset).clamp(0, i32::from(u16::MAX));
            writeln!(
                out,
                "                CraftSkillRequirement {{ skill: {id}, minimum: {minimum}u16 }},"
            )
            .unwrap();
        }
        writeln!(
            out,
            "            ],\n            needs: {row_needs}u8,\n        }},"
        )
        .unwrap();
    }
    out.push_str("    ]\n}\n");

    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("output directory"));
    fs::write(output.join("craft_catalogue.rs"), out).expect("write craft catalogue artifact");
    write_house_catalogue(&output, &items, &material_entries);
}

fn write_house_catalogue(output: &Path, items: &[ItemInfo], materials: &[MaterialInfo]) {
    let mut out = String::from("pub const HOUSE_ITEM_CATALOGUE: &[HouseCatalogueEntry] = &[\n");
    for item in items {
        let graphic = item.graphics[0];
        write_house_entry(&mut out, item, None, &item.name, graphic, 0);
        if let Some(family) = &item.family {
            for material in materials.iter().filter(|material| &material.family == family) {
                let name = format!("{} {}", material.name, item.name);
                write_house_entry(&mut out, item, Some(material.id), &name, graphic, material.hue);
            }
        }
    }
    out.push_str("];\n");
    fs::write(output.join("house_item_catalogue.rs"), out).expect("write house item catalogue artifact");
}

fn write_house_entry(
    out: &mut String,
    item: &ItemInfo,
    material: Option<u16>,
    name: &str,
    graphic: u16,
    hue: u16,
) {
    writeln!(out, "    HouseCatalogueEntry {{").unwrap();
    writeln!(
        out,
        "        identity: HouseItemIdentity::Semantic {{ kind: ItemKindId({}), material: {} }},",
        item.id,
        option_u16(material, "MaterialId")
    )
    .unwrap();
    writeln!(out, "        name: {name:?},").unwrap();
    out.push_str("        tags: &[");
    for tag in &item.tags {
        write!(out, "{tag:?},").unwrap();
    }
    if let Some(family) = &item.family {
        write!(out, "{family:?},").unwrap();
    }
    out.push_str("],\n");
    writeln!(out, "        graphic: Graphic({graphic}),").unwrap();
    writeln!(out, "        hue: Hue({hue}),\n    }},").unwrap();
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())))
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value.get(name).unwrap_or_else(|| panic!("missing {name}"))
}

fn array<'a>(value: &'a Value, name: &str) -> &'a [Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{name} is not an array"))
}

fn string<'a>(value: &'a Value, name: &str) -> &'a str {
    value.as_str().unwrap_or_else(|| panic!("{name} is not a string"))
}

fn integer(value: &Value, name: &str) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| panic!("{name} is not an integer"))
}

fn bool_field(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

fn hex(raw: &str) -> u16 {
    u16::from_str_radix(raw.strip_prefix("0x").expect("hex value starts with 0x"), 16)
        .unwrap_or_else(|error| panic!("invalid hex {raw}: {error}"))
}

fn cliloc(value: Option<&Value>) -> u32 {
    value.and_then(Value::as_u64).unwrap_or(0) as u32
}

fn option_u32(value: Option<u32>, type_name: &str) -> String {
    value.map_or_else(
        || "None".to_owned(),
        |value| format!("Some({type_name}({value}))"),
    )
}

fn option_u16(value: Option<u16>, type_name: &str) -> String {
    value.map_or_else(
        || "None".to_owned(),
        |value| format!("Some({type_name}({value}))"),
    )
}

fn parse_items(value: &Value) -> Vec<ItemInfo> {
    array(value, "items")
        .iter()
        .map(|item| {
            let mut graphics = vec![hex(string(field(item, "graphic"), "item graphic"))];
            if let Some(legacy) = item.get("legacy_graphics") {
                graphics.extend(
                    array(legacy, "legacy graphics")
                        .iter()
                        .map(|value| hex(string(value, "legacy graphic"))),
                );
            }
            ItemInfo {
                id: integer(field(item, "id"), "item id") as u32,
                name: string(field(item, "name"), "item name").to_owned(),
                family: item
                    .get("material_family")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                tags: item
                    .get("tags")
                    .map(|tags| {
                        array(tags, "item tags")
                            .iter()
                            .map(|tag| string(tag, "item tag").to_owned())
                            .collect()
                    })
                    .unwrap_or_else(Vec::new),
                graphics,
            }
        })
        .collect()
}

fn parse_material_entries(value: &Value) -> Vec<MaterialInfo> {
    array(value, "materials")
        .iter()
        .map(|material| {
            MaterialInfo {
                id:     integer(field(material, "id"), "material id") as u16,
                family: string(field(material, "family"), "material family").to_owned(),
                name:   string(field(material, "name"), "material name").to_owned(),
                hue:    hex(string(field(material, "hue"), "material hue")),
            }
        })
        .collect()
}

fn inferred_semantic(
    graphic: u16,
    hue: u16,
    items: &[ItemInfo],
    materials: &BTreeMap<(String, u16), u16>,
) -> (Option<u32>, Option<u16>) {
    let mut matches = items.iter().filter(|item| item.graphics.contains(&graphic));
    let Some(item) = matches.next() else {
        return (None, None);
    };
    if matches.next().is_some() {
        return (None, None);
    }
    let material = item
        .family
        .as_ref()
        .and_then(|family| materials.get(&(family.clone(), hue)).copied());
    if item.family.is_some() && material.is_none() {
        return (None, None);
    }
    (Some(item.id), material)
}

fn resource_selector(
    resource: &Value,
    axis: Option<&Value>,
    items: &[ItemInfo],
    materials: &BTreeMap<(String, u16), u16>,
) -> (StockSelector, u32, Option<u16>) {
    let graphic = hex(string(field(resource, "graphic"), "resource graphic"));
    let mut hue = resource
        .get("hue")
        .map_or(0, |value| hex(string(value, "resource hue")));
    let from_axis = bool_field(resource.get("from_axis"));
    let mut semantic = None;
    let mut shown_material = None;
    let mut name = cliloc(resource.get("name"));
    if from_axis {
        if let Some(axis) = axis {
            let entry = array(field(axis, "entries"), "axis entries")
                .first()
                .expect("axis has a default entry");
            hue = hex(string(field(entry, "hue"), "axis hue"));
            name = cliloc(entry.get("name"));
            let material = integer(field(entry, "material"), "axis material") as u16;
            shown_material = Some(material);
            semantic = Some((
                integer(field(axis, "item_kind"), "axis item kind") as u32,
                Some(material),
            ));
        }
    } else if let Some(selector) = resource.get("selector") {
        match string(field(selector, "type"), "selector type") {
            "exact" => semantic = Some((integer(field(selector, "kind"), "selector kind") as u32, None)),
            "kind_with_material" => {
                let kind = integer(field(selector, "kind"), "selector kind") as u32;
                let material = selector
                    .get("material")
                    .and_then(Value::as_u64)
                    .map(|value| value as u16);
                shown_material = material;
                semantic = Some((kind, material));
            }
            kind => panic!("unsupported catalogue selector {kind}"),
        }
    }
    let (kind, material) = match semantic {
        Some((kind, material)) => (Some(kind), material),
        None => inferred_semantic(graphic, hue, items, materials),
    };
    (
        StockSelector {
            kind,
            material,
            graphic,
            hue,
        },
        name,
        shown_material.or(material),
    )
}

fn needs_mask(value: Option<&Value>) -> u8 {
    let Some(value) = value else { return 0 };
    u8::from(bool_field(value.get("forge")))
        | (u8::from(bool_field(value.get("anvil"))) << 1)
        | (u8::from(bool_field(value.get("heat"))) << 2)
        | (u8::from(bool_field(value.get("oven"))) << 3)
        | (u8::from(bool_field(value.get("mill"))) << 4)
        | (u8::from(bool_field(value.get("water"))) << 5)
}

fn skill_id(name: &str) -> u8 {
    match name {
        "Alchemy" => 0,
        "Blacksmith" => 7,
        "Fletching" => 8,
        "Carpentry" => 11,
        "Magery" => 25,
        "Musicianship" => 29,
        "Tailoring" => 34,
        "Tinkering" => 37,
        "AnimalLore" => 2,
        other => panic!("unknown craft skill {other}"),
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
