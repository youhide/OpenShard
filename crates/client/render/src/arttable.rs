//! What was measured off a client's art, written down once and read back.
//!
//! [`crate::facing`] measures which edge of its tile a wall stands on by walking
//! the sprite's pixels, and until this module existed it did that *in a frame*:
//! [`StaticAtlas`](crate::atlas::StaticAtlas) called it while packing, on the
//! frame a graphic was first seen, on the player's machine. `docs/archive/render/lighting.md`'s
//! decision 31 is why that stops: a scroll that introduces four hundred graphics
//! pays for four hundred measurements at once, and every measurement this pass
//! still wants is a bigger one than the last — an aperture is a hole to be found,
//! a corner is two fits, a mesh would be a solve. A budget of a frame buys a
//! scanline trick; a budget of a minute buys connected components, a fit, a
//! cross-check and an outlier list for a person to read.
//!
//! So the measurement moves out of the frame entirely: `openshard-client-artscan`
//! reads an install and writes one of these, and the client loads it. What lives
//! here is the **table** — the type, its text, and nothing that opens a file,
//! because this crate does not (see its `Cargo.toml`).
//!
//! # The text
//!
//! Line-oriented, `#` for a comment, and hand-editable on purpose — decision 31.2
//! is that a shard fixing one wall edits a file rather than patching a detector:
//!
//! ```text
//! table 5
//! detector 3
//! art artLegacyMUL.uop 447160596
//! examined 15234
//! 0x0007 face S
//! 0x003C face E hole 93 185 10 15
//! 0x0104 corner E S
//! 0x0166 corner E S prism W 1 3 5
//! 0x0171 none authored
//! 0x0736 none block 0 2 0 8 0 20 block 6 8 0 8 0 20 block 0 8 0 8 15 20 authored
//! 0x0A97 none footprint 0 8 3 8
//! ```
//!
//! A `hole` is the second verdict [`crate::facing`] reads off the same picture —
//! `near far bottom top`, the four numbers of a [`Hole`](crate::facing::Hole) —
//! and it is optional because almost nothing has one. Only a `face` may carry it:
//! a corner is two surfaces in one picture and there is nothing in a silhouette
//! that says which of them a hole belongs to, so the parser refuses the pairing
//! rather than letting a row state something the detector will not.
//!
//! A `prism` is the third, and the expensive one: the face the climb ends at and
//! one height per tread, which is a [`Prism`](crate::facing::Prism). It rides
//! *beside* the verdict rather than replacing it, because a solid's base is pixel
//! for pixel what two walls meeting leave — so the wall detector calls a staircase
//! a corner and the prism is the truer of the two answers, with
//! [`Builder::add`](crate::occlusion::Builder::add) picking between them on the
//! client's own `CLIMBABLE` bit. A `face` may not carry one for the mirror of the
//! hole's reason: [`Shape::of`](crate::occlusion::Shape::of) only ever scores
//! prisms against a picture it read as a corner, and a row saying otherwise would
//! state what no detector will.
//!
//! `block` is the fourth, and unlike the other three it is never derived: `x0
//! x1 y0 y1 z0 z1`, a box in eighths of the tile on the ground and `z` above
//! the static's own base, and a row may carry several — `docs/archive/render/lighting.md`'s
//! decision 41. An arch is a post, a post and a lintel; nothing a single climb
//! profile can be fitted to, which is what `prism` is. It may accompany any
//! verdict, including `none`, because nothing about it is a claim about which
//! edge a plane stands on.
//!
//! `footprint` is the fifth: `x0 x1 y0 y1`, the horizontal box
//! [`crate::facing::measure_footprint`] reads off a picture's own base edge, in
//! eighths of the tile like `block` — `docs/render/design_footprints.md`'s S1 and S2. Unlike
//! `block` it *is* derived, and unlike `hole` and `prism` it belongs to the
//! *absence* of a verdict: only a `none` row may carry one, because a face or a
//! corner already answers which edge the picture stands on, and a footprint
//! beside either would be a second, independent claim about the same base with
//! nothing saying which one [`crate::occlusion::boxes_of`] should read.
//!
//! **This is what a table is for.** The prism search is the whole cost of a scan —
//! seconds against milliseconds for the faces — and a table that could not carry
//! its answer meant it was paid again on every machine that packed the atlas, or,
//! where a table *was* installed, not paid at all and the staircase silently went
//! back to occluding like a run of wall. Measured once, written down, read back.
//!
//! **A row that is not there is a graphic that was measured and refused**, which
//! is why [`ArtTable::examined`] is in the header: it is the coverage count that
//! makes an absent row mean "undecided" rather than "never looked at". A detector
//! with no coverage number is a green light for having checked nothing, and a
//! *table* with no coverage number is the same green light one step further from
//! the pixels.
//!
//! # One table, and an authored row wins
//!
//! Decision 31.2 again: an override is a row in the same file with `authored` on
//! the end, and re-deriving leaves it alone — [`ArtTable::derive`] is what the
//! tool calls per graphic and it refuses to overwrite one. Two files with two
//! grammars would be two code paths and a question about which of them answers
//! first; one file with a marked row is neither.
//!
//! # And it is checked against what it was measured from
//!
//! [`Stamp`] is the file the art came out of and the version of the rules that
//! read it, and a table whose stamp does not match the install in front of it is
//! not trusted — `docs/client_versions.md` is why that is not paranoia: art
//! changes between client versions, and a table silently describing a *different*
//! install would move every wall's face by a rule nobody could see.

use std::collections::BTreeMap;
use std::fmt::{
    self,
    Write,
};

use openshard_protocol::wire::Graphic;

use crate::facing::{
    Block,
    Blocks,
    Face,
    Facing,
    Footprint,
    Hole,
    Prism,
    Span,
};
use crate::occlusion::Shape;

