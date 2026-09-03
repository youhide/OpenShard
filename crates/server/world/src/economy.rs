//! The economy as a graph: what the shard pays out, what eats it, and what
//! neither.
//!
//! # Why this exists
//!
//! Every table this reads is correct on its own. Mining yields nine grades of
//! ore, and the smith's material axis offers nine grades of ingot; lumberjacking
//! yields seven grades of log, and the carpenter's axis offers seven grades of
//! board. Both pairs read as a closed loop *until you ask what turns the left
//! side into the right one* — and for wood there is nothing, because ServUO
//! spends that step in `BaseLog.OnDoubleClick` and this engine has a
//! [`smelt`](openshard_crafting::smelt) for ore and no counterpart for logs.
//!
//! A gap like that is invisible to every existing check. `crafting`'s tests ask
//! whether a recipe's rows agree with each other; `harvest`'s ask whether the
//! hues agree with the ores. Nothing asked the question that spans them: **can a
//! player standing in an empty world actually get hold of this?** That is one
//! reachability query over one graph, and this module is that graph.
//!
//! # The shape
//!
//! One node is a [`Resource`] — a semantic kind and grade where the registry
//! knows the thing, and bare client art where it does not. One edge is a
//! [`Step`]: what it eats, what it pays, and [`why`](Step::via). A step with no
//! inputs is a *source* — a vein, a shelf, a corpse, a field — so sources and
//! conversions need no separate machinery and the fixed point in
//! [`Economy::of`] treats them alike.
//!
//! # Where the edges come from, and the one place they are hand-written
//!
//! Recipes, harvest tables, vendor shelves, loot tables and crop fields are all
//! *data*, and are walked. So are spinning and weaving, which
//! [`Fibre`](openshard_state::components::Fibre) and
//! [`is_cloth_material`](openshard_items::is_cloth_material) each answer for a
//! swept graphic. Three bridges are neither: smelting and the two cuts of the
//! scissors are `match` arms inside functions, and no sweep can enumerate a
//! function. Those are [`CONVERSIONS`], declared here — every row naming the
//! module that implements it, and every row built out of that module's own public
//! constants so a rename cannot leave the declaration behind.
//!
//! # What it will not tell you
//!
//! Whether a thing is *worth* getting, whether the creature that drops it is
//! spawned anywhere, and whether the skill gate on a step is reachable. This
//! answers existence, not economy balance: a resource is reachable here if some
//! chain of steps produces it from nothing at all.

use std::collections::{
    BTreeMap,
    BTreeSet,
};
use std::fmt;

use openshard_crafting::consume::axis_pick;
use openshard_crafting::craft::CraftOutput;
use openshard_crafting::defs::SYSTEMS;
use openshard_crafting::recipe::Recipe;
use openshard_crafting::system::{
    CraftSystemDef,
    SystemId,
};
use openshard_protocol::item_kind::{
    ItemKindId,
    MaterialFamilyId,
    MaterialId,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_state::components::{
    Drawn,
    Fibre,
};
use openshard_state::harvest::{
    HarvestKind,
    HarvestResource,
};
use openshard_state::item_definition::{
    LEATHER,
    MATERIAL_DEFINITIONS,
    METAL,
    WOOD,
};

/// One thing the economy can hold.
///
/// Two identity models rather than one, because the tree genuinely has two: a
/// migrated row names an [`ItemKindId`] and a grade, and an unmigrated one is
/// still a graphic and a hue. Legacy art that the audited registry *does*
/// recognise is canonicalized to [`Self::Kind`] on construction — otherwise the
/// vendor's board (`0x1BD7` at hue zero, written as bare art in `townsfolk.json`)
/// and the carpenter's axis (kind 36 at regular wood) would be two nodes, and the
/// board would look both reachable and unreachable at once.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Resource {
    /// A registered kind, with its grade where the kind has a material family.
    Kind {
        /// The semantic kind.
        kind:     ItemKindId,
        /// Its grade, or `None` for a kind with no material axis.
        material: Option<MaterialId>,
    },
    /// Client art the registry does not name. Not a fallback for a *failed*
    /// lookup of a kind — it is the identity of everything that has not been
    /// migrated yet, and most of the catalogue is still here.
    Art(Graphic, Hue),
}

impl Resource {
    /// The node for an identity a caller has already resolved, falling back to
    /// its art when there is no semantic half.
    #[must_use]
    pub fn resolve(semantic: Option<(ItemKindId, Option<MaterialId>)>, legacy: Drawn) -> Self {
        match semantic {
            Some((kind, material)) => Self::Kind { kind, material },
            None => Self::of_art(legacy),
        }
    }

    /// The node for a piece of client art, through the audited registry bridge.
    #[must_use]
    pub fn of_art(drawn: Drawn) -> Self {
        match openshard_state::kind_from_drawn(drawn) {
            Some((kind, material)) => Self::Kind { kind, material },
            None => Self::Art(drawn.id, drawn.hue),
        }
    }

