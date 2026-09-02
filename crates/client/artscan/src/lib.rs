//! Measure a client's art once, off the clock, and read the table back.
//!
//! `docs/archive/render/lighting.md`'s decision 31. [`facing_of`](openshard_client_render::facing::facing_of)
//! used to run while the atlas packed a sprite — on the frame a graphic was first
//! seen, on the player's machine — which was right when the measurement was one
//! pass over 44×80 pixels and there was one of them. It stops being right twice
//! over: a scroll that introduces four hundred graphics pays for four hundred at
//! once, and every measurement this pass still wants is a bigger one than the
//! last. An aperture is a hole to be found (step 16), a corner is two fits, a
//! mesh would be a solve.
//!
//! So the measurement moves out of the frame entirely. The budget goes from a
//! frame to a minute, and what that buys is not speed but *ambition*: a runtime
//! pass has to be a scanline trick, and an offline one can afford whatever the
//! art needs.
//!
//! # The three halves of this crate
//!
//! - **The sweep** — [`measure`], which offers every graphic the install ships to
//!   the detector and writes down what came back.
//! - **The file** — [`table_path`], [`save`] and [`load`]. The table is
//!   *derived from copyrighted art*, so it is written beside the install and
//!   never into this repository (decision 31.3); what is checked in is this tool
//!   and [`OVERRIDES`].
//! - **The reader the client uses** — [`load`] again, which is here rather than
//!   in `client/app` so that the tool and the client cannot drift apart about
//!   where the file lives or about what makes it stale.
//!
//! # Running it
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo run -p openshard-client-artscan
//! ```
//!
//! Four seconds over an install of 39,189 pictures — see the binary's own header
//! for why that is a measurement rather than an estimate.
//!
//! It is not required. A client with no table measures as it packs and says so in
//! a log line — decision 31.6, a slow first frame rather than a shard that will
//! not start.

use std::path::{
    Path,
    PathBuf,
};

use openshard_client_render::arttable::{
    ArtTable,
    Stamp,
    TableError,
};
use openshard_client_render::facing;
use openshard_client_render::occlusion::Shape;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;

pub mod interiors;

/// The rows this repository ships: a table with no install behind it, holding
/// only what somebody decided the detector gets wrong.
///
/// Empty of rows today, and that is the honest state — every graphic the client
/// ships is read by the rules in `facing.rs` or refused by them, and nobody has
/// yet had to correct one by hand. What it is for is the moment somebody does:
/// decision 31.2 says a shard fixing one wall edits a file rather than patching a
/// detector, and this is the file that travels with the engine rather than with
/// one person's install.
///
/// `include_str!` rather than a read: this is content, it is small, and a tool
/// that could not find its own overrides would be a tool that quietly derived
/// everything.
pub const OVERRIDES: &str = include_str!("../data/overrides.table");

/// Where the table for an install lives.
///
/// Beside the install by default, because that is what it describes and because
/// a machine with two installs on it has two tables. `OPENSHARD_ART_TABLE` moves
/// it, which is the answer for an install on a read-only volume — the case that
/// would otherwise turn "derive once" into "derive every run".
pub fn table_path(client_dir: &Path) -> PathBuf {
    match std::env::var_os("OPENSHARD_ART_TABLE") {
        Some(path) => PathBuf::from(path),
        None => client_dir.join(TABLE_FILE),
    }
}

/// The name the table is written under beside an install.
const TABLE_FILE: &str = "openshard-art.table";

/// The art container every measurement in this crate is made from.
const ART_FILE: &str = "artLegacyMUL.uop";

/// What an install in front of this build would stamp a fresh table with.
///
/// The length of the art file and nothing more expensive: hashing hundreds of
/// megabytes costs more than re-deriving the table it would be validating, and
/// what this has to catch is a *different install* rather than a corrupted one.
pub fn stamp_of(client_dir: &Path) -> Result<Stamp, ScanError> {
    let path = client_dir.join(ART_FILE);
    let bytes = std::fs::metadata(&path)
        .map_err(|source| {
            ScanError::Io {
                path: path.clone(),
                source,
            }
        })?
        .len();
    Ok(Stamp {
        art: ART_FILE.to_string(),
        bytes,
        detector: facing::DETECTOR,
    })
}

