//! `anim.idx` and `anim.mul`: the frames a body is drawn from.
//!
//! Every mobile in UO is a body id, a group (standing, walking, attacking), a
//! facing, and a frame within that. The file is indexed by exactly that tuple —
//! flattened, in that order — and each entry holds a 256-colour palette and the
//! run-length encoded frames that use it.
//!
//! # The index is arithmetic over a table
//!
//! The position of a body's frames in `anim.idx` is computed, not looked up:
//! one of three constants times the body id. Which of the three is
//! [`IndexLayout`] — 22 groups based at 0, 13 based at 22,000, or 35 based at
//! 35,000, each with five stored directions, so 110, 65 and 175 blocks per
//! body. `AnimationsLoader.CalculateOffset` is where those numbers come from,
//! and they are not derivable from anything in the file: an index computed
//! with the wrong constant reads real frames belonging to another creature,
//! which draws something plausible and wrong.
//!
//! *Which* layout a body uses is the one part that is a table —
//! [`crate::mobtypes`], read from the install beside these two files. The body
//! id alone only approximates it ([`BodyKind::of`], [`IndexLayout::of`]), and
//! the client falls back to that approximation only when it has no table.
//!
//! # And `anim.mul` is not the only file
//!
//! An install ships six pairs, `anim` and `anim2` through `anim6`, and a body
//! added by a later expansion lives in one of the five rather than in the first
//! — under an id of its own, re-numbered from zero. [`crate::bodyconv`] is the
//! table that says which file and which id, so a lookup here is a body, a
//! group, a direction *and* an [`AnimFile`]. The stock install moves 875 bodies
//! that way, 460 of which have a standing animation the first file does not —
//! and a reader that opens only the first pair draws every one of them as
//! nothing at all: no error, no log, a creature hitting a player from an empty
//! tile.
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

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{
    Read,
    Seek,
    SeekFrom,
};
use std::path::{
    Path,
    PathBuf,
};

use openshard_protocol::direction::Direction;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};

use crate::bodyconv::BodyConv;
use crate::color::Color16;
use crate::image::Image;
use crate::mobtypes::{
    MobType,
    MobTypes,
};

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

/// One animation in the client's files: body, action group and stored direction.
///
/// The index is addressed by exactly this triple.  Keeping it as one value
/// prevents a caller from passing a direction where a group belongs — both are
/// bytes, but the file answers that mix-up with another animation rather than
/// an error.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AnimationKey {
    /// The body graphic the file stores.
    pub body:      Graphic,
    /// The body-specific action group.
    pub group:     AnimationGroup,
    /// The stored direction, zero through four.
    pub direction: AnimationDirection,
}

impl AnimationKey {
    #[must_use]
    pub const fn new(body: Graphic, group: AnimationGroup, direction: AnimationDirection) -> Self {
        Self {
            body,
            group,
            direction,
        }
    }
}

/// A group in one body's animation numbering.
///
/// Monster, animal and human bodies each assign different meanings to the same
/// byte, so this is intentionally a named value rather than a global enum.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AnimationGroup(pub u8);

impl AnimationGroup {
    #[must_use]
    pub const fn new(raw: u8) -> Self {
        Self(raw)
    }

    #[must_use]
    /// Its zero-based index in this body's animation-group table.
    ///
    /// The same index names a different action for each [`BodyKind`], but the
    /// `anim.idx` layout uses it as the group coordinate within that body's
    /// block.
    pub const fn index(self) -> u8 {
        self.0
    }
}

// Keep assertions against protocol fixtures readable; construction and all
// production APIs still require the named type.
impl PartialEq<u8> for AnimationGroup {
    fn eq(&self, other: &u8) -> bool {
        self.0 == *other
    }
}

impl fmt::Display for AnimationGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The stored, unmirrored direction index used by `anim.mul`.
///
/// This is deliberately not [`Direction`]: one is a five-entry file index,
/// while the other is an eight-way world-facing value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AnimationDirection(pub u8);

impl AnimationDirection {
    #[must_use]
    pub const fn new(raw: u8) -> Self {
        Self(raw)
    }

    #[must_use]
    /// Its zero-based index among the five directions stored in `anim.mul`.
    pub const fn index(self) -> u8 {
        self.0
    }
}

impl fmt::Display for AnimationDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The zero-based picture within one animation group.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct AnimationFrameIndex(pub u16);

impl AnimationFrameIndex {
    #[must_use]
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    #[must_use]
    /// Its zero-based position in this animation group's frame list.
    pub const fn index(self) -> u16 {
        self.0
    }
}

/// What can go wrong reading an animation.
#[derive(Debug)]
#[non_exhaustive]
pub enum AnimError {
    /// A file could not be read.
    Read {
        /// Which file.
        path:   PathBuf,
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
        /// Which animation's entry was malformed.
        key:    AnimationKey,
        /// What was wrong.
        detail: String,
    },
}

impl fmt::Display for AnimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotAnIndex { path, size } => {
                write!(
                    f,
                    "{} is {size} bytes, which is not a whole number of {IDX_ENTRY}-byte entries; \
                 it is not anim.idx",
                    path.display(),
                )
            }
            Self::Malformed { key, detail } => {
                write!(
                    f,
                    "body {} group {} direction {}: {detail}",
                    key.body.0, key.group, key.direction
                )
            }
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

/// Which of the three ways a body's actions are numbered.
///
/// The kinds are the client's own three enumerations. They are not a taxonomy:
/// a "monster" here means "actions named the way `HighAnimationGroup` names
/// them", and a body that looks like a horse is an animal because the client's
/// `mobtypes.txt` says so — see [`crate::mobtypes`], which is where the answer
/// actually comes from.
///
/// **This is the numbering only.** Which block of `anim.idx` the frames sit in
/// is [`IndexLayout`], and the two genuinely disagree: most animals below body
/// 200 are animal-numbered and stored in a monster-shaped block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyKind {
    /// `HighAnimationGroup`: 22 actions, walk at 0 and stand at 1.
    Monster,
    /// `LowAnimationGroup`: 13 actions, with a run the other two lack.
    Animal,
    /// `PeopleAnimationGroup`: 35 actions, which is where players live.
    Human,
}