    /// The node for a kind at one grade.
    #[must_use]
    pub const fn graded(kind: ItemKindId, material: MaterialId) -> Self {
        Self::Kind {
            kind,
            material: Some(material),
        }
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kind { kind, material } => {
                let name = openshard_state::item_definition(*kind).map_or("unregistered", |def| def.name);
                write!(f, "{name} ({})", kind.0)?;
                if let Some(material) = material {
                    let grade = openshard_state::material_definition(*material)
                        .map_or("unregistered", |def| def.name);
                    write!(f, " [{grade}]")?;
                }
                Ok(())
            }
            Self::Art(graphic, hue) => write!(f, "art {:#06X} hue {:#06X}", graphic.0, hue.0),
        }
    }
}

/// Why a step exists — which table or which piece of code pays this out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// A vein worked with a harvest tool.
    Harvest(HarvestKind),
    /// A carcass opened with a blade — `openshard_items::carve`.
    Butchery(Graphic),
    /// A sheep — `openshard_items::shear`.
    Shearing,
    /// A crop field — `openshard_items::crop`.
    Crop,
    /// A shopkeeper's shelf.
    Vendor,
    /// A corpse's loot table.
    Loot(Graphic),
    /// A spinning wheel — `openshard_items::spin`.
    Spinning(Graphic),
    /// A loom — `openshard_items::weave`.
    Weaving(Graphic),
    /// One of the hand-written bridges in [`CONVERSIONS`].
    Conversion(&'static str),
    /// One recipe of one trade, at one grade of its material axis.
    Craft {
        /// Which trade.
        system: SystemId,
        /// The recipe's own art, which is how it is found again in the tables.
        recipe: Graphic,
        /// The axis grade this instance was resolved at, for a recipe that
        /// spends the axis at all.
        grade:  Option<MaterialId>,
    },
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Harvest(kind) => write!(f, "harvest/{kind:?}"),
            Self::Butchery(body) => write!(f, "butchery of body {:#06X}", body.0),
            Self::Shearing => f.write_str("shearing"),
            Self::Crop => f.write_str("a crop field"),
            Self::Vendor => f.write_str("a vendor's shelf"),
            Self::Loot(body) => write!(f, "loot of body {:#06X}", body.0),
            Self::Spinning(fibre) => write!(f, "spinning {:#06X}", fibre.0),
            Self::Weaving(material) => write!(f, "weaving {:#06X}", material.0),
            Self::Conversion(name) => f.write_str(name),
            Self::Craft {
                system,
                recipe,
                grade,
            } => {
                let skill = SYSTEMS
                    .get(system.index())
                    .map_or_else(|| "unknown".to_string(), |def| format!("{:?}", def.skill));
                write!(f, "{skill} {:#06X}", recipe.0)?;
                if let Some(grade) = grade {
                    let name =
                        openshard_state::material_definition(*grade).map_or("unregistered", |def| def.name);
                    write!(f, " [{name}]")?;
                }
                Ok(())
            }
        }
    }
}

/// One edge: what it eats and what it pays. Empty `inputs` makes it a source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    /// Where this came from.
    pub via:     Origin,
    /// What has to be in hand first. Empty for a source.
    pub inputs:  Vec<Resource>,
    /// What comes out.
    pub outputs: Vec<Resource>,
}

/// Which side of a [`Conversion`] a row names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    /// A kind with no grade.
    Kind(ItemKindId),
    /// A kind at every grade of one family, expanded one row per grade. Two
    /// graded sides of one conversion must name the same family: the bridges
    /// declared here all carry the grade through unchanged, which is exactly why
    /// nine ores make nine ingots without nine rows.
    Graded(ItemKindId, MaterialFamilyId),
    /// Client art, at hue zero.
    Art(Graphic),
}

/// A bridge that is code rather than data.
///
/// The handful of them, and no more: everything else in the graph is walked out
/// of a table. Each row names the module that implements it so a reader can check
/// the claim, and is built from that module's own public constants so that a
/// renamed kind is a compile error here rather than a silently wrong edge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Conversion {
    /// What it is called in the report.
    pub name: &'static str,
    /// What goes in.
    pub from: Side,
    /// What comes out.
    pub to:   Side,
}

