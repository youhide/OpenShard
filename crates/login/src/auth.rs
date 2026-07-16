//! Auth keys: the thread tying a shard selection to the game login that follows.
//!
//! The client picks a shard, the login server answers `0x8C` with an address and
//! a 32-bit key, the client reconnects to that address and echoes the key in
//! `0x91`. That is the only thing linking the two TCP connections.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// What an issued key stands for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PendingLogin {
    /// The account that was verified on the login connection.
    pub account: String,
    /// When the key was issued.
    pub issued_at: Instant,
}

/// Keys issued at relay and redeemed at game login.
///
/// # Not a global
///
/// This is cross-connection state, which is exactly the kind of thing that
/// wants to become a singleton. It does not: the login server owns one as a
/// plain field, and a test owns as many as it likes.
///
/// # The clock is a parameter
///
/// Every method that cares about time takes `now`. Reading the clock internally
/// would mean testing expiry with `sleep`, which is slow and flaky, so instead
/// the tests hand it whatever instant they want to talk about.
#[derive(Debug)]
pub struct AuthKeys {
    issued: HashMap<u32, PendingLogin>,
    ttl: Duration,
}

/// How long a key stays valid.
///
/// Long enough for a client to tear down one TCP connection and open another,
/// which is all it is for. Anything longer is a window for a key to be reused
/// by someone who read it off the wire — the protocol has no encryption worth
/// the name, so the key is visible to anyone on the path.
pub const DEFAULT_TTL: Duration = Duration::from_secs(30);

impl AuthKeys {
    /// A store with the default expiry.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    /// A store with a custom expiry.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            issued: HashMap::new(),
            ttl,
        }
    }

    /// Issue a key for `account`.
    ///
    /// The key comes from the OS entropy pool, not a counter. A predictable key
    /// would let anyone who guesses it skip the login server and present
    /// themselves at the game port as a session that had already been
    /// verified — the account name and password are re-sent in `0x91`, so this
    /// is not the only gate, but it should not be a free one either.
    pub fn issue(&mut self, account: &str, now: Instant) -> u32 {
        // Never hand out 0: the client sends 0 when it has no key, so a real key
        // of 0 would make "no key" and "this key" indistinguishable.
        let key = loop {
            let candidate = random_u32();
            if candidate != 0 && !self.issued.contains_key(&candidate) {
                break candidate;
            }
        };
        self.issued.insert(
            key,
            PendingLogin {
                account: account.to_owned(),
                issued_at: now,
            },
        );
        key
    }

    /// Redeem a key, consuming it.
    ///
    /// One shot: a key that has been used is gone. Two clients presenting the
    /// same key means one of them read it off the wire, and neither should get
    /// a second try with it.
    ///
    /// Returns `None` for a key that was never issued, has already been
    /// redeemed, or has expired.
    pub fn redeem(&mut self, key: u32, now: Instant) -> Option<PendingLogin> {
        let pending = self.issued.remove(&key)?;
        if now.duration_since(pending.issued_at) > self.ttl {
            // Already removed, so an expired key cannot be retried either.
            return None;
        }
        Some(pending)
    }

    /// Drop expired keys.
    ///
    /// Redemption checks expiry on its own, so this is only about memory: a
    /// client that selects a shard and never reconnects leaves a key behind.
    /// Call it on a timer.
    pub fn expire(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.issued
            .retain(|_, pending| now.duration_since(pending.issued_at) <= ttl);
    }

    /// How many keys are outstanding.
    pub fn len(&self) -> usize {
        self.issued.len()
    }

    /// Whether any key is outstanding.
    pub fn is_empty(&self) -> bool {
        self.issued.is_empty()
    }
}

impl Default for AuthKeys {
    fn default() -> Self {
        Self::new()
    }
}

