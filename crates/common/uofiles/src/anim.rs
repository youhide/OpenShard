//! `anim.idx` and `anim.mul`: the frames a body is drawn from.
//!
//! Every mobile in UO is a body id, a group (standing, walking, attacking), a
//! facing, and a frame within that. The file is indexed by exactly that tuple —
//! flattened, in that order — and each entry holds a 256-colour palette and the
//! run-length encoded frames that use it.
//!
//! # The index is arithmetic, not a table
//!
//! There is no lookup: which block of `anim.idx` a body's frames live in is
//! computed from the body id, and the constant depends on which of three kinds
//! it is. A monster gets 22 groups, an animal 13 and a human 35, each with five
//! stored directions — so the blocks per body are 110, 65 and 175, and the
//! bases are 0, 22,000 and 35,000. `AnimationsLoader.CalculateOffset` is where
//! those numbers come from, and they are not derivable from anything in the
//! file: an index computed with the wrong constant reads real frames belonging
//! to another creature, which draws something plausible and wrong.
//!
//! # Five directions, eight facings
//!
//! Only five are stored. The other three are the mirror images of 1, 2 and 3 —
//! see [`facing`] — which is also why a mobile's sprite is placed from its
//! *far* edge when it is flipped.
//!
//! # This one is not read into memory
//!
//! `anim.mul` is 195MB on a modern install, and a renderer wants a handful of
//! bodies out of it. So the index is held — it is 1.7MB and every lookup needs
//! it — and the frames are read from the file on demand, which is why
//! [`Anim::frames`] takes `&mut self`. It is the first reader here that does
//! not slurp its container, and the browser is the reason the rest will follow.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openshard_protocol::direction::Direction;

use crate::color::Color16;
use crate::image::Image;

/// Bytes per `anim.idx` entry: offset, length, extra.
const IDX_ENTRY: usize = 12;
/// Colours in a body's palette.
const PALETTE_COLORS: usize = 256;
/// Bytes the palette occupies at the head of an entry.
const PALETTE_BYTES: usize = PALETTE_COLORS * 2;
/// Stored directions per group. The other three facings are mirrors.
pub const DIRECTIONS: u8 = 5;
/// The run header that ends a frame's pixel data.
const END_OF_FRAME: u32 = 0x7FFF_7FFF;
/// An index entry the file uses to say "nothing here".
const NO_ENTRY: u32 = 0xFFFF_FFFF;

/// What can go wrong reading an animation.
#[derive(Debug)]
#[non_exhaustive]
pub enum AnimError {
    /// A file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// `anim.idx` is not a whole number of entries.
    NotAnIndex {
        /// Which file.
        path: PathBuf,
        /// How big it is.
        size: u64,
    },
    /// An entry's bytes are not a body's frames.
    Malformed {
        /// Which body.
        body: u16,
        /// Which group.
        group: u8,
        /// Which stored direction.
        direction: u8,
        /// What was wrong.
        detail: String,
    },
}

impl fmt::Display for AnimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotAnIndex { path, size } => write!(
                f,
                "{} is {size} bytes, which is not a whole number of {IDX_ENTRY}-byte entries; \
                 it is not anim.idx",
                path.display(),
            ),
            Self::Malformed {
                body,
                group,
                direction,
                detail,
            } => write!(f, "body {body} group {group} direction {direction}: {detail}"),
        }
    }
}

impl std::error::Error for AnimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::NotAnIndex { .. } | Self::Malformed { .. } => None,
        }
    }
}

/// Which of the three shapes of the index a body uses.
///
/// The kinds are the client's and they are decided by the body id alone —
/// `AnimationsLoader.GetAnimType` for the file this reader opens. They are not
/// a taxonomy: a "monster" here means "22 groups, based at zero", and a body
/// that looks like a horse is an animal because of its number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyKind {
    /// Bodies below 200: 22 groups.
    Monster,
    /// Bodies 200 to 399: 13 groups.
    Animal,
    /// Bodies 400 and up: 35 groups, which is where players live.
    Human,
}