/// The hand-written bridges, and the whole of them.
///
/// **The absence of a wood row here was the finding this module was written to
/// state**, and it is closed: ServUO bridges lumberjacking to carpentry through
/// `IAxe` rather than a double click — an axe clicked on a `BaseLog` through the
/// lumberjack's own harvest cursor calls `TryCreateBoards`, which gates on
/// Carpentry or Lumberjacking (0 for plain wood, 65 oak, 80 ash, 95 yew, 100 for
/// heartwood, bloodwood and frostwood, `Scripts/Items/Resource/Log.cs`) and pays
/// boards of the log's own resource. [`openshard_crafting::chop`] is that step,
/// and the row below is what makes the seven grades meet.
pub const CONVERSIONS: &[Conversion] = &[
    // A miner is paid in ore and every smithing row eats ingots.
    Conversion {
        name: "smelting (openshard_crafting::smelt)",
        from: Side::Graded(openshard_crafting::smelt::ORE_KIND, METAL),
        to:   Side::Graded(openshard_crafting::smelt::INGOT_KIND, METAL),
    },
    // A lumberjack is paid in logs and every carpentry, fletching and tinkering
    // row that works wood eats boards.
    Conversion {
        name: "an axe on a log (openshard_crafting::chop)",
        from: Side::Graded(openshard_crafting::chop::LOG_KIND, WOOD),
        to:   Side::Graded(openshard_crafting::chop::BOARD_KIND, WOOD),
    },
    // A carved corpse pays hides and every tailoring row eats leather.
    Conversion {
        name: "scissors on hides (openshard_items::cut)",
        from: Side::Graded(openshard_items::HIDES_KIND, LEATHER),
        to:   Side::Graded(openshard_items::LEATHER_KIND, LEATHER),
    },
    // A loom pays bolts and fifty-odd rows eat cloth.
    Conversion {
        name: "scissors on a bolt (openshard_items::cut)",
        from: Side::Art(openshard_items::BOLT_GRAPHIC),
        to:   Side::Art(openshard_items::CLOTH_GRAPHIC),
    },
    // A fisherman lands fish and the cook's rows eat steaks.
    Conversion {
        name: "a blade on a fish (openshard_items::carve)",
        from: Side::Art(openshard_state::harvest::FISH_GRAPHIC),
        to:   Side::Art(openshard_items::RAW_FISH_STEAK),
    },
    // The mill's craft makes a closed sack of flour and every dough row eats an
    // open one.
    Conversion {
        name: "opening a sack of flour (openshard_items::flour)",
        from: Side::Art(openshard_items::SACK_OF_FLOUR),
        to:   Side::Art(openshard_items::OPEN_SACK_OF_FLOUR),
    },
];

/// The whole graph, and which of its nodes a player can actually reach.
#[derive(Clone, Debug)]
pub struct Economy {
    /// Every edge, sources included.
    pub steps:     Vec<Step>,
    /// Every node reachable from the sources by any chain of steps.
    pub reachable: BTreeSet<Resource>,
}

impl Economy {
    /// Build the graph for one expansion and run the reachability to a fixed
    /// point.
    ///
    /// `ml` is [`Gameplay::is_ml`](openshard_state::Gameplay::is_ml), and it
    /// genuinely changes the answer: before Mondain's Legacy a tree gives one
    /// kind of log, and the six special woods are not merely unreachable but
    /// absent.
    #[must_use]
    pub fn of(ml: bool) -> Self {
        let mut steps = Vec::new();
        harvest_steps(&mut steps, ml);
        butchery_steps(&mut steps);
        shearing_step(&mut steps);
        crop_steps(&mut steps);
        vendor_steps(&mut steps);
        loot_steps(&mut steps);
        fibre_steps(&mut steps);
        conversion_steps(&mut steps);
        craft_steps(&mut steps);

        // The fixed point. A step fires once every input it wants is in hand,
        // and firing may put a new input in reach of a step already passed over,
        // so the sweep repeats until a whole pass adds nothing. Bounded by the
        // node count: each pass that changes anything adds at least one node.
        let mut reachable = BTreeSet::new();
        loop {
            let before = reachable.len();
            for step in &steps {
                if step.inputs.iter().all(|input| reachable.contains(input)) {
                    reachable.extend(step.outputs.iter().copied());
                }
            }
            if reachable.len() == before {
                break;
            }
        }
        Self { steps, reachable }
    }

    /// Every node any step consumes.
    #[must_use]
    pub fn consumed(&self) -> BTreeSet<Resource> {
        self.steps
            .iter()
            .flat_map(|step| step.inputs.iter().copied())
            .collect()
    }

    /// What is wrong with it.
    #[must_use]
    pub fn report(&self) -> Report {
        let consumed = self.consumed();

        // Wanted by something, and no chain of steps produces it. Both halves of
        // "unreachable" at once: a node nothing at all makes, and a node whose
        // every maker is itself stalled, are the same problem to a player.
        let mut unreachable: BTreeMap<Resource, Vec<Origin>> = BTreeMap::new();
        for step in &self.steps {
            for input in &step.inputs {
                if !self.reachable.contains(input) {
                    unreachable.entry(*input).or_default().push(step.via);
                }
            }
        }

        // Paid out and eaten by nothing.
        //
        // Only asked of the steps that *produce raw material*, which is not the
        // same set as "steps with no inputs". A vendor's shelf and a corpse's
        // loot table also have no inputs, and both are full of finished goods: a
        // longsword nothing consumes is the point of a longsword. A vein, a
        // carcass, a fleece, a field, a wheel, a loom and the declared bridges are
        // the seven things on the shard whose whole purpose is to feed something
        // else, so a dead end there is a trade paying in an item with no use.
        let mut dead_ends: BTreeMap<Resource, Vec<Origin>> = BTreeMap::new();
        for step in &self.steps {
            let raw = matches!(
                step.via,
                Origin::Harvest(_)
                    | Origin::Butchery(_)
                    | Origin::Shearing
                    | Origin::Crop
                    | Origin::Spinning(_)
                    | Origin::Weaving(_)
                    | Origin::Conversion(_)
            );
            if !raw {
                continue;
            }
            for output in &step.outputs {
                if !consumed.contains(output) {
                    dead_ends.entry(*output).or_default().push(step.via);
                }
            }
        }

        // Steps that can never run. Reported beside the resources above rather
        // than instead of them: the resource says what is missing, and this says
        // what is lost by its absence.
        let stalled: Vec<(Origin, Vec<Resource>)> = self
            .steps
            .iter()
            .filter_map(|step| {
                let missing: Vec<Resource> = step
                    .inputs
                    .iter()
                    .filter(|input| !self.reachable.contains(input))
                    .copied()
                    .collect();
                (!missing.is_empty()).then_some((step.via, missing))
            })
            .collect();

        Report {
            unreachable,
            dead_ends,
            stalled,
        }
    }
}

