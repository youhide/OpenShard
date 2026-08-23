//! The statics that move on their own, and the clock they move on.
//!
//! A fire, a torch, a water wheel. What they cycle through is `animdata.mul` —
//! [`openshard_uofiles::animdata`] — and what this adds is *when*: a table
//! keyed by graphic, a [`Duration`] of its own, and one question,
//! [`StaticAnimations::showing`], that turns a graphic into the graphic on
//! screen at this instant.
//!
//! # Why this is a system and not a field on a quad
//!
//! It is the same seam D10 draws for a body: a sprite is drawn at a position and
//! at a graphic that were *decided* somewhere else, and the collectors below
//! ([`crate::statics`], [`crate::items`]) place what they are handed. Put the
//! clock in the collector instead and there are two of them — the map's statics
//! and the server's items are the same art through the same atlas — and two
//! clocks on one subject drift, which on screen is two fires side by side out of
//! step for no reason in the world.
//!
//! # This reads no clock, and that is what makes it testable
//!
//! It is handed `dt`, exactly as [`crate::follow::Follower`] is. `client/app`
//! samples time once a frame and every clock in the client is advanced from that
//! one sample — so an animation cannot be a frame ahead of the camera, and a test
//! can hand it four hundred milliseconds without owning a window.
//!
//! # What it costs the atlas, which is the part worth stating
//!
//! An animated static *is* several statics: the offsets are added to the graphic
//! id, so a fire is four ordinary art entries shown in turn. So the atlas has to
//! be offered the whole cycle and not the frame on screen — see
//! [`StaticAnimations::cycle`]. Offering only the current graphic works, packs
//! nothing extra, and grows the atlas every time a fire ticks over: a periodic
//! hitch manufactured by the animation system, arriving on whichever frame the
//! cycle happened to turn.

use std::time::Duration;

use rustc_hash::FxHashMap;

use openshard_protocol::wire::Graphic;
use openshard_tiles::TileData;
use openshard_uofiles::animdata::{AnimData, Sequence};

/// How long one step of an animated static's cycle lasts, at an interval of one.
///
/// `AnimatedStaticsManager.Process` schedules the next step at
/// `Time.Ticks + info.FrameInterval * delay + 1`, where `delay` is
/// `Constants.ITEM_EFFECT_ANIMATION_DELAY * 2` — 50ms doubled, under a comment
/// saying it is doubled to match the standard client. So a step is a multiple of
/// this, and an interval of zero is one of them rather than none: that branch
/// schedules `Time.Ticks + delay`.
///
/// The reference's trailing `+ 1` is dropped deliberately. It is an artefact of
/// scheduling against a polling loop — a millisecond of guard so a step cannot
/// be due at the instant it was scheduled — and carrying it would make a step
/// 1/100th long and, worse, make the phase a function of how many steps have
/// been taken rather than of the time. A cycle that is a pure function of elapsed
/// time is what lets [`StaticAnimations::showing`] be a lookup instead of a
/// state machine, and it is what makes two clients showing the same second show
/// the same fire.
pub const FRAME_STEP: Duration = Duration::from_millis(100);

/// Every static that animates, and how far into its cycle they all are.
///
/// One clock for the lot, and that is the file's own model rather than a saving:
/// `animdata.mul` is keyed by graphic and not by position, so every fire of a
/// given graphic burns in step everywhere on the map. Fires that look out of step
/// are *different graphics* whose offsets are rotations of one another — see
/// [`openshard_uofiles::animdata::Sequence`].
#[derive(Clone, Debug, Default)]
pub struct StaticAnimations {
    /// Only the graphics that both carry the flag and have a cycle to play. A
    /// lookup that misses is a static that does not animate, which is almost all
    /// of them.
    cycles: FxHashMap<u16, Sequence>,
    /// Real time since this was built. Its own clock rather than an `Instant`,
    /// for the reason [`crate::follow`] gives: a rule that reads the wall cannot
    /// be handed a cadence by a test.
    elapsed: Duration,
}

impl StaticAnimations {
    /// The animated statics of one client install.
    ///
    /// Both files, because it takes both to answer the question: `tiledata.mul`
    /// says which graphics animate and `animdata.mul` says what they animate to.
    /// A graphic flagged with nothing to play is dropped here rather than checked
    /// on every lookup — the reference keeps it and draws the base graphic, which
    /// is the same picture through a longer path.
    pub fn build(animdata: &AnimData, tiledata: &TileData) -> Self {
        let mut cycles = FxHashMap::default();
        for graphic in 0..=u16::MAX {
            if !tiledata.static_tile(graphic).flags.is_animated() {
                continue;
            }
            if let Some(sequence) = animdata.sequence(Graphic(graphic)) {
                cycles.insert(graphic, sequence);
            }
        }
        Self {
            cycles,
            elapsed: Duration::ZERO,
        }
    }

    /// How many graphics animate at all.
    pub fn len(&self) -> usize {
        self.cycles.len()
    }

    /// Whether nothing does — a client with no `animdata.mul`, or a test's empty
    /// tiledata.
    pub fn is_empty(&self) -> bool {
        self.cycles.is_empty()
    }