impl BodyKind {
    /// Which kind a body id is.
    pub const fn of(body: u16) -> Self {
        if body < 200 {
            Self::Monster
        } else if body < 400 {
            Self::Animal
        } else {
            Self::Human
        }
    }

    /// How many animation groups this kind has.
    pub const fn groups(self) -> u8 {
        match self {
            Self::Monster => 22,
            Self::Animal => 13,
            Self::Human => 35,
        }
    }

    /// Which group means "standing still" for this kind.
    ///
    /// The three group *numberings* are not the same list with different
    /// lengths: they are three enumerations, and the same number means three
    /// different actions. `HighAnimationGroup.Stand` is 1, `LowAnimationGroup`'s
    /// is 2, `PeopleAnimationGroup`'s is 4 — so a client that stands everything
    /// in group 4 stands a player, plays a *jab* on a monster
    /// (`HighAnimationGroup.Attack1`) and *feeds* a horse
    /// (`LowAnimationGroup.Unknown`, one past `Eat`). All three exist, so
    /// nothing fails; it just plays the wrong picture forever.
    pub const fn standing(self) -> u8 {
        match self {
            Self::Monster => 1,
            Self::Animal => 2,
            Self::Human => 4,
        }
    }

    /// Which group means "walking".
    ///
    /// Zero in all three, which is the one coincidence between them — and the
    /// reason it is written out anyway: a caller reading `walking()` beside
    /// [`BodyKind::standing`] cannot conclude that the numbering is shared.
    /// A human's is `WalkUnarmed`; what a weapon does to it is M4's problem,
    /// since nothing here knows what anyone is holding.
    pub const fn walking(self) -> u8 {
        match self {
            Self::Monster | Self::Animal | Self::Human => 0,
        }
    }

    /// Which group means "running".
    ///
    /// `None` for a monster: `HighAnimationGroup` has no run at all — it goes
    /// `Walk`, `Stand`, `Die1` — so a running monster keeps walking, which is
    /// what the client draws too. An animal's is 1 and a human's is 2
    /// (`RunUnarmed`).
    pub const fn running(self) -> Option<u8> {
        match self {
            Self::Monster => None,
            Self::Animal => Some(1),
            Self::Human => Some(2),
        }
    }

    /// The first index block belonging to `body`.
    ///
    /// The three cases of `AnimationsLoader.CalculateOffset`, in blocks rather
    /// than bytes.
    const fn base(self, body: u16) -> usize {
        match self {
            Self::Monster => body as usize * 110,
            Self::Animal => 22_000 + (body as usize - 200) * 65,
            Self::Human => 35_000 + (body as usize - 400) * 175,
        }
    }
}

/// The body an animation is *read* under, which is not always the body the
/// shard named.
///
/// `Mobile.GetGraphicForAnimation`. A handful of body ids exist on the wire and
/// nowhere in `anim.mul`, and the client quietly reads a different one for them.
/// The ghosts are the pair that matters here: a dead player is `0x0192` or
/// `0x0193` — 402 and 403, male and female — and the index has no block for
/// either, so a client that asks for what it was told draws *nothing at all*.
/// The living body two below it is the picture; what makes it a ghost is the
/// translucency the drawing decides, not the animation.
///
/// Silent in both directions, which is why it is a named function on the file
/// reader rather than a `match` at a call site: a missing index block is `None`
/// from [`Anim::frames`] and an absent mobile on screen, with nothing logged and
/// nothing failing. It was found by a player dying and disappearing.
/// Whether a body is a dead player's.
///
/// The same two ids [`animation_body`] remaps, asked as the question the drawing
/// has: a ghost is not a different animation, it is the living body drawn
/// differently — translucent, and with neither hair nor beard on it. Here rather
/// than at the renderer that asks, because it is a fact about the client's body
/// ids and the pair must not be written down twice.
pub const fn is_ghost(body: u16) -> bool {
    matches!(body, 0x0192 | 0x0193)
}