/// The version of this file format.
///
/// A table written by a newer build is refused rather than half-read: the rows
/// are the same shape today and the day a row grows a field, a reader that
/// ignored what it did not understand would answer confidently about a graphic
/// whose hole it never saw.
///
/// **Two** since step 16, which is that day arriving: a row may now carry the
/// hole measured off the picture, and a build reading a format-1 table would
/// read every window as a solid wall while looking perfectly fresh. The
/// [`Stamp`]'s detector version says the same thing about the same tables and it
/// is deliberately not the only one that does — a format is about what the file
/// can *say*, a detector about what was said.
///
/// **Three** for the prism, and the bump matters more here than it did for the
/// hole: a format-2 table is a file that *cannot* say a staircase is a solid, so
/// a build that half-read one would answer `prism: None` for every stair — the
/// one state `docs/archive/render/lighting.md`'s backlog called a trap, because the client then
/// occludes a flight of steps like a run of wall and the only sign is that
/// somebody once ran a tool.
///
/// **Four** for the block list, `docs/archive/render/lighting.md`'s decision 41: a shape a
/// single climb profile cannot describe — an arch's posts and lintel — gets a
/// second, independent kind of solid rather than a wider `Prism`. Authored
/// only, the same trap as the prism's: a format-3 reader would silently drop
/// every block a person had placed, and a table that had never carried one
/// would look exactly like a table that could not.
///
/// **Five** for the footprint, `docs/render/design_footprints.md`'s S2: a row may now carry
/// the horizontal box S1 measures off a `none` verdict's own base edge —
/// derived, unlike the block list, and the same trap as the hole's and the
/// prism's each: a format-4 reader would answer `footprint: None` for every
/// row that has one, and look exactly as fresh as a table written after,
/// while the fallback it was meant to replace keeps drawing every one of
/// those statics a whole tile wide.
pub const FORMAT: u32 = 5;

/// What a table was measured from, and by which rules.
///
/// Two independent things and both of them make a table stale:
///
/// - **The art.** `artLegacyMUL.uop`'s name and its length in bytes. Not a
///   content hash: the file is hundreds of megabytes, hashing it costs more than
///   re-deriving the table it would be validating, and the thing this has to
///   catch is a *different install* rather than a corrupted one. A patch that
///   changes the art without changing its length is what this misses, and the
///   sweep in `artscan`'s own test — every graphic's row against a live
///   `facing_of` — is what catches that on the machine that has the files.
/// - **The rules.** [`crate::facing::DETECTOR`], bumped when a gate in `facing`
///   changes, because a table written under the old gates describes yesterday's
///   detector exactly and looks perfectly fresh.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stamp {
    /// The art container's file name, as the tool found it.
    pub art:      String,
    /// Its length in bytes.
    pub bytes:    u64,
    /// [`crate::facing::DETECTOR`] at the time it was written.
    pub detector: u32,
}

/// One graphic's verdict, and where it came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Row {
    /// What the art says the picture is: which surface, and the hole in it. Both
    /// `None` is a graphic nothing may be said about — which is only ever written
    /// down when a person said so, since a derived refusal is the absence of a
    /// row.
    shape:    Shape,
    /// Whether a person wrote this row. The tool leaves it alone.
    authored: bool,
}

/// Everything measured off one install's art, keyed by graphic.
///
/// Ordered rather than hashed, because the file it is written to is read by
/// people and diffed by them: a table whose rows moved about between two runs of
/// the same tool over the same art would make every re-derivation look like a
/// change.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct ArtTable {
    /// What it was measured from, or `None` for a sheet of overrides — a file
    /// with rows and no install behind them, which is what ships in this
    /// repository. A table with no stamp is never [`fresh`](Self::fresh).
    stamp:    Option<Stamp>,
    /// How many graphics with art were offered to the detector. Zero for an
    /// override sheet, and the reason an absent row means "refused" for a
    /// measured table and nothing at all for a sheet.
    examined: usize,
    rows:     BTreeMap<Graphic, Row>,
}

impl ArtTable {
    /// An empty table that says what it is about to be measured from.
    pub fn measured(stamp: Stamp) -> Self {
        Self {
            stamp:    Some(stamp),
            examined: 0,
            rows:     BTreeMap::new(),
        }
    }

    /// Record what the detector said about a graphic, and count it as examined.
    ///
    /// **An authored row is left alone**, which is decision 31.2's whole
    /// mechanism: the tool walks every graphic in the install and calls this for
    /// each, and a row a person wrote survives the walk. It is still counted as
    /// examined — the coverage number is about the *art*, not about who answered.
    pub fn derive(&mut self, graphic: Graphic, shape: Shape) {
        self.examined += 1;
        if self.rows.get(&graphic).is_some_and(|row| row.authored) {
            return;
        }
        match (shape.facing, shape.prism, shape.footprint) {
            // A refusal is the absence of a row: writing fifteen thousand
            // `none`s would bury the two hundred lines a person might want to
            // read, and `examined` is what makes the absence mean something.
            //
            // What decides is whether anything about the *shape* was read — a
            // facing, a prism, or now a footprint. A hole is not on that list
            // because it is not a third answer: it is a rectangle in a plane, so
            // without the plane there is nothing to write down, and
            // `aperture_of` refuses one for the same reason at the other end.
            (None, None, None) => self.rows.remove(&graphic),
            _ => {
                self.rows.insert(
                    graphic,
                    Row {
                        shape,
                        authored: false,
                    },
                )
            }
        };
    }

    /// Write a row by hand, overriding whatever was derived.
    ///
    /// [`Shape::UNREAD`] is a person saying *nothing may be said about this
    /// picture*, which is a different statement from a derived refusal and is
    /// therefore written down: it survives a re-derivation, where a derived
    /// refusal does not.
    pub fn author(&mut self, graphic: Graphic, shape: Shape) {
        self.rows.insert(
            graphic,
            Row {
                shape,
                authored: true,
            },
        );
    }

