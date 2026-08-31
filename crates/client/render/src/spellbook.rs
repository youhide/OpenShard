//! The local Magery spellbook window.
//!
//! The shard sends only a book serial and a 64-bit membership mask.  This
//! module supplies the familiar scroll, spell names, and the small buttons a
//! player uses to choose one; casting itself remains a client/app effect.

use std::collections::BTreeMap;

use openshard_protocol::casting::SpellId;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};

use crate::gump::{
    self,
    GumpArt,
    GumpPixel,
    Picture,
    PictureIndex,
    Scissor,
};
use crate::text::GumpLabel;

/// The window's fixed dimensions, shared by layout and its owner.
pub const EXTENT: (i32, i32) = (345, 320);
const TOP_HEIGHT: i32 = 37;
const BOTTOM_HEIGHT: i32 = 34;
const BODY_WIDTH: i32 = 302;
const BOTTOM_WIDTH: i32 = 314;
const VIEWPORT_AT: GumpPixel = GumpPixel::new(42, 56);
const VIEWPORT_WIDTH: i32 = 246;
const VIEWPORT_HEIGHT: i32 = 220;
/// One mouse wheel step is one visible row.
pub const ROW_HEIGHT: i32 = 17;

const SCROLL_TOP: Graphic = Graphic(0x1F40);
const SCROLL_EDGE: Graphic = Graphic(0x1F41);
const SCROLL_BODY: Graphic = Graphic(0x1F42);
const SCROLL_BOTTOM: Graphic = Graphic(0x1F43);
const TITLE_PLATE: Graphic = Graphic(0x0834);
const RULE: Graphic = Graphic(0x082B);
const CAST_BUTTON: Graphic = Graphic(0x0837);
const TITLE_FONT: Font = Font(6);
const ROW_FONT: Font = Font(9);
const TITLE_HUE: Hue = Hue(0x0386);
const ROW_HUE: Hue = Hue(0x0288);

/// A spell the book says may be cast.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    pub spell: SpellId,
    pub name:  &'static str,
}

/// What an opaque picture in the window means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Cast(SpellId),
}

/// One text line that the application pass draws after the art.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    pub at:      GumpPixel,
    pub text:    String,
    pub font:    Font,
    pub hue:     Hue,
    pub scissor: Option<Scissor>,
}

impl Line {
    #[must_use]
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at:   self.at,
            text: &self.text,
            font: self.font,
            hue:  self.hue,
            clip: None,
        }
    }
}

/// The spellbook, already laid out for one frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    pub pictures: Vec<Picture>,
    pub hits:     BTreeMap<PictureIndex, Hit>,
    pub lines:    Vec<Line>,
    pub viewport: Scissor,
    entries:      usize,
}

impl Window {
    /// The clipping height used by the pane to clamp its local scroll.
    #[must_use]
    pub const fn viewport_height() -> i32 {
        VIEWPORT_HEIGHT
    }

    /// The semantic hit at this window-local coordinate.
    #[must_use]
    pub fn hit(&self, point: GumpPixel, atlas: &gump::GumpAtlas) -> Option<Hit> {
        gump::pick(&self.pictures, point, atlas).and_then(|index| self.hits.get(&index).copied())
    }

    /// The total height the current book needs before clipping.
    #[must_use]
    pub const fn content_height(&self) -> i32 {
        self.entries as i32 * ROW_HEIGHT
    }
}

/// The known Magery entry named by this zero-based spell id.
#[must_use]
pub const fn name_of(spell: SpellId) -> Option<&'static str> {
    match spell.0 {
        0 => Some("Clumsy"),
        1 => Some("Create Food"),
        2 => Some("Feeblemind"),
        3 => Some("Heal"),
        4 => Some("Magic Arrow"),
        5 => Some("Night Sight"),
        6 => Some("Reactive Armor"),
        7 => Some("Weaken"),
        8 => Some("Agility"),
        9 => Some("Cunning"),
        10 => Some("Cure"),
        11 => Some("Harm"),
        12 => Some("Magic Trap"),
        13 => Some("Magic Untrap"),
        14 => Some("Protection"),
        15 => Some("Strength"),
        16 => Some("Bless"),
        17 => Some("Fireball"),
        18 => Some("Magic Lock"),
        19 => Some("Poison"),
        20 => Some("Telekinesis"),
        21 => Some("Teleport"),
        22 => Some("Unlock"),
        23 => Some("Wall of Stone"),
        24 => Some("Arch Cure"),
        25 => Some("Arch Protection"),
        26 => Some("Curse"),
        27 => Some("Fire Field"),
        28 => Some("Greater Heal"),
        29 => Some("Lightning"),
        30 => Some("Mana Drain"),
        31 => Some("Recall"),
        32 => Some("Blade Spirits"),
        33 => Some("Dispel Field"),
        34 => Some("Incognito"),
        35 => Some("Magic Reflection"),
        36 => Some("Mind Blast"),
        37 => Some("Paralyze"),
        38 => Some("Poison Field"),
        39 => Some("Summon Creature"),
        40 => Some("Dispel"),
        41 => Some("Energy Bolt"),
        42 => Some("Explosion"),
        43 => Some("Invisibility"),
        44 => Some("Mark"),
        45 => Some("Mass Curse"),
        46 => Some("Paralyze Field"),
        47 => Some("Reveal"),
        48 => Some("Chain Lightning"),
        49 => Some("Energy Field"),
        50 => Some("Flamestrike"),
        51 => Some("Gate Travel"),
        52 => Some("Mana Vampire"),
        53 => Some("Mass Dispel"),
        54 => Some("Meteor Swarm"),
        55 => Some("Polymorph"),
        56 => Some("Earthquake"),
        57 => Some("Energy Vortex"),
        58 => Some("Resurrection"),
        59 => Some("Air Elemental"),
        60 => Some("Summon Daemon"),
        61 => Some("Earth Elemental"),
        62 => Some("Fire Elemental"),
        63 => Some("Water Elemental"),
        _ => None,
    }
}

