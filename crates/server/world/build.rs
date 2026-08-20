//! Turns `data/*.json` into the spawn regions and the decoration this crate
//! ships — between them, everything on a facet that the map itself does not draw.
//!
//! The `build.rs` in `world`, following the conventions `state/build.rs` sets
//! out: serde structs live here rather than in the crate, every one
//! `deny_unknown_fields`, the doc comments for generated items are `const`s in
//! this file, and the invariants are checked here so a failure names the JSON
//! rather than surfacing as a quiet oddity in a running shard.
//!
//! Both files factor their repetition into a named table and refer to it —
//! creatures by name, door hinges by graphic — and this script expands the
//! references. What the runtime sees is unchanged; what a person reads is a file
//! that says each thing once.
//!
//! # Why the creatures are a named table and not written where they are used
//!
//! Felucca's 1,430 spawn regions reference 8,338 creatures, and there are **193
//! distinct ones**. Written inline that is 8,338 copies of an eight-field struct
//! in the data and something like 150,000 lines in the generated source — a
//! compile-time cost and a diff nobody can read, for a file that is 97.7%
//! repetition.
//!
//! So `data/spawns.json` has two halves: a `creatures` table keyed by name, and
//! spawners that list the names they may put down. This script resolves the
//! references, and a name with no entry is a build failure that says which
//! spawner asked for it. The generated source is one `CreatureTemplate` literal
//! per reference — the resolution happens here, so nothing is looked up at
//! runtime and `Spawner` is unchanged.
//!
//! The names are authored, not derived. Most come from the same creature the
//! engine's own `creature_name` table knows; the rest are `body 0x00e8` and say
//! so, because that body genuinely has no name in the tree yet — which is also
//! why those creatures single-click with no label in game.
//!
//! # Ten regions used to start off the map, and nobody could have known
//!
//! The converter's output had ten regions with a negative `x` or `y`. The engine
//! never saw one: the script boundary turned it into `0` on the way in, keeping
//! the width and height, so the box was quietly shifted onto the map rather than
//! clipped. This data is what the engine *received*, so those ten carry the
//! zeroes, and the `u16` here means the case cannot come back.
//!
//! It is worth knowing which of the two a fix would be. Clipping instead —
//! keeping the far edge and taking the overhang off the size — is the more
//! defensible geometry, and it is a **change to where creatures spawn**, so it
//! belongs in a commit that says so rather than riding in on a data move.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::Deserialize;

/// The doc over the generated `shipped`.
const SPAWNS_DOC: &str = "\
/// Every spawn set the shard ships, built fresh from `data/spawns.json`.
///
/// Ported from ServUO's `Spawner` placements, one region per `spawner.map`
/// entry, with each creature's stats read off the `BaseCreature` subclass it
/// names.
///
/// **Keyed by an admin verb**, like `region::shipped`: `populate:felucca` is a
/// button in the staff menu and a `--seed` argument, because an operator lays and
/// clears a facet's population by hand. The verb travels with the data rather
/// than being spelled into the server.
///
/// The `Spawner`s come out with `id: 0` and `next_spawn: 0`, which is not a
/// value — it is the same placeholder the script bridge passes.
/// [`World::register_spawner`](crate::World) assigns the real id and jitters the
/// first spawn across the respawn window, and it does that for *any* caller, so
/// this side has neither to give.
///
/// Written in the file's order. Nothing reads a spawner by position — the id is
/// assigned on registration and the de-duplication is by `SpawnArea` — so the
/// order is free, and keeping the file's makes a diff against the JSON legible.
";

/// One set in `data/spawns.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnFile {
    /// The admin verb that lays it — `world::admin`'s `ROWS` is the other half.
    verb: String,
    /// Which facet every region in the set belongs to.
    ///
    /// One facet per set, not per region: a set *is* a facet's population, and
    /// the verb says which ("populate:felucca"). A second facet is a second set.
    facet: u8,
    /// The distinct creatures, by the name the spawners refer to them by.
    ///
    /// A `BTreeMap` so the generated source does not reorder when serde's map
    /// iteration does — the emitted code is diffed by people.
    creatures: BTreeMap<String, Creature>,
    /// The regions, each listing creature names from the table above.
    spawners: Vec<SpawnerDef>,
}

/// One creature a region may put down.
///
/// Mirrors `CreatureTemplate`, with everything ServUO leaves at a default left
/// out of the data. The two that are *not* plain `Default` are called out below.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Creature {
    /// The body graphic.
    body: u16,
    /// Its hue. Zero — no hue — for every creature so far.
    #[serde(default)]
    hue: u16,
    /// Starting and maximum hit points.
    #[serde(default = "one")]
    hits: u16,
    /// The health-bar colour, as the wire value.
    #[serde(default = "innocent")]
    notoriety: u8,
    /// Melee damage before resistance.
    #[serde(default)]
    damage: u16,
    /// Physical resistance, a percentage.
    #[serde(default)]
    resistance: u8,
    /// How widely known it is.
    #[serde(default)]
    fame: i32,
    /// Which way. Negative is evil.
    #[serde(default)]
    karma: i32,
    /// Swing cadence in ticks; `0` derives it from dexterity.
    #[serde(default)]
    swing: u64,
    /// How far it notices a target; `0` for a placid animal.
    #[serde(default)]
    sight: u8,
    /// Whether it starts fights. Left out, it is [`natural_aggression`]'s answer
    /// for the body — the rule the script bridge applied, moved here with it.
    #[serde(default)]
    aggression: Option<u8>,
    /// Ticks between beats while hunting; `0` takes the shard default.
    #[serde(default)]
    beat: u64,
    /// Its ranged reach, if it has one.
    #[serde(default)]
    ranged: Option<u8>,
    /// The ranged attack's damage type, as the wire value.
    #[serde(default)]
    ranged_kind: u8,
    /// Whether it drifts when idle.
    #[serde(default)]
    wander: bool,
    /// Trained combat skills, `(skill id, value in tenths)`.
    #[serde(default)]
    skills: Vec<(u8, u16)>,
}