    /// Whether this base graphic changes with the presentation clock.
    ///
    /// Immutable map-block composites use this to keep a whole block on the
    /// detailed path when it contains animated art; baking a fire into a cache
    /// entry would make its next animation step permanently stale.
    pub fn is_animated(&self, graphic: Graphic) -> bool {
        self.cycles.contains_key(&graphic.0)
    }

    /// Move the clock forward. The one writer, and it takes the frame's span.
    pub fn advance(&mut self, dt: Duration) {
        self.elapsed += dt;
    }

    /// Real time this animation set has been playing.
    ///
    /// This is primarily useful to keep higher-level presentation-clock tests
    /// independent of a particular tile's animation sequence.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// The shared animation interval currently being shown.
    ///
    /// Every static cycle changes only on a multiple of [`FRAME_STEP`]. A
    /// caller caching a fully collected static scene may therefore retain it
    /// within one tick, but must invalidate it at this boundary even if the
    /// camera did not move.
    pub fn tick(&self) -> u128 {
        self.elapsed.as_millis() / FRAME_STEP.as_millis()
    }

    /// What is on screen for `graphic` at this instant.
    ///
    /// The graphic itself when it does not animate, which is the answer for all
    /// but a thousand of them. Total, and a lookup rather than a state machine:
    /// the frame index is `elapsed / step % count`, so nothing here depends on
    /// how many frames have been drawn or on the order they were asked in.
    ///
    /// The arithmetic is `u16`-wrapping by construction rather than by luck — the
    /// offsets are signed and a cycle may start below its own base — and every
    /// graphic a real `animdata.mul` reaches is checked to be in range by
    /// `animdata`'s own test against the file.
    pub fn showing(&self, graphic: Graphic) -> Graphic {
        let Some(sequence) = self.cycles.get(&graphic.0) else {
            return graphic;
        };
        let step = FRAME_STEP * u32::from(sequence.interval().max(1));
        // `as_millis` on both sides: the step is a whole number of milliseconds
        // and so is any clock a frame advances by, so this is exact division
        // rather than a float that could land a hair either side of a boundary
        // and show two different frames to two readers of the same instant.
        let index = (self.elapsed.as_millis() / step.as_millis()) % u128::from(sequence.count().get());
        let offset = sequence
            .offsets()
            .nth(index as usize)
            .expect("the index is taken modulo the count");
        Graphic(graphic.0.wrapping_add_signed(i16::from(offset)))
    }

