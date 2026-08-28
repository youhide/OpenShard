//! Resource gathering: what the ground yields, and to which tool.
//!
//! A port of ServUO's `Scripts/Services/Harvest/` — `HarvestDefinition`,
//! `HarvestVein` and `HarvestResource` — as core data, the shape [`crate::weapon`]
//! and [`crate::instrument`] already use. What a mountain face *is worth* is a
//! property of the tile and the skill, not of the shard; what the swing *does*
//! lives in `skills`.
//!
//! Everything is fixed point. Skills are in tenths, as [`crate::skill`] keeps them
//! and as `skills::roll_skill_band` takes them; chances are in hundredths of a
//! percent, so ServUO's `49.6` is `4960`. Every duration is a **tick count**,
//! never a `Duration`, for the reason decay and swing timers are: the tick
//! replays and a wall clock does not.
//!
//! The banks — the depletion-and-respawn state that makes a vein run dry — are
//! [`Banks`], which lives on `FacetState` beside the sector grid.

use std::collections::HashMap;
use std::num::NonZeroU32;

use openshard_protocol::wire::{ClilocId, Graphic, Hue, SoundId};
use openshard_protocol::world::Facet;

use crate::WorldTick;
use crate::rng::Rng;
use crate::runtime::TICKS_PER_SECOND;
use crate::skill::Skill;

/// Which of the four definitions a row is, and the key half of a bank's address.
///
/// An index rather than a pointer so a [`Bank`] can name its definition in a
/// `HashMap` key without borrowing one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HarvestKind {
    /// Ore and stone out of a mountain or cave wall.
    Ore,
    /// Sand out of a beach or a desert, for glassblowing.
    Sand,
    /// Logs out of a tree.
    Lumber,
    /// Fish out of open water.
    Fish,
}

/// A tile id in the normalized form harvest definitions use.
///
/// Land ids are raw; static ids have their high bit set. Keeping the normalized
/// value distinct from a wire [`Graphic`] makes it impossible to compare a raw
/// tile to a harvest-definition entry by accident.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct HarvestTile(pub u16);

/// How a definition matches a tile id.
#[derive(Clone, Copy, Debug)]
pub enum TileSet {
    /// An explicit list — ServUO's default `Validate`.
    List(&'static [HarvestTile]),
    /// Inclusive `(low, high)` pairs — ServUO's `RangedTiles`, which is how the
    /// several thousand water tiles are written without listing them.
    Ranges(&'static [(HarvestTile, HarvestTile)]),
}

impl TileSet {
    /// Whether a tile id belongs to this set.
    #[must_use]
    pub fn contains(&self, tile: HarvestTile) -> bool {
        match self {
            Self::List(ids) => ids.contains(&tile),
            Self::Ranges(pairs) => pairs.iter().any(|(lo, hi)| tile >= *lo && tile <= *hi),
        }
    }
}

/// What one vein yields at one skill level — ServUO's `HarvestResource`.
#[derive(Clone, Copy, Debug)]
pub struct HarvestResource {
    /// The skill needed to work this at all, in tenths. Below it the vein falls
    /// back, and there is no roll.
    pub req_skill: i32,
    /// The bottom of the band the roll is made against, in tenths.
    pub min_skill: i32,
    /// And the top.
    pub max_skill: i32,
    /// The cliloc said on a success — "You have found some iron ore."
    pub success_cliloc: ClilocId,
    /// The item art the yield is made of.
    pub graphic: Graphic,
    /// And its hue — ServUO's `CraftResources.GetHue`, which is the *only* thing
    /// telling valorite ore from iron.
    pub hue: Hue,
}

/// Which of a definition's `resources` a vein points at.
///
/// Its own index space, never a [`VeinIdx`] — a definition's `resources` and
/// `veins` are different lists of different lengths, and a bare `usize` would
/// let one be handed to the other's lookup without a compile error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResourceIdx(pub usize);

/// Which of a definition's `veins` a bank holds. See [`ResourceIdx`] — the
/// index space this is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VeinIdx(pub usize);

/// One vein of a bank — which resource it holds and how often it disappoints.
#[derive(Clone, Copy, Debug)]
pub struct HarvestVein {
    /// How likely this vein is under any given bank, in hundredths of a percent.
    pub chance: u32,
    /// How often a swing at it yields the fallback instead, in hundredths of a
    /// percent. ServUO's `ChanceToFallback`, and it is what stops a valorite vein
    /// being pure valorite.
    pub fallback_chance: u32,
    /// Index into the definition's `resources`.
    pub primary: ResourceIdx,
    /// What it yields instead, where it has one. Iron, for every ore vein but
    /// iron's own.
    pub fallback: Option<ResourceIdx>,
}

/// The clilocs one definition speaks with. ServUO keeps these as loose `object`
/// fields on the definition; they are grouped here because they are one thing —
/// what this kind of harvesting says when it will not work.
#[derive(Clone, Copy, Debug)]
pub struct HarvestMessages {
    /// The bank is empty, said when the swing is *begun*.
    pub no_resources: ClilocId,
    /// The bank ran empty *during* the swing — somebody else got there first.
    pub double_harvest: ClilocId,
    /// Too far away, said when the swing is begun.
    pub out_of_range: ClilocId,
    /// Walked away mid-swing. A different line from `out_of_range`, and the
    /// difference is the point: one is a mistake, the other is giving up.
    pub timed_out_of_range: ClilocId,
    /// The roll failed — "You loosen some rocks but fail to find any useable ore."
    pub fail: ClilocId,
    /// The yield would not fit in the pack.
    pub pack_full: ClilocId,
    /// The tool is spent.
    pub tool_broke: ClilocId,
}

/// One harvesting system — ServUO's `HarvestDefinition`.
#[derive(Clone, Copy, Debug)]
pub struct HarvestDef {
    /// Which of the four this is.
    pub kind: HarvestKind,
    /// The skill rolled, and trained.
    pub skill: Skill,
    /// A bank covers `bank_w` × `bank_h` tiles.
    pub bank_w: u16,
    /// The other side of the bank.
    pub bank_h: u16,
    /// The least a fresh bank holds.
    pub min_total: u16,
    /// And the most.
    pub max_total: u16,
    /// The soonest an emptied bank repays, in ticks.
    pub min_respawn: u64,
    /// And the latest.
    pub max_respawn: u64,
    /// Which tiles this works on.
    pub tiles: TileSet,
    /// How far the harvester may stand, in tiles.
    pub max_range: u32,
    /// How much a successful swing takes out of the bank.
    pub consumed: u16,
    /// And how much on Felucca, which pays double for the danger.
    pub consumed_felucca: u16,
    /// Whether a yield too big for the pack falls at the harvester's feet rather
    /// than being lost — ServUO's `PlaceAtFeetIfFull`.
    pub place_at_feet: bool,
    /// The gesture each beat plays.
    pub action: HarvestAction,
    /// The sounds each beat may play, rolled between. Fishing has none.
    pub sounds: &'static [SoundId],
    /// How many beats one harvest takes.
    pub beats: u16,
    /// How long a beat is, in ticks.
    pub beat_ticks: u64,
    /// How far into a beat its sound plays, in ticks — ServUO's
    /// `EffectSoundDelay`, and the reason a fishing line has eight seconds of
    /// silence before the splash.
    pub sound_ticks: u64,
    /// What it says when it will not work.
    pub messages: HarvestMessages,
    /// What its veins can yield.
    pub resources: &'static [HarvestResource],
    /// And the veins themselves.
    pub veins: &'static [HarvestVein],
    /// Whether a bank re-rolls its vein when it repays — ServUO's
    /// `RandomizeVeins`, which is `Core.ML`. Off, the vein is a fixed property of
    /// the ground.
    pub randomize_veins: bool,
}

/// The gesture a harvest beat plays. A semantic, resolved to wire ids by
/// [`Action`](crate::Action) — the same split every other animation takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HarvestAction {
    /// Swinging a pick at a rock face.
    Mine,
    /// Swinging an axe at a tree.
    Chop,
    /// Casting a line.
    Fish,
}

/// What kind of tile a harvest target is. The distinction is the client's, and it
/// decides how the tile id is read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileSource {
    /// Bare ground. The id comes from the map, because the client sends none.
    Land,
    /// A static. The id is the graphic the client sent, verified against the map.
    Static,
}