/// A creature's default hit points: one. `u16`'s own default is zero, which is a
/// creature that is dead where it stands.
const fn one() -> u16 {
    1
}

/// A creature's default notoriety: `Innocent`, the wire value 1.
const fn innocent() -> u8 {
    1
}

/// The posture a creature gets when the data does not set one.
///
/// Carried over verbatim from the script bridge's `default_aggression`, which is
/// where this rule lived while the spawn data was a pack's. Ordinary horses are
/// tameable mounts, not monsters: a body that is one must not hunt nearby players
/// merely because nobody wrote a field. Every other body keeps the historic
/// aggressive default, and the data can always say otherwise.
const fn natural_aggression(body: u16) -> u8 {
    match body {
        // Aggression::Passive.
        0x00C8 | 0x00CC | 0x00E2 | 0x00E4 => 0,
        // Aggression::Aggressive.
        _ => 2,
    }
}

/// One spawn region.
///
/// No `id` and no `next_spawn`: both belong to the live spawner rather than to
/// the content. `register_spawner` assigns the id and jitters the first spawn,
/// and a number written here would be a second source for either.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnerDef {
    /// West edge.
    x: u16,
    /// North edge.
    y: u16,
    /// Width in tiles.
    width: u16,
    /// Height in tiles.
    height: u16,
    /// The most live creatures it keeps.
    max_count: u16,
    /// Ticks to wait after a spawn before the next.
    respawn_delay: u64,
    /// Which creatures, by name into the file's table.
    creatures: Vec<String>,
}

/// The Rust expression for one creature, fully qualified — the generated file is
/// `include!`d into a module whose imports it cannot see.
///
/// The wire-value conversions are the total ones (`from_bits`, `from_u8`), which
/// fold anything unrecognised to a safe default rather than failing. That would
/// turn a typo into a blue health bar, so the *range* checks are asserts in
/// [`spawns`] instead, where the message can name the creature.
fn creature_expr(name: &str, c: &Creature, indent: &str) -> String {
    let aggression = c.aggression.unwrap_or_else(|| natural_aggression(c.body));
    let ranged = match c.ranged {
        Some(range) => format!(
            "Some(openshard_protocol::world::RangedRange::new({range}).expect({:?}))",
            format!("{name} has a ranged reach of zero, which is no reach at all")
        ),
        None => "None".to_owned(),
    };
    let skills: Vec<String> = c
        .skills
        .iter()
        .map(|(id, value)| {
            format!(
                "(openshard_state::Skill::from_id({id}).expect({:?}), {value})",
                format!("{name} names skill id {id}, which is not a skill")
            )
        })
        .collect();
    let skills = if skills.is_empty() {
        "Vec::new()".to_owned()
    } else {
        format!("vec![{}]", skills.join(", "))
    };

    let mut out = String::new();
    // The name is a comment rather than a field: `CreatureTemplate` has no room
    // for one, and a table of 193 anonymous stat blocks is unreadable.
    writeln!(out, "{indent}// {name}").unwrap();
    writeln!(out, "{indent}crate::spawner::CreatureTemplate {{").unwrap();
    writeln!(
        out,
        "{indent}    body: openshard_protocol::wire::Graphic({}),",
        c.body
    )
    .unwrap();
    writeln!(out, "{indent}    hue: openshard_protocol::wire::Hue({}),", c.hue).unwrap();
    writeln!(out, "{indent}    hits: {},", c.hits).unwrap();
    writeln!(
        out,
        "{indent}    notoriety: openshard_protocol::mobile::Notoriety::from_bits({}),",
        c.notoriety
    )
    .unwrap();
    writeln!(out, "{indent}    damage: {},", c.damage).unwrap();
    writeln!(
        out,
        "{indent}    resistance: openshard_protocol::world::PhysicalResistance::new({}),",
        c.resistance
    )
    .unwrap();
    writeln!(out, "{indent}    fame: {},", c.fame).unwrap();
    writeln!(out, "{indent}    karma: {},", c.karma).unwrap();
    writeln!(out, "{indent}    swing: {},", c.swing).unwrap();
    writeln!(
        out,
        "{indent}    sight: openshard_protocol::world::Sight({}),",
        c.sight
    )
    .unwrap();
    writeln!(
        out,
        "{indent}    aggression: openshard_protocol::world::Aggression::from_bits({aggression}),"
    )
    .unwrap();
    writeln!(out, "{indent}    beat: {},", c.beat).unwrap();
    writeln!(out, "{indent}    ranged: {ranged},").unwrap();
    writeln!(
        out,
        "{indent}    ranged_kind: openshard_protocol::world::DamageType::from_u8({}),",
        c.ranged_kind
    )
    .unwrap();
    writeln!(out, "{indent}    wander: {},", c.wander).unwrap();
    writeln!(out, "{indent}    skills: {skills},").unwrap();
    write!(out, "{indent}}}").unwrap();
    out
}