/// What the audit found.
#[derive(Clone, Debug)]
pub struct Report {
    /// A resource some step wants that no chain of steps can produce, and every
    /// step that wants it.
    pub unreachable: BTreeMap<Resource, Vec<Origin>>,
    /// A raw material the world pays out that nothing consumes, and where it
    /// comes from.
    pub dead_ends:   BTreeMap<Resource, Vec<Origin>>,
    /// Every step that can never run, with the inputs that stop it.
    pub stalled:     Vec<(Origin, Vec<Resource>)>,
}

impl Report {
    /// Whether the economy closes: nothing wanted is out of reach, and nothing
    /// paid out is useless.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.unreachable.is_empty() && self.dead_ends.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "unreachable resources: {}", self.unreachable.len())?;
        for (resource, wanted_by) in &self.unreachable {
            writeln!(f, "  {resource}")?;
            writeln!(
                f,
                "    wanted by {} step(s), first: {}",
                wanted_by.len(),
                wanted_by[0]
            )?;
        }
        writeln!(f, "\nraw materials nothing consumes: {}", self.dead_ends.len())?;
        for (resource, from) in &self.dead_ends {
            writeln!(f, "  {resource}")?;
            writeln!(f, "    paid by {} step(s), first: {}", from.len(), from[0])?;
        }
        writeln!(f, "\nsteps that can never run: {}", self.stalled.len())?;
        for (via, missing) in &self.stalled {
            let names: Vec<String> = missing.iter().map(ToString::to_string).collect();
            writeln!(f, "  {via} — missing {}", names.join(", "))?;
        }
        Ok(())
    }
}

/// Every grade of one material family, in registry order.
fn grades(family: MaterialFamilyId) -> Vec<MaterialId> {
    MATERIAL_DEFINITIONS
        .iter()
        .filter(|def| def.family == family)
        .map(|def| def.id)
        .collect()
}

/// The node a harvest row pays out.
fn harvest_resource(row: &HarvestResource) -> Resource {
    Resource::resolve(
        row.item_kind.map(|kind| (kind, row.material)),
        Drawn {
            id:  row.graphic,
            hue: row.hue,
        },
    )
}

fn harvest_steps(steps: &mut Vec<Step>, ml: bool) {
    for kind in [
        HarvestKind::Ore,
        HarvestKind::Sand,
        HarvestKind::Lumber,
        HarvestKind::Fish,
    ] {
        let def = openshard_state::harvest::definition(kind, ml);
        for row in def.resources {
            steps.push(Step {
                via:     Origin::Harvest(kind),
                inputs:  Vec::new(),
                outputs: vec![harvest_resource(row)],
            });
        }
        // And what a swing turns up *besides* its resource — the Mondain's
        // Legacy bonus tables, which are the only source of a bark fragment or a
        // special gem on any shard. A separate step from the resource rather than
        // a second output on it: the two are rolled independently, and a graph
        // that paired them would claim a miner cannot find a diamond without also
        // finding ore that swing.
        for row in def.bonus {
            steps.push(Step {
                via:     Origin::Harvest(kind),
                inputs:  Vec::new(),
                outputs: vec![Resource::of_art(Drawn {
                    id:  row.graphic,
                    hue: Hue(0),
                })],
            });
        }
    }
}

/// Carving, swept over the whole graphic space.
///
/// A sweep rather than a table because the table is a `match` in
/// [`carved_yield`](openshard_items::carved_yield) and there is no other way to
/// enumerate one. It is 65,536 `const fn` calls and costs nothing measurable.
fn butchery_steps(steps: &mut Vec<Step>) {
    for body in (0..=u16::MAX).map(Graphic) {
        let Some(yielded) = openshard_items::carved_yield(body) else {
            continue;
        };
        let mut outputs = Vec::new();
        if yielded.bird {
            outputs.push(Resource::of_art(Drawn {
                id:  openshard_items::RAW_BIRD,
                hue: Hue(0),
            }));
        } else if yielded.ribs != 0 {
            outputs.push(Resource::of_art(Drawn {
                id:  openshard_items::RAW_RIBS,
                hue: Hue(0),
            }));
        }
        if yielded.hides != 0 {
            outputs.push(Resource::graded(openshard_items::HIDES_KIND, yielded.hide));
        }
        if yielded.feathers != 0 {
            outputs.push(Resource::of_art(Drawn {
                id:  openshard_items::FEATHERS,
                hue: Hue(0),
            }));
        }
        if yielded.wool != 0 {
            // Tainted rather than the fleece, which is [`Origin::Shearing`]'s:
            // the two are different items and the wheel spins both.
            outputs.push(Resource::of_art(Drawn {
                id:  openshard_items::TAINTED_WOOL,
                hue: Hue(0),
            }));
        }
        if !outputs.is_empty() {
            steps.push(Step {
                via: Origin::Butchery(body),
                inputs: Vec::new(),
                outputs,
            });
        }
    }
}