/// A tile id as a definition matches it.
///
/// ServUO's `HarvestSystem.GetHarvestDetails`: a static is matched as
/// `(id & 0x3FFF) | 0x4000`, and land raw. Both halves of every tile table are
/// written in those terms, so a mountain *wall* static and the mountain *ground*
/// under it both hit the ore definition.
#[must_use]
pub fn tile_key(tile: Graphic, source: TileSource) -> HarvestTile {
    HarvestTile(match source {
        TileSource::Land => tile.0,
        TileSource::Static => (tile.0 & 0x3FFF) | 0x4000,
    })
}

/// The definition a tile belongs to, if any.
///
/// `ml` is the shard's expansion (`Gameplay::is_ml`). It changes exactly one
/// thing: before Mondain's Legacy a tree yields one kind of log, and from it seven
/// — so the whole lumber definition is chosen rather than its vein list patched,
/// which keeps every caller downstream free of the question.
#[must_use]
pub fn definition_for(tile: Graphic, source: TileSource, ml: bool) -> Option<&'static HarvestDef> {
    let key = tile_key(tile, source);
    definitions(ml).iter().find(|def| def.tiles.contains(key))
}

/// The definition for a kind.
#[must_use]
pub fn definition(kind: HarvestKind, ml: bool) -> &'static HarvestDef {
    definitions(ml)
        .iter()
        .find(|def| def.kind == kind)
        .expect("every kind has a definition")
}

/// The four definitions this shard runs.
const fn definitions(ml: bool) -> &'static [HarvestDef] {
    if ml { DEFINITIONS_ML } else { DEFINITIONS_PRE_ML }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// A harvesting tool: which system it drives, and how many swings it holds.
#[derive(Clone, Copy, Debug)]
pub struct ToolData {
    /// The skill it harvests with. `Mining` covers both ore and sand: which of the
    /// two a swing is comes from the *tile*, not the tool, exactly as in ServUO.
    pub skill: Skill,
    /// The fewest swings a fresh one holds — ServUO's `InitMinUses`.
    pub min_uses: u16,
    /// And the most.
    pub max_uses: u16,
}

/// The tool row for an item graphic, or `None` for anything that is not one.
///
/// The axes are **derived** from [`crate::weapon`]'s `is_axe` rather than listed
/// again: an axe you can swing at an orc and not at a tree is the kind of split
/// two hand-kept tables produce, and the mount table already taught that lesson.
#[must_use]
pub fn tool_data(graphic: Graphic) -> Option<ToolData> {
    if let Some(row) = TOOLS.iter().find(|(art, _)| *art == graphic) {
        return Some(row.1);
    }
    // Every axe chops wood — ServUO's `Lumberjacking` accepts any `BaseAxe`, and
    // the weapon table already knows which weapons those are.
    if crate::weapon::weapon_data(graphic).is_some_and(|w| w.is_axe) {
        return Some(ToolData {
            skill: Skill::Lumberjacking,
            min_uses: AXE_MIN_USES,
            max_uses: AXE_MAX_USES,
        });
    }
    None
}