/// `data/spawns.json` into the `shipped` constructor.
fn spawns(text: &str) -> String {
    let file: SpawnFile = serde_json::from_str(text).expect("spawns.json");

    assert!(
        !file.verb.is_empty(),
        "a spawn set with no verb can never be laid"
    );
    assert!(
        !file.spawners.is_empty(),
        "spawn set {:?} lays no regions at all",
        file.verb
    );

    // A creature nothing spawns is dead weight that reads as content. Collected
    // before the emit so the message can name all of them at once.
    let referenced: std::collections::BTreeSet<&str> = file
        .spawners
        .iter()
        .flat_map(|s| s.creatures.iter().map(String::as_str))
        .collect();
    let orphans: Vec<&str> = file
        .creatures
        .keys()
        .map(String::as_str)
        .filter(|name| !referenced.contains(name))
        .collect();
    assert!(
        orphans.is_empty(),
        "spawns.json defines creatures no region spawns: {}",
        orphans.join(", ")
    );

    // Each creature's range checks, once per definition rather than once per
    // reference — the conversions the emitted code uses are total and would fold
    // a typo into a safe default instead of failing.
    for (name, c) in &file.creatures {
        assert!(
            c.hits > 0,
            "{name} has no hit points, so it is dead where it stands"
        );
        assert!(
            (1..=7).contains(&c.notoriety),
            "{name} has notoriety {}, which is not a wire value — the health bar would \
             silently read innocent",
            c.notoriety
        );
        assert!(
            c.aggression.is_none_or(|a| a <= 2),
            "{name} has an aggression that is not 0, 1 or 2, and anything else reads as \
             aggressive"
        );
        assert!(
            c.ranged_kind <= 4,
            "{name} has a ranged damage kind that is not a wire value, and anything else \
             reads as physical"
        );
        assert!(
            c.resistance <= 100,
            "{name} resists {}% of physical damage",
            c.resistance
        );
    }

    // The table, once. Emitting a literal per *reference* would be 8,338 of them
    // and something like 150,000 lines of source for rustc to chew through; the
    // spawners index into this instead and clone what they name.
    let index: BTreeMap<&str, usize> = file
        .creatures
        .keys()
        .enumerate()
        .map(|(i, name)| (name.as_str(), i))
        .collect();

    let mut out = String::from("// @generated by build.rs from data/spawns.json.\n\n");
    out.push_str(
        "/// Every distinct creature `data/spawns.json` defines, in the order the file\n\
         /// lists them. Built once per call and indexed into by the spawners below —\n\
         /// 1,430 regions name 8,338 creatures between them and there are 193 of them,\n\
         /// so a literal per reference would be almost all repetition.\n",
    );
    out.push_str("fn creature_table() -> Vec<crate::spawner::CreatureTemplate> {\n    vec![\n");
    for (name, creature) in &file.creatures {
        out.push_str(&creature_expr(name, creature, "        "));
        out.push_str(",\n");
    }
    out.push_str("    ]\n}\n\n");

    out.push_str(SPAWNS_DOC);
    out.push_str("#[must_use]\npub fn shipped() -> Vec<SpawnSet> {\n");
    out.push_str("    let c = creature_table();\n    vec![SpawnSet {\n");
    writeln!(out, "        verb: {:?}.to_owned(),", file.verb).unwrap();
    out.push_str("        spawners: vec![\n");

    for spawner in &file.spawners {
        assert!(
            spawner.width > 0 && spawner.height > 0,
            "a spawn region at {},{} is {}x{}, which contains no tile to spawn on",
            spawner.x,
            spawner.y,
            spawner.width,
            spawner.height
        );
        assert!(
            spawner.max_count > 0,
            "the spawn region at {},{} keeps no creatures alive, so it does nothing",
            spawner.x,
            spawner.y
        );
        assert!(
            !spawner.creatures.is_empty(),
            "the spawn region at {},{} has nothing to spawn",
            spawner.x,
            spawner.y
        );

        let picks: Vec<String> = spawner
            .creatures
            .iter()
            .map(|name| {
                let at = index.get(name.as_str()).unwrap_or_else(|| {
                    panic!(
                        "the spawn region at {},{} spawns {name:?}, which no creature in \
                         spawns.json is called",
                        spawner.x, spawner.y
                    )
                });
                format!("c[{at}].clone()")
            })
            .collect();

        out.push_str("            crate::spawner::Spawner::new(\n");
        // The placeholder id, overwritten by `register_spawner`.
        out.push_str("                0,\n");
        writeln!(
            out,
            "                crate::spawner::SpawnArea {{ x: {}, y: {}, width: {}, height: {}, \
             facet: openshard_protocol::world::Facet({}) }},",
            spawner.x, spawner.y, spawner.width, spawner.height, file.facet
        )
        .unwrap();
        // Wrapped rather than one index per line: these are 8,338 short
        // expressions, and a line each is the file size this table exists to avoid.
        out.push_str("                vec![");
        let mut column = 0;
        for (i, pick) in picks.iter().enumerate() {
            if column > 0 && column + pick.len() > 88 {
                out.push_str("\n                     ");
                column = 21;
            }
            out.push_str(pick);
            column += pick.len();
            if i + 1 < picks.len() {
                out.push_str(", ");
                column += 2;
            }
        }
        out.push_str("],\n");
        writeln!(out, "                {},", spawner.max_count).unwrap();
        writeln!(out, "                {},", spawner.respawn_delay).unwrap();
        out.push_str("            ),\n");
    }

    out.push_str("        ],\n    }]\n}\n");
    out
}

/// The doc over the generated `decoration::shipped`.
const DECO_DOC: &str = "\
/// Everything the shard lays on a facet that is not terrain: the statics a
/// building needs beyond its map art, the doors that open, the containers that
/// hold something, and the boxes `doorgen` scans for the shop doors the art only
/// implies.
///
/// Ported from ServUO's `Decorate.cs` output, the same `Static`/`Door`/`Container`
/// rows it places on a `[decorate`.
///
/// **`const` slices, unlike the other three datasets here.** Quests, speech,
/// regions and spawns are all replaced wholesale in something that owns them, so
/// each is built fresh. Decoration is read once and copied into a command, and it
/// is twenty-five thousand rows — so it stays static data and the copy happens at
/// the one call site, where it is a `to_vec` rather than twenty-five thousand
/// allocations at every build of the table.
";

