//! The skill table: what the fifty-eight skills *are*.
//!
//! Data and pure functions over it, so it sits here with [`title`](crate::title)
//! and the body table rather than in `skills`: several crates name a skill (combat
//! rolls a weapon skill, magic rolls Magery, the wire draws the window) and only
//! one owns the rules for training one. The *rules* — the check, the gain curve,
//! the stat gain — are the `skills` crate.
//!
//! Ported from ServUO's `Server/Skills.cs`: the `SkillName` enum and the
//! fifty-eight-row `SkillInfo` table, verbatim. The ids are not ours to choose —
//! they are the client's own indices into `skills.mul`, and they ride on the
//! `0x3A` packet in both directions, so a wrong one trains the wrong bar on
//! somebody's screen.
//!
//! # Fixed point
//!
//! Everything here is an integer. A tick has to replay roll for roll, and the rest
//! of the engine already works this way (`combat::weapons` in per-mille). ServUO's
//! doubles are carried as:
//!
//! - **scales** in hundredths (`7.5` → `750`),
//! - **gains** in thousandths (`1.625` → `1625`),
//! - **`gain_factor`** in per-mille (`1.0` → `1000`).

/// Which of the three stats a skill leans on — ServUO's `StatCode`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StatCode {
    /// Strength.
    Str,
    /// Dexterity.
    Dex,
    /// Intelligence.
    Int,
}

/// A skill, by the id the client knows it as.
///
/// The numbering is OSI's, not alphabetical and not ours: `skills.mul` fixes it,
/// and ids 0–48 are the pre-AoS set, 49–51 AoS, 52–53 SE, 54 ML, 55–57 SA.
///
/// `PartialOrd`/`Ord` follow the `#[repr(u8)]` discriminant, i.e. `id()` order —
/// what [`components::Skills::ids`](crate::components::Skills::ids) sorts by.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum Skill {
    /// Alchemy — "Alchemist".
    Alchemy = 0,
    /// Anatomy — "Biologist".
    Anatomy = 1,
    /// Animal Lore — "Naturalist".
    AnimalLore = 2,
    /// Item Identification — "Merchant".
    ItemId = 3,
    /// Arms Lore — "Weapon Master".
    ArmsLore = 4,
    /// Parrying — "Duelist".
    Parry = 5,
    /// Begging — "Beggar".
    Begging = 6,
    /// Blacksmithy — "Blacksmith".
    Blacksmith = 7,
    /// Bowcraft/Fletching — "Bowyer".
    Fletching = 8,
    /// Peacemaking — "Pacifier".
    Peacemaking = 9,
    /// Camping — "Explorer".
    Camping = 10,
    /// Carpentry — "Carpenter".
    Carpentry = 11,
    /// Cartography — "Cartographer".
    Cartography = 12,
    /// Cooking — "Chef".
    Cooking = 13,
    /// Detecting Hidden — "Scout".
    DetectHidden = 14,
    /// Discordance — "Demoralizer".
    Discordance = 15,
    /// Evaluating Intelligence — "Scholar".
    EvalInt = 16,
    /// Healing — "Healer".
    Healing = 17,
    /// Fishing — "Fisherman".
    Fishing = 18,
    /// Forensic Evaluation — "Detective".
    Forensics = 19,
    /// Herding — "Shepherd".
    Herding = 20,
    /// Hiding — "Shade".
    Hiding = 21,
    /// Provocation — "Rouser".
    Provocation = 22,
    /// Inscription — "Scribe".
    Inscribe = 23,
    /// Lockpicking — "Infiltrator".
    Lockpicking = 24,
    /// Magery — "Mage".
    Magery = 25,
    /// Resisting Spells — "Warder".
    MagicResist = 26,
    /// Tactics — "Tactician".
    Tactics = 27,
    /// Snooping — "Spy".
    Snooping = 28,
    /// Musicianship — "Bard".
    Musicianship = 29,
    /// Poisoning — "Assassin".
    Poisoning = 30,
    /// Archery — "Archer".
    Archery = 31,
    /// Spirit Speak — "Medium".
    SpiritSpeak = 32,
    /// Stealing — "Pickpocket".
    Stealing = 33,
    /// Tailoring — "Tailor".
    Tailoring = 34,
    /// Animal Taming — "Tamer".
    AnimalTaming = 35,
    /// Taste Identification — "Praegustator".
    TasteId = 36,
    /// Tinkering — "Tinker".
    Tinkering = 37,
    /// Tracking — "Ranger".
    Tracking = 38,
    /// Veterinary — "Veterinarian".
    Veterinary = 39,
    /// Swordsmanship — "Swordsman".
    Swords = 40,
    /// Mace Fighting — "Armsman".
    Macing = 41,
    /// Fencing — "Fencer".
    Fencing = 42,
    /// Wrestling — "Wrestler".
    Wrestling = 43,
    /// Lumberjacking — "Lumberjack".
    Lumberjacking = 44,
    /// Mining — "Miner".
    Mining = 45,
    /// Meditation — "Stoic".
    Meditation = 46,
    /// Stealth — "Rogue".
    Stealth = 47,
    /// Remove Trap — "Trap Specialist".
    RemoveTrap = 48,
    /// Necromancy — "Necromancer".
    Necromancy = 49,
    /// Focus — "Driven".
    Focus = 50,
    /// Chivalry — "Paladin".
    Chivalry = 51,
    /// Bushido — "Samurai".
    Bushido = 52,
    /// Ninjitsu — "Ninja".
    Ninjitsu = 53,
    /// Spellweaving — "Arcanist".
    Spellweaving = 54,
    /// Mysticism — "Mystic".
    Mysticism = 55,
    /// Imbuing — "Artificer".
    Imbuing = 56,
    /// Throwing — "Bladeweaver".
    Throwing = 57,
}