/// An axe is a weapon first: it does not wear out on the *tree* in ServUO unless
/// the shard says so, but it does hold a count here so a hatchet bought to chop
/// with behaves like the pickaxe beside it on the shelf. ServUO's
/// `BaseHarvestTool` bounds.
const AXE_MIN_USES: u16 = 50;
/// The top of that range.
const AXE_MAX_USES: u16 = 100;

/// The purpose-built tools, each with `BaseHarvestTool`'s `50..=100` uses.
/// A hatchet and the axe classes come through [`tool_data`]'s `is_axe` branch.
#[rustfmt::skip]
static TOOLS: &[(Graphic, ToolData)] = &[
    (Graphic(0x0E86), ToolData { skill: Skill::Mining,  min_uses: 50, max_uses: 100 }), // pickaxe
    (Graphic(0x0E85), ToolData { skill: Skill::Mining,  min_uses: 50, max_uses: 100 }), // pickaxe (flipped art)
    (Graphic(0x0F39), ToolData { skill: Skill::Mining,  min_uses: 50, max_uses: 100 }), // shovel
    (Graphic(0x0F3A), ToolData { skill: Skill::Mining,  min_uses: 50, max_uses: 100 }), // shovel (flipped art)
    (Graphic(0x0DC0), ToolData { skill: Skill::Fishing, min_uses: 50, max_uses: 100 }), // fishing pole
    (Graphic(0x0DBF), ToolData { skill: Skill::Fishing, min_uses: 50, max_uses: 100 }), // fishing pole (flipped art)
];

// ---------------------------------------------------------------------------
// Banks
// ---------------------------------------------------------------------------

/// One block of ground's stock — ServUO's `HarvestBank`.
#[derive(Clone, Copy, Debug)]
pub struct Bank {
    /// What a full bank holds, rolled once between the definition's bounds.
    pub maximum: u16,
    /// What is left.
    pub current: u16,
    /// The tick it repays on, once something has been taken.
    pub next_respawn: WorldTick,
    /// Which vein this block holds, as an index into the definition's `veins`.
    pub vein: VeinIdx,
}

/// Every facet's harvest banks.
///
/// **Deliberately not persisted, exactly as ServUO does not persist them**: a
/// restart repays every vein on the shard. That is a real consequence and it is
/// written here rather than left to be rediscovered as a bug — a bank is derived
/// state about a patch of ground, not something a player owns, and saving it would
/// mean saving a row for every 8×8 block anyone has ever swung at.
#[derive(Default, Debug)]
pub struct Banks {
    banks: HashMap<(HarvestKind, u16, u16), Bank>,
}

/// An inclusive gameplay range represented as the bound [`Rng::below`] needs.
///
/// `None` from [`RngSpan::inclusive`] means the definition is backwards or its
/// width cannot fit the generator. Keeping that check separate from drawing
/// prevents either mistake from silently becoming a one-value range.
#[derive(Clone, Copy, Debug)]
struct RngSpan(NonZeroU32);

impl RngSpan {
    fn inclusive(min: u64, max: u64) -> Option<Self> {
        let width = max.checked_sub(min)?.checked_add(1)?;
        Some(Self(NonZeroU32::new(u32::try_from(width).ok()?)?))
    }

    fn draw(self, rng: &mut Rng) -> u32 {
        rng.below(self.0.get())
    }
}

/// Roll a fresh bank's capacity from its definition.
fn roll_maximum(def: &HarvestDef, rng: &mut Rng) -> u16 {
    let span = RngSpan::inclusive(u64::from(def.min_total), u64::from(def.max_total))
        .expect("a harvest total range is ordered and fits the world's RNG");
    let offset = u16::try_from(span.draw(rng)).expect("a draw inside a range of u16 totals fits u16");
    def.min_total
        .checked_add(offset)
        .expect("a draw inside the total range does not exceed its maximum")
}

/// Roll how long a depleted bank waits to repay.
fn roll_respawn(def: &HarvestDef, rng: &mut Rng) -> u64 {
    let span = RngSpan::inclusive(def.min_respawn, def.max_respawn)
        .expect("a harvest respawn range is ordered and fits the world's RNG");
    def.min_respawn
        .checked_add(u64::from(span.draw(rng)))
        .expect("a draw inside the respawn range does not exceed its maximum")
}

impl Banks {
    /// The bank covering a point for a definition, creating it on first use.
    ///
    /// `facet` only seeds the positional vein, so two facets' banks over the same
    /// coordinates do not hold the same ore — they are already separate entries,
    /// since `Banks` is per-facet.
    pub fn get(
        &mut self,
        def: &HarvestDef,
        x: u16,
        y: u16,
        facet: Facet,
        now: WorldTick,
        rng: &mut Rng,
    ) -> &mut Bank {
        let key = (def.kind, x / def.bank_w, y / def.bank_h);
        let bank = self.banks.entry(key).or_insert_with(|| {
            let maximum = roll_maximum(def, rng);
            Bank {
                maximum,
                current: maximum,
                next_respawn: WorldTick::ZERO,
                vein: default_vein(def, key.1, key.2, facet),
            }
        });
        bank.check_respawn(def, now, rng);
        bank
    }

    /// How many banks are live. Only tests and diagnostics care.
    #[must_use]
    pub fn len(&self) -> usize {
        self.banks.len()
    }

    /// Whether nothing has been harvested yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.banks.is_empty()
    }
}

impl Bank {
    /// Repay the bank if its clock has run out — ServUO's `CheckRespawn`, called
    /// before every read.
    fn check_respawn(&mut self, def: &HarvestDef, now: WorldTick, rng: &mut Rng) {
        if self.current == self.maximum || now < self.next_respawn {
            return;
        }
        self.current = self.maximum;
        if def.randomize_veins {
            self.vein = roll_vein(def, rng.below(10_000));
        }
    }

