//! The hover's property card: when to ask for one, when it opens, and what its
//! lines say.
//!
//! The client half of the AoS property protocol above the wire.
//! [`openshard_client_net::properties`] writes the packet and
//! [`WorldView::tooltips`](openshard_client_net::view::WorldView::tooltips)
//! holds what came back; this decides when a request is worth sending, turns a
//! held list into text, and keeps the little state a card has between frames.
//! What that text *looks like* is
//! [`openshard_client_render::tooltip`](openshard_client_render::tooltip) —
//! measuring, wrapping and placement live there and read nothing of the world.
//!
//! # Why anything has to remember what was asked
//!
//! Because "stale" stays true until the answer lands, and a hover lasts many
//! frames. Asking straight off
//! [`Tooltip::stale`](openshard_client_net::view::Tooltip::stale) would put one
//! `0xD6` on the wire *per frame* for as long as the pointer sat still — sixty a
//! second for a round trip that takes one. So a request is remembered by what it
//! asked about: the serial **and the revision it was asked at**. A newer
//! revision is a different key and asks again, which is exactly right, and the
//! shard going quiet is silence rather than a flood.
//!
//! # Why anything has to remember what is showing
//!
//! For two reasons, and neither of them is the property list — that is the
//! view's. The first is the clock: a card opens from its title to the whole
//! list after the pointer has stayed on one object for [`DETAIL_AFTER`], and
//! "has stayed" is a fact about frames that no packet carries. The second is
//! the *placement*: a card that recomputed its anchor from the live pointer
//! every frame would flip between the four corners as the pointer wandered a
//! pixel near an edge. So the pointer is read once, when the card's content
//! first becomes drawable, and that reading is what the card is placed by until
//! its subject changes.

use std::collections::HashSet;
use std::time::{
    Duration,
    Instant,
};

use openshard_client_net::view::Tooltip;
use openshard_client_render::gump::GumpPixel;
use openshard_client_render::tooltip::Phase;
use openshard_protocol::properties::PropertyEntry;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::cliloc::{
    Cliloc,
    ClilocNumber,
};

use crate::net_command::resolve_cliloc_arguments;

/// How long the pointer has to stay on one object, after its property list is
/// drawable, before the card opens from a title to the whole list.
///
/// Measured from the moment there is something to show rather than from the
/// moment the pointer arrived: an object whose list took a round trip to arrive
/// would otherwise open the instant it appeared, having spent its own wait on
/// the network. A player who is scanning a bag sees titles; a player who has
/// stopped on one thing gets the stats.
pub const DETAIL_AFTER: Duration = Duration::from_millis(350);

/// What the card is about, and since when.
///
/// Only ever the *showing* object: the moment the pointer moves to another
/// serial this is replaced, which is what makes a late reply for the object the
/// pointer has left unable to open a card attributed to it.
#[derive(Clone, Copy)]
struct Showing {
    subject:  Serial,
    /// When this subject's list first became drawable — the clock
    /// [`DETAIL_AFTER`] is measured on.
    ready_at: Instant,
    /// Where the pointer was at that moment, which is what the card is placed
    /// by for as long as it stands. See the module docs.
    cursor:   GumpPixel,
}

/// What the card is showing this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shown {
    pub phase:  Phase,
    /// The pointer position the card's placement is frozen to.
    pub cursor: GumpPixel,
}

/// Everything the presentation layer needs to draw one object's card.
///
/// A typed value rather than the `Vec<String>` this used to be, and the
/// difference is what a bare list of lines cannot say: which object the lines
/// belong to (so a reply that arrives after the pointer has moved cannot be
/// drawn under the wrong thing), what art to put beside them, how much of the
/// list is open, and — the case a list of strings collapses outright — the
/// difference between an object that says nothing and no object at all. The
/// first is `Some` with no lines; the second is no presentation.
///
/// The lines are resolved here rather than carried as raw
/// [`PropertyEntry`]s, which is the one deviation from the plan's sketch and it
/// is a borrow rather than a choice: this value outlives the frame's borrow of
/// the view by several statements (see `App::draw_from`), so it owns what it
/// carries. Resolution stays in [`lines`] either way, which is the single place
/// that knows a missing cliloc table means no card.
pub struct Presentation {
    /// The object the card is about.
    pub serial:  Serial,
    /// Its drawn art, or `None` for a mobile and for anything with no picture
    /// worth an icon.
    pub graphic: Option<Graphic>,
    /// The property list as text, title first. Empty when the object said
    /// nothing, or when this client has no text table to say it with.
    pub lines:   Vec<String>,
    /// How much of that list is open.
    pub phase:   Phase,
    /// The pointer the card is placed by — see [`Showing::cursor`].
    pub cursor:  GumpPixel,
}

/// What this client has already asked the shard about, and what its card is
/// currently showing.
///
/// Not a cache of tooltips — that is the view's, filled by the packets. This is
/// the record of outstanding questions and of the card's own timing, both of
/// which are state the wire has no packet for and therefore nowhere else to
/// live.
pub struct Tooltips {
    /// One entry per question asked: the object, and the revision that was
    /// current when it was asked. See the module docs for why the revision is
    /// part of the key.
    asked:   HashSet<(Serial, Option<u32>)>,
    /// The card standing this frame, or `None` when nothing is showing.
    showing: Option<Showing>,
}