/// Whether a body is a female one.
///
/// `Mobile.CheckGraphicChange` in the reference client, which is the *only*
/// place a client decides this: nothing on the wire says a mobile's sex — not
/// `0x78`, not `0x88` — and what the drawing needs it for is which of two
/// pictures to reach for. Four ids, the female half of each playable race and
/// the female ghost among them, because a ghost's paperdoll is still hers.
///
/// Here rather than in the renderer that asks, for [`is_ghost`]'s reason: it is
/// a fact about the client's body ids, and a body id table written down twice
/// is a body id table that disagrees with itself.
pub const fn is_female(body: u16) -> bool {
    matches!(body, 0x0191 | 0x0193 | 0x025E | 0x029B)
}

/// Whether a body is a gargoyle one, living or dead.
///
/// Asked by exactly one thing — the paperdoll's layer ordering, which puts a
/// gargoyle's torso where a female body's goes
/// (`PaperDollInteractable.IsGargoyleBody`) — and it is the four ids rather
/// than the two, because `0x02B6`/`0x02B7` are the dead pair of `0x029A`/
/// `0x029B` and a corpse is drawn on a paperdoll like anything else.
pub const fn is_gargoyle(body: u16) -> bool {
    matches!(body, 0x029A | 0x029B | 0x02B6 | 0x02B7)
}

pub const fn animation_body(body: u16) -> u16 {
    match body {
        // The ghosts, drawn from the living body of the same sex.
        0x0192 | 0x0193 => body - 2,
        // The other two the client remaps, kept because the reason is the same
        // — an id with no block of its own — and because leaving half a port
        // behind is how the next one gets re-derived.
        0x02B6 => 667,
        0x02B7 => 666,
        _ => body,
    }
}

/// The stored direction a facing is drawn from, and whether it is mirrored.
///
/// `Animation.GetAnimDirection`. Facing 3 is the one stored as direction 0, and
/// the four facings on the other side of it reuse 1, 2 and 3 flipped — which is
/// why a flipped sprite is placed from `width - center_x` rather than from
/// `center_x`.
pub const fn facing(facing: Direction) -> (u8, bool) {
    match facing {
        Direction::North => (3, true),
        Direction::NorthEast => (2, true),
        Direction::East => (1, true),
        Direction::SouthEast => (0, false),
        Direction::South => (1, false),
        Direction::SouthWest => (2, false),
        Direction::West => (3, false),
        Direction::NorthWest => (4, false),
    }
}

/// One frame: a picture and where the body's feet are in it.
///
/// The centre is not the middle of the picture. It is the offset from the
/// tile's own screen position to the sprite's, and it is what makes a mobile
/// stand on the ground rather than float above it — a walking frame leans, and
/// the lean lives in this pair rather than in the pixels.
#[derive(Clone, PartialEq, Debug)]
pub struct AnimFrame {
    /// Horizontal offset from the tile's centre to the sprite's left edge.
    pub center_x: i16,
    /// Vertical offset, measured up from the sprite's bottom edge.
    pub center_y: i16,
    /// The picture itself, with everything no run covered transparent.
    pub image: Image,
}

/// One entry of `anim.idx`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct IdxEntry {
    position: u32,
    size: u32,
}

impl IdxEntry {
    /// Whether this entry names any data at all.
    ///
    /// The file says "nothing here" in three different ways — all zeroes, all
    /// ones, or a zero length — and the client accepts all three
    /// (`ReadMULAnimationFrames`). A reader that only checked one of them
    /// would seek to 0xFFFFFFFF on a body the client draws as nothing.
    const fn is_present(self) -> bool {
        self.position != NO_ENTRY && self.size != NO_ENTRY && self.size != 0
    }
}

/// The client's animations, indexed and ready to read from.
#[derive(Debug)]
pub struct Anim {
    entries: Vec<IdxEntry>,
    mul: File,
    mul_path: PathBuf,
    mul_len: u64,
}

