//! The three strings that identify a player, kept apart so they cannot be
//! mixed up at a call site: which account they log in as, which character
//! they play, and the plaintext password the client just sent (never to be
//! confused with the argon2 hash a store keeps instead).
//!
//! # Raw off the wire, validated once
//!
//! [`AccountName`], [`CharacterName`] and [`PlaintextPassword`] are the
//! trusted, already-good forms — what `[[accounts]]` in a config file names,
//! what a store keys its rows by. A `0x80`/`0x91`/`0x00`/`0xF8` packet carries
//! the *same-shaped* bytes but they are client input, not an invariant: a
//! client can send an empty name, a 30-byte field padded with garbage, or
//! anything else the wire format does not forbid. [`RawAccountName`],
//! [`RawCharacterName`] and [`RawPlaintextPassword`] are that unchecked form —
//! every client-to-server wire struct in [`crate::login`] and [`crate::world`]
//! carries the `Raw` type, and the only way to a trusted [`AccountName`] or
//! [`CharacterName`] is through the check that makes one, in
//! `openshard_login::DevAccounts` lookup and character creation. This is the same
//! shape as `openshard_config`'s `RawAccessLevel` vs. `AccessLevel`.
//!
//! `PlaintextPassword` and `RawPlaintextPassword` differ only for this
//! symmetry: a password has no "validated" long-lived form (it is hashed or
//! compared and then dropped), but it is client input all the same and its
//! type should say so.
//!
//! This crate carries no dependencies (it is the leaf everything else in
//! `common`/`server` builds on), so these types stay serde-free like every
//! other newtype here — a config or persistence crate that needs to
//! (de)serialize one does it with a small `serialize_with`/`deserialize_with`
//! pair at its own field, not by pulling serde in here.
//!
//! `.0` unwraps only where a value crosses into a different domain — the wire
//! codec, a SQL bind, a `HashMap` key — never in ordinary call-tree code.

use std::fmt;

/// An account name, as typed at login or written in `[[accounts]]`.
///
/// Login is case-insensitive — `"Admin"` and `"admin"` are the same account —
/// but this type does not fold case itself. Every layer that keys a map by
/// account name calls [`AccountName::normalized`] explicitly at the point it
/// builds the key, the same way `Serial`/`EntityId` never hide validation
/// inside a trait impl. There is deliberately no `Default`: config validation
/// rejects the empty string, and there is no context-free account to invent.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct AccountName(pub String);

impl AccountName {
    /// Build one from borrowed text — the everyday constructor. The field
    /// stays `pub` for the rare case that needs to move an owned `String` in
    /// without a copy; call sites that only have a `&str` use this instead.
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }

    /// The case-folded form used to key a map, so `"Admin"` and `"admin"`
    /// collide on purpose instead of shadowing each other silently.
    pub fn normalized(&self) -> String {
        self.0.to_lowercase()
    }
}

impl fmt::Display for AccountName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Test fixtures compare against string literals constantly; comparing the
// wrapper directly against `&str` keeps those assertions readable without
// reaching for `.0` at every call site. Construction still goes through
// `AccountName(...)` — this is read-only ergonomics, not `Deref`.
impl PartialEq<str> for AccountName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for AccountName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// An account name exactly as a `0x80`/`0x91` packet carried it: whatever sat
/// in the fixed-width field, not yet checked for length or emptiness. See the
/// module docs — the login account lookup is the only place this becomes a real
/// [`AccountName`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RawAccountName(pub String);

impl RawAccountName {
    /// Build one from borrowed text. See [`AccountName::new`].
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl fmt::Display for RawAccountName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<str> for RawAccountName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for RawAccountName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// A character name, as typed at creation and shown in the character list.
///
/// `Default` is the empty name, meaning an unused character-list slot — see
/// [`crate::login::CharacterEntry`].
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct CharacterName(pub String);

impl CharacterName {
    /// Build one from borrowed text. See [`AccountName::new`].
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }

    /// The case-folded form used to key a map. See [`AccountName::normalized`].
    pub fn normalized(&self) -> String {
        self.0.to_lowercase()
    }
}

impl fmt::Display for CharacterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<str> for CharacterName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for CharacterName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// A character name exactly as a `0x00`/`0xF8` create-character packet
/// carried it: not yet trimmed, checked for length, or checked for
/// emptiness/duplication against the account. See the module docs —
/// character-creation code is the only place this becomes a real
/// [`CharacterName`].
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawCharacterName(pub String);

impl RawCharacterName {
    /// Build one from borrowed text. See [`AccountName::new`].
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl fmt::Display for RawCharacterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<str> for RawCharacterName {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for RawCharacterName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// A password as an operator wrote it in `openshard.toml`: plaintext, not yet
/// hashed.
///
/// No `Display`, and `Debug` is hand-written to redact — a stray `{:?}` in a
/// log line is a credential leak.
#[derive(Clone, PartialEq, Eq)]
pub struct PlaintextPassword(pub String);

impl PlaintextPassword {
    /// Build one from borrowed text. See [`AccountName::new`].
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl fmt::Debug for PlaintextPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaintextPassword(<redacted>)")
    }
}

/// A password exactly as a `0x80`/`0x91` packet carried it: plaintext, and —
/// unlike [`AccountName`]/[`CharacterName`] — never promoted to a "validated"
/// form, because a password has no long-lived trusted shape: it is hashed or
/// compared once and then dropped. It is still client input, so it gets its
/// own type rather than reusing [`PlaintextPassword`] — see the module docs.
///
/// The UO login packet's password field is plaintext inside encryption that
/// is trivially broken, so the protocol itself treats it as public on the
/// wire; `Debug` still redacts, because "public on the wire" is not a license
/// to put it in a log line.
#[derive(Clone, PartialEq, Eq)]
pub struct RawPlaintextPassword(pub String);

impl RawPlaintextPassword {
    /// Build one from borrowed text. See [`AccountName::new`].
    pub fn new(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl fmt::Debug for RawPlaintextPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RawPlaintextPassword(<redacted>)")
    }
}