/// One `[graphic, x, y, z]` row of `data/deco.json`'s statics.
///
/// A tuple rather than an object, alone among the data files here, because there
/// are 18,832 of them and four keys repeated 18,832 times is three quarters of
/// the file's bytes spent saying `graphic` again. The order is the one the
/// `Command::Decorate` payload uses.
type StaticRow = (u16, u16, u16, i8);

/// One `[closed, x, y, z]` door row of `data/deco.json`.
///
/// **Neither the open graphic nor the hinge offset is written per door**, and the
/// two are left out for different reasons.
///
/// `open` is `closed + 1` for every door ServUO places, because that is the
/// door-family layout itself: a leaf is followed by its opened twin. Derived, and
/// so it cannot drift.
///
/// The hinge offset comes from `door_hinges`, keyed by the closed graphic —
/// eighty rows for 638 doors. That the offset is a *function* of the graphic is
/// an observed fact about this data, not a rule, so it is checked: two doors of
/// one graphic hanging different ways is a build failure. It is emphatically not
/// derivable by arithmetic — only sixteen of the eighty match what
/// [`crate::doorgen`]'s `OFFSETS` computes for their facing, because ServUO keeps
/// a door's facing on the placed object. `doorgen` derives offsets for the doors
/// *it* generates from map frames, which is a different population; the two do
/// not duplicate each other, which is what a reading of this data first suggests.
type DoorRow = (u16, u16, u16, i8);

/// One container in `data/deco.json` — a town chest or crate that opens onto a
/// gump.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Container {
    /// The item graphic.
    graphic: u16,
    /// The gump the client opens for it.
    gump: u16,
    x: u16,
    y: u16,
    z: i8,
    /// Its hue, or 0.
    #[serde(default)]
    hue: u16,
    /// Which key opens it; `0` is unlocked.
    #[serde(default)]
    key_value: u32,
}

/// One box `doorgen` scans for implied shop doors.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DoorRegion {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

/// `data/deco.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecoFile {
    /// The admin verb that lays it.
    verb: String,
    /// Which facet all of it belongs to.
    facet: u8,
    /// Which way each door graphic's leaf swings, by closed graphic. See
    /// [`DoorRow`].
    door_hinges: BTreeMap<u16, (i16, i16)>,
    statics: Vec<StaticRow>,
    doors: Vec<DoorRow>,
    containers: Vec<Container>,
    door_regions: Vec<DoorRegion>,
}

/// The component tiles that make up one ServUO `BaseAddon`.
///
/// `deco.json` predates this distinction and has one row for the graphic on a
/// `.cfg` type line.  The corresponding addon may place several tiles, and its
/// root graphic is not necessarily at offset zero.  Keep the source's type and
/// component layout in `deco_addons.json`, then expand it here with the ordinary
/// statics the world command already understands.
type AddonComponent = (u16, i16, i16, i8);

/// The flattened row the old converter wrote for one addon instance.
type AddonInstance = (u16, u16, u16, i8);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddonFile {
    addons: Vec<Addon>,
}

/// A multi-tile ServUO addon and every place this facet puts it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Addon {
    /// The ServUO class name, retained so an import remains auditable.
    name: String,
    /// `[graphic, dx, dy, dz]`, exactly as `AddComponent` receives it.
    components: Vec<AddonComponent>,
    /// `[flattened graphic, x, y, z]` rows that the former importer emitted.
    instances: Vec<AddonInstance>,
}