    /// Take every authored row from another table, overwriting what is here.
    ///
    /// What the tool does with the overrides this repository ships and with
    /// whatever a shard has edited into the table beside its own install: both
    /// are "the authored rows of a table", one grammar and one merge.
    pub fn adopt_authored(&mut self, from: &ArtTable) {
        for (graphic, row) in &from.rows {
            if row.authored {
                self.rows.insert(*graphic, *row);
            }
        }
    }

    /// What this says about a graphic: which surface its picture is of, and the
    /// hole in it. [`Shape::UNREAD`] for a graphic nothing may be said about.
    ///
    /// The two refusals — by the detector and by a person — are deliberately one
    /// answer here. They differ in whether a re-derivation keeps them, and
    /// nothing that *draws* has any use for the difference.
    pub fn shape(&self, graphic: Graphic) -> Shape {
        self.rows.get(&graphic).map_or(Shape::UNREAD, |row| row.shape)
    }

    /// The surface half of it alone, which is what everything that places a quad
    /// asks — see [`Sprite::facing`](crate::atlas::Sprite).
    pub fn facing(&self, graphic: Graphic) -> Option<Facing> {
        self.shape(graphic).facing
    }

    /// Whether this table describes the install in front of it, measured by the
    /// rules this build has.
    pub fn fresh(&self, against: &Stamp) -> bool {
        self.stamp.as_ref().is_some_and(|stamp| stamp == against)
    }

    /// What it says it was measured from, or `None` for an override sheet.
    pub fn stamp(&self) -> Option<&Stamp> {
        self.stamp.as_ref()
    }

    /// How many graphics with art were offered to the detector.
    pub fn examined(&self) -> usize {
        self.examined
    }

    /// How many of them it could say something about.
    pub fn decided(&self) -> usize {
        self.rows
            .values()
            .filter(|row| row.shape.facing.is_some())
            .count()
    }

    /// And how many of *those* have a hole in them, which is the tail step 16
    /// added: fifty-eight over a 2D install, and the only rows a window comes
    /// from. Its own number and not a share, for the reason [`corners`] is —
    /// a percentage would hide it going to zero.
    ///
    /// [`corners`]: Self::corners
    pub fn holed(&self) -> usize {
        self.rows.values().filter(|row| row.shape.hole.is_some()).count()
    }

    /// And how many are solids — the staircases and boxes step 22 measures.
    ///
    /// Its own number for the third time and the sharpest of the three: the prism
    /// search is nearly the whole cost of a scan, so a gate that quietly stopped
    /// admitting prisms would make the tool *faster* and leave every other count
    /// here where it was.
    pub fn prisms(&self) -> usize {
        self.rows.values().filter(|row| row.shape.prism.is_some()).count()
    }

    /// And how many carry a footprint — `docs/render/design_footprints.md`'s S1 and S2, and
    /// its own number for the same reason [`holed`] and [`prisms`] are: a share
    /// of the whole table would hide a gate that quietly stopped admitting them
    /// going to zero.
    ///
    /// [`holed`]: Self::holed
    /// [`prisms`]: Self::prisms
    pub fn footprints(&self) -> usize {
        self.rows
            .values()
            .filter(|row| row.shape.footprint.is_some())
            .count()
    }

    /// How many rows it holds at all — the decided ones and the hand-written
    /// refusals together.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether it holds none, which for a measured table is a detector that read
    /// nothing and for a sheet is the state this repository ships in.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// How many rows a person wrote.
    pub fn authored(&self) -> usize {
        self.rows.values().filter(|row| row.authored).count()
    }

    /// How many of the decided ones are corners, which is the tail decision 25
    /// added and the one a share would hide going to zero.
    pub fn corners(&self) -> usize {
        self.rows
            .values()
            .filter(|row| matches!(row.shape.facing, Some(Facing::Corner { .. })))
            .count()
    }

