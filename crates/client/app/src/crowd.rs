//! What the view does not keep: what each mobile was doing a moment ago.
//!
//! `WorldView` is a record of what arrived and deliberately nothing else, so it
//! cannot answer the two questions a picture needs — is this creature walking,
//! and how far into its walk is it. Both are about *history*: a `0x77` says
//! where a body is, and only the previous one says it moved.
//!
//! So this is the layer above the view that ages what it sees. It is small on
//! purpose: it holds a position, a group and a clock per serial, and it decides
//! nothing about the wire and nothing about a GPU.
//!
//! # A step is heard, not seen
//!
//! Nothing on the wire says "stopped walking". A shard sends a `0x77` per step
//! and then silence, so walking is inferred from a step having arrived recently
//! and standing from one not having. [`WALK_HOLD`] is how long "recently" is,
//! and it is one full step on foot rather than a number chosen to look right —
//! a body that took a step less than a step ago has not finished it.
//!
//! # And a step is a distance, not a jump
//!
//! The same history answers the other half: a step that has been running for
//! half its length is a body half a tile along, and drawing it on the tile the
//! packet named teleports it 44 pixels. So the step carries what it left and
//! when, and [`Crowd::glide_for`] turns that into the fraction
//! [`openshard_client_render::mobiles::Glide`] wants. The pixels are the
//! renderer's; the clock is here.
//!
//! # Why it lives here and not in `client/render`
//!
//! It reads `client/net`'s view and produces `client/render`'s `Mobile`. Putting
//! it in the renderer would make the renderer depend on the wire, which is the
//! one thing the crate layout forbids. It is the app's job to join the two, and
//! this is that join — with tests, because it is arithmetic over a clock and not
//! a picture.

use std::collections::HashMap;
use std::time::Duration;

use openshard_client_render::animation::AnimationClock;
use openshard_client_render::follow::Gaze;
use openshard_client_render::mobiles::{
    EquipmentLayer,
    Mobile,
};
use openshard_movement::step_hold;
use openshard_protocol::direction::{
    Direction,
    Facing,
};
use openshard_protocol::feedback::{
    ActionPhase,
    ActionStage,
    Animation,
    AnimationFrameCount,
    BalkState,
    CombatActionBalked,
    CombatActionEnded,
    CombatActionKind,
    CombatActionOutcome,
    CombatActionPhase,
    CombatActionStage,
    DEFAULT_ANIMATION_FRAME_MS,
    HarvestPreview,
    HarvestRefused,
    HarvestToolVisual,
    InterruptReason,
    NewAnimation,
    SwingTiming,
};
use openshard_protocol::mobile::Equipment;
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    Layer,
};
use openshard_protocol::world::Point;
use openshard_tiles::{
    AnimId,
    TileData,
};
use openshard_uofiles::anim::{
    AnimationFrameIndex,
    AnimationGroup,
    BodyKind,
};
use openshard_uofiles::mobtypes::MobTypes;

/// The wire's list, as [`Mobile::equipment`] wants it.
///
/// A worn item's default picture is its own tiledata `AnimID`, not its wire
/// graphic — a different index space, read from `art.mul`'s neighbour rather
/// than `anim.mul`'s — so that resolution happens here, once, rather than at
/// every place `Mobile::equipment` is read. Whether *this* body draws
/// something else again is [`EquipConv`](openshard_uofiles::equipconv::EquipConv)'s
/// question, asked downstream in `client/render`, which is why this module
/// still holds no such table: it needs `tiledata`, not `anim.mul`.
///
/// **The mount is the one layer the file cannot answer for**, and it is resolved
/// from the shared table instead — see [`mount_picture`].
pub fn worn(equipment: &[Equipment], tiledata: &TileData) -> Vec<EquipmentLayer> {
    equipment
        .iter()
        .map(|item| {
            EquipmentLayer {
                graphic: mount_picture(item.layer, item.graphic)
                    .unwrap_or_else(|| tiledata.static_tile(item.graphic.0).anim_id),
                hue:     item.hue,
                // The wire's slot, carried through rather than resolved here: what
                // it decides — hair on a ghost, and the order a paperdoll draws in —
                // is the renderer's, and a layer this end reinterpreted would be a
                // second opinion about a number the shard already stated.
                layer:   item.layer,
            }
        })
        .collect()
}

/// What a saddle draws as: the *creature* under the rider, in the same
/// body-animation space every other layer's `AnimID` is read from — or `None`
/// for any layer that is not a mount, and for a mount item this engine's table
/// does not know.
///
/// A mount is not a picture on the rider's frame; it is a second animated body
/// drawn beneath them (`mobiles::mount_of` is what draws it). The number this
/// puts in the layer is therefore a body id and not a worn item's `AnimID`, and
/// the two spaces happen to be the one `anim.idx` indexes, which is why the
/// field can carry either.
///
/// # Why not tiledata's `AnimID`, like every other layer
///
/// Because on a stock install it is a lie for this block. `0x3E9F` — the
/// ordinary horse — is named "ship" in `tiledata.mul` and carries `AnimID` 820,
/// and `anim.idx` has no animation for body 820 at all: its lookup word is
/// `0xFFFFFFFF`. Read the file and the atlas holds no frame, `place` answers
/// `None`, and the rider floats along with nothing underneath — which is exactly
/// how this was found. The reference client carries the same table for the same
/// reason (`Game/Data/Mounts.cs`), and ours is
/// [`openshard_protocol::mounts`] because the shard needs the other direction of
/// it to equip the saddle in the first place.
fn mount_picture(layer: Layer, graphic: Graphic) -> Option<AnimId> {
    match layer == Layer::MOUNT {
        true => openshard_protocol::mounts::mount_body_for(graphic).map(|body| AnimId(body.0)),
        false => None,
    }
}

/// How long a spoken line stays drawn above its speaker's head, once heard.
///
/// A fixed hold rather than one scaled by the message's length — which is what
/// `Mobile.m_SpeechTime` does in the reference client — because nothing that
/// reaches here carries an expiry: the wire's `0x1C` has none, and neither does
/// [`openshard_protocol::speech::SpokenMessage`]. Long enough to read a short
/// line before it is gone, and this is the one place to widen it once a real
/// expiry exists.
pub const SPEECH_HOLD: Duration = Duration::from_secs(5);

/// How many lines one mobile may have stacked above it at once.
///
/// The reference client stacks and this one did not, which is what made the
/// bug: a single click sends the guild line and then the name as two `0x1C`s,
/// and a map holding one line per speaker showed only the second. Two lines in a
/// row from one NPC had always been losing the first, silently — the click is
/// what made it every time rather than sometimes.
///
/// Bounded because a shard can talk faster than [`SPEECH_HOLD`] retires lines,
/// and a body under thirty of them is a wall of text with a mobile somewhere
/// behind it. The oldest is dropped, so what a player sees is the most recent
/// few.
pub const SPEECH_STACK: usize = 4;

/// One line, and when [`Crowd::hear`] recorded it.
#[derive(Clone, Debug)]
struct Speech {
    text:    String,
    font:    Font,
    hue:     Hue,
    started: Duration,
}

/// How a body's picture is allowed to lag the walk it is doing.
///
/// The ease into and out of a walk, and the whole of `docs/client/design_camera_rig.md` D10 in one
/// number. A step cannot itself be eased — a body has to cross one tile per hold
/// and no profile that starts at rest does that without going faster than a walk
/// somewhere in the middle — so what an ease *is* is a lag, and this is how much
/// of one the drawn body may carry.
///
/// **Deliberately not a [`Rig`](openshard_client_render::follow::Rig) field.** A
/// rig is the parameter set of the *eye*, and the eye's pipeline begins by being
/// handed a body to look at; this is a property of that body, one stage earlier
/// and one subject over. The two were one struct for a day, on the argument that
/// they are tuned in the same sitting, and it read as though the camera were
/// what moved the character. The arithmetic is still shared —
/// [`Gaze::eased_towards`] is the eye's own [`approach`] per channel — because
/// two dampers is what one pipeline exists to refuse.
///
/// [`approach`]: openshard_client_render::follow::approach
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ease {
    /// Seconds for the picture to close all but `1/e` of its gap to where the
    /// walk says the body is. Zero draws it exactly there.
    pub tau: f32,
}

impl Ease {
    /// No ease: the picture is the walk, to the pixel, every frame.
    ///
    /// What this client did before there was an ease at all, and what every
    /// measurement of the *walk* runs at — a body deliberately behind is not a
    /// body that failed to keep up, and only a harness that says which one it is
    /// measuring can tell them apart.
    pub const NONE: Self = Self { tau: 0.0 };

    /// The one the window opens with.
    ///
    /// `0.08` is chosen by what it costs, the way `Rig::LIFT`'s constant is. A
    /// walk is 78 pixels a second, so the picture settles `78 * 0.08` behind —
    /// 6.3 pixels, under a seventh of a tile, small enough that nobody can see
    /// the body is not centred on its tile and large enough that the start and
    /// the stop are visibly eased. The ease-out is the same number spent in
    /// reverse and costs no second rule. `dst::dump_the_ramp` is the table it
    /// was read off; `docs/client/evidence/2026-08-14-the-camera-rig-record.md`
    /// C3 records the sitting.
    pub const WALK: Self = Self { tau: 0.08 };
}

/// Where between two tiles a body is, and how far along.
///
/// Was a public type in `client/render` until the renderer stopped deriving a
/// position at all (D10): a sprite is drawn at [`Mobile::drawn`], and how that
/// number came about — a step's clock, an ease's filter — is this layer's
/// business and nobody else's. So it is private here, and the interpolation with
/// it.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Glide {
    /// Where the body was drawn when the step began.
    ///
    /// **Not the tile it stepped off.** A step begins when a packet arrives, and
    /// a packet arrives when the wire and the event loop between them get round
    /// to it; the body at that instant is wherever the previous step had reached.
    /// Anchoring to the tile boundary instead is a discontinuity of exactly the
    /// arrival's error, once per tile.
    from:     Gaze,
    /// How far into the step: `0.0` at [`Glide::from`], `1.0` at the tile.
    progress: f32,
}

/// A step in progress: where it came from, when it started, and how long it
/// takes.
#[derive(Clone, Copy, Debug)]
struct Step {
    /// Where the body was drawn when the step began, or `None` when the move was
    /// not a step at all.
    ///
    /// Absent for a jump of more than one tile — a gate, a recall, a `0x22`
    /// putting a mispredicted body back. Interpolating one of those slides the
    /// character across the map over the length of a step, which is a far
    /// stranger picture than the teleport it is hiding.
    ///
    /// The drawn position and not the tile stepped off: see
    /// [`openshard_client_render::mobiles::Glide::from`], which this becomes.
    /// The two are the same number when the previous step ended exactly as this
    /// one began, and every millisecond they differ by is a jump on screen.
    from:    Option<Gaze>,
    /// The tile it was standing on when the step began.
    ///
    /// The *tile* and not the pixels, which is exactly the distinction
    /// [`Step::from`] is on the other side of: the picture starts wherever the
    /// last crossing had got to, and the ordering is a question about grid cells
    /// with no fractions in it. Read only while the glide is running, so its
    /// value on a move that was not a step (`from` absent) is never asked for.
    was:     Point,
    /// When it started, on [`Crowd::now`]'s clock: the instant it was heard.
    ///
    /// What is *not* read off the arrival is when it ends — for the body this
    /// client commands, that comes from the cadence. See [`crossing`].
    started: Duration,
    /// How long it takes — see [`glide_time`]. Never zero, which is what lets
    /// [`Tracked::glide`] divide.
    takes:   Duration,
}

/// How long to spend crossing a tile, given the pace the wire claims and how
/// long ago the previous step was heard.
///
/// # Why the nominal length is not enough
///
/// A glide has to *end* exactly when the next step begins, or the walk is not
/// continuous: finish early and the body stands on its tile until the next
/// packet, finish late and the next step yanks it forward from wherever it had
/// got to. Both read as a stutter once a tile, which is the whole complaint.
///
/// [`WALK_HOLD`] is how long a step takes in theory, and it is the right answer
/// only for the first one. After that the *observed* gap between two steps is a
/// far better prediction of the gap to the next: it already contains the round
/// trip, the shard's own tick granularity, and — for everyone who is not this
/// client — whatever pace that creature actually walks at, which for an NPC is
/// nothing this end can look up. So the second step onwards is glided over the
/// measured gap.
///
/// Believed only within half and double the nominal length. Outside that band
/// the measurement is not a pace at all: a gap of four seconds is a body that
/// had stopped and started again, and a gap of nothing is two steps arriving in
/// one packet burst. Both are answered with the wire's own claim, which is at
/// least a walking speed.
fn glide_time(nominal: Duration, since: Option<Duration>) -> Duration {
    match since {
        Some(gap) if gap >= nominal / 2 && gap <= nominal * 2 => gap,
        // Nothing worth measuring: a body that was standing, one heard from for
        // the first time, or a gap that is not a pace.
        _ => nominal,
    }
}

/// How long a body keeps *playing* its walk after a step, given how long that
/// step takes to cross its tile.
///
/// Half a step longer than the crossing. The two used to be one number, and that
/// is a flicker: the hold expires the instant the body lands, so any latency at
/// all leaves a frame or two of standing between two steps — and standing is a
/// different animation group, so the walk's clock is restarted at frame zero
/// every single tile. Half a step of slack costs a body that has genuinely
/// stopped 200ms of walking on the spot, which nobody notices, and it is what
/// the reference client's own `m_AnimationInterval` slack is for.
///
/// What the slack must *not* do is play. See [`Tracked::striding`].
fn animation_hold(takes: Duration) -> Duration {
    takes * 3 / 2
}

/// Translate the semantic categories of `0xE2` into the groups in the classic
/// animation files.  The packet intentionally has no body-specific group: the
/// body already on screen supplies that half of the key.
///
/// The timings mirror the shard's `0x6E` fallback.  The group lookup remains
/// here because only the client has the currently displayed body's kind.
fn modern_action(kind: BodyKind, animation_type: u16, sub_action: u16) -> Option<(u16, u16)> {
    let human = matches!(kind, BodyKind::Human);
    // The swing every creature has, in its own numbering — see
    // [`BodyKind::attacking`]. Four frames, matching the shard's `0x6E`
    // fallback for the same action.
    let creature_attack = (u16::from(kind.attacking().index()), 4);
    match animation_type {
        // Attack. The sub-action is ServUO's weapon motion; harvesting uses the
        // same ids because it deliberately asks for a particular tool swing.
        0 => {
            Some(match (human, sub_action) {
                (true, 1) => (18, 7), // bow
                (true, 2) => (19, 7), // crossbow
                (true, 3) => (11, 5), // one-handed bash / mine
                (true, 4) => (9, 7),  // one-handed slash
                (true, 5) => (10, 7), // one-handed pierce
                (true, 6) => (12, 5), // two-handed bash / fish
                (true, 7) => (13, 6), // two-handed slash / chop
                (true, 8) => (14, 7), // two-handed pierce
                (true, _) => (31, 7), // wrestle / unknown
                (false, _) => creature_attack,
            })
        }
        3 => {
            Some((
                u16::from(kind.dying().index()),
                // A person falls in six pictures and a creature in four.
                if human { 6 } else { 4 },
            ))
        }
        9 => Some(if human { (32, 5) } else { creature_attack }), // bow
        // Spell. An animal's numbering has no cast at all, and 12 there is
        // `Die2` — see [`BodyKind::casting`].
        11 => {
            Some(match kind.casting() {
                Some(group) => (u16::from(group.index()), 7),
                None => creature_attack,
            })
        }
        _ => None,
    }
}

/// Turn an on-foot human attack into the corresponding mounted action.
///
/// Both animation packets ultimately reach [`Crowd::play`]: `0xE2` first maps
/// its weapon sub-action through [`modern_action`], while `0x6E` already names
/// the on-foot group directly.  Applying the saddle rule at that common seam
/// keeps both protocol paths identical and also covers harvest previews, which
/// carry the same mining/chopping/fishing attack groups without an `0xE2`.
///
/// The frame counts belong to the mounted art, not to the group it replaces.
/// In the classic human table the generic attack, bow and slap-horse groups
/// have five frames; the mounted crossbow has seven.
fn action_on_mount(
    kind: BodyKind,
    mounted: bool,
    action: u16,
    frames: AnimationFrameCount,
) -> (u16, AnimationFrameCount) {
    if !mounted || !matches!(kind, BodyKind::Human) {
        return (action, frames);
    }

    match action {
        18 => (27, AnimationFrameCount(5)),     // OnmountAttackBow
        19 => (28, AnimationFrameCount(7)),     // OnmountAttackCrossbow
        31 => (29, AnimationFrameCount(5)),     // OnmountSlapHorse (unarmed)
        9..=14 => (26, AnimationFrameCount(5)), // OnmountAttack
        _ => (action, frames),
    }
}

/// One mobile's history: where it was, what it is playing, and since when.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    /// Where the last packet put it.
    at:                Point,
    /// Which way it was last seen facing.
    ///
    /// Kept for one rule only: a facing change with no position change is a
    /// turn, and a turn is a step that covers no ground — see [`Crowd::see`].
    /// The run flag is not kept with it, because it belongs to the step being
    /// taken rather than to the body.
    facing:            Direction,
    /// Which body it was last seen as. Kept because a walk that ends has to
    /// know what "standing" means, and a horse and a player stop into different
    /// group numbers — see [`BodyKind::standing`].
    body:              Graphic,
    /// Which numbering [`Tracked::body`]'s actions are named in.
    ///
    /// Kept beside the body rather than derived at each use, because it is not
    /// derivable from the body: it is a row of the install's `mobtypes.txt`,
    /// and the range rule that looks like a derivation calls every wolf, bear
    /// and cougar a monster. Written wherever `body` is written, from the table
    /// the caller holds, so the group numbers this history plays are the ones
    /// the creature's own family names.
    kind:              BodyKind,
    /// Whether it stands in war mode.
    ///
    /// Kept for exactly [`Tracked::body`]'s reason, and it is the same sentence:
    /// a walk that ends has to know what "standing" means, and a body at war
    /// stands in a different group from the same body at peace. Every packet
    /// that describes a mobile carries this — it is a bit of the `0x77`/`0x78`
    /// flag byte — so it is restated on each [`Crowd::see`] rather than
    /// remembered from whenever the stance last changed.
    war:               bool,
    /// Whether it is in the saddle.
    ///
    /// Kept for [`Tracked::war`]'s own reason: [`Crowd::advance`] switches a
    /// finished walk to standing on the clock alone, with no fresh packet to
    /// read it from, so the group that decision lands on has to already know
    /// whether this body is riding. A mount is worn as an ordinary equipment
    /// layer (`Layer::MOUNT`), which `Tracked` otherwise never sees — equipment
    /// belongs to the caller, see [`Mobile::equipment`] — so the caller states
    /// it here the same way it states `war`, rather than this module reaching
    /// into a list it does not otherwise read.
    mounted:           bool,
    /// Whether this is an item corpse rather than a living mobile.
    ///
    /// Corpses borrow the mobile renderer because `0x2006`'s payload is a body
    /// id, not a stack count. Unlike a live body they hold the final frame of
    /// their death group forever.
    corpse:            bool,
    /// Whether a newly-created corpse is still playing the death that made it.
    ///
    /// Combat sends the death animation immediately before the world tick
    /// replaces a creature with its corpse item.  The two packets normally
    /// reach one presentation batch, so drawing the corpse's final frame right
    /// away erases the animation before a frame can show it.  This flag keeps
    /// the copied action alive under the corpse item's serial, then turns it
    /// into the ordinary held corpse pose when the action finishes.
    settles_as_corpse: bool,
    /// Which animation group is playing.
    group:             AnimationGroup,
    /// The step it is in the middle of.
    ///
    /// `None` for a body that is standing. Not an "unknown": a standing body
    /// genuinely has no step to finish, which is what [`Option`] is for here.
    step:              Option<Step>,
    /// When the previous step was heard, kept after that step has finished.
    ///
    /// What the gap in [`glide_time`] is measured against, which is why it
    /// outlives [`Tracked::step`]: the pace a body is walking at is a property
    /// of the last two packets, not of the one still being drawn. `None` until a
    /// body has been seen to move at all.
    stepped_at:        Option<Duration>,
    /// Where the sprite is actually drawn, which trails [`Tracked::gaze_at`] by
    /// whatever the ease is holding.
    ///
    /// The filter's state, and it is per body because every body is eased and
    /// there is one of these per body — `docs/client/design_camera_rig.md` D10. Equal to the
    /// unfiltered position under [`Ease::NONE`], which is what makes the
    /// baseline still exactly the baseline.
    drawn:             Gaze,
    /// Its own animation clock.
    ///
    /// Per mobile, and reset when the group changes: one clock for everybody
    /// makes a standing crowd breathe in unison, which is wrong and looks it,
    /// and a clock carried across a group change starts the new animation
    /// wherever the old one happened to be.
    clock:             AnimationClock,
    action:            Option<ActionAnimation>,
    /// A backpack axe temporarily borrowed by the harvest animation's picture.
    harvest_tool:      Option<EquipmentLayer>,
}

#[derive(Clone, Copy, Debug)]
struct ActionAnimation {
    /// A fall is terminal: a late ordinary action must not pull a dead body
    /// back upright before its corpse has claimed the final frame.
    death:      bool,
    frames:     AnimationFrameCount,
    repeats:    u16,
    forward:    bool,
    repeat:     bool,
    delay:      Duration,
    /// Exact duration of one complete cycle when the server supplied one.
    /// This lets a six-frame chop take 1.6 seconds instead of six default
    /// 80ms frames, while still using the same frame machine as locomotion.
    cycle:      Option<Duration>,
    /// Exact server interval for a timed action. It is rounded up to a complete
    /// animation loop before the state gives the body back to standing.
    duration:   Option<Duration>,
    /// A cursor click has started this locally, but the shard has not yet
    /// accepted or refused it. It must keep looping while that answer is in
    /// flight; otherwise latency would make the predicted action end early.
    optimistic: bool,
    /// This action was started by the harvesting protocol, not ordinary combat.
    harvest:    bool,
    elapsed:    Duration,
}

impl ActionAnimation {
    fn frame(self, available: u16) -> u16 {
        let frames = self.frames.0.clamp(1, available);
        let frame = match (self.cycle, self.duration) {
            // Harvest owns an explicit loop length. It remains a continuously
            // cycling action even while waiting for the authoritative result.
            (Some(cycle), _) => {
                let cycle_ns = cycle.as_nanos().max(1);
                let elapsed_in_cycle = self.elapsed.as_nanos() % cycle_ns;
                (elapsed_in_cycle.saturating_mul(u128::from(frames)) / cycle_ns) as u16
            }
            // A combat timing names one wind-up to its impact, and the art is
            // spread evenly across it: the gesture takes exactly as long as the
            // shard says the action does.
            (None, Some(duration)) => timed_swing_frame(self.elapsed, duration, frames),
            (None, None) => {
                let ticks = self.elapsed.as_nanos() / self.delay.as_nanos().max(1);
                (ticks % u128::from(frames)) as u16
            }
        };
        if self.forward { frame } else { frames - 1 - frame }
    }