/// `data/deco.json` into four `const` slices and the set that names them.
///
/// # What is *not* checked here
///
/// Duplicates. Thirty-nine statics repeat an exact graphic and position, and 1,471
/// tiles hold more than one static — both are ordinary in UO decoration, where a
/// tile carries a floor, a rug and what stands on the rug. Rejecting either would
/// reject the data ServUO itself produces. The question a second press of the
/// button raises is answered in `tick::decor`, against the world rather than
/// against the file.
fn deco(text: &str, addons_text: &str) -> String {
    let file: DecoFile = serde_json::from_str(text).expect("deco.json");
    let addon_file: AddonFile = serde_json::from_str(addons_text).expect("deco_addons.json");

    assert!(
        !file.verb.is_empty(),
        "a decoration set with no verb can never be laid"
    );
    assert!(
        !file.statics.is_empty() || !file.doors.is_empty() || !file.containers.is_empty(),
        "decoration set {:?} lays nothing",
        file.verb
    );

    // The old converter emitted one static for every addon, using the graphic
    // written on the .cfg type line.  Remove those placeholders before emitting
    // the complete component layout below.  The assertion makes a new import
    // fail loudly instead of silently returning to one-tile furniture.
    let static_rows: BTreeSet<StaticRow> = file.statics.iter().copied().collect();
    let mut addon_roots = BTreeSet::new();
    for addon in &addon_file.addons {
        assert!(!addon.name.is_empty(), "an unnamed addon cannot be audited");
        assert!(
            addon.components.len() > 1,
            "{} is not multi-tile and does not belong in deco_addons.json",
            addon.name
        );
        for &(graphic, x, y, z) in &addon.instances {
            assert!(
                static_rows.contains(&(graphic, x, y, z)),
                "{} at {x},{y},{z} is absent from deco.json",
                addon.name
            );
            assert!(
                addon_roots.insert((graphic, x, y, z)),
                "more than one addon claims graphic {graphic} at {x},{y},{z}"
            );
        }
    }

    let mut out =
        String::from("// @generated by build.rs from data/deco.json and data/deco_addons.json.\n\n");

    out.push_str(
        "/// The plain statics, as the `Command::Decorate` payload wants them.\n\
         /// Hueless: no decoration ServUO places carries one.\n",
    );
    out.push_str(
        "const STATICS: &[(openshard_protocol::wire::Graphic, openshard_protocol::wire::Hue, \
         openshard_protocol::world::Point)] = &[\n",
    );
    for &(graphic, x, y, z) in &file.statics {
        if addon_roots.contains(&(graphic, x, y, z)) {
            continue;
        }
        writeln!(
            out,
            "    (openshard_protocol::wire::Graphic({graphic}), openshard_protocol::wire::Hue(0), \
             openshard_protocol::world::Point::new({x}, {y}, {z})),"
        )
        .unwrap();
    }
    for addon in &addon_file.addons {
        for &(_, x, y, z) in &addon.instances {
            for &(graphic, dx, dy, dz) in &addon.components {
                let x = i32::from(x) + i32::from(dx);
                let y = i32::from(y) + i32::from(dy);
                let z = i16::from(z) + i16::from(dz);
                assert!(
                    (0..=i32::from(u16::MAX)).contains(&x)
                        && (0..=i32::from(u16::MAX)).contains(&y)
                        && (i16::from(i8::MIN)..=i16::from(i8::MAX)).contains(&z),
                    "{} has a component outside the world at {x},{y},{z}",
                    addon.name
                );
                writeln!(
                    out,
                    "    (openshard_protocol::wire::Graphic({graphic}), openshard_protocol::wire::Hue(0), openshard_protocol::world::Point::new({x}, {y}, {z})),"
                )
                .unwrap();
            }
        }
    }
    out.push_str("];\n\n");

    // A hinge nothing hangs on is a row that reads as content and is not, the
    // same check the creature table gets.
    let hung: std::collections::BTreeSet<u16> = file.doors.iter().map(|&(closed, ..)| closed).collect();
    let unused: Vec<String> = file
        .door_hinges
        .keys()
        .filter(|graphic| !hung.contains(graphic))
        .map(u16::to_string)
        .collect();
    assert!(
        unused.is_empty(),
        "deco.json gives a hinge to door graphics it never places: {}",
        unused.join(", ")
    );

    out.push_str(
        "/// The doors, with the open graphic derived as `closed + 1` and the hinge\n\
         /// looked up by graphic — see `build.rs`'s `DoorRow` for why each is not\n\
         /// written per door.\n",
    );
    out.push_str("const DOORS: &[crate::DecorDoor] = &[\n");
    for &(closed, x, y, z) in &file.doors {
        assert!(
            closed < u16::MAX,
            "the door at {x},{y} is graphic {closed}, which has no room for an opened twin"
        );
        let &(offset_x, offset_y) = file.door_hinges.get(&closed).unwrap_or_else(|| {
            panic!("the door at {x},{y} is graphic {closed}, which door_hinges does not cover")
        });
        writeln!(
            out,
            "    crate::DecorDoor {{ key_value: 0, \
             closed: openshard_protocol::wire::Graphic({closed}), \
             open: openshard_protocol::wire::Graphic({}), \
             offset_x: {offset_x}, offset_y: {offset_y}, \
             position: openshard_protocol::world::Point::new({x}, {y}, {z}) }},",
            closed + 1
        )
        .unwrap();
    }
    out.push_str("];\n\n");

    out.push_str("/// The containers.\n");
    out.push_str("const CONTAINERS: &[crate::DecorContainer] = &[\n");
    for c in &file.containers {
        writeln!(
            out,
            "    crate::DecorContainer {{ key_value: {}, \
             graphic: openshard_protocol::wire::Graphic({}), \
             gump: openshard_protocol::wire::Graphic({}), \
             hue: openshard_protocol::wire::Hue({}), \
             position: openshard_protocol::world::Point::new({}, {}, {}) }},",
            c.key_value, c.graphic, c.gump, c.hue, c.x, c.y, c.z
        )
        .unwrap();
    }
    out.push_str("];\n\n");

    out.push_str(
        "/// The boxes `doorgen` scans, as `(x, y, width, height)`. Laid after the\n\
         /// statics, because a generated door goes in a gap between frames the\n\
         /// decoration may have just put there.\n",
    );
    out.push_str("const DOOR_REGIONS: &[(u16, u16, u16, u16)] = &[\n");
    for r in &file.door_regions {
        assert!(
            r.width > 0 && r.height > 0,
            "a door-generation box at {},{} is {}x{} and holds no doorway",
            r.x,
            r.y,
            r.width,
            r.height
        );
        writeln!(out, "    ({}, {}, {}, {}),", r.x, r.y, r.width, r.height).unwrap();
    }
    out.push_str("];\n\n");

    out.push_str(DECO_DOC);
    out.push_str("#[must_use]\npub fn shipped() -> Vec<DecorSet> {\n    vec![DecorSet {\n");
    writeln!(out, "        verb: {:?},", file.verb).unwrap();
    writeln!(
        out,
        "        facet: openshard_protocol::world::Facet({}),",
        file.facet
    )
    .unwrap();
    out.push_str(
        "        statics: STATICS,\n        doors: DOORS,\n        containers: CONTAINERS,\n\
         \x20       door_regions: DOOR_REGIONS,\n    }]\n}\n",
    );
    out
}