    /// The table as the text a file holds. The inverse of [`parse`](Self::parse).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("# Measured off a client's own art by `openshard-client-artscan`.\n");
        out.push_str("# A row that is not here is a graphic that was measured and refused.\n");
        out.push_str("# `authored` marks a row a person wrote; re-deriving leaves it alone.\n");
        writeln!(&mut out, "table {FORMAT}").expect("writing to a String cannot fail");
        if let Some(stamp) = &self.stamp {
            writeln!(&mut out, "detector {}", stamp.detector).expect("writing to a String cannot fail");
            writeln!(&mut out, "art {} {}", stamp.art, stamp.bytes).expect("writing to a String cannot fail");
            writeln!(&mut out, "examined {}", self.examined).expect("writing to a String cannot fail");
        }
        for (graphic, row) in &self.rows {
            let verdict = match row.shape.facing {
                None => "none".to_string(),
                Some(Facing::One(face)) => format!("face {}", letter(face)),
                Some(Facing::Corner { right, left }) => {
                    format!("corner {} {}", letter(right), letter(left))
                }
            };
            let hole = match row.shape.hole {
                None => String::new(),
                Some(hole) => format!(" hole {} {} {} {}", hole.near, hole.far, hole.bottom, hole.top),
            };
            let prism = match row.shape.prism {
                None => String::new(),
                Some(prism) => {
                    let mut clause = format!(" prism {}", letter(prism.up()));
                    for tread in prism.treads() {
                        write!(&mut clause, " {tread}").expect("writing to a String cannot fail");
                    }
                    clause
                }
            };
            let mut blocks = String::new();
            for block in row.shape.blocks.blocks() {
                write!(
                    &mut blocks,
                    " block {} {} {} {} {} {}",
                    block.x.min, block.x.max, block.y.min, block.y.max, block.z.min, block.z.max
                )
                .expect("writing to a String cannot fail");
            }
            let footprint = match row.shape.footprint {
                None => String::new(),
                Some(footprint) => {
                    format!(
                        " footprint {} {} {} {}",
                        footprint.x.min, footprint.x.max, footprint.y.min, footprint.y.max
                    )
                }
            };
            let authored = if row.authored { " authored" } else { "" };
            writeln!(
                &mut out,
                "{:#06X} {verdict}{hole}{prism}{blocks}{footprint}{authored}",
                graphic.0
            )
            .expect("writing to a String cannot fail");
        }
        out
    }

    /// Read a table back out of its text.
    ///
    /// A `Result` and not an `Option`, and every arm of [`TableError`] says which
    /// line: this is a file somebody may have edited by hand, which puts it in the
    /// same class as a packet off the wire — outside the process, and not an
    /// invariant. A table that will not parse is a table the client does without,
    /// which is what decision 31.6 promises.
    pub fn parse(text: &str) -> Result<Self, TableError> {
        let mut table = Self::default();
        let mut version: Option<u32> = None;
        let mut detector: Option<u32> = None;
        let mut art: Option<(String, u64)> = None;
        for (index, line) in text.lines().enumerate() {
            let at = index + 1;
            let line = match line.split_once('#') {
                Some((before, _)) => before.trim(),
                None => line.trim(),
            };
            if line.is_empty() {
                continue;
            }
            let mut words = line.split_whitespace();
            // The first word is the row's key or a header's name, and there is
            // one because the line is not empty.
            let head = words.next().unwrap();
            match head {
                "table" => version = Some(number(&mut words, at, "table")?),
                "detector" => detector = Some(number(&mut words, at, "detector")?),
                "art" => {
                    let name = words.next().ok_or(TableError::Line {
                        at,
                        detail: "art wants a file name and a length",
                    })?;
                    art = Some((name.to_string(), number(&mut words, at, "art")?));
                }
                "examined" => table.examined = number(&mut words, at, "examined")?,
                _ => {
                    let (graphic, row) = row(head, &mut words, at)?;
                    table.rows.insert(graphic, row);
                }
            }
            if words.next().is_some() {
                return Err(TableError::Line {
                    at,
                    detail: "trailing words",
                });
            }
        }

        // The version is checked after the whole file rather than at the first
        // line, so that a table from the future says so with its version in the
        // message rather than dying on whatever row it grew.
        match version {
            Some(FORMAT) => {}
            Some(found) => return Err(TableError::Format { found }),
            None => return Err(TableError::NoFormat),
        }
        // A stamp is all three or none of them: a table naming an install with no
        // detector version behind it is a claim about art with no claim about the
        // rules that read it, and half a stamp would pass `fresh` half the time.
        table.stamp = match (art, detector) {
            (Some((name, bytes)), Some(detector)) => {
                Some(Stamp {
                    art: name,
                    bytes,
                    detector,
                })
            }
            (None, None) => None,
            _ => return Err(TableError::HalfStamped),
        };
        Ok(table)
    }
}

/// One letter per face, which is what a row carries.
fn letter(face: Face) -> char {
    match face {
        Face::North => 'N',
        Face::East => 'E',
        Face::South => 'S',
        Face::West => 'W',
    }
}

/// And back. `None` for anything that is not one of the four.
fn face(letter: &str) -> Option<Face> {
    match letter {
        "N" => Some(Face::North),
        "E" => Some(Face::East),
        "S" => Some(Face::South),
        "W" => Some(Face::West),
        _ => None,
    }
}

/// A header's number, whatever the header is.
fn number<T: std::str::FromStr>(
    words: &mut std::str::SplitWhitespace<'_>,
    at: usize,
    what: &'static str,
) -> Result<T, TableError> {
    words
        .next()
        .and_then(|word| word.parse().ok())
        .ok_or(TableError::Line { at, detail: what })
}

