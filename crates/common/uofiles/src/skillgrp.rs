//! `skillgrp.mul`: which heading each skill is filed under — the tree a skill
//! window is drawn as.
//!
//! Six named groups and one that is not named: the file carries a count, then
//! `count - 1` fixed-width names, then one `int32` per skill saying which group
//! it belongs to. The numbering the trailing table speaks is one-based into the
//! names, and zero means the group the file never names — the leftovers, which
//! the reference tooling calls **Misc** and which this reader names the same, in
//! [`SkillGroups::MISC`]. Nothing about that zero is a sentinel: it is an
//! ordinary heading with ordinary skills under it (Item Identification, Arms
//! Lore, Begging), and the shipped English file files eight skills there.
//!
//! Ported from ServUO's `Ultima/SkillGroups.cs`, which is the UOFiddler reader
//! and the only one that reads this file at all — ClassicUO ignores it and keeps
//! a grouping of its own, in a file of its own, that a player can rearrange. We
//! read the client's, because it is the grouping the player's own install
//! already agrees with and because a table of our own would be a second opinion
//! about somebody else's data.
//!
//! What the file does **not** carry: which group is drawn first (it is the file's
//! own order, with [`SkillGroups::MISC`] ahead of it), and whether a group starts
//! open or shut. Both are the window's, in `openshard_client_render::skills`.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::skills::SkillId;

/// Which heading a skill is filed under.
///
/// The second of the two id spaces a skill window holds — see [`SkillId`], which
/// is the other and is *not* interchangeable with it: both are small integers,
/// both index a table, and a window that mixed them would draw the right names
/// under the wrong headings without ever being out of range.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct GroupId(pub u8);

/// The grouping a client install ships.
#[derive(Clone, Debug, Default)]
pub struct SkillGroups {
    /// Every heading, `MISC` first and then the file's own, in its order.
    names: Vec<String>,
    /// One group per skill, by [`SkillId`] — indexed, not keyed, because the
    /// file's trailing table is exactly as long as the skill list and in the
    /// same order.
    of_skill: Vec<GroupId>,
}

/// How wide one name is in the byte form of the file.
const NAME_BYTES: usize = 17;

/// Where the names start in that form: after the count.
const NAMES_AT: usize = 4;

/// The count a Unicode file writes before its real count.
const UNICODE_MARK: i32 = -1;

/// `skillgrp.mul` could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum SkillGroupsError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file is shorter than the header it declares — a count of *n* groups
    /// with fewer than *n* names behind it. Refused rather than read short: the
    /// names and the per-skill table are laid end to end, so a name that runs off
    /// the end means every group index after it is being read out of the wrong
    /// bytes.
    Truncated {
        /// Which file.
        path: PathBuf,
        /// How long it was.
        size: usize,
        /// How long its own header says it must be, at least.
        wanted: usize,
    },
}

impl fmt::Display for SkillGroupsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Truncated { path, size, wanted } => write!(
                f,
                "{} is {size} bytes, and its own header needs at least {wanted}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SkillGroupsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Truncated { .. } => None,
        }
    }
}

impl SkillGroups {
    /// The group the file does not name, and the one every unfiled skill lands
    /// in. Always present, even in a file with no names at all.
    pub const MISC: GroupId = GroupId(0);

    /// The name this reader gives [`MISC`](Self::MISC) — not a string from the
    /// file, which has none for it, but the one UOFiddler and ServUO's SDK use.
    const MISC_NAME: &'static str = "Misc";