/// The doc over the generated `townsfolk::shipped`.
const TOWNSFOLK_DOC: &str = "\
/// The named townsfolk the shard places, built fresh from `data/townsfolk.json`.
///
/// The bankers, shopkeepers, guildmasters and waiting travellers a town is made
/// of — placed once, at a fixed tile, rather than maintained by a spawn region.
/// Ported from ServUO's own vendor placements, with each one's stock read off the
/// `SB*.cs` its class names.
///
/// **Each row carries its whole self**: where it stands, what it wears, what it
/// sells, and where it wants to be escorted to. That is the point of the file.
/// The same content used to be three tables joined on the tile an NPC stands on —
/// a placement here, a shelf there, an escort destination in a third — because
/// nothing outside the world could name a mobile until the world had answered
/// with its serial. Content in the tree is handed to the world as one command and
/// needs no such rendezvous.
";

/// One worn item in `data/townsfolk.json`'s outfit table.
///
/// The creature-table shape from `spawns.json`, twice over: 789 townsfolk wear
/// **fourteen** distinct outfits between them, and 443 shopkeepers sell
/// **twenty-six** distinct shelves — 10,192 stock lines that are twenty-six
/// lists. Named once, referred to by name, resolved here.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Worn {
    /// The item graphic.
    graphic: u16,
    /// Which paperdoll layer it goes on.
    layer: u8,
    /// Its hue, or 0.
    #[serde(default)]
    hue: u16,
}

/// One line of a shelf in `data/townsfolk.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stock {
    /// The goods' graphic.
    graphic: u16,
    /// Their hue.
    #[serde(default)]
    hue: u16,
    /// How many the vendor holds.
    amount: u16,
    /// What one unit costs.
    price: u32,
    /// The label the client shows.
    name: String,
}

/// One placed townsperson in `data/townsfolk.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Townsperson {
    x: u16,
    y: u16,
    z: i8,
    /// The body graphic. Four hundred of them share body 400, which is why the
    /// de-duplication in `tick.rs` is keyed by the trade and not by this.
    body: u16,
    /// The trade it plies, ServUO-style ("the blacksmith").
    #[serde(default)]
    title: Option<String>,
    #[serde(default = "one")]
    hits: u16,
    /// The health-bar colour, as the wire value.
    #[serde(default = "innocent")]
    notoriety: u8,
    /// What it wears on its feet, `ShoeType`'s wire byte.
    #[serde(default)]
    shoe: u8,
    /// Whether it answers "bank".
    #[serde(default)]
    banker: bool,
    /// Whether double-clicking it opens a shop.
    #[serde(default)]
    vendor: bool,
    /// Where it sleeps, for the optional daily routine.
    #[serde(default)]
    night_home: Option<(u16, u16, i8)>,
    /// Which outfit it wears, by name into `outfits`.
    #[serde(default)]
    outfit: Option<String>,
    /// Which shelf it sells, by name into `shelves`.
    #[serde(default)]
    shelf: Option<String>,
    /// Where it wants to be escorted. **An empty string is not nothing**: it means
    /// "wherever the quest picks", ServUO's `PickRandomDestination`, while the
    /// field being absent means it is not escortable at all.
    #[serde(default)]
    escort_to: Option<String>,
    /// The quests it offers, by key.
    #[serde(default)]
    quests: Vec<String>,
}

/// `data/townsfolk.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TownsfolkFile {
    verb: String,
    facet: u8,
    outfits: BTreeMap<String, Vec<Worn>>,
    shelves: BTreeMap<String, Vec<Stock>>,
    townsfolk: Vec<Townsperson>,
}