impl BodyKind {
    /// Which kind a body id is *by its number alone*.
    ///
    /// The reference client's `CalculateTypeByGraphic`, which it reaches only
    /// when the install ships no `mobtypes.txt`. It disagrees with the shipped
    /// file for 322 bodies — every wolf, bear and cougar among them — so this
    /// is the fallback and [`crate::mobtypes::MobTypes::kind_of`] is the
    /// answer. Kept `const` and public because the fallback is a real answer
    /// for an install that has no table, and because a caller with no table in
    /// hand must reach something rather than inventing a fourth rule.
    pub const fn of(body: Graphic) -> Self {
        if body.0 < 200 {
            Self::Monster
        } else if body.0 < 400 {
            Self::Animal
        } else {
            Self::Human
        }
    }

    /// Which kind a body id is by its number alone *inside one of the install's
    /// animation files*.
    ///
    /// `CalculateTypeByGraphic(graphic, fileIndex)`, and the reason it takes the
    /// file: the ranges are not the same in every one of them. `anim2` holds no
    /// people at all — everything from 200 up is an animal there — and `anim3`
    /// puts its animals *below* its monsters rather than above. The other four
    /// use [`of`](Self::of)'s ranges.
    ///
    /// Reached for the same reason [`of`](Self::of) is, and only then: the
    /// install's `mobtypes.txt` has no line for this body. Five of the stock
    /// install's 875 redirected bodies are in that position.
    pub const fn in_file(body: Graphic, file: AnimFile) -> Self {
        match file {
            AnimFile::Second => {
                if body.0 < 200 {
                    Self::Monster
                } else {
                    Self::Animal
                }
            }
            AnimFile::Third => {
                if body.0 < 300 {
                    Self::Animal
                } else if body.0 < 400 {
                    Self::Monster
                } else {
                    Self::Human
                }
            }
            AnimFile::First | AnimFile::Fourth | AnimFile::Fifth | AnimFile::Sixth => Self::of(body),
        }
    }

    /// How many actions this numbering names.
    ///
    /// Deliberately not [`IndexLayout::groups`], which counts the *slots* in a
    /// block. The two carry the same three numbers and answer different
    /// questions, and a sea monster is where they part company: 13 actions in a
    /// 22-slot block. A group number this side of `actions` is one the
    /// numbering has a name for; one this side of `groups` is merely one the
    /// index can address.
    pub const fn actions(self) -> u8 {
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
    pub const fn standing(self) -> AnimationGroup {
        match self {
            Self::Monster => AnimationGroup(1),
            Self::Animal => AnimationGroup(2),
            Self::Human => AnimationGroup(4),
        }
    }

    /// Which group is the first death animation for this kind of body.
    ///
    /// Like standing and walking, these are three distinct enumerations: group
    /// 2 is `Die1` for a monster, but group 8 is the animal equivalent and 21
    /// is the human one. A corpse is drawn from this group rather than from the
    /// `0x2006` item's static art, whose payload names the body it was.
    pub const fn dying(self) -> AnimationGroup {
        match self {
            Self::Monster => AnimationGroup(2),
            Self::Animal => AnimationGroup(8),
            Self::Human => AnimationGroup(21),
        }
    }

    /// Which group means "the ordinary swing".
    ///
    /// `HighAnimationGroup.Attack1` is 4 and `LowAnimationGroup.Attack1` is 5,
    /// which is the pair that made an attacking cougar disappear: 4 in the
    /// animal numbering is `Unknown`, a group most animals have in one
    /// direction of five or in none at all, so a shard that reached for the
    /// monster's number handed the renderer nothing to draw.
    ///
    /// A human's is 31, `PeopleAnimationGroup.AttackUnarmedAndWalk` — the
    /// bare-handed swing, and the same number `WeaponAnimation::Wrestle`
    /// carries. What a *weapon* does to it needs to know what is in the hands,
    /// which nothing in this crate does; this is the answer for a body that is
    /// holding nothing, and the one every creature has.
    pub const fn attacking(self) -> AnimationGroup {
        match self {
            Self::Monster => AnimationGroup(4),
            Self::Animal => AnimationGroup(5),
            Self::Human => AnimationGroup(31),
        }
    }

    /// Which group means "casting a spell", or `None` for a kind with no such
    /// pose.
    ///
    /// `HighAnimationGroup.Cast` is 12 and a human's directed cast is 16. The
    /// low numbering has neither: 12 there is `Die2`, so an animal handed the
    /// monster's number plays a *death* and then holds its last frame. `None`
    /// is "it has no cast", and the caller's answer for that is
    /// [`attacking`](Self::attacking) — a creature that cannot gesture still
    /// lunges.
    pub const fn casting(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster => Some(AnimationGroup(12)),
            Self::Animal => None,
            Self::Human => Some(AnimationGroup(16)),
        }
    }

    /// Which group means "walking".
    ///
    /// Zero in all three, which is the one coincidence between them — and the
    /// reason it is written out anyway: a caller reading `walking()` beside
    /// [`BodyKind::standing`] cannot conclude that the numbering is shared.
    /// A human's is `WalkUnarmed`; what a weapon does to it is M4's problem,
    /// since nothing here knows what anyone is holding.
    pub const fn walking(self) -> AnimationGroup {
        match self {
            Self::Monster | Self::Animal | Self::Human => AnimationGroup(0),
        }
    }