    /// Whether this timed action has reached the next complete animation loop.
    ///
    /// The action may only hand the body back on this boundary.  That makes a
    /// resource arrive after a whole axe stroke, even if a future server timing
    /// is not an exact multiple of the client's frame cadence.
    fn finished(self) -> bool {
        let Some(duration) = self.duration else {
            if self.optimistic {
                return false;
            }
            let ticks = self.elapsed.as_nanos() / self.delay.as_nanos().max(1);
            return ticks >= u128::from(self.frames.0.max(1)) * u128::from(self.repeats.max(1));
        };
        let cycle = self
            .cycle
            .unwrap_or_else(|| self.delay.saturating_mul(u32::from(self.frames.0.max(1))));
        let cycle_ns = cycle.as_nanos().max(1);
        let duration_ns = duration.as_nanos();
        let complete_at = duration_ns.div_ceil(cycle_ns).saturating_mul(cycle_ns);
        self.elapsed.as_nanos() >= complete_at
    }
}

/// Place sparse combat art evenly along the server-owned wind-up timeline.
///
/// **The gesture takes exactly as long as the action does**, which is the whole
/// of the rule: the shard says a shot is 1600ms and the seven frames of the bow
/// are spread across 1600ms, so a body that is preparing something is a body
/// that is visibly moving for the whole of it.
///
/// It used to be staged instead — one ordinary cadence into an "anticipation"
/// pose, that single frame *held* for as long as it took, then the remaining
/// frames flicked through at 80ms each immediately before the impact. The
/// arithmetic of that on a 2.5-second draw was 2020ms frozen on frame 1 and
/// 400ms of the entire shot, and what it drew was a statue that twitched: it was
/// reported, in as many words, as *"the character just stands there and then
/// suddenly fires"*. The bar beside it was meanwhile filling honestly, so the
/// picture also contradicted itself — a body that had not moved under a bar at
/// sixty percent.
///
/// The staging was written to stop a slow weapon finishing its visible swing in
/// half a second and then idling invisibly, which is a real defect and the
/// opposite end of this one. Spreading solves it too, and without inventing a
/// pose the art never had.
///
/// Harvest previews provide their own repeating cycle and deliberately do not
/// take this path: an axe on a tree is many strokes, not one.
fn timed_swing_frame(elapsed: Duration, duration: Duration, frames: u16) -> u16 {
    let progress = elapsed.as_nanos().saturating_mul(u128::from(frames)) / duration.as_nanos().max(1);
    u16::try_from(progress.min(u128::from(frames - 1))).unwrap_or(frames - 1)
}

/// Who a tracked body is.
///
/// `None` is this client's own body *before a shard has named it* — the offline
/// map viewer walks a placeholder around with no serial, because nobody has
/// given it one. Absent rather than zero: a serial of zero is a real wire value
/// meaning "nothing", and a made-up one would collide with a real mobile the
/// moment the client logs in.
pub type Who = Option<Serial>;

/// An explicit movement command for the client-controlled body.  Other
/// mobiles still arrive as position snapshots through [`Crowd::see`], but the
/// player must not make Crowd infer a transition from two independent stores.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandedMove {
    /// Start or refresh a transition whose logical endpoints are already owned
    /// by the movement core.
    Transition { from: Point, to: Point },
    /// Keep the body at a known standing tile (including a turn in place).
    Standing { at: Point },
    /// Put the body down without interpolation after reconciliation.
    Snap { at: Point },
}

/// A death heard of, waiting for the corpse it was promised.
#[derive(Clone, Copy, Debug)]
struct Fall {
    /// The body mid-fall, as it stood when the shard named its corpse.
    body:  Tracked,
    /// When that was, so an unclaimed pairing does not live for ever.
    heard: Duration,
}

/// How long a death waits for its corpse before the pairing is forgotten.
///
/// The corpse normally arrives in the same batch as the death, and always within
/// a tick of it. This is not a timeout the mechanism relies on — it is the bound
/// on a pairing whose corpse never comes, because it was laid out of view or the
/// shard changed its mind. Longer than any death animation, and short enough that
/// a serial cannot plausibly be reused inside it.
const FALL_HELD: Duration = Duration::from_secs(5);

/// How long the way an action ended stays on screen after it is over.
///
/// The bar answers *what is happening*; this is what answers *what just
/// happened*, and the second question is the one a fight leaves behind. Long
/// enough to read a word at a glance, and short enough that it is the *last*
/// blow being reported and not one from an exchange ago.
const OUTCOME_HOLD: Duration = Duration::from_millis(1_200);

/// How long a bar that has run out of interval is still drawn, waiting for the
/// shard to say how it ended.
///
/// **The interval is a prediction; the ending is a fact, and only one of them is
/// ours.** The bar used to vanish the instant its own arithmetic said the impact
/// was due, which makes the picture wrong whenever the two clocks disagree by
/// any amount at all — and they always disagree, because the shard measures in
/// ticks it may be late delivering and this measures in the wall clock of a
/// frame. A shard running even slightly behind its own tick rate therefore blanks
/// the tail of *every single action*: a bar, then nothing, then the next bar.
///
/// So a finished bar is held, full, until the ending arrives. It reads correctly
/// — a full bar is a blow that is due — and the timeout behind it goes back to
/// being what it was always described as: a bound on a leak, for a body that
/// walked out of range mid-swing and will never be told about again.
const RUNNING_GRACE: Duration = Duration::from_secs(3);

/// One combat action a body is part way through, as the wire told it.
///
/// The wire's own [`ActionPhase`] is kept whole rather than unpacked into a flag
/// beside a number: it already says which of the two intervals it is carrying —
/// how long an armed action may wait, how long a released one takes to land —
/// and a second local copy of that distinction would be a second thing to keep
/// in step with the shard.
#[derive(Clone, Copy, Debug)]
struct PendingAction {
    /// What the impact will do.
    kind: CombatActionKind,
    /// The phase this body entered, and the interval that phase measures.
    phase: ActionPhase,
    /// The crowd's own clock when the phase packet arrived. The interval is
    /// measured from here rather than from a server tick, for the reason every
    /// other clock in this module is local: what is being drawn is a picture
    /// ageing on this screen.
    started: Duration,
    /// Which named stretch of the action the shard last said it was in.
    ///
    /// Told rather than derived, and that is the whole point of the extra
    /// packet: where a draw stops and an aim begins is an operator setting on
    /// the shard, so a client reading it off its own bar's percentage would be
    /// stating a fact nobody gave it — and would be wrong on every shard that
    /// retuned the shares.
    stage: ActionStage,
    /// This release follows a completed held draw rather than a fresh commit.
    ///
    /// The release packet carries only the time until impact — deliberately, as
    /// that is the interval its bar measures.  It cannot make the earlier draw
    /// disappear from the picture, though: a held bow that finds its target
    /// spends only the short loose, not another full draw.  Keep that fact with
    /// the locally continuous action so the HUD can name both states beside one
    /// another.  A watcher who first sees the body during the loose has no
    /// earlier phase to preserve and says only "loosing", which is the honest
    /// limit of the packet it received.
    released_from_held_draw: bool,
}

/// What this client last heard about one body's fighting: what it is preparing,
/// and how the one before that ended.
///
/// **Two facts side by side and not two states**, which is a correction of the
/// obvious design and the reason is measurable. A fighter's next gesture opens
/// on the very tick the last one lands — `Combat::next_swing` is still the
/// impact, see `docs/combat/evidence/2026-08-27-the-action-phases.md`'s Ф1 backlog
/// — so an outcome that were
/// merely *replaced* by the next commit would be on screen for one frame at
/// most, and the word "hit" would be legible only for the last blow of a fight.
/// The two are therefore remembered independently, and an exchange reads as a
/// bar filling with the previous blow's verdict still standing beside it.
#[derive(Clone, Copy, Debug)]
struct ActionRecord {
    /// The action being prepared now. `None` for a body between gestures, which
    /// is a real state and not a missing value: it is the picture of somebody
    /// who has just landed a blow and not yet begun the next.
    running: Option<PendingAction>,
    /// How the last one ended and when, held for [`OUTCOME_HOLD`] from then.
    /// `None` until this body has finished an action in this client's sight.
    ended:   Option<(CombatActionOutcome, Duration)>,
    /// What is stopping this body from beginning one at all.
    ///
    /// The third fact, and unlike the other two it has **no clock**: an outcome
    /// is a message and fades, a running action arrives with its own interval,
    /// and this is a *standing condition* — an archer whose quarry went round a
    /// corner is held up for exactly as long as the corner is there. The shard
    /// sends it on the edge in both directions, so what ends it is being told it
    /// ended, and nothing else.
    balked:  Option<InterruptReason>,
}

/// What a body's combat action looks like right now, for whoever draws it.
///
/// The projection of [`ActionRecord`] the HUD is owed: the wire's durations are
/// already spent against this client's clock, and what is left is what a picture
/// is made of. Never handed out with both halves empty — see
/// [`Crowd::preparing`], which answers `None` for a body with nothing to say.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ActionProgress {
    /// What the impact will do and how far along it is, for a body part way
    /// through an action.
    pub running: Option<RunningAction>,
    /// How the last one ended, while that is still worth saying.
    ///
    /// This is the half of a fight the client could never draw: a blow that
    /// vanished said nothing, and *"it landed"*, *"it missed"* and *"the wall
    /// got in the way"* were the same picture — nothing at all.
    pub ended:   Option<CombatActionOutcome>,
    /// What is stopping this body from starting anything, if something is.
    ///
    /// The gap the other two left: a fighter who *wants* to act and cannot was
    /// drawn exactly like a fighter standing about, for as long as the obstacle
    /// lasted. An archer holding at a target behind a wall is the case a player
    /// meets first, and it read as the shard having quietly stopped.
    pub balked:  Option<InterruptReason>,
}

/// An action part way through, as a picture wants it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RunningAction {
    /// A drawn bow is not a raised axe.
    pub kind: CombatActionKind,
    /// Which half of the action's life this is.
    pub fill: ActionFill,
    /// Which named stretch of it the shard last announced.
    pub stage: ActionStage,
    /// Whether this short release follows a bow that was already held at full
    /// draw on this client.
    pub released_from_held_draw: bool,
}

/// How much of a preparation bar is filled, which is a different question in
/// each phase.
///
/// Not a fraction and a flag: an armed action has an endurance, but the fraction
/// of it that has gone is *not* what a watcher is being told — the picture is a
/// fighter holding something ready, and it is held until the world releases it
/// or the arm gives out.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ActionFill {
    /// Raising/loading: `filled` runs from the nock to the full draw.
    Arming { filled: f32 },
    /// Waiting on the world. The bar is held rather than filling.
    Armed,
    /// Landing: `filled` runs from `0.0` at the release to `1.0` at the impact.
    Releasing { filled: f32 },
}

/// Everyone on screen, aged.
#[derive(Clone, Debug)]
pub struct Crowd {
    tracked: HashMap<Who, Tracked>,
    /// The deaths this client was told about, by the corpse each one leaves.
    ///
    /// A death is two unrelated facts on the wire — a mobile stops being drawn,
    /// an item appears — and `0xAF` is the one packet that pairs them. It
    /// arrives while the fall is still playing, so the falling body is lifted
    /// out of [`Crowd::tracked`] and kept here until its corpse is drawn; that
    /// way the hand-off survives the mobile being pruned in between, which is
    /// what happens when the removal and the corpse land in different batches.
    ///
    /// Keyed by the *corpse's* serial, because that is what asks for it. An
    /// entry nobody claims is dropped by [`FALL_HELD`].
    falls: HashMap<Serial, Fall>,
    /// The lines each serial was heard saying, oldest first, each still within
    /// [`SPEECH_HOLD`]. Separate from `tracked`: the system talks (`who` is
    /// `None` for it too, same as the offline placeholder) and a body nobody
    /// has otherwise seen can still say something the moment it is heard from.
    ///
    /// A stack and not one line: see [`SPEECH_STACK`].
    speech: HashMap<Who, Vec<Speech>>,
    /// Server-owned duration for the immediately following action, by mobile.
    swing_timings: HashMap<Serial, Duration>,
    /// What each body is part way through committing, by the actor the phase
    /// packet named.
    ///
    /// Keyed by [`Serial`] and not by [`Who`]: an action always belongs to a
    /// body the shard has named, so there is no phase packet the offline
    /// placeholder could ever be the actor of.
    ///
    /// Separate from `tracked`'s `action`, which is an *animation*: the two are
    /// told by different packets, they end at different moments — a landed blow
    /// keeps swinging through its last frames while its preparation is over —
    /// and one of them is the shard's authority about a fight where the other is
    /// this client's picture of a body.
    actions: HashMap<Serial, ActionRecord>,
    /// Tool pictures received just before their paired action packet.
    pending_harvest_tools: HashMap<Serial, EquipmentLayer>,
    /// Real time since this crowd was built. Its own clock rather than an
    /// `Instant`, so every rule here can be tested by handing it durations.
    now: Duration,
    /// Whose steps this client *commands* rather than merely hears about.
    ///
    /// Our own body: the offline placeholder to begin with, and our serial from
    /// the moment a shard names us. The distinction is a pace: see
    /// [`glide_time`], and [`Crowd::commanding`] for why one measurement is
    /// right and the other is not.
    ///
    /// A [`Who`] and not an `Option<Who>`, which is to say there is no state
    /// where this client commands nobody. The placeholder's steps are always
    /// ours to issue — nothing else can move it — so `None` is not "not named
    /// yet", it is the body we walk before a shard has given it a serial. As an
    /// `Option<Who>` it had to be armed by hand, which meant the offline walk
    /// was measured through the event loop's wake jitter on every path that
    /// forgot to.
    commanded: Who,
    /// How far the drawn bodies may lag the walk they are doing. See [`Ease`].
    ease: Ease,
    /// Which animation family each body id belongs to, from the install's
    /// `mobtypes.txt`.
    ///
    /// **Here because this is where group numbers are chosen.** A walk that
    /// ends picks a standing group, a death picks a dying one, and both are
    /// numbered differently for a monster, an animal and a person — so the
    /// table has to be reachable from the same place that decision is. Held
    /// rather than passed in at each of the eleven such decisions, for the
    /// reason `WorldState` holds its tile table: it is one fact about the
    /// install, and a caller that could pass a different one at each call is a
    /// caller that eventually will.
    ///
    /// **The client's only copy.** `Anim` deliberately does not keep one — it
    /// takes the resolved [`openshard_uofiles::anim::IndexLayout`] at each read
    /// — so this is the single owner of the parsed file on the client side, and
    /// the renderer borrows it through [`Crowd::mob_types`] to address the
    /// index with.
    ///
    /// Empty until [`Crowd::read_mob_types`], which is what every test that
    /// does not name an install gets.
    mob_types: MobTypes,
}

impl Default for Crowd {
    /// A crowd nobody is easing: the picture is the walk, to the pixel.
    ///
    /// [`Ease::NONE`] for the reason `Follower` starts at `Rig::HARD` — a test
    /// that does not mention an ease is measuring the walk and not a filter.
    fn default() -> Self {
        Self {
            tracked: HashMap::new(),
            falls: HashMap::new(),
            speech: HashMap::new(),
            swing_timings: HashMap::new(),
            actions: HashMap::new(),
            pending_harvest_tools: HashMap::new(),
            now: Duration::ZERO,
            commanded: None,
            ease: Ease::NONE,
            // The install has not been read yet, and an empty table answers
            // every body from the range rule — which is what a crowd built
            // without one is entitled to assume. See [`Crowd::read_mob_types`].
            mob_types: MobTypes::empty(),
        }
    }
}

impl Crowd {
    /// Project one explicit player-motion command into Crowd's clocks.
    ///
    /// The method deliberately delegates clock arithmetic to [`Self::see`] and
    /// [`Self::snap`]; its contract is ownership, not a second interpolation
    /// implementation. Its explicit source lets it recover if a blocked frame
    /// advanced more than one core transition before presentation ran again.
    // One argument per fact the wire carried about the mobile, [`see_inner`]'s
    // own reason: there is no struct here that is not just this list with a
    // name.
    #[allow(clippy::too_many_arguments)]
    pub fn command(
        &mut self,
        who: Who,
        command: CommandedMove,
        body: Graphic,
        facing: Facing,
        hue: Hue,
        war: bool,
        mounted: bool,
    ) -> Mobile {
        match command {
            CommandedMove::Transition { from, to } => {
                debug_assert_ne!(from, to, "a transition must cross a tile");
                self.see_inner(who, to, body, facing, hue, war, mounted, Some(from))
            }
            CommandedMove::Standing { at } => self.see_inner(who, at, body, facing, hue, war, mounted, None),
            CommandedMove::Snap { at } => self.snap(who, at, body, facing, hue, war, mounted),
        }
    }

    /// Ease every body by a different amount from now on.
    ///
    /// The eased positions are deliberately *not* reset: a body is where it is,
    /// and a swap that teleported every sprite onto its tile would be a jump per
    /// mobile at the exact moment somebody drags a slider. A tau that just went
    /// to zero closes the gap on the next frame anyway, which is one frame of
    /// catching up and is what "no ease" means.
    pub fn set_ease(&mut self, ease: Ease) {
        self.ease = ease;
    }

    /// What it is easing by, for the panel that edits it.
    pub const fn ease(&self) -> Ease {
        self.ease
    }

    /// Give this crowd the install's `mobtypes.txt`, so the group numbers
    /// chosen here are the ones each creature's own family names.
    ///
    /// The file is read where every other client file is — see `lib.rs` — and
    /// handed over whole. An install that ships none hands over an empty table,
    /// which leaves every answer on the body-id range rule.
    pub fn set_mob_types(&mut self, mob_types: MobTypes) {
        self.mob_types = mob_types;
    }

    /// The table this crowd chooses group numbers with, for the renderer that
    /// has to address the index with the same answer. See the field.
    pub const fn mob_types(&self) -> &MobTypes {
        &self.mob_types
    }

    /// Name the body this client walks itself.
    ///
    /// Everybody else's pace has to be *measured*, because nothing on the wire
    /// says how fast a creature walks. Our own we already know: `steer.rs` sends
    /// one step every hold and `client/net` predicts it here the same
    /// millisecond, so the nominal length is not an estimate of that walk — it
    /// is the walk. Measuring it anyway feeds the event loop's own wake jitter
    /// into the crossing *length*, and consecutive gaps jitter in opposite
    /// directions (late, then early), so the estimate is worse than the number
    /// it replaced: the body arrives early, stands, and is yanked. The walk
    /// oracle in `dst.rs` is what put a number on it.
    ///
    /// Said when a shard names our body, and not before: a client without one is
    /// already commanding the offline placeholder — see the field.
    pub fn commanding(&mut self, who: Who) {
        self.commanded = who;
    }

    /// Move every clock forward, and stop whoever has finished their step.
    pub fn advance(&mut self, dt: Duration) {
        let was = self.now;
        self.now += dt;
        let (now, ease) = (self.now, self.ease);
        // A pairing whose corpse never arrived. See [`FALL_HELD`] — this is a
        // bound on a leak, not part of how the hand-off works.
        self.falls
            .retain(|_, fall| now.saturating_sub(fall.heard) < FALL_HELD);
        for tracked in self.tracked.values_mut() {
            let action_done = if let Some(action) = tracked.action.as_mut() {
                action.elapsed += dt;
                !action.repeat && action.finished()
            } else {
                false
            };
            if action_done {
                tracked.action = None;
                tracked.harvest_tool = None;
                if tracked.settles_as_corpse {
                    tracked.settles_as_corpse = false;
                    tracked.corpse = true;
                    // The action was already the body's Die1 group.  Leaving
                    // that group in place makes `frame_for` hold its last
                    // frame from now on.
                } else {
                    let standing = tracked.standing_group();
                    tracked.change_to(standing);
                }
            }
            // Only the part of the span the body was actually covering ground
            // in — see [`Tracked::mid_stride`]. Not a whole-span test against
            // either end: a frame that straddles the moment the crossing ended
            // would either play the whole span or none of it, and at 400ms a
            // step the second one freezes the entire walk.
            tracked.clock.advance(tracked.striding(was, dt));
            // The ease, on the position the step arithmetic just produced. After
            // the clock and before anything is read, so every reader this frame
            // sees one answer — see [`Tracked::drawn`].
            tracked.drawn = tracked.drawn.eased_towards(tracked.gaze_at(now), ease.tau, dt);
            // The *animation* outlives the crossing by half a step — see
            // `animation_hold`. The glide itself ends on time; this is only what
            // decides when the walk gives way to standing.
            if tracked
                .step
                .is_some_and(|step| now >= step.started + animation_hold(step.takes))
            {
                tracked.step = None;
                let standing = tracked.standing_group();
                tracked.change_to(standing);
            }
        }
    }

    /// What the server says about a body now, folded into what it said before.
    ///
    /// Returns the mobile to draw. The frame is left at zero — the atlas is what
    /// knows how many frames there are, and it is not built yet when this is
    /// called; [`Crowd::frame_for`] fills it in once it is.
    #[allow(clippy::too_many_arguments)]
    pub fn see(
        &mut self,
        who: Who,
        at: Point,
        body: Graphic,
        facing: Facing,
        hue: Hue,
        war: bool,
        mounted: bool,
    ) -> Mobile {
        self.see_inner(who, at, body, facing, hue, war, mounted, None)
    }