    /// Take `amount` out, starting the respawn clock on the first bite out of a
    /// full bank — ServUO's `Consume`, where a bank already being worked keeps the
    /// clock it started with rather than pushing it back on every swing.
    pub fn consume(&mut self, def: &HarvestDef, amount: u16, now: WorldTick, rng: &mut Rng) {
        if self.current == self.maximum {
            self.next_respawn = now + roll_respawn(def, rng);
        }
        self.current = self.current.saturating_sub(amount);
    }
}

/// Which vein a block of ground holds when veins are *not* randomized.
///
/// ServUO seeds a `Random` with `(x * 17) + (y * 11) + (map * 3)` and takes one
/// draw, so the answer is a fixed property of the coordinates. Keeping that
/// property matters more than reproducing C#'s generator: a bank is not saved, so
/// a vein rolled on the world's `Rng` would move at every restart, and a valorite
/// vein that wanders is a different game. This is a small integer hash over the
/// same three inputs, which has the property ServUO's arithmetic was there for.
fn default_vein(def: &HarvestDef, bank_x: u16, bank_y: u16, facet: Facet) -> VeinIdx {
    if def.veins.len() == 1 {
        return VeinIdx(0);
    }
    // `.0` at the arithmetic leaf: the seed wants the facet *number*, the way
    // ServUO's `map * 3` term did. The domain type is carried to here and no
    // further, which is the point — nothing downstream of this line is a facet.
    let mut h = u64::from(bank_x) * 17 + u64::from(bank_y) * 11 + u64::from(facet.0) * 3;
    // A cheap avalanche (splitmix64's finaliser): the raw sum alone is smooth
    // enough that neighbouring blocks would share a vein in stripes.
    h = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    roll_vein(def, u32::try_from(h % 10_000).unwrap_or(0))
}

/// Pick a vein from a draw in hundredths of a percent — ServUO's `GetVeinFrom`.
fn roll_vein(def: &HarvestDef, mut draw: u32) -> VeinIdx {
    for (index, vein) in def.veins.iter().enumerate() {
        if draw <= vein.chance {
            return VeinIdx(index);
        }
        draw -= vein.chance;
    }
    VeinIdx(def.veins.len() - 1)
}

// ---------------------------------------------------------------------------
// The definitions
// ---------------------------------------------------------------------------

/// One and a bit seconds — `EffectDelay` on every definition but fishing's.
const BEAT_TICKS: u64 = (TICKS_PER_SECOND * 16) / 10;
/// Nine tenths of a second into the beat — `EffectSoundDelay`.
const SOUND_TICKS: u64 = (TICKS_PER_SECOND * 9) / 10;
/// A minute, in ticks. Respawn windows are written in minutes in ServUO.
const MINUTE: u64 = TICKS_PER_SECOND * 60;

/// The four systems, in the order [`definition_for`] tries them.
///
/// Ore comes before sand because a handful of tile ids (286–297) are in *both*
/// ServUO tables, and ServUO's own `GetDefinition` returns the first definition
/// that validates — with `OreAndStone` added first. Reproducing the order is
/// reproducing the behaviour.
static DEFINITIONS_ML: &[HarvestDef] = &[ORE, SAND, LUMBER_ML, FISHING];
/// The same, with the pre-ML lumber table: one wood, and a vein that does not move.
static DEFINITIONS_PRE_ML: &[HarvestDef] = &[ORE, SAND, LUMBER_PRE_ML, FISHING];

/// Mining, for ore and stone.
const ORE: HarvestDef = HarvestDef {
    kind: HarvestKind::Ore,
    skill: Skill::Mining,
    bank_w: 8,
    bank_h: 8,
    min_total: 10,
    max_total: 34,
    min_respawn: 10 * MINUTE,
    max_respawn: 20 * MINUTE,
    tiles: TileSet::List(MOUNTAIN_AND_CAVE_TILES),
    max_range: 2,
    consumed: 1,
    consumed_felucca: 2,
    place_at_feet: false,
    action: HarvestAction::Mine,
    sounds: &[SoundId(0x125), SoundId(0x126)],
    beats: 1,
    beat_ticks: BEAT_TICKS,
    sound_ticks: SOUND_TICKS,
    messages: HarvestMessages {
        no_resources: ClilocId(503_040),       // There is no metal here to mine.
        double_harvest: ClilocId(503_042),     // Someone has gotten to the metal before you.
        out_of_range: ClilocId(500_446),       // That is too far away.
        timed_out_of_range: ClilocId(503_041), // You have moved too far away to continue mining.
        fail: ClilocId(503_043),               // You loosen some rocks but fail to find any useable ore.
        pack_full: ClilocId(1_010_481),        // Your backpack is full, so the ore you mined is lost.
        tool_broke: ClilocId(1_044_038),       // You have worn out your tool!
    },
    resources: ORES,
    veins: ORE_VEINS,
    randomize_veins: false,
};

/// Mining, for sand.
const SAND: HarvestDef = HarvestDef {
    kind: HarvestKind::Sand,
    skill: Skill::Mining,
    bank_w: 8,
    bank_h: 8,
    min_total: 6,
    max_total: 13,
    min_respawn: 10 * MINUTE,
    max_respawn: 20 * MINUTE,
    tiles: TileSet::List(SAND_TILES),
    max_range: 2,
    consumed: 1,
    consumed_felucca: 2,
    place_at_feet: false,
    action: HarvestAction::Mine,
    sounds: &[SoundId(0x125), SoundId(0x126)],
    beats: 6,
    beat_ticks: BEAT_TICKS,
    sound_ticks: SOUND_TICKS,
    messages: HarvestMessages {
        no_resources: ClilocId(1_044_629),     // There is no sand here to mine.
        double_harvest: ClilocId(1_044_629),   // There is no sand here to mine.
        out_of_range: ClilocId(500_446),       // That is too far away.
        timed_out_of_range: ClilocId(503_041), // You have moved too far away to continue mining.
        fail: ClilocId(1_044_630), // You dig for a while but fail to find any of sufficient quality.
        pack_full: ClilocId(1_044_632), // Your backpack can't hold the sand, and it is lost!
        tool_broke: ClilocId(1_044_038), // You have worn out your tool!
    },
    resources: SANDS,
    veins: ONE_VEIN,
    randomize_veins: false,
};