/// One row: `0x0104 corner E S`, with `hole N F B T`, `prism U h…`, any number
/// of `block x0 x1 y0 y1 z0 z1` and `authored` optional on the end, in that
/// order.
fn row(
    head: &str,
    words: &mut std::str::SplitWhitespace<'_>,
    at: usize,
) -> Result<(Graphic, Row), TableError> {
    let digits = head.strip_prefix("0x").unwrap_or(head);
    let graphic = u16::from_str_radix(digits, 16).map_err(|_| {
        TableError::Line {
            at,
            detail: "a row starts with a graphic in hex",
        }
    })?;
    let bad = TableError::Line {
        at,
        detail: "a verdict is `face F`, `corner R L` or `none`",
    };
    let facing = match words.next().ok_or(bad)? {
        "none" => None,
        "face" => Some(Facing::One(face(words.next().ok_or(bad)?).ok_or(bad)?)),
        "corner" => {
            let right = face(words.next().ok_or(bad)?).ok_or(bad)?;
            let left = face(words.next().ok_or(bad)?).ok_or(bad)?;
            // The halves are not interchangeable — the right half of a tile's
            // column can only be north or east and the left only south or west,
            // which is how `facing_of` reads them — so a row that names them the
            // other way round is a row that would light a wall from inside.
            if !matches!(right, Face::North | Face::East) || !matches!(left, Face::South | Face::West) {
                return Err(TableError::Line {
                    at,
                    detail: "a corner is a right half (N or E) and a left half (S or W)",
                });
            }
            Some(Facing::Corner { right, left })
        }
        _ => return Err(bad),
    };
    // The hole, if the row states one. **Only a face may**: a corner is two
    // surfaces in one picture and nothing in a silhouette says which of them a
    // hole belongs to, so `aperture_of` refuses to measure one there and a row
    // that stated one anyway would be a window in the wall beside the window.
    let hole = match words.clone().next() {
        Some("hole") => {
            words.next();
            if !matches!(facing, Some(Facing::One(_))) {
                return Err(TableError::Line {
                    at,
                    detail: "only a `face` may carry a hole",
                });
            }
            let mut span = || number::<u8>(words, at, "a hole is `near far bottom top`");
            Some(Hole {
                near:   span()?,
                far:    span()?,
                bottom: span()?,
                top:    span()?,
            })
        }
        _ => None,
    };
    // The prism, if the row states one: the face the climb ends at and one height
    // per tread. **A `face` may not carry one**, which is the mirror of the hole's
    // rule and comes from the same place — `Shape::of` scores prisms only against
    // a picture the wall detector read as a corner, so a plain face with a prism
    // on it is a row stating what no detector will. A `none` may: that is a person
    // saying *this picture is a solid and nothing else*, which is exactly
    // `Shape::solid`.
    let prism = match words.clone().next() {
        Some("prism") => {
            words.next();
            if matches!(facing, Some(Facing::One(_))) {
                return Err(TableError::Line {
                    at,
                    detail: "a `face` may not carry a prism",
                });
            }
            let up = face(words.next().ok_or(bad)?).ok_or(bad)?;
            // The treads run to the end of the numbers. Nothing else on the line
            // parses as one — `authored` is the only word that may follow — so the
            // list needs no count in front of it, and a count would be a second
            // statement of the same fact for a hand-editor to get wrong.
            let mut treads = Vec::new();
            while let Some(word) = words.clone().next() {
                let Ok(height) = word.parse::<u8>() else { break };
                words.next();
                treads.push(height);
            }
            // `Prism::new` is the one that knows the cap, and it holds the
            // invariant a hand-written row could otherwise break: at least one
            // tread and at most `facing::MAX_TREADS`.
            Some(Prism::new(up, &treads).ok_or(TableError::Line {
                at,
                detail: "a prism is a face and one to four tread heights",
            })?)
        }
        _ => None,
    };
    // The blocks, zero or more: a box a person placed by eye, `x0 x1 y0 y1 z0
    // z1`. Never derived and never restricted to a verdict — see the module
    // doc's own argument for why an arch's shape is not a claim about which
    // edge a plane stands on.
    let mut blocks = Vec::new();
    while words.clone().next() == Some("block") {
        words.next();
        let mut span = || number::<u8>(words, at, "a block is `x0 x1 y0 y1 z0 z1`");
        let (x0, x1, y0, y1, z0, z1) = (span()?, span()?, span()?, span()?, span()?, span()?);
        blocks.push(
            Block::new(Span::new(x0, x1), Span::new(y0, y1), Span::new(z0, z1)).ok_or(TableError::Line {
                at,
                detail: "a block's spans must be non-empty and its x/y at most 8",
            })?,
        );
    }
    // `Blocks::new` is the one that knows the cap, the same discipline as the
    // prism's own: at most `facing::MAX_BLOCKS`.
    let blocks = Blocks::new(&blocks).ok_or(TableError::Line {
        at,
        detail: "a row may carry at most facing::MAX_BLOCKS blocks",
    })?;
    // The footprint, if the row states one: the horizontal box
    // `facing::measure_footprint` reads off the art's own base edge, in eighths
    // of the tile like `block`. **Only a `none` verdict may carry one** —
    // `docs/render/design_footprints.md`'s D4, the mirror of the hole's and the prism's own
    // restrictions: a face or a corner already says which edge the picture
    // stands on, and a footprint beside either would be a second, independent
    // answer about the same base with nothing saying which one `boxes_of`
    // should read.
    let footprint = match words.clone().next() {
        Some("footprint") => {
            words.next();
            if facing.is_some() {
                return Err(TableError::Line {
                    at,
                    detail: "only a `none` verdict may carry a footprint",
                });
            }
            let mut span = || number::<u8>(words, at, "a footprint is `x0 x1 y0 y1`");
            let (x0, x1, y0, y1) = (span()?, span()?, span()?, span()?);
            Some(
                Footprint::new(Span::new(x0, x1), Span::new(y0, y1)).ok_or(TableError::Line {
                    at,
                    detail: "a footprint's spans must be non-empty and at most 8",
                })?,
            )
        }
        _ => None,
    };
    let authored = match words.clone().next() {
        Some("authored") => {
            words.next();
            true
        }
        _ => false,
    };
    Ok((
        Graphic(graphic),
        Row {
            shape: Shape {
                facing,
                hole,
                prism,
                blocks,
                footprint,
            },
            authored,
        },
    ))
}

/// Why a table would not parse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableError {
    /// Written by a build that speaks a different version of this format.
    Format {
        /// What the file says.
        found: u32,
    },
    /// No `table` line at all, which is what any other text file looks like.
    NoFormat,
    /// An `art` line with no `detector` beside it or the other way round.
    HalfStamped,
    /// A line this could not read, and what was expected on it.
    Line {
        /// Which line, counting from one.
        at:     usize,
        /// What was wanted there.
        detail: &'static str,
    },
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format { found } => {
                write!(f, "the table is format {found} and this build reads {FORMAT}")
            }
            Self::NoFormat => write!(f, "no `table` line: this is not an art table"),
            Self::HalfStamped => write!(f, "`art` and `detector` are one stamp and want each other"),
            Self::Line { at, detail } => write!(f, "line {at}: {detail}"),
        }
    }
}