/// Offer every graphic the install ships to the detector, and write down what
/// came back.
///
/// **Every graphic and not only the walls.** The atlas packs whatever is on
/// screen and asks about each, so a table that had looked at the `WALL` graphics
/// alone would answer "measured and refused" about a picture nobody measured —
/// which is exactly the lie [`ArtTable::examined`] exists to make impossible.
///
/// `keep` is the authored rows this run must not overwrite: the overrides this
/// repository ships, and whatever a person has edited into the table beside their
/// own install. Both are tables and both are merged the same way, because
/// decision 31.2 is one file format and one precedence rule.
pub fn measure(art: &Art, stamp: Stamp, keep: &[ArtTable]) -> ArtTable {
    let mut table = ArtTable::measured(stamp);
    for sheet in keep {
        table.adopt_authored(sheet);
    }
    for id in 0..=u16::MAX {
        let graphic = Graphic(id);
        // A graphic the client ships no art for is not "refused" — there was
        // nothing to look at — so it is not offered and not counted. The atlas
        // reaches the same conclusion the same way: no picture, nothing packed.
        let Ok(Some(image)) = art.static_art(graphic) else {
            continue;
        };
        // Both measurements, by the same call the client's own fallback path
        // makes — see `Shape::of`. Two routes to one answer is how a table and a
        // client come to disagree about a picture.
        table.derive(graphic, Shape::of(&image));
    }
    table
}

