//! The most recent measured walk acknowledgement, split at the app mailbox.
//!
//! A walk request carries a sequence byte that its `WalkAck` echoes. That makes
//! the handshake an accurate RTT probe without sending a diagnostic packet or
//! involving a server clock.  The network worker stamps the acknowledgement
//! before it enters the application mailbox; otherwise a blocked renderer
//! looks indistinguishable from a slow shard.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use openshard_protocol::world::StepSequence;

#[derive(Default)]
pub(crate) struct Ping {
    sent: BTreeMap<StepSequence, Instant>,
    latest: Option<Sample>,
}

#[derive(Clone, Copy)]
struct Sample {
    transport: Duration,
    app_delivery: Duration,
}

impl Ping {
    pub(crate) fn sent(&mut self, sequence: StepSequence, now: Instant) {
        self.sent.insert(sequence, now);
    }

    pub(crate) fn acknowledged(&mut self, sequence: StepSequence, received: Instant, applied: Instant) {
        if let Some(sent) = self.sent.remove(&sequence) {
            self.latest = Some(Sample {
                transport: received.saturating_duration_since(sent),
                app_delivery: applied.saturating_duration_since(received),
            });
        }
    }

    /// A rejection or relocation invalidates every prediction that was still
    /// awaiting an answer, so none of their later packets can be an RTT sample.
    pub(crate) fn discard_pending(&mut self) {
        self.sent.clear();
    }

    pub(crate) const fn latest(&self) -> Option<Duration> {
        match self.latest {
            Some(sample) => Some(sample.transport),
            None => None,
        }
    }

    /// Time the decoded acknowledgement sat before the event loop handled it.
    pub(crate) const fn latest_app_delivery(&self) -> Option<Duration> {
        match self.latest {
            Some(sample) => Some(sample.app_delivery),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_measures_the_matching_walk_request() {
        let started = Instant::now();
        let mut ping = Ping::default();
        ping.sent(StepSequence(7), started);
        ping.sent(StepSequence(8), started + Duration::from_millis(3));

        ping.acknowledged(
            StepSequence(7),
            started + Duration::from_millis(17),
            started + Duration::from_millis(19),
        );

        assert_eq!(ping.latest(), Some(Duration::from_millis(17)));
        assert_eq!(ping.latest_app_delivery(), Some(Duration::from_millis(2)));
    }

    #[test]
    fn correction_discards_unanswered_requests() {
        let started = Instant::now();
        let mut ping = Ping::default();
        ping.sent(StepSequence(7), started);
        ping.discard_pending();
        ping.acknowledged(
            StepSequence(7),
            started + Duration::from_millis(17),
            started + Duration::from_millis(19),
        );

        assert_eq!(ping.latest(), None);
    }
}
