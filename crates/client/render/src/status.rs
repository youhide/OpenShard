//! The player's status window, in both of the shapes the reference client
//! draws it in.
//!
//! `0x11` says the numbers and `0xBF 0x19` says the three arrows; this module
//! says only how those facts occupy the client's own frames. It does not know a
//! socket, a `WorldView`, or which player supplied them — the caller joins
//! those at the app/render boundary, as it does for [`crate::skills`].
//!
//! # Two frames, one layout module
//!
//! The reference client has two status windows and picks between them by
//! setting: [`Form::Old`] is `StatusGumpOld`, the 282×151 plate with five
//! labelled rows a side; [`Form::Modern`] is `StatusGumpModern`, the 560×196
//! AoS frame whose six columns of icons stand in for the labels. Both are laid
//! out here rather than in a module apiece because they are the same eleven
//! facts placed twice — a second module would be a second answer to "what does
//! the status window say", free to drift from this one the day a number is
//! added.
//!
//! Every coordinate below is ClassicUO's own, transcribed from
//! `Game/UI/Gumps/StatusGump.cs`. Where that file branches on `UseUOPGumps`,
//! this module takes the UOP arm: the art in a modern install *is* the UOP art,
//! and the two arms differ by up to twenty pixels a field.

use std::collections::BTreeMap;

use openshard_client_model::Status;
use openshard_protocol::mobile::{
    Stat,
    StatLockBits,
    Vitals,
};
use openshard_protocol::skill::SkillLock;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};

use crate::geometry::Rect;
use crate::gump::{
    GumpArt,
    GumpPixel,
    Picture,
    PictureIndex,
};
use crate::sprite::SpriteQuad;
use crate::text::GumpLabel;

/// Which of the reference client's two status windows to lay out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Form {
    /// `StatusGumpOld`: gump `0x0802`, five rows a side, the labels painted
    /// into the art.
    Old,
    /// `StatusGumpModern`: gump `0x2A6C`, six columns of icons and no words.
    Modern,
}

/// One line the status window writes, already placed in gump pixels.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at:   GumpPixel,
    pub text: String,
}

impl Line {
    /// Draw the status line in the reference frame's face and hue.
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at:   self.at,
            text: &self.text,
            font: FONT,
            hue:  HUE,
            clip: None,
        }
    }
}

/// One of the hairlines the modern frame rules between a current and its
/// maximum — ClassicUO's `Line` control, `0xFF383838`, one pixel high.
///
/// A picture would be the other way to draw it, and there is no art for it:
/// the reference paints a rectangle, and [`crate::gump::plate`] is this
/// renderer's word for the same primitive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rule {
    /// Its left end, in the window's own gump pixels.
    pub at:    GumpPixel,
    /// How far it runs. One pixel high, always.
    pub width: i32,
}

/// A status window laid out for one frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The background art, the stat arrows, and anything else with pixels.
    pub pictures: Vec<Picture>,
    /// The values written over that art.
    pub lines:    Vec<Line>,
    /// The hairlines the modern frame rules under a current value. Empty for
    /// [`Form::Old`], which has none.
    pub rules:    Vec<Rule>,
    /// Which picture is which stat's arrow, for the press that cycles it.
    ///
    /// A [`PictureIndex`] and not a rectangle, for
    /// [`crate::paperdoll::Doll::hits`]' reason: the pick is
    /// [`crate::gump::pick`]'s, over these very pictures, so a click lands on
    /// the arrow's *opaque* pixels rather than on the box around them.
    pub locks:    BTreeMap<PictureIndex, Stat>,
}

/// Everything the frame writes, gathered from the places that each own one of
/// these facts.
///
/// Hits and mana are not read out of [`Status`]: each has a packet of its own
/// (`0xA1`, `0xA2`) that restates it between `0x11` replies, so the view keeps
/// one home for each and hands both here. The arrows are the same shape again —
/// `0xBF 0x19` states them, and `None` means the shard has not, which is why
/// this is an [`Option`] and not three arrows pointing up.
#[derive(Clone, Copy, Debug)]
pub struct Numbers<'a> {
    pub status: &'a Status,
    pub hits:   Vitals,
    pub mana:   Vitals,
    pub locks:  Option<StatLockBits>,
}