/// How many skills there are — the length of [`SKILLS`], and the id one past the
/// last. Not what a *client* is sent: an older client's skill array is shorter,
/// which is `openshard_protocol::skill_count`'s job.
pub const SKILL_COUNT: usize = 58;

impl Skill {
    /// The wire id — what `Skills` is keyed by and what the `0x3A` carries.
    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// The skill an id names, or `None` for an id past the table.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Alchemy,
            1 => Self::Anatomy,
            2 => Self::AnimalLore,
            3 => Self::ItemId,
            4 => Self::ArmsLore,
            5 => Self::Parry,
            6 => Self::Begging,
            7 => Self::Blacksmith,
            8 => Self::Fletching,
            9 => Self::Peacemaking,
            10 => Self::Camping,
            11 => Self::Carpentry,
            12 => Self::Cartography,
            13 => Self::Cooking,
            14 => Self::DetectHidden,
            15 => Self::Discordance,
            16 => Self::EvalInt,
            17 => Self::Healing,
            18 => Self::Fishing,
            19 => Self::Forensics,
            20 => Self::Herding,
            21 => Self::Hiding,
            22 => Self::Provocation,
            23 => Self::Inscribe,
            24 => Self::Lockpicking,
            25 => Self::Magery,
            26 => Self::MagicResist,
            27 => Self::Tactics,
            28 => Self::Snooping,
            29 => Self::Musicianship,
            30 => Self::Poisoning,
            31 => Self::Archery,
            32 => Self::SpiritSpeak,
            33 => Self::Stealing,
            34 => Self::Tailoring,
            35 => Self::AnimalTaming,
            36 => Self::TasteId,
            37 => Self::Tinkering,
            38 => Self::Tracking,
            39 => Self::Veterinary,
            40 => Self::Swords,
            41 => Self::Macing,
            42 => Self::Fencing,
            43 => Self::Wrestling,
            44 => Self::Lumberjacking,
            45 => Self::Mining,
            46 => Self::Meditation,
            47 => Self::Stealth,
            48 => Self::RemoveTrap,
            49 => Self::Necromancy,
            50 => Self::Focus,
            51 => Self::Chivalry,
            52 => Self::Bushido,
            53 => Self::Ninjitsu,
            54 => Self::Spellweaving,
            55 => Self::Mysticism,
            56 => Self::Imbuing,
            57 => Self::Throwing,
            _ => return None,
        })
    }

    /// This skill's row in [`SKILLS`].
    #[must_use]
    pub const fn info(self) -> &'static SkillInfo {
        &SKILLS[self as usize]
    }

    /// The skill that goes by `name`, ignoring case and any spaces or slashes in
    /// it.
    ///
    /// For an operator typing a name rather than an id. The punctuation is
    /// dropped on both sides because the table's own spelling is the client's —
    /// "Item Identification", "Bowcraft/Fletching" — and nobody types either at
    /// a command prompt. It is a scan of fifty-eight rows, asked once when
    /// somebody presses Enter.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let squashed = |text: &str| {
            text.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        };
        let wanted = squashed(name);
        (0..SKILL_COUNT as u8)
            .filter_map(Self::from_id)
            .find(|skill| squashed(skill.info().name) == wanted)
    }
}