    /// Which group means "running".
    ///
    /// `None` for a monster: `HighAnimationGroup` has no run at all — it goes
    /// `Walk`, `Stand`, `Die1` — so a running monster keeps walking, which is
    /// what the client draws too. An animal's is 1 and a human's is 2
    /// (`RunUnarmed`).
    pub const fn running(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster => None,
            Self::Animal => Some(AnimationGroup(1)),
            Self::Human => Some(AnimationGroup(2)),
        }
    }

    /// Which group means "standing, at war", or `None` for a kind that stands
    /// the same either way.
    ///
    /// A human's is 7, `PeopleAnimationGroup.StandOnehandedAttack` — a body on
    /// guard, weight forward, which is the whole visible difference between a
    /// shopkeeper and a shopkeeper who means it. `None` for a monster and an
    /// animal because the classic path has no such group for either: the
    /// reference's own `GetGroupForAnimation` reaches its war branch only under
    /// `AnimationGroupsType.Human`/`Equipment`, and a wolf at war stands
    /// exactly as a wolf at peace does.
    ///
    /// `None` is "use [`standing`](Self::standing)", which is what makes a
    /// caller one `unwrap_or` rather than a second `match` over the kind.
    ///
    /// # What is deliberately not here
    ///
    /// `8` (`StandTwohandedAttack`) is the same stance with a two-handed weapon,
    /// and choosing between 7 and 8 needs to know what is in the hands. Nothing
    /// in this crate does: an `AnimID` reaches the renderer and the item's wire
    /// graphic — which is what the reference branches on — is thrown away on the
    /// way (`crowd::worn` in the client). One missing field, and it is the same
    /// one `MobileView.IsCovered` wants; see `docs/combat/design_fight_loop.md` D2.
    pub const fn standing_at_war(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster | Self::Animal => None,
            Self::Human => Some(AnimationGroup(7)),
        }
    }

    /// Which group means "walking, at war", or `None` for a kind that walks the
    /// same either way.
    ///
    /// A human's is 15, `PeopleAnimationGroup.WalkWarmode`: a guarded walk, not
    /// the `WalkUnarmed` of 0. `None` for the other two, for
    /// [`standing_at_war`](Self::standing_at_war)'s reason.
    ///
    /// **Running is not affected and has no war twin.** The reference falls
    /// straight through to the ordinary run for `isRun || !InWarMode ||
    /// IsDead` — a body sprinting is a body not fighting, whatever its stance
    /// says — so [`running`](Self::running) needs no companion here.
    pub const fn walking_at_war(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster | Self::Animal => None,
            Self::Human => Some(AnimationGroup(15)),
        }
    }

    /// Which group means "standing, mounted", or `None` for a kind that never
    /// rides.
    ///
    /// A human's is 25, `PeopleAnimationGroup.OnmountStand` — the frames a
    /// rider is drawn from are not the on-foot stand's, because they seat the
    /// body in a saddle rather than plant its feet on the ground. `None` for a
    /// monster and an animal: nothing in `Layer::MOUNT` ever equips onto one,
    /// so the block those kinds' own numbering could name here is simply never
    /// reached — the mount itself keeps playing its ordinary
    /// [`standing`](Self::standing).
    ///
    /// `26`-`29` (`OnmountAttack`/`OnmountAttackBow`/`OnmountAttackCrossbow`/
    /// `OnmountSlapHorse`) are the mounted combat poses. They are selected in
    /// `client/app` where the action packet's weapon motion and the tracked
    /// saddle state are both available; this type owns only the stance.
    pub const fn standing_mounted(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster | Self::Animal => None,
            Self::Human => Some(AnimationGroup(25)),
        }
    }

    /// Which group means "walking, mounted", or `None` for a kind that never
    /// rides.
    ///
    /// A human's is 23, `PeopleAnimationGroup.OnmountRideSlow`. `None` for
    /// [`standing_mounted`](Self::standing_mounted)'s reason.
    pub const fn walking_mounted(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster | Self::Animal => None,
            Self::Human => Some(AnimationGroup(23)),
        }
    }

    /// Which group means "running, mounted", or `None` for a kind that never
    /// rides.
    ///
    /// A human's is 24, `PeopleAnimationGroup.OnmountRideFast`. `None` for
    /// [`standing_mounted`](Self::standing_mounted)'s reason — unlike
    /// [`running`](Self::running), this is never reached for a monster or an
    /// animal in the first place, so there is no "keeps walking" fallback to
    /// state here.
    pub const fn running_mounted(self) -> Option<AnimationGroup> {
        match self {
            Self::Monster | Self::Animal => None,
            Self::Human => Some(AnimationGroup(24)),
        }
    }
}

/// Which of an install's six animation file pairs a body's frames are in.
///
/// `AnimationsLoader._files`, whose slots the reference fills from `anim.mul`,
/// `anim2.mul` and so on. Not a property of a body id: which file holds a body
/// is a row of [`crate::bodyconv`], and the same id means different creatures
/// in different files — body 29 is one thing in `anim.mul` and the body 752 is
/// drawn from in `anim2.mul`.
///
/// Six because that is what an install ships and what `Bodyconv.def` can name:
/// its rows carry five columns, one per file after the first. The reference
/// keeps ten slots and fills the four above these with nothing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AnimFile {
    /// `anim.idx`/`anim.mul`, the one every body without a `Bodyconv.def` row
    /// is read from.
    First,
    /// `anim2.idx`/`anim2.mul`.
    Second,
    /// `anim3.idx`/`anim3.mul`.
    Third,
    /// `anim4.idx`/`anim4.mul`.
    Fourth,
    /// `anim5.idx`/`anim5.mul`.
    Fifth,
    /// `anim6.idx`/`anim6.mul`.
    Sixth,
}

impl AnimFile {
    /// The five files a `Bodyconv.def` row can send a body to, in the order its
    /// columns name them.
    ///
    /// [`First`](Self::First) is deliberately not here: a row exists to say the
    /// body is somewhere *else*, and the reference's own columns start at
    /// `_files[1]`.
    pub const REDIRECTS: [Self; 5] = [Self::Second, Self::Third, Self::Fourth, Self::Fifth, Self::Sixth];

    /// What both halves of this pair are called, without the extension:
    /// `anim`, `anim2`, and so on.
    #[must_use]
    pub const fn stem(self) -> &'static str {
        match self {
            Self::First => "anim",
            Self::Second => "anim2",
            Self::Third => "anim3",
            Self::Fourth => "anim4",
            Self::Fifth => "anim5",
            Self::Sixth => "anim6",
        }
    }
}

/// Which shape of `anim.idx` block a body's frames are stored in.
///
/// The three cases of `AnimationsLoader.CalculateOffset`. Separate from
/// [`BodyKind`] because the client's own table makes them disagree: an animal
/// carrying `CalculateOffsetLowGroupExtended` — which is most animals numbered
/// below body 200 — is animal-*numbered* and monster-*shaped*, and a sea
/// monster is both of those too. Conflating them reads another creature's
/// frames, which draws something plausible and wrong rather than failing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IndexLayout {
    /// 22 groups of five, based at block zero: `CalculateHighGroupOffset`.
    High,
    /// 13 groups of five, based at block 22,000: `CalculateLowGroupOffset`.
    Low,
    /// 35 groups of five, based at block 35,000: `CalculatePeopleGroupOffset`.
    People,
}

impl IndexLayout {
    /// Which layout a body id uses *by its number alone*.
    ///
    /// [`BodyKind::of`]'s counterpart, and the same fallback: what to answer
    /// when the install ships no `mobtypes.txt`.
    pub const fn of(body: Graphic) -> Self {
        Self::in_file(body, AnimFile::First)
    }

    /// Which layout a body id uses by its number alone *inside one of the
    /// install's animation files*.
    ///
    /// [`BodyKind::in_file`]'s counterpart, and written in terms of it because
    /// they are one question asked twice: with no flag word to steer it — and
    /// there is none, since this is reached only when `mobtypes.txt` says
    /// nothing about the body — the reference's `CalculateOffset` gives a
    /// monster the high block, an animal the low one and a person the people
    /// one.
    pub const fn in_file(body: Graphic, file: AnimFile) -> Self {
        match BodyKind::in_file(body, file) {
            BodyKind::Monster => Self::High,
            BodyKind::Animal => Self::Low,
            BodyKind::Human => Self::People,
        }
    }