impl Anim {
    /// Open the `anim.idx`/`anim.mul` pair in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, AnimError> {
        let dir = client_dir.as_ref();
        Self::from_files(dir.join("anim.idx"), dir.join("anim.mul"))
    }

    /// Open a named pair, for the other four `animN` files and for tests.
    ///
    /// Only the first file's index arithmetic is implemented — see
    /// [`BodyKind`] — so the others are openable and not yet addressable. That
    /// is deliberate: `anim2` through `anim5` re-base the same three kinds
    /// differently, and guessing which without a body that needs one would put
    /// an unverified constant in the one place that must be right.
    pub fn from_files(idx: impl AsRef<Path>, mul: impl AsRef<Path>) -> Result<Self, AnimError> {
        let idx_path = idx.as_ref();
        let raw = std::fs::read(idx_path).map_err(|source| AnimError::Read {
            path: idx_path.to_owned(),
            source,
        })?;
        if raw.is_empty() || !raw.len().is_multiple_of(IDX_ENTRY) {
            return Err(AnimError::NotAnIndex {
                path: idx_path.to_owned(),
                size: raw.len() as u64,
            });
        }
        let entries = raw
            .chunks_exact(IDX_ENTRY)
            .map(|entry| IdxEntry {
                position: u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]),
                size: u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]),
            })
            .collect();

        let mul_path = mul.as_ref().to_owned();
        let file = File::open(&mul_path).map_err(|source| AnimError::Read {
            path: mul_path.clone(),
            source,
        })?;
        let mul_len = file
            .metadata()
            .map_err(|source| AnimError::Read {
                path: mul_path.clone(),
                source,
            })?
            .len();

        Ok(Self {
            entries,
            mul: file,
            mul_path,
            mul_len,
        })
    }

    /// How many index entries the file holds. Most of them are empty.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Whether a body, group and stored direction has any frames at all.
    ///
    /// Cheap — it reads the index only — and worth having separately: most of
    /// the index is empty, and "does this body exist" is a question a caller
    /// asks far more often than it asks for pixels.
    pub fn has_frames(&self, body: u16, group: u8, direction: u8) -> bool {
        self.entry(body, group, direction)
            .is_some_and(IdxEntry::is_present)
    }

    /// The frames of one body, group and *stored* direction.
    ///
    /// `direction` is 0 to 4, which is what the file holds — pass a wire facing
    /// through [`facing`] first, and mirror the sprite yourself if it says so.
    /// Drawing is not this crate's business and the flip belongs where the
    /// quad is built.
    ///
    /// `None` means the client ships no animation there, which is the ordinary
    /// answer for most of the index: a group a body does not have, or a body id
    /// nothing uses.
    pub fn frames(
        &mut self,
        body: u16,
        group: u8,
        direction: u8,
    ) -> Result<Option<Vec<AnimFrame>>, AnimError> {
        let Some(entry) = self.entry(body, group, direction) else {
            return Ok(None);
        };
        if !entry.is_present() {
            return Ok(None);
        }
        // An entry pointing outside the file is the file's problem and not a
        // reason to read somebody else's bytes: the client drops it too.
        if u64::from(entry.position) + u64::from(entry.size) > self.mul_len {
            return Ok(None);
        }

        let mut raw = vec![0u8; entry.size as usize];
        self.mul
            .seek(SeekFrom::Start(u64::from(entry.position)))
            .and_then(|_| self.mul.read_exact(&mut raw))
            .map_err(|source| AnimError::Read {
                path: self.mul_path.clone(),
                source,
            })?;

        decode_body(body, group, direction, &raw).map(Some)
    }

    /// The index entry for a body, group and stored direction.
    fn entry(&self, body: u16, group: u8, direction: u8) -> Option<IdxEntry> {
        let kind = BodyKind::of(body);
        if group >= kind.groups() || direction >= DIRECTIONS {
            return None;
        }
        let block = kind.base(body) + usize::from(group) * usize::from(DIRECTIONS) + usize::from(direction);
        self.entries.get(block).copied()
    }
}