/// Lumberjacking, from Mondain's Legacy on: seven woods, and a vein that re-rolls
/// when the bank repays.
const LUMBER_ML: HarvestDef = HarvestDef {
    kind: HarvestKind::Lumber,
    skill: Skill::Lumberjacking,
    // A tree owns its own stock. Nearby trunks must not make one another empty:
    // the click names a particular static, so depletion follows that tile.
    bank_w: 1,
    bank_h: 1,
    min_total: 20,
    max_total: 45,
    min_respawn: 20 * MINUTE,
    max_respawn: 30 * MINUTE,
    tiles: TileSet::List(TREE_TILES),
    max_range: 2,
    // One deliberately long chopping job replaces two of the old short jobs.
    // Throughput stays the same, while the player targets the tree half as often.
    consumed: 20,
    consumed_felucca: 40,
    place_at_feet: false,
    action: HarvestAction::Chop,
    sounds: &[SoundId(0x13E)],
    // Six 1.6-second beats give the chop six uninterrupted full cycles before
    // the larger bundle of logs arrives. Each impact sound falls 0.9 seconds
    // into its own cycle, matching the reference's per-effect sound timer.
    beats: 6,
    beat_ticks: BEAT_TICKS,
    sound_ticks: SOUND_TICKS,
    messages: HarvestMessages {
        no_resources: ClilocId(500_493),   // There's not enough wood here to harvest.
        double_harvest: ClilocId(500_493), // There's not enough wood here to harvest.
        out_of_range: ClilocId(500_446),   // That is too far away.
        timed_out_of_range: ClilocId(500_446), // That is too far away.
        fail: ClilocId(500_495), // You hack at the tree for a while, but fail to produce any useable wood.
        pack_full: ClilocId(500_497), // You can't place any wood into your backpack!
        tool_broke: ClilocId(500_499), // You broke your axe.
    },
    resources: WOODS,
    veins: WOOD_VEINS,
    randomize_veins: true,
};

/// Lumberjacking before Mondain's Legacy: a tree is a tree, and a log is a log.
///
/// ServUO writes this as an `if (Core.ML)` around the resource and vein tables,
/// with `RaceBonus` and `RandomizeVeins` both `Core.ML` as well — so the older
/// form is the same definition with one wood and a fixed vein.
const LUMBER_PRE_ML: HarvestDef = HarvestDef {
    resources: PLAIN_WOOD,
    veins: ONE_VEIN,
    randomize_veins: false,
    ..LUMBER_ML
};

/// Fishing.
const FISHING: HarvestDef = HarvestDef {
    kind: HarvestKind::Fish,
    skill: Skill::Fishing,
    bank_w: 8,
    bank_h: 8,
    min_total: 5,
    max_total: 15,
    min_respawn: 10 * MINUTE,
    max_respawn: 20 * MINUTE,
    tiles: TileSet::Ranges(WATER_TILES),
    max_range: 4,
    consumed: 1,
    consumed_felucca: 1,
    // The one definition that catches its yield rather than digging it out: a fish
    // too big for the pack lands at your feet instead of being thrown back.
    place_at_feet: true,
    action: HarvestAction::Fish,
    sounds: &[],
    beats: 1,
    // ServUO gives fishing an `EffectDelay` of zero and an eight-second
    // `EffectSoundDelay`, which together are one long cast rather than a beat and
    // a splash. The beat is that eight seconds; there is no sound to place inside
    // it.
    beat_ticks: TICKS_PER_SECOND * 8,
    sound_ticks: 0,
    messages: HarvestMessages {
        no_resources: ClilocId(503_172),       // The fish don't seem to be biting here.
        double_harvest: ClilocId(503_172),     // The fish don't seem to be biting here.
        out_of_range: ClilocId(500_976),       // You need to be closer to the water to fish!
        timed_out_of_range: ClilocId(500_976), // You need to be closer to the water to fish!
        fail: ClilocId(503_171),               // You fish a while, but fail to catch anything.
        pack_full: ClilocId(503_176),          // You do not have room in your backpack for a fish.
        tool_broke: ClilocId(503_174),         // You broke your fishing pole.
    },
    resources: FISHES,
    veins: ONE_VEIN,
    randomize_veins: false,
};

/// The nine ores, from ServUO's `Mining` resource table with
/// `CraftResources.GetHue` supplying each colour.
///
/// **All nine share one art.** ServUO rolls between four pile graphics
/// (`BaseOre.RandomSize`) and swaps the art as a pile grows or shrinks
/// (`BaseOre.OnDragDrop`). Without that swap, rolling the art would leave a miner
/// with four piles of iron ore that refuse to merge, because a merge matches on
/// graphic *and* hue — so this takes the common one (75% of ServUO's roll) and
/// the pile-size art is a recorded gap, not an oversight.
/// Public because two crates read the metals: `skills` pays a miner in them, and
/// `crafting` smelts them into ingots and offers them as a smith's material axis.
/// One table, so a hue can never mean valorite on the ground and copper at the
/// forge.
#[rustfmt::skip]
pub static ORES: &[HarvestResource] = &[
    ore(   0,    0, 1000, 1_007_072, 0x0000), // iron
    ore( 650,  250, 1050, 1_007_073, 0x0973), // dull copper
    ore( 700,  300, 1100, 1_007_074, 0x0966), // shadow iron
    ore( 750,  350, 1150, 1_007_075, 0x096D), // copper
    ore( 800,  400, 1200, 1_007_076, 0x0972), // bronze
    ore( 850,  450, 1250, 1_007_077, 0x08A5), // gold
    ore( 900,  500, 1300, 1_007_078, 0x0979), // agapite
    ore( 950,  550, 1350, 1_007_079, 0x089F), // verite
    ore( 990,  590, 1390, 1_007_080, 0x08AB), // valorite
];