/// One skill's data — ServUO's `SkillInfo`, one row of its table.
#[derive(Clone, Copy, Debug)]
pub struct SkillInfo {
    /// What it is called: "Alchemy", "Item Identification".
    pub name: &'static str,
    /// The title a grandmaster earns: "Alchemist", "Merchant".
    pub title: &'static str,
    /// How much strength lends to the skill's effective value, in hundredths.
    pub str_scale: u32,
    /// How much dexterity lends, in hundredths.
    pub dex_scale: u32,
    /// How much intelligence lends, in hundredths.
    pub int_scale: u32,
    /// The ceiling on the whole stat bonus, in hundredths.
    ///
    /// ServUO's `StatTotal`, and the one place its arithmetic looks like a slip
    /// and is not: the constructor divides the three scales by 100 and then sums
    /// the **undivided** values into this field (`Server/Skills.cs:545-559`). The
    /// asymmetry is load-bearing — it makes the bonus cap out at the raw sum, so a
    /// mobile with stats above 100 stops gaining from them. Carried as-is; do not
    /// "fix" it into a sum of the scaled values.
    pub stat_total: u32,
    /// The chance weight that training this skill nudges strength, in thousandths.
    pub str_gain: u32,
    /// The same for dexterity.
    pub dex_gain: u32,
    /// The same for intelligence.
    pub int_gain: u32,
    /// A multiplier on how readily the skill trains, in per-mille. `1000` on every
    /// row ServUO ships; the column exists so a shard can slow one skill down.
    pub gain_factor: u32,
    /// The stat the ML gain mechanic tries first.
    pub primary: StatCode,
    /// The stat it falls back to, one time in four.
    pub secondary: StatCode,
    /// Whether the skill can be used straight from the window's button.
    ///
    /// Twenty-three of the fifty-eight; the rest answer cliloc 500014, "That skill
    /// cannot be used directly" — they are passive (Tactics), item-driven
    /// (Lockpicking, Mining) or opened by something else (Magery, Blacksmithy).
    /// ServUO expresses this as a `Callback` being non-null; a bool says the same
    /// thing without a function pointer in a `const`.
    pub usable: bool,
    /// Whether the skill may be used with a spell in flight. Spirit Speak alone.
    pub use_while_casting: bool,
}

/// The cliloc the client localizes a skill's name from — ServUO's
/// `SkillInfo.Localization`.
#[must_use]
pub const fn cliloc(id: u8) -> u32 {
    1_044_060 + id as u32
}

/// The data for a skill id, or `None` past the table.
#[must_use]
pub const fn info(id: u8) -> Option<&'static SkillInfo> {
    if (id as usize) < SKILL_COUNT {
        Some(&SKILLS[id as usize])
    } else {
        None
    }
}