/// Decode one index entry: a palette, then the frames that use it.
///
/// Split out so the format can be exercised on bytes built by hand — the
/// failures worth catching are an offset past the end and a run that draws
/// outside its own frame, and a shipped file offers neither on demand.
fn decode_body(body: u16, group: u8, direction: u8, raw: &[u8]) -> Result<Vec<AnimFrame>, AnimError> {
    let malformed = |detail: String| AnimError::Malformed {
        body,
        group,
        direction,
        detail,
    };

    let palette_bytes = raw
        .get(..PALETTE_BYTES)
        .ok_or_else(|| malformed("shorter than its own palette".to_owned()))?;
    let mut palette = [Color16::TRANSPARENT; PALETTE_COLORS];
    for (slot, word) in palette.iter_mut().zip(palette_bytes.chunks_exact(2)) {
        *slot = Color16(u16::from_le_bytes([word[0], word[1]]));
    }

    // Every frame offset is counted from here, not from the start of the entry.
    let body_data = &raw[PALETTE_BYTES..];
    let count = u32::from_le_bytes(
        body_data
            .get(..4)
            .ok_or_else(|| malformed("no frame count".to_owned()))?
            .try_into()
            .expect("four bytes"),
    ) as usize;
    // A frame count large enough to overflow the allocation below is a
    // corrupt entry rather than a body with a lot of frames: the longest
    // animation a client ships is well under a hundred.
    let offsets = body_data
        .get(4..4 + count * 4)
        .ok_or_else(|| malformed(format!("claims {count} frames it has no offsets for")))?;

    let mut frames = Vec::with_capacity(count);
    for offset in offsets.chunks_exact(4) {
        let at = u32::from_le_bytes([offset[0], offset[1], offset[2], offset[3]]) as usize;
        let frame = body_data
            .get(at..)
            .ok_or_else(|| malformed(format!("a frame starts at {at}, past the end")))?;
        frames.push(decode_frame(frame, &palette, &malformed)?);
    }
    Ok(frames)
}

/// Decode one frame: a header, then runs until the terminator.
fn decode_frame(
    raw: &[u8],
    palette: &[Color16; PALETTE_COLORS],
    malformed: &impl Fn(String) -> AnimError,
) -> Result<AnimFrame, AnimError> {
    let header = raw
        .get(..8)
        .ok_or_else(|| malformed("a frame shorter than its header".to_owned()))?;
    let center_x = i16::from_le_bytes([header[0], header[1]]);
    let center_y = i16::from_le_bytes([header[2], header[3]]);
    let width = i16::from_le_bytes([header[4], header[5]]);
    let height = i16::from_le_bytes([header[6], header[7]]);
    if width <= 0 || height <= 0 {
        // The client returns an empty frame here rather than failing, and so
        // does this: an animation with a blank frame in it is a real thing.
        return Ok(AnimFrame {
            center_x,
            center_y,
            image: Image::new(0, 0, Vec::new()),
        });
    }
    let (width, height) = (width as u16, height as u16);

    let mut pixels = vec![Color16::TRANSPARENT; usize::from(width) * usize::from(height)];
    let mut at = 8;
    loop {
        let word = raw
            .get(at..at + 4)
            .ok_or_else(|| malformed("runs off the end without a terminator".to_owned()))?;
        let header = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        at += 4;
        if header == END_OF_FRAME {
            break;
        }

        // Three fields in one word: a length, and two signed ten-bit offsets
        // that are *relative to the centre*. Sign-extending them is what makes
        // a frame's pixels land around the body rather than in the corner.
        let run = (header & 0x0FFF) as usize;
        let x = i32::from(sign_extend_10(((header >> 22) & 0x03FF) as u16)) + i32::from(center_x);
        let y = i32::from(sign_extend_10(((header >> 12) & 0x03FF) as u16))
            + i32::from(center_y)
            + i32::from(height);

        let span = raw
            .get(at..at + run)
            .ok_or_else(|| malformed(format!("a run claims {run} pixels it does not have")))?;
        at += run;

        // A run outside the frame is refused rather than clamped. The client
        // would write past the buffer here; we would silently draw a body's
        // arm wrapped onto the row above it, which looks like art and is not.
        if y < 0 || y >= i32::from(height) || x < 0 || x + run as i32 > i32::from(width) {
            return Err(malformed(format!(
                "a run of {run} at ({x}, {y}) leaves a {width}x{height} frame",
            )));
        }

        let start = y as usize * usize::from(width) + x as usize;
        for (offset, index) in span.iter().enumerate() {
            pixels[start + offset] = palette[usize::from(*index)];
        }
    }

    Ok(AnimFrame {
        center_x,
        center_y,
        image: Image::new(width, height, pixels),
    })
}