    /// How many animation groups a block of this shape holds.
    pub const fn groups(self) -> u8 {
        match self {
            Self::High => 22,
            Self::Low => 13,
            Self::People => 35,
        }
    }

    /// The first index block belonging to `body`, or `None` where the
    /// arithmetic does not reach one.
    ///
    /// `CalculateOffset` in blocks rather than bytes. The absent answer is the
    /// low and people bases applied to a body below their own first id: the
    /// reference subtracts anyway and casts the negative product to `uint`,
    /// which lands the read in another family's region. The shipped table asks
    /// for exactly that twice — bodies 95 and 169 are `ANIMAL` with no
    /// extended flag — and reading a stranger's frames there is worse than
    /// reading none.
    const fn base(self, body: u16) -> Option<usize> {
        let body = body as usize;
        match self {
            Self::High => Some(body * 110),
            Self::Low if body >= 200 => Some(22_000 + (body - 200) * 65),
            Self::People if body >= 400 => Some(35_000 + (body - 400) * 175),
            Self::Low | Self::People => None,
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
pub const fn is_ghost(body: Graphic) -> bool {
    matches!(body.0, 0x0192 | 0x0193)
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
pub const fn is_female(body: Graphic) -> bool {
    matches!(body.0, 0x0191 | 0x0193 | 0x025E | 0x029B)
}

/// Whether a body is a gargoyle one, living or dead.
///
/// Asked by exactly one thing — the paperdoll's layer ordering, which puts a
/// gargoyle's torso where a female body's goes
/// (`PaperDollInteractable.IsGargoyleBody`) — and it is the four ids rather
/// than the two, because `0x02B6`/`0x02B7` are the dead pair of `0x029A`/
/// `0x029B` and a corpse is drawn on a paperdoll like anything else.
pub const fn is_gargoyle(body: Graphic) -> bool {
    matches!(body.0, 0x029A | 0x029B | 0x02B6 | 0x02B7)
}

pub const fn animation_body(body: Graphic) -> Graphic {
    Graphic(match body.0 {
        // The ghosts, drawn from the living body of the same sex.
        0x0192 | 0x0193 => body.0 - 2,
        // The other two the client remaps, kept because the reason is the same
        // — an id with no block of its own — and because leaving half a port
        // behind is how the next one gets re-derived.
        0x02B6 => 667,
        0x02B7 => 666,
        _ => body.0,
    })
}

/// One visual override from the optional `Body.def` beside a client install.
///
/// The server still names the original body.  The classic client redirects it
/// before it chooses an animation; for example, body 25 (grey wolf) becomes
/// body 225.  Without this table a modern install's legacy `anim.mul` has no
/// frames for many otherwise ordinary spawn bodies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BodyAppearance {
    pub body: Graphic,
    pub hue:  Hue,
}

/// The visual body redirects in a client's optional `Body.def`.
#[derive(Clone, Default, Debug)]
pub struct BodyDef {
    redirects: BTreeMap<Graphic, BodyAppearance>,
}

impl BodyDef {
    /// Read `Body.def` when the install ships one.
    ///
    /// Older installs legitimately do not have the file, which is the same as
    /// an empty redirect table.  A malformed individual line is ignored: the
    /// stock file contains comments and historical examples in several forms,
    /// and one stale override must not prevent all other bodies from drawing.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, AnimError> {
        let path = client_dir.as_ref().join("Body.def");
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(AnimError::Read { path, source }),
        };

        Ok(Self::from_text(&source))
    }

    /// Parse the file's text.
    ///
    /// Public for the same reason [`crate::mobtypes::MobTypes::from_text`] is:
    /// a test about one redirect should be able to state that one row, rather
    /// than needing a client install on disk to say "a barded horse is drawn as
    /// body 200".
    #[must_use]
    pub fn from_text(source: &str) -> Self {
        let mut redirects = BTreeMap::new();
        for line in source.lines() {
            let line = line.split('#').next().unwrap_or_default();
            let fields: Vec<_> = line
                .split(|c: char| c.is_whitespace() || matches!(c, '{' | '}' | ','))
                .filter(|field| !field.is_empty())
                .collect();
            let Some((from, rest)) = fields.split_first() else {
                continue;
            };
            let Some(to) = rest.first() else {
                continue;
            };
            let Some(hue) = rest.last() else {
                continue;
            };
            let (Ok(from), Ok(to), Ok(hue)) = (from.parse::<u16>(), to.parse::<u16>(), hue.parse::<u16>())
            else {
                continue;
            };
            redirects.insert(
                Graphic(from),
                BodyAppearance {
                    body: Graphic(to),
                    hue:  Hue(hue),
                },
            );
        }
        Self { redirects }
    }

    /// The visual body and forced hue a client uses for `body`.
    pub fn appearance(&self, body: Graphic) -> BodyAppearance {
        self.redirects
            .get(&body)
            .copied()
            .unwrap_or(BodyAppearance { body, hue: Hue::NONE })
    }
}

/// The stored direction a facing is drawn from, and whether it is mirrored.
///
/// `Animation.GetAnimDirection`. Facing 3 is the one stored as direction 0, and
/// the four facings on the other side of it reuse 1, 2 and 3 flipped — which is
/// why a flipped sprite is placed from `width - center_x` rather than from
/// `center_x`.
pub const fn facing(facing: Direction) -> (AnimationDirection, bool) {
    match facing {
        Direction::North => (AnimationDirection(3), true),
        Direction::NorthEast => (AnimationDirection(2), true),
        Direction::East => (AnimationDirection(1), true),
        Direction::SouthEast => (AnimationDirection(0), false),
        Direction::South => (AnimationDirection(1), false),
        Direction::SouthWest => (AnimationDirection(2), false),
        Direction::West => (AnimationDirection(3), false),
        Direction::NorthWest => (AnimationDirection(4), false),
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
    pub image:    Image,
}

/// One entry of `anim.idx`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct IdxEntry {
    position: u32,
    size:     u32,
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

/// Where one body's frames are actually read from.
///
/// Three answers that a body id alone gives none of, and that no caller should
/// have to assemble twice: [`crate::bodyconv`] says which file and which id
/// inside it, and `mobtypes.txt` — or, where it is silent, the range rule for
/// that file — says which block shape. Produced by [`Anim::source`] and
/// consumed by the two lookups beside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AnimSource {
    /// Which of the install's pairs holds the frames.
    pub file:   AnimFile,
    /// The id the body carries *in that file*, which is its own id whenever
    /// `Bodyconv.def` does not move it.
    pub body:   Graphic,
    /// Which block shape the index is addressed with.
    pub layout: IndexLayout,
}

/// One `animN.idx`/`animN.mul` pair: the index held, the frames left on disk.
#[derive(Debug)]
struct AnimPair {
    entries:  Vec<IdxEntry>,
    mul:      File,
    mul_path: PathBuf,
    mul_len:  u64,
}

/// The client's animations, indexed and ready to read from.
///
/// All six pairs an install can ship, and the `Bodyconv.def` that says which of
/// them holds a body — that table is *this* reader's, because it answers a
/// question about the files it opened and nothing else in the client asks it.
///
/// Which block a body's frames sit in is a different matter and is not decided
/// here: it is a row of the install's `mobtypes.txt`, so every lookup takes the
/// [`crate::mobtypes::MobType`] its caller looked up. Holding a copy of that
/// table here would put a second owner of one file's contents beside the
/// client's own, which chooses group *numbers* out of it.
#[derive(Debug)]
pub struct Anim {
    first: AnimPair,
    /// `anim2` through `anim6`, in [`AnimFile::REDIRECTS`] order. `None` is an
    /// install that ships neither half of that pair, which every install below
    /// the expansion that added it does.
    later: [Option<AnimPair>; AnimFile::REDIRECTS.len()],
    conv:  BodyConv,
}

impl AnimPair {
    /// Open one named `animN.idx`/`animN.mul` pair.
    fn open(idx: impl AsRef<Path>, mul: impl AsRef<Path>) -> Result<Self, AnimError> {
        let idx_path = idx.as_ref();
        let raw = std::fs::read(idx_path).map_err(|source| {
            AnimError::Read {
                path: idx_path.to_owned(),
                source,
            }
        })?;
        if raw.is_empty() || !raw.len().is_multiple_of(IDX_ENTRY) {
            return Err(AnimError::NotAnIndex {
                path: idx_path.to_owned(),
                size: raw.len() as u64,
            });
        }
        let entries = raw
            .as_chunks::<IDX_ENTRY>()
            .0
            .iter()
            .map(|entry| {
                IdxEntry {
                    position: u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]),
                    size:     u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]),
                }
            })
            .collect();

        let mul_path = mul.as_ref().to_owned();
        let file = File::open(&mul_path).map_err(|source| {
            AnimError::Read {
                path: mul_path.clone(),
                source,
            }
        })?;
        let mul_len = file
            .metadata()
            .map_err(|source| {
                AnimError::Read {
                    path: mul_path.clone(),
                    source,
                }
            })?
            .len();

        Ok(Self {
            entries,
            mul: file,
            mul_path,
            mul_len,
        })
    }

    /// How many index entries this file holds. Most of them are empty.
    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// The frames of one animation, addressed as `source` says.
    ///
    /// `key` is the caller's own — the body it named, not the one the index is
    /// read under — so a malformed entry is reported against the body a shard
    /// put on the wire rather than against an id only this crate knows about.
    fn frames(&mut self, key: AnimationKey, source: AnimSource) -> Result<Option<Vec<AnimFrame>>, AnimError> {
        let Some(entry) = self.entry(source, key.group, key.direction) else {
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
            .map_err(|source| {
                AnimError::Read {
                    path: self.mul_path.clone(),
                    source,
                }
            })?;

        decode_body(key, &raw).map(Some)
    }

    /// The index entry for one animation.
    fn entry(
        &self,
        source: AnimSource,
        group: AnimationGroup,
        direction: AnimationDirection,
    ) -> Option<IdxEntry> {
        if group.index() >= source.layout.groups() || direction.index() >= DIRECTIONS {
            return None;
        }
        let block = source.layout.base(source.body.0)?
            + usize::from(group.index()) * usize::from(DIRECTIONS)
            + usize::from(direction.index());
        self.entries.get(block).copied()
    }
}