/// Four bytes from the OS entropy pool.
fn random_u32() -> u32 {
    let mut bytes = [0u8; 4];
    // A failure here means the OS has no entropy source, which is not a thing
    // this process can recover from or paper over — issuing a predictable key
    // would be worse than not starting.
    getrandom::getrandom(&mut bytes).expect("the OS entropy pool is unavailable");
    u32::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_round_trips() {
        let mut keys = AuthKeys::new();
        let now = Instant::now();
        let key = keys.issue("admin", now);

        let pending = keys.redeem(key, now).unwrap();
        assert_eq!(pending.account, "admin");
    }

    #[test]
    fn a_key_is_one_shot() {
        // Two clients with the same key means one of them was listening.
        let mut keys = AuthKeys::new();
        let now = Instant::now();
        let key = keys.issue("admin", now);

        assert!(keys.redeem(key, now).is_some());
        assert_eq!(keys.redeem(key, now), None, "a key must not be reusable");
    }

    #[test]
    fn an_unissued_key_is_refused() {
        let mut keys = AuthKeys::new();
        assert_eq!(keys.redeem(0xDEAD_BEEF, Instant::now()), None);
        assert_eq!(keys.redeem(0, Instant::now()), None);
    }

    #[test]
    fn a_key_expires() {
        let mut keys = AuthKeys::with_ttl(Duration::from_secs(30));
        let issued = Instant::now();
        let key = keys.issue("admin", issued);

        let just_in_time = issued + Duration::from_secs(30);
        assert!(
            keys.redeem(key, just_in_time).is_some(),
            "the boundary is inclusive"
        );

        let key = keys.issue("admin", issued);
        let too_late = issued + Duration::from_secs(31);
        assert_eq!(keys.redeem(key, too_late), None);
    }

    #[test]
    fn an_expired_key_is_consumed_anyway() {
        // Otherwise a key sitting past its expiry could be retried forever, and
        // an attacker gets unlimited attempts at a key they half-know.
        let mut keys = AuthKeys::with_ttl(Duration::from_secs(1));
        let issued = Instant::now();
        let key = keys.issue("admin", issued);

        assert_eq!(keys.redeem(key, issued + Duration::from_secs(5)), None);
        assert!(keys.is_empty(), "the expired key is gone, not retryable");
        assert_eq!(keys.redeem(key, issued), None, "even at a valid instant");
    }

    #[test]
    fn expire_reclaims_abandoned_keys() {
        // A client that picks a shard and never reconnects.
        let mut keys = AuthKeys::with_ttl(Duration::from_secs(30));
        let issued = Instant::now();
        for _ in 0..100 {
            keys.issue("admin", issued);
        }
        assert_eq!(keys.len(), 100);

        keys.expire(issued + Duration::from_secs(10));
        assert_eq!(keys.len(), 100, "not yet");

        keys.expire(issued + Duration::from_secs(31));
        assert!(keys.is_empty());
    }

    #[test]
    fn keys_are_never_zero() {
        // The client sends 0 for "no key"; a real key of 0 would collide.
        let mut keys = AuthKeys::new();
        let now = Instant::now();
        for _ in 0..1000 {
            assert_ne!(keys.issue("admin", now), 0);
        }
    }

    #[test]
    fn keys_are_not_a_counter() {
        // A weak check — it would pass for any halfway-random source — but it
        // catches the one failure that matters: someone swapping the CSPRNG for
        // an incrementing id because it was simpler.
        let mut keys = AuthKeys::new();
        let now = Instant::now();
        let issued: Vec<u32> = (0..64).map(|_| keys.issue("admin", now)).collect();

        assert_eq!(keys.len(), 64, "keys must be distinct");
        let sequential = issued.windows(2).filter(|w| w[1] == w[0] + 1).count();
        assert!(sequential < 8, "{sequential} of 63 pairs were consecutive");
    }

    #[test]
    fn keys_from_different_accounts_do_not_mix() {
        let mut keys = AuthKeys::new();
        let now = Instant::now();
        let a = keys.issue("alice", now);
        let b = keys.issue("bob", now);

        assert_eq!(keys.redeem(a, now).unwrap().account, "alice");
        assert_eq!(keys.redeem(b, now).unwrap().account, "bob");
    }
}