/// `data/townsfolk.json` into the `shipped` constructor.
fn townsfolk(text: &str) -> String {
    let file: TownsfolkFile = serde_json::from_str(text).expect("townsfolk.json");

    assert!(
        !file.verb.is_empty(),
        "a townsfolk set with no verb can never be laid"
    );
    assert!(!file.townsfolk.is_empty(), "the set places nobody");

    // A tile holding two townsfolk would make the de-duplication in `tick.rs`
    // ambiguous, and it is the shape ServUO's own placements have: nobody stands
    // on anybody.
    let mut tiles: BTreeMap<(u16, u16), &str> = BTreeMap::new();
    for person in &file.townsfolk {
        let title = person.title.as_deref().unwrap_or("");
        if let Some(first) = tiles.insert((person.x, person.y), title) {
            panic!(
                "two townsfolk stand on {},{}: {first} and {title}",
                person.x, person.y
            );
        }
        assert!(
            person.shelf.is_none() || person.vendor,
            "the {title} at {},{} has a shelf but is not a vendor, so nothing can be bought",
            person.x,
            person.y
        );
    }

    let mut out = String::from("// @generated by build.rs from data/townsfolk.json.\n\n");

    // The two tables, once each, as functions rather than consts: both hold
    // `String`s and `Vec`s the command takes ownership of.
    out.push_str("/// The distinct outfits, in the file's order.\n");
    out.push_str(
        "fn outfit_table() -> Vec<Vec<(openshard_protocol::wire::Graphic, \
         openshard_protocol::wire::Layer, openshard_protocol::wire::Hue)>> {\n    vec![\n",
    );
    for (name, items) in &file.outfits {
        writeln!(out, "        // {name}").unwrap();
        let worn: Vec<String> = items
            .iter()
            .map(|w| {
                format!(
                    "(openshard_protocol::wire::Graphic({}), openshard_protocol::wire::Layer({}), \
                     openshard_protocol::wire::Hue({}))",
                    w.graphic, w.layer, w.hue
                )
            })
            .collect();
        writeln!(out, "        vec![{}],", worn.join(", ")).unwrap();
    }
    out.push_str("    ]\n}\n\n");

    out.push_str("/// The distinct shelves, in the file's order.\n");
    out.push_str("fn shelf_table() -> Vec<Vec<openshard_npc::StockLine>> {\n    vec![\n");
    for (name, lines) in &file.shelves {
        writeln!(out, "        // {name}").unwrap();
        out.push_str("        vec![\n");
        for line in lines {
            assert!(
                line.amount > 0,
                "the {name} shelf stocks none of {:?}, so it is a listing nobody can buy",
                line.name
            );
            writeln!(
                out,
                "            openshard_npc::StockLine {{ graphic: openshard_protocol::wire::Graphic({}), \
                 hue: openshard_protocol::wire::Hue({}), amount: {}, price: {}, name: {:?}.to_owned() }},",
                line.graphic, line.hue, line.amount, line.price, line.name
            )
            .unwrap();
        }
        out.push_str("        ],\n");
    }
    out.push_str("    ]\n}\n\n");

    let outfit_at: BTreeMap<&str, usize> = file
        .outfits
        .keys()
        .enumerate()
        .map(|(i, name)| (name.as_str(), i))
        .collect();
    let shelf_at: BTreeMap<&str, usize> = file
        .shelves
        .keys()
        .enumerate()
        .map(|(i, name)| (name.as_str(), i))
        .collect();

    out.push_str(TOWNSFOLK_DOC);
    out.push_str("#[must_use]\npub fn shipped() -> Vec<TownsfolkSet> {\n");
    out.push_str("    let outfits = outfit_table();\n    let shelves = shelf_table();\n");
    out.push_str("    vec![TownsfolkSet {\n");
    writeln!(out, "        verb: {:?}.to_owned(),", file.verb).unwrap();
    out.push_str("        townsfolk: vec![\n");
    for person in &file.townsfolk {
        let title = person.title.as_deref().unwrap_or("");
        let equipment = match &person.outfit {
            Some(name) => {
                let at = outfit_at.get(name.as_str()).unwrap_or_else(|| {
                    panic!(
                        "the {title} at {},{} wears {name:?}, which no outfit is called",
                        person.x, person.y
                    )
                });
                format!("outfits[{at}].clone()")
            }
            None => "Vec::new()".to_owned(),
        };
        let stock = match &person.shelf {
            Some(name) => {
                let at = shelf_at.get(name.as_str()).unwrap_or_else(|| {
                    panic!(
                        "the {title} at {},{} sells {name:?}, which no shelf is called",
                        person.x, person.y
                    )
                });
                format!("shelves[{at}].clone()")
            }
            None => "Vec::new()".to_owned(),
        };
        let night_home = match person.night_home {
            Some((x, y, z)) => format!("Some(openshard_protocol::world::Point::new({x}, {y}, {z}))"),
            None => "None".to_owned(),
        };
        let title_expr = match &person.title {
            Some(title) => format!("Some({title:?}.to_owned())"),
            None => "None".to_owned(),
        };
        let escort = match &person.escort_to {
            Some(to) => format!("Some({to:?}.to_owned())"),
            None => "None".to_owned(),
        };

        out.push_str("            crate::Command::SpawnMobile {\n");
        writeln!(
            out,
            "                body: openshard_protocol::wire::Graphic({}),",
            person.body
        )
        .unwrap();
        out.push_str("                hue: openshard_protocol::wire::Hue(0),\n");
        writeln!(out, "                hits: {},", person.hits).unwrap();
        writeln!(
            out,
            "                notoriety: openshard_protocol::mobile::Notoriety::from_bits({}),",
            person.notoriety
        )
        .unwrap();
        out.push_str("                damage: 0,\n");
        out.push_str("                resistance: openshard_protocol::world::PhysicalResistance::new(0),\n");
        out.push_str("                swing: 0,\n");
        out.push_str("                sight: openshard_protocol::world::Sight(0),\n");
        // The same rule the creature templates get, and for the migration's
        // reason rather than a gameplay one: this is what the script bridge's
        // `default_aggression` gave a townsperson, so it is what the world has
        // always received. It never shows — a mobile with `sight: 0` notices
        // nobody, so a shopkeeper's aggressive posture has no one to act on.
        writeln!(
            out,
            "                aggression: openshard_protocol::world::Aggression::from_bits({}),",
            natural_aggression(person.body)
        )
        .unwrap();
        out.push_str("                beat: 0,\n");
        out.push_str("                ranged: None,\n");
        out.push_str("                ranged_kind: openshard_protocol::world::DamageType::Physical,\n");
        out.push_str("                wander: false,\n");
        writeln!(
            out,
            "                position: openshard_protocol::world::Point::new({}, {}, {}),",
            person.x, person.y, person.z
        )
        .unwrap();
        writeln!(
            out,
            "                facet: openshard_protocol::world::Facet({}),",
            file.facet
        )
        .unwrap();
        // The personal name is generated at spawn on the world's seeded rng, from
        // the title — see `npc::names`. Nothing here names anybody.
        out.push_str("                name: None,\n");
        writeln!(out, "                title: {title_expr},").unwrap();
        writeln!(out, "                shoe: {},", person.shoe).unwrap();
        out.push_str("                fame: 0,\n                karma: 0,\n");
        writeln!(out, "                night_home: {night_home},").unwrap();
        writeln!(out, "                banker: {},", person.banker).unwrap();
        writeln!(out, "                vendor: {},", person.vendor).unwrap();
        out.push_str("                healer: false,\n");
        writeln!(out, "                equipment: {equipment},").unwrap();
        out.push_str("                skills: Vec::new(),\n");
        writeln!(out, "                stock: {stock},").unwrap();
        writeln!(out, "                escort_to: {escort},").unwrap();
        let offers: Vec<String> = person
            .quests
            .iter()
            .map(|key| {
                assert!(
                    !key.is_empty(),
                    "the {title} at {},{} offers a quest with no key",
                    person.x,
                    person.y
                );
                format!("{key:?}.to_owned()")
            })
            .collect();
        match offers.is_empty() {
            true => out.push_str("                quests: Vec::new(),\n"),
            false => writeln!(out, "                quests: vec![{}],", offers.join(", ")).unwrap(),
        }
        out.push_str("            },\n");
    }
    out.push_str("        ],\n    }]\n}\n");
    out
}