/// The entries the membership mask makes available.  `offset` is one-based on
/// the wire, so `offset = 1` and bit zero name program spell zero.
#[must_use]
pub fn entries(offset: u16, content: u64) -> Vec<Entry> {
    (0..64)
        .filter_map(|bit| {
            (content & (1u64 << bit) != 0).then(|| {
                let one_based = offset.checked_add(bit)?;
                let spell = SpellId(one_based.checked_sub(1)?);
                Some(Entry {
                    spell,
                    name: name_of(spell).unwrap_or("Unknown spell"),
                })
            })?
        })
        .collect()
}

/// Lay out a spellbook at a window-local origin.
#[must_use]
pub fn window(offset: u16, content: u64, scroll: i32, at: GumpPixel) -> Window {
    let entries = entries(offset, content);
    let viewport = Scissor {
        at:     at.offset(VIEWPORT_AT),
        width:  VIEWPORT_WIDTH,
        height: VIEWPORT_HEIGHT,
    };
    let mut window = Window {
        pictures: Vec::new(),
        hits: BTreeMap::new(),
        lines: Vec::new(),
        viewport,
        entries: entries.len(),
    };
    frame(&mut window, at);
    window.lines.push(Line {
        at:      at.offset(GumpPixel::new(142, 17)),
        text:    "Spellbook".to_owned(),
        font:    TITLE_FONT,
        hue:     TITLE_HUE,
        scissor: None,
    });
    if entries.is_empty() {
        window.lines.push(Line {
            at:      at.offset(GumpPixel::new(VIEWPORT_AT.x + 16, VIEWPORT_AT.y + 8)),
            text:    "No spells in this book".to_owned(),
            font:    ROW_FONT,
            hue:     ROW_HUE,
            scissor: Some(viewport),
        });
        return window;
    }
    for (row, entry) in entries.iter().enumerate() {
        let y = VIEWPORT_AT.y + row as i32 * ROW_HEIGHT - scroll;
        window.pictures.push(
            Picture::plain(
                GumpArt::Gump(CAST_BUTTON),
                at.offset(GumpPixel::new(VIEWPORT_AT.x + 7, y + 2)),
            )
            .inside(viewport),
        );
        window.hits.insert(
            PictureIndex::new(window.pictures.len() - 1),
            Hit::Cast(entry.spell),
        );
        window.lines.push(Line {
            at:      at.offset(GumpPixel::new(VIEWPORT_AT.x + 28, y)),
            text:    format!("{}. {}", entry.spell.0 + 1, entry.name),
            font:    ROW_FONT,
            hue:     ROW_HUE,
            scissor: Some(viewport),
        });
    }
    window
}

fn frame(window: &mut Window, at: GumpPixel) {
    let body_height = EXTENT.1 - TOP_HEIGHT - BOTTOM_HEIGHT;
    let body_x = (EXTENT.0 - BODY_WIDTH) / 2;
    let bottom_off = EXTENT.0 - BOTTOM_WIDTH;
    window.pictures.extend([
        Picture::plain(
            GumpArt::Gump(SCROLL_EDGE),
            at.offset(GumpPixel::new(body_x, TOP_HEIGHT)),
        )
        .tiled(BODY_WIDTH, body_height),
        Picture::plain(
            GumpArt::Gump(SCROLL_BODY),
            at.offset(GumpPixel::new(body_x, TOP_HEIGHT)),
        )
        .tiled(BODY_WIDTH, body_height),
        Picture::plain(GumpArt::Gump(SCROLL_TOP), at),
        Picture::plain(
            GumpArt::Gump(SCROLL_BOTTOM),
            at.offset(GumpPixel::new(
                bottom_off / 2 + bottom_off / 4,
                EXTENT.1 - BOTTOM_HEIGHT,
            )),
        ),
        Picture::plain(GumpArt::Gump(TITLE_PLATE), at.offset(GumpPixel::new(140, 12))),
        Picture::plain(GumpArt::Gump(RULE), at.offset(GumpPixel::new(50, 42))),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magery_mask_names_only_the_spells_it_holds() {
        assert_eq!(
            entries(1, 1 | (1 << 17) | (1 << 63)),
            vec![
                Entry {
                    spell: SpellId(0),
                    name:  "Clumsy",
                },
                Entry {
                    spell: SpellId(17),
                    name:  "Fireball",
                },
                Entry {
                    spell: SpellId(63),
                    name:  "Water Elemental",
                },
            ]
        );
    }

    #[test]
    fn the_mask_offset_is_one_based() {
        assert_eq!(
            entries(18, 1),
            vec![Entry {
                spell: SpellId(17),
                name:  "Fireball",
            }]
        );
    }
}
