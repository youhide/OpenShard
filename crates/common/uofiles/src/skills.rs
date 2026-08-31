//! `Skills.idx`/`skills.mul`: what the fifty-eight skills are *called*, and
//! which of them a window may be used from.
//!
//! The names a skill window writes down its left-hand column. They are not the
//! shard's: `openshard_state::skill` has a table of its own with the same rows,
//! but it lives in a server crate and says what a skill *does* — the scales, the
//! gain curve, the title a grandmaster earns. This file is the client's own copy
//! of one column of that table, and it is the one a client draws from, for the
//! same reason a client draws art out of `art.mul` rather than being sent it:
//! the strings are already on the player's disk, in the player's own install.
//!
//! Ported from ClassicUO's `SkillsLoader.Load`, a BSD-2 reference client kept
//! outside this repository. Two things in it are not guessable from the bytes:
//!
//! - the record's **first byte is not part of the name** — it is a flag, and
//!   the reference calls it `HasAction`: whether the client's own window offers
//!   a button that *uses* the skill. Twenty-odd rows have it, and it is the same
//!   fact `openshard_state::SkillInfo::usable` carries on the other side of the
//!   wire, arrived at independently from ServUO's table;
//! - an index entry with no data is **not a skill**, and does not take an id.
//!   The reference numbers the skills it found, in order, skipping the empty
//!   entries — so a hole in the middle of the index would shift every id after
//!   it. The shipped file has no hole (`the_shipped_names_are_dense_and_are_the
//!   _wires_own_numbering` in `tests/client_files.rs` pins that), and the
//!   numbering the shard's `0x3A` speaks is this one.
//!
//! The localized names — `SKILL30.ENU` and its kin — are a different file and
//! not read here. This is the English table the client ships in `skills.mul`.

use std::fmt;
use std::path::{Path, PathBuf};

/// Which skill, in the numbering the client's own files and the `0x3A` packets
/// share: Alchemy is 0, Mining is 45.
///
/// A newtype and not a `u8` because a skill window holds two id spaces at once —
/// this and [`crate::skillgrp::GroupId`], the group a skill is filed under — and
/// they are both small integers that index different tables. The wire's own
/// `openshard_protocol::wire::RawSkillId` is a third thing: an id a *client*
/// sent that nothing has checked. This one is a row that exists, and is only
/// made by [`Skills::id`] or by [`Skills`]' own iteration.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SkillId(pub u8);

/// One row of `skills.mul`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Skill {
    /// What it is called: "Alchemy", "Item Identification".
    ///
    /// Decoded a byte to a character, which is Latin-1 and not UTF-8: the file
    /// is a table of single-byte strings, and so is the `fonts.mul` face this
    /// is drawn in. Every name in the shipped English file is ASCII, where the
    /// two agree exactly.
    pub name: String,
    /// Whether the client's window offers a button that *uses* this skill —
    /// the record's first byte. Passive skills (Tactics), item-driven ones
    /// (Mining) and the ones another window opens (Magery) have it clear.
    pub has_action: bool,
}

/// The skill names a client install ships.
#[derive(Clone, Debug, Default)]
pub struct Skills {
    skills: Vec<Skill>,
}

/// Bytes per `Skills.idx` entry: offset, length, extra.
const INDEX_ENTRY: usize = 12;

/// The length an index entry has when it names no record. The file writes "no
/// entry" as an all-ones offset, and a zero or negative length says the same
/// thing; the reference accepts either.
const NO_ENTRY: u32 = 0xFFFF_FFFF;

/// The skill files could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum SkillsError {
    /// A file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// `Skills.idx` is not a whole number of twelve-byte entries, so it is not
    /// that index at all — refused rather than truncated, because a reader that
    /// rounds down here numbers every skill after the ragged end wrongly.
    NotAnIndex {
        /// Which file.
        path: PathBuf,
        /// How long it was.
        size: usize,
    },
}

impl fmt::Display for SkillsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotAnIndex { path, size } => write!(
                f,
                "{} is {size} bytes, not a whole number of {INDEX_ENTRY}-byte entries",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SkillsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::NotAnIndex { .. } => None,
        }
    }
}