/// The doc over the generated `loot::shipped`.
const LOOT_DOC: &str = "\
/// What a slain creature's corpse holds beyond the baseline gold, by body.
///
/// The engine lays the corpse and drops a flat gold baseline so a bare shard
/// still loots; this is the table on top of it. Sorted by body and searched, the
/// same shape `creature_name` is keyed by.
///
/// A `chance` and an `amount` range are rolled on the world's seeded rng, so a
/// replayed tick loots identically. The script pack that held this table rolled
/// `Math.random` and wrote itself an explicit exemption from that guarantee;
/// moving the data in-tree is what retires the exemption.
";

/// One drop in `data/loot.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Drop {
    /// The item tile to drop.
    graphic: u16,
    /// Its colour.
    #[serde(default)]
    hue: u16,
    /// A fixed count, or an inclusive `[min, max]` range.
    #[serde(default)]
    amount: Option<Amount>,
    /// True for gold, reagents and arrows, which merge into a pile; false for a
    /// discrete weapon or piece of armour.
    #[serde(default)]
    stackable: bool,
    /// The chance it drops at all, `0.0`–`1.0`. Absent is always.
    #[serde(default)]
    chance: Option<f64>,
}

/// A drop's count: one number, or an inclusive range.
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Amount {
    Fixed(u16),
    Range(u16, u16),
}

/// One creature's table in `data/loot.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LootTable {
    /// The body it drops for.
    body: u16,
    /// What that body is, for the reader. Not used at runtime — the engine's own
    /// `creature_name` is the label a player sees.
    creature: String,
    drops: Vec<Drop>,
}

/// `data/loot.json` into a sorted `const` table.
fn loot(text: &str) -> String {
    let mut tables: Vec<LootTable> = serde_json::from_str(text).expect("loot.json");

    // Sorted here so the lookup can binary-search, and checked for the duplicate
    // it would answer arbitrarily.
    tables.sort_by_key(|t| t.body);
    for pair in tables.windows(2) {
        assert_ne!(
            pair[0].body, pair[1].body,
            "loot.json gives body {} two tables, {} and {}",
            pair[0].body, pair[0].creature, pair[1].creature
        );
    }

    let mut out = String::from("// @generated by build.rs from data/loot.json.\n\n");
    out.push_str(LOOT_DOC);
    out.push_str("pub const SHIPPED: &[(u16, &[Drop])] = &[\n");
    for table in &tables {
        assert!(
            !table.drops.is_empty(),
            "the {} table drops nothing, which is the same as having no table",
            table.creature
        );
        writeln!(out, "    // {}", table.creature).unwrap();
        writeln!(out, "    ({}, &[", table.body).unwrap();
        for drop in &table.drops {
            let (least, most) = match drop.amount {
                None => (1, 1),
                Some(Amount::Fixed(n)) => (n, n),
                Some(Amount::Range(least, most)) => (least, most),
            };
            assert!(
                least >= 1 && least <= most,
                "a drop in the {} table asks for {least}..{most} of graphic {}",
                table.creature,
                drop.graphic
            );
            // Written as a percentage rather than a float: the roll is on the
            // world's integer rng, and a float in a `const` here would only be
            // converted back.
            let chance = match drop.chance {
                None => 100,
                Some(chance) => {
                    assert!(
                        (0.0..=1.0).contains(&chance),
                        "a drop in the {} table has a chance of {chance}",
                        table.creature
                    );
                    let percent = (chance * 100.0).round() as u32;
                    assert!(
                        percent > 0,
                        "a drop in the {} table rounds to a chance of nothing",
                        table.creature
                    );
                    percent
                }
            };
            writeln!(
                out,
                "        Drop {{ graphic: openshard_protocol::wire::Graphic({}), \
                 hue: openshard_protocol::wire::Hue({}), least: {least}, most: {most}, \
                 stackable: {}, percent: {chance} }},",
                drop.graphic, drop.hue, drop.stackable
            )
            .unwrap();
        }
        out.push_str("    ]),\n");
    }
    out.push_str("];\n");
    out
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let out_dir = Path::new(&out_dir);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data");

    for (name, render) in [
        ("spawns", spawns as fn(&str) -> String),
        ("townsfolk", townsfolk),
        ("loot", loot),
    ] {
        let path = Path::new("data").join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        std::fs::write(out_dir.join(format!("{name}.rs")), render(&text))
            .unwrap_or_else(|e| panic!("writing {name}.rs: {e}"));
    }

    let deco_path = Path::new("data").join("deco.json");
    let deco_text =
        std::fs::read_to_string(&deco_path).unwrap_or_else(|e| panic!("{}: {e}", deco_path.display()));
    let addons_path = Path::new("data").join("deco_addons.json");
    let addons_text =
        std::fs::read_to_string(&addons_path).unwrap_or_else(|e| panic!("{}: {e}", addons_path.display()));
    std::fs::write(out_dir.join("deco.rs"), deco(&deco_text, &addons_text))
        .unwrap_or_else(|e| panic!("writing deco.rs: {e}"));
}
