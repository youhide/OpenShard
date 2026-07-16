//! What outlives the process.
//!
//! # These are not components
//!
//! A [`CharacterRecord`] looks like `Position` plus `Name` plus `Body` flattened
//! into one struct, and the temptation is to serialise the components directly
//! and delete this file.
//!
//! The reason not to is that the two change for different reasons. A component
//! changes whenever the simulation wants a better shape — split `Body` in two,
//! move `Heading` into `Position`, add a field the tick needs — and none of that
//! should reach into a database that already has a million rows in it. A record
//! changes only when the *saved* meaning changes, which is rare and deliberate,
//! and when it does it comes with [`SCHEMA_VERSION`] and a migration.
//!
//! The conversion between them is the seam where that difference is absorbed.
//! Serialising components directly deletes the seam and welds the simulation's
//! internal shape to the on-disk format forever.

use serde::{Deserialize, Serialize};

/// The version of the saved shape.
///
/// Bumped when a record changes meaning, not when the simulation is refactored
/// around it. A store that opens a save from the future must refuse rather than
/// guess: reading a newer save with older code is how a shard silently drops
/// every field it does not recognise and then writes the loss back.
pub const SCHEMA_VERSION: u32 = 1;

/// An account, as saved.
///
/// # The password is not here, and that is deliberate
///
/// This carries whatever the account store uses to check a login, and nothing
/// says that is a password. Today it is plaintext, because there is no hashing
/// yet and pretending otherwise in the type name would be worse than admitting
/// it. When hashing lands, this field changes and [`SCHEMA_VERSION`] moves.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AccountRecord {
    /// The login name. Unique; this is the key.
    pub name: String,
    /// The credential, as the account store stores it.
    pub credential: String,
}

/// A character, as saved.
///
/// # Why the serial is in here
///
/// A serial is not an implementation detail the server may re-pick on load. It
/// is on the wire, in every packet a client has ever been sent, and — once there
/// are items — it is what a container's contents point at. A character that
/// comes back with a different serial is a different character with the same
/// name, and everything that referred to the old one now refers to nothing.
///
/// So it is saved, and [`openshard_entities::Registry::bind_serial`] reserves it
/// on the way back in rather than handing it out again to someone else.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CharacterRecord {
    /// The wire serial. Stable across restarts; see the type docs.
    pub serial: u32,
    /// Which account it belongs to.
    pub account: String,
    /// The character's name.
    pub name: String,
    /// The body graphic.
    pub body: u16,
    /// The body hue.
    pub hue: u16,
    /// Which facet.
    pub facet: u8,
    /// Where it stands.
    pub x: u16,
    /// Where it stands.
    pub y: u16,
    /// How high it stands. Signed: UO has basements.
    pub z: i8,
    /// Which way it faces, as the wire byte.
    pub facing: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_record_round_trips_through_json() {
        // Not a test of serde. A test that every field is reachable by name from
        // outside the crate: a field that is private, skipped, or renamed by
        // accident is a field that comes back as its default, and a character
        // that loads with a default position is standing in the ocean.
        let record = CharacterRecord {
            serial: 0x0000_0001,
            account: "admin".into(),
            name: "Alpha".into(),
            body: 0x0190,
            hue: 0,
            facet: 0,
            x: 1363,
            y: 1600,
            z: 30,
            facing: 3,
        };
        let json = serde_json::to_string(&record).expect("a record must serialise");
        let back: CharacterRecord = serde_json::from_str(&json).expect("and come back");
        assert_eq!(back, record);
    }

    #[test]
    fn a_negative_height_survives_the_round_trip() {
        // z is i8 and the obvious mistake is u8. UO has basements, mines and
        // dungeons at negative heights, and a character saved at z=-40 that
        // loads at z=216 is somewhere else entirely.
        let record = CharacterRecord {
            serial: 1,
            account: "admin".into(),
            name: "Alpha".into(),
            body: 0x0190,
            hue: 0,
            facet: 0,
            x: 5000,
            y: 500,
            z: -40,
            facing: 0,
        };
        let json = serde_json::to_string(&record).expect("a record must serialise");
        let back: CharacterRecord = serde_json::from_str(&json).expect("and come back");
        assert_eq!(back.z, -40);
    }
}