impl std::error::Error for TableError {
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp {
            art:      "artLegacyMUL.uop".to_string(),
            bytes:    447_160_596,
            detector: crate::facing::DETECTOR,
        }
    }

    /// The shape a plain wall has: one face and no hole.
    fn faced(face: Face) -> Shape {
        Shape::faced(Facing::One(face))
    }

    /// Every verdict survives the round trip, including the two a corner is made
    /// of, the hole step 16 measures, and the hand-written refusal.
    ///
    /// The property is that the *whole table* comes back equal, which is what
    /// makes a re-derivation a diff of what changed rather than a diff of how it
    /// was written down.
    #[test]
    fn a_table_reads_back_as_the_table_it_was_written_from() {
        let mut table = ArtTable::measured(stamp());
        table.derive(Graphic(0x0007), faced(Face::South));
        table.derive(Graphic(0x0100), faced(Face::East));
        // A window: the same face, with the rectangle the art left out of it.
        table.derive(
            Graphic(0x003C),
            Shape {
                facing:    Some(Facing::One(Face::East)),
                hole:      Some(WINDOW),
                prism:     None,
                blocks:    Blocks::EMPTY,
                footprint: None,
            },
        );
        table.derive(
            Graphic(0x0104),
            Shape::faced(Facing::Corner {
                right: Face::East,
                left:  Face::South,
            }),
        );
        table.derive(Graphic(0x0009), Shape::UNREAD);
        table.author(Graphic(0x0171), Shape::UNREAD);
        table.author(Graphic(0x02D8), faced(Face::West));

        let read = ArtTable::parse(&table.to_text()).expect("its own text");
        assert_eq!(read, table);
        assert_eq!(read.examined(), 5, "the five derived, not the two authored");
        assert_eq!(read.decided(), 5, "four derived faces and one authored");
        assert_eq!(read.authored(), 2);
        assert_eq!(read.corners(), 1);
        assert_eq!(read.holed(), 1);
        assert_eq!(read.shape(Graphic(0x003C)).hole, Some(WINDOW));
        assert!(read.fresh(&stamp()));
    }

    /// `0x003C`'s own hole, as the detector reads it off a real install: the
    /// middle third of the tile, from ten `z` above the sill to fifteen.
    const WINDOW: Hole = Hole {
        near:   93,
        far:    185,
        bottom: 10,
        top:    15,
    };

    /// A graphic the detector refused has no row, and that is how a reader tells
    /// it from one nobody looked at — with [`ArtTable::examined`] beside it.
    #[test]
    fn a_refused_graphic_is_an_absent_row() {
        let mut table = ArtTable::measured(stamp());
        table.derive(Graphic(0x0009), Shape::UNREAD);
        assert_eq!(table.facing(Graphic(0x0009)), None);
        assert!(
            !table.to_text().contains("0x0009"),
            "a derived refusal is not written down:\n{}",
            table.to_text()
        );
        assert_eq!(table.examined(), 1, "it was still measured");
    }

    /// **Decision 31.2**: re-deriving leaves an authored row alone, whichever way
    /// the two disagree.
    ///
    /// Both directions, because the mechanism is a `get` and a `return` and a
    /// version of it that only protected the `Some` case would look right in a
    /// test of one: a person saying "read nothing off this picture" is exactly
    /// the override a detector reading it wrongly needs.
    #[test]
    fn a_rederivation_leaves_an_authored_row_alone() {
        let mut table = ArtTable::measured(stamp());
        table.author(Graphic(0x02D8), faced(Face::West));
        table.author(Graphic(0x0171), Shape::UNREAD);

        table.derive(Graphic(0x02D8), faced(Face::East));
        table.derive(Graphic(0x0171), faced(Face::North));

        assert_eq!(table.facing(Graphic(0x02D8)), Some(Facing::One(Face::West)));
        assert_eq!(table.facing(Graphic(0x0171)), None);
        assert_eq!(table.authored(), 2, "and they are still a person's rows");
    }

    /// An override sheet is a table with rows and no install behind it, and its
    /// authored rows travel into a measured one.
    ///
    /// This is the shape the repository ships overrides in: one grammar, one
    /// parser, and a merge that is a filter on a flag rather than a second file
    /// format with a precedence rule to argue about.
    #[test]
    fn an_override_sheet_hands_its_rows_to_a_measured_table() {
        let sheet = ArtTable::parse("table 5\n0x02D8 face W authored\n0x0100 face N\n")
            .expect("a sheet of overrides");
        assert!(sheet.stamp().is_none());
        assert!(!sheet.fresh(&stamp()), "a sheet describes no install");

        let mut table = ArtTable::measured(stamp());
        table.derive(Graphic(0x02D8), faced(Face::East));
        table.derive(Graphic(0x0100), faced(Face::East));
        table.adopt_authored(&sheet);

        assert_eq!(table.facing(Graphic(0x02D8)), Some(Facing::One(Face::West)));
        assert_eq!(
            table.facing(Graphic(0x0100)),
            Some(Facing::One(Face::East)),
            "an unmarked row in a sheet is not an override",
        );
    }

    /// A table from a different install, or read by different rules, is not
    /// fresh — and neither half of the stamp may be the only one that matters.
    #[test]
    fn a_stamp_that_differs_in_either_half_is_stale() {
        let table = ArtTable::measured(stamp());
        assert!(table.fresh(&stamp()));
        let other_art = Stamp { bytes: 12, ..stamp() };
        let other_rules = Stamp {
            detector: crate::facing::DETECTOR + 1,
            ..stamp()
        };
        assert!(!table.fresh(&other_art), "a different install");
        assert!(!table.fresh(&other_rules), "different rules");
    }

    /// A table this build does not speak is refused rather than half-read.
    #[test]
    fn a_table_from_another_format_is_refused() {
        assert_eq!(
            ArtTable::parse("table 99\n0x0007 face S\n"),
            Err(TableError::Format { found: 99 })
        );
        assert_eq!(ArtTable::parse("0x0007 face S\n"), Err(TableError::NoFormat));
        assert_eq!(
            ArtTable::parse("table 5\nart artLegacyMUL.uop 12\n"),
            Err(TableError::HalfStamped)
        );
        // And the formats this one replaced, which is the case the bump is for: a
        // table of faces measured before holes existed reads every window as
        // solid stone and says nothing about it, one measured before prisms
        // existed reads every staircase as a corner of two walls, one measured
        // before blocks existed silently drops every arch a person had authored
        // by hand, and one measured before footprints existed answers
        // `footprint: None` for every bookcase drawn narrower than its tile.
        assert_eq!(
            ArtTable::parse("table 1\n0x0007 face S\n"),
            Err(TableError::Format { found: 1 })
        );
        assert_eq!(
            ArtTable::parse("table 2\n0x0104 corner E S\n"),
            Err(TableError::Format { found: 2 })
        );
        assert_eq!(
            ArtTable::parse("table 3\n0x0166 corner E S prism W 1 3 5\n"),
            Err(TableError::Format { found: 3 })
        );
        assert_eq!(
            ArtTable::parse(
                "table 4\n0x0736 none block 0 2 0 8 0 20 block 6 8 0 8 0 20 block 0 8 0 8 15 20 authored\n"
            ),
            Err(TableError::Format { found: 4 })
        );
    }

    /// And a row that says nothing readable says which line it is on.
    ///
    /// Each of these is a plausible hand-edit: a face that is not one of the
    /// four, a corner with its halves the wrong way round — which would light a
    /// wall from inside the house — a verdict nobody defined, and a hole with a
    /// number missing.
    #[test]
    fn an_unreadable_row_names_its_line() {
        for text in [
            "table 5\n0x0007 face Q\n",
            "table 5\n0x0007 corner S E\n",
            "table 5\n0x0007 wall\n",
            "table 5\nnotahex face S\n",
            "table 5\n0x0007 face S and more\n",
            "table 5\n0x0007 face S hole 93 185 10\n",
            "table 5\n0x0007 face S hole 93 185 10 300\n",
        ] {
            let error = ArtTable::parse(text).expect_err(text);
            assert!(matches!(error, TableError::Line { at: 2, .. }), "{text}: {error}");
        }
    }

    /// A hole belongs to a face, and a corner may not carry one.
    ///
    /// The refusal `aperture_of` makes at the other end, stated in the grammar so
    /// that a hand-written row cannot say what the detector will not: a corner is
    /// two surfaces in one picture, and a hole given to both is a window in the
    /// wall beside the window.
    #[test]
    fn only_a_face_may_carry_a_hole() {
        assert!(matches!(
            ArtTable::parse("table 5\n0x0104 corner E S hole 93 185 10 15\n"),
            Err(TableError::Line { at: 2, .. })
        ));
        assert!(matches!(
            ArtTable::parse("table 5\n0x0104 none hole 93 185 10 15\n"),
            Err(TableError::Line { at: 2, .. })
        ));
        let table = ArtTable::parse("table 5\n0x003C face E hole 93 185 10 15 authored\n").expect("a row");
        assert_eq!(table.shape(Graphic(0x003C)).hole, Some(WINDOW));
        assert_eq!(table.authored(), 1, "the marker after the hole is still read");
    }

    /// **A staircase survives the file.**
    ///
    /// The measurement this table exists to make once: a flight of steps reads as
    /// a corner of two walls *and* as a solid, and it is the solid that is true.
    /// Before format 3 the corner was all a row could hold, so a table turned
    /// every stair back into a wall — which is what makes this the round trip
    /// worth its own test rather than a line in the one above.
    #[test]
    fn a_prism_survives_the_round_trip_beside_the_corner_it_was_read_from() {
        let stair = Prism::new(Face::West, &[1, 3, 5]).expect("three treads");
        let mut table = ArtTable::measured(stamp());
        table.derive(
            Graphic(0x0166),
            Shape {
                facing:    Some(Facing::Corner {
                    right: Face::East,
                    left:  Face::South,
                }),
                hole:      None,
                prism:     Some(stair),
                blocks:    Blocks::EMPTY,
                footprint: None,
            },
        );
        // And a person's own: a solid with no facing at all, which is what
        // `Shape::solid` is and what no detector produces.
        table.author(Graphic(0x071E), Shape::solid(Prism::box_of(5)));

        let read = ArtTable::parse(&table.to_text()).expect("its own text");
        assert_eq!(read, table);
        assert_eq!(read.prisms(), 2);
        assert_eq!(read.shape(Graphic(0x0166)).prism, Some(stair));
        assert_eq!(
            read.facing(Graphic(0x0166)),
            Some(Facing::Corner {
                right: Face::East,
                left:  Face::South,
            }),
            "the corner is still there: which of the two is believed is the grid's call",
        );
        assert_eq!(read.shape(Graphic(0x071E)).prism, Some(Prism::box_of(5)));
        assert_eq!(
            read.shape(Graphic(0x071E)).facing,
            None,
            "a solid a person wrote down states no facing, and the row is still kept",
        );
    }

    /// A prism belongs to a corner or to nothing, and a `face` may not carry one.
    ///
    /// The refusal `Shape::of` makes at the other end — it scores prisms only
    /// against a picture the wall detector called a corner — stated in the grammar
    /// so a hand-written row cannot say what no detector will. With the profile's
    /// own bounds beside it: `Prism::new` refuses an empty profile and one longer
    /// than `MAX_TREADS`, and a row is exactly the place a hand-edit would try.
    #[test]
    fn a_prism_row_states_only_what_a_detector_would() {
        for text in [
            "table 5\n0x0007 face S prism W 1 3 5\n",
            "table 5\n0x0104 corner E S prism Q 1\n",
            "table 5\n0x0104 corner E S prism W\n",
            "table 5\n0x0104 corner E S prism W 1 2 3 4 5\n",
            "table 5\n0x0104 corner E S prism W 300\n",
        ] {
            let error = ArtTable::parse(text).expect_err(text);
            assert!(matches!(error, TableError::Line { at: 2, .. }), "{text}: {error}");
        }
        let table = ArtTable::parse("table 5\n0x0104 none prism W 1 3 5 authored\n").expect("a row");
        assert_eq!(
            table.shape(Graphic(0x0104)).prism,
            Prism::new(Face::West, &[1, 3, 5]),
        );
        assert_eq!(table.authored(), 1, "the marker after the treads is still read");
    }

    /// **An arch survives the file beside its prism**, decision 41: a shape a
    /// single climb profile cannot describe, in three boxes rather than one,
    /// and a row may hold both a prism and a block list at once — nothing here
    /// forces a choice between them.
    #[test]
    fn a_block_list_survives_the_round_trip_beside_a_prism() {
        let post = |x: Span| Block::new(x, Span::new(0, 8), Span::new(0, 20)).expect("a post, full depth");
        let lintel = Block::new(Span::new(0, 8), Span::new(0, 8), Span::new(15, 20))
            .expect("the beam over both posts");
        let arch =
            Blocks::new(&[post(Span::new(0, 2)), post(Span::new(6, 8)), lintel]).expect("three of four");
        let mut table = ArtTable::measured(stamp());
        table.author(Graphic(0x0736), Shape::pieced(arch));
        // And a graphic whose derived verdict is a plain wall, with a
        // hand-authored block on top of it — nothing here refuses the pairing,
        // per the module doc's own argument that a block is not a claim about
        // which edge a plane stands on.
        table.derive(Graphic(0x0007), faced(Face::South));
        table.author(
            Graphic(0x0007),
            Shape {
                blocks: Blocks::new(&[post(Span::new(3, 5))]).expect("one block"),
                ..faced(Face::South)
            },
        );

        let read = ArtTable::parse(&table.to_text()).expect("its own text");
        assert_eq!(read, table);
        assert_eq!(read.shape(Graphic(0x0736)).blocks, arch);
        assert_eq!(
            read.shape(Graphic(0x0736)).facing,
            None,
            "the arch states no facing of its own",
        );
        assert_eq!(
            read.shape(Graphic(0x0007)).facing,
            Some(Facing::One(Face::South)),
            "a face may carry a block: nothing about it is a claim on the plane's edge",
        );
        assert!(
            read.shape(Graphic(0x0009)).blocks.is_empty(),
            "a graphic nobody authored has no blocks",
        );
    }

    /// A row may carry no more than [`crate::facing::MAX_BLOCKS`], and a block
    /// with an empty or an out-of-range span is refused — the same discipline
    /// [`Prism::new`] holds for its own treads, and a hand-edit is exactly the
    /// place either bound would be tried.
    #[test]
    fn a_block_states_a_non_empty_span_and_a_row_states_at_most_max_blocks() {
        for text in [
            "table 5\n0x0007 none block 4 4 0 8 0 20\n",
            "table 5\n0x0007 none block 0 4 4 4 0 20\n",
            "table 5\n0x0007 none block 0 4 0 8 5 5\n",
            "table 5\n0x0007 none block 0 9 0 8 0 20\n",
            "table 5\n0x0007 none block 0 8 0 9 0 20\n",
            "table 5\n0x0007 none block 0 1 0 1 0 1 block 1 2 0 1 0 1 \
             block 2 3 0 1 0 1 block 3 4 0 1 0 1 block 4 5 0 1 0 1\n",
        ] {
            let error = ArtTable::parse(text).expect_err(text);
            assert!(matches!(error, TableError::Line { at: 2, .. }), "{text}: {error}");
        }
    }

    /// **A bookcase survives the file**, `docs/render/design_footprints.md`'s S1 and S2: a
    /// picture the wall detector named no edge for, with the box its own base
    /// edge states written down beside the `none` verdict.
    #[test]
    fn a_footprint_survives_the_round_trip_on_a_none_verdict() {
        let footprint = Footprint::new(Span::new(0, 8), Span::new(3, 8)).expect("a box narrower on one axis");
        let mut table = ArtTable::measured(stamp());
        table.derive(
            Graphic(0x0A97),
            Shape {
                footprint: Some(footprint),
                ..Shape::UNREAD
            },
        );
        table.derive(Graphic(0x0007), faced(Face::South));

        let read = ArtTable::parse(&table.to_text()).expect("its own text");
        assert_eq!(read, table);
        assert_eq!(read.footprints(), 1);
        assert_eq!(read.shape(Graphic(0x0A97)).footprint, Some(footprint));
        assert_eq!(
            read.shape(Graphic(0x0A97)).facing,
            None,
            "a footprint states no facing of its own",
        );
        assert!(
            read.shape(Graphic(0x0007)).footprint.is_none(),
            "a plain wall was never offered one",
        );
    }

    /// A footprint belongs to a `none` verdict, and a face or a corner may not
    /// carry one — `docs/render/design_footprints.md`'s D4, the mirror of the hole's and the
    /// prism's own restrictions stated in the grammar so a hand-written row
    /// cannot say what no detector will.
    #[test]
    fn a_footprint_row_states_only_what_a_detector_would() {
        for text in [
            "table 5\n0x0007 face S footprint 0 8 3 8\n",
            "table 5\n0x0104 corner E S footprint 0 8 3 8\n",
        ] {
            let error = ArtTable::parse(text).expect_err(text);
            assert!(matches!(error, TableError::Line { at: 2, .. }), "{text}: {error}");
        }
        let table = ArtTable::parse("table 5\n0x0A97 none footprint 0 8 3 8 authored\n").expect("a row");
        assert_eq!(
            table.shape(Graphic(0x0A97)).footprint,
            Footprint::new(Span::new(0, 8), Span::new(3, 8)),
        );
        assert_eq!(table.authored(), 1, "the marker after the spans is still read");
    }

    /// A footprint states a non-empty span on each axis, at most a whole tile —
    /// the same discipline [`Block::new`] holds for its own spans, and a
    /// hand-edit is exactly the place either bound would be tried.
    #[test]
    fn a_footprint_states_a_non_empty_span_at_most_a_tile() {
        for text in [
            "table 5\n0x0007 none footprint 4 4 0 8\n",
            "table 5\n0x0007 none footprint 0 4 4 4\n",
            "table 5\n0x0007 none footprint 0 9 0 8\n",
            "table 5\n0x0007 none footprint 0 8 0 9\n",
        ] {
            let error = ArtTable::parse(text).expect_err(text);
            assert!(matches!(error, TableError::Line { at: 2, .. }), "{text}: {error}");
        }
    }

    /// Comments and blank lines are a person's, and the parser keeps its hands
    /// off them.
    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let table =
            ArtTable::parse("# what this is\ntable 5\n\n0x0007 face S  # the south face of a marble wall\n")
                .expect("a commented sheet");
        assert_eq!(table.facing(Graphic(0x0007)), Some(Facing::One(Face::South)));
    }
}