fn shearing_step(steps: &mut Vec<Step>) {
    steps.push(Step {
        via:     Origin::Shearing,
        inputs:  Vec::new(),
        outputs: vec![Resource::of_art(Drawn {
            id:  openshard_items::WOOL,
            hue: Hue(0),
        })],
    });
}

/// The crops the shipped fields actually plant, rather than every
/// [`CropKind`](openshard_state::components::CropKind) that exists: a crop
/// nothing sows is not a source.
fn crop_steps(steps: &mut Vec<Step>) {
    let mut seen = BTreeSet::new();
    for set in crate::crops::shipped() {
        for field in &set.fields {
            let (graphic, _) = field.crop.yield_of();
            if !seen.insert(graphic) {
                continue;
            }
            steps.push(Step {
                via:     Origin::Crop,
                inputs:  Vec::new(),
                outputs: vec![Resource::of_art(Drawn {
                    id:  graphic,
                    hue: Hue(0),
                })],
            });
        }
    }
}

fn vendor_steps(steps: &mut Vec<Step>) {
    let mut seen = BTreeSet::new();
    for set in crate::townsfolk::shipped() {
        for command in &set.townsfolk {
            let crate::Command::SpawnMobile { stock, .. } = command else {
                continue;
            };
            for line in stock {
                let resource = Resource::resolve(
                    line.item_kind.map(|kind| (kind, line.material)),
                    Drawn {
                        id:  line.graphic,
                        hue: line.hue,
                    },
                );
                if !seen.insert(resource) {
                    continue;
                }
                steps.push(Step {
                    via:     Origin::Vendor,
                    inputs:  Vec::new(),
                    outputs: vec![resource],
                });
            }
        }
    }
}

fn loot_steps(steps: &mut Vec<Step>) {
    for &(body, drops) in crate::loot::SHIPPED {
        let outputs: Vec<Resource> = drops
            .iter()
            .map(|drop| {
                Resource::of_art(Drawn {
                    id:  drop.graphic,
                    hue: drop.hue,
                })
            })
            .collect();
        steps.push(Step {
            via: Origin::Loot(Graphic(body)),
            inputs: Vec::new(),
            outputs,
        });
    }
}

/// The wheel and the loom, both swept for [`butchery_steps`]' reason.
fn fibre_steps(steps: &mut Vec<Step>) {
    for graphic in (0..=u16::MAX).map(Graphic) {
        if let Some(fibre) = Fibre::from_graphic(graphic) {
            let (spun, _) = fibre.spun_into();
            steps.push(Step {
                via:     Origin::Spinning(graphic),
                inputs:  vec![Resource::of_art(Drawn {
                    id:  graphic,
                    hue: Hue(0),
                })],
                outputs: vec![Resource::of_art(Drawn {
                    id:  spun,
                    hue: Hue(0),
                })],
            });
        }
        if openshard_items::is_cloth_material(graphic) {
            steps.push(Step {
                via:     Origin::Weaving(graphic),
                inputs:  vec![Resource::of_art(Drawn {
                    id:  graphic,
                    hue: Hue(0),
                })],
                outputs: vec![Resource::of_art(Drawn {
                    id:  openshard_items::BOLT_GRAPHIC,
                    hue: Hue(0),
                })],
            });
        }
    }
}

/// One side of a [`Conversion`], expanded to the nodes it names.
fn side_nodes(side: Side) -> Vec<Resource> {
    match side {
        Side::Kind(kind) => {
            vec![Resource::Kind { kind, material: None }]
        }
        Side::Graded(kind, family) => {
            grades(family)
                .into_iter()
                .map(|material| Resource::graded(kind, material))
                .collect()
        }
        Side::Art(graphic) => {
            vec![Resource::of_art(Drawn {
                id:  graphic,
                hue: Hue(0),
            })]
        }
    }
}

fn conversion_steps(steps: &mut Vec<Step>) {
    for conversion in CONVERSIONS {
        let from = side_nodes(conversion.from);
        let to = side_nodes(conversion.to);
        // Grade-for-grade, which is what every declared bridge does. A row whose
        // two sides disagree in length is a mis-declaration, and the test below
        // is what says so rather than this silently zipping the short list.
        for (input, output) in from.into_iter().zip(to) {
            steps.push(Step {
                via:     Origin::Conversion(conversion.name),
                inputs:  vec![input],
                outputs: vec![output],
            });
        }
    }
}