impl Anim {
    /// Open every animation pair a client directory ships, and the
    /// `Bodyconv.def` that says which of them holds what.
    ///
    /// The first pair is required — an install without it has no animations at
    /// all — and the other five are taken when both halves are there. An
    /// install below the expansion that added `anim6` is not a broken install.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, AnimError> {
        let dir = client_dir.as_ref();
        let named = |file: AnimFile| {
            (
                dir.join(format!("{}.idx", file.stem())),
                dir.join(format!("{}.mul", file.stem())),
            )
        };
        let (idx, mul) = named(AnimFile::First);
        let first = AnimPair::open(idx, mul)?;

        let mut later = [None, None, None, None, None];
        for (slot, file) in later.iter_mut().zip(AnimFile::REDIRECTS) {
            let (idx, mul) = named(file);
            // Half a pair is as unreadable as none of it, and the reference
            // requires both too: an index with no data file behind it would
            // name offsets into nothing.
            if idx.is_file() && mul.is_file() {
                *slot = Some(AnimPair::open(idx, mul)?);
            }
        }

        Ok(Self {
            first,
            later,
            conv: BodyConv::open(dir)?,
        })
    }

    /// Open one named pair on its own, with no companions and no redirects.
    ///
    /// For a test that has built an index by hand, and for a tool pointed at a
    /// single file. Every body then reads from that one pair under its own id,
    /// which is what an install with no `Bodyconv.def` does anyway.
    pub fn from_files(idx: impl AsRef<Path>, mul: impl AsRef<Path>) -> Result<Self, AnimError> {
        Ok(Self {
            first: AnimPair::open(idx, mul)?,
            later: [None, None, None, None, None],
            conv:  BodyConv::empty(),
        })
    }

    /// How many index entries the first file holds. Most of them are empty.
    pub fn entry_count(&self) -> usize {
        self.first.entry_count()
    }

    /// Where this install reads a body's frames from.
    ///
    /// `mob_type` is what `mobtypes.txt` says about the body the *shard* named
    /// — see [`crate::mobtypes::MobTypes::get`] — and `None` is the file saying
    /// nothing about it, not a caller with no opinion: the range rule that
    /// answers for it is the reference's own fallback and depends on which file
    /// the body ends up in.
    ///
    /// Public because "which file is this creature in" is a question a test and
    /// a tool both ask, and because the answer is otherwise invisible: a body
    /// read out of the wrong file decodes another creature's frames perfectly.
    ///
    /// # Only half of the fallback is here
    ///
    /// Where `mobtypes.txt` is silent this resolves the *block shape* against
    /// the file the body ends up in, as the reference does. The other half —
    /// which numbering names the body's actions, and so which group number a
    /// caller asks for — is chosen before a lookup reaches this crate, out of
    /// [`crate::mobtypes::MobTypes::kind_of`], which knows nothing about the
    /// redirect and answers from the id the shard named. The two agree for
    /// every row the stock install has: its five redirected bodies with no
    /// `mobtypes.txt` line all land in `anim3` above id 400, where the file's
    /// own range rule and the general one say the same word. A later install
    /// that breaks that tie is what would make the numbering worth threading
    /// through the client as well.
    #[must_use]
    pub fn source(&self, body: Graphic, mob_type: Option<MobType>) -> AnimSource {
        let (file, read_as) = self.redirect(body);
        AnimSource {
            file,
            body: read_as,
            layout: mob_type.map_or_else(|| IndexLayout::in_file(read_as, file), |named| named.layout),
        }
    }

    /// Which file `body` is actually read from on this install, and what id
    /// it carries there.
    ///
    /// [`source`](Self::source)'s redirect walk, on its own: a caller with no
    /// frames to read still needs this half — see
    /// [`redirect_kinds`](Self::redirect_kinds). [`BodyConv::redirect`] does
    /// the actual walk; this just hands it "is that file open here".
    fn redirect(&self, body: Graphic) -> (AnimFile, Graphic) {
        self.conv.redirect(body, |file| self.pair(file).is_some())
    }

    /// Which numbering a redirected body falls back to when `mobtypes.txt`
    /// has no line for it.
    ///
    /// [`crate::mobtypes::MobTypes::kind_of`]'s own fallback is
    /// [`BodyKind::of`] — the id-range rule, blind to `Bodyconv.def` — and
    /// that can read the wrong numbering for a body the table moves into a
    /// file whose own range disagrees with the original id's, exactly the way
    /// the id-range rule already misreads the block shape for a body
    /// `mobtypes.txt` names but the range rule alone would not
    /// (`docs/roadmap/backlog/gameplay.md`, "A body id names a file too").
    /// The stock install's five bodies in this position happen to land where
    /// [`BodyKind::of`] and [`BodyKind::in_file`] already agree — see
    /// [`source`](Self::source)'s own note — so this closes a install-shaped
    /// gap rather than a bug the shipped files can be seen to hit yet. The
    /// block-shape half of the same fallback is resolved this way already,
    /// inside `source`; this is the numbering half, exposed so a caller that
    /// chooses group numbers before it has any frame to read — the client's
    /// `Crowd` — can resolve the two the same way instead of disagreeing with
    /// its own renderer.
    ///
    /// A snapshot rather than a live query: this cannot change once the
    /// install is open, and `Crowd` needs the answer long before it has any
    /// reason to hold this (large, file-backed) reader itself. Restricted to
    /// bodies `mobtypes.txt` is silent about — a body the file *does* name
    /// already has its numbering, and this must not second-guess it.
    #[must_use]
    pub fn redirect_kinds(&self, mob_types: &MobTypes) -> BTreeMap<Graphic, BodyKind> {
        self.conv
            .bodies()
            .filter(|&body| mob_types.get(body).is_none())
            .filter_map(|body| {
                let (file, read_as) = self.redirect(body);
                (file != AnimFile::First).then(|| (body, BodyKind::in_file(read_as, file)))
            })
            .collect()
    }

    /// Whether one animation has any frames at all.
    ///
    /// Cheap — it reads the index only — and worth having separately: most of
    /// the index is empty, and "does this body exist" is a question a caller
    /// asks far more often than it asks for pixels.
    pub fn has_frames(&self, key: AnimationKey, mob_type: Option<MobType>) -> bool {
        let source = self.source(key.body, mob_type);
        self.pair(source.file)
            .and_then(|pair| pair.entry(source, key.group, key.direction))
            .is_some_and(IdxEntry::is_present)
    }

    /// The frames of one body, group and *stored* direction.
    ///
    /// `direction` is 0 to 4, which is what the file holds — pass a wire facing
    /// through [`facing`] first, and mirror the sprite yourself if it says so.
    /// Drawing is not this crate's business and the flip belongs where the
    /// quad is built.
    ///
    /// `mob_type` is [`source`](Self::source)'s: what the install's own table
    /// says about this body. It is a parameter rather than something derived
    /// here because the block shape is not derivable from the body id — the
    /// range rule that looks like a derivation puts every wolf and cougar in
    /// the wrong region, and a block read at the wrong stride decodes another
    /// creature's frames perfectly.
    ///
    /// `None` means the client ships no animation there, which is the ordinary
    /// answer for most of the index: a group a body does not have, or a body id
    /// nothing uses.
    pub fn frames(
        &mut self,
        key: AnimationKey,
        mob_type: Option<MobType>,
    ) -> Result<Option<Vec<AnimFrame>>, AnimError> {
        let source = self.source(key.body, mob_type);
        match self.pair_mut(source.file) {
            Some(pair) => pair.frames(key, source),
            // Unreachable through `source`, which only names a file this reader
            // holds — and a cheaper answer than a panic if that ever stops
            // being true: no frames is what every other absence here reads as.
            None => Ok(None),
        }
    }

    /// The pair one file's frames are in, or `None` for a file this install
    /// does not ship.
    fn pair(&self, file: AnimFile) -> Option<&AnimPair> {
        let [second, third, fourth, fifth, sixth] = &self.later;
        match file {
            AnimFile::First => Some(&self.first),
            AnimFile::Second => second.as_ref(),
            AnimFile::Third => third.as_ref(),
            AnimFile::Fourth => fourth.as_ref(),
            AnimFile::Fifth => fifth.as_ref(),
            AnimFile::Sixth => sixth.as_ref(),
        }
    }

    /// [`pair`](Self::pair), for the half of the reader that seeks.
    fn pair_mut(&mut self, file: AnimFile) -> Option<&mut AnimPair> {
        let [second, third, fourth, fifth, sixth] = &mut self.later;
        match file {
            AnimFile::First => Some(&mut self.first),
            AnimFile::Second => second.as_mut(),
            AnimFile::Third => third.as_mut(),
            AnimFile::Fourth => fourth.as_mut(),
            AnimFile::Fifth => fifth.as_mut(),
            AnimFile::Sixth => sixth.as_mut(),
        }
    }
}

