//! The hover's tooltip: when to ask for one, and what its lines say.
//!
//! The client half of the AoS property protocol above the wire.
//! [`openshard_client_net::properties`] writes the packet and
//! [`WorldView::tooltips`](openshard_client_net::view::WorldView::tooltips)
//! holds what came back; this decides when a request is worth sending and turns
//! a held list into text.
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

use std::collections::HashSet;

use openshard_client_net::view::Tooltip;
use openshard_protocol::properties::PropertyEntry;
use openshard_protocol::serial::Serial;
use openshard_uofiles::cliloc::{
    Cliloc,
    ClilocNumber,
};

use crate::net_command::resolve_cliloc_arguments;

/// What this client has already asked the shard about.
///
/// Not a cache of tooltips — that is the view's, filled by the packets. This is
/// only the record of outstanding questions, which is state the wire has no
/// packet for and therefore nowhere else to live.
#[derive(Default)]
pub struct Tooltips {
    /// One entry per question asked: the object, and the revision that was
    /// current when it was asked. See the module docs for why the revision is
    /// part of the key.
    asked: HashSet<(Serial, Option<u32>)>,
}

impl Tooltips {
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
        let mut tooltips = Tooltips::default();
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
        let mut tooltips = Tooltips::default();
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
        let mut tooltips = Tooltips::default();
        assert!(!tooltips.should_ask(serial(), None));
    }

    #[test]
    fn a_reset_lets_the_question_be_asked_again() {
        let mut tooltips = Tooltips::default();
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
}