    /// `explicit_from` is supplied only for the locally commanded body.  Its
    /// step is a fact from `PlayerMotion`, unlike a remote mobile's snapshot.
    // One argument per fact the wire carried about the mobile, plus the one
    // `see` does not have. They arrive from different packets, so there is no
    // struct here that is not just this list with a name.
    #[allow(clippy::too_many_arguments)]
    fn see_inner(
        &mut self,
        who: Who,
        at: Point,
        body: Graphic,
        facing: Facing,
        hue: Hue,
        war: bool,
        mounted: bool,
        explicit_from: Option<Point>,
    ) -> Mobile {
        let kind = self.mob_types.kind_of(body);
        let now = self.now;
        let commanded = self.commanded == who;
        let tracked = self.tracked.entry(who).or_insert(Tracked {
            at,
            facing: facing.direction,
            body,
            kind,
            war,
            mounted,
            corpse: false,
            settles_as_corpse: false,
            // A body first heard of is standing: it may well be mid-stride, but
            // the only thing that could say so is a previous packet and there
            // is none. In the stance the packet stated, which for a body that
            // walks into view already fighting is the war stance from its first
            // frame, and a body already in the saddle is the mounted stand from
            // its.
            group: entry_stand(kind, war, mounted),
            step: None,
            stepped_at: None,
            // A body seen for the first time is drawn where it is. There is
            // nothing to ease from: the alternative is easing in from wherever
            // the last body with this serial stood, which is a stranger sliding
            // in from off screen.
            drawn: Gaze::on(at),
            clock: AnimationClock::default(),
            action: None,
            harvest_tool: None,
        });

        // A serial is normally never reused while it remains on screen, but
        // making the ordinary mobile path explicit keeps this history honest if
        // a shard ever does reuse one after removing a corpse.
        tracked.corpse = false;

        // A step is a *position* change. A turn on the spot is not one — the
        // client draws a turning body standing, and a facing change arrives for
        // every step too, so treating it as movement would keep everyone
        // walking forever.
        let moved = match explicit_from {
            Some(_) if tracked.at == at => false,
            Some(from) => {
                // `PlayerMotion` is authoritative for the locally controlled
                // body.  A long blocked frame can finish several queued
                // transitions before the renderer gets one chance to project
                // again; Crowd then legitimately has missed intermediate
                // commands.  Rebase this presentation-only clock at the
                // motion core's named source rather than treating Crowd's old
                // destination as movement truth.
                if tracked.at != from {
                    tracked.at = from;
                    tracked.drawn = Gaze::on(from);
                    tracked.step = None;
                    tracked.stepped_at = None;
                }
                true
            }
            None => tracked.at != at,
        };
        if moved {
            let was = tracked.at;
            let nominal = step_hold(facing.running, mounted);
            // Two ways to know how long a tile takes, and which one is available
            // is exactly what [`Crowd::commanded`] answers.
            //
            // A body we only hear about is glided over the gap *measured* since
            // its last step — that already contains the round trip, the shard's
            // tick and whatever pace the creature actually walks at, none of
            // which this end can look up. See `glide_time`.
            //
            // The one we command has a cadence instead of a measurement: we sent
            // the step and we know when the next is owed, so what is chained is
            // the instant this crossing should *end*. See `crossing`.
            let takes = match commanded {
                true => crossing(tracked.step, commanded, now, nominal),
                false => {
                    let since = tracked.stepped_at.map(|heard| now.saturating_sub(heard));
                    glide_time(nominal, since)
                }
            };
            // Where the body is drawn *now*, which is where the new step has to
            // pick up from. Read before `at` is moved, because it is a question
            // about the step that is ending.
            let from = tracked.gaze_at(now);
            let walked = is_one_step(was, at);
            tracked.at = at;
            tracked.step = Some(Step {
                from: walked.then_some(from),
                was,
                started: now,
                takes,
            });
            // A move of more than one tile is a gate, a recall, a `0x20`. It is
            // the same cut `Crowd::snap` makes and it has to be made here too:
            // there is no step to walk, so the ease would have nothing between
            // it and the destination but a time constant — and a body eased
            // across half a facet is a smear of world nobody is looking at,
            // which is D6's own argument for the distance backstop, arriving
            // here as an event instead.
            if !walked {
                tracked.drawn = Gaze::on(at);
            }
            tracked.stepped_at = Some(now);
            // The stance is folded in before the group is picked, so a step that
            // arrives in the same packet as a stance change walks in the new
            // one — see [`Tracked::moving_group`].
            tracked.war = war;
            tracked.mounted = mounted;
            let moving = tracked.moving_group(facing.running);
            tracked.change_to(moving);
        } else if tracked.facing != facing.direction {
            // A turn is a whole step in UO: it covers no ground, and it costs
            // exactly as long as one that does. So it is not movement — the body
            // above is deliberately not walked — but it *is* a pace sample, and
            // recording it is what keeps the walk after a turn continuous.
            //
            // Found by the walk oracle in `dst.rs`: without this the gap the
            // next step measures spans the turn as well, two holds rather than
            // one, and `glide_time`'s band is just wide enough to believe it.
            // The tile after a turn was then crossed at half speed — half a tile
            // behind where the player had asked for the body to be — and the
            // step after it yanked the body forward to catch up.
            tracked.stepped_at = Some(now);
        }
        tracked.facing = facing.direction;
        tracked.body = body;
        tracked.kind = kind;
        // The stance, for the body that is *not* stepping: drawing a sword is a
        // packet with the same position in it as the one before, so a stance
        // that only reached the group through the walk above would not be seen
        // until the player took a step. `change_to` restarts the clock exactly
        // when the group really changed, so a body already standing where it
        // belongs is not re-started once a packet.
        //
        // Deliberately gated on "not mid-step": a body that changes stance while
        // crossing a tile keeps the walk it is playing and picks the new one up
        // on the next step, rather than snapping to a stand with its feet still
        // moving.
        tracked.war = war;
        tracked.mounted = mounted;
        if tracked.step.is_none() && tracked.action.is_none() {
            let standing = tracked.standing_group();
            tracked.change_to(standing);
        }

        // The same answer [`Crowd::stepping_from`] gives, and it has to be given
        // here too: this mobile is drawn before the next frame re-reads it, and
        // a step's first frame sorted on the wrong tile is the flicker the
        // ordering exists to prevent.
        let stepped_off = tracked.glide(now).and(tracked.step).map(|step| step.was);

        Mobile {
            at,
            body,
            group: tracked.group,
            facing: facing.direction,
            frame: AnimationFrameIndex(0),
            from: stepped_off,
            corpse: false,
            hue,
            drawn: tracked.drawn,
            // `Crowd` ages a position and a clock; equipment has neither —
            // it is the caller's to set, straight from the view.
            equipment: Vec::new().into(),
        }
    }

    /// Put a body somewhere without walking it there.
    ///
    /// What a rollback is: this client steps on its own prediction and the
    /// server may refuse — a `0x21`, or a `0x20` that moves the body somewhere it
    /// did not walk to. The tile it is put back on was never walked across, so
    /// gliding into it would draw the character strolling backwards; and the gap
    /// between the step and the refusal is not a pace, so the measurement
    /// [`glide_time`] makes is dropped along with it.
    ///
    /// The animation is deliberately left alone: a walker whose third step is
    /// refused is still walking, and restarting the group here would drop it to
    /// standing for the one frame between the refusal and the next step.
    #[allow(clippy::too_many_arguments)]
    pub fn snap(
        &mut self,
        who: Who,
        at: Point,
        body: Graphic,
        facing: Facing,
        hue: Hue,
        war: bool,
        mounted: bool,
    ) -> Mobile {
        let kind = self.mob_types.kind_of(body);
        let tracked = self.tracked.entry(who).or_insert(Tracked {
            at,
            facing: facing.direction,
            body,
            kind,
            war,
            mounted,
            corpse: false,
            settles_as_corpse: false,
            group: entry_stand(kind, war, mounted),
            step: None,
            stepped_at: None,
            // A body seen for the first time is drawn where it is. There is
            // nothing to ease from: the alternative is easing in from wherever
            // the last body with this serial stood, which is a stranger sliding
            // in from off screen.
            drawn: Gaze::on(at),
            clock: AnimationClock::default(),
            action: None,
            harvest_tool: None,
        });
        tracked.at = at;
        tracked.facing = facing.direction;
        tracked.body = body;
        tracked.kind = kind;
        // The stance is restated here as it is in `see`, and the animation is
        // still deliberately left alone: whatever group is playing keeps
        // playing, and the walk-to-standing check below picks the new stance up
        // when it expires. A correction is not a moment to change what a body
        // is doing — it is a moment to change where it is.
        tracked.war = war;
        tracked.mounted = mounted;
        // Not cleared to `None`: `Crowd::advance`'s walk-to-standing check is
        // gated on there being a step to time against, so clearing it left a
        // body whose next step never came (a wall it gave up on, a paralyze)
        // stuck playing its walk forever — nothing was left to expire.
        //
        // `started`/`takes` are kept as they were — that is what the
        // `animation_hold` grace this body is still owed is measured
        // against, so a refusal that *is* followed by another step right
        // away does not flicker to standing between them. Only `from` drops,
        // which kills the glide (the cut below) and — since [`Tracked::striding`]
        // now stops the clock the moment `from` is gone — the walk playing
        // itself out on a body that already stopped moving.
        tracked.step = tracked.step.map(|step| Step { from: None, ..step });
        tracked.stepped_at = None;
        // The cut, and here it is an event rather than a threshold: this *is*
        // the rollback, the teleport, the `0x20`. D6 says a cut is an event and
        // the distance is a backstop for the ones nobody raised; at this level
        // nobody has to infer anything. Easing across it would draw the body
        // strolling over ground it never crossed, which is the same picture the
        // glide is skipped for two lines above.
        tracked.corpse = false;
        tracked.drawn = Gaze::on(at);

        Mobile {
            at,
            body,
            group: tracked.group,
            facing: facing.direction,
            frame: AnimationFrameIndex(0),
            // A body put somewhere is standing on the tile it was put on: the
            // step's `from` was just dropped above, so there is no crossing left
            // for the order to be between.
            from: None,
            corpse: false,
            hue,
            drawn: tracked.drawn,
            equipment: Vec::new().into(),
        }
    }

    /// Which frame this body is on, out of what the atlas turned out to pack.
    ///
    /// Asked after the atlas is built, for the reason the frame is not filled in
    /// by [`Crowd::see`]: the count belongs to the atlas and the atlas belongs
    /// to the frame being drawn.
    pub fn frame_for(&self, who: Who, frame_count: AnimationFrameCount) -> u16 {
        self.tracked.get(&who).map_or(0, |tracked| {
            // `frame_count` is the length of the group we are about to draw,
            // not merely advisory metadata.  A movement packet may replace an
            // action group with a walk/run group while the action packet still
            // owns its cadence and declared frame count.  In that overlap an
            // action with seven frames and a run with six used to produce frame
            // six of the run.  There is no such atlas key, so the renderer quite
            // correctly omitted the mobile for that frame.
            //
            // Keep the rendered key total: every non-empty packed group gets
            // an index in its own bounds, whatever packet sequences overlap.
            // An empty group remains frame zero; `place` handles the genuinely
            // absent art without manufacturing another body's sprite.
            let available = frame_count.0;
            if available == 0 {
                return 0;
            }
            if tracked.corpse {
                return available - 1;
            }
            match tracked.action {
                Some(action) => action.frame(available),
                None => tracked.clock.frame(frame_count),
            }
        })
    }

    /// A mobile died, and the shard has named the corpse it leaves (`0xAF`).
    ///
    /// The fall is already playing — combat sends the death action before the
    /// world lays the body down — so what this does is *lift* the falling body
    /// out of the crowd and hold it under the corpse's serial, for
    /// [`Crowd::corpse`] to finish. Holding it rather than leaving it in place is
    /// what makes the hand-off survive the mobile being pruned first: the removal
    /// and the corpse do not have to reach this client in one batch.
    ///
    /// A death with no corpse (`None`) or a body this client never saw fall is
    /// nothing to hold: the corpse, when it comes, is drawn already prone. That
    /// is the same picture a client gets for a corpse that was lying there before
    /// it arrived, and it is the honest one — there is no fall to run.
    ///
    /// `0xAF` is also the fallback that makes the hand-off reliable.  The normal
    /// `0x6E`/`0xE2` action precedes it, but the corpse packet must not turn a
    /// missed or late action packet into an instant death.  This packet still
    /// has the body we last drew, which is enough to select that body's death
    /// group and play the ordinary fall for every animation table.
    pub fn died(&mut self, killed: Serial, corpse: Option<Serial>) {
        let Some(corpse) = corpse else {
            return;
        };
        let Some(tracked) = self.tracked.get_mut(&Some(killed)) else {
            return;
        };
        if tracked.corpse {
            return;
        }
        // Do not make a death depend on the separate action packet having
        // reached this presentation first.  In particular, a low-animation
        // cow needs group 8, rather than the high-animation group's 2; derive
        // the group from the body instead of borrowing whatever it was doing
        // when its hit points reached zero.
        let kind = tracked.kind;
        let death_group = kind.dying();
        // Keep a fall that the ordinary action packet already started at its
        // current frame.  Resetting it here would make two simultaneous deaths
        // visibly rewind when their `0xAF`s arrive at different times.
        if tracked.group != death_group || tracked.action.is_none() {
            tracked.change_to(death_group);
            tracked.action = Some(death_action(kind));
            tracked.harvest_tool = None;
        }
        let body = *tracked;
        self.tracked.remove(&Some(killed));
        self.falls.insert(
            corpse,
            Fall {
                body,
                heard: self.now,
            },
        );
    }

    /// Project an item corpse through the mobile renderer.
    ///
    /// The server sends a corpse as item `0x2006`; its payload is the dead
    /// body's graphic and the direction it fell in. Static art has no picture
    /// for that protocol marker, so the corpse instead holds the last frame of
    /// that body's `Die1` group — and that frame exists per direction, which is
    /// why the facing has to come off the wire rather than be picked here: a
    /// corpse drawn facing anywhere but the way the death animation just played
    /// spins on the ground the moment it settles.
    pub fn corpse(
        &mut self,
        who: Who,
        at: Point,
        body: Graphic,
        facing: Direction,
        hue: Hue,
        equipment: std::rc::Rc<[EquipmentLayer]>,
    ) -> Mobile {
        let facing = Facing::walking(facing);
        let kind = self.mob_types.kind_of(body);
        let group = kind.dying();
        // The fall this corpse was promised, if the shard named one: `0xAF` said
        // which body becomes which corpse while that body was still falling, and
        // [`Crowd::died`] has been holding it since. Taken by serial and not by
        // tile — two of the same creature dying together on one tile is the case
        // a tile hand-off gets wrong, and it is not a rare one where a spawn
        // stands in a group.
        //
        // Nothing is claimed when there is no pairing: a corpse that was already
        // lying there when this client came into range has no fall to finish, and
        // is drawn in its final pose from its first frame.
        let dying = who
            .and_then(|serial| self.falls.remove(&serial))
            .map(|fall| fall.body);
        let tracked = self.tracked.entry(who).or_insert_with(|| {
            dying.map_or(
                Tracked {
                    at,
                    facing: facing.direction,
                    body,
                    kind,
                    war: false,
                    mounted: false,
                    corpse: true,
                    settles_as_corpse: false,
                    group,
                    step: None,
                    stepped_at: None,
                    drawn: Gaze::on(at),
                    clock: AnimationClock::default(),
                    action: None,
                    harvest_tool: None,
                },
                |dying| {
                    Tracked {
                        at,
                        body,
                        kind,
                        war: false,
                        mounted: false,
                        corpse: false,
                        settles_as_corpse: true,
                        group,
                        step: None,
                        stepped_at: None,
                        drawn: Gaze::on(at),
                        // The death action owns the cadence; the ordinary clock is
                        // not consulted until it has become a held corpse.
                        clock: dying.clock,
                        facing: dying.facing,
                        action: dying.action,
                        harvest_tool: dying.harvest_tool,
                    }
                },
            )
        });
        // A corpse stays in this state across ordinary redraw-triggering world
        // updates.  In particular, do not replace an in-flight death with the
        // final pose just because its item was mentioned again.
        if tracked.settles_as_corpse {
            return Mobile {
                at,
                body,
                group: tracked.group,
                facing: tracked.facing,
                frame: AnimationFrameIndex(0),
                from: None,
                corpse: true,
                hue,
                drawn: tracked.drawn,
                equipment: equipment.clone(),
            };
        }
        tracked.at = at;
        tracked.facing = facing.direction;
        tracked.body = body;
        tracked.kind = kind;
        tracked.war = false;
        tracked.corpse = true;
        tracked.settles_as_corpse = false;
        tracked.step = None;
        tracked.stepped_at = None;
        tracked.drawn = Gaze::on(at);
        tracked.action = None;
        tracked.harvest_tool = None;
        tracked.change_to(group);

        Mobile {
            at,
            body,
            group,
            facing: facing.direction,
            frame: AnimationFrameIndex(0),
            from: None,
            corpse: true,
            hue,
            drawn: tracked.drawn,
            equipment,
        }
    }

    /// Play a server-selected classic body action until it finishes.
    pub fn play(&mut self, animation: Animation) {
        let duration = self.swing_timings.remove(&animation.serial);
        let harvest_tool = self.pending_harvest_tools.remove(&animation.serial);
        let Some(tracked) = self.tracked.get_mut(&Some(animation.serial)) else {
            return;
        };
        let (action, frame_count) = action_on_mount(
            tracked.kind,
            tracked.mounted,
            animation.action,
            animation.frame_count,
        );
        let Ok(group) = u8::try_from(action) else {
            return;
        };
        let death = AnimationGroup(group) == tracked.kind.dying();
        // Ordinary actions are latest-wins: a new authoritative swing starts
        // cleanly at frame zero instead of queuing stale motions.  Death is the
        // one terminal action, so it wins over any delayed attack packet.
        if tracked.action.is_some_and(|action| action.death) && !death {
            return;
        }
        if !death {
            if let (Some(duration), Some(action)) = (duration, tracked.action.as_mut()) {
                if action.optimistic {
                    // Network time must never re-time a swing already on screen.
                    // It may add a whole stroke, but the promised 1.6-second tempo
                    // stays immutable.
                    action.frames = frame_count;
                    action.forward = animation.forward;
                    action.repeat = animation.repeat;
                    action.delay = Duration::from_millis(if animation.delay == 0 {
                        DEFAULT_ANIMATION_FRAME_MS
                    } else {
                        u64::from(animation.delay)
                    });
                    action.duration = Some(duration.max(action.elapsed));
                    action.optimistic = false;
                    if harvest_tool.is_some() {
                        tracked.harvest_tool = harvest_tool;
                    }
                    tracked.change_to(AnimationGroup(group));
                    return;
                }
            }
        }
        tracked.action = Some(ActionAnimation {
            death,
            frames: frame_count,
            repeats: animation.repeat_count,
            forward: animation.forward,
            repeat: animation.repeat,
            // The classic packet uses zero for the client's normal mobile
            // animation cadence rather than for a one-millisecond interval.
            delay: Duration::from_millis(if animation.delay == 0 {
                DEFAULT_ANIMATION_FRAME_MS
            } else {
                u64::from(animation.delay)
            }),
            cycle: None,
            duration,
            optimistic: false,
            harvest: false,
            elapsed: Duration::ZERO,
        });
        tracked.harvest_tool = harvest_tool;
        tracked.change_to(AnimationGroup(group));
    }

    /// Play a modern action packet by translating its body-agnostic category
    /// into the group numbering for the body currently on screen.
    pub fn play_new(&mut self, animation: NewAnimation) {
        let Some(tracked) = self.tracked.get(&Some(animation.serial)) else {
            return;
        };
        let Some((action, frames)) = modern_action(tracked.kind, animation.animation_type, animation.action)
        else {
            return;
        };
        self.play(Animation {
            serial: animation.serial,
            action,
            frame_count: AnimationFrameCount(frames),
            repeat_count: 1,
            forward: true,
            repeat: false,
            delay: animation.delay,
        });
    }

    /// Apply the server's duration to this mobile's immediately following action.
    pub fn time_swing(&mut self, timing: SwingTiming) {
        let duration = Duration::from_millis(u64::from(timing.duration.millis()));
        if duration.is_zero() {
            self.swing_timings.remove(&timing.serial);
        } else {
            self.swing_timings.insert(timing.serial, duration);
        }
    }

    /// The shard's word that a body has entered a phase of a combat action.
    ///
    /// Latest-wins, and that is a rule rather than an accident. The packet
    /// arrives more than once for one action by design: an armed action
    /// announces again when it looses, and a `Slow` rule re-announces a
    /// running one with its impact pushed further out
    /// (`docs/combat/evidence/2026-08-27-the-action-phases.md`'s Ф3). Both are the
    /// same action handing this
    /// client a *new* interval to measure, so both restart the picture from now
    /// — a bar still filling towards the old impact would be the desync the
    /// re-announcement exists to prevent.
    ///
    /// This does not touch the animation: what the body is drawn doing is the
    /// ordinary action packet's, and this is the fact behind it.
    pub fn begin_action(&mut self, phase: CombatActionPhase) {
        let started = self.now;
        let record = self.actions.entry(phase.actor).or_insert(ActionRecord {
            running: None,
            ended:   None,
            balked:  None,
        });
        let released_from_held_draw = matches!(phase.phase, ActionPhase::Releasing { .. })
            && record.running.is_some_and(|running| {
                running.kind == phase.kind && matches!(running.phase, ActionPhase::Armed { .. })
            });
        // The previous blow's verdict is deliberately left standing: it is a
        // *different* action's, it fades on its own clock, and the whole reason
        // it is a separate field is that the next commit lands on the same tick
        // as the last impact. See [`ActionRecord`].
        record.running = Some(PendingAction {
            kind: phase.kind,
            phase: phase.phase,
            started,
            // Every action opens in the first stretch and is *told* about each
            // one after it. Assumed here rather than waited for because a commit
            // and the first stage are the same moment on the shard, and sending
            // both would be a packet saying what the other one already implies.
            stage: ActionStage::FIRST,
            released_from_held_draw,
        });
        // A commit is proof the obstacle is gone, whether or not the clearing
        // packet has been applied yet: a fighter cannot both be held up and be
        // swinging. The shard sends the clear too — this is the belt, and it is
        // here so a dropped or reordered pair can never leave a bar filling
        // under the words "out of reach".
        record.balked = None;
    }

    /// The shard's word that a combat action is over, and how it ended.
    ///
    /// Only an end that did *not* land stops the picture. A hit and a miss **are**
    /// the impact: the stroke's last frames are the blow, and the whole point of
    /// stretching it was to make them meet this moment — cutting it here would
    /// truncate the one frame it was aimed at. An interruption is the opposite
    /// and is why the packet exists: a telegraph nobody is making any more used
    /// to run out its promised duration over an empty tile.
    ///
    /// A fall is terminal and outlives a cancelled swing, and a harvest is not
    /// this packet's to cancel — the stroke on screen may be an axe on a tree.
    ///
    /// The *record* ends on every outcome, including the two that leave the
    /// picture running: a blow that landed is a blow nobody is still preparing,
    /// and it is at exactly that moment the outcome becomes the thing worth
    /// saying. That is the split this method is made of — the animation and the
    /// action end at two different moments, and only one of them is here.
    pub fn end_action(&mut self, ended: CombatActionEnded) {
        let at = self.now;
        let record = self.actions.entry(ended.actor).or_insert(ActionRecord {
            running: None,
            ended:   None,
            balked:  None,
        });
        record.running = None;
        record.ended = Some((ended.outcome, at));
        if matches!(
            ended.outcome,
            CombatActionOutcome::Hit | CombatActionOutcome::Miss
        ) {
            return;
        }
        let Some(tracked) = self.tracked.get_mut(&Some(ended.actor)) else {
            return;
        };
        let Some(action) = tracked.action else {
            return;
        };
        if action.death || action.harvest {
            return;
        }
        tracked.action = None;
        tracked.harvest_tool = None;
        let standing = tracked.standing_group();
        tracked.change_to(standing);
    }

    /// The shard's word that a fighter cannot begin an action, or can again.
    ///
    /// Held without a clock, unlike everything else in this module: an outcome
    /// is a message that fades and an action arrives with an interval to age
    /// against, but this is a *condition* — it lasts precisely as long as the
    /// wall, the distance or the empty quiver does, and the shard says when that
    /// is over. Timing it out locally would put the picture back to the silence
    /// this packet exists to end.
    pub fn balk_action(&mut self, balked: CombatActionBalked) {
        let record = self.actions.entry(balked.actor).or_insert(ActionRecord {
            running: None,
            ended:   None,
            balked:  None,
        });
        record.balked = match balked.balk {
            BalkState::Blocked(reason) => Some(reason),
            BalkState::Clear => None,
        };
    }