impl Skills {
    /// Open the `Skills.idx`/`skills.mul` pair in a client directory.
    ///
    /// The capital in `Skills.idx` is the install's own, and the reference asks
    /// for it by that name — which matters on a case-sensitive filesystem, where
    /// this is the one pair in the client whose two halves are not spelled alike.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, SkillsError> {
        let dir = client_dir.as_ref();
        Self::from_files(dir.join("Skills.idx"), dir.join("skills.mul"))
    }

    /// Open a named pair, for tests.
    pub fn from_files(idx: impl AsRef<Path>, mul: impl AsRef<Path>) -> Result<Self, SkillsError> {
        let idx_path = idx.as_ref();
        let index = std::fs::read(idx_path).map_err(|source| SkillsError::Read {
            path: idx_path.to_owned(),
            source,
        })?;
        let mul_path = mul.as_ref();
        let records = std::fs::read(mul_path).map_err(|source| SkillsError::Read {
            path: mul_path.to_owned(),
            source,
        })?;
        Self::parse(&index, &records).ok_or_else(|| SkillsError::NotAnIndex {
            path: idx_path.to_owned(),
            size: index.len(),
        })
    }

    /// Read a pair already in memory, or `None` if the index is not a whole
    /// number of entries.
    ///
    /// A record the index points past the end of `skills.mul` is dropped the way
    /// an empty entry is: the alternative is to invent a name, and a window with
    /// a blank row in it says less than a window with one row fewer.
    pub fn parse(index: &[u8], records: &[u8]) -> Option<Self> {
        if !index.len().is_multiple_of(INDEX_ENTRY) {
            return None;
        }
        let mut skills = Vec::new();
        for entry in index.as_chunks::<INDEX_ENTRY>().0 {
            let offset = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let length = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
            // "Nothing here", written three ways, all of which the reference
            // accepts: no offset, no length, or a record too short to hold even
            // the flag byte its name follows.
            if offset == NO_ENTRY || length == NO_ENTRY || length < 2 {
                continue;
            }
            let (Ok(start), Ok(len)) = (usize::try_from(offset), usize::try_from(length)) else {
                continue;
            };
            let Some(record) = records.get(start..start + len) else {
                continue;
            };
            let name: String = record[1..]
                .iter()
                .take_while(|byte| **byte != 0)
                .map(|byte| char::from(*byte))
                .collect();
            skills.push(Skill {
                name,
                has_action: record[0] != 0,
            });
        }
        Some(Self { skills })
    }

    /// How many skills this install names.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether it names none.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// The id of the `nth` skill the file named, or `None` past the end — the
    /// one door onto [`SkillId`], so that an id can only name a row that is
    /// there.
    pub fn id(&self, nth: usize) -> Option<SkillId> {
        (nth < self.skills.len() && nth <= usize::from(u8::MAX)).then_some(SkillId(nth as u8))
    }

    /// One skill's row, or `None` for an id past this install's table — which is
    /// what a shard sending a newer client's skill list to an older install
    /// looks like.
    pub fn get(&self, id: SkillId) -> Option<&Skill> {
        self.skills.get(usize::from(id.0))
    }

    /// Every skill, in the file's own order, with its id.
    pub fn iter(&self) -> impl Iterator<Item = (SkillId, &Skill)> {
        self.skills
            .iter()
            .enumerate()
            .map(|(nth, skill)| (SkillId(nth as u8), skill))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two records, laid out the way the file does: a flag byte, then the name,
    /// then its terminator.
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        let mut records = Vec::new();
        records.push(0); // Alchemy: no button
        records.extend_from_slice(b"Alchemy\0");
        records.push(1); // Anatomy: a button
        records.extend_from_slice(b"Anatomy\0");
        let mut index = Vec::new();
        for (offset, length) in [(0u32, 9u32), (9, 9)] {
            index.extend_from_slice(&offset.to_le_bytes());
            index.extend_from_slice(&length.to_le_bytes());
            index.extend_from_slice(&0u32.to_le_bytes());
        }
        (index, records)
    }

    #[test]
    fn a_record_is_a_flag_byte_and_then_its_name() {
        let (index, records) = fixture();
        let skills = Skills::parse(&index, &records).expect("the index is whole entries");
        assert_eq!(skills.len(), 2);
        let alchemy = skills.get(SkillId(0)).expect("id 0 was read");
        assert_eq!(alchemy.name, "Alchemy");
        assert!(!alchemy.has_action, "the leading zero is the flag, not the name");
        assert!(skills.get(SkillId(1)).expect("id 1 was read").has_action);
    }

    /// The empty entries are most of the file — 256 index entries and 58 skills
    /// — and they are not skills: one that took an id would put a blank row in
    /// the window and shift every skill after it.
    #[test]
    fn an_empty_index_entry_is_not_a_skill() {
        let (mut index, records) = fixture();
        let mut padded = vec![0xFFu8; INDEX_ENTRY]; // an all-ones entry
        padded.extend_from_slice(&index);
        index = padded;
        index.extend_from_slice(&[0xFF; INDEX_ENTRY]);
        let skills = Skills::parse(&index, &records).expect("the index is whole entries");
        assert_eq!(skills.len(), 2, "the two real records, and only those");
        assert_eq!(skills.get(SkillId(0)).expect("id 0 was read").name, "Alchemy");
    }

    #[test]
    fn a_record_past_the_end_of_the_file_is_dropped() {
        let (mut index, records) = fixture();
        index.extend_from_slice(&9_000u32.to_le_bytes());
        index.extend_from_slice(&9u32.to_le_bytes());
        index.extend_from_slice(&0u32.to_le_bytes());
        let skills = Skills::parse(&index, &records).expect("the index is whole entries");
        assert_eq!(skills.len(), 2);
    }

    #[test]
    fn a_ragged_index_is_refused_rather_than_rounded_down() {
        let (mut index, records) = fixture();
        index.push(0);
        assert!(Skills::parse(&index, &records).is_none());
    }

    /// An id is only ever made from a row that exists, which is what keeps
    /// [`SkillId`] from being a `u8` with a nicer name.
    #[test]
    fn an_id_names_a_row_that_is_there() {
        let (index, records) = fixture();
        let skills = Skills::parse(&index, &records).expect("the index is whole entries");
        assert_eq!(skills.id(1), Some(SkillId(1)));
        assert_eq!(skills.id(2), None, "past the table");
        assert_eq!(skills.get(SkillId(2)), None);
    }
}