/// The authored rows already beside an install, or `None` when no table has
/// been written there yet.
///
/// An existing unreadable or malformed table is an error: it may contain the
/// only copy of a person's overrides, so treating it like an absent file would
/// let the next [`save`] silently overwrite those edits.
pub fn authored_beside(path: &Path) -> Result<Option<ArtTable>, ScanError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ScanError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    ArtTable::parse(&text).map(Some).map_err(|source| {
        ScanError::Table {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Write a table where the client will look for it.
pub fn save(path: &Path, table: &ArtTable) -> Result<(), ScanError> {
    std::fs::write(path, table.to_text()).map_err(|source| {
        ScanError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Read the table for an install, or say why there is none to read.
///
/// **The client's whole side of decision 31**, and every arm of [`Missing`] is a
/// client that still starts: it measures as it packs, exactly as it did before
/// this crate existed. That is decision 31.6, and it is why this returns a reason
/// rather than an error — the caller's job is to log which of these happened and
/// carry on, not to decide whether to run.
///
/// [`Missing`]: LoadError
pub fn load(client_dir: &Path) -> Result<ArtTable, LoadError> {
    let path = table_path(client_dir);
    let want = stamp_of(client_dir).map_err(LoadError::Install)?;
    let text = std::fs::read_to_string(&path).map_err(|source| {
        LoadError::Unread {
            path: path.clone(),
            source,
        }
    })?;
    let table = ArtTable::parse(&text).map_err(|source| {
        LoadError::Unparsed {
            path: path.clone(),
            source,
        }
    })?;
    // Decision 31.4: staleness is detected, not assumed. A table describing a
    // different install would move every wall's face by a rule nobody could see,
    // and it would look exactly like a table that was right.
    if !table.fresh(&want) {
        return Err(LoadError::Stale {
            path,
            found: table.stamp().cloned(),
            want,
        });
    }
    Ok(table)
}

/// Why there is no table to read. Each of these is a client that starts anyway.
#[derive(Debug)]
pub enum LoadError {
    /// The install itself could not be looked at, so there is nothing to be
    /// fresh against.
    Install(ScanError),
    /// No file where the table would be — the ordinary case on a machine where
    /// the tool has never been run.
    Unread {
        /// Where it was looked for.
        path:   PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },
    /// A file that is not a table this build can read.
    Unparsed {
        /// Which file.
        path:   PathBuf,
        /// Which line, and what was wanted on it.
        source: TableError,
    },
    /// A table measured from another install, or by another version of the
    /// rules.
    Stale {
        /// Which file.
        path:  PathBuf,
        /// What it says it was measured from, or `None` for a sheet of
        /// overrides that was handed to a client by mistake.
        found: Option<Stamp>,
        /// What this install and this build would have stamped it with.
        want:  Stamp,
    },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Install(source) => write!(f, "{source}"),
            Self::Unread { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Unparsed { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Stale { path, found, want } => {
                match found {
                    Some(found) => {
                        write!(
                            f,
                            "{}: measured from {} of {} bytes by detector {}, and this is {} of {} bytes \
                     by detector {}",
                            path.display(),
                            found.art,
                            found.bytes,
                            found.detector,
                            want.art,
                            want.bytes,
                            want.detector,
                        )
                    }
                    None => {
                        write!(
                            f,
                            "{}: a sheet of overrides, not a measured table",
                            path.display()
                        )
                    }
                }
            }
        }
    }
}

impl std::error::Error for LoadError {
}

/// Why a measurement could not be made.
#[derive(Debug)]
pub enum ScanError {
    /// A file would not open, or a table would not be written.
    Io {
        /// Which one.
        path:   PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },
    /// An existing table could not be parsed safely enough to preserve its
    /// authored rows.
    Table {
        /// Which table.
        path:   PathBuf,
        /// Which line, and what was wanted on it.
        source: TableError,
    },
    /// The client's art would not open.
    Art(openshard_uofiles::art::ArtError),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Table { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Art(source) => write!(f, "opening {ART_FILE}: {source}"),
        }
    }
}

impl std::error::Error for ScanError {
}

/// The overrides this repository ships, parsed.
///
/// A panic and not a `Result`: the file is checked in, it is compiled into this
/// binary, and a build that shipped an unparseable one would derive everything
/// and say nothing. The test below is what keeps that from being discovered by a
/// player.
pub fn overrides() -> ArtTable {
    ArtTable::parse(OVERRIDES).expect("data/overrides.table is checked in and parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sheet this crate ships parses, and it is a sheet rather than a table:
    /// no install behind it, so it can never be mistaken for a measurement.
    #[test]
    fn the_shipped_overrides_are_a_readable_sheet() {
        let sheet = overrides();
        assert!(sheet.stamp().is_none(), "an override sheet describes no install",);
        assert_eq!(
            sheet.len(),
            sheet.authored(),
            "an unmarked row in the shipped sheet would be a derived answer nobody derived, and \
             `adopt_authored` would drop it silently",
        );
    }

    /// Absence is harmless, but a broken existing table may hold the only copy
    /// of somebody's authored rows and must not be silently overwritten.
    #[test]
    fn a_broken_existing_table_is_not_treated_as_absent() {
        let dir = std::env::temp_dir().join(format!(
            "openshard-artscan-broken-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time after the epoch")
                .as_nanos(),
        ));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join(TABLE_FILE);

        assert!(
            authored_beside(&path)
                .expect("an absent table is not an error")
                .is_none(),
        );
        std::fs::write(&path, "not an art table\n").expect("writing a broken table");

        let error = authored_beside(&path).expect_err("a broken table must stop the overwrite");
        assert!(
            matches!(error, ScanError::Table { path: failed, .. } if failed == path),
            "the parse error identifies the table that must be preserved",
        );

        std::fs::remove_file(&path).expect("cleaning up the table");
        std::fs::remove_dir(&dir).expect("cleaning up the temp dir");
    }

    /// A table from a different install is not loaded, whichever half differs.
    ///
    /// The file is written and read back through the real [`save`] and
    /// [`ArtTable::parse`], so what is being tested is the pair of them over a
    /// real filesystem — the staleness rule is one `==` and the thing that
    /// actually breaks is a stamp that never made it into the text.
    #[test]
    fn a_table_written_for_another_install_is_refused() {
        let dir = std::env::temp_dir().join(format!("openshard-artscan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join(TABLE_FILE);

        let stamp = Stamp {
            art:      ART_FILE.to_string(),
            bytes:    12_345,
            detector: facing::DETECTOR,
        };
        let mut table = ArtTable::measured(stamp.clone());
        table.derive(
            Graphic(0x0007),
            Shape::faced(facing::Facing::One(facing::Face::South)),
        );
        save(&path, &table).expect("writing the table");

        let read =
            ArtTable::parse(&std::fs::read_to_string(&path).expect("reading it back")).expect("its own text");
        assert!(read.fresh(&stamp), "the same install and the same rules");
        assert!(
            !read.fresh(&Stamp {
                bytes: 999,
                ..stamp.clone()
            }),
            "another install's art",
        );
        assert!(
            !read.fresh(&Stamp {
                detector: facing::DETECTOR + 1,
                ..stamp
            }),
            "another version of the rules",
        );

        std::fs::remove_file(&path).expect("cleaning up");
    }
}