/// The old status frame, 282×151 in the post-ML client files this app reads.
const OLD_FRAME: Graphic = Graphic(0x0802);
/// The AoS frame, 560×196 in the same files.
const MODERN_FRAME: Graphic = Graphic(0x2A6C);
const FONT: Font = Font(1);
const HUE: Hue = Hue(0x0386);

/// The grey ClassicUO rules its hairlines in: `0xFF383838`.
const RULE_SHADE: f32 = 0x38 as f32 / 255.0;

impl Window {
    /// This window's hairlines as the quads the gump pass draws.
    ///
    /// Here rather than at each of the two places that composite a window — the
    /// client's own pass and the layout screenshots — because a rule turned
    /// into a plate twice is a rule that can be one pixel high in one picture
    /// and two in the other. They are painted after the pictures wherever they
    /// are drawn: painter's order, so a rule lies over the frame it divides.
    #[must_use]
    pub fn rule_quads(&self) -> Vec<SpriteQuad> {
        self.rules
            .iter()
            .map(|rule| {
                crate::gump::plate(
                    Rect {
                        x:      rule.at.x as f32,
                        y:      rule.at.y as f32,
                        width:  rule.width as f32,
                        height: 1.0,
                    },
                    Hue::NONE,
                    crate::gump::Shade::new(RULE_SHADE),
                )
            })
            .collect()
    }
}

