//! `Bodyconv.def`: which of an install's animation files holds a body.
//!
//! An install ships more than one `anim*.mul`. The first is the one everything
//! else in this crate reads; the five beside it — `anim2` through `anim6` — hold
//! the bodies added by later expansions, each *re-numbered* from zero rather
//! than appended to the first file's index. This table is the only thing that
//! says where a body went, and it says it twice over: which file, and which id
//! the body carries inside it.
//!
//! # A row is an id and five columns
//!
//! ```text
//! 752     29      -1      -1      -1      -1
//! 794     205     -1      -1      -1      -1
//! 1533    -1      -1      -1      -1      300
//! ```
//!
//! The columns are `anim2`, `anim3`, `anim4`, `anim5`, `anim6` in that order,
//! and `-1` is "not in this one". Body 752 is drawn from `anim2.mul` under id
//! 29; nothing in `anim.mul` holds a body 752 at all, which is why a shard that
//! spawns one and a reader that only opens the first file put a creature on
//! screen as *nothing*.
//!
//! No shipped row names two files. The reference client
//! (`AnimationsLoader.ProcessBodyConvDef`) still walks every column and keeps
//! the last one whose file the install actually ships, so that is what
//! [`BodyConv`] hands back — see [`BodyConvRow::target`].
//!
//! # What this table is deliberately not
//!
//! - **Not a hue.** `BodyConvInfo.Hue` exists in the reference and no line of
//!   `ProcessBodyConvDef` ever assigns it. The forced hue that *does* exist is
//!   `Body.def`'s ([`crate::anim::BodyDef`]), which is a different file with a
//!   different job.
//! - **Not a mount height.** The reference attaches a `MountHeight` to some of
//!   these rows — but it reads it out of six hard-coded body ids in its own
//!   source rather than out of the file, so it is a fact about drawing a rider
//!   and not about this table. Recorded in the roadmap rather than half-ported
//!   here.
//! - **Not `Body.def`.** That one redirects a body to another body *in the same
//!   file* and is applied before this table is consulted, exactly as the
//!   reference applies `ReplaceBody` before `GetIndices`.

use std::collections::BTreeMap;
use std::path::Path;

use openshard_protocol::wire::Graphic;

use crate::anim::{
    AnimError,
    AnimFile,
};

/// Where one body's frames were moved to, one column per file.
///
/// Kept whole rather than resolved to a single answer at parse time: which
/// column wins depends on which files the install ships, and that is known to
/// the reader that opened them rather than to the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BodyConvRow {
    /// The id this body carries in each of [`AnimFile::REDIRECTS`], in that
    /// order. `None` is the file's own `-1`: this body is not in that file.
    targets: [Option<Graphic>; AnimFile::REDIRECTS.len()],
}

impl BodyConvRow {
    /// The id this body carries in `file`, or `None` where the row's column for
    /// that file is `-1`.
    ///
    /// [`AnimFile::First`] is always `None`: a row exists precisely to say that
    /// the body is *not* in the first file, and the reference has no column for
    /// it either — its columns start at `_files[1]`.
    #[must_use]
    pub fn target(self, file: AnimFile) -> Option<Graphic> {
        let [second, third, fourth, fifth, sixth] = self.targets;
        match file {
            AnimFile::First => None,
            AnimFile::Second => second,
            AnimFile::Third => third,
            AnimFile::Fourth => fourth,
            AnimFile::Fifth => fifth,
            AnimFile::Sixth => sixth,
        }
    }
}

/// The `Bodyconv.def` beside a client install.
///
/// An install that ships none is an empty table, which is not a stand-in for
/// the file: it is the file saying nothing, and every body is then read from
/// the first animation file exactly as the reference does when
/// `ProcessBodyConvDef` finds no file to process.
#[derive(Clone, Debug)]
pub struct BodyConv {
    rows: BTreeMap<u16, BodyConvRow>,
}