    /// The shard's word that a running action has moved into a new stretch.
    ///
    /// Only ever applied to an action this client already knows about: a stage
    /// with no phase under it is a stage for an action that began out of sight
    /// or was already ended here, and inventing a record to hold it would draw a
    /// stretch of something nobody is doing.
    pub fn stage_action(&mut self, staged: CombatActionStage) {
        let Some(record) = self.actions.get_mut(&staged.actor) else {
            return;
        };
        let Some(running) = record.running.as_mut() else {
            return;
        };
        running.stage = staged.stage;
    }

    /// Start the local half of a harvest at its targeting click.
    ///
    /// The preview is issued only by the shard that raised this particular
    /// cursor. Its later animation packet confirms the work without changing
    /// the tempo already visible on screen; a refusal finishes the current full
    /// stroke without ever granting a resource client-side.
    pub fn preview_harvest(&mut self, preview: HarvestPreview) {
        let Some(tracked) = self.tracked.get_mut(&Some(preview.serial)) else {
            return;
        };
        let (action, frame_count) =
            action_on_mount(tracked.kind, tracked.mounted, preview.action, preview.frame_count);
        let Ok(group) = u8::try_from(action) else {
            return;
        };
        if tracked.action.is_some_and(|action| action.death) {
            return;
        }
        tracked.action = Some(ActionAnimation {
            death:      false,
            frames:     frame_count,
            repeats:    preview.cycles.max(1),
            forward:    true,
            repeat:     false,
            delay:      Duration::from_millis(DEFAULT_ANIMATION_FRAME_MS),
            cycle:      Some(Duration::from_millis(
                u64::from(preview.duration.millis()) / u64::from(preview.cycles.max(1)),
            )),
            duration:   None,
            optimistic: true,
            harvest:    true,
            elapsed:    Duration::ZERO,
        });
        tracked.harvest_tool = self.pending_harvest_tools.remove(&preview.serial);
        tracked.change_to(AnimationGroup(group));
    }

    /// Let an optimistic harvest settle at the end of its current animation.
    pub fn refuse_harvest(&mut self, refusal: HarvestRefused) {
        let Some(action) = self
            .tracked
            .get_mut(&Some(refusal.serial))
            .and_then(|tracked| tracked.action.as_mut())
            .filter(|action| action.harvest && action.optimistic)
        else {
            return;
        };
        action.duration = Some(action.elapsed);
        action.optimistic = false;
    }

    /// The shard has made its final harvest decision and, if any, queued its
    /// resource update. Keep only the partial axe stroke currently on screen;
    /// an item update alone cannot safely be used as this signal because it may
    /// be a merged stack or the harvest may have yielded nothing.
    pub fn complete_harvest(&mut self, completion: openshard_protocol::feedback::HarvestCompleted) {
        let Some(action) = self
            .tracked
            .get_mut(&Some(completion.serial))
            .and_then(|tracked| tracked.action.as_mut())
            .filter(|action| action.harvest)
        else {
            return;
        };
        action.duration = Some(action.duration.unwrap_or(action.elapsed).min(action.elapsed));
        action.optimistic = false;
    }

    /// Forget the visual hint that belonged to a target cursor the player
    /// cancelled before clicking. It must not leak into a later unrelated swing.
    pub fn cancel_harvest_preview(&mut self, serial: Serial) {
        self.pending_harvest_tools.remove(&serial);
    }

    /// Hold a backpack axe's visual layer until its immediately following
    /// animation completes.  It never enters the authoritative equipment list.
    pub fn harvest_tool(&mut self, visual: HarvestToolVisual, tiledata: &TileData) {
        self.pending_harvest_tools.insert(
            visual.serial,
            EquipmentLayer {
                graphic: tiledata.static_tile(visual.graphic.0).anim_id,
                hue:     visual.hue,
                layer:   visual.layer,
            },
        );
    }

    /// The supplied equipment plus an in-flight harvest tool, if there is one.
    /// The borrowed tool replaces the matching hand layer only in this rendered
    /// frame; the next projection restores the real weapon automatically.
    pub fn worn(
        &self,
        who: Who,
        equipment: &[Equipment],
        tiledata: &TileData,
    ) -> std::rc::Rc<[EquipmentLayer]> {
        let mut layers = worn(equipment, tiledata);
        if let Some(tool) = self.tracked.get(&who).and_then(|tracked| tracked.harvest_tool) {
            match layers.iter_mut().find(|item| item.layer == tool.layer) {
                Some(layer) => *layer = tool,
                None => layers.push(tool),
            }
        }
        layers.into()
    }

    /// Where this body is drawn now, in the sub-pixel form the sprite and the
    /// camera both read.
    ///
    /// Asked every frame and not only when a packet arrives, which is the whole
    /// point: a position read once and stored would freeze at whatever it was
    /// when the `0x77` landed, and the body would jump a tile on the next one —
    /// the teleport this exists to remove, arriving 400ms late.
    ///
    /// `None` for a body this crowd is not tracking, which is absence and not a
    /// default: the caller has a mobile the crowd has never been told about, and
    /// the tile it would otherwise be given is a guess.
    pub fn drawn_for(&self, who: Who) -> Option<Gaze> {
        Some(self.tracked.get(&who)?.drawn)
    }

    /// The tile this body is stepping *off*, while it is between two — `None`
    /// once it has arrived, and for a body this crowd is not tracking.
    ///
    /// Asked every frame, like [`Crowd::drawn_for`] and for the same reason: a
    /// step ends on a clock and nothing arrives to say so. What reads it is the
    /// renderer's depth order — a sprite mid-step covers both tiles and has to
    /// sort at the nearer of them (`depth::mobile_tile`).
    ///
    /// Tied to the *glide* and not to [`Tracked::step`], which outlives it: the
    /// animation is deliberately held half a step past the crossing (see
    /// `animation_hold`), and a body that has landed sorts on the tile it landed
    /// on. And absent along with the glide for a move that was never a step —
    /// a gate or a rollback covers no ground between two tiles.
    pub fn stepping_from(&self, who: Who) -> Option<Point> {
        let tracked = self.tracked.get(&who)?;
        // The glide is what says a body is between two tiles at all: it is
        // absent once the crossing is over, and absent from the start for a move
        // that was never a step.
        tracked.glide(self.now)?;
        Some(tracked.step?.was)
    }

    /// Which animation group this body is playing now, in the sub-pixel form
    /// the sprite and the camera both read.
    ///
    /// Asked every frame for the same reason [`Crowd::drawn_for`] is: a group
    /// read once, at the last `see`/`snap`, and cached from then on goes stale
    /// the moment [`Crowd::advance`] drops a body from walking to standing on
    /// its own — nothing calls `see` again just because a body stopped, so a
    /// caller that only ever re-read the group off the packet that started
    /// the walk would keep asking the atlas for the walking group's frames
    /// forever, timed by a clock that had already moved on to the standing
    /// group's. That mismatch is what "the character walks in place" turned
    /// out to be: the position stopped, the clock kept advancing, and the
    /// sprite was the walk's because nothing ever asked the crowd again.
    ///
    /// `None` for a body this crowd is not tracking, same as [`Crowd::drawn_for`].
    pub fn group_for(&self, who: Who) -> Option<AnimationGroup> {
        Some(self.tracked.get(&who)?.group)
    }

    /// Whether anybody is part way through a step.
    ///
    /// What the window asks to decide how often to redraw: a crowd standing
    /// still changes a pixel once every `FRAME_DELAY` and one mid-step changes
    /// several every frame. A glide drawn on the animation clock arrives in
    /// five visible jumps, which is the teleport it exists to remove.
    pub fn anyone_gliding(&self) -> bool {
        self.tracked
            .values()
            .any(|tracked| tracked.glide(self.now).is_some())
    }

    /// Forget everyone not in this set.
    ///
    /// Called with the serials the view still holds: a mobile that walked out of
    /// range is gone, and a `HashMap` that kept it would grow for as long as the
    /// client is connected. Forgetting is also the right *behaviour* — one that
    /// comes back is a body seen for the first time again, and pretending to
    /// remember what it was doing while off screen would be inventing it.
    pub fn retain(&mut self, present: impl Fn(Who) -> bool) {
        self.tracked.retain(|who, _| present(*who));
        self.speech.retain(|who, _| present(*who));
        self.actions.retain(|serial, _| present(Some(*serial)));
    }

    /// Record that `who` said `text`, above whatever they were already saying.
    ///
    /// Not folded into [`Crowd::see`]: a `0x1C` and a `0x77` are different
    /// packets that arrive on their own schedules, and a speaker does not have
    /// to have moved for a line to be worth showing.
    ///
    /// Stacked rather than replaced — a single click alone sends two lines for
    /// one mobile — and trimmed to [`SPEECH_STACK`] from the front, so the
    /// newest is what survives a talkative shard.
    pub fn hear(&mut self, who: Who, text: String, font: Font, hue: Hue) {
        let started = self.now;
        let lines = self.speech.entry(who).or_default();
        lines.push(Speech {
            text,
            font,
            hue,
            started,
        });
        if lines.len() > SPEECH_STACK {
            lines.drain(..lines.len() - SPEECH_STACK);
        }
    }

    /// What `who` is still saying, oldest first, and empty once every line has
    /// passed [`SPEECH_HOLD`].
    ///
    /// Checked against the clock here rather than expired in [`Crowd::advance`]:
    /// nothing downstream needs to know the *moment* a line goes stale, only
    /// whether it still is one, and a lazy check is one fewer place that has to
    /// agree with [`SPEECH_HOLD`].
    pub fn speaking(&self, who: Who) -> impl Iterator<Item = (&str, Font, Hue)> {
        self.speech
            .get(&who)
            .into_iter()
            .flatten()
            .filter(|line| self.now.saturating_sub(line.started) < SPEECH_HOLD)
            .map(|line| (line.text.as_str(), line.font, line.hue))
    }

    /// What `who` is doing about fighting, or `None` for a body that is not.
    ///
    /// Checked against the clock here rather than expired in [`Crowd::advance`],
    /// for [`Crowd::speaking`]'s reason: nothing downstream needs to know the
    /// *moment* a bar goes stale, only whether it still is one, and a lazy check
    /// is one fewer place that has to agree with the holds.
    ///
    /// Two things time out here, and neither is how an action is supposed to
    /// end. An outcome fades at [`OUTCOME_HOLD`] because it is a message and not
    /// a state. A *running* action that outlives its own interval **by
    /// [`RUNNING_GRACE`]** is the bound on a leak: every action ends and every
    /// end crosses the wire (`docs/combat/design_actions.md`'s D2), so in the ordinary
    /// run [`Crowd::end_action`] has already replaced it — what this stops is a
    /// body that walked out of range mid-swing holding a full bar for as long as
    /// the client is connected.
    ///
    /// The grace is the difference between a bound and a schedule, and it is the
    /// whole reason the tail of an action is no longer blank: see
    /// [`RUNNING_GRACE`].
    pub fn preparing(&self, who: Who) -> Option<ActionProgress> {
        let record = self.actions.get(&who?)?;
        let ended = record
            .ended
            .filter(|(_, at)| self.now.saturating_sub(*at) < OUTCOME_HOLD)
            .map(|(outcome, _)| outcome);
        let running = record.running.and_then(|action| {
            let elapsed = self.now.saturating_sub(action.started);
            let span = Duration::from_millis(u64::from(action.phase.duration().millis()));
            if elapsed > span + RUNNING_GRACE {
                return None;
            }
            let fill = match action.phase {
                ActionPhase::Arming { .. } => {
                    ActionFill::Arming {
                        filled: match span.is_zero() {
                            true => 1.0,
                            false => (elapsed.as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0),
                        },
                    }
                }
                ActionPhase::Armed { .. } => ActionFill::Armed,
                // A zero interval is a lie the wire refuses to tell (see
                // `CombatActionPhase`), so this is a division guard and not a
                // case: a released action that takes no time has already
                // arrived.
                ActionPhase::Releasing { .. } => {
                    ActionFill::Releasing {
                        filled: match span.is_zero() {
                            true => 1.0,
                            false => (elapsed.as_secs_f32() / span.as_secs_f32()).clamp(0.0, 1.0),
                        },
                    }
                }
            };
            Some(RunningAction {
                kind: action.kind,
                fill,
                stage: action.stage,
                released_from_held_draw: action.released_from_held_draw,
            })
        });
        // Deliberately not timed out with the other two: see [`balk_action`].
        let balked = record.balked;
        match (running, ended, balked) {
            (None, None, None) => None,
            (running, ended, balked) => {
                Some(ActionProgress {
                    running,
                    ended,
                    balked,
                })
            }
        }
    }
}

/// The ordinary one-shot fall supplied by `0xAF` when its matching action did
/// not make it through the presentation boundary first.
fn death_action(kind: BodyKind) -> ActionAnimation {
    ActionAnimation {
        death:      true,
        frames:     AnimationFrameCount(if matches!(kind, BodyKind::Human) { 6 } else { 4 }),
        repeats:    1,
        forward:    true,
        repeat:     false,
        delay:      Duration::from_millis(DEFAULT_ANIMATION_FRAME_MS),
        cycle:      None,
        duration:   None,
        optimistic: false,
        harvest:    false,
        elapsed:    Duration::ZERO,
    }
}

/// The group a body stands still in: the mounted stand where it is riding and
/// its kind has one, the war stand where it is fighting and its kind has one,
/// and the ordinary stand otherwise.
///
/// A free function rather than a method so the two `Tracked` initializers
/// below can reach it before there is a `self` to call it on — a mobile heard
/// of for the first time is drawn already sitting the stance its first packet
/// named, not standing plain for one frame and correcting itself. One
/// implementation for those two places and [`Tracked::standing_group`], for
/// [`Tracked::standing_group`]'s own reason: they must not answer differently.
fn entry_stand(kind: BodyKind, war: bool, mounted: bool) -> AnimationGroup {
    if mounted {
        if let Some(group) = kind.standing_mounted() {
            return group;
        }
    }
    match war {
        true => kind.standing_at_war().unwrap_or(kind.standing()),
        false => kind.standing(),
    }
}

/// The group a body moves in, at the pace the step said — [`entry_stand`]'s
/// counterpart for a body that is walking or running rather than standing.
///
/// Mounted wins over war the same way it wins over the ordinary walk: a
/// galloping war stance does not exist in the classic numbering any more than
/// a war run does, so a horse ridden into a fight plays the ordinary ride.
/// War otherwise changes a *walk* and never a run: the reference falls
/// straight through to the ordinary run whenever the step is a run, because a
/// body sprinting is a body not fighting. And a running monster walks — the
/// high numbering has no run at all, which is [`BodyKind::running`]'s `None`.
fn entry_move(kind: BodyKind, running: bool, war: bool, mounted: bool) -> AnimationGroup {
    if mounted {
        let mounted_group = match running {
            true => kind.running_mounted(),
            false => kind.walking_mounted(),
        };
        if let Some(group) = mounted_group {
            return group;
        }
    }
    match (running, kind.running()) {
        (true, Some(running)) => running,
        (true, None) => kind.walking(),
        (false, _) => {
            match war {
                true => kind.walking_at_war().unwrap_or(kind.walking()),
                false => kind.walking(),
            }
        }
    }
}

impl Tracked {
    /// Start playing a group, restarting the clock if it is a different one.
    fn change_to(&mut self, group: AnimationGroup) {
        if self.group != group {
            self.group = group;
            self.clock = AnimationClock::default();
        }
    }

    /// The group this body stands still in. See [`entry_stand`].
    fn standing_group(&self) -> AnimationGroup {
        entry_stand(self.kind, self.war, self.mounted)
    }

    /// The group this body moves in, at the pace the step said. See
    /// [`entry_move`].
    fn moving_group(&self, running: bool) -> AnimationGroup {
        entry_move(self.kind, running, self.war, self.mounted)
    }

    /// How much of a frame's span this body spent actually crossing its tile.
    ///
    /// The whole span for a body that is walking or standing, and only the part
    /// before the crossing ended for one that arrived part way through it. `was`
    /// is where the crowd's clock stood before the span.
    ///
    /// # Why a body that has arrived stops playing
    ///
    /// The window between the crossing ending and [`animation_hold`] expiring is
    /// a foot in the air over ground already covered. The hold exists so that a
    /// walker whose next step is a round trip away does not drop to standing for
    /// a frame and restart the walk at frame zero — and *holding a group* is all
    /// it was ever meant to do. Advancing the clock through it plays the rest of
    /// the stride on the spot: 200ms at a walk, two and a half frames of a body
    /// striding while its feet cover no ground, at the end of every walk. Which
    /// is the complaint it came from — a step that plays itself out after the
    /// walking has stopped.
    ///
    /// The reference draws exactly this line. `Mobile.NoIterateAnimIndex` is
    /// true while `LastStepTime > Time.Ticks - Constants.WALKING_DELAY &&
    /// Steps.Count == 0` — still counted as walking, no step left to walk, so
    /// the frame index does not advance. The group is held and the picture is
    /// frozen, and the next step carries the stride on from where it stopped.
    fn striding(&self, was: Duration, dt: Duration) -> Duration {
        let Some(step) = self.step else {
            // Standing, and a standing animation plays: this is the walk's
            // freeze, not a general one.
            return dt;
        };
        // No ground being covered: a jump of more than one tile (`from` is
        // never set for those — see [`Step::from`]), or a step a refusal
        // snapped back to nothing but its timer — see [`Crowd::snap`]. Either
        // way `started`/`takes` describe a crossing that was never actually
        // walked, and letting the clock run through it plays the stride out
        // on a body sitting still on its tile, which reads as walking in
        // place. Traced with `trace_walking_in_place_after_an_unfollowed_refusal`:
        // a body snapped back stayed put while its legs kept moving for most
        // of a step. The group is still held, same as the tail of a genuine
        // glide — only the clock stops.
        if step.from.is_none() {
            return Duration::ZERO;
        }
        let ends = step.started + step.takes;
        match ends.checked_sub(was) {
            Some(left) => dt.min(left),
            // The crossing was already over when the span began.
            None => Duration::ZERO,
        }
    }

    /// Where this body is drawn at a moment on the crowd's clock, sub-tile.
    ///
    /// The whole of what makes a step continuous: a new step starts here rather
    /// than on the tile boundary, so an arrival that is early or late changes
    /// the *speed* of the crossing that follows it and never the position. The
    /// picture cannot jump, by construction, whatever the wire does.
    ///
    /// `when` is normally the instant a step is beginning, which for a chained
    /// one is in the past — see [`began`]. A body with nothing in flight is on
    /// its tile.
    fn gaze_at(&self, when: Duration) -> Gaze {
        let at = Gaze::on(self.at);
        let Some(glide) = self.glide(when) else {
            return at;
        };
        // How much of the step has not been walked yet. Clamped rather than
        // trusted: a progress past one means a clock ran past the step, and the
        // honest picture of that is a body standing on its destination.
        //
        // Backwards from the destination rather than forwards from the origin,
        // which is the form that lands *exactly* on the tile when the step is
        // over — see [`Gaze::back_towards`]. A body that never quite arrives
        // shimmers.
        let left = f64::from(1.0 - glide.progress.clamp(0.0, 1.0));
        at.back_towards(glide.from, left)
    }

    /// Where between two tiles this body is, at a moment on the crowd's clock.
    fn glide(&self, now: Duration) -> Option<Glide> {
        let step = self.step?;
        let from = step.from?;
        // Saturating rather than asserted: `now` is the crowd's own clock and
        // only ever moves forward, but a step heard *this* instant divides
        // zero by the step's length, which is the honest zero.
        let elapsed = now.saturating_sub(step.started);
        // Arrived. Not a glide of progress 1.0, which is the same picture and a
        // different answer to "is anybody mid-step": the window redraws at 60Hz
        // for as long as one is, and the body would keep it there for the half
        // step its animation goes on playing.
        if elapsed >= step.takes {
            return None;
        }
        Some(Glide {
            from,
            progress: elapsed.as_secs_f32() / step.takes.as_secs_f32(),
        })
    }
}

/// How long this crossing has left to run, for a body whose cadence is known.
///
/// # The arrival is noise and the cadence is not
///
/// A step is handed to this layer when the event loop wakes with it — after the
/// wire, after an mpsc, after however late the operating system got round to the
/// thread. The *cadence* underneath that has none of the noise in it: `steer.rs`
/// arms each step from the previous one's deadline rather than from the wake it
/// happened to be taken at, precisely so that lateness does not accumulate.
///
/// Crossing every tile in the nominal time *from the arrival* therefore
/// re-randomises the phase of every tile: each crossing is the right length and
/// starts a few milliseconds late, by a different few each time, so the body's
/// position steps by the difference of two of them at every tile boundary. Eight
/// milliseconds of wake jitter is a pixel a tile, once every 400ms — and the
/// camera is locked to the body, so the whole world takes it. That is the jerk
/// the walk dump in `dst.rs` was written to measure.
///
/// So what is chained is the *end* of the crossing and not its start. The step
/// begins now, from where the body is now — which is what keeps the picture
/// continuous, see [`Step::from`] — and it is given however long is left until
/// the instant the cadence says it should arrive. The few milliseconds of
/// lateness come out of the crossing's *speed*, where at eight milliseconds in
/// four hundred they are two per cent and nobody can see them, instead of out of
/// its position, where they are a pixel and everybody can.
///
/// Chaining the start instead would put it in the past, which is arithmetically
/// the same schedule and a jump on screen: the body was drawn parked on its tile
/// for the frames between the boundary and the arrival, and a retroactive start
/// makes the next frame catch up all at once.
///
/// The band it is believed in is [`crossing_left`](openshard_movement::crossing_left),
/// which is where the rule itself lives now: the app's own movement core draws
/// the body this client commands and needs the identical schedule, so the band
/// is stated once in `common/movement` beside the four rates and read from both
/// ends. Everyone else gets the nominal length, because nothing on the wire says
/// when an NPC set off and a schedule invented for a guessed pace would move it
/// somewhere it never was.
fn crossing(previous: Option<Step>, commanded: bool, now: Duration, nominal: Duration) -> Duration {
    if !commanded {
        return nominal;
    }
    let Some(previous) = previous else { return nominal };
    // Where the cadence says this crossing ends: the previous one's scheduled
    // end, which is when this step set off, plus a step.
    let ends = previous.started + previous.takes + nominal;
    let Some(left) = ends.checked_sub(now) else {
        return nominal;
    };
    openshard_movement::crossing_left(left, nominal)
}

/// Whether a move of one tile is what happened, in the distance UO measures.
///
/// Chebyshev, because the grid's eight directions are all one step: a diagonal
/// moves both axes and is no further than a straight one. Anything longer is a
/// teleport wearing a step's clothes.
fn is_one_step(from: Point, to: Point) -> bool {
    let dx = i32::from(to.x) - i32::from(from.x);
    let dy = i32::from(to.y) - i32::from(from.y);
    dx.abs().max(dy.abs()) <= 1
}

#[cfg(test)]
mod tests {
    use openshard_movement::{
        RUN_HOLD,
        WALK_HOLD,
    };
    use openshard_protocol::direction::Direction;
    use openshard_protocol::feedback::InterruptReason;

    use super::*;

    const PLAYER: u16 = 400;
    const HORSE: u16 = 204;
    const DRAGON: u16 = 12;