/// Decode one index entry: a palette, then the frames that use it.
///
/// Split out so the format can be exercised on bytes built by hand — the
/// failures worth catching are an offset past the end and a run that draws
/// outside its own frame, and a shipped file offers neither on demand.
fn decode_body(key: AnimationKey, raw: &[u8]) -> Result<Vec<AnimFrame>, AnimError> {
    let malformed = |detail: String| AnimError::Malformed { key, detail };

    let palette_bytes = raw
        .get(..PALETTE_BYTES)
        .ok_or_else(|| malformed("shorter than its own palette".to_owned()))?;
    let mut palette = [Color16::TRANSPARENT; PALETTE_COLORS];
    for (slot, word) in palette.iter_mut().zip(palette_bytes.as_chunks::<2>().0) {
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
    for offset in offsets.as_chunks::<4>().0 {
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
        let run_header = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        at += 4;
        if run_header == END_OF_FRAME {
            break;
        }

        // Three fields in one word: a length, and two signed ten-bit offsets
        // that are *relative to the centre*. Sign-extending them is what makes
        // a frame's pixels land around the body rather than in the corner.
        let run = (run_header & 0x0FFF) as usize;
        let x = i32::from(sign_extend_10(((run_header >> 22) & 0x03FF) as u16)) + i32::from(center_x);
        let y = i32::from(sign_extend_10(((run_header >> 12) & 0x03FF) as u16))
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

    #[test]
    fn frame_index_names_a_pictures_position_in_its_group() {
        assert_eq!(AnimationFrameIndex::new(37).index(), 37);
    }

    /// The three index shapes, at their boundaries. Each constant is one the
    /// file cannot confirm — a body read with the wrong one lands on another
    /// creature's frames, which decode perfectly.
    #[test]
    fn a_bodys_index_block_is_its_layouts_arithmetic() {
        assert_eq!(IndexLayout::of(Graphic(0)), IndexLayout::High);
        assert_eq!(IndexLayout::of(Graphic(199)), IndexLayout::High);
        assert_eq!(IndexLayout::of(Graphic(200)), IndexLayout::Low);
        assert_eq!(IndexLayout::of(Graphic(399)), IndexLayout::Low);
        assert_eq!(IndexLayout::of(Graphic(400)), IndexLayout::People);

        assert_eq!(IndexLayout::High.base(0), Some(0));
        assert_eq!(IndexLayout::High.base(1), Some(110));
        // The layouts abut: the last monster block is where the animals start.
        assert_eq!(IndexLayout::High.base(200), IndexLayout::Low.base(200));
        assert_eq!(IndexLayout::Low.base(200), Some(22_000));
        assert_eq!(IndexLayout::Low.base(400), IndexLayout::People.base(400));
        assert_eq!(IndexLayout::People.base(400), Some(35_000));

        // And each layout's blocks are its groups times its directions, which
        // is the relation the three constants encode.
        assert_eq!(
            IndexLayout::High.base(1).unwrap() - IndexLayout::High.base(0).unwrap(),
            usize::from(IndexLayout::High.groups()) * usize::from(DIRECTIONS),
        );
        assert_eq!(
            IndexLayout::Low.base(201).unwrap() - IndexLayout::Low.base(200).unwrap(),
            usize::from(IndexLayout::Low.groups()) * usize::from(DIRECTIONS),
        );
        assert_eq!(
            IndexLayout::People.base(401).unwrap() - IndexLayout::People.base(400).unwrap(),
            usize::from(IndexLayout::People.groups()) * usize::from(DIRECTIONS),
        );
    }

    /// The range rule is not one rule: two of the six files number their own
    /// bodies differently, and the fallback has to ask which file it is in.
    ///
    /// Pinned because it is silent in the way everything about this index is:
    /// body 250 read as a person in `anim2` lands at block 35,000 upward, which
    /// is real data belonging to somebody else. `CalculateTypeByGraphic`'s two
    /// special cases are the whole of it.
    #[test]
    fn two_of_the_files_number_their_bodies_on_their_own_ranges() {
        // anim2: monsters below 200, animals all the way up — no people at all.
        assert_eq!(
            BodyKind::in_file(Graphic(199), AnimFile::Second),
            BodyKind::Monster
        );
        assert_eq!(
            BodyKind::in_file(Graphic(200), AnimFile::Second),
            BodyKind::Animal
        );
        assert_eq!(
            BodyKind::in_file(Graphic(400), AnimFile::Second),
            BodyKind::Animal,
            "a body that would be a person anywhere else",
        );
        assert_eq!(
            IndexLayout::in_file(Graphic(400), AnimFile::Second),
            IndexLayout::Low
        );

        // anim3: animals *below* monsters, and people above both.
        assert_eq!(BodyKind::in_file(Graphic(299), AnimFile::Third), BodyKind::Animal);
        assert_eq!(
            BodyKind::in_file(Graphic(300), AnimFile::Third),
            BodyKind::Monster
        );
        assert_eq!(BodyKind::in_file(Graphic(400), AnimFile::Third), BodyKind::Human);
        assert_eq!(
            IndexLayout::in_file(Graphic(150), AnimFile::Third),
            IndexLayout::Low,
            "and an animal below 200 there has no block of its own",
        );

        // The other four are the general rule, which is what `of` is.
        for file in [
            AnimFile::First,
            AnimFile::Fourth,
            AnimFile::Fifth,
            AnimFile::Sixth,
        ] {
            for body in [0u16, 199, 200, 399, 400, 1000] {
                assert_eq!(
                    BodyKind::in_file(Graphic(body), file),
                    BodyKind::of(Graphic(body))
                );
                assert_eq!(
                    IndexLayout::in_file(Graphic(body), file),
                    IndexLayout::of(Graphic(body)),
                );
            }
        }
    }

    /// A base below its own layout's first body is not a block at all. The
    /// reference subtracts anyway and lands in another family's region; the
    /// shipped table asks for exactly that for bodies 95 and 169.
    #[test]
    fn a_layout_applied_below_its_own_first_body_names_no_block() {
        assert_eq!(IndexLayout::Low.base(95), None);
        assert_eq!(IndexLayout::Low.base(199), None);
        assert_eq!(IndexLayout::People.base(399), None);
        assert_eq!(
            IndexLayout::High.base(0),
            Some(0),
            "the high layout starts at body zero"
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
        assert_eq!(
            BodyKind::Monster.standing(),
            AnimationGroup(1),
            "HighAnimationGroup.Stand"
        );
        assert_eq!(
            BodyKind::Animal.standing(),
            AnimationGroup(2),
            "LowAnimationGroup.Stand"
        );
        assert_eq!(
            BodyKind::Human.standing(),
            AnimationGroup(4),
            "PeopleAnimationGroup.Stand"
        );
        assert_eq!(
            BodyKind::Monster.dying(),
            AnimationGroup(2),
            "HighAnimationGroup.Die1"
        );
        assert_eq!(
            BodyKind::Animal.dying(),
            AnimationGroup(8),
            "LowAnimationGroup.Die1"
        );
        assert_eq!(
            BodyKind::Human.dying(),
            AnimationGroup(21),
            "PeopleAnimationGroup.Die1"
        );
        // Walking is the coincidence, and the reason standing is not.
        for kind in [BodyKind::Monster, BodyKind::Animal, BodyKind::Human] {
            assert_eq!(kind.walking(), AnimationGroup(0));
            // Whatever a kind names, it names inside its own table.
            assert!(kind.standing().index() < kind.actions());
            assert!(kind.running().is_none_or(|group| group.index() < kind.actions()));
        }
        assert_eq!(BodyKind::Monster.running(), None, "High has no run");
        assert_eq!(BodyKind::Animal.running(), Some(AnimationGroup(1)));
        assert_eq!(BodyKind::Human.running(), Some(AnimationGroup(2)), "RunUnarmed");
    }

    /// War changes a human's stance and nobody else's, and the two groups it
    /// changes to are inside the human table.
    ///
    /// The same silent failure the test above guards: 7 and 15 exist in all
    /// three numberings, so a war stance handed to a horse would play
    /// `LowAnimationGroup.GetHit` at it for as long as it stood there.
    #[test]
    fn only_a_human_has_a_war_stance() {
        assert_eq!(
            BodyKind::Human.standing_at_war(),
            Some(AnimationGroup(7)),
            "PeopleAnimationGroup.StandOnehandedAttack"
        );
        assert_eq!(
            BodyKind::Human.walking_at_war(),
            Some(AnimationGroup(15)),
            "PeopleAnimationGroup.WalkWarmode"
        );
        for kind in [BodyKind::Monster, BodyKind::Animal] {
            assert_eq!(kind.standing_at_war(), None);
            assert_eq!(kind.walking_at_war(), None);
        }
        // Whatever war names, it names inside the kind's own table — and it
        // names something *different* from the peacetime group, or the whole
        // pair would be a stance nobody can see.
        for kind in [BodyKind::Monster, BodyKind::Animal, BodyKind::Human] {
            assert!(
                kind.standing_at_war()
                    .is_none_or(|group| group.index() < kind.actions())
            );
            assert!(
                kind.walking_at_war()
                    .is_none_or(|group| group.index() < kind.actions())
            );
            assert!(
                kind.standing_at_war()
                    .is_none_or(|group| group != kind.standing())
            );
            assert!(kind.walking_at_war().is_none_or(|group| group != kind.walking()));
        }
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
        assert_eq!(animation_body(Graphic(0x0192)), Graphic(0x0190), "the male ghost");
        assert_eq!(
            animation_body(Graphic(0x0193)),
            Graphic(0x0191),
            "the female ghost"
        );
        for living in [Graphic(0x0190), Graphic(0x0191)] {
            assert_eq!(animation_body(living), living, "a living body is itself");
            assert_eq!(
                animation_body(Graphic(living.0 + 2)),
                living,
                "and its ghost is it"
            );
        }
        assert_eq!(animation_body(Graphic(0x02B6)), Graphic(667));
        assert_eq!(animation_body(Graphic(0x02B7)), Graphic(666));
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
        let table = crate::mobtypes::MobTypes::open(&dir).expect("a readable optional mobtypes.txt");
        for ghost in [0x0192u16, 0x0193] {
            assert!(
                anim.frames(
                    AnimationKey::new(Graphic(ghost), BodyKind::Human.standing(), AnimationDirection(0)),
                    table.get(Graphic(ghost)),
                )
                .expect("reading the index")
                .is_none(),
                "body {ghost:#06x} has frames after all, and the remap is hiding them",
            );
            let drawn = animation_body(Graphic(ghost));
            assert!(
                anim.frames(
                    AnimationKey::new(drawn, BodyKind::Human.standing(), AnimationDirection(0)),
                    table.get(drawn),
                )
                .expect("reading the index")
                .is_some_and(|frames| !frames.is_empty()),
                "body {:#06x} is what a ghost is drawn from and has no frames either",
                drawn.0,
            );
        }
    }

    /// Eight facings, five pictures, and the mirror is the whole difference.
    #[test]
    fn four_facings_are_mirrors_of_the_other_four() {
        assert_eq!(facing(Direction::SouthEast), (AnimationDirection(0), false));
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
        assert_eq!(facing(Direction::NorthWest), (AnimationDirection(4), false));
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

        let frames = decode_body(
            AnimationKey::new(Graphic(400), AnimationGroup(4), AnimationDirection(0)),
            &entry,
        )
        .expect("a well-formed entry");
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
            decode_body(
                AnimationKey::new(Graphic(400), AnimationGroup(4), AnimationDirection(0)),
                &entry,
            ),
            Err(AnimError::Malformed { .. })
        ));
    }

    /// The three ways the index says "nothing here", all of which the client
    /// accepts and any one of which read as data is a seek into the middle of
    /// somebody else's body.
    #[test]
    fn an_absent_entry_is_absent_in_three_shapes() {
        assert!(
            !IdxEntry {
                position: 0,
                size:     0,
            }
            .is_present()
        );
        assert!(
            !IdxEntry {
                position: NO_ENTRY,
                size:     100,
            }
            .is_present()
        );
        assert!(
            !IdxEntry {
                position: 512,
                size:     NO_ENTRY,
            }
            .is_present()
        );
        assert!(
            IdxEntry {
                position: 512,
                size:     100,
            }
            .is_present()
        );
    }

    #[test]
    fn body_def_redirects_a_grey_wolf_to_its_real_animation_body() {
        let body_def = BodyDef::from_text(
            "# original body { visual body } hue\n\
             25 {225} 946\n\
             46 {12, 59} 1106\n\
             not a redirect\n",
        );

        assert_eq!(
            body_def.appearance(Graphic(25)),
            BodyAppearance {
                body: Graphic(225),
                hue:  Hue(946),
            }
        );
        assert_eq!(
            body_def.appearance(Graphic(46)),
            BodyAppearance {
                body: Graphic(12),
                hue:  Hue(1106),
            },
            "the first client-listed alternate is the deterministic base appearance"
        );
        assert_eq!(
            body_def.appearance(Graphic(400)),
            BodyAppearance {
                body: Graphic(400),
                hue:  Hue::NONE,
            }
        );
    }
}