impl BodyConv {
    /// The table an install with no `Bodyconv.def` has.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            rows: BTreeMap::new(),
        }
    }

    /// Read `Bodyconv.def` when the install ships one.
    ///
    /// Pre-3.0.0 clients legitimately have no such file — the reference does
    /// not even look for one below that version — and that is the same as an
    /// empty table rather than an error.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, AnimError> {
        let path = client_dir.as_ref().join("Bodyconv.def");
        let source = match std::fs::read(&path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(source) => return Err(AnimError::Read { path, source }),
        };
        // Decoded lossily for the reason `mobtypes.txt` is: the rows are ASCII
        // and the comments naming each creature are not always, and a body's
        // file must not be lost because a comment is not UTF-8.
        Ok(Self::from_text(&String::from_utf8_lossy(&source)))
    }

    /// Parse the file's text.
    ///
    /// Public for the same reason [`crate::mobtypes::MobTypes::from_text`] is:
    /// a test about one row should be able to state that row rather than needing
    /// an install on disk to say "752 is drawn out of `anim2`".
    #[must_use]
    pub fn from_text(source: &str) -> Self {
        let mut rows = BTreeMap::new();
        for line in source.lines() {
            // Everything from `#` is a comment. The stock file uses it for the
            // header, for whole commented-out rows, and for a trailing note
            // naming the creature — and that note contains numbers.
            let line = line.split('#').next().unwrap_or_default();
            let mut fields = line.split_whitespace();
            let Some(Ok(id)) = fields.next().map(str::parse::<u16>) else {
                continue;
            };
            let mut targets = [None; AnimFile::REDIRECTS.len()];
            for slot in &mut targets {
                // A column that is absent, negative or unreadable all mean the
                // same thing the file's own `-1` means: this body is not in
                // that file. Nothing is guessed here — a `0` would be body zero,
                // which is a real body.
                let Some(Ok(target)) = fields.next().map(str::parse::<i32>) else {
                    break;
                };
                *slot = u16::try_from(target).ok().map(Graphic);
            }
            rows.insert(id, BodyConvRow { targets });
        }
        Self { rows }
    }

    /// What the file says about a body, or `None` where it says nothing — which
    /// is the ordinary answer, and means the body is read from
    /// [`AnimFile::First`] under its own id.
    #[must_use]
    pub fn row(&self, body: Graphic) -> Option<BodyConvRow> {
        self.rows.get(&body.0).copied()
    }

    /// Every body this table redirects, in id order.
    ///
    /// Public so a caller deriving a per-body table of its own — a fallback
    /// numbering resolved against each redirect, for one — can walk every row
    /// without also carrying an opinion about which id is interesting.
    /// `impl Iterator` rather than a named type: the honest type is a tower of
    /// `Keys`/`Copied`/`Map`, and every caller of this crate reads it as
    /// "some bodies", never as anything that type names.
    pub fn bodies(&self) -> impl Iterator<Item = Graphic> + '_ {
        self.rows.keys().copied().map(Graphic)
    }

    /// Which file `body` is actually read from, and what id it carries there,
    /// once a file this install does not ship is excluded.
    ///
    /// [`BodyConvRow::target`] alone answers what the file *says*; this
    /// answers what a reader with only some of the six pairs open does with
    /// that — the last column naming a file `present` accepts wins, which is
    /// [`crate::anim::Anim::source`]'s own redirect walk. `present` is a
    /// predicate rather than a concrete reader so a test can state "this
    /// install ships `anim3` and nothing else" without opening one.
    ///
    /// A body the table says nothing about reads as itself, out of
    /// [`AnimFile::First`] — the ordinary answer, and the one every id gets
    /// when this table is [`empty`](Self::empty).
    #[must_use]
    pub fn redirect(&self, body: Graphic, present: impl Fn(AnimFile) -> bool) -> (AnimFile, Graphic) {
        let mut file = AnimFile::First;
        let mut read_as = body;
        if let Some(row) = self.row(body) {
            for candidate in AnimFile::REDIRECTS {
                // The last column naming a file the predicate accepts wins,
                // which is what the reference's loop leaves behind: it
                // overwrites the body's entry as it walks the row and skips a
                // column whose file is not open. No shipped row names two.
                match (row.target(candidate), present(candidate)) {
                    (Some(target), true) => {
                        file = candidate;
                        read_as = target;
                    }
                    _ => continue,
                }
            }
        }
        (file, read_as)
    }

    /// How many bodies the table moves. Zero is an install with no file.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the table moves no bodies at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::BodyKind;

    /// [`BodyConv::redirect`] is what [`crate::anim::Anim::source`]'s layout
    /// fallback and [`crate::anim::Anim::redirect_kinds`]'s numbering
    /// fallback both read, so a body whose original id and landing id
    /// disagree about which family it belongs to gets the *landing*'s answer
    /// for both. Reading a body that is a monster by the plain id-range rule
    /// as a monster's numbering while its actual frames are a person's would
    /// draw one creature two ways — the id-range rule alone cannot tell.
    #[test]
    fn a_body_moved_across_a_files_own_boundary_is_typed_by_where_it_lands() {
        let table = BodyConv::from_text("100\t-1\t500\t-1\t-1\t-1\n");
        let (file, read_as) = table.redirect(Graphic(100), |candidate| candidate == AnimFile::Third);
        assert_eq!(file, AnimFile::Third);
        assert_eq!(read_as, Graphic(500));
        assert_eq!(
            BodyKind::of(Graphic(100)),
            BodyKind::Monster,
            "the id-range rule this fallback replaces",
        );
        assert_eq!(
            BodyKind::in_file(read_as, file),
            BodyKind::Human,
            "id 500 is a person in anim3's own range",
        );
    }

    /// The presence check is not decorative: an install below the expansion
    /// that added `anim3` has no such file open, and the reference reads the
    /// body under its own id rather than fail — [`Anim::source`]'s own
    /// behaviour, mirrored here without opening a real pair.
    #[test]
    fn a_redirect_to_a_file_this_install_does_not_ship_reads_as_the_original_body() {
        let table = BodyConv::from_text("100\t-1\t500\t-1\t-1\t-1\n");
        let (file, read_as) = table.redirect(Graphic(100), |_| false);
        assert_eq!(file, AnimFile::First, "no candidate file is open");
        assert_eq!(read_as, Graphic(100), "so the body reads under its own id");
    }

    #[test]
    fn a_row_is_an_id_and_one_column_per_animation_file() {
        let table = BodyConv::from_text("752\t29\t-1\t-1\t-1\t-1\n1533\t-1\t-1\t-1\t-1\t300\n");
        let row = table.row(Graphic(752)).expect("the row it has");
        assert_eq!(row.target(AnimFile::Second), Some(Graphic(29)));
        assert_eq!(row.target(AnimFile::Third), None);
        assert_eq!(row.target(AnimFile::Sixth), None);

        let last = table.row(Graphic(1533)).expect("the sixth-file row");
        assert_eq!(last.target(AnimFile::Second), None);
        assert_eq!(
            last.target(AnimFile::Sixth),
            Some(Graphic(300)),
            "the fifth column is anim6, not anim5",
        );
    }

    /// The first file has no column: a row is the statement that the body is
    /// somewhere else.
    #[test]
    fn the_first_file_is_not_one_of_the_columns() {
        let table = BodyConv::from_text("752\t29\t-1\t-1\t-1\t-1\n");
        let row = table.row(Graphic(752)).expect("the row it has");
        assert_eq!(row.target(AnimFile::First), None);
    }

    #[test]
    fn a_body_the_table_does_not_name_has_no_row() {
        let table = BodyConv::from_text("752\t29\t-1\t-1\t-1\t-1\n");
        assert_eq!(table.row(Graphic(753)), None);
        assert_eq!(table.len(), 1);
    }

    /// The stock file's own shapes: a comment header, a commented-out row, and
    /// a trailing note that contains numbers of its own.
    #[test]
    fn comments_carry_no_bodies_and_a_trailing_note_is_not_a_column() {
        let table = BodyConv::from_text(
            "# Bodyconv.def\n#752\t29\t-1\t-1\t-1\t-1\n666\t-1\t-1\t-1\t666\t-1\t# \tGargoyle Male\n",
        );
        assert_eq!(table.len(), 1, "only the row that is not commented out");
        let row = table.row(Graphic(666)).expect("the gargoyle row");
        assert_eq!(row.target(AnimFile::Fifth), Some(Graphic(666)));
        assert_eq!(row.target(AnimFile::Sixth), None, "the note is not a column");
    }

    /// A row shorter than five columns is the columns it has. The stock file
    /// pads every row to five, but a hand-written one need not, and the missing
    /// columns are absences rather than zeroes.
    #[test]
    fn a_short_row_is_the_columns_it_has() {
        let table = BodyConv::from_text("752\t29\n");
        let row = table.row(Graphic(752)).expect("the short row");
        assert_eq!(row.target(AnimFile::Second), Some(Graphic(29)));
        assert_eq!(row.target(AnimFile::Third), None);
    }

    #[test]
    fn an_empty_table_moves_nothing() {
        let table = BodyConv::empty();
        assert!(table.is_empty());
        assert_eq!(table.row(Graphic(752)), None);
    }
}
