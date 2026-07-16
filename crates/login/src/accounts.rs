//! Who is allowed in.

use std::collections::HashMap;

use openshard_protocol::{CharacterEntry, DenyReason, ACCOUNT_NAME_LENGTH, PASSWORD_LENGTH};

/// Somewhere accounts live.
///
/// A trait because the dev store below is a placeholder: real shards keep
/// accounts in PostgreSQL, and the login state machine must not care which.
///
/// # Implementors must hash
///
/// The UO protocol sends passwords in plaintext — there is no challenge, no
/// nonce, nothing. That is the client's fault and cannot be fixed server-side.
/// What *can* be fixed is what happens next: an implementation of this trait
/// must compare against a slow password hash (argon2, bcrypt, scrypt) and must
/// never persist the plaintext. [`verify`](Accounts::verify) taking the
/// plaintext is unavoidable; storing it is not.
pub trait Accounts {
    /// Check a name and password.
    ///
    /// Returns the reason on failure so the caller can log it. What the client
    /// is told is a separate decision — see [`DenyReason::wire_code`].
    fn verify(&self, account: &str, password: &str) -> Result<(), DenyReason>;

    /// The characters on an account, in slot order.
    ///
    /// Empty for an account with none; the 0xA9 encoder pads the list out.
    fn characters(&self, account: &str) -> Vec<CharacterEntry>;
}

/// Compare two strings without leaking their contents through timing.
///
/// `==` on strings returns at the first differing byte, so how long it takes
/// tells an attacker how much of the password was right. That turns a
/// 2^n search into an n-by-256 one.
///
/// This is a small win — the network jitter in front of it dwarfs the signal,
/// and the real answer is a slow hash whose comparison is over fixed-width
/// digests. It costs three lines, so there is no reason to skip it.
fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    // Length is not secret: it is visible in the packet either way.
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

/// One account in the dev store.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DevAccount {
    /// The password, in plaintext.
    pub password: String,
    /// Whether logins are refused.
    pub blocked: bool,
    /// The characters on this account.
    pub characters: Vec<CharacterEntry>,
}

/// An in-memory account store, for development only.
///
/// # Not for production
///
/// It holds plaintext passwords, because the TOML file it is built from does
/// too. That is acceptable for a shard you are testing on a laptop and is not
/// acceptable anywhere else. When `openshard-persistence` lands, the real
/// [`Accounts`] implementation hashes and this one keeps its current job:
/// letting a test spin up a login server in one line.
#[derive(Clone, Default, Debug)]
pub struct DevAccounts {
    /// Keyed by lowercased name — the client does not preserve case reliably
    /// and players do not expect it to matter.
    accounts: HashMap<String, DevAccount>,
}

impl DevAccounts {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an account with a password and no characters.
    pub fn with_account(mut self, account: &str, password: &str) -> Self {
        self.accounts.insert(
            account.to_lowercase(),
            DevAccount {
                password: password.to_owned(),
                blocked: false,
                characters: Vec::new(),
            },
        );
        self
    }

    /// Add a character to an existing account. Ignored if there is no account.
    pub fn with_character(mut self, account: &str, name: &str) -> Self {
        if let Some(entry) = self.accounts.get_mut(&account.to_lowercase()) {
            entry.characters.push(CharacterEntry {
                name: name.to_owned(),
            });
        }
        self
    }

    /// Block an existing account. Ignored if there is no account.
    pub fn blocked(mut self, account: &str) -> Self {
        if let Some(entry) = self.accounts.get_mut(&account.to_lowercase()) {
            entry.blocked = true;
        }
        self
    }
}

impl Accounts for DevAccounts {
    fn verify(&self, account: &str, password: &str) -> Result<(), DenyReason> {
        // Reject nonsense before touching the store. These are the widths of
        // the wire fields, so anything longer never came from a real client.
        if account.is_empty() || account.len() > ACCOUNT_NAME_LENGTH {
            return Err(DenyReason::MalformedAccount);
        }
        if password.len() > PASSWORD_LENGTH {
            return Err(DenyReason::MalformedPassword);
        }

        let Some(entry) = self.accounts.get(&account.to_lowercase()) else {
            return Err(DenyReason::NoAccount);
        };
        if entry.blocked {
            return Err(DenyReason::Blocked);
        }
        if !constant_time_eq(&entry.password, password) {
            return Err(DenyReason::BadPassword);
        }
        Ok(())
    }

    fn characters(&self, account: &str) -> Vec<CharacterEntry> {
        self.accounts
            .get(&account.to_lowercase())
            .map(|entry| entry.characters.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> DevAccounts {
        DevAccounts::new()
            .with_account("admin", "hunter2")
            .with_character("admin", "Lord British")
            .with_account("banned", "x")
            .blocked("banned")
    }

    #[test]
    fn accepts_the_right_password() {
        assert_eq!(store().verify("admin", "hunter2"), Ok(()));
    }

    #[test]
    fn rejects_the_wrong_password() {
        assert_eq!(
            store().verify("admin", "hunter3"),
            Err(DenyReason::BadPassword)
        );
        assert_eq!(store().verify("admin", ""), Err(DenyReason::BadPassword));
    }

    #[test]
    fn rejects_an_unknown_account() {
        assert_eq!(
            store().verify("nobody", "hunter2"),
            Err(DenyReason::NoAccount)
        );
    }

    #[test]
    fn rejects_a_blocked_account_before_checking_the_password() {
        // Order matters: telling a banned account its password was right is a
        // small thing, but there is no reason to.
        assert_eq!(store().verify("banned", "x"), Err(DenyReason::Blocked));
        assert_eq!(store().verify("banned", "wrong"), Err(DenyReason::Blocked));
    }

    #[test]
    fn account_names_are_case_insensitive() {
        // The client does not round-trip case reliably, and no player expects
        // "Admin" and "admin" to be different accounts.
        assert_eq!(store().verify("ADMIN", "hunter2"), Ok(()));
        assert_eq!(store().verify("AdMiN", "hunter2"), Ok(()));
    }

    #[test]
    fn passwords_are_case_sensitive() {
        assert_eq!(
            store().verify("admin", "HUNTER2"),
            Err(DenyReason::BadPassword)
        );
    }

    #[test]
    fn rejects_names_that_no_client_could_have_sent() {
        // The wire field is 30 bytes, so anything longer is a forged packet or
        // a bug upstream. Either way it must not reach the store.
        let long = "x".repeat(ACCOUNT_NAME_LENGTH + 1);
        assert_eq!(
            store().verify(&long, "x"),
            Err(DenyReason::MalformedAccount)
        );
        assert_eq!(store().verify("", "x"), Err(DenyReason::MalformedAccount));

        let long_password = "x".repeat(PASSWORD_LENGTH + 1);
        assert_eq!(
            store().verify("admin", &long_password),
            Err(DenyReason::MalformedPassword)
        );
    }

    #[test]
    fn characters_come_back_in_order() {
        let store = DevAccounts::new()
            .with_account("a", "p")
            .with_character("a", "First")
            .with_character("a", "Second");
        let characters = store.characters("a");
        assert_eq!(characters.len(), 2);
        assert_eq!(characters[0].name, "First");
        assert_eq!(characters[1].name, "Second");
    }

    #[test]
    fn an_unknown_account_has_no_characters() {
        assert_eq!(store().characters("nobody"), vec![]);
    }

    #[test]
    fn constant_time_eq_still_compares_correctly() {
        // It is only useful if it is also right.
        assert!(constant_time_eq("", ""));
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("ab", "abc"));
        assert!(!constant_time_eq("abc", ""));
    }
}