/// Every recipe, at every grade of its trade's material axis.
///
/// One step per grade rather than one per recipe, because the grade is exactly
/// what decides whether the step can run: a carpenter's chair is one recipe and
/// seven different questions about whether the board it wants exists.
fn craft_steps(steps: &mut Vec<Step>) {
    for (index, system) in SYSTEMS.iter().enumerate() {
        let id = SystemId::from_index(index).expect("a system index fits its stored byte");
        for recipe in system.recipes {
            for sub_res in axis_range(system, recipe) {
                steps.push(craft_step(id, system, recipe, sub_res));
            }
        }
    }
}

/// The material selections worth resolving a recipe at: every grade for a recipe
/// that spends the axis, and only the plain one for a recipe that does not.
fn axis_range(system: &CraftSystemDef, recipe: &Recipe) -> std::ops::Range<usize> {
    match axis_pick(system, recipe, 0) {
        Some(_) => 0..system.sub_res.map_or(1, |axis| axis.entries.len()),
        None => 0..1,
    }
}

fn craft_step(id: SystemId, system: &CraftSystemDef, recipe: &Recipe, sub_res: usize) -> Step {
    let pick = axis_pick(system, recipe, sub_res);
    // Zero-amount lines are dropped here for the same reason `consume::check`
    // drops them, and it is load-bearing rather than tidy: `InheritInput` indexes
    // the list the craft counts, so keeping a free line in would make a typed
    // output inherit its grade from the wrong ingredient.
    let ingredients: Vec<_> = openshard_crafting::consume::ingredients(system, recipe, sub_res)
        .into_iter()
        .filter(|line| line.amount != 0)
        .collect();
    let inputs: Vec<Resource> = ingredients
        .iter()
        .map(|line| Resource::resolve(line.semantic, line.legacy))
        .collect();

    let resolved: Vec<_> = ingredients.iter().map(|line| line.semantic).collect();
    let output = match openshard_crafting::craft::output_identity(recipe, &resolved) {
        CraftOutput::Typed { kind, material, .. } => Resource::Kind { kind, material },
        CraftOutput::Legacy => {
            // The hue rule `craft::finish` applies to an unmigrated row: a fixed
            // hue wins, else the axis grade for a row that keeps its colour, else
            // plain.
            let hue = if recipe.hue != Hue(0) {
                recipe.hue
            } else if recipe.retain_color {
                pick.map_or(Hue(0), |pick| pick.entry.hue)
            } else {
                Hue(0)
            };
            Resource::of_art(Drawn {
                id: recipe.graphic,
                hue,
            })
        }
        // A row that contradicts itself makes nothing at all — the live craft
        // refuses it too — so it contributes no output rather than a guessed one.
        CraftOutput::Unresolvable => {
            return Step {
                via: Origin::Craft {
                    system: id,
                    recipe: recipe.graphic,
                    grade:  pick.map(|pick| pick.entry.material),
                },
                inputs,
                outputs: Vec::new(),
            };
        }
    };

    Step {
        via: Origin::Craft {
            system: id,
            recipe: recipe.graphic,
            grade:  pick.map(|pick| pick.entry.material),
        },
        inputs,
        outputs: vec![output],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client art some recipe eats that nothing on the shard produces.
    ///
    /// Recorded rather than explained: the audit's job is to *state* the list, and
    /// deciding what each one wants — a conversion, a loot table, a vendor line,
    /// or a recipe deleted as unshippable — is a decision per row and belongs in
    /// `docs/roadmap/backlog/gameplay.md`. What can be said about them as classes:
    ///
    /// - `0x3183`–`0x3199` is one contiguous run of **Mondain's Legacy special
    ///   ingredients** (the `1032…` name clilocs), which upstream pays out of
    ///   Heartwood quest turn-ins and champion drops. This shard has neither, so
    ///   every ML recipe that wants one is dead on arrival — twenty-two arts, and
    ///   by a wide margin the largest group.
    /// - `0x3183`–`0x318E` are what is **left** of that run: the twelve peerless
    ///   ingredients — blight, corruption, scourge, putrefaction, taint,
    ///   muculent, the lard of Paroxysmus, a dread horn's mane, diseased bark,
    ///   grizzled bones, the eye of the Travesty and a captured essence. Every
    ///   one of them is a peerless boss's drop, and this shard has no peerless.
    ///   `0x315A` (a pristine dread horn) and `0x4005` (a toxic venom sac) are
    ///   the same fact under other names.
    /// - **Quest and faction content**: `0x0EF0` silver, `0x1879` copper wire,
    ///   `0x14F8` a rope and `0x1374` a bridle, `0x2F57` a runed prism and
    ///   `0x2F5C` an enchanted switch, `0x1E25` a shelf of academic books. None
    ///   of them is bought, crafted or dropped upstream either — they are
    ///   Heartwood turn-ins, faction stores and Mad Scientist statics.
    /// - **Upstream cannot build these two either.** `0x15F8`, an empty wooden
    ///   bowl, appears in exactly two places in the whole of ServUO: its own
    ///   class and the fruit-bowl recipe that eats it. Nothing sells it, crafts
    ///   it or drops it, so `DefCooking`'s fruit bowl is unbuildable on OSI's own
    ///   shards. `0x0F7C`, cocoa pulp, comes off a Time of Legends cocoa tree,
    ///   and behind it `0x1044` cocoa butter.
    /// - `0x573B`, **crushed glass**: the alchemy row that spends it is
    ///   `if (Core.SA)` upstream and the blacksmithy row that *makes* it is too —
    ///   the consumer was imported and the producer was not, so this is an era
    ///   that leaked rather than a source that is missing.
    ///
    /// **Rows leave this list as their sources land, and eleven have.** For the
    /// record of what each fix was shaped like: `0x0F7E` a bone, loot off the
    /// undead rather than butchery; `0x101F` tainted wool, which comes off a
    /// woolly *corpse* where the shear pays the fleece; `0x1EBD` a sheaf of
    /// wheat, which wanted a field; `0x103A` an open sack of flour, which wanted
    /// the double-click that opens the closed one; `0x1F9D` a pitcher of water
    /// and `0x0F8A`/`0x0F8F` two necromancer reagents, all three of them vendor
    /// lines the converter dropped because they were not `GenericBuyInfo` rows;
    /// and `0x103D`, `0x103F`, `0x1042`, `0x1083`, which were only ever waiting
    /// on the water. `0x171F`, a banana, went with the innkeeper's shelf — the
    /// one vendor upstream sells one from, and one of the twenty-odd shopkeepers
    /// this shard placed with an outfit and no stock at all.
    const UNSOURCED_ART: &[u16] = &[
        0x0EF0, 0x0F7C, 0x1044, 0x1374, 0x14F8, 0x15F8, 0x1879, 0x1E25, 0x2F57, 0x2F5C, 0x315A, 0x3183,
        0x3184, 0x3185, 0x3186, 0x3187, 0x3188, 0x3189, 0x318A, 0x318B, 0x318C, 0x318D, 0x318E, 0x4005,
        0x573B,
    ];

    /// What only Mondain's Legacy pays out — the three harvest bonus tables.
    ///
    /// Upstream guards every one of them with `Core.ML`, so before that
    /// expansion a bark fragment, the six mining gems, the amber in a tree and
    /// the pearl in the sea have no source at all. Unreachable for an era rather
    /// than for a hole, which is the same shape as the six special logs below.
    const ML_ONLY_ART: &[u16] = &[
        0x318F, 0x3192, 0x3193, 0x3194, 0x3195, 0x3196, 0x3197, 0x3198, 0x3199,
    ];

    /// The holes the shard ships with, as of 2026-09-03.
    ///
    /// A ratchet, and deliberately compared **both ways**: a new hole fails the
    /// test, and so does closing one without deleting its row here. A list that
    /// only grew would rot into the stale queue entry `gameplay.md` already has a
    /// lesson about.
    ///
    /// Every row is a resource some step wants and nothing can produce, and what
    /// is left of them is [`UNSOURCED_ART`] — unmigrated rows, each wanting a
    /// verdict of its own.
    ///
    /// **Two groups have left this list.** The six special boards went when an
    /// axe learned to cut a log ([`openshard_crafting::chop`], and its row in
    /// [`CONVERSIONS`]) — in Mondain's Legacy, at least: before it a tree gives
    /// one wood, so the other six logs do not exist, the axe has nothing to cut
    /// and the six boards stay out of reach. That is the era having one wood
    /// rather than the shard having a hole, and it is why this list takes `ml`.
    /// The horned and barbed hides went when the dragon family became carvable —
    /// `carved_yield`'s own doc used to say no body on the shard wore them, and
    /// dragons, wyrms, drakes, wyverns and sea serpents had been spawning all
    /// along.
    fn known_gaps(ml: bool) -> Vec<Resource> {
        let mut gaps: Vec<Resource> = Vec::new();
        if !ml {
            // The pre-ML pair, and both halves of it: the special logs are wanted
            // by the axe and paid by no tree, and the boards they would become are
            // wanted by the carpenter's axis, which offers seven grades in every
            // era whether or not a tree can give seven.
            for material in grades(WOOD).into_iter().skip(1) {
                gaps.push(Resource::graded(openshard_crafting::chop::LOG_KIND, material));
                gaps.push(Resource::graded(openshard_crafting::chop::BOARD_KIND, material));
            }
            gaps.extend(ML_ONLY_ART.iter().map(|art| Resource::Art(Graphic(*art), Hue(0))));
        }
        gaps.extend(
            UNSOURCED_ART
                .iter()
                .map(|art| Resource::Art(Graphic(*art), Hue(0))),
        );
        gaps.sort_unstable();
        gaps
    }

    #[test]
    fn the_shipped_economy_has_exactly_the_holes_we_know_about() {
        for ml in [true, false] {
            let report = Economy::of(ml).report();
            let found: Vec<Resource> = report.unreachable.keys().copied().collect();
            assert_eq!(
                found,
                known_gaps(ml),
                "ml={ml}: the reachability holes moved.\n{report}"
            );
        }
    }

    #[test]
    fn the_only_raw_materials_with_no_sink_are_the_ones_we_know_about() {
        // The other direction, and the one that catches a trade paying in an item
        // with no use. **Nothing is left**, in either era, and the three that
        // were are worth naming for what closing one looked like: logs, which the
        // axe now cuts into boards; fish, which a blade cuts into the steaks
        // `DefCooking` always had rows for; and sand, which waited for the whole
        // trade that spends it — `defs::glassblowing`, thirteen rows and a
        // blowpipe.
        //
        // An empty expectation is a real assertion here rather than a vacuous
        // one: this list is built from the report, so a resource that stopped
        // being spent tomorrow appears in it and fails.
        for ml in [true, false] {
            let report = Economy::of(ml).report();
            let expected: Vec<Resource> = Vec::new();
            let found: Vec<Resource> = report.dead_ends.keys().copied().collect();
            assert_eq!(found, expected, "ml={ml}: the dead ends moved.\n{report}");
        }
    }

    #[test]
    fn wood_closes_the_loop_it_used_to_break() {
        // The finding this module was written to state, now asserted the other
        // way round: seven grades of log are paid out, the axe cuts each into a
        // board of its own grade, and the carpenter can spend every one. The
        // assertion here used to be that no log had a sink at all — that is what
        // closing a hole looks like in a ratchet.
        let economy = Economy::of(true);
        let consumed = economy.consumed();
        for material in grades(WOOD) {
            let log = Resource::graded(openshard_crafting::chop::LOG_KIND, material);
            let board = Resource::graded(openshard_crafting::chop::BOARD_KIND, material);
            assert!(economy.reachable.contains(&log), "{log} is not even paid out");
            assert!(consumed.contains(&log), "{log} has no sink");
            assert!(economy.reachable.contains(&board), "{board} cannot be made");
        }
    }

    #[test]
    fn ore_closes_its_loop_the_same_way() {
        // The control for the test above, and the older of the two chains: nine
        // ores are paid out, nine ingots are reachable, and a smith can spend
        // every one of them. Without this, a graph that had simply lost all its
        // conversion edges would look exactly like a shard where both bridges
        // work.
        let economy = Economy::of(true);
        let consumed = economy.consumed();
        for material in grades(METAL) {
            let ore = Resource::graded(openshard_crafting::smelt::ORE_KIND, material);
            let ingot = Resource::graded(openshard_crafting::smelt::INGOT_KIND, material);
            assert!(economy.reachable.contains(&ore), "{ore}");
            assert!(consumed.contains(&ore), "{ore} has no sink");
            assert!(economy.reachable.contains(&ingot), "{ingot}");
        }
    }

    #[test]
    fn every_declared_conversion_pairs_its_two_sides_grade_for_grade() {
        // `conversion_steps` zips the two sides. A row whose sides are different
        // lengths would silently drop the tail rather than fail, which is the one
        // way a hand-written table here can be wrong without anybody noticing.
        for conversion in CONVERSIONS {
            let from = side_nodes(conversion.from);
            let to = side_nodes(conversion.to);
            assert_eq!(
                from.len(),
                to.len(),
                "{} pairs {} inputs with {} outputs",
                conversion.name,
                from.len(),
                to.len()
            );
            assert!(!from.is_empty(), "{} names nothing", conversion.name);
        }
    }

    #[test]
    fn a_vendors_board_and_a_carpenters_board_are_one_node() {
        // The canonicalization that makes the whole graph meaningful. The shelf
        // writes bare art (`townsfolk.json` has no hue column) and the axis names
        // kind 36 at a grade; if those stayed two nodes, the plain board would be
        // reported unreachable and the six special ones would not stand out at
        // all.
        let shelf = Resource::of_art(Drawn {
            id:  Graphic(0x1BD7),
            hue: Hue(0),
        });
        assert_eq!(shelf, Resource::graded(ItemKindId(36), MaterialId(20)));
        assert!(Economy::of(true).reachable.contains(&shelf));
    }

    #[test]
    fn the_report_names_the_recipes_a_gap_costs() {
        // A resource list alone does not say what is lost. Carpentry is still the
        // trade the remaining gap empties — it was the board bridge and it is now
        // the peerless ingredients, which its Mondain's Legacy rows spend by the
        // dozen — so its stalled rows are the check that `stalled` is populated
        // and pointed at the right trade.
        //
        // The bound is deliberately loose. It was `> 100` when the boards were
        // missing and 1,213 rows were stalled; a number that tracked the true
        // count would have to be edited by every commit on this page, which is
        // the ratchet's job and not this test's.
        let report = Economy::of(true).report();
        let carpentry = SYSTEMS
            .iter()
            .position(|def| def.skill == openshard_state::Skill::Carpentry)
            .expect("carpentry is a shipped trade");
        let stalled = report
            .stalled
            .iter()
            .filter(|(via, _)| matches!(via, Origin::Craft { system, .. } if system.index() == carpentry))
            .count();
        assert!(stalled > 10, "only {stalled} carpentry rows are blocked");
    }
}
