//! Password checks, run where the shard's loop is not waiting for them.
//!
//! # Why a login leaves the loop at all
//!
//! One task owns the world and the network — see [`shard::run_shard`] — and that
//! is deliberate: the tick is single-threaded because determinism is the point.
//! It means anything that blocks that task blocks the shard, and argon2 blocks it
//! for tens of milliseconds: 19 MiB and two passes, on purpose, because that is
//! what makes a stolen credential file expensive to grind. Most of a 50 ms tick,
//! for one client's password, with every other player waiting.
//!
//! So the login state machine hands the comparison back as a value
//! ([`openshard_login::Outcome::Verify`]) instead of doing it, this module runs it
//! on a blocking task, and the verdict returns through a channel like everything
//! else the loop reacts to. The connection waits — it was going to wait either
//! way — and the shard does not.
//!
//! # What is deliberately *not* here
//!
//! Any decision. A verdict is yes-or-no about a credential and carries no
//! identity: who was being logged in stayed in the login session, which is the
//! only thing that will act on the answer. See `Credential::against`.

use openshard_login::{
    CredentialCheck,
    PasswordVerdict,
};

use super::*;

/// One password check's answer, on its way back to the connection that asked for
/// it.
#[derive(Debug)]
pub(crate) struct Verdict {
    /// Whose check this was. The verdict means nothing without it, and the login
    /// session refuses one that arrives at a connection which asked for none.
    pub(crate) connection: ConnectionId,
    pub(crate) verdict:    PasswordVerdict,
}

/// Sender half of the verdict channel, held by the tasks doing the hashing.
#[derive(Debug, Clone)]
struct VerdictTx(mpsc::UnboundedSender<Verdict>);

/// Receiver half of [`VerdictTx`], read by the shard loop's own `select!` arm.
#[derive(Debug)]
pub(crate) struct VerdictRx(mpsc::UnboundedReceiver<Verdict>);

impl VerdictRx {
    pub(crate) async fn recv(&mut self) -> Option<Verdict> {
        self.0.recv().await
    }
}

/// Hands password checks to blocking tasks and their answers back to the loop.
pub(crate) struct Verifier {
    done:    VerdictTx,
    /// How many hashes may run at once — see [`Verifier::with_permits`].
    permits: Arc<Semaphore>,
}

impl Verifier {
    /// A verifier bounded by this machine's parallelism, and the receiver the
    /// shard loop reads verdicts from.
    pub(crate) fn new() -> (Self, VerdictRx) {
        // `available_parallelism` fails only where the count cannot be
        // determined; one at a time is the safe reading of "unknown".
        let cores = std::thread::available_parallelism().map_or(1, |cores| cores.get());
        Self::with_permits(cores)
    }

    /// A verifier that will not run more than `permits` hashes at once.
    ///
    /// # This bound is memory, not throughput
    ///
    /// Every argon2 in flight holds 19 MiB for as long as it runs, and
    /// `spawn_blocking` is happy to start 512 of them — ten gigabytes, on a burst
    /// of logins from clients none of which has proved anything yet. That is a
    /// door this module would otherwise be opening: before it, the loop ran
    /// checks one at a time because it had no choice.
    ///
    /// One per core is as fast as hashing can go anyway; the rest queue on the
    /// permit, which costs a task and a few hundred bytes each.
    fn with_permits(permits: usize) -> (Self, VerdictRx) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                done:    VerdictTx(tx),
                permits: Arc::new(Semaphore::new(permits)),
            },
            VerdictRx(rx),
        )
    }

    /// Start one check. Returns immediately; the verdict arrives on the channel.
    ///
    /// A check that cannot be run comes back as [`PasswordVerdict::Rejected`]
    /// rather than not at all. A connection whose verdict never arrives is a
    /// client hung on "verifying account" forever — the login session is waiting
    /// for exactly one thing and will accept nothing else — so the failure has to
    /// be *an answer*, and the only safe answer about a password nobody checked is
    /// no.
    pub(crate) fn spawn(&self, connection: ConnectionId, check: CredentialCheck) {
        let done = self.done.clone();
        let permits = Arc::clone(&self.permits);
        tokio::spawn(async move {
            // Waiting here is the queueing, and it is where a burst of logins
            // piles up instead of in the blocking pool's memory.
            let Ok(_permit) = permits.acquire().await else {
                // The semaphore is closed: the shard is going away and there is
                // nobody left to tell.
                return;
            };
            let verdict = match tokio::task::spawn_blocking(move || check.run()).await {
                Ok(verdict) => verdict,
                Err(error) => {
                    error!(%connection, %error, "a password check did not finish; refusing the login");
                    PasswordVerdict::Rejected
                }
            };
            let _ = done.0.send(Verdict { connection, verdict });
        });
    }
}

#[cfg(test)]
mod tests {
    use openshard_login::DevAccounts;
    use openshard_protocol::identity::{
        AccountName,
        PlaintextPassword,
        RawAccountName,
        RawPlaintextPassword,
    };

    use super::*;

    fn accounts() -> DevAccounts {
        DevAccounts::new().with_account(&AccountName::new("admin"), &PlaintextPassword::new("hunter2"))
    }

    /// The check a `0x80` for `admin` with this password would produce.
    fn check(password: &str) -> CredentialCheck {
        let offered = RawPlaintextPassword::new(password);
        let (_account, check) = accounts()
            .credential(&RawAccountName::new("admin"), &offered)
            .expect("admin exists and is not blocked")
            .against(offered);
        check
    }

    #[tokio::test]
    async fn a_verdict_comes_back_for_the_connection_that_asked() {
        // The whole plumbing, end to end: a check goes out, argon2 runs somewhere
        // else, and what comes back is an answer tagged with whose it is. The tag
        // is the part worth pinning — the verdict carries no identity of its own,
        // so a check whose connection got lost would authenticate whoever it
        // reached.
        let (verifier, mut verdicts) = Verifier::new();
        let first = ConnectionId::from_raw(1);
        verifier.spawn(first, check("hunter2"));
        let answer = verdicts.recv().await.expect("the verifier answers");
        assert_eq!(answer.connection, first);
        assert_eq!(answer.verdict, PasswordVerdict::Matched);
    }

    #[tokio::test]
    async fn a_wrong_password_comes_back_rejected() {
        // And the other way, so the test above cannot pass on a verifier that says
        // yes to everything.
        let (verifier, mut verdicts) = Verifier::new();
        verifier.spawn(ConnectionId::from_raw(2), check("wrong"));
        let answer = verdicts.recv().await.expect("the verifier answers");
        assert_eq!(answer.verdict, PasswordVerdict::Rejected);
    }

    #[tokio::test]
    async fn every_check_past_the_bound_still_gets_an_answer() {
        // The permit is a queue, not a cap on how many logins are served. With one
        // permit, three checks run one after another and all three come back — a
        // permit held across the wrong await, or dropped before the send, would
        // strand the clients behind it in a state nothing else can move.
        let (verifier, mut verdicts) = Verifier::with_permits(1);
        for raw in 1..=3 {
            verifier.spawn(ConnectionId::from_raw(raw), check("hunter2"));
        }
        for _ in 0..3 {
            let answer = verdicts.recv().await.expect("every check answers");
            assert_eq!(answer.verdict, PasswordVerdict::Matched);
        }
    }
}