impl Tooltips {
    /// A client that has asked nothing and is showing nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            asked:   HashSet::new(),
            showing: None,
        }
    }

    /// Note this frame's subject and answer how much of its card to draw.
    ///
    /// `ready` is whether there is anything to draw *now* — a subject whose
    /// list has not arrived, or which resolved to nothing, has not started its
    /// clock and cannot open. `cursor` is the live pointer, read only on the
    /// frame a card begins; every later frame is answered with the reading that
    /// began it.
    ///
    /// `None` for no subject at all, which is also what closes a standing card:
    /// the pointer moving off an object, leaving the surface, or picking
    /// something up all arrive here as the same absence.
    pub fn track(
        &mut self,
        subject: Option<Serial>,
        ready: bool,
        cursor: GumpPixel,
        now: Instant,
    ) -> Option<Shown> {
        let Some(subject) = subject else {
            self.showing = None;
            return None;
        };
        if !ready {
            // A subject with nothing to show yet. The clock does not start, and
            // any card that was standing is closed — the pointer has moved to
            // an object this client cannot describe, and keeping the last one
            // up would attribute it to the wrong thing.
            self.showing = None;
            return Some(Shown {
                phase: Phase::Compact,
                cursor,
            });
        }
        let showing = match self.showing {
            Some(showing) if showing.subject == subject => showing,
            _ => {
                Showing {
                    subject,
                    ready_at: now,
                    cursor,
                }
            }
        };
        self.showing = Some(showing);
        Some(Shown {
            phase:  match now.duration_since(showing.ready_at) >= DETAIL_AFTER {
                true => Phase::Detail,
                false => Phase::Compact,
            },
            cursor: showing.cursor,
        })
    }

    /// Whether to put a request on the wire for `serial`, and remember that it
    /// went.
    ///
    /// `held` is the view's entry, `None` when the shard has never mentioned the
    /// object at all — which is answered `false`. The one thing that would make
    /// it `true` is `off` tooltip mode, where no request will ever be answered,
    /// and the one thing it would cost is a packet per hover forever.
    pub fn should_ask(&mut self, serial: Serial, held: Option<&Tooltip>) -> bool {
        let Some(held) = held else { return false };
        if !held.stale() {
            return false;
        }
        self.asked.insert((serial, held.revision))
    }

    /// Forget every outstanding question.
    ///
    /// For the two moments that make one meaningless: the connection ending, and
    /// a second `0x1B` restarting the session — after either, an answer would
    /// name a world this client no longer has.
    pub fn reset(&mut self) {
        self.asked.clear();
        // And whatever card was up: it names an object of the world that has
        // just ended, and the next frame's subject is a serial in a new one.
        self.showing = None;
    }
}