    /// Open `skillgrp.mul` in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, SkillGroupsError> {
        Self::from_file(client_dir.as_ref().join("skillgrp.mul"))
    }

    /// Open a named file, for tests.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, SkillGroupsError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| SkillGroupsError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&bytes).ok_or_else(|| {
            // What the header asked for, so the message can say how far short the
            // file fell rather than only that it did.
            let wanted = Self::wanted(&bytes).unwrap_or(NAMES_AT);
            SkillGroupsError::Truncated {
                path: path.to_owned(),
                size: bytes.len(),
                wanted,
            }
        })
    }

    /// How many bytes the header says the names alone take up, header included.
    fn wanted(bytes: &[u8]) -> Option<usize> {
        let (count, unicode) = Self::header(bytes)?;
        let (start, width) = Self::layout(unicode);
        Some(start + count.saturating_sub(1) * width)
    }

    /// The declared group count and whether the names are Unicode.
    fn header(bytes: &[u8]) -> Option<(usize, bool)> {
        let word = |at: usize| -> Option<i32> {
            let four = bytes.get(at..at + 4)?;
            Some(i32::from_le_bytes([four[0], four[1], four[2], four[3]]))
        };
        match word(0)? {
            UNICODE_MARK => Some((usize::try_from(word(4)?).ok()?, true)),
            count => Some((usize::try_from(count).ok()?, false)),
        }
    }

    /// Where the names start and how wide one is — doubled for a Unicode file,
    /// which is the whole of the difference between the two forms.
    const fn layout(unicode: bool) -> (usize, usize) {
        match unicode {
            true => (NAMES_AT * 2, NAME_BYTES * 2),
            false => (NAMES_AT, NAME_BYTES),
        }
    }

    /// Read a file already in memory, or `None` if it is shorter than its own
    /// header declares.
    ///
    /// A group index past the names is filed under [`MISC`](Self::MISC), here at
    /// the door and nowhere else: every reader downstream wants a heading to draw
    /// a skill under, and "a group with no name" is exactly what `MISC` already
    /// is. The shipped file needs none of that — see
    /// `every_shipped_skill_is_filed_under_a_group_that_has_a_name`.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (count, unicode) = Self::header(bytes)?;
        let (start, width) = Self::layout(unicode);
        // The file names every group but the zeroth, which is why one name fewer
        // than the count is the right number and not an off-by-one.
        let named = count.saturating_sub(1);
        let table_at = start + named * width;
        if bytes.len() < table_at {
            return None;
        }
        let mut names = vec![Self::MISC_NAME.to_owned()];
        for nth in 0..named {
            let at = start + nth * width;
            let record = bytes.get(at..at + width)?;
            names.push(match unicode {
                true => record
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .take_while(|unit| *unit != 0)
                    .map(|unit| char::from_u32(u32::from(unit)).unwrap_or('?'))
                    .collect(),
                // Latin-1, the same single-byte reading `skills.mul`'s own names
                // get — see `crate::skills::Skill::name`.
                false => record
                    .iter()
                    .take_while(|byte| **byte != 0)
                    .map(|byte| char::from(*byte))
                    .collect(),
            });
        }
        let of_skill = bytes[table_at..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|four| i32::from_le_bytes([four[0], four[1], four[2], four[3]]))
            .map(|group| match usize::try_from(group) {
                Ok(group) if group < names.len() => GroupId(group as u8),
                _ => Self::MISC,
            })
            .collect();
        Some(Self { names, of_skill })
    }

    /// Every heading, in the order a window draws them: `Misc` first, then the
    /// file's own order.
    pub fn groups(&self) -> impl Iterator<Item = (GroupId, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(nth, name)| (GroupId(nth as u8), name.as_str()))
    }

    /// One heading's name. Every [`GroupId`] this reader hands out has one.
    pub fn name(&self, group: GroupId) -> Option<&str> {
        self.names.get(usize::from(group.0)).map(String::as_str)
    }

    /// Which heading a skill is filed under, or `None` for a skill this install's
    /// file says nothing about — an id past the table, which is what an older
    /// install looks like when a shard sends a newer client's skill list.
    pub fn group_of(&self, skill: SkillId) -> Option<GroupId> {
        self.of_skill.get(usize::from(skill.0)).copied()
    }

    /// How many skills the file files.
    pub fn filed(&self) -> usize {
        self.of_skill.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with two named groups and four skills across all three.
    fn fixture(groups: &[&str], of_skill: &[i32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((groups.len() + 1) as i32).to_le_bytes());
        for name in groups {
            let mut record = vec![0u8; NAME_BYTES];
            record[..name.len()].copy_from_slice(name.as_bytes());
            bytes.extend_from_slice(&record);
        }
        for group in of_skill {
            bytes.extend_from_slice(&group.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn the_count_is_one_more_than_the_names_and_the_extra_one_is_misc() {
        let bytes = fixture(&["Combat", "Magic"], &[0, 1, 2]);
        let groups = SkillGroups::parse(&bytes).expect("the fixture is whole");
        let headings: Vec<_> = groups.groups().map(|(_, name)| name.to_owned()).collect();
        assert_eq!(headings, ["Misc", "Combat", "Magic"]);
        assert_eq!(groups.group_of(SkillId(0)), Some(SkillGroups::MISC));
        assert_eq!(groups.group_of(SkillId(1)), Some(GroupId(1)));
        assert_eq!(groups.name(GroupId(2)), Some("Magic"));
    }

    /// The trailing table is one entry per skill and in the same order, so the
    /// id is its index and nothing else needs to be carried alongside.
    #[test]
    fn a_skill_past_the_table_is_filed_nowhere_rather_than_under_misc() {
        let bytes = fixture(&["Combat"], &[1, 0]);
        let groups = SkillGroups::parse(&bytes).expect("the fixture is whole");
        assert_eq!(groups.filed(), 2);
        assert_eq!(
            groups.group_of(SkillId(2)),
            None,
            "the file says nothing about it"
        );
    }

    /// A group index the file's own names do not reach is the unnamed group,
    /// which is what `Misc` is — normalized here so that every skill a window
    /// draws has a heading to draw it under.
    #[test]
    fn a_group_index_past_the_names_lands_in_misc() {
        let bytes = fixture(&["Combat"], &[9, -3]);
        let groups = SkillGroups::parse(&bytes).expect("the fixture is whole");
        assert_eq!(groups.group_of(SkillId(0)), Some(SkillGroups::MISC));
        assert_eq!(groups.group_of(SkillId(1)), Some(SkillGroups::MISC));
    }

    #[test]
    fn a_file_shorter_than_its_own_header_is_refused() {
        let mut bytes = fixture(&["Combat", "Magic"], &[]);
        bytes.truncate(NAMES_AT + NAME_BYTES); // one name short
        assert!(SkillGroups::parse(&bytes).is_none());
    }

    /// The Unicode form doubles both the header and every name, and is the whole
    /// of the difference between the two layouts.
    #[test]
    fn a_unicode_file_reads_its_names_two_bytes_at_a_time() {
        let mut bytes = (-1i32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&2i32.to_le_bytes());
        let mut record = vec![0u8; NAME_BYTES * 2];
        for (nth, unit) in "Magic".encode_utf16().enumerate() {
            record[nth * 2..nth * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&record);
        bytes.extend_from_slice(&1i32.to_le_bytes());
        let groups = SkillGroups::parse(&bytes).expect("the fixture is whole");
        assert_eq!(groups.name(GroupId(1)), Some("Magic"));
        assert_eq!(groups.group_of(SkillId(0)), Some(GroupId(1)));
    }
}