/// Lay out the status window at `at`.
///
/// `width_of` measures a string in a face — [`crate::text::gump_width`] with
/// the caller's font atlas already in hand, exactly as
/// [`crate::skills::window`] takes it. The modern frame centres six of its
/// values in a 40-pixel cell, and a centred value cannot be placed without
/// asking how wide it came out.
pub fn window<F>(form: Form, numbers: Numbers<'_>, width_of: F, at: GumpPixel) -> Window
where
    F: Fn(&str, Font) -> i32,
{
    match form {
        Form::Old => old(numbers, at),
        Form::Modern => modern(numbers, width_of, at),
    }
}

/// `StatusGumpOld`: the name at `(86, 42)`, the five left-column values at
/// `y = 62 + 12n`, and the five right-column values at the same rows. The art
/// itself carries the labels and no caller supplies a second localized copy of
/// them.
fn old(numbers: Numbers<'_>, at: GumpPixel) -> Window {
    let status = numbers.status;
    let mut window = Window {
        pictures: vec![Picture::plain(GumpArt::Gump(OLD_FRAME), at)],
        lines:    Vec::new(),
        rules:    Vec::new(),
        locks:    BTreeMap::new(),
    };
    // The arrows first, so the frame is behind them — painter's order. They are
    // 12 pixels apart down the left edge, exactly as the rows beside them are.
    arrows_at(
        &mut window,
        numbers.locks,
        [
            at.offset(GumpPixel::new(28, 62)),
            at.offset(GumpPixel::new(28, 74)),
            at.offset(GumpPixel::new(28, 86)),
        ],
    );

    let left = 86;
    let right = 171;
    let mut row = |x, y, text: String| {
        window.lines.push(Line {
            at: at.offset(GumpPixel::new(x, y)),
            text,
        });
    };
    row(left, 42, status.name.clone());
    row(left, 62, status.strength.to_string());
    row(left, 74, status.dexterity.to_string());
    row(left, 86, status.intelligence.to_string());
    row(left, 98, if status.female { "Female" } else { "Male" }.to_owned());
    row(left, 110, status.armor.to_string());
    row(right, 62, pair(numbers.hits));
    row(right, 74, pair(numbers.mana));
    row(right, 86, pair(status.stamina));
    row(right, 98, status.gold.to_string());
    row(right, 110, format!("{}/{}", status.weight, status.max_weight));
    window
}

/// `StatusGumpModern`, UOP arm. Six columns, and the three vitals written as a
/// current over a maximum with a hairline between them rather than as `a/b`.
fn modern<F>(numbers: Numbers<'_>, width_of: F, at: GumpPixel) -> Window
where
    F: Fn(&str, Font) -> i32,
{
    let status = numbers.status;
    let aos = status.aos;
    let mut window = Window {
        pictures: vec![Picture::plain(GumpArt::Gump(MODERN_FRAME), at)],
        lines:    Vec::new(),
        rules:    Vec::new(),
        locks:    BTreeMap::new(),
    };
    // 26, 30 and 30 apart rather than the old frame's even 12: the modern art's
    // three stat rows are not evenly spaced either.
    arrows_at(
        &mut window,
        numbers.locks,
        [
            at.offset(GumpPixel::new(28, 76)),
            at.offset(GumpPixel::new(28, 102)),
            at.offset(GumpPixel::new(28, 132)),
        ],
    );

    let mut left = |x, y, text: String| {
        window.lines.push(Line {
            at: at.offset(GumpPixel::new(x, y)),
            text,
        });
    };
    // The name is centred across the frame's title strip, x = 90 .. 410.
    let name_width = width_of(&status.name, FONT);
    left(90 + (320 - name_width) / 2, 50, status.name.clone());

    // Column 1: the three stats, hit chance increase under them.
    left(80, 77, status.strength.to_string());
    left(80, 105, status.dexterity.to_string());
    left(80, 133, status.intelligence.to_string());
    left(80, 161, aos.hit_chance.to_string());

    // Column 3: stat cap and luck; weight is centred, below.
    left(240, 77, status.stat_cap.to_string());
    left(240, 105, status.luck.to_string());
    left(240, 162, aos.lower_mana_cost.to_string());

    // Column 4: the weapon's damage, its increase, the followers, swing speed.
    left(320, 77, format!("{}-{}", status.damage.min, status.damage.max));
    left(320, 105, aos.damage_increase.to_string());
    left(320, 133, format!("{}-{}", status.followers, status.followers_max));
    left(320, 161, aos.swing_speed.to_string());

    // Column 5: the caster's four, and gold under them.
    left(400, 77, aos.lower_reagent_cost.to_string());
    left(400, 105, aos.spell_damage.to_string());
    left(400, 133, aos.faster_casting.to_string());
    left(400, 161, aos.faster_cast_recovery.to_string());
    left(480, 161, status.gold.to_string());

    // Column 6: the five resistances, each against the cap it may be raised to.
    // The physical one is `armor` — the field the packet carries it in.
    let resistances = status.resistances;
    left(475, 74, format!("{}/{}", status.armor, aos.max_physical));
    left(475, 92, format!("{}/{}", resistances.fire, aos.max_fire));
    left(475, 106, format!("{}/{}", resistances.cold, aos.max_cold));
    left(475, 120, format!("{}/{}", resistances.poison, aos.max_poison));
    left(475, 134, format!("{}/{}", resistances.energy, aos.max_energy));
    left(
        150,
        161,
        format!("{}/{}", aos.defense_chance, aos.max_defense_chance),
    );

    // Column 2, and the weight pair in column 3: a current over its maximum,
    // each centred in a 40-pixel cell with a hairline ruled between them.
    let mut stacked = |x: i32, y: i32, top: String, bottom: String, rule_x: i32, rule_width: i32| {
        for (offset, text) in [(0, top), (13, bottom)] {
            let centred = x + (CELL_WIDTH - width_of(&text, FONT)) / 2;
            window.lines.push(Line {
                at: at.offset(GumpPixel::new(centred, y + offset)),
                text,
            });
        }
        window.rules.push(Rule {
            at:    at.offset(GumpPixel::new(rule_x, y + 12)),
            width: rule_width,
        });
    };
    stacked(
        145,
        70,
        numbers.hits.current.to_string(),
        numbers.hits.max.to_string(),
        150,
        35,
    );
    stacked(
        145,
        98,
        status.stamina.current.to_string(),
        status.stamina.max.to_string(),
        150,
        35,
    );
    stacked(
        145,
        126,
        numbers.mana.current.to_string(),
        numbers.mana.max.to_string(),
        150,
        35,
    );
    stacked(
        230,
        126,
        status.weight.to_string(),
        status.max_weight.to_string(),
        236,
        34,
    );
    window
}

/// The width of the modern frame's centred value cells.
const CELL_WIDTH: i32 = 40;

/// The three arrows at the three places a frame puts them.
///
/// Nothing is drawn while `locks` is `None`: the shard has not said which way
/// the stats train, and three raise-arrows would be this window inventing an
/// answer the player never set — the same reason the frame draws nothing at all
/// before its first `0x11`.
fn arrows_at(window: &mut Window, locks: Option<StatLockBits>, places: [GumpPixel; 3]) {
    let Some(locks) = locks else {
        return;
    };
    for (place, (stat, lock)) in places.into_iter().zip([
        (Stat::Strength, locks.strength),
        (Stat::Dexterity, locks.dexterity),
        (Stat::Intelligence, locks.intelligence),
    ]) {
        let index = PictureIndex::new(window.pictures.len());
        window
            .pictures
            .push(Picture::plain(GumpArt::Gump(crate::lock::art(lock)), place));
        window.locks.insert(index, stat);
    }
}

/// The arrow a press moves this one to: up, down, held, and round again — the
/// reference client's `(lock + 1) % 3`.
#[must_use]
pub const fn next(lock: SkillLock) -> SkillLock {
    match lock {
        SkillLock::Up => SkillLock::Down,
        SkillLock::Down => SkillLock::Locked,
        SkillLock::Locked => SkillLock::Up,
    }
}

/// One stat's arrow out of the three.
#[must_use]
pub const fn arrow_of(locks: StatLockBits, stat: Stat) -> SkillLock {
    match stat {
        Stat::Strength => locks.strength,
        Stat::Dexterity => locks.dexterity,
        Stat::Intelligence => locks.intelligence,
    }
}

fn pair(vitals: Vitals) -> String {
    format!("{}/{}", vitals.current, vitals.max)
}

#[cfg(test)]
mod tests {
    use openshard_protocol::mobile::{
        AosStatus,
        DamageRange,
        Resistances,
    };

    use super::*;

    fn status() -> Status {
        Status {
            name:          "Lord British".to_owned(),
            female:        false,
            strength:      100,
            dexterity:     50,
            intelligence:  75,
            stamina:       Vitals {
                current: 49,
                max:     50,
            },
            gold:          1_234,
            armor:         42,
            weight:        12,
            max_weight:    450,
            stat_cap:      225,
            followers:     0,
            followers_max: 5,
            resistances:   Resistances {
                fire:   12,
                cold:   8,
                poison: 3,
                energy: 5,
            },
            luck:          140,
            damage:        DamageRange { min: 5, max: 11 },
            tithing:       40,
            aos:           AosStatus {
                max_physical:         70,
                max_fire:             70,
                max_cold:             70,
                max_poison:           70,
                max_energy:           70,
                defense_chance:       15,
                max_defense_chance:   45,
                hit_chance:           20,
                swing_speed:          25,
                damage_increase:      30,
                lower_reagent_cost:   35,
                spell_damage:         40,
                faster_cast_recovery: 4,
                faster_casting:       2,
                lower_mana_cost:      8,
            },
        }
    }

    fn numbers<'a>(status: &'a Status, locks: Option<StatLockBits>) -> Numbers<'a> {
        Numbers {
            status,
            hits: Vitals {
                current: 98,
                max:     100,
            },
            mana: Vitals {
                current: 72,
                max:     75,
            },
            locks,
        }
    }

    /// Eight pixels a character, which is what the reference's font 1 measures
    /// for a digit — enough for the centring arithmetic to be checkable without
    /// a client installation behind the test.
    fn measure(text: &str, _font: Font) -> i32 {
        text.chars().count() as i32 * 8
    }

    #[test]
    fn the_old_frame_carries_its_own_numbers_at_the_reference_positions() {
        let status = status();
        let window = window(
            Form::Old,
            numbers(&status, None),
            measure,
            GumpPixel::new(300, 200),
        );
        assert_eq!(
            window.pictures,
            vec![Picture::plain(GumpArt::Gump(OLD_FRAME), GumpPixel::new(300, 200))]
        );
        assert!(window.rules.is_empty(), "the old frame rules no hairlines");
        assert_eq!(
            window
                .lines
                .iter()
                .map(|line| (line.at, line.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (GumpPixel::new(386, 242), "Lord British"),
                (GumpPixel::new(386, 262), "100"),
                (GumpPixel::new(386, 274), "50"),
                (GumpPixel::new(386, 286), "75"),
                (GumpPixel::new(386, 298), "Male"),
                (GumpPixel::new(386, 310), "42"),
                (GumpPixel::new(471, 262), "98/100"),
                (GumpPixel::new(471, 274), "72/75"),
                (GumpPixel::new(471, 286), "49/50"),
                (GumpPixel::new(471, 298), "1234"),
                (GumpPixel::new(471, 310), "12/450"),
            ]
        );
    }

    /// The arrows are the shard's word, and until it has given one the frame
    /// draws none: three raise-arrows would be a window claiming the player
    /// left every stat training up.
    #[test]
    fn arrows_appear_only_once_the_shard_has_stated_them() {
        let status = status();
        let bare = window(Form::Old, numbers(&status, None), measure, GumpPixel::new(0, 0));
        assert_eq!(bare.pictures.len(), 1, "the frame and nothing else");
        assert!(bare.locks.is_empty(), "and nothing to press");

        let locks = StatLockBits {
            strength:     SkillLock::Locked,
            dexterity:    SkillLock::Down,
            intelligence: SkillLock::Up,
        };
        let armed = window(
            Form::Old,
            numbers(&status, Some(locks)),
            measure,
            GumpPixel::new(0, 0),
        );
        let arrows: Vec<_> = armed
            .pictures
            .iter()
            .skip(1)
            .map(|picture| (picture.graphic, picture.at))
            .collect();
        assert_eq!(
            arrows,
            vec![
                (GumpArt::Gump(crate::lock::HELD), GumpPixel::new(28, 62)),
                (GumpArt::Gump(crate::lock::DOWN), GumpPixel::new(28, 74)),
                (GumpArt::Gump(crate::lock::UP), GumpPixel::new(28, 86)),
            ],
            "each arrow wears the face of its own stat's lock"
        );
        assert_eq!(
            armed.locks.values().copied().collect::<Vec<_>>(),
            vec![Stat::Strength, Stat::Dexterity, Stat::Intelligence]
        );
    }

    /// The modern frame's six stacked values are centred in a 40-pixel cell and
    /// ruled apart. Left-aligning them instead is the mistake that looks almost
    /// right on a two-digit number and visibly wrong on a three-digit one, so
    /// the test states both.
    #[test]
    fn the_modern_frame_centres_a_current_over_its_maximum() {
        let status = status();
        let window = window(
            Form::Modern,
            numbers(&status, None),
            measure,
            GumpPixel::new(0, 0),
        );
        // By row and not by text alone: `100` is also strength's value, and a
        // search that found that one would pass while the centring was wrong.
        let line = |text: &str, y: i32| {
            window
                .lines
                .iter()
                .find(|line| line.text == text && line.at.y == y)
                .unwrap_or_else(|| panic!("the frame writes {text} on row {y}"))
                .at
        };
        // "98" is 16 wide in `measure`, so it sits 12 pixels into the cell at
        // x = 145; "100" is 24 wide and sits 8 pixels in.
        assert_eq!(line("98", 70), GumpPixel::new(145 + 12, 70));
        assert_eq!(line("100", 83), GumpPixel::new(145 + 8, 83));
        assert_eq!(line("450", 139), GumpPixel::new(230 + 8, 139));
        assert_eq!(
            window.rules,
            vec![
                Rule {
                    at:    GumpPixel::new(150, 82),
                    width: 35,
                },
                Rule {
                    at:    GumpPixel::new(150, 110),
                    width: 35,
                },
                Rule {
                    at:    GumpPixel::new(150, 138),
                    width: 35,
                },
                Rule {
                    at:    GumpPixel::new(236, 138),
                    width: 34,
                },
            ]
        );
    }

    /// Every AoS number the modern frame has a field for is written, and the
    /// physical resistance comes out of `armor` — the field the packet carries
    /// it in. A frame that quietly dropped one of these would look complete.
    #[test]
    fn the_modern_frame_writes_the_whole_aos_block() {
        let status = status();
        let window = window(
            Form::Modern,
            numbers(&status, None),
            measure,
            GumpPixel::new(0, 0),
        );
        let written: Vec<&str> = window.lines.iter().map(|line| line.text.as_str()).collect();
        for expected in [
            "42/70", // physical over its cap
            "12/70", "8/70", "3/70", "5/70",  // fire, cold, poison, energy
            "15/45", // defence chance over its cap
            "225", "140", // stat cap, luck
            "5-11", "0-5", // weapon damage, followers
            "20", "25", "30", "35", "40", "4", "2", "8", // the eight bonuses
            "1234",
        ] {
            assert!(
                written.contains(&expected),
                "the modern frame never wrote {expected}: {written:?}"
            );
        }
    }

    #[test]
    fn a_pressed_arrow_cycles_up_down_held_and_round() {
        assert_eq!(next(SkillLock::Up), SkillLock::Down);
        assert_eq!(next(SkillLock::Down), SkillLock::Locked);
        assert_eq!(next(SkillLock::Locked), SkillLock::Up);
    }
}
