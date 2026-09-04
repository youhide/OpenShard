//! `mobtypes.txt`: which animation family each body id belongs to.
//!
//! A body id says nothing about itself. Whether `63` names its actions in the
//! monster numbering or the animal one, and whether its frames sit in a
//! 22-group block or a 13-group one, is a fact the client reads out of this
//! file — one line per body, `id`, a type name, and a hexadecimal flag word.
//!
//! # Why a table and not a range
//!
//! [`BodyKind::of`] and [`IndexLayout::of`] answer the same two questions from
//! the id alone: below 200 a monster, below 400 an animal, above a human. That
//! is the reference client's *fallback* — `AnimationsLoader.CalculateTypeByGraphic`,
//! reached only when the install ships no `mobtypes.txt` — and the shipped file
//! disagrees with it for 322 of its 1313 bodies. Among them every wolf, bear,
//! cougar and panther: they are numbered below 200, so the range rule calls
//! them monsters, and the file calls them `ANIMAL`.
//!
//! Getting that wrong is silent. Nothing fails to load: the shard picks group
//! 4 for an attack because that is `HighAnimationGroup.Attack1`, and group 4 in
//! the animal numbering is a group the cougar does not have — so an attacking
//! cougar has no frame to draw and vanishes for the length of the swing, while
//! `LowAnimationGroup.Attack1`, five frames in every direction, sits unread at
//! group 5. ServUO answers the same question from its own `Data/bodyTable.cfg`,
//! and both ends of the wire have to agree about it.
//!
//! # Two answers, not one
//!
//! The type decides the *numbering* — which action each group number means —
//! and the type **together with the flags** decides the *layout*, which block
//! of `anim.idx` the frames are read from. They are not the same answer:
//! `AnimationFlags.CalculateOffsetLowGroupExtended` means "animal numbering in
//! a monster-shaped block", which is what most animals below body 200 are, and
//! a sea monster is animal-numbered in a monster block too. So this file
//! returns [`BodyKind`] and [`IndexLayout`] separately.

use std::collections::BTreeMap;
use std::path::Path;

use openshard_protocol::wire::Graphic;

use crate::anim::{
    AnimError,
    BodyKind,
    IndexLayout,
};

/// `AnimationFlags.CalculateOffsetLowGroupExtended`: an animal whose frames are
/// stored in a monster-shaped block rather than in the animal region.
const LOW_GROUP_EXTENDED: u32 = 0x0000_0020;
/// `AnimationFlags.CalculateOffsetByLowGroup`.
const BY_LOW_GROUP: u32 = 0x0000_0040;
/// `AnimationFlags.CalculateOffsetByPeopleGroup`.
const BY_PEOPLE_GROUP: u32 = 0x0000_0400;

/// One body's entry: how its actions are numbered, and where its frames live.
///
/// The flag word is resolved here rather than kept, because every other bit in
/// it answers a question this crate does not ask — `IdleAt8Frame`, `CanFlying`
/// and `UseUopAnimation` belong to a client that plays idles, flies things and
/// reads the UOP containers, and none of the three is a fact about the index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobType {
    /// Which of the three group numberings names this body's actions.
    pub kind:   BodyKind,
    /// Which shape of `anim.idx` block holds its frames.
    pub layout: IndexLayout,
}

/// The `mobtypes.txt` beside a client install.
///
/// An install that ships none is an empty table, which is not a stand-in for
/// the file: it is the file saying nothing, and every lookup then falls through
/// to the range rule exactly as the reference client's does.
#[derive(Clone, Debug)]
pub struct MobTypes {
    entries: BTreeMap<u16, MobType>,
}