    /// A serial, as the crowd keys them.
    fn serial(raw: u32) -> Who {
        Some(Serial::new(raw).expect("a nonzero serial"))
    }

    /// One worn item as the wire states it.
    fn equipped(graphic: u16, layer: Layer) -> Equipment {
        Equipment {
            serial: Serial::new(0x4000_0001).expect("a nonzero serial"),
            graphic: Graphic(graphic),
            layer,
            hue: Hue::NONE,
        }
    }

    #[test]
    fn a_saddle_is_carried_as_the_horse_under_it() {
        // The mount layer is the one layer whose picture does not come out of
        // `tiledata.mul` — see `mount_picture`. An empty tiledata is exactly the
        // stock install's answer for `0x3E9F` as far as this end is concerned
        // (an `AnimID` naming an animation that does not exist), and the horse
        // has to come out of it anyway.
        let worn = worn(
            &[equipped(0x3E9F, Layer::MOUNT), equipped(0x1517, Layer::SHIRT)],
            &TileData::empty(),
        );
        assert_eq!(worn[0].graphic, AnimId(0x00C8), "the ordinary horse's body");
        assert_eq!(
            worn[1].graphic,
            AnimId(0),
            "an ordinary layer still reads the file, which here says nothing"
        );
    }

    #[test]
    fn a_mount_item_nobody_has_a_body_for_falls_back_to_the_file() {
        // `0x3E96` is a mount-layer item the table does not list — the reference
        // client's "no mount at all" sentinel. Nothing is invented for it: the
        // ordinary tiledata path answers, and answering `AnimID` 0 is how
        // `mobiles::mount_of` is told to draw nothing.
        let worn = worn(&[equipped(0x3E96, Layer::MOUNT)], &TileData::empty());
        assert_eq!(worn[0].graphic, AnimId(0));
    }

    fn chopping_crowd() -> (Crowd, Who, Serial) {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.preview_harvest(HarvestPreview {
            cursor_id:   openshard_protocol::wire::CursorId(mobile.raw()),
            serial:      mobile,
            action:      13,
            frame_count: AnimationFrameCount(6),
            duration:    openshard_protocol::feedback::SwingDuration(4_800),
            cycles:      3,
        });
        (crowd, who, mobile)
    }

    fn confirm_chop(crowd: &mut Crowd, mobile: Serial) {
        crowd.time_swing(SwingTiming {
            serial:   mobile,
            duration: openshard_protocol::feedback::SwingDuration(4_800),
        });
        crowd.play(Animation {
            serial:       mobile,
            action:       13,
            frame_count:  AnimationFrameCount(6),
            repeat_count: 1,
            forward:      true,
            repeat:       false,
            delay:        0,
        });
    }