    /// Every graphic `graphic` will ever show, itself included.
    ///
    /// What the atlas is grown for. Asking for the whole cycle up front is the
    /// difference between a fire that ticks over for free and one that grows the
    /// atlas — and uploads a band of rows to the GPU — every time it does. See
    /// this module's header.
    ///
    /// One element for a static that does not animate, so a caller does not need
    /// to know which kind it has.
    pub fn cycle(&self, graphic: Graphic) -> impl Iterator<Item = Graphic> {
        let sequence = self.cycles.get(&graphic.0).copied();
        std::iter::once(graphic).chain(
            sequence
                .into_iter()
                .flat_map(move |sequence| sequence.offsets())
                .map(move |offset| Graphic(graphic.0.wrapping_add_signed(i16::from(offset)))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table built by hand, so the clock can be tested without a client.
    ///
    /// `StaticAnimations::build` needs two real files; what is being tested here
    /// is the arithmetic on top of them, and a fixture is honest for that in a
    /// way it would not be for the parse.
    fn fixture(cycles: &[(u16, Sequence)]) -> StaticAnimations {
        StaticAnimations {
            cycles: cycles.iter().copied().collect(),
            elapsed: Duration::ZERO,
        }
    }

    /// A sequence with the given offsets and interval, built through the parser
    /// rather than by constructing the type — its fields are private, and that is
    /// deliberate: a `Sequence` this crate assembled would be one whose shape
    /// this crate believes in.
    fn sequence(offsets: &[i8], interval: u8) -> Sequence {
        let mut bytes = vec![0xAA; 4];
        let mut entry = [0u8; 68];
        for (slot, offset) in entry.iter_mut().zip(offsets) {
            *slot = *offset as u8;
        }
        entry[65] = offsets.len() as u8;
        entry[66] = interval;
        bytes.extend_from_slice(&entry);
        AnimData::parse(&bytes)
            .sequence(Graphic(0))
            .expect("a whole entry")
    }

    /// The whole of what the clock does: one step per interval, wrapping at the
    /// end, and the same instant always giving the same frame.
    #[test]
    fn a_cycle_steps_once_per_interval_and_wraps() {
        let fire = Graphic(0x006D);
        let mut animations = fixture(&[(fire.0, sequence(&[-3, -2, -1, 0], 1))]);
        for (at, expected) in [(0, -3), (99, -3), (100, -2), (250, -1), (399, 0), (400, -3)] {
            let mut clock = fixture(&[(fire.0, sequence(&[-3, -2, -1, 0], 1))]);
            clock.advance(Duration::from_millis(at));
            assert_eq!(
                clock.showing(fire),
                Graphic(fire.0.wrapping_add_signed(expected)),
                "at {at}ms",
            );
        }
        // And the same clock advanced in pieces lands where one jump does: the
        // frame is a function of the total and not of how it arrived.
        for _ in 0..25 {
            animations.advance(Duration::from_millis(16));
        }
        assert_eq!(animations.showing(fire), Graphic(fire.0 - 3), "400ms in pieces");
    }

    /// An interval of zero is an interval of one — the branch
    /// `AnimatedStaticsManager.Process` writes as `Time.Ticks + delay`. Read the
    /// other way it is a division by nothing, which is the shape of defect this
    /// codebase keeps finding in a `tau` of zero.
    #[test]
    fn an_interval_of_zero_is_one_step_and_not_a_division_by_nothing() {
        let mut animations = fixture(&[(9, sequence(&[0, 1], 0))]);
        assert_eq!(animations.showing(Graphic(9)), Graphic(9));
        animations.advance(FRAME_STEP);
        assert_eq!(animations.showing(Graphic(9)), Graphic(10));
    }

    /// A slower cycle is slower by its interval and by nothing else.
    #[test]
    fn the_interval_multiplies_the_step() {
        let mut animations = fixture(&[(9, sequence(&[0, 1], 6))]);
        animations.advance(FRAME_STEP * 5);
        assert_eq!(animations.showing(Graphic(9)), Graphic(9), "five of six steps");
        animations.advance(FRAME_STEP);
        assert_eq!(animations.showing(Graphic(9)), Graphic(10));
    }

    /// A static that does not animate is its own graphic, whatever the clock
    /// says — the answer for all but a thousand graphics, and the one this is
    /// asked for most.
    #[test]
    fn a_static_that_does_not_animate_is_itself() {
        let mut animations = fixture(&[(9, sequence(&[1], 1))]);
        animations.advance(Duration::from_secs(37));
        assert!(animations.is_animated(Graphic(9)));
        assert!(!animations.is_animated(Graphic(0x0FAC)));
        assert_eq!(animations.showing(Graphic(0x0FAC)), Graphic(0x0FAC));
        assert_eq!(
            animations.cycle(Graphic(0x0FAC)).collect::<Vec<_>>(),
            [Graphic(0x0FAC)],
            "and it wants exactly itself packed",
        );
    }

    /// The atlas is offered every graphic the cycle can reach, including the ones
    /// below the base — which is what an offset of `-3` means and what a cycle
    /// asked for as `base..base+count` would miss.
    ///
    /// The property that matters is containment rather than the exact list: what
    /// breaks the atlas is a graphic that gets *shown* and was never *offered*.
    #[test]
    fn the_cycle_offered_to_the_atlas_contains_every_graphic_that_will_be_shown() {
        let fire = Graphic(0x006D);
        let offsets = [-3i8, -2, -1, 0, 1, 2];
        let offered: Vec<Graphic> = fixture(&[(fire.0, sequence(&offsets, 3))]).cycle(fire).collect();
        assert!(offered.contains(&fire), "the base graphic is always packed");

        // Every frame of a full cycle, walked on the clock rather than derived
        // from the offsets a second time: the two would agree by construction if
        // this test read `offsets` again, and the thing worth catching is
        // `showing` and `cycle` disagreeing.
        let mut animations = fixture(&[(fire.0, sequence(&offsets, 3))]);
        for step in 0..offsets.len() * 2 {
            let shown = animations.showing(fire);
            assert!(offered.contains(&shown), "step {step} shows {shown:?}, unpacked");
            animations.advance(FRAME_STEP * 3);
        }
    }

    /// The reference's step, pinned beside the constant: 50ms doubled, because a
    /// number taken from a reference is worth nothing once it has quietly
    /// drifted.
    #[test]
    fn a_step_is_the_references_item_delay_doubled() {
        assert_eq!(FRAME_STEP, Duration::from_millis(50) * 2);
    }

    /// The real client's table, and the one thing a fixture cannot check: that
    /// building from two real files finds animated statics at all, and that every
    /// graphic any of them will show is one the atlas was offered.
    #[test]
    fn a_real_install_animates_and_offers_every_frame_it_shows() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let animdata = AnimData::load(&dir).expect("animdata.mul");
        let tiledata = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))
            .expect("tiledata.mul")
            .tiles;
        let mut animations = StaticAnimations::build(&animdata, &tiledata);
        assert!(animations.len() > 1_000, "only {} animate", animations.len());

        let graphics: Vec<Graphic> = animations.cycles.keys().copied().map(Graphic).collect();
        let offered: Vec<Vec<Graphic>> = graphics
            .iter()
            .map(|graphic| animations.cycle(*graphic).collect())
            .collect();
        // Ten seconds of clock, which is longer than the slowest cycle in the
        // file: every one of them has come all the way round inside it.
        for _ in 0..100 {
            animations.advance(FRAME_STEP);
            for (graphic, offered) in graphics.iter().zip(&offered) {
                let shown = animations.showing(*graphic);
                assert!(
                    offered.contains(&shown),
                    "0x{:04X} shows 0x{:04X}, which the atlas was never offered",
                    graphic.0,
                    shown.0,
                );
            }
        }
    }
}