/// The item art every ore pile takes — ServUO's most common `RandomSize` result.
pub const ORE_GRAPHIC: Graphic = Graphic(0x19B9);
/// A log, ServUO's `BaseLog`.
pub const LOG_GRAPHIC: Graphic = Graphic(0x1BDD);
/// A pile of sand.
pub const SAND_GRAPHIC: Graphic = Graphic(0x423A);
/// A fish. ServUO rolls between `0x09CC..0x09CF`; one art, for the reason the ore
/// table gives.
pub const FISH_GRAPHIC: Graphic = Graphic(0x09CC);

/// An ore row, so the table above reads as data.
const fn ore(req: i32, min: i32, max: i32, cliloc: u32, hue: u16) -> HarvestResource {
    HarvestResource {
        req_skill: req,
        min_skill: min,
        max_skill: max,
        success_cliloc: ClilocId(cliloc),
        graphic: ORE_GRAPHIC,
        hue: Hue(hue),
    }
}

/// ServUO's nine ore veins: iron half the ground, valorite one part in seventy,
/// and every richer vein disappointing into iron one swing in two hundred.
#[rustfmt::skip]
static ORE_VEINS: &[HarvestVein] = &[
    vein(4960,  0, ResourceIdx(0), None),                    // iron
    vein(1120, 50, ResourceIdx(1), Some(ResourceIdx(0))), // dull copper
    vein( 980, 50, ResourceIdx(2), Some(ResourceIdx(0))), // shadow iron
    vein( 840, 50, ResourceIdx(3), Some(ResourceIdx(0))), // copper
    vein( 700, 50, ResourceIdx(4), Some(ResourceIdx(0))), // bronze
    vein( 560, 50, ResourceIdx(5), Some(ResourceIdx(0))), // gold
    vein( 420, 50, ResourceIdx(6), Some(ResourceIdx(0))), // agapite
    vein( 280, 50, ResourceIdx(7), Some(ResourceIdx(0))), // verite
    vein( 140, 50, ResourceIdx(8), Some(ResourceIdx(0))), // valorite
];

/// The seven woods — ServUO's ML table, with `CraftResources.GetHue`'s colours.
#[rustfmt::skip]
static WOODS: &[HarvestResource] = &[
    wood(   0,   0, 1000, 1_072_540, 0x0000), // regular
    wood( 650, 250, 1050, 1_072_541, 0x07DA), // oak
    wood( 800, 400, 1200, 1_072_542, 0x04A7), // ash
    wood( 950, 550, 1350, 1_072_543, 0x04A8), // yew
    wood(1000, 600, 1400, 1_072_544, 0x04A9), // heartwood
    wood(1000, 600, 1400, 1_072_545, 0x04AA), // bloodwood
    wood(1000, 600, 1400, 1_072_546, 0x047F), // frostwood
];

/// The one wood a pre-ML tree gives, ServUO's `500498` line.
static PLAIN_WOOD: &[HarvestResource] = &[HarvestResource {
    req_skill: 0,
    min_skill: 0,
    max_skill: 1000,
    success_cliloc: ClilocId(500_498), // You put some logs in your backpack.
    graphic: LOG_GRAPHIC,
    hue: Hue(0),
}];

/// A wood row.
const fn wood(req: i32, min: i32, max: i32, cliloc: u32, hue: u16) -> HarvestResource {
    HarvestResource {
        req_skill: req,
        min_skill: min,
        max_skill: max,
        success_cliloc: ClilocId(cliloc),
        graphic: LOG_GRAPHIC,
        hue: Hue(hue),
    }
}

/// ServUO's seven wood veins.
#[rustfmt::skip]
static WOOD_VEINS: &[HarvestVein] = &[
    vein(4900,  0, ResourceIdx(0), None),                    // regular
    vein(3000, 50, ResourceIdx(1), Some(ResourceIdx(0))), // oak
    vein(1000, 50, ResourceIdx(2), Some(ResourceIdx(0))), // ash
    vein( 500, 50, ResourceIdx(3), Some(ResourceIdx(0))), // yew
    vein( 300, 50, ResourceIdx(4), Some(ResourceIdx(0))), // heartwood
    vein( 200, 50, ResourceIdx(5), Some(ResourceIdx(0))), // bloodwood
    vein( 100, 50, ResourceIdx(6), Some(ResourceIdx(0))), // frostwood
];

/// Sand, one grade, and it wants a real miner: ServUO bands it `70.0..100.0`.
static SANDS: &[HarvestResource] = &[HarvestResource {
    req_skill: 1000,
    min_skill: 700,
    max_skill: 1000,
    success_cliloc: ClilocId(1_044_631), // You carefully dig up some workable sand.
    graphic: SAND_GRAPHIC,
    hue: Hue(0),
}];

/// Fish, one grade, banded so a beginner still catches something.
static FISHES: &[HarvestResource] = &[HarvestResource {
    req_skill: 0,
    min_skill: 0,
    max_skill: 1200,
    success_cliloc: ClilocId(1_043_297), // You pull out a heavy and beautiful fish!
    graphic: FISH_GRAPHIC,
    hue: Hue(0),
}];

/// The single vein the one-resource definitions have.
static ONE_VEIN: &[HarvestVein] = &[HarvestVein {
    chance: 10_000,
    fallback_chance: 0,
    primary: ResourceIdx(0),
    fallback: None,
}];