    /// A body nobody has seen before is standing, in its own kind's numbering.
    #[test]
    fn a_body_first_heard_of_is_standing() {
        let mut crowd = Crowd::default();
        let human = crowd.see(
            serial(1),
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(human.group, 4, "PeopleAnimationGroup.Stand");
        let horse = crowd.see(
            serial(2),
            Point::new(10, 10, 0),
            Graphic(HORSE),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(horse.group, 2, "LowAnimationGroup.Stand");
        let dragon = crowd.see(
            serial(3),
            Point::new(10, 10, 0),
            Graphic(DRAGON),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(dragon.group, 1, "HighAnimationGroup.Stand");
    }

    #[test]
    fn a_modern_attack_uses_each_bodys_attack_group() {
        let mut crowd = Crowd::default();
        for (who, body, expected_group, expected_frames) in [
            (serial(1), Graphic(DRAGON), 4, 4),
            (serial(2), Graphic(HORSE), 5, 4),
            (serial(3), Graphic(PLAYER), 31, 7),
        ] {
            crowd.see(
                who,
                Point::new(10, 10, 0),
                body,
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                false,
            );
            crowd.play_new(NewAnimation {
                serial:         who.expect("a serial"),
                animation_type: 0,
                action:         0,
                delay:          0,
            });
            assert_eq!(crowd.group_for(who), Some(AnimationGroup(expected_group)));
            crowd.advance(Duration::from_millis(80));
            assert_eq!(
                crowd.frame_for(who, AnimationFrameCount(expected_frames)),
                1,
                "body {} advances its attack",
                body.0
            );
        }
    }

    #[test]
    fn a_modern_human_attack_uses_the_weapon_sub_action() {
        for (sub_action, expected) in [
            (0, (31, 7)), // fists
            (3, (11, 5)), // one-handed bash
            (4, (9, 7)),  // one-handed slash
            (5, (10, 7)), // one-handed pierce
            (6, (12, 5)), // two-handed bash
            (7, (13, 6)), // axe / two-handed slash
            (8, (14, 7)), // two-handed pierce
            (1, (18, 7)), // bow
            (2, (19, 7)), // crossbow
        ] {
            assert_eq!(modern_action(BodyKind::Human, 0, sub_action), Some(expected));
        }
    }

    /// Mounted attacks occupy their own block in the human animation table.
    /// The modern packet still carries the same weapon sub-action as it does on
    /// foot, so the saddle state has to replace both the group and its length.
    #[test]
    fn every_modern_human_attack_uses_its_mounted_group() {
        for (sub_action, expected_group, expected_frames) in [
            (0, 29, 5), // unarmed / slap horse
            (1, 27, 5), // bow
            (2, 28, 7), // crossbow
            (3, 26, 5), // one-handed bash
            (4, 26, 5), // one-handed slash
            (5, 26, 5), // one-handed pierce
            (6, 26, 5), // two-handed bash
            (7, 26, 5), // two-handed slash
            (8, 26, 5), // two-handed pierce
        ] {
            let who = serial(1);
            let mobile = who.expect("a serial");
            let mut crowd = Crowd::default();
            crowd.see(
                who,
                Point::new(10, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                true,
            );
            crowd.play_new(NewAnimation {
                serial:         mobile,
                animation_type: 0,
                action:         sub_action,
                delay:          0,
            });

            assert_eq!(
                crowd.group_for(who),
                Some(AnimationGroup(expected_group)),
                "mounted sub-action {sub_action} selected the on-foot group"
            );
            crowd.advance(Duration::from_millis(80));
            assert_eq!(
                crowd.frame_for(who, AnimationFrameCount(expected_frames)),
                1,
                "mounted group {expected_group} did not advance"
            );
            crowd.advance(Duration::from_millis(80 * u64::from(expected_frames - 1)));
            assert_eq!(
                crowd.group_for(who),
                Some(AnimationGroup(25)),
                "mounted group {expected_group} did not settle back into the saddle"
            );
        }
    }

    /// A classic `0x6E` names the on-foot group instead of the weapon
    /// sub-action. It must reach the same mounted art as `0xE2`, including the
    /// mounted group's own frame count.
    #[test]
    fn classic_human_attacks_are_mounted_too() {
        for (on_foot, on_foot_frames, expected_group, expected_frames) in [
            (18_u16, 7_u16, 27_u8, 5_u16), // bow
            (19, 7, 28, 7),                // crossbow
            (9, 7, 26, 5),                 // melee
            (31, 7, 29, 5),                // unarmed
        ] {
            let who = serial(1);
            let mobile = who.expect("a serial");
            let mut crowd = Crowd::default();
            crowd.see(
                who,
                Point::new(10, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                true,
            );
            crowd.play(Animation {
                serial:       mobile,
                action:       on_foot,
                frame_count:  AnimationFrameCount(on_foot_frames),
                repeat_count: 1,
                forward:      true,
                repeat:       false,
                delay:        0,
            });

            assert_eq!(crowd.group_for(who), Some(AnimationGroup(expected_group)));
            crowd.advance(Duration::from_millis(80 * u64::from(expected_frames)));
            assert_eq!(
                crowd.group_for(who),
                Some(AnimationGroup(25)),
                "the mounted frame count must own the action's lifetime"
            );
        }
    }

    #[test]
    fn a_timed_mounted_bow_draw_uses_all_of_the_mounted_art() {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            true,
        );
        crowd.time_swing(SwingTiming {
            serial:   mobile,
            duration: openshard_protocol::feedback::SwingDuration(1_600),
        });
        crowd.play_new(NewAnimation {
            serial:         mobile,
            animation_type: 0,
            action:         1,
            delay:          0,
        });

        assert_eq!(crowd.group_for(who), Some(AnimationGroup(27)));
        for frame in 0..5_u16 {
            assert_eq!(
                crowd.frame_for(who, AnimationFrameCount(5)),
                frame,
                "mounted bow frame {frame} starts at {}ms",
                u64::from(frame) * 320
            );
            crowd.advance(Duration::from_millis(320));
        }
        assert_eq!(
            crowd.group_for(who),
            Some(AnimationGroup(25)),
            "the completed bow draw returns to the mounted stand"
        );
    }

    #[test]
    fn a_mounted_harvest_preview_keeps_the_worker_in_the_saddle() {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            true,
        );
        crowd.preview_harvest(HarvestPreview {
            cursor_id:   openshard_protocol::wire::CursorId(mobile.raw()),
            serial:      mobile,
            action:      13,
            frame_count: AnimationFrameCount(6),
            duration:    openshard_protocol::feedback::SwingDuration(1_600),
            cycles:      1,
        });

        assert_eq!(
            crowd.group_for(who),
            Some(AnimationGroup(26)),
            "a mounted tool swing uses the generic mounted attack"
        );
        crowd.advance(Duration::from_millis(320));
        assert_eq!(
            crowd.frame_for(who, AnimationFrameCount(5)),
            1,
            "the five mounted frames are spread across the preview interval"
        );
    }

    /// A cancelled telegraph used to run out its promised duration over an empty
    /// tile, because the wire never said it had stopped. A landed one is the
    /// opposite case and must not be cut: its last frames *are* the impact, and
    /// they are what the stretched timing was aimed at.
    #[test]
    fn an_interrupted_swing_stops_being_drawn_and_a_landed_one_finishes() {
        for (outcome, still_swinging) in [
            (
                CombatActionOutcome::Interrupted(InterruptReason::TargetGone),
                false,
            ),
            (CombatActionOutcome::Expired, false),
            (CombatActionOutcome::Hit, true),
            (CombatActionOutcome::Miss, true),
        ] {
            let who = serial(1);
            let mobile = who.expect("a serial");
            let mut crowd = Crowd::default();
            crowd.see(
                who,
                Point::new(10, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                false,
            );
            crowd.time_swing(SwingTiming {
                serial:   mobile,
                duration: openshard_protocol::feedback::SwingDuration(1_600),
            });
            crowd.play_new(NewAnimation {
                serial:         mobile,
                animation_type: 0,
                action:         0,
                delay:          0,
            });
            let swinging = crowd.group_for(who);
            assert_ne!(swinging, Some(BodyKind::Human.standing()));

            crowd.advance(Duration::from_millis(400));
            crowd.end_action(CombatActionEnded {
                actor: mobile,
                outcome,
            });

            let expected = if still_swinging {
                swinging
            } else {
                Some(BodyKind::Human.standing())
            };
            assert_eq!(
                crowd.group_for(who),
                expected,
                "a {outcome:?} left the wrong body on screen"
            );
        }
    }

    /// A body the crowd has seen, doing nothing in particular.
    fn standing_crowd() -> (Crowd, Who, Serial) {
        let who = serial(1);
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        (crowd, who, who.expect("a serial"))
    }

    /// One phase packet, as the shard sends it.
    fn phase(actor: Serial, kind: CombatActionKind, phase: ActionPhase) -> CombatActionPhase {
        CombatActionPhase {
            actor,
            target: Serial::new(0x99).expect("a nonzero serial"),
            kind,
            phase,
        }
    }

    /// A body preparing something, with nothing behind it yet. An action opens
    /// in the first stretch and is told about every one after it, so this is the
    /// shape of everything the shard has not yet said anything more about.
    fn running(kind: CombatActionKind, fill: ActionFill) -> Option<ActionProgress> {
        staged(kind, fill, ActionStage::FIRST)
    }

    /// The same, part way through — for the tests that are about the stretches.
    fn staged(kind: CombatActionKind, fill: ActionFill, stage: ActionStage) -> Option<ActionProgress> {
        Some(ActionProgress {
            running: Some(RunningAction {
                kind,
                fill,
                stage,
                released_from_held_draw: false,
            }),
            ended:   None,
            balked:  None,
        })
    }

    /// A released action is the bar filling: it began at the commit and the
    /// impact is the far end. The whole point of the packet is that this
    /// fraction is the shard's interval and not a guess off an animation.
    #[test]
    fn a_released_action_fills_across_its_own_interval() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Swing,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(1_600),
            },
        ));
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Swing, ActionFill::Releasing { filled: 0.0 }),
        );

        crowd.advance(Duration::from_millis(400));
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Swing, ActionFill::Releasing { filled: 0.25 }),
        );

        crowd.advance(Duration::from_millis(1_200));
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Swing, ActionFill::Releasing { filled: 1.0 }),
            "the bar is full at the impact, not past it",
        );
    }

    /// An armed action is *waiting*, and a bar creeping along its endurance
    /// would read as an impact approaching. It is held instead.
    #[test]
    fn an_armed_action_is_held_rather_than_filling() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Shot,
            ActionPhase::Armed {
                endurance: openshard_protocol::feedback::SwingDuration(8_000),
            },
        ));
        crowd.advance(Duration::from_millis(4_000));
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Shot, ActionFill::Armed),
            "half the endurance gone is still a held bow",
        );
    }

    #[test]
    fn an_arming_action_fills_until_the_bow_is_ready() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Shot,
            ActionPhase::Arming {
                ready_in: openshard_protocol::feedback::SwingDuration(1_600),
            },
        ));
        crowd.advance(Duration::from_millis(800));
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Shot, ActionFill::Arming { filled: 0.5 }),
            "the draw is visible as preparation, not a full held bar"
        );
    }

    /// A `Slow` rule pushes the impact and re-announces it, and a bar still
    /// running towards the old one is exactly the desync the re-announcement
    /// exists to stop.
    #[test]
    fn a_re_announced_impact_restarts_the_bar() {
        let (mut crowd, who, mobile) = standing_crowd();
        let announce = |crowd: &mut Crowd, millis: u32| {
            crowd.begin_action(phase(
                mobile,
                CombatActionKind::Swing,
                ActionPhase::Releasing {
                    impact_in: openshard_protocol::feedback::SwingDuration(millis),
                },
            ));
        };
        announce(&mut crowd, 1_600);
        crowd.advance(Duration::from_millis(800));
        announce(&mut crowd, 1_600);
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Swing, ActionFill::Releasing { filled: 0.0 }),
        );
    }

    /// The half a picture could never state: *why* it stopped. Every outcome
    /// ends the preparation — a landed blow is not something anyone is still
    /// preparing — and the reason outlives it by [`OUTCOME_HOLD`].
    #[test]
    fn every_ending_ends_the_bar_and_says_which_it_was() {
        for outcome in [
            CombatActionOutcome::Hit,
            CombatActionOutcome::Miss,
            CombatActionOutcome::Expired,
            CombatActionOutcome::Interrupted(InterruptReason::NoLineOfSight),
        ] {
            let (mut crowd, who, mobile) = standing_crowd();
            crowd.begin_action(phase(
                mobile,
                CombatActionKind::Swing,
                ActionPhase::Releasing {
                    impact_in: openshard_protocol::feedback::SwingDuration(1_600),
                },
            ));
            crowd.advance(Duration::from_millis(800));
            crowd.end_action(CombatActionEnded {
                actor: mobile,
                outcome,
            });
            assert_eq!(
                crowd.preparing(who),
                Some(ActionProgress {
                    running: None,
                    ended:   Some(outcome),
                    balked:  None,
                }),
                "a {outcome:?} left the wrong state on screen",
            );

            crowd.advance(OUTCOME_HOLD);
            assert_eq!(
                crowd.preparing(who),
                None,
                "a {outcome:?} is a message and not a state to hold",
            );
        }
    }

    /// The case the two halves were split for. A fighter's next gesture opens
    /// on the tick the last one landed, so the commit and the ending arrive
    /// together; a record that held one *or* the other would show the verdict
    /// for a single frame, and "hit" would be unreadable in every fight that
    /// went beyond one blow.
    #[test]
    fn a_verdict_stands_beside_the_next_blow_it_arrived_with() {
        let (mut crowd, who, mobile) = standing_crowd();
        let swing = phase(
            mobile,
            CombatActionKind::Swing,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(1_600),
            },
        );
        crowd.begin_action(swing);
        crowd.advance(Duration::from_millis(1_600));
        // Both on the same tick, in the order the shard sends them: the blow
        // resolves and the next one is committed in the same pass.
        crowd.end_action(CombatActionEnded {
            actor:   mobile,
            outcome: CombatActionOutcome::Hit,
        });
        crowd.begin_action(swing);

        assert_eq!(
            crowd.preparing(who),
            Some(ActionProgress {
                running: Some(RunningAction {
                    kind: CombatActionKind::Swing,
                    fill: ActionFill::Releasing { filled: 0.0 },
                    stage: ActionStage::FIRST,
                    released_from_held_draw: false,
                }),
                ended:   Some(CombatActionOutcome::Hit),
                balked:  None,
            }),
        );

        // And the verdict is the *previous* blow's alone: it goes at its own
        // hold, without waiting for the swing it is standing beside.
        crowd.advance(OUTCOME_HOLD);
        assert_eq!(
            crowd.preparing(who).expect("the swing is still running").ended,
            None,
        );
    }

    /// The bound on a leak, not part of the mechanism: an action whose end
    /// packet this client never heard — the actor walked out of range mid-swing
    /// — must not hold a full bar over its head forever.
    ///
    /// And the other half, which is what the bound is *for*: right up to that
    /// point the bar is still there, full. The interval is this client's
    /// arithmetic and the ending is the shard's fact, and dropping the bar the
    /// moment the arithmetic ran out blanked the tail of every action the shard
    /// was even slightly late to finish. See [`RUNNING_GRACE`].
    #[test]
    fn an_action_whose_end_never_arrived_is_held_full_and_then_let_go() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Breath,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(1_600),
            },
        ));
        crowd.advance(Duration::from_millis(1_601));
        assert_eq!(
            crowd.preparing(who).and_then(|progress| progress.running),
            Some(RunningAction {
                kind: CombatActionKind::Breath,
                fill: ActionFill::Releasing { filled: 1.0 },
                stage: ActionStage::FIRST,
                released_from_held_draw: false,
            }),
            "a blow that is due reads as a full bar, not as an empty patch of sky"
        );
        crowd.advance(RUNNING_GRACE);
        assert_eq!(
            crowd.preparing(who),
            None,
            "and an ending that never came is still bounded"
        );
    }

    /// A refusal is a *state*, and the difference from an outcome is the whole
    /// reason it is a third field: an archer whose quarry is behind a wall is
    /// held up for as long as the wall stands, and a word that faded after a
    /// second would put the picture back to the silence this packet ends.
    #[test]
    fn a_refusal_stands_until_the_shard_lifts_it() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.balk_action(CombatActionBalked {
            actor: mobile,
            balk:  BalkState::Blocked(InterruptReason::OutOfReach),
        });
        assert_eq!(
            crowd.preparing(who),
            Some(ActionProgress {
                running: None,
                ended:   None,
                balked:  Some(InterruptReason::OutOfReach),
            }),
        );

        // Ten times the hold an outcome gets, and it is still there.
        crowd.advance(OUTCOME_HOLD * 10);
        assert_eq!(
            crowd.preparing(who).expect("still held up").balked,
            Some(InterruptReason::OutOfReach),
            "a standing condition does not fade; the shard says when it is over",
        );

        crowd.balk_action(CombatActionBalked {
            actor: mobile,
            balk:  BalkState::Clear,
        });
        assert_eq!(crowd.preparing(who), None);
    }

    /// The belt beside the shard's braces: a commit is itself proof the way is
    /// clear, so a bar can never fill under the words "out of reach" even if the
    /// clearing packet were lost or arrived late.
    #[test]
    fn committing_an_action_clears_a_standing_refusal() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.balk_action(CombatActionBalked {
            actor: mobile,
            balk:  BalkState::Blocked(InterruptReason::NoLineOfSight),
        });
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Shot,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(1_600),
            },
        ));
        assert_eq!(
            crowd.preparing(who),
            running(CombatActionKind::Shot, ActionFill::Releasing { filled: 0.0 }),
        );
    }

    /// A short release after an overwatch is not another draw.  The packet
    /// names only the release interval, so preserve the preceding armed phase
    /// in the presentation record and let the HUD state it alongside the loose.
    #[test]
    fn a_held_bow_keeps_its_drawn_state_when_it_looses() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Shot,
            ActionPhase::Armed {
                endurance: openshard_protocol::feedback::SwingDuration(8_000),
            },
        ));
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Shot,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(400),
            },
        ));
        assert_eq!(
            crowd.preparing(who).and_then(|progress| progress.running),
            Some(RunningAction {
                kind: CombatActionKind::Shot,
                fill: ActionFill::Releasing { filled: 0.0 },
                stage: ActionStage::FIRST,
                released_from_held_draw: true,
            }),
            "the short loose says that the bow had already been drawn",
        );
    }

    /// A draw opens in the first stretch and is *told* every one after it. The
    /// bar and the stretch move independently on purpose: where a draw ends and
    /// an aim begins is an operator setting on the shard, and a client reading
    /// it off its own percentage would be wrong on every shard that retuned it.
    #[test]
    fn a_bow_walks_the_stretches_the_shard_announces() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Shot,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(1_000),
            },
        ));
        assert_eq!(
            crowd.preparing(who),
            staged(
                CombatActionKind::Shot,
                ActionFill::Releasing { filled: 0.0 },
                ActionStage::Ready,
            ),
        );

        crowd.advance(Duration::from_millis(500));
        crowd.stage_action(CombatActionStage {
            actor: mobile,
            stage: ActionStage::Aim,
        });
        assert_eq!(
            crowd.preparing(who),
            staged(
                CombatActionKind::Shot,
                ActionFill::Releasing { filled: 0.5 },
                ActionStage::Aim,
            ),
        );
    }

    /// A stretch of an action nobody here knows about is not an action: the
    /// packet is dropped rather than made into a record with no interval, which
    /// would be a bar this client could neither fill nor age.
    #[test]
    fn a_stage_for_no_running_action_is_dropped() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.stage_action(CombatActionStage {
            actor: mobile,
            stage: ActionStage::Release,
        });
        assert_eq!(crowd.preparing(who), None);
    }

    #[test]
    fn retain_forgets_actions_along_with_position() {
        let (mut crowd, who, mobile) = standing_crowd();
        crowd.begin_action(phase(
            mobile,
            CombatActionKind::Swing,
            ActionPhase::Releasing {
                impact_in: openshard_protocol::feedback::SwingDuration(1_600),
            },
        ));
        crowd.retain(|_| false);
        assert_eq!(crowd.preparing(who), None);
    }

    /// The stroke on screen may be an axe on a tree: a combat action's end is
    /// not this client's licence to stop whatever happens to be playing.
    #[test]
    fn a_combat_interruption_does_not_cancel_a_harvest() {
        let (mut crowd, who, mobile) = chopping_crowd();
        crowd.advance(Duration::from_millis(400));
        crowd.end_action(CombatActionEnded {
            actor:   mobile,
            outcome: CombatActionOutcome::Interrupted(InterruptReason::Abandoned),
        });
        assert_eq!(
            crowd.group_for(who),
            Some(AnimationGroup(13)),
            "the chop keeps swinging through somebody else's combat packet"
        );
    }

    #[test]
    fn a_timed_axe_action_holds_its_windup_until_its_last_complete_stroke() {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.time_swing(SwingTiming {
            serial:   mobile,
            duration: openshard_protocol::feedback::SwingDuration(4_800),
        });
        crowd.play_new(NewAnimation {
            serial:         mobile,
            animation_type: 0,
            action:         7,
            delay:          0,
        });

        // Six frames over 4800ms is eight hundred milliseconds a frame, and the
        // claim is that they are *evenly* eight hundred: the gesture is slowed
        // to fit the interval rather than posed at the start of it.
        // Six frames over 4800ms is eight hundred milliseconds a frame, and the
        // claim is that they are *evenly* eight hundred: the gesture is slowed
        // to fit the interval rather than posed at the start of it. Walked as
        // "at the boundary" and "a millisecond before the next", because an
        // off-by-one here is a frame that never shows.
        assert_eq!(crowd.group_for(who), Some(AnimationGroup(13)));
        for frame in 0..6_u16 {
            assert_eq!(
                crowd.frame_for(who, AnimationFrameCount(6)),
                frame,
                "frame {frame} should begin at {}ms of a 4800ms swing",
                u64::from(frame) * 800
            );
            crowd.advance(Duration::from_millis(799));
            assert_eq!(
                crowd.frame_for(who, AnimationFrameCount(6)),
                frame,
                "and should still be showing a millisecond before the next"
            );
            crowd.advance(Duration::from_millis(1));
        }
        assert_eq!(
            crowd.group_for(who),
            Some(BodyKind::Human.standing()),
            "an exact whole-cycle duration ends at the end of its last stroke"
        );
    }

    /// A combat swing's *interval* is gameplay timing, and the art is stretched
    /// over it: a slower weapon is a slower gesture, all the way through, and
    /// never the same gesture with a pause in it.
    ///
    /// The claim is the ratio rather than a frame number, which is what makes it
    /// a test of the rule instead of of one duration. It is the half of the
    /// picture the bar cannot supply — a bar at sixty percent over a body that
    /// has not moved is a screen contradicting itself, and that was the
    /// arithmetic of the staged wind-up this replaced.
    #[test]
    fn a_timed_swing_spreads_its_art_across_the_whole_interval() {
        // Sampled at the *middle* of each frame's slot — `(2n+1)/14` of the
        // interval — so the claim is about which seventh the art is in and not
        // about which side of a boundary integer division lands on.
        for duration in [1_600_u32, 3_200] {
            for frame in [0_u16, 1, 4, 6] {
                let elapsed = duration * (2 * u32::from(frame) + 1) / 14;
                let expected = frame;
                let who = serial(1);
                let mobile = who.expect("a serial");
                let mut crowd = Crowd::default();
                crowd.see(
                    who,
                    Point::new(10, 10, 0),
                    Graphic(PLAYER),
                    Facing::walking(Direction::South),
                    Hue::NONE,
                    false,
                    false,
                );
                crowd.time_swing(SwingTiming {
                    serial:   mobile,
                    duration: openshard_protocol::feedback::SwingDuration(duration),
                });
                crowd.play_new(NewAnimation {
                    serial:         mobile,
                    animation_type: 0,
                    action:         4, // one-handed slash, seven frames
                    delay:          0,
                });

                crowd.advance(Duration::from_millis(u64::from(elapsed)));
                assert_eq!(
                    crowd.frame_for(who, AnimationFrameCount(7)),
                    expected,
                    "{elapsed}ms into a {duration}ms swing is {expected} sevenths of the art"
                );
            }
        }
    }

    #[test]
    fn a_timed_action_waits_for_a_complete_last_stroke() {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.time_swing(SwingTiming {
            serial:   mobile,
            duration: openshard_protocol::feedback::SwingDuration(500),
        });
        crowd.play_new(NewAnimation {
            serial:         mobile,
            animation_type: 0,
            action:         7,
            delay:          0,
        });

        crowd.advance(Duration::from_millis(500));
        assert_eq!(
            crowd.group_for(who),
            Some(AnimationGroup(13)),
            "the requested interval ends mid-cycle, so do not cut off the axe"
        );
        crowd.advance(Duration::from_millis(460));
        assert_eq!(
            crowd.group_for(who),
            Some(BodyKind::Human.standing()),
            "the action hands the body back only after the next complete cycle"
        );
    }

    #[test]
    fn a_harvest_preview_starts_at_the_target_click_and_waits_for_confirmation() {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.preview_harvest(HarvestPreview {
            cursor_id:   openshard_protocol::wire::CursorId(mobile.raw()),
            serial:      mobile,
            action:      13,
            frame_count: AnimationFrameCount(6),
            duration:    openshard_protocol::feedback::SwingDuration(4_800),
            cycles:      3,
        });
        assert_eq!(crowd.group_for(who), Some(AnimationGroup(13)));
        crowd.advance(Duration::from_millis(800));
        assert_eq!(
            crowd.frame_for(who, AnimationFrameCount(6)),
            3,
            "a tree swing takes 1.6 seconds, with its impact halfway through"
        );
        crowd.advance(Duration::from_millis(800));
        assert_eq!(
            crowd.frame_for(who, AnimationFrameCount(6)),
            0,
            "the chop loops at the end of its full 1.6-second cycle"
        );
        crowd.advance(Duration::from_millis(3_200));
        assert_eq!(
            crowd.group_for(who),
            Some(AnimationGroup(13)),
            "a delayed server answer cannot make the optimistic chop stop"
        );

        confirm_chop(&mut crowd, mobile);
        crowd.advance(Duration::ZERO);
        assert_eq!(
            crowd.group_for(who),
            Some(BodyKind::Human.standing()),
            "confirmation after the predicted endpoint does not add another stroke"
        );
    }

    /// The preview starts at the click, while the confirmation returns anywhere
    /// inside a round trip. That transport timing must not alter the visual
    /// cadence: it is the regression that made chopping look randomly slow or
    /// fast from one tree to the next.
    #[test]
    fn harvest_confirmation_never_retimes_a_stroke() {
        for confirmation_at in [100_u64, 790, 1_590] {
            let (mut crowd, who, mobile) = chopping_crowd();
            crowd.advance(Duration::from_millis(confirmation_at));
            confirm_chop(&mut crowd, mobile);

            // At 800ms into any 1.6-second stroke, the six-frame chop must be
            // on its third frame. The old latency-compensation code changed
            // this answer depending on `confirmation_at`.
            let impact_at = if confirmation_at < 800 { 800 } else { 2_400 };
            crowd.advance(Duration::from_millis(impact_at - confirmation_at));
            assert_eq!(
                crowd.frame_for(who, AnimationFrameCount(6)),
                3,
                "a {confirmation_at}ms confirmation changed the chop tempo"
            );
        }
    }

    #[test]
    fn a_refused_harvest_finishes_its_current_stroke() {
        let who = serial(1);
        let mobile = who.expect("a serial");
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.preview_harvest(HarvestPreview {
            cursor_id:   openshard_protocol::wire::CursorId(mobile.raw()),
            serial:      mobile,
            action:      13,
            frame_count: AnimationFrameCount(6),
            duration:    openshard_protocol::feedback::SwingDuration(4_800),
            cycles:      3,
        });
        crowd.advance(Duration::from_millis(100));
        crowd.refuse_harvest(HarvestRefused { serial: mobile });
        assert_eq!(crowd.group_for(who), Some(AnimationGroup(13)));
        crowd.advance(Duration::from_millis(1_500));
        assert_eq!(
            crowd.group_for(who),
            Some(BodyKind::Human.standing()),
            "a refusal returns to standing at the cycle boundary, never mid-swing"
        );
    }

    #[test]
    fn a_completed_harvest_stops_its_prediction_at_the_current_stroke_boundary() {
        let (mut crowd, who, mobile) = chopping_crowd();
        crowd.advance(Duration::from_millis(3_500));
        crowd.complete_harvest(openshard_protocol::feedback::HarvestCompleted { serial: mobile });

        crowd.advance(Duration::from_millis(1_299));
        assert_eq!(
            crowd.group_for(who),
            Some(AnimationGroup(13)),
            "the final stroke is not cut off when the logs arrive"
        );
        crowd.advance(Duration::from_millis(1));
        assert_eq!(
            crowd.group_for(who),
            Some(BodyKind::Human.standing()),
            "the harvest completion prevents another complete loop"
        );
    }

    /// A movement update may change the displayed group before the one-shot
    /// attack packet has finished.  The action still supplies timing, but it
    /// must never nominate a frame outside the newly displayed group's atlas
    /// range: that is a missing key and therefore an invisible body.
    #[test]
    fn an_action_overlapping_a_run_stays_within_the_runs_frames() {
        let who = serial(1);
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.play_new(NewAnimation {
            serial:         who.expect("a serial"),
            animation_type: 0,
            action:         0,
            delay:          0,
        });
        // A human unarmed attack has seven frames, while the running group may
        // have fewer.  Movement owns the displayed group, so this is the
        // overlap that previously let the action reach a nonexistent run frame.
        crowd.see(
            who,
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            Facing::running(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(crowd.group_for(who), Some(AnimationGroup(2)));

        for _ in 0..7 {
            let frame = crowd.frame_for(who, AnimationFrameCount(2));
            assert!(frame < 2, "run frame {frame} must be packed");
            crowd.advance(Duration::from_millis(80));
        }
    }

    #[test]
    fn an_in_flight_step_reports_the_tile_the_body_is_leaving() {
        let who = serial(4);
        let mut crowd = Crowd::default();
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.see(
            who,
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::East),
            Hue::NONE,
            false,
            false,
        );

        assert_eq!(crowd.stepping_from(who), Some(Point::new(10, 10, 0)));
    }

    /// War is a *group*, and it reaches the body through every door: the packet
    /// that first shows it, the packet that changes its mind while it stands
    /// still, the step it takes, and the walk timing out afterwards.
    ///
    /// Four assertions because those are four different code paths into
    /// `change_to`, and the one that was easiest to forget is the third: a
    /// stance drawn only when a body moves is a sword nobody sees until they
    /// take a step.
    #[test]
    fn a_human_at_war_stands_and_walks_in_the_war_groups() {
        let mut crowd = Crowd::default();
        let see = |crowd: &mut Crowd, x: u16, war: bool| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                war,
                false,
            )
        };
        assert_eq!(
            see(&mut crowd, 10, true).group,
            7,
            "a body first seen at war stands at war from its first frame"
        );
        assert_eq!(
            see(&mut crowd, 10, false).group,
            4,
            "and sheathing it, standing still, is seen without a step"
        );
        assert_eq!(see(&mut crowd, 10, true).group, 7, "as is drawing it again");
        assert_eq!(
            see(&mut crowd, 11, true).group,
            15,
            "PeopleAnimationGroup.WalkWarmode"
        );
        crowd.advance(openshard_movement::WALK_HOLD * 2);
        assert_eq!(
            crowd.group_for(serial(1)),
            Some(AnimationGroup(7)),
            "the walk times out into the stance it was walking in"
        );
    }

    /// Mounted is a *group* the same way war is, and reaches the body through
    /// the same doors: first sight, standing still with no fresh step, a step
    /// itself, and the walk timing out afterwards. Mirrors
    /// [`a_human_at_war_stands_and_walks_in_the_war_groups`] because
    /// [`entry_stand`]/[`entry_move`] are the same functions war goes through.
    #[test]
    fn a_mounted_human_stands_and_moves_in_the_mounted_groups() {
        let mut crowd = Crowd::default();
        let see = |crowd: &mut Crowd, x: u16, running: bool, mounted: bool| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                match running {
                    true => Facing::running(Direction::South),
                    false => Facing::walking(Direction::South),
                },
                Hue::NONE,
                false,
                mounted,
            )
        };
        assert_eq!(
            see(&mut crowd, 10, false, true).group,
            25,
            "a body first seen mounted sits the saddle from its first frame"
        );
        assert_eq!(
            see(&mut crowd, 10, false, false).group,
            4,
            "and dismounting, standing still, is seen without a step"
        );
        assert_eq!(
            see(&mut crowd, 10, false, true).group,
            25,
            "as is swinging back into the saddle"
        );
        assert_eq!(
            see(&mut crowd, 11, false, true).group,
            23,
            "PeopleAnimationGroup.OnmountRideSlow"
        );
        assert_eq!(
            see(&mut crowd, 12, true, true).group,
            24,
            "PeopleAnimationGroup.OnmountRideFast"
        );
        crowd.advance(openshard_movement::MOUNTED_RUN_HOLD * 2);
        assert_eq!(
            crowd.group_for(serial(1)),
            Some(AnimationGroup(25)),
            "the ride times out into the mounted stand"
        );
    }

    /// A cougar is an animal whose body id sits among the monsters, and every
    /// group it plays is chosen from the install's table rather than from that
    /// id.
    ///
    /// Three groups, all wrong before the table was read: a monster has no run
    /// at all (`BodyKind::running` is `None`), so a sprinting cougar was drawn
    /// walking; its stand is 1 in the high numbering and 2 in the low one; and
    /// its attack is 4 rather than 5 — a group the file has in one direction of
    /// five, so the creature had no frame to draw and vanished mid-swing.
    #[test]
    fn a_cougar_plays_the_animal_groups_its_body_id_would_deny_it() {
        const COUGAR: Graphic = Graphic(63);
        let mut crowd = Crowd::default();
        // One row of the shipped `mobtypes.txt`, as the file writes it.
        crowd.set_mob_types(MobTypes::from_text("63\tANIMAL\t20\t# Cougar\n"));
        assert_eq!(
            BodyKind::of(COUGAR),
            BodyKind::Monster,
            "the id-range rule this test exists to replace",
        );

        let at = Point::new(10, 10, 0);
        let standing = crowd.see(
            serial(1),
            at,
            COUGAR,
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(
            standing.group,
            BodyKind::Animal.standing(),
            "LowAnimationGroup.Stand, not the high numbering's 1",
        );

        let running = crowd.see(
            serial(1),
            Point::new(11, 10, 0),
            COUGAR,
            Facing::running(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(
            running.group,
            BodyKind::Animal.running().expect("an animal runs"),
            "a monster has no run, so this used to be the walk",
        );

        // `0xE2`'s attack category, which the client turns into a group itself.
        crowd.play_new(NewAnimation {
            serial:         serial(1).expect("a real mobile serial"),
            animation_type: 0,
            action:         0,
            delay:          0,
        });
        assert_eq!(
            crowd.group_for(serial(1)),
            Some(BodyKind::Animal.attacking()),
            "LowAnimationGroup.Attack1",
        );
        assert_eq!(
            BodyKind::Monster.attacking(),
            AnimationGroup(4),
            "and the group it used to be sent, kept here so this stops passing if 4 becomes right",
        );
    }

    /// A run is a run whatever the stance, and a horse has no war stance at all.
    /// Both are the reference's own rules and both are easy to get wrong in the
    /// same place — `Tracked::moving_group` is where they meet.
    #[test]
    fn running_and_animals_are_untouched_by_war() {
        let mut crowd = Crowd::default();
        let running = crowd.see(
            serial(1),
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::running(Direction::South),
            Hue::NONE,
            true,
            false,
        );
        assert_eq!(running.group, 7, "standing still, even in a running facing");
        let running = crowd.see(
            serial(1),
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            Facing::running(Direction::South),
            Hue::NONE,
            true,
            false,
        );
        assert_eq!(running.group, 2, "RunUnarmed: a sprint is not a war walk");

        let horse = crowd.see(
            serial(2),
            Point::new(10, 10, 0),
            Graphic(HORSE),
            Facing::walking(Direction::South),
            Hue::NONE,
            true,
            false,
        );
        assert_eq!(horse.group, 2, "LowAnimationGroup.Stand, war or not");
        let horse = crowd.see(
            serial(2),
            Point::new(11, 10, 0),
            Graphic(HORSE),
            Facing::walking(Direction::South),
            Hue::NONE,
            true,
            false,
        );
        assert_eq!(horse.group, 0, "and it walks in the walk it has");
    }

    /// A step starts a walk, and the walk ends by itself: nothing on the wire
    /// says "stopped".
    #[test]
    fn a_step_walks_and_silence_stands() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                false,
            )
        };
        assert_eq!(step(&mut crowd, 10).group, 4, "standing to begin with");
        assert_eq!(step(&mut crowd, 11).group, 0, "a step is a walk");

        // Most of a step later it is still walking, which is what keeps a body
        // that is genuinely walking from flickering between two animations.
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(step(&mut crowd, 11).group, 0, "no new step, but not done yet");
        crowd.advance(WALK_HOLD);
        assert_eq!(step(&mut crowd, 11).group, 4, "and then it stands");
    }

    /// A single step of *our own* body, with nothing at all calling `see`
    /// again afterwards — `App::walk` offline, one key press, then silence.
    ///
    /// `a_step_walks_and_silence_stands` above covers a body this client only
    /// hears about, and re-asks `Crowd::see` at the same position on every
    /// check — which itself pokes at the `Tracked` the same way a fresh
    /// packet would. The one body that is never re-`see`n while it stands
    /// still is the commanded one: `App::about_to_wait`'s safety-net timer
    /// only ever calls `Crowd::advance`. If the walk-to-standing transition
    /// depended on anything `see` does, this is the case that would miss it.
    #[test]
    fn the_commanded_body_stops_walking_when_nothing_asks_it_to_walk_again() {
        let mut crowd = Crowd::default();
        let facing = Facing::walking(Direction::East);
        crowd.see(
            None,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        let stepped = crowd.see(
            None,
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(stepped.group, 0, "a step is a walk");

        // Nothing walks it further. Advanced in `FRAME_DELAY`-sized ticks —
        // the safety net's own granularity — well past any hold this step
        // could be owed.
        for _ in 0..40 {
            crowd.advance(openshard_client_render::animation::FRAME_DELAY);
        }
        assert_eq!(
            crowd.tracked.get(&None).expect("still tracked").group,
            BodyKind::of(Graphic(PLAYER)).standing(),
            "stopped walking once nothing was left to hold it there"
        );
    }

    /// The `Mobile` a caller got back from `see`/`snap` is a snapshot, not a
    /// window — its `group` does not follow the body's later automatically
    /// once the walk gives up. `App::draw` used to read `mobile.group` off
    /// exactly such a snapshot (`self.player`, cached from the last `see`)
    /// instead of asking the crowd again every frame, which is the source of
    /// the walking-in-place complaint this pair of tests chases: a body that
    /// really had stopped, drawn from a `Mobile` that still said so.
    /// [`Crowd::group_for`] is what a caller has to re-ask instead.
    #[test]
    fn a_returned_mobiles_group_goes_stale_but_group_for_does_not() {
        let mut crowd = Crowd::default();
        let facing = Facing::walking(Direction::East);
        crowd.see(
            None,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        let stepped = crowd.see(
            None,
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(stepped.group, 0, "walking, in the snapshot returned");

        crowd.advance(WALK_HOLD * 2);
        assert_eq!(
            stepped.group, 0,
            "the snapshot itself cannot change — it is a plain value"
        );
        assert_eq!(
            crowd.group_for(None),
            Some(BodyKind::of(Graphic(PLAYER)).standing()),
            "but the crowd, asked again, has moved on"
        );
    }

    /// A body that keeps stepping keeps walking, however long it goes on.
    #[test]
    fn a_body_that_keeps_stepping_never_stands() {
        let mut crowd = Crowd::default();
        for x in 10..30u16 {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                false,
            );
            // Each step arrives before the previous one has finished, which is
            // what a real walk looks like.
            crowd.advance(WALK_HOLD * 3 / 4);
        }
        let drawn = crowd.see(
            serial(1),
            Point::new(30, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(drawn.group, 0);
    }

    /// Turning on the spot is not a step.
    ///
    /// A facing change arrives with every step too, so a layer that watched the
    /// facing instead of the position would keep a standing crowd walking on
    /// the spot forever — and would still pass the test above.
    #[test]
    fn a_turn_on_the_spot_is_not_a_step() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        crowd.see(
            serial(1),
            at,
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        let turned = crowd.see(
            serial(1),
            at,
            Graphic(PLAYER),
            Facing::walking(Direction::North),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(turned.group, 4, "still standing");
        assert_eq!(turned.facing, Direction::North, "and facing the new way");
    }

    /// Running is the wire's own flag, and a monster has no run to play.
    #[test]
    fn running_is_a_group_of_its_own_where_the_kind_has_one() {
        let mut crowd = Crowd::default();
        let run = |crowd: &mut Crowd, who: u32, body: u16, x: u16| {
            crowd.see(
                serial(who),
                Point::new(x, 10, 0),
                Graphic(body),
                Facing::running(Direction::South),
                Hue::NONE,
                false,
                false,
            )
        };
        run(&mut crowd, 1, PLAYER, 10);
        assert_eq!(run(&mut crowd, 1, PLAYER, 11).group, 2, "RunUnarmed");
        run(&mut crowd, 2, HORSE, 10);
        assert_eq!(run(&mut crowd, 2, HORSE, 11).group, 1, "LowAnimationGroup.Run");
        run(&mut crowd, 3, DRAGON, 10);
        assert_eq!(run(&mut crowd, 3, DRAGON, 11).group, 0, "High walks instead");
    }

    /// Everybody keeps their own clock, so a crowd does not breathe in unison.
    #[test]
    fn two_bodies_that_started_at_different_times_are_on_different_frames() {
        let mut crowd = Crowd::default();
        let stand = |crowd: &mut Crowd, who: u32| {
            crowd.see(
                serial(who),
                Point::new(10, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                false,
            );
        };
        stand(&mut crowd, 1);
        crowd.advance(Duration::from_millis(80 * 3));
        stand(&mut crowd, 2);
        assert_eq!(crowd.frame_for(serial(1), AnimationFrameCount(6)), 3);
        assert_eq!(
            crowd.frame_for(serial(2), AnimationFrameCount(6)),
            0,
            "the newcomer starts at zero"
        );
        // And a serial nobody is tracking answers with a frame rather than
        // nothing: the atlas may hold a body the crowd has forgotten.
        assert_eq!(crowd.frame_for(serial(3), AnimationFrameCount(6)), 0);
    }

    /// A group change restarts the clock, so a walk begins at its first frame
    /// rather than wherever the stand had got to.
    #[test]
    fn changing_group_restarts_the_animation() {
        let mut crowd = Crowd::default();
        crowd.see(
            serial(1),
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        crowd.advance(Duration::from_millis(80 * 5));
        assert_eq!(crowd.frame_for(serial(1), AnimationFrameCount(6)), 5);
        crowd.see(
            serial(1),
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(
            crowd.frame_for(serial(1), AnimationFrameCount(6)),
            0,
            "the walk starts at its start"
        );
    }

    /// A turn picks a different direction's sprites, not a new walking
    /// animation.  The clock belongs to the body and group, so the new facing
    /// has to continue at the frame its previous facing had reached.
    #[test]
    fn turning_while_walking_keeps_the_stride_phase() {
        let mut crowd = Crowd::default();
        let who = serial(1);
        crowd.see(
            who,
            Point::new(10, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::East),
            Hue::NONE,
            false,
            false,
        );
        crowd.see(
            who,
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::East),
            Hue::NONE,
            false,
            false,
        );
        crowd.advance(Duration::from_millis(80 * 3));
        let before_turn = crowd.frame_for(who, AnimationFrameCount(6));
        assert_eq!(before_turn, 3);

        let turned = crowd.see(
            who,
            Point::new(11, 9, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::North),
            Hue::NONE,
            false,
            false,
        );

        assert_eq!(turned.facing, Direction::North, "the sprite set changed");
        assert_eq!(turned.group, AnimationGroup(0), "it is still walking");
        assert_eq!(
            crowd.frame_for(who, AnimationFrameCount(6)),
            before_turn,
            "the new direction continues the existing stride rather than restarting it"
        );
        crowd.advance(Duration::from_millis(80));
        assert_eq!(
            crowd.frame_for(who, AnimationFrameCount(6)),
            4,
            "the shared cycle advances"
        );
    }

    /// A step is walked across, not jumped: the body leaves the tile it was
    /// standing on and reaches the new one exactly when the walk ends.
    #[test]
    fn a_step_is_glided_across_and_ends_on_its_tile() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::SouthEast),
                Hue::NONE,
                false,
                false,
            )
        };
        let standing = step(&mut crowd, 10);
        assert_eq!(
            standing.drawn,
            Gaze::on(Point::new(10, 10, 0)),
            "standing still is on its tile",
        );

        let stepped = step(&mut crowd, 11);
        assert_eq!(
            stepped.drawn,
            Gaze::on(Point::new(10, 10, 0)),
            "a step begins where the body was, not where it is going",
        );

        // A quarter and a half of the way across, in the pixels the sprite is
        // actually placed at: a step south-east is 22 pixels down the screen and
        // none across.
        let quarter = crossed(&mut crowd, WALK_HOLD / 4);
        assert!((quarter - 0.25).abs() < 1e-6, "{quarter}");
        let half = crossed(&mut crowd, WALK_HOLD / 4);
        assert!((half - 0.5).abs() < 1e-6, "{half}");

        // The instant the walk ends the body is on its tile, exactly — the two
        // have to agree, or the sprite jumps the remaining pixels as the
        // animation stops.
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(crowd.drawn_for(serial(1)), Some(Gaze::on(Point::new(11, 10, 0))));
        // And it keeps *playing* the walk for half a step longer, which is what
        // stops a body that is genuinely walking from standing for a frame
        // between two steps. See `animation_hold`.
        assert_eq!(step(&mut crowd, 11).group, 0, "still playing the walk");
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(step(&mut crowd, 11).group, 4, "standing");
    }

    /// The tile a body is stepping off is reported for exactly the crossing:
    /// from the packet that starts the step to the instant it lands, and not for
    /// the half step the *animation* goes on playing afterwards.
    ///
    /// What reads it is the depth order — a sprite mid-step covers both tiles
    /// and has to sort at the nearer of them — so the two ends matter for
    /// different reasons. Report it too early and a standing body sorts a tile
    /// in front of itself; too late and it keeps drawing over the ground behind
    /// it after it has arrived.
    #[test]
    fn the_tile_being_stepped_off_lasts_exactly_as_long_as_the_crossing() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::SouthEast),
                Hue::NONE,
                false,
                false,
            )
        };
        assert_eq!(step(&mut crowd, 10).from, None, "standing still is on one tile");

        let stepped = step(&mut crowd, 11);
        assert_eq!(
            stepped.from,
            Some(Point::new(10, 10, 0)),
            "the step's first frame is already between two tiles",
        );
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(crowd.stepping_from(serial(1)), Some(Point::new(10, 10, 0)));

        // Landed. The animation is still the walk for half a step more — see
        // `a_step_is_glided_across_and_ends_on_its_tile` — and the ordering is
        // deliberately not tied to that.
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(crowd.stepping_from(serial(1)), None, "arrived");
        assert_eq!(step(&mut crowd, 11).group, 0, "though still playing the walk");

        // A body put somewhere it did not walk to crossed nothing.
        let snapped = crowd.snap(
            serial(1),
            Point::new(40, 40, 0),
            Graphic(PLAYER),
            Facing::walking(Direction::SouthEast),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(snapped.from, None);
        assert_eq!(crowd.stepping_from(serial(1)), None);
        // And a body this crowd has never been told about has no tiles at all.
        assert_eq!(crowd.stepping_from(serial(2)), None);
    }

    /// How far along a step south-east from `(10, 10)` the body is drawn, after
    /// another span of the clock.
    ///
    /// In the drawn position and not in a progress field, because the drawn
    /// position is the only thing anybody sees. A step south-east moves the
    /// body one tile down the screen and nothing across, so the fraction is the
    /// vertical distance over a tile's half-height.
    fn crossed(crowd: &mut Crowd, dt: Duration) -> f64 {
        crowd.advance(dt);
        let at = crowd.drawn_for(serial(1)).expect("a tracked body");
        let from = Gaze::on(Point::new(10, 10, 0));
        let to = Gaze::on(Point::new(11, 10, 0));
        (at.y - from.y) / (to.y - from.y)
    }

    /// The defect the split between the crossing and the animation was written
    /// for: a walking client's steps arrive one step apart give or take the
    /// round trip, and a hold of exactly one step expires in that gap — so the
    /// body stands for a frame every tile, and standing is a different group,
    /// which restarts the walk's clock at frame zero.
    #[test]
    fn a_step_that_arrives_late_does_not_drop_the_walk_to_standing() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            )
        };
        step(&mut crowd, 10);
        step(&mut crowd, 11);
        for x in 12..20u16 {
            // A tenth of a step late, every step: the jitter of a real
            // connection, and enough to have flickered.
            crowd.advance(WALK_HOLD + WALK_HOLD / 10);
            assert_eq!(step(&mut crowd, x).group, 0, "walking at tile {x}");
        }
    }

    /// The last step of a walk does not play itself out on the spot.
    ///
    /// The complaint: the character finishes walking and takes one more stride
    /// where it stands. It is the animation hold — the walk's group is kept for
    /// half a step past the crossing so that a walker does not flicker into
    /// standing between two tiles — and the clock was running through it, which
    /// is two and a half frames of stride over ground already covered.
    ///
    /// So the frame is frozen instead, which is what the reference does
    /// (`Mobile.NoIterateAnimIndex`). Both halves are asserted: the frame stops
    /// where the crossing ended, and the group is still the walk while it does —
    /// a body that dropped to standing here would also have a still frame, and
    /// it is the flicker the hold exists to prevent.
    #[test]
    fn the_last_step_of_a_walk_does_not_play_itself_out() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            )
        };
        step(&mut crowd, 10);
        assert_eq!(step(&mut crowd, 11).group, 0, "walking");

        // The crossing: the frame moves, because the body is covering ground.
        crowd.advance(WALK_HOLD);
        let arrived = crowd.frame_for(serial(1), AnimationFrameCount(6));
        assert!(arrived > 0, "the walk played while it walked");
        assert_eq!(
            crowd.drawn_for(serial(1)),
            Some(Gaze::on(Point::new(11, 10, 0))),
            "and the tile is crossed",
        );

        // The hold: the group is held and the picture is not. Three eighths of
        // a step and not four, because the fourth lands exactly on
        // `animation_hold` — where the body is *meant* to drop to standing, and
        // a test that straddled the boundary would be asserting about which
        // side of it a rounding fell on.
        for _ in 0..3 {
            crowd.advance(WALK_HOLD / 8);
            assert_eq!(
                crowd.frame_for(serial(1), AnimationFrameCount(6)),
                arrived,
                "the stride played on after the walking stopped",
            );
            assert_eq!(
                step(&mut crowd, 11).group,
                0,
                "and it is still holding the walk rather than standing",
            );
        }
        // And then it stands, which is a different group and its own clock.
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(step(&mut crowd, 11).group, 4);
        assert_eq!(crowd.frame_for(serial(1), AnimationFrameCount(6)), 0);
    }

    /// The companion, and it is the whole of what makes the test above an
    /// assertion: a walk that is still walking keeps playing.
    ///
    /// A freeze applied to every frame would pass the test above and produce a
    /// character that slides across the map with its feet still — which is the
    /// same false green a still scene always is.
    #[test]
    fn a_walk_that_is_still_walking_keeps_playing() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            );
        };
        step(&mut crowd, 10);
        let mut frames = std::collections::BTreeSet::new();
        for x in 11..16u16 {
            step(&mut crowd, x);
            for _ in 0..5 {
                crowd.advance(WALK_HOLD / 5);
                frames.insert(crowd.frame_for(serial(1), AnimationFrameCount(6)));
            }
        }
        assert!(
            frames.len() >= 5,
            "a walk of five tiles showed {} distinct frames",
            frames.len(),
        );
    }

    /// And the next step picks the stride up where it was frozen rather than
    /// restarting it.
    ///
    /// Free, because the group has not changed and only a change restarts the
    /// clock — and worth pinning, because the alternative reads as a body that
    /// hesitates every time the wire is slow.
    #[test]
    fn a_step_after_the_freeze_carries_the_stride_on() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            );
        };
        step(&mut crowd, 10);
        step(&mut crowd, 11);
        crowd.advance(WALK_HOLD + WALK_HOLD / 4);
        let frozen = crowd.frame_for(serial(1), AnimationFrameCount(6));
        step(&mut crowd, 12);
        assert_eq!(
            crowd.frame_for(serial(1), AnimationFrameCount(6)),
            frozen,
            "no hesitation"
        );
        crowd.advance(Duration::from_millis(80));
        assert_ne!(
            crowd.frame_for(serial(1), AnimationFrameCount(6)),
            frozen,
            "and it walks on"
        );
    }

    /// The offline placeholder's steps are ours from the first frame, without
    /// anything having to say so.
    ///
    /// A body we command crosses its tile in the nominal time — we sent the step
    /// and we know when — where a body we merely hear about is glided over the
    /// gap that was measured, wake jitter and all. Nothing but this client can
    /// move the placeholder, so it is the first case, and it used to be armed by
    /// hand in `App::start_replay`: every other path that walks it offline, and
    /// there is one per key, measured the walk through the event loop instead.
    #[test]
    fn a_client_with_no_serial_commands_the_body_it_walks() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                None,
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            );
        };
        step(&mut crowd, 10);
        // Half a step late, which is what a wake the loop slept through looks
        // like. Measured, the next crossing would take one and a half steps.
        crowd.advance(WALK_HOLD + WALK_HOLD / 2);
        step(&mut crowd, 11);
        crowd.advance(WALK_HOLD - Duration::from_millis(1));
        let short = arrived(&crowd, None, Point::new(11, 10, 0));
        assert!(
            short > 0.0 && short < 0.2,
            "all but arrived: {short} pixels short"
        );
        crowd.advance(Duration::from_millis(1));
        assert_eq!(
            crowd.drawn_for(None),
            Some(Gaze::on(Point::new(11, 10, 0))),
            "the nominal step, not the gap the loop happened to wake at",
        );
    }

    /// The step we command ends when the cadence says, whenever we heard about
    /// it.
    ///
    /// The jerk this whole pair of rules came from: `steer.rs` arms each step
    /// from the previous deadline, so the *asks* are an exact metronome, but the
    /// news of each one reaches this layer whenever the loop wakes. A crossing
    /// measured from the arrival is the right length starting at the wrong
    /// instant, by a different wrong instant every tile — so the body's position
    /// steps at every boundary and the camera, locked to it, takes the whole
    /// world with it.
    ///
    /// Here the second step is heard a tenth of a step late and must still
    /// arrive on the beat, which it does by being that much shorter.
    #[test]
    fn a_step_we_command_arrives_on_the_beat_however_late_it_was_heard() {
        let mut crowd = Crowd::default();
        crowd.commanding(None);
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                None,
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            );
        };
        let late = WALK_HOLD / 10;
        step(&mut crowd, 10);
        step(&mut crowd, 11);
        crowd.advance(WALK_HOLD + late);
        step(&mut crowd, 12);

        // A hold after the *first* step's own deadline, less the lateness, the
        // body is on its tile — which is to say two tiles in two holds, and not
        // two tiles in two holds plus the two latenesses.
        crowd.advance(WALK_HOLD - late - Duration::from_millis(1));
        let short = arrived(&crowd, None, Point::new(12, 10, 0));
        assert!(
            short > 0.0 && short < 0.2,
            "all but arrived: {short} pixels short"
        );
        crowd.advance(Duration::from_millis(1));
        assert_eq!(
            crowd.drawn_for(None),
            Some(Gaze::on(Point::new(12, 10, 0))),
            "on the beat",
        );
    }

    /// A step starts from where the body is drawn, not from the tile it is
    /// leaving — so nothing an arrival does can move the sprite.
    ///
    /// The case that names the difference is the opposite of the one above: a
    /// step heard *early*, while the previous crossing still has a quarter to
    /// run. Anchored to the tile boundary the sprite jumps a quarter of a tile
    /// backwards to start the new step; anchored to itself it carries on from
    /// where it is and covers the extra ground by going a little faster.
    #[test]
    fn a_step_picks_up_from_where_the_body_is_drawn() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            )
        };
        step(&mut crowd, 10);
        step(&mut crowd, 11);
        crowd.advance(WALK_HOLD * 3 / 4);
        let before = drawn(&crowd, serial(1));
        let before_gaze = crowd.drawn_for(serial(1)).expect("a tracked body");

        let stepped = step(&mut crowd, 12);
        let after = drawn(&crowd, serial(1));
        assert_eq!(before, after, "the sprite moved on an arrival, not on a clock");
        assert_eq!(
            stepped.drawn, before_gaze,
            "and the mobile handed over says the same",
        );
    }

    /// Where a body is drawn, to a thousandth of a pixel.
    ///
    /// Rounded because the comparison is about a quarter of a tile, and an exact
    /// one would be asserting about the last bit of an `f64` division.
    fn drawn(crowd: &Crowd, who: Who) -> (i64, i64) {
        let (x, y) = crowd.drawn_for(who).expect("a tracked body").exact();
        ((x * 1_000.0).round() as i64, (y * 1_000.0).round() as i64)
    }

    /// How far along the segment from `from` to `to` a body is drawn.
    ///
    /// The fraction in pixels, which is the only place it exists now: there is
    /// no progress field to read, because the sprite's position is the answer
    /// and a second number beside it could disagree with the picture.
    fn halfway(crowd: &Crowd, who: Who, from: Point, to: Point) -> f64 {
        let at = crowd.drawn_for(who).expect("a tracked body").exact();
        let (from, to) = (Gaze::on(from).exact(), Gaze::on(to).exact());
        let span = (to.0 - from.0).hypot(to.1 - from.1);
        (at.0 - from.0).hypot(at.1 - from.1) / span
    }

    /// How far short of `tile` a body still is, in pixels.
    fn arrived(crowd: &Crowd, who: Who, tile: Point) -> f64 {
        let at = crowd.drawn_for(who).expect("a tracked body");
        let (x, y) = at.exact();
        let (tx, ty) = Gaze::on(tile).exact();
        (x - tx).hypot(y - ty)
    }

    /// And a walk already under way crosses each tile in the time the last one
    /// took, so a body whose steps arrive slower than the nominal rate glides
    /// the whole way rather than arriving early and waiting.
    #[test]
    fn the_crossing_takes_as_long_as_the_last_step_did() {
        let mut crowd = Crowd::default();
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            );
        };
        step(&mut crowd, 10);
        step(&mut crowd, 11);
        // Half a step late. The next crossing is measured from it, so it takes
        // one and a half steps and is still going when the one after arrives.
        let gap = WALK_HOLD + WALK_HOLD / 2;
        crowd.advance(gap);
        step(&mut crowd, 12);
        crowd.advance(gap - Duration::from_millis(1));
        let short = arrived(&crowd, serial(1), Point::new(12, 10, 0));
        assert!(
            short > 0.0 && short < 0.2,
            "all but arrived: {short} pixels short"
        );
        crowd.advance(Duration::from_millis(1));
        assert_eq!(
            crowd.drawn_for(serial(1)),
            Some(Gaze::on(Point::new(12, 10, 0))),
            "and arrived exactly on time",
        );
    }

    /// The two representations are one body: the picture converges on the tile
    /// and never leaves it by more than a walk's worth of lag.
    ///
    /// This is the whole contract between them. `at` is the server's word and
    /// the only thing anything but the sprite reads — depth order, atlas key,
    /// distance, targeting. `drawn` is the physical body: it accelerates,
    /// coasts, and is behind while it does. They are allowed to disagree, and
    /// what makes that safe rather than a second source of truth is that the
    /// disagreement is **bounded while walking and zero at rest**.
    ///
    /// Both halves are here because either alone is satisfied by something
    /// broken: a picture nailed to the tile converges trivially, and a picture
    /// that drifts off and never comes back is bounded on any single frame.
    #[test]
    fn the_drawn_body_trails_its_tile_and_always_catches_up() {
        let mut crowd = Crowd::default();
        crowd.set_ease(Ease::WALK);
        crowd.commanding(None);
        let step = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                None,
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::East),
                Hue::NONE,
                false,
                false,
            );
        };
        // A walk is 78 pixels a second, so an `Ease` of 0.08 settles the lag
        // at about 6.3 pixels. Ten is the bound with room for the frame the ease
        // is sampled on; a hundred would pass for a body left behind entirely.
        const BOUND: f64 = 10.0;
        let mut worst = 0.0f64;
        let mut moving = false;
        step(&mut crowd, 10);
        for tile in 11..21u16 {
            step(&mut crowd, tile);
            for _ in 0..25 {
                crowd.advance(WALK_HOLD / 25);
                let drawn = crowd.drawn_for(None).expect("a tracked body").exact();
                // Against the *unfiltered* walk and not against the destination
                // tile: mid-step the body is legitimately most of a tile short
                // of the tile it is walking onto, and measuring that would call
                // the step itself a lag. What the ease holds is the difference
                // between where the body is drawn and where the step arithmetic
                // says it is, which is the only thing an `Ease` controls.
                let behind = {
                    let walked = crowd.tracked[&None].gaze_at(crowd.now).exact();
                    (drawn.0 - walked.0).hypot(drawn.1 - walked.1)
                };
                worst = worst.max(behind);
                moving |= behind > 1.0;
            }
        }
        assert!(moving, "nothing was ever behind: the ease did not happen");
        assert!(
            worst < BOUND,
            "the picture fell {worst:.1} pixels behind its tile"
        );

        // And the walk stops. The lag is spent coasting — which *is* the
        // ease-out — and what it converges on is the tile, exactly.
        for _ in 0..200 {
            crowd.advance(Duration::from_millis(16));
        }
        let at = crowd.tracked[&None].at;
        let (drawn, tile) = (
            crowd.drawn_for(None).expect("a tracked body").exact(),
            Gaze::on(at).exact(),
        );
        let left = (drawn.0 - tile.0).hypot(drawn.1 - tile.1);
        assert!(
            left < 0.01,
            "the body settled {left} pixels off the tile it is standing on",
        );
    }

    /// And with no ease the two paths are the same number, to the bit.
    ///
    /// What keeps [`Ease::NONE`] honest as a baseline, the way `Rig::HARD` is
    /// one (D1): if the eased path cannot express "no ease at all" exactly,
    /// every measurement taken against the baseline is measuring a filter nobody
    /// asked for.
    #[test]
    fn no_ease_draws_the_body_on_the_walks_own_arithmetic() {
        let mut eased = Crowd::default();
        eased.set_ease(Ease::NONE);
        let mut plain = Crowd::default();
        for tile in 11..16u16 {
            for crowd in [&mut eased, &mut plain] {
                crowd.see(
                    serial(1),
                    Point::new(tile, 10, 0),
                    Graphic(PLAYER),
                    Facing::walking(Direction::East),
                    Hue::NONE,
                    false,
                    false,
                );
            }
            for _ in 0..7 {
                for crowd in [&mut eased, &mut plain] {
                    crowd.advance(WALK_HOLD / 7);
                }
                assert_eq!(
                    eased.drawn_for(serial(1)),
                    plain.drawn_for(serial(1)),
                    "an ease of zero is not the same as no ease at tile {tile}",
                );
            }
        }
        // The companion: the body did move, so the equality above is not two
        // still pictures agreeing.
        assert_ne!(plain.drawn_for(serial(1)), Some(Gaze::on(Point::new(11, 10, 0))));
    }

    /// A gap that is not a pace is not believed: a body that had stopped, or two
    /// steps arriving in one burst, are glided at the rate the wire claims.
    #[test]
    fn a_gap_that_is_not_a_pace_falls_back_to_the_wires_own_rate() {
        assert_eq!(glide_time(WALK_HOLD, None), WALK_HOLD);
        assert_eq!(
            glide_time(WALK_HOLD, Some(Duration::from_secs(30))),
            WALK_HOLD,
            "a body that had stopped and started again"
        );
        assert_eq!(
            glide_time(WALK_HOLD, Some(Duration::from_millis(1))),
            WALK_HOLD,
            "two steps in one burst"
        );
        assert_eq!(
            glide_time(WALK_HOLD, Some(WALK_HOLD + WALK_HOLD / 4)),
            WALK_HOLD + WALK_HOLD / 4,
            "and a plausible pace is taken at its word"
        );
    }

    /// A running body crosses its tile in half the time, because it takes half
    /// as long to take the step.
    ///
    /// The hold and the glide are one number for exactly this reason: held for
    /// a walk's length, a runner would still be half a tile back when the next
    /// step arrived and would jump forward to catch up, every step.
    #[test]
    fn a_runner_glides_in_half_the_time() {
        let mut crowd = Crowd::default();
        let run = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::running(Direction::SouthEast),
                Hue::NONE,
                false,
                false,
            )
        };
        run(&mut crowd, 10);
        run(&mut crowd, 11);
        let half = crossed(&mut crowd, RUN_HOLD / 2);
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        crowd.advance(RUN_HOLD / 2);
        assert_eq!(
            crowd.drawn_for(serial(1)),
            Some(Gaze::on(Point::new(11, 10, 0))),
            "and it is there",
        );
        crowd.advance(RUN_HOLD / 2);
        assert_eq!(run(&mut crowd, 11).group, 4, "standing, half a walk early");
        assert_eq!(RUN_HOLD * 2, WALK_HOLD, "ServUO's RunFoot against its WalkFoot");
    }

    /// A mounted runner gallops in half the time a runner on foot does —
    /// [`a_runner_glides_in_half_the_time`]'s own reasoning, one rung faster,
    /// and the test that exercises `mounted` reaching [`step_hold`] through
    /// [`Crowd::see_inner`] rather than only through the group it picks.
    #[test]
    fn a_mounted_runner_gallops_in_half_a_runners_time() {
        let mut crowd = Crowd::default();
        let gallop = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::running(Direction::SouthEast),
                Hue::NONE,
                false,
                true,
            )
        };
        gallop(&mut crowd, 10);
        gallop(&mut crowd, 11);
        let half = crossed(&mut crowd, openshard_movement::MOUNTED_RUN_HOLD / 2);
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        crowd.advance(openshard_movement::MOUNTED_RUN_HOLD / 2);
        assert_eq!(
            crowd.drawn_for(serial(1)),
            Some(Gaze::on(Point::new(11, 10, 0))),
            "a gallop crosses its tile in a quarter of a walker's time",
        );
        assert_eq!(
            openshard_movement::MOUNTED_RUN_HOLD * 2,
            RUN_HOLD,
            "ServUO's RunMount against its RunFoot"
        );
    }

    /// A rollback is put, not walked: the tile the body is sent back to was
    /// never crossed, so gliding into it would draw the character strolling
    /// backwards a tile at a time for as long as a wall refuses it.
    #[test]
    fn a_refused_step_is_snapped_back_rather_than_glided() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let facing = Facing::walking(Direction::East);
        crowd.see(serial(1), at, Graphic(PLAYER), facing, Hue::NONE, false, false);
        // The client predicts a step east and the server refuses it.
        let stepped = crowd.see(
            serial(1),
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(
            stepped.drawn,
            Gaze::on(at),
            "the prediction is walked into, from here"
        );
        crowd.advance(WALK_HOLD / 4);

        let back = crowd.snap(serial(1), at, Graphic(PLAYER), facing, Hue::NONE, false, false);
        assert_eq!(back.at, at);
        assert_eq!(
            back.drawn,
            Gaze::on(at),
            "and back is a jump, drawn there at once"
        );
        assert_eq!(crowd.drawn_for(serial(1)), Some(Gaze::on(at)));
        assert_eq!(back.group, 0, "still walking: the next step is already coming");

        // The gap between a step and a refusal is not a pace, so the next step
        // is glided at the wire's own rate rather than at a quarter of one.
        crowd.advance(WALK_HOLD / 4);
        crowd.see(
            serial(1),
            Point::new(10, 11, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        crowd.advance(WALK_HOLD / 2);
        assert_eq!(
            halfway(&crowd, serial(1), Point::new(10, 10, 0), Point::new(10, 11, 0)),
            0.5,
            "a full walk's crossing"
        );
    }

    /// A refused step nothing follows up plays no more of its stride: the
    /// body is not gliding (`Tracked::glide` is `None` throughout — it never
    /// left the tile it was snapped back to), so the frame it is drawn on
    /// must not keep changing underneath it either, or the picture is a body
    /// standing still with its legs still moving.
    ///
    /// Caught by tracing this exact scenario frame by frame: before this
    /// fix, `Tracked::striding` kept advancing the clock through the
    /// refused step's original `takes` regardless of `from` being gone, so
    /// the frame walked 1→2→3→4→5 over the next ~300ms while `drawn` sat
    /// fixed the whole time.
    #[test]
    fn a_refused_step_with_nothing_gliding_does_not_advance_its_frame() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let facing = Facing::walking(Direction::East);
        crowd.see(serial(1), at, Graphic(PLAYER), facing, Hue::NONE, false, false);
        crowd.see(
            serial(1),
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        // A quarter step in, the server refuses it — same timing as
        // `a_refused_step_is_snapped_back_rather_than_glided`.
        crowd.advance(WALK_HOLD / 4);
        crowd.snap(serial(1), at, Graphic(PLAYER), facing, Hue::NONE, false, false);

        let frame_just_after_the_snap = crowd.frame_for(serial(1), AnimationFrameCount(6));
        for _ in 0..20 {
            crowd.advance(Duration::from_millis(20));
            assert!(!crowd.anyone_gliding(), "snapped back, never left the tile");
            assert_eq!(
                crowd.frame_for(serial(1), AnimationFrameCount(6)),
                frame_just_after_the_snap,
                "held on the spot, not walking in place"
            );
        }
    }

    /// A refused step with no step behind it to catch it up: the body has
    /// genuinely stopped (a wall it gave up on, a paralyze), not merely
    /// mispredicted its next tile.
    ///
    /// `Crowd::snap` used to clear the step entirely, and the only thing that
    /// ever drops a walking group back to standing is gated on there being one
    /// to time against — so with nothing left to expire, the walk played on
    /// screen forever.
    #[test]
    fn a_refused_step_with_no_step_following_it_still_stops_walking() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let facing = Facing::walking(Direction::East);
        crowd.see(serial(1), at, Graphic(PLAYER), facing, Hue::NONE, false, false);
        let stepped = crowd.see(
            serial(1),
            Point::new(11, 10, 0),
            Graphic(PLAYER),
            facing,
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(stepped.group, 0, "walking");

        let back = crowd.snap(serial(1), at, Graphic(PLAYER), facing, Hue::NONE, false, false);
        assert_eq!(back.group, 0, "still walking right after the refusal");

        // Nothing steps again. The walk it was mid-stride of still has to give
        // up eventually, the same way it would have had the step gone through.
        crowd.advance(WALK_HOLD * 2);
        let standing = BodyKind::of(Graphic(PLAYER)).standing();
        assert_eq!(
            crowd.tracked.get(&serial(1)).expect("still tracked").group,
            standing,
            "the walk gave up rather than playing forever"
        );
    }

    /// A jump of more than one tile is a teleport, and is not glided.
    ///
    /// A gate, a recall, or a `0x22` putting a mispredicted body back: sliding
    /// smoothly across half a facet takes the same 400ms as a step and looks
    /// far stranger than the teleport it is hiding.
    #[test]
    fn a_teleport_is_not_glided() {
        let mut crowd = Crowd::default();
        let go = |crowd: &mut Crowd, x: u16, y: u16| {
            crowd.see(
                serial(1),
                Point::new(x, y, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::SouthEast),
                Hue::NONE,
                false,
                false,
            )
        };
        go(&mut crowd, 10, 10);
        // A diagonal is one step: both axes move, and UO measures Chebyshev.
        assert_eq!(
            go(&mut crowd, 11, 11).drawn,
            Gaze::on(Point::new(10, 10, 0)),
            "a diagonal is a step, and it starts where the body was",
        );
        let jumped = go(&mut crowd, 1500, 1500);
        assert_eq!(
            jumped.drawn,
            Gaze::on(Point::new(1500, 1500, 0)),
            "and this is not"
        );
    }

    /// The window redraws fast only while there is something to redraw fast
    /// for, and a teleport is not one: it moves the body once, in one frame.
    #[test]
    fn only_a_step_asks_the_window_for_frames() {
        let mut crowd = Crowd::default();
        let go = |crowd: &mut Crowd, x: u16| {
            crowd.see(
                serial(1),
                Point::new(x, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::NorthEast),
                Hue::NONE,
                false,
                false,
            );
        };
        go(&mut crowd, 10);
        assert!(!crowd.anyone_gliding(), "standing still");
        go(&mut crowd, 11);
        assert!(crowd.anyone_gliding(), "mid-step");
        crowd.advance(WALK_HOLD);
        assert!(!crowd.anyone_gliding(), "arrived");
        go(&mut crowd, 900);
        assert!(!crowd.anyone_gliding(), "a teleport is drawn once and done");
    }

    /// Turning on the spot moves nothing, so there is nothing to glide across.
    #[test]
    fn a_turn_is_not_glided() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        crowd.see(
            serial(1),
            at,
            Graphic(PLAYER),
            Facing::walking(Direction::South),
            Hue::NONE,
            false,
            false,
        );
        let turned = crowd.see(
            serial(1),
            at,
            Graphic(PLAYER),
            Facing::walking(Direction::North),
            Hue::NONE,
            false,
            false,
        );
        assert_eq!(turned.drawn, Gaze::on(at), "a turn moves nobody");
    }

    /// Whoever the view no longer holds is forgotten, or the map grows for as
    /// long as the client is connected.
    #[test]
    fn a_mobile_the_view_dropped_is_forgotten() {
        let mut crowd = Crowd::default();
        for who in 1..=3 {
            crowd.see(
                serial(who),
                Point::new(10, 10, 0),
                Graphic(PLAYER),
                Facing::walking(Direction::South),
                Hue::NONE,
                false,
                false,
            );
        }
        crowd.advance(Duration::from_millis(80));
        crowd.retain(|who| who == serial(2));
        assert_eq!(crowd.tracked.len(), 1);
        // And the one that came back is new: its clock starts again rather than
        // resuming a walk nobody watched.
        assert_eq!(crowd.frame_for(serial(1), AnimationFrameCount(6)), 0);
        assert_eq!(crowd.frame_for(serial(2), AnimationFrameCount(6)), 1);
    }

    #[test]
    fn an_item_corpse_holds_the_last_death_frame() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let skeleton = Graphic(0x0038);
        let equipment: std::rc::Rc<[EquipmentLayer]> = vec![EquipmentLayer {
            graphic: openshard_tiles::AnimId(0x01),
            hue:     Hue(0x0455),
            layer:   openshard_protocol::wire::Layer::TORSO,
        }]
        .into();
        let corpse = crowd.corpse(
            serial(0x4000_0001),
            at,
            skeleton,
            Direction::SouthEast,
            Hue::NONE,
            equipment.clone(),
        );

        assert_eq!(corpse.group, BodyKind::Monster.dying());
        assert_eq!(corpse.at, at);
        assert_eq!(corpse.equipment, equipment, "the corpse keeps its worn layers");
        crowd.advance(Duration::from_secs(10));
        assert_eq!(
            crowd.group_for(serial(0x4000_0001)),
            Some(BodyKind::Monster.dying()),
            "a corpse never falls back to a standing animation"
        );
        assert_eq!(
            crowd.frame_for(serial(0x4000_0001), AnimationFrameCount(4)),
            3,
            "the final frame is the corpse pose"
        );
    }

    #[test]
    fn a_corpse_finishes_the_mobs_death_animation_before_holding_its_pose() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let skeleton = Graphic(0x0038);
        let mob = serial(1);
        let corpse = serial(0x4000_0001);
        crowd.see(
            mob,
            at,
            skeleton,
            Facing::walking(Direction::SouthEast),
            Hue::NONE,
            false,
            false,
        );
        crowd.play_new(NewAnimation {
            serial:         mob.expect("a real mobile serial"),
            animation_type: 3,
            action:         0,
            delay:          80,
        });
        // What the shard says next: this body becomes that corpse.
        crowd.died(
            mob.expect("a real mobile serial"),
            Some(corpse.expect("a real corpse serial")),
        );

        let falling = crowd.corpse(
            corpse,
            at,
            skeleton,
            Direction::SouthEast,
            Hue::NONE,
            Vec::new().into(),
        );
        assert_eq!(falling.group, BodyKind::Monster.dying());
        assert_eq!(
            crowd.frame_for(corpse, AnimationFrameCount(4)),
            0,
            "the corpse starts at the first death frame rather than appearing already prone"
        );
        crowd.advance(Duration::from_millis(80));
        assert_eq!(
            crowd.frame_for(corpse, AnimationFrameCount(4)),
            1,
            "it advances through the death frames"
        );
        crowd.advance(Duration::from_millis(240));
        assert_eq!(
            crowd.frame_for(corpse, AnimationFrameCount(4)),
            3,
            "once finished it remains at the final corpse pose"
        );
    }

    /// Server actions are latest-wins while a body lives, but a fall is
    /// terminal. A delayed hit packet must not make a dying cow stand up and
    /// attack for one frame before its corpse takes over.
    #[test]
    fn a_late_ordinary_action_cannot_interrupt_a_death() {
        let mut crowd = Crowd::default();
        let cow = Graphic(0x00D8);
        let mob = serial(1);
        crowd.see(
            mob,
            Point::new(10, 10, 0),
            cow,
            Facing::walking(Direction::SouthEast),
            Hue::NONE,
            false,
            false,
        );
        crowd.play_new(NewAnimation {
            serial:         mob.expect("a real mobile serial"),
            animation_type: 3,
            action:         0,
            delay:          80,
        });
        crowd.advance(Duration::from_millis(80));
        crowd.play(Animation {
            serial:       mob.expect("a real mobile serial"),
            action:       5, // LowAnimationGroup.Attack1
            frame_count:  AnimationFrameCount(4),
            repeat_count: 1,
            forward:      true,
            repeat:       false,
            delay:        80,
        });

        assert_eq!(crowd.group_for(mob), Some(BodyKind::Animal.dying()));
        assert_eq!(crowd.frame_for(mob, AnimationFrameCount(4)), 1);
    }

    /// `0xAF` has enough information to start a fall on its own.  The action
    /// packet normally arrives first, but making the visible death depend on
    /// that ordering made a creature that was removed in the same update turn
    /// straight into its corpse.  In particular this pins the distinct death
    /// groups for high, low, and people animation bodies.
    #[test]
    fn a_death_packet_starts_the_fall_for_every_body_animation_table() {
        let at = Point::new(10, 10, 0);
        for (number, body, kind, frames) in [
            (1, Graphic(0x0038), BodyKind::Monster, 4),
            (2, Graphic(0x00D8), BodyKind::Animal, 4), // cow
            (3, Graphic(0x0190), BodyKind::Human, 6),
        ] {
            let mut crowd = Crowd::default();
            let mob = serial(number);
            let corpse = serial(0x4000_0000 + number);
            crowd.see(
                mob,
                at,
                body,
                Facing::walking(Direction::SouthEast),
                Hue::NONE,
                false,
                false,
            );
            // No preceding 0x6E/0xE2: this is the packet ordering that used
            // to skip the animation entirely.
            crowd.died(
                mob.expect("a real mobile serial"),
                Some(corpse.expect("a real corpse serial")),
            );
            let falling = crowd.corpse(
                corpse,
                at,
                body,
                Direction::SouthEast,
                Hue::NONE,
                Vec::new().into(),
            );
            assert_eq!(falling.group, kind.dying(), "body {:#06x}", body.0);
            assert_eq!(crowd.frame_for(corpse, AnimationFrameCount(frames)), 0);
            crowd.advance(Duration::from_millis(80));
            assert_eq!(
                crowd.frame_for(corpse, AnimationFrameCount(frames)),
                1,
                "body {:#06x} advances its death animation",
                body.0
            );
        }
    }

    /// Two of the same creature dying on one tile keep their own falls.
    ///
    /// The hand-off used to be a search of the crowd for *a* body of the right
    /// graphic, on the right tile, playing the right group — which is every one
    /// of a pair that died together. One corpse claimed the other's fall, and
    /// with the two falls a step apart in their cadence the swap showed as one
    /// corpse jumping to a frame it had not reached. `0xAF` names the pair, so
    /// there is nothing left to search.
    #[test]
    fn two_bodies_falling_on_one_tile_keep_their_own_deaths() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let skeleton = Graphic(0x0038);
        let (first, second) = (serial(1), serial(2));
        let (first_corpse, second_corpse) = (serial(0x4000_0001), serial(0x4000_0002));
        for (who, facing) in [(first, Direction::West), (second, Direction::East)] {
            crowd.see(
                who,
                at,
                skeleton,
                Facing::walking(facing),
                Hue::NONE,
                false,
                false,
            );
            crowd.play_new(NewAnimation {
                serial:         who.expect("a real mobile serial"),
                animation_type: 3,
                action:         0,
                delay:          80,
            });
        }
        // The first fell a frame before the second, and each was named with the
        // corpse it becomes.
        crowd.advance(Duration::from_millis(80));
        crowd.died(
            first.expect("a real mobile serial"),
            Some(first_corpse.expect("a real corpse serial")),
        );
        crowd.died(
            second.expect("a real mobile serial"),
            Some(second_corpse.expect("a real corpse serial")),
        );

        let one = crowd.corpse(
            first_corpse,
            at,
            skeleton,
            Direction::West,
            Hue::NONE,
            Vec::new().into(),
        );
        let two = crowd.corpse(
            second_corpse,
            at,
            skeleton,
            Direction::East,
            Hue::NONE,
            Vec::new().into(),
        );
        assert_eq!(one.facing, Direction::West);
        assert_eq!(
            two.facing,
            Direction::East,
            "the second did not inherit the first's fall"
        );
        // Both are still finishing their own animation rather than one of them
        // being handed a fall that was already spoken for.
        assert_eq!(crowd.frame_for(first_corpse, AnimationFrameCount(4)), 1);
        assert_eq!(crowd.frame_for(second_corpse, AnimationFrameCount(4)), 1);
    }

    /// A corpse the shard never paired is drawn prone, not frozen mid-fall.
    ///
    /// The ordinary case for one that was already lying there when this client
    /// came into range: there is no fall to run, and inventing one out of
    /// whatever body happens to be standing on the tile is what the tile
    /// hand-off did.
    #[test]
    fn an_unannounced_corpse_is_drawn_in_its_final_pose() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let skeleton = Graphic(0x0038);
        let mob = serial(1);
        crowd.see(
            mob,
            at,
            skeleton,
            Facing::walking(Direction::West),
            Hue::NONE,
            false,
            false,
        );
        crowd.play_new(NewAnimation {
            serial:         mob.expect("a real mobile serial"),
            animation_type: 3,
            action:         0,
            delay:          80,
        });

        let corpse = serial(0x4000_0001);
        crowd.corpse(
            corpse,
            at,
            skeleton,
            Direction::West,
            Hue::NONE,
            Vec::new().into(),
        );
        assert_eq!(
            crowd.frame_for(corpse, AnimationFrameCount(4)),
            3,
            "no pairing, no fall to finish"
        );
    }

    /// A pairing whose corpse never arrives is forgotten rather than held for
    /// ever — see [`FALL_HELD`].
    #[test]
    fn a_death_whose_corpse_never_comes_is_let_go() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let skeleton = Graphic(0x0038);
        let mob = serial(1);
        let corpse = serial(0x4000_0001);
        crowd.see(
            mob,
            at,
            skeleton,
            Facing::walking(Direction::West),
            Hue::NONE,
            false,
            false,
        );
        crowd.play_new(NewAnimation {
            serial:         mob.expect("a real mobile serial"),
            animation_type: 3,
            action:         0,
            delay:          80,
        });
        crowd.died(
            mob.expect("a real mobile serial"),
            Some(corpse.expect("a real corpse serial")),
        );
        assert_eq!(crowd.falls.len(), 1);

        crowd.advance(FALL_HELD + Duration::from_millis(1));
        assert!(
            crowd.falls.is_empty(),
            "the promise expired with nothing to claim it"
        );
    }

    /// The body falls one way and lies another — the defect this facing was
    /// added for.
    ///
    /// The death animation played in the direction the mobile was last seen
    /// facing, and it looked right, because a settling corpse inherits the dying
    /// body's tracked facing. The corpse then went on being mentioned by every
    /// later fold of the world, and each of those overwrote that facing with a
    /// fixed southeast — so the body spun on the ground a moment after it
    /// stopped moving. Both halves are asserted here: the fall, and the second
    /// telling.
    #[test]
    fn a_corpse_keeps_lying_the_way_it_fell() {
        let mut crowd = Crowd::default();
        let at = Point::new(10, 10, 0);
        let skeleton = Graphic(0x0038);
        let mob = serial(1);
        let corpse = serial(0x4000_0001);
        crowd.see(
            mob,
            at,
            skeleton,
            Facing::walking(Direction::West),
            Hue::NONE,
            false,
            false,
        );
        crowd.play_new(NewAnimation {
            serial:         mob.expect("a real mobile serial"),
            animation_type: 3,
            action:         0,
            delay:          80,
        });
        crowd.died(
            mob.expect("a real mobile serial"),
            Some(corpse.expect("a real corpse serial")),
        );

        let falling = crowd.corpse(
            corpse,
            at,
            skeleton,
            Direction::West,
            Hue::NONE,
            Vec::new().into(),
        );
        assert_eq!(falling.facing, Direction::West, "it falls the way it faced");
        crowd.advance(Duration::from_millis(400));

        let settled = crowd.corpse(
            corpse,
            at,
            skeleton,
            Direction::West,
            Hue::NONE,
            Vec::new().into(),
        );
        assert_eq!(
            settled.facing,
            Direction::West,
            "and the next telling of the same corpse does not turn it"
        );
    }

    /// What `who` is saying, as plain lines, oldest first.
    fn said(crowd: &Crowd, who: Who) -> Vec<&str> {
        crowd.speaking(who).map(|(text, ..)| text).collect()
    }

    /// A line is there the instant it is heard, and gone once the hold runs
    /// out.
    #[test]
    fn a_line_is_spoken_and_then_expires() {
        let mut crowd = Crowd::default();
        crowd.hear(serial(1), "hello".to_string(), Font(0), Hue::NONE);
        assert_eq!(said(&crowd, serial(1)), ["hello"]);
        crowd.advance(SPEECH_HOLD - Duration::from_millis(1));
        assert_eq!(said(&crowd, serial(1)), ["hello"], "not yet");
        crowd.advance(Duration::from_millis(2));
        assert!(said(&crowd, serial(1)).is_empty(), "and now it has");
    }

    /// A second line stacks above the first rather than replacing it, and each
    /// keeps its own clock.
    ///
    /// This is what a single click needs: the shard sends the guild line and
    /// then the name as two `0x1C`s for one mobile, and a map holding one line
    /// per speaker showed only the name. Two lines in a row from one NPC had
    /// always been losing the first — the click just made it every time.
    #[test]
    fn lines_stack_and_each_keeps_its_own_clock() {
        let mut crowd = Crowd::default();
        crowd.hear(serial(1), "[OSS]".to_string(), Font(0), Hue::NONE);
        crowd.hear(serial(1), "Wilbur".to_string(), Font(0), Hue::NONE);
        assert_eq!(said(&crowd, serial(1)), ["[OSS]", "Wilbur"]);

        // The first was heard a moment earlier, so it goes first — and the
        // second is still there on its own clock.
        crowd.advance(SPEECH_HOLD);
        assert!(said(&crowd, serial(1)).is_empty());
    }

    /// The stack is bounded: a shard can talk faster than the hold retires
    /// lines, and the oldest is what gives way.
    #[test]
    fn a_talkative_mobile_keeps_only_its_most_recent_lines() {
        let mut crowd = Crowd::default();
        for line in 0..SPEECH_STACK + 3 {
            crowd.hear(serial(1), line.to_string(), Font(0), Hue::NONE);
        }
        let said = said(&crowd, serial(1));
        assert_eq!(said.len(), SPEECH_STACK);
        assert_eq!(said[0], "3", "the oldest lines were not the ones dropped");
        assert_eq!(said[SPEECH_STACK - 1], (SPEECH_STACK + 2).to_string());
    }

    /// Nobody not yet heard from is saying anything.
    #[test]
    fn a_serial_never_heard_is_not_speaking() {
        let crowd = Crowd::default();
        assert!(said(&crowd, serial(1)).is_empty());
    }

    /// `retain` forgets a stale line along with the rest of what a departed
    /// mobile was doing.
    #[test]
    fn retain_forgets_speech_along_with_position() {
        let mut crowd = Crowd::default();
        crowd.hear(serial(1), "bye".to_string(), Font(0), Hue::NONE);
        crowd.retain(|who| who != serial(1));
        assert!(said(&crowd, serial(1)).is_empty());
    }
}