// The table is `data/skills.json`; `build.rs` emits the `const`, with its
// documentation, before this crate compiles. It stays an array indexed by the
// `Skill` discriminant, so a row without a variant will not compile.
include!(concat!(env!("OUT_DIR"), "/skills.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name in the table finds its own row, and the punctuation the client's
    /// spelling carries is not something anybody has to type.
    #[test]
    fn a_skill_is_found_by_the_name_a_person_would_type() {
        for id in 0..SKILL_COUNT as u8 {
            let skill = Skill::from_id(id).expect("every id in range names a skill");
            assert_eq!(Skill::from_name(skill.info().name), Some(skill));
        }
        assert_eq!(Skill::from_name("mining"), Some(Skill::Mining));
        assert_eq!(Skill::from_name("  MINING "), Some(Skill::Mining));
        assert_eq!(
            Skill::from_name("bowcraftfletching"),
            Some(Skill::Fletching),
            "the slash in the table's own spelling"
        );
        assert_eq!(
            Skill::from_name("item identification"),
            Some(Skill::ItemId),
            "and the space in it"
        );
        assert_eq!(Skill::from_name("mimning"), None);
        assert_eq!(Skill::from_name(""), None, "the empty name is nobody's");
    }

    #[test]
    fn the_table_covers_every_id() {
        assert_eq!(SKILLS.len(), SKILL_COUNT);
        for id in u8::MIN..=u8::MAX {
            match Skill::from_id(id) {
                Some(skill) => {
                    assert!(usize::from(id) < SKILL_COUNT, "id {id} is outside the table");
                    assert_eq!(skill.id(), id, "the enum discriminant is the id");
                    assert!(!skill.info().name.is_empty());
                }
                None => assert!(usize::from(id) >= SKILL_COUNT, "id {id} is inside the table"),
            }
        }
    }

    #[test]
    fn the_combat_ids_are_the_clients_own() {
        // These five were wrong in the engine before the table existed, and wrong
        // silently: the roll trained the id it was given, so a swordsman's gains
        // showed up on the Mace Fighting bar. Pinned here because they are the
        // numbers on the wire, not an internal choice.
        assert_eq!(Skill::Tactics.id(), 27);
        assert_eq!(Skill::Swords.id(), 40);
        assert_eq!(Skill::Macing.id(), 41);
        assert_eq!(Skill::Fencing.id(), 42);
        assert_eq!(Skill::Wrestling.id(), 43);
        // And the four that were already right.
        assert_eq!(Skill::Anatomy.id(), 1);
        assert_eq!(Skill::Magery.id(), 25);
        assert_eq!(Skill::Archery.id(), 31);
        assert_eq!(Skill::Lumberjacking.id(), 44);
    }

    #[test]
    fn the_stat_total_is_the_undivided_sum() {
        // The asymmetry documented on `SkillInfo::stat_total`. Parrying scales
        // 7.5 str + 2.5 dex, and its ceiling is their raw sum, 10.0 — which in
        // hundredths is 1000, not the 100 a "sum of the scaled values" reading
        // would give. If this ever reads 100, someone has "fixed" ServUO.
        let parry = Skill::Parry.info();
        assert_eq!((parry.str_scale, parry.dex_scale, parry.int_scale), (750, 250, 0));
        assert_eq!(parry.stat_total, 1000);
    }

    #[test]
    fn the_scales_and_gains_survived_the_fixed_point() {
        // Herding is the only row with three decimal places in a gain column, and
        // Tailoring the only one ServUO itself rounds (0.375 written as 0.38).
        let herding = Skill::Herding.info();
        assert_eq!(herding.str_gain, 1625, "1.625");
        assert_eq!(herding.str_scale, 1625, "16.25");
        assert_eq!(Skill::Tailoring.info().str_gain, 380, "ServUO writes 0.38");
        // Every row ships gain_factor 1.0.
        assert!(SKILLS.iter().all(|s| s.gain_factor == 1000));
    }

    #[test]
    fn twenty_three_skills_are_usable_from_the_button() {
        let usable: Vec<&str> = SKILLS.iter().filter(|s| s.usable).map(|s| s.name).collect();
        assert_eq!(usable.len(), 23, "{usable:?}");
        assert!(Skill::Hiding.info().usable);
        assert!(Skill::Meditation.info().usable);
        // Passive, item-driven and spellbook skills are not.
        assert!(!Skill::Tactics.info().usable);
        assert!(!Skill::Mining.info().usable);
        assert!(!Skill::Magery.info().usable);
    }

    #[test]
    fn only_spirit_speak_may_be_used_mid_cast() {
        let casting: Vec<&str> = SKILLS
            .iter()
            .filter(|s| s.use_while_casting)
            .map(|s| s.name)
            .collect();
        assert_eq!(casting, ["Spirit Speak"]);
    }

    #[test]
    fn a_skills_name_localizes_from_its_id() {
        assert_eq!(cliloc(Skill::Alchemy.id()), 1_044_060);
        assert_eq!(cliloc(Skill::Throwing.id()), 1_044_117);
    }
}