impl MobTypes {
    /// The table an install with no `mobtypes.txt` has. Every lookup answers
    /// from the range rule.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Read `mobtypes.txt` when the install ships one.
    ///
    /// Pre-5.0.0a clients legitimately have no such file — the reference client
    /// does not even look for one below that version — and that is the same as
    /// an empty table rather than an error.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, AnimError> {
        let path = client_dir.as_ref().join("mobtypes.txt");
        let source = match std::fs::read(&path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(source) => return Err(AnimError::Read { path, source }),
        };
        // Read as bytes and converted lossily: the shipped file is mostly ASCII
        // but its comments are not — three `EQUIPMENT` lines carry a stray
        // non-breaking space — and a body-to-family table must not be lost
        // wholesale because a comment is not UTF-8.
        Ok(Self::from_text(&String::from_utf8_lossy(&source)))
    }

    /// Parse the file's text.
    ///
    /// Public so a test can state the two or three rows it is about — the
    /// shapes that matter are a comment, a blank line and a flag word with
    /// rubbish welded to it — rather than needing an install to say "the cougar
    /// is an animal".
    #[must_use]
    pub fn from_text(source: &str) -> Self {
        let mut entries = BTreeMap::new();
        for line in source.lines() {
            // Everything from `#` is a comment, and the file uses it both for
            // whole-line headers and for a trailing note naming the creature.
            let line = line.split('#').next().unwrap_or_default();
            let mut fields = line.split_whitespace();
            let (Some(id), Some(name), Some(flags)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            let (Ok(id), Ok(flags)) = (id.parse::<u16>(), u32::from_str_radix(flags, 16)) else {
                // A line whose id or flags do not parse is dropped rather than
                // defaulted: the shipped file has three of them, where a
                // non-breaking space is welded to the flag word, and guessing
                // `0` for those would quietly claim they carry no flags at all.
                continue;
            };
            let Some(entry) = resolve(name, flags) else {
                continue;
            };
            entries.insert(id, entry);
        }
        Self { entries }
    }

    /// What the file says about a body, or `None` where it says nothing.
    ///
    /// The absent answer is deliberately visible: a caller that wants the range
    /// rule when the table is silent asks [`kind_of`](Self::kind_of) or
    /// [`layout_of`](Self::layout_of), and the two together are what makes
    /// "the file has no line for this body" different from "the file says
    /// monster".
    #[must_use]
    pub fn get(&self, body: Graphic) -> Option<MobType> {
        self.entries.get(&body.0).copied()
    }

    /// Which numbering names this body's actions, falling back to the range
    /// rule where the table is silent.
    #[must_use]
    pub fn kind_of(&self, body: Graphic) -> BodyKind {
        self.get(body)
            .map_or_else(|| BodyKind::of(body), |entry| entry.kind)
    }

    /// Which shape of index block holds this body's frames, falling back to the
    /// range rule where the table is silent.
    #[must_use]
    pub fn layout_of(&self, body: Graphic) -> IndexLayout {
        self.get(body)
            .map_or_else(|| IndexLayout::of(body), |entry| entry.layout)
    }

    /// How many bodies the table names. Zero is an install with no file.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table names no bodies at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One line's type name and flag word, as the two answers the index needs.
///
/// `AnimationsLoader.CalculateOffset`, whose five type names are matched
/// case-insensitively exactly as the reference does. An unknown name is `None`
/// — the file is the client's, and inventing a family for a word it does not
/// use would put a body in a block nobody chose.
fn resolve(name: &str, flags: u32) -> Option<MobType> {
    // The choice the reference makes for a monster, and again for an animal
    // that carries `CalculateOffsetLowGroupExtended`: the flags name the block,
    // and `High` is what a body with neither flag gets.
    let by_flags = if flags & BY_PEOPLE_GROUP != 0 {
        IndexLayout::People
    } else if flags & BY_LOW_GROUP != 0 {
        IndexLayout::Low
    } else {
        IndexLayout::High
    };

    let (kind, layout) = match name.to_ascii_uppercase().as_str() {
        "MONSTER" => (BodyKind::Monster, by_flags),
        // Animal numbering in a monster-shaped block. The reference reads only
        // the 13 low groups out of the 22-group block; we allow all 22, which
        // differs only in that a group past the numbering's own end reads this
        // body's own unused slots instead of nothing.
        "SEA_MONSTER" => (BodyKind::Animal, IndexLayout::High),
        "ANIMAL" => {
            match flags & LOW_GROUP_EXTENDED != 0 {
                true => (BodyKind::Animal, by_flags),
                false => (BodyKind::Animal, IndexLayout::Low),
            }
        }
        // A worn item's animation is drawn from the human block: it is a
        // garment on a person-shaped body, and the file's `EQUIPMENT` rows are
        // 810 of its 1313 lines.
        "HUMAN" | "EQUIPMENT" => (BodyKind::Human, IndexLayout::People),
        _ => return None,
    };
    Some(MobType { kind, layout })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_an_id_a_family_and_a_hexadecimal_flag_word() {
        let table = MobTypes::from_text("63\tANIMAL\t20\n400\tHUMAN\t0\n");
        assert_eq!(
            table.get(Graphic(63)),
            Some(MobType {
                kind:   BodyKind::Animal,
                // 0x20 is `CalculateOffsetLowGroupExtended` with neither of the
                // two group flags beside it, which is the monster-shaped block.
                layout: IndexLayout::High,
            }),
            "the cougar is an animal stored in a monster-shaped block"
        );
        assert_eq!(
            table.get(Graphic(400)),
            Some(MobType {
                kind:   BodyKind::Human,
                layout: IndexLayout::People,
            })
        );
    }

    #[test]
    fn the_flags_are_hexadecimal_and_not_decimal() {
        // 40 hex is `CalculateOffsetByLowGroup`; 40 decimal is 0x28, which is
        // `LowGroupExtended | CanFlying` and names a different block entirely.
        let table = MobTypes::from_text("1\tMONSTER\t40\n");
        assert_eq!(table.layout_of(Graphic(1)), IndexLayout::Low);
    }

    #[test]
    fn an_animal_without_the_extended_flag_lives_in_the_animal_region() {
        let table = MobTypes::from_text("214\tANIMAL\t0\n");
        assert_eq!(
            table.get(Graphic(214)),
            Some(MobType {
                kind:   BodyKind::Animal,
                layout: IndexLayout::Low,
            })
        );
    }

    #[test]
    fn comments_headers_and_blank_lines_carry_no_bodies() {
        let table = MobTypes::from_text(
            "# Animation types for animation lookups\n# ID\tTYPE\tFLAGS\n\n   \n63\tANIMAL\t20\n",
        );
        assert_eq!(table.len(), 1, "only the one real row");
    }

    #[test]
    fn a_trailing_comment_does_not_reach_the_flag_word() {
        let table = MobTypes::from_text("34\tANIMAL\t20\t# Wolf\n");
        assert_eq!(table.layout_of(Graphic(34)), IndexLayout::High);
    }

    #[test]
    fn a_flag_word_with_rubbish_welded_to_it_drops_its_line() {
        // Reading `0` for such a line would claim it carries no flags, which is
        // a different statement from "the file is unreadable here" — and 0 is
        // the answer that puts an animal in the wrong block.
        let table = MobTypes::from_text("1698\tEQUIPMENT\t10000Z\n1699\tEQUIPMENT\t10000\n");
        assert_eq!(table.get(Graphic(1698)), None);
        assert!(table.get(Graphic(1699)).is_some());
    }

    /// The three `EQUIPMENT` rows at the end of the shipped file carry a
    /// non-breaking space between the flag word and the comment. The reference
    /// splits on tab and space only and throws on them; splitting on Unicode
    /// whitespace reads them, which is the whole reason the file is decoded
    /// lossily rather than parsed as ASCII.
    #[test]
    fn a_non_breaking_space_after_the_flag_word_is_still_a_separator() {
        let raw = b"1698\tEQUIPMENT\t10000\xc2\xa0\xc2 # Tanto_G\n";
        let table = MobTypes::from_text(&String::from_utf8_lossy(raw));
        assert_eq!(
            table.get(Graphic(1698)),
            Some(MobType {
                kind:   BodyKind::Human,
                layout: IndexLayout::People,
            })
        );
    }

    #[test]
    fn an_unknown_family_name_is_not_a_body() {
        let table = MobTypes::from_text("7\tDRAGONKIN\t0\n");
        assert_eq!(table.get(Graphic(7)), None);
    }

    #[test]
    fn an_empty_table_answers_from_the_range_rule() {
        let table = MobTypes::empty();
        assert!(table.is_empty());
        assert_eq!(table.kind_of(Graphic(63)), BodyKind::Monster);
        assert_eq!(table.layout_of(Graphic(63)), IndexLayout::High);
        assert_eq!(table.kind_of(Graphic(400)), BodyKind::Human);
    }

    #[test]
    fn a_body_the_table_does_not_name_falls_through_to_the_range_rule() {
        let table = MobTypes::from_text("63\tANIMAL\t20\n");
        assert_eq!(table.kind_of(Graphic(63)), BodyKind::Animal, "the row it has");
        assert_eq!(
            table.kind_of(Graphic(214)),
            BodyKind::Animal,
            "and the range rule for the row it has not"
        );
        assert_eq!(table.kind_of(Graphic(1)), BodyKind::Monster);
    }
}