/// Sign-extend a ten-bit field.
const fn sign_extend_10(value: u16) -> i16 {
    if value & 0x0200 == 0 {
        value as i16
    } else {
        (value | 0xFC00) as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three index shapes, at their boundaries. Each constant is one the
    /// file cannot confirm — a body read with the wrong one lands on another
    /// creature's frames, which decode perfectly.
    #[test]
    fn a_bodys_index_block_is_its_kinds_arithmetic() {
        assert_eq!(BodyKind::of(0), BodyKind::Monster);
        assert_eq!(BodyKind::of(199), BodyKind::Monster);
        assert_eq!(BodyKind::of(200), BodyKind::Animal);
        assert_eq!(BodyKind::of(399), BodyKind::Animal);
        assert_eq!(BodyKind::of(400), BodyKind::Human);

        assert_eq!(BodyKind::Monster.base(0), 0);
        assert_eq!(BodyKind::Monster.base(1), 110);
        // The kinds abut: the last monster block is where the animals start.
        assert_eq!(BodyKind::Monster.base(200), BodyKind::Animal.base(200));
        assert_eq!(BodyKind::Animal.base(200), 22_000);
        assert_eq!(BodyKind::Animal.base(400), BodyKind::Human.base(400));
        assert_eq!(BodyKind::Human.base(400), 35_000);

        // And each kind's blocks are its groups times its directions, which is
        // the relation the three constants encode.
        assert_eq!(
            BodyKind::Monster.base(1) - BodyKind::Monster.base(0),
            usize::from(BodyKind::Monster.groups()) * usize::from(DIRECTIONS),
        );
        assert_eq!(
            BodyKind::Animal.base(201) - BodyKind::Animal.base(200),
            usize::from(BodyKind::Animal.groups()) * usize::from(DIRECTIONS),
        );
        assert_eq!(
            BodyKind::Human.base(401) - BodyKind::Human.base(400),
            usize::from(BodyKind::Human.groups()) * usize::from(DIRECTIONS),
        );
    }

    /// The three enumerations, at the three numbers a client actually picks.
    ///
    /// Pinned because the failure is silent in the worst way: every one of these
    /// groups exists in every kind, so a wrong number draws a real animation of
    /// the wrong action, forever, with nothing to catch. The values are
    /// `HighAnimationGroup`, `LowAnimationGroup` and `PeopleAnimationGroup` in
    /// `AnimationsLoader.cs`.
    #[test]
    fn the_three_kinds_number_their_groups_differently() {
        assert_eq!(BodyKind::Monster.standing(), 1, "HighAnimationGroup.Stand");
        assert_eq!(BodyKind::Animal.standing(), 2, "LowAnimationGroup.Stand");
        assert_eq!(BodyKind::Human.standing(), 4, "PeopleAnimationGroup.Stand");
        // Walking is the coincidence, and the reason standing is not.
        for kind in [BodyKind::Monster, BodyKind::Animal, BodyKind::Human] {
            assert_eq!(kind.walking(), 0);
            // Whatever a kind names, it names inside its own table.
            assert!(kind.standing() < kind.groups());
            assert!(kind.running().is_none_or(|group| group < kind.groups()));
        }
        assert_eq!(BodyKind::Monster.running(), None, "High has no run");
        assert_eq!(BodyKind::Animal.running(), Some(1));
        assert_eq!(BodyKind::Human.running(), Some(2), "RunUnarmed");
    }

    /// A ghost is read under the living body of the same sex, and the living
    /// bodies are read under themselves.
    ///
    /// The pairs are `Mobile.GetGraphicForAnimation`'s. Written as a *relation*
    /// between the two ids rather than four literals, because the relation is
    /// the client's rule: the ghost is the body two above the living one, male
    /// and female alike.
    #[test]
    fn a_ghost_is_drawn_from_the_living_body_two_below_it() {
        assert_eq!(animation_body(0x0192), 0x0190, "the male ghost");
        assert_eq!(animation_body(0x0193), 0x0191, "the female ghost");
        for living in [0x0190u16, 0x0191] {
            assert_eq!(animation_body(living), living, "a living body is itself");
            assert_eq!(animation_body(living + 2), living, "and its ghost is it");
        }
        assert_eq!(animation_body(0x02B6), 667);
        assert_eq!(animation_body(0x02B7), 666);
    }

    /// And the half of that the file has to agree with: the ghost ids have no
    /// index block at all, so asking for what the shard named draws *nothing*.
    ///
    /// This is the test the defect deserved. A dead player simply stopped being
    /// on screen — no error, no log, the mobile silently dropped for want of a
    /// frame — and the only thing that says the remap is necessary rather than
    /// decorative is the file itself.
    ///
    /// Skipped without the client's files.
    #[test]
    fn the_ghost_bodies_are_in_no_index_block_and_the_bodies_they_map_to_are() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let mut anim = Anim::open(&dir).expect("anim.idx and anim.mul");
        for ghost in [0x0192u16, 0x0193] {
            assert!(
                anim.frames(ghost, BodyKind::Human.standing(), 0)
                    .expect("reading the index")
                    .is_none(),
                "body {ghost:#06x} has frames after all, and the remap is hiding them",
            );
            let drawn = animation_body(ghost);
            assert!(
                anim.frames(drawn, BodyKind::Human.standing(), 0)
                    .expect("reading the index")
                    .is_some_and(|frames| !frames.is_empty()),
                "body {drawn:#06x} is what a ghost is drawn from and has no frames either",
            );
        }
    }

    /// Eight facings, five pictures, and the mirror is the whole difference.
    #[test]
    fn four_facings_are_mirrors_of_the_other_four() {
        assert_eq!(facing(Direction::SouthEast), (0, false));
        for (left, right) in [
            (Direction::East, Direction::South),
            (Direction::NorthEast, Direction::SouthWest),
            (Direction::North, Direction::West),
        ] {
            let (stored_left, mirror_left) = facing(left);
            let (stored_right, mirror_right) = facing(right);
            assert_eq!(
                stored_left, stored_right,
                "{left:?} and {right:?} share a picture"
            );
            assert!(mirror_left && !mirror_right, "{left:?} is the flipped one");
        }
        // The eighth facing is the only one with a picture of its own.
        assert_eq!(facing(Direction::NorthWest), (4, false));
        assert_eq!(
            Direction::ALL.iter().filter(|d| facing(**d).1).count(),
            3,
            "three facings are mirrors and five are stored",
        );
    }

    /// The ten-bit offsets are signed, and a frame's runs are placed relative
    /// to its centre. Read unsigned, every negative offset lands 1,024 pixels
    /// to the right and the frame decodes to an empty picture.
    #[test]
    fn ten_bit_offsets_are_signed() {
        assert_eq!(sign_extend_10(0), 0);
        assert_eq!(sign_extend_10(0x01FF), 511);
        assert_eq!(sign_extend_10(0x0200), -512);
        assert_eq!(sign_extend_10(0x03FF), -1);
    }

    /// One frame, built by hand, decoded back.
    ///
    /// The fixture is the *format*, not a reader's understanding of a body:
    /// two runs at known offsets, and the assertion is where their pixels
    /// landed. That is what pins the sign extension and the `+ height` in the
    /// row calculation, neither of which a shipped file would report on.
    #[test]
    fn a_hand_built_frame_puts_its_runs_where_the_header_says() {
        let mut palette = vec![0u8; PALETTE_BYTES];
        // Palette entry 1 is red, entry 2 is green.
        palette[2..4].copy_from_slice(&0x7C00u16.to_le_bytes());
        palette[4..6].copy_from_slice(&0x03E0u16.to_le_bytes());

        let mut frame = Vec::new();
        // A 4x3 frame whose centre is two in and one up.
        frame.extend_from_slice(&2i16.to_le_bytes());
        frame.extend_from_slice(&(-1i16).to_le_bytes());
        frame.extend_from_slice(&4i16.to_le_bytes());
        frame.extend_from_slice(&3i16.to_le_bytes());
        // A run of two at x = -2 + 2 = 0, y = -2 + -1 + 3 = 0.
        let run = |x: i32, y: i32, len: u32| -> u32 {
            (((x as u32) & 0x3FF) << 22) | (((y as u32) & 0x3FF) << 12) | len
        };
        frame.extend_from_slice(&run(-2, -2, 2).to_le_bytes());
        frame.extend_from_slice(&[1, 2]);
        // And a run of one at x = 1, y = 2.
        frame.extend_from_slice(&run(-1, 0, 1).to_le_bytes());
        frame.extend_from_slice(&[2]);
        frame.extend_from_slice(&END_OF_FRAME.to_le_bytes());

        let mut entry = palette;
        entry.extend_from_slice(&1u32.to_le_bytes());
        // One frame, starting right after the count and the offset itself.
        entry.extend_from_slice(&8u32.to_le_bytes());
        entry.extend_from_slice(&frame);

        let frames = decode_body(400, 4, 0, &entry).expect("a well-formed entry");
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!((frame.center_x, frame.center_y), (2, -1));
        assert_eq!((frame.image.width(), frame.image.height()), (4, 3));
        assert_eq!(frame.image.pixel(0, 0), Some(Color16(0x7C00)));
        assert_eq!(frame.image.pixel(1, 0), Some(Color16(0x03E0)));
        assert_eq!(frame.image.pixel(2, 0), Some(Color16::TRANSPARENT));
        assert_eq!(frame.image.pixel(1, 2), Some(Color16(0x03E0)), "the second run");
    }

    /// A run that would draw outside its frame is refused, not clamped or
    /// wrapped. Wrapping is what a naive index does, and it produces a picture
    /// — a limb repeated one row up — that reads as art.
    #[test]
    fn a_run_outside_the_frame_is_refused() {
        let mut entry = vec![0u8; PALETTE_BYTES];
        entry.extend_from_slice(&1u32.to_le_bytes());
        entry.extend_from_slice(&8u32.to_le_bytes());
        entry.extend_from_slice(&0i16.to_le_bytes());
        entry.extend_from_slice(&0i16.to_le_bytes());
        entry.extend_from_slice(&2i16.to_le_bytes());
        entry.extend_from_slice(&2i16.to_le_bytes());
        // A run of three on a frame two wide.
        // x = 0, y = -2 (which the `+ height` puts on row 0), length 3.
        entry.extend_from_slice(&((0x3FEu32 << 12) | 3).to_le_bytes());
        entry.extend_from_slice(&[0, 0, 0]);
        entry.extend_from_slice(&END_OF_FRAME.to_le_bytes());

        assert!(matches!(
            decode_body(400, 4, 0, &entry),
            Err(AnimError::Malformed { .. })
        ));
    }

    /// The three ways the index says "nothing here", all of which the client
    /// accepts and any one of which read as data is a seek into the middle of
    /// somebody else's body.
    #[test]
    fn an_absent_entry_is_absent_in_three_shapes() {
        assert!(!IdxEntry { position: 0, size: 0 }.is_present());
        assert!(
            !IdxEntry {
                position: NO_ENTRY,
                size: 100
            }
            .is_present()
        );
        assert!(
            !IdxEntry {
                position: 512,
                size: NO_ENTRY
            }
            .is_present()
        );
        assert!(
            IdxEntry {
                position: 512,
                size: 100
            }
            .is_present()
        );
    }
}