/// A vein row.
const fn vein(
    chance: u32,
    fallback_chance: u32,
    primary: ResourceIdx,
    fallback: Option<ResourceIdx>,
) -> HarvestVein {
    HarvestVein {
        chance,
        fallback_chance,
        primary,
        fallback,
    }
}

// ---------------------------------------------------------------------------
// Tile tables
// ---------------------------------------------------------------------------

// The four tile tables are `data/harvest_tiles.json`; `build.rs` emits them
// before this crate compiles. They are `contains` tables of ServUO ids and
// nothing more — kept in ServUO's order, so the two can still be diffed.
include!(concat!(env!("OUT_DIR"), "/harvest_tiles.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_bank_ranges_are_valid_rng_spans() {
        for def in DEFINITIONS_ML.iter().chain(DEFINITIONS_PRE_ML) {
            assert!(
                RngSpan::inclusive(u64::from(def.min_total), u64::from(def.max_total)).is_some(),
                "{:?} has an invalid total range",
                def.kind
            );
            assert!(
                RngSpan::inclusive(def.min_respawn, def.max_respawn).is_some(),
                "{:?} has an invalid respawn range",
                def.kind
            );
        }

        assert!(
            RngSpan::inclusive(5, 4).is_none(),
            "a backwards range is not a roll"
        );
        assert!(
            RngSpan::inclusive(0, u64::from(u32::MAX)).is_none(),
            "an inclusive width of 2^32 cannot be represented by Rng::below"
        );
    }

    #[test]
    fn bank_rolls_stay_inside_both_inclusive_bounds() {
        let def = definition(HarvestKind::Ore, true);
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            assert!((def.min_total..=def.max_total).contains(&roll_maximum(def, &mut rng)));
            assert!((def.min_respawn..=def.max_respawn).contains(&roll_respawn(def, &mut rng)));
        }
    }

    #[test]
    fn the_tile_tables_hold_the_tiles_they_are_named_for() {
        // Pinned against ServUO's own arrays, the `NO_SHOOT` rule: a table of
        // several hundred magic numbers is exactly where a transcription slip
        // hides, and it surfaces months later as "mining does not work in Minoc".
        // Mountain and cave land, and the Ter Mur cave statics at the end.
        assert!(MOUNTAIN_AND_CAVE_TILES.contains(&HarvestTile(220)));
        assert!(MOUNTAIN_AND_CAVE_TILES.contains(&HarvestTile(2105)));
        assert!(MOUNTAIN_AND_CAVE_TILES.contains(&HarvestTile(0x454F)));
        // Sand runs from 22, and takes in the desert at 1650.
        assert!(SAND_TILES.contains(&HarvestTile(22)));
        assert!(SAND_TILES.contains(&HarvestTile(1650)));
        // Trees are statics, so every id already carries the 0x4000 bit.
        assert!(TREE_TILES.iter().all(|&t| t >= HarvestTile(0x4000)));
        assert!(TREE_TILES.contains(&HarvestTile(0x4CCA)));
        assert!(TREE_TILES.contains(&HarvestTile(0x52C7)));
        // Water is ranges: the open-sea land tiles and the deep-water statics.
        assert!(TileSet::Ranges(WATER_TILES).contains(HarvestTile(0x00A9)));
        assert!(TileSet::Ranges(WATER_TILES).contains(HarvestTile(0x75D5)));
        assert!(!TileSet::Ranges(WATER_TILES).contains(HarvestTile(0x00A7)));
        assert!(!TileSet::Ranges(WATER_TILES).contains(HarvestTile(0x75D6)));
    }

    #[test]
    fn a_tile_finds_its_definition() {
        // A mountain's land tile is ore; the same number read as a *static* is
        // not, because a static is matched with the 0x4000 bit set.
        let ore = definition_for(Graphic(220), TileSource::Land, true).expect("mountain land is minable");
        assert_eq!(ore.kind, HarvestKind::Ore);
        assert!(definition_for(Graphic(220), TileSource::Static, true).is_none());
        // A tree is only ever a static, and 0x4CCA & 0x3FFF | 0x4000 is itself.
        let tree = definition_for(Graphic(0x4CCA), TileSource::Static, true).expect("a tree is choppable");
        assert_eq!(tree.kind, HarvestKind::Lumber);
        // Ordinary grass is nothing.
        assert!(definition_for(Graphic(3), TileSource::Land, true).is_none());
    }

    #[test]
    fn ore_beats_sand_where_the_two_tables_overlap() {
        // 286..294 are in both of ServUO's arrays and it resolves them by table
        // order, `OreAndStone` first. If this ever flips, a stretch of Britannia
        // silently becomes a sand pit.
        assert!(MOUNTAIN_AND_CAVE_TILES.contains(&HarvestTile(286)));
        assert!(SAND_TILES.contains(&HarvestTile(286)));
        assert_eq!(
            definition_for(Graphic(286), TileSource::Land, true).map(|d| d.kind),
            Some(HarvestKind::Ore)
        );
    }

    #[test]
    fn every_vein_table_is_a_whole_hundred_percent() {
        for def in DEFINITIONS_ML.iter().chain(DEFINITIONS_PRE_ML) {
            let total: u32 = def.veins.iter().map(|v| v.chance).sum();
            assert_eq!(total, 10_000, "{:?} veins do not sum to 100%", def.kind);
        }
    }

    #[test]
    fn a_veins_indices_point_at_real_resources() {
        for def in DEFINITIONS_ML.iter().chain(DEFINITIONS_PRE_ML) {
            for vein in def.veins {
                assert!(vein.primary.0 < def.resources.len(), "{:?}", def.kind);
                if let Some(fallback) = vein.fallback {
                    assert!(fallback.0 < def.resources.len(), "{:?}", def.kind);
                }
            }
        }
    }

    #[test]
    fn a_blocks_vein_is_the_same_after_a_restart() {
        // The property the positional roll exists for. Banks are not saved, so a
        // vein rolled on the world's generator would move at every reboot — and a
        // valorite vein that wanders is a different game. Same coordinates, same
        // answer, with no shared state between the two calls.
        let def = definition(HarvestKind::Ore, true);
        for (x, y) in [(0, 0), (37, 12), (600, 601), (5000, 4096)] {
            assert_eq!(
                default_vein(def, x, y, Facet(0)),
                default_vein(def, x, y, Facet(0)),
                "the vein at ({x}, {y}) moved"
            );
        }
        // And two facets do not agree, or every dungeon would mirror Felucca.
        let differ =
            (0..64u16).any(|n| default_vein(def, n, n, Facet(0)) != default_vein(def, n, n, Facet(1)));
        assert!(differ, "the facet seed does nothing");
    }

    #[test]
    fn the_positional_vein_is_not_all_one_ore() {
        // A hash that stripes or collapses would hand the whole map iron and
        // nothing would ever look wrong — the "statistical test needs a companion"
        // rule. Iron is 49.6% of ServUO's table, so a fair spread over a few
        // thousand blocks must find several distinct veins and must not be iron
        // everywhere.
        let def = definition(HarvestKind::Ore, true);
        let mut seen = [0usize; 9];
        for x in 0..64u16 {
            for y in 0..64u16 {
                seen[default_vein(def, x, y, Facet(0)).0] += 1;
            }
        }
        assert!(
            seen.iter().filter(|n| **n > 0).count() >= 7,
            "only {} of nine veins ever occur: {seen:?}",
            seen.iter().filter(|n| **n > 0).count()
        );
        // Iron is common, not universal: within a few points of its 49.6%.
        let iron = seen[0] as f64 / 4096.0;
        assert!((0.40..0.60).contains(&iron), "iron is {iron} of the ground");
    }

    #[test]
    fn a_bank_runs_dry_and_repays_on_the_tick_counter() {
        let def = definition(HarvestKind::Ore, true);
        let mut rng = Rng::new(1);
        let mut banks = Banks::default();
        let full = {
            let bank = banks.get(def, 10, 10, Facet(0), WorldTick::ZERO, &mut rng);
            bank.maximum
        };
        // Empty it a swing at a time, from tick zero.
        for _ in 0..full {
            let bank = banks.get(def, 10, 10, Facet(0), WorldTick::ZERO, &mut rng);
            assert!(bank.current > 0);
            bank.consume(def, 1, WorldTick::ZERO, &mut rng);
        }
        assert_eq!(
            banks
                .get(def, 10, 10, Facet(0), WorldTick::ZERO, &mut rng)
                .current,
            0
        );
        // Still empty a minute later; full again after the longest window.
        assert_eq!(
            banks
                .get(def, 10, 10, Facet(0), WorldTick::from_raw(MINUTE), &mut rng)
                .current,
            0
        );
        assert_eq!(
            banks
                .get(def, 10, 10, Facet(0), WorldTick::from_raw(20 * MINUTE), &mut rng,)
                .current,
            full
        );
    }

    #[test]
    fn one_bank_covers_a_whole_block_of_tiles() {
        // 8x8 for ore: mining the far corner of the block draws on the same stock,
        // which is the whole reason a bank is not per-tile.
        let def = definition(HarvestKind::Ore, true);
        let mut rng = Rng::new(7);
        let mut banks = Banks::default();
        banks
            .get(def, 16, 16, Facet(0), WorldTick::ZERO, &mut rng)
            .consume(def, 5, WorldTick::ZERO, &mut rng);
        assert_eq!(banks.len(), 1);
        let neighbour = banks.get(def, 23, 23, Facet(0), WorldTick::ZERO, &mut rng);
        assert_eq!(neighbour.current, neighbour.maximum - 5);
        assert_eq!(banks.len(), 1);
        // And the next block along is its own.
        banks.get(def, 24, 16, Facet(0), WorldTick::ZERO, &mut rng);
        assert_eq!(banks.len(), 2);
    }

    #[test]
    fn every_tree_has_its_own_depleting_stock() {
        let def = definition(HarvestKind::Lumber, true);
        assert_eq!((def.bank_w, def.bank_h), (1, 1));

        let mut rng = Rng::new(7);
        let mut banks = Banks::default();
        let full = banks
            .get(def, 16, 16, Facet(0), WorldTick::ZERO, &mut rng)
            .maximum;
        banks
            .get(def, 16, 16, Facet(0), WorldTick::ZERO, &mut rng)
            .consume(def, full, WorldTick::ZERO, &mut rng);

        assert_eq!(
            banks
                .get(def, 16, 16, Facet(0), WorldTick::ZERO, &mut rng)
                .current,
            0,
            "the chopped tree should stay empty until its respawn"
        );
        assert!(
            banks
                .get(def, 17, 16, Facet(0), WorldTick::ZERO, &mut rng)
                .current
                > 0,
            "the tree next door should keep its own stock"
        );
        assert_eq!(banks.len(), 2);
    }

    #[test]
    fn a_pickaxe_mines_and_an_axe_chops() {
        assert_eq!(tool_data(Graphic(0x0E86)).map(|t| t.skill), Some(Skill::Mining));
        assert_eq!(tool_data(Graphic(0x0DC0)).map(|t| t.skill), Some(Skill::Fishing));
        // Derived from the weapon table's `is_axe`, not listed twice: a hatchet.
        assert_eq!(
            tool_data(Graphic(0x0F43)).map(|t| t.skill),
            Some(Skill::Lumberjacking)
        );
        // A katana is a weapon and nothing else.
        assert!(tool_data(Graphic(0x13FF)).is_none());
    }
}