/// Turn a held property list into the lines to draw, first line first.
///
/// Empty when the list is empty *or* when no `Cliloc` is loaded: with no text
/// table there is nothing to say, and drawing the bare numbers would be a
/// tooltip that reads `1050045`. Note this is not the same as having no
/// tooltip — see [`Tooltips`]' own docs for what an absent entry means.
#[must_use]
pub fn lines(entries: &[PropertyEntry], cliloc: Option<&Cliloc>) -> Vec<String> {
    let Some(cliloc) = cliloc else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let template = cliloc.get(ClilocNumber::new(entry.cliloc.0))?;
            let text = resolve_cliloc_arguments(template, &entry.arguments);
            // An item's tiledata name arrives with no arguments and often with
            // padding around it; a mobile's arrives as three fields of which two
            // are usually blank. Either way the drawn line is what is left after
            // the spaces the format needed.
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use openshard_protocol::wire::ClilocId;

    use super::*;

    fn serial() -> Serial {
        Serial::new(0x4000_0001).unwrap()
    }

    /// The defect this type exists to prevent. Everything else here is detail.
    #[test]
    fn a_pointer_sitting_still_asks_once() {
        let mut tooltips = Tooltips::new();
        let held = Tooltip {
            revision:      Some(7),
            held_revision: None,
            entries:       Vec::new(),
        };
        assert!(tooltips.should_ask(serial(), Some(&held)));
        assert!(
            !tooltips.should_ask(serial(), Some(&held)),
            "still stale, still hovered, and the question is already out"
        );
    }

    #[test]
    fn a_new_revision_is_a_new_question() {
        let mut tooltips = Tooltips::new();
        let first = Tooltip {
            revision:      Some(7),
            held_revision: Some(7),
            entries:       Vec::new(),
        };
        assert!(!tooltips.should_ask(serial(), Some(&first)), "nothing to ask");

        let changed = Tooltip {
            revision:      Some(8),
            held_revision: Some(7),
            entries:       Vec::new(),
        };
        assert!(tooltips.should_ask(serial(), Some(&changed)));
    }

    #[test]
    fn an_object_the_shard_never_mentioned_is_not_asked_about() {
        let mut tooltips = Tooltips::new();
        assert!(!tooltips.should_ask(serial(), None));
    }

    #[test]
    fn a_reset_lets_the_question_be_asked_again() {
        let mut tooltips = Tooltips::new();
        let held = Tooltip {
            revision:      Some(7),
            held_revision: None,
            entries:       Vec::new(),
        };
        assert!(tooltips.should_ask(serial(), Some(&held)));
        tooltips.reset();
        assert!(tooltips.should_ask(serial(), Some(&held)));
    }

    #[test]
    fn with_no_text_table_there_are_no_lines() {
        let entries = vec![PropertyEntry {
            cliloc:    ClilocId(1_050_045),
            arguments: " \tLord British\t ".to_owned(),
        }];
        assert!(lines(&entries, None).is_empty(), "not the bare number");
    }

    fn other() -> Serial {
        Serial::new(0x4000_0002).unwrap()
    }

    fn at(x: i32, y: i32) -> GumpPixel {
        GumpPixel::new(x, y)
    }

    /// A pointer that passes over an object gets its title; a pointer that
    /// stops on one gets the rest. Both from one property list — the clock
    /// changes what is shown, not what was asked for.
    #[test]
    fn a_card_opens_after_the_pointer_has_stayed() {
        let mut tooltips = Tooltips::new();
        let start = Instant::now();
        assert_eq!(
            tooltips.track(Some(serial()), true, at(10, 10), start),
            Some(Shown {
                phase:  Phase::Compact,
                cursor: at(10, 10),
            })
        );
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(11, 10), start + DETAIL_AFTER / 2)
                .map(|shown| shown.phase),
            Some(Phase::Compact),
            "not yet"
        );
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(11, 10), start + DETAIL_AFTER)
                .map(|shown| shown.phase),
            Some(Phase::Detail)
        );
    }

    /// The clock starts when there is something to show, not when the pointer
    /// arrived: an object whose list took a round trip would otherwise open the
    /// instant it landed, having spent its wait on the network.
    #[test]
    fn the_clock_starts_when_the_list_arrives() {
        let mut tooltips = Tooltips::new();
        let start = Instant::now();
        for waited in [0, 1, 2] {
            assert_eq!(
                tooltips
                    .track(Some(serial()), false, at(10, 10), start + DETAIL_AFTER * waited)
                    .map(|shown| shown.phase),
                Some(Phase::Compact),
                "nothing to show, however long the pointer sits"
            );
        }
        let arrived = start + DETAIL_AFTER * 2;
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(10, 10), arrived)
                .map(|shown| shown.phase),
            Some(Phase::Compact),
            "the list has only just landed"
        );
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(10, 10), arrived + DETAIL_AFTER)
                .map(|shown| shown.phase),
            Some(Phase::Detail)
        );
    }

    /// The placement defect this state exists to prevent: a card that reread
    /// the live pointer every frame would flip between anchors as the pointer
    /// wandered a pixel near a screen edge.
    #[test]
    fn a_standing_card_keeps_the_pointer_it_was_placed_by() {
        let mut tooltips = Tooltips::new();
        let start = Instant::now();
        tooltips.track(Some(serial()), true, at(400, 300), start);
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(403, 299), start + DETAIL_AFTER)
                .map(|shown| shown.cursor),
            Some(at(400, 300)),
            "the same object, so the same placement"
        );
        assert_eq!(
            tooltips
                .track(Some(other()), true, at(403, 299), start + DETAIL_AFTER)
                .map(|shown| shown.cursor),
            Some(at(403, 299)),
            "a different object is a new card, placed where the pointer is now"
        );
    }

    /// Moving to another object restarts the clock rather than inheriting the
    /// last one's — which is what stops a card opening instantly all the way
    /// across a bag once the pointer has dwelt anywhere.
    #[test]
    fn another_object_starts_its_own_clock() {
        let mut tooltips = Tooltips::new();
        let start = Instant::now();
        tooltips.track(Some(serial()), true, at(10, 10), start);
        let opened = start + DETAIL_AFTER;
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(10, 10), opened)
                .map(|shown| shown.phase),
            Some(Phase::Detail)
        );
        assert_eq!(
            tooltips
                .track(Some(other()), true, at(20, 10), opened)
                .map(|shown| shown.phase),
            Some(Phase::Compact)
        );
    }

    /// Nothing under the pointer is no card, and it also forgets the one that
    /// was standing: a pointer that leaves and comes back gets a fresh clock,
    /// not the remains of the old one.
    #[test]
    fn no_subject_closes_the_card() {
        let mut tooltips = Tooltips::new();
        let start = Instant::now();
        tooltips.track(Some(serial()), true, at(10, 10), start);
        assert_eq!(tooltips.track(None, true, at(10, 10), start), None);
        assert_eq!(
            tooltips
                .track(Some(serial()), true, at(10, 10), start + DETAIL_AFTER)
                .map(|shown| shown.phase),
            Some(Phase::Compact),
            "back on the same object, and the clock starts again"
        );
    }
}
