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

use openshard_protocol::identity::{AccountName, CharacterName};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::serial::Serial;
#[cfg(test)]
use openshard_protocol::world::PoisonLevel;
use openshard_protocol::world::{
    Aggression, DamageType, FollowerSlots, PhysicalResistance, RangedRange, Sight,
};
use serde::{Deserialize, Serialize};

/// The persisted state of a container trap.
///
/// The database has three independently indexed columns and older JSON saves
/// encode the same value as a three-element array.  A named Rust value makes
/// those meanings impossible to exchange while `trap` below keeps that saved
/// representation stable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrapRecord {
    pub kind: u8,
    pub power: u16,
    pub level: u8,
}

mod trap {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};
    use std::fmt;

    use super::TrapRecord;

    pub fn serialize<S: Serializer>(value: &Option<TrapRecord>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            None => serializer.serialize_none(),
            Some(trap) => {
                let mut sequence = serializer.serialize_seq(Some(3))?;
                sequence.serialize_element(&trap.kind)?;
                sequence.serialize_element(&trap.power)?;
                sequence.serialize_element(&trap.level)?;
                sequence.end()
            }
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<TrapRecord>, D::Error> {
        struct TrapVisitor;

        impl<'de> Visitor<'de> for TrapVisitor {
            type Value = Option<TrapRecord>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a trap array `[kind, power, level]` or null")
            }

            fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
                deserializer.deserialize_seq(self)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
                let kind = sequence
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(0, &self))?;
                let power = sequence
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(1, &self))?;
                let level = sequence
                    .next_element()?
                    .ok_or_else(|| A::Error::invalid_length(2, &self))?;
                if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::invalid_length(4, &self));
                }
                Ok(Some(TrapRecord { kind, power, level }))
            }
        }

        deserializer.deserialize_option(TrapVisitor)
    }
}

/// (De)serialize an [`AccountName`] as the bare string, no wrapper object.
///
/// `openshard-protocol` carries no dependencies, so `AccountName` stays
/// serde-free and each crate that needs to (de)serialize one supplies this
/// small `serialize_with`/`deserialize_with` pair itself — see
/// `openshard_config`'s identical `account_name` module for the same reasoning.
mod account_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::AccountName;

    pub fn serialize<S: Serializer>(value: &AccountName, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<AccountName, D::Error> {
        String::deserialize(d).map(AccountName)
    }
}

/// (De)serialize a [`CharacterName`] as the bare string. See [`account_name`].
mod character_name {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::CharacterName;

    pub fn serialize<S: Serializer>(value: &CharacterName, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CharacterName, D::Error> {
        String::deserialize(d).map(CharacterName)
    }
}

/// (De)serialize a [`Serial`] as the bare wire integer, no wrapper object.
///
/// The database has always stored these as plain `u32` columns; wrapping the
/// Rust-side field in a checked newtype must not move the on-disk shape, so
/// this writes and reads exactly the integer [`Serial::raw`] returns. A value
/// read back that fails [`Serial::new`] is a corrupt row, not an absent one —
/// every field this is used on is a serial that was valid when saved — so
/// deserialization errors out rather than silently defaulting to something
/// that would address a different object.
mod serial {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Serial;

    pub fn serialize<S: Serializer>(value: &Serial, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(value.raw())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Serial, D::Error> {
        let raw = u32::deserialize(d)?;
        Serial::new(raw).ok_or_else(|| D::Error::custom(format!("not a valid serial: {raw:#010X}")))
    }
}

/// (De)serialize an `Option<Serial>` as the bare wire integer, `0` for `None`.
///
/// Matches the sentinel this crate already used for "no object" before the
/// field was typed: a ground item's `owner`, an NPC's absent `spawned_by`, and
/// the rest were `u32` `0`, and the on-disk shape stays that same `0` — only
/// the Rust-side type changes, from a magic number to an absent [`Serial`].
mod optional_serial {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Serial;

    pub fn serialize<S: Serializer>(value: &Option<Serial>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(serial) => s.serialize_u32(serial.raw()),
            None => s.serialize_u32(0),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Serial>, D::Error> {
        let raw = u32::deserialize(d)?;
        if raw == 0 {
            Ok(None)
        } else {
            Serial::new(raw)
                .map(Some)
                .ok_or_else(|| D::Error::custom(format!("not a valid serial: {raw:#010X}")))
        }
    }
}

/// The version of the saved shape.
///
/// Bumped when a record changes meaning, not when the simulation is refactored
/// around it. A store that opens a save from the future must refuse rather than
/// guess: reading a newer save with older code is how a shard silently drops
/// every field it does not recognise and then writes the loss back.
///
/// - v1: characters only.
/// - v2: items — a character's carried inventory, and loose things on the ground.
/// - v3: an item's `stackable` flag.
/// - v4: spawn regions and their respawn timers.
/// - v5: the whole world — NPC mobiles (with gear and vendor stock), decoration
///   (with door state), and an item's `price`/`name`.
/// - v6: a character's stats and skills (with their lock arrows).
/// - v7: a mobile's active effects — poison today, buffs and debuffs as they
///   land — so a relog cannot wash a debuff off, the way ServUO keeps a
///   logged-out mobile's timers running and Sphere saves its effect tags.
/// - v8: a spellbook's learned-spell bitmask, so a bought book still opens to
///   its spells after a relog.
/// - v9: a character's `dead` flag, so a player who logged out a ghost logs back
///   in a ghost — the ghost body is re-derived, but the fact of death is saved.
/// - v10: an account's credential is an argon2 hash, not plaintext. A mismatched
///   older database is recreated (the account rows re-seed from config, hashed),
///   which is the convention every bump above shares.
/// - v11: a character's quest log — an opaque JSON blob the community pack owns
///   and the engine only stores, so quest progress survives a relog. Empty for a
///   character with no quests, and old saves default it, so the bump is additive.
/// - v12: a facet's named regions, and the world clock. Both are things a restart
///   would otherwise lose to a shard that looks fine: no guards, no town music,
///   daylight in every dungeon, and every night starting over at boot.
/// - v13: quests, structurally. The v11 blob is **gone**, not migrated: it held a
///   format only the script pack understood, and the quest system that understands
///   it now lives in the engine, so there is nothing to translate it into that
///   would not be a guess. A shard upgrading past v13 loses quest progress and
///   keeps everything else, which is the recreate-on-mismatch convention every
///   bump above shares. What replaces it: a character's quests and their
///   cooldowns, and, on a mobile, whether it gives quests or can be escorted —
///   the last two being why quest givers went inert after every restart.
/// - v14: a townsperson's **trade** (`MobileRecord::title`) and, with the optional
///   routine, where it sleeps (`night_home`). The trade is the key its outfit, its
///   generated name and — every time anyone speaks near it — its keyword table are
///   all looked up by, so an NPC restored without it is a mute statue that a save
///   file cannot tell apart from a working one. This is the `quest_giver` lesson
///   applied before it could bite: a binding that lives only in the spawn call that
///   placed it is lost at the first restart, silently. Added with `serde(default)`,
///   so an older save reads as a town of trade-less NPCs rather than failing. It
///   also carries a vendor's **full shelf** (`restock`), for the same reason: the
///   crate's live contents are what is *left*, so a restock timer with nothing to
///   compare them against would forget what full meant at every reboot.
/// - v15: standing — a character's **fame**, **karma** and **murder count**. The first
///   two are new (ServUO's `Titles`); the third was a bug, not an addition: the count
///   that makes a repeat killer permanently red lived only in memory, so every restart
///   washed every murderer blue while the decay clock and the notoriety rule around it
///   were both already correct. Creature fame/karma rides the JSON `MobileRecord` and
///   `CreatureData` with no column of its own.
/// - v16: a character's **skill caps** and its three **stat arrows**, together
///   with when each stat last rose. All three are inputs the gain reads every
///   time it fires: a per-skill cap decides how much headroom is left, a "down"
///   arrow decides which skill gives ground at the total cap, and a stat arrow
///   decides whether a stat may rise at all. Kept only in memory they reset at
///   every restart, which reads as the arrows the player set quietly snapping
///   back to "up" — the `Murders` lesson from v15, caught before it shipped.
/// - v17: a **corpse's story** — who it was, who killed it, who has read it with
///   Forensic Evaluation and who has rifled it. A corpse lies for seven minutes
///   and a shard restarts inside that window, so without this the body somebody
///   was investigating comes back anonymous, killed by nobody and disturbed by
///   no one. One nullable JSON column on the item row, `None` for every item that
///   is not a corpse.
/// - v18: the **poison on an item** — a bottled dose or the coating the Poisoning
///   skill put on a blade. It has to be saved for the same reason a spellbook's
///   mask does: all four poison potions are the same graphic, so an unsaved bottle
///   comes back as an empty one, and a coated sword a player spent a potion on
///   comes back clean.
/// - v19: the **trap on a container**. A restart that quietly disarms every chest
///   on the shard is the same class of silent loss as one that forgets a lock, and
///   the disarm is a skill somebody spent points on.
/// - v20: **how much is left in a thing that wears out** — a harvesting tool's
///   swings and an instrument's tunes, which are one interface in ServUO
///   (`IUsesRemaining`) and so one column here. The instrument half is a bug this
///   fixes rather than a feature it adds: a lute bought and half played came back
///   full at every reboot, because nothing saved the count.
/// - v22: **where a rune points and what a runebook holds**. Both columns land
///   together even though the book fills a slice later than the rune: there are
///   no migrations here — [`SqliteStore::init`](crate::SqliteStore) and its
///   Postgres twin stamp a fresh database and refuse any other version — so two
///   bumps inside one piece of work means an operator throwing their test shard
///   away twice for one feature.
/// - v23: **where the world's roll generator got to** ([`WorldRecord::rng_state`]),
///   beside the clock it already shares a row with. Unsaved, the generator was
///   re-seeded from a constant at every boot, so a restart dealt out the exact
///   sequence of rolls the previous run had dealt — every skill gain, every
///   swing, every loot roll, in order. That is the `Murders` class of bug from
///   v15 (a rule correct in memory and lost at the door) with an exploit attached,
///   since the one thing a player can do to a roll they dislike is get the shard
///   restarted.
/// - v24: **guilds**. A guild lived only in memory, so a shard that restarted
///   dissolved every one of them — the name, the roster, the wars — while every
///   member's `GuildMember` component pointed at an id nothing answered to. Three
///   things land together: the guilds themselves ([`GuildRecord`]), each
///   character's membership and title, and the id counter
///   ([`WorldRecord::guild_high_water`]), which is the one that is not obvious.
///   Without the counter a restart hands the next guild founded an id a
///   disbanded one already used, and every stale member record silently joins it.
/// - v25: **guild ranks**. One field,
///   [`CharacterRecord::guild_rank`], and a version anyway. It defaults to `0` —
///   Ronin — which is the *safe* reading and the wrong one: every existing
///   member, the leaders included, would come back holding no permission at all,
///   and a guild whose leader could not invite, promote or disband is a guild
///   with no way out of that state. There is no migration that could fix it
///   either, because which of them led is on the guild and which were Emissaries
///   was never written down. So the version refuses the old database rather than
///   opening it into a shard where every guild is inert.
/// - v26: **named alliances**. Being allied was a pairwise declaration stored in
///   the same list as a war, told apart by [`GuildStanding::at_war`]. It is a
///   named group now ([`AllianceRecord`]) that a guild is invited into, and the
///   old rows cannot be converted: an `at_war: false` standing between A and B
///   says nothing about which alliance they were both meant to be in, or what it
///   was called. Every one would have to become an alliance of two under a
///   made-up name, which is worse than refusing.
/// - v27: **houses**. A new table and no unreadable old data, which makes this
///   the first bump that is not about *reading*. A v26 database opens fine and
///   simply holds no houses, which is true of it. What it must not do is keep
///   being written by a build that does not know about them: an older engine
///   would read the version, agree, ignore the `houses` table, and go on handing
///   out item serials — one of which a saved house already holds. The rows would
///   survive and point at somebody's chest. So the bump is for the *writer*, not
///   the reader, and it is worth saying because every version above it was the
///   other way round.
/// - v28: a house's **access lists** — co-owners, friends and bans. v27's own
///   argument, one turn further: a v27 build knows about houses and not about
///   who may enter one, so it would read a house, drop the three lists, and
///   write it back without them. That is not a shard with no lists, it is a
///   shard that *deletes* them on the first save, which is worse than refusing
///   to open.
/// - v29: **lockdowns and secures**. v28's argument a third time, and the
///   sharpest of the three, because this one is not a list on the house — it is
///   a component on every pinned *item*. A v28 build reads those items as
///   ordinary ground clutter, writes them back without the pin, and a shard's
///   houses come up with every lockdown released and every secure standing open.
///   The house's own ceiling goes the same way: dropped on the read, written
///   back as nothing, and then no lockdown fits anywhere.
/// - v30: **house decay**. One column, `houses.age`, and a bump for the writer
///   rather than the reader — v27's own case. A v29 build opens the database,
///   ignores the column, and writes every house back with the default: which is
///   `0`, so every house on the shard silently becomes freshly refreshed on the
///   first save, and nothing ever collapses again. The reader's side is harmless
///   by comparison, which is why the bump is about the other one.
/// - v31: **house designs**, and for once the *reader's* case. The last four
///   bumps were about stopping an older writer; this one is about an older
///   reader being confidently wrong. A v30 build opens the database, does not
///   know the `house_designs` table so does not drop it, reads a house, sees a
///   foundation multi id, and computes the footprint from `multi_components` —
///   which for a foundation is a bare platform. The shard comes up with a
///   customised house wearing the foundation's walls, and nothing says so. That
///   is worse than a house with no walls, which is at least visible.
/// - v32: **boats**. A new table and nothing else touched, so an older *reader*
///   is only missing ships — but an older **writer** is the problem again, and
///   worse than v30's: it does not know the `boats` table, saves a world with
///   none in it, and every ship on the shard is gone on the next boot along with
///   whatever was standing on the deck's tiles. A house at least stays where it
///   was; a fleet does not come back.
pub const SCHEMA_VERSION: u32 = 32;

/// One component of a house whose shape nobody shipped.
///
/// # Why components are saved here and nowhere else
///
/// [`HouseRecord`]'s own note says the components are deliberately absent: a
/// multi's shape is a pure function of its id and lives in the client's files,
/// so a copy goes stale the day the operator updates their install. That rule
/// holds and it needed saying more precisely — **what is never saved is a copy
/// of something the client's files already state.** A design says nothing they
/// say. It *is* the original, with nothing to go stale against.
///
/// # A table, not a column
///
/// One row per component, keyed by the house's serial. A `HouseRecord` is small
/// and swept for every house on every save; a design is a few hundred rows. And a
/// classic house writes **no rows at all**, so the overwhelmingly common case
/// pays nothing. The cost, named: a second query on restore, joined by serial.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HouseDesignRecord {
    /// Which house, by its item serial.
    #[serde(with = "serial")]
    pub house: Serial,
    /// Bumped on every commit, so a client can cache the design by
    /// `(serial, revision)`. Repeated on every row of one house rather than kept
    /// beside the house, because the design is what it versions and a house with
    /// no design has no revision to store.
    pub revision: u32,
    /// The static this component draws as.
    pub graphic: u16,
    /// East of the house's origin.
    pub dx: i16,
    /// South of it.
    pub dy: i16,
    /// And above.
    pub dz: i16,
    /// Whether the client draws it. A `u64` because the two multi formats
    /// disagree about the field's width *and* its sense — see
    /// `openshard_uofiles::multi` — and the reader has already normalised both
    /// into "non-zero is drawn" by the time a design is built from one.
    pub flags: u64,
}

/// A ship, as saved.
///
/// **The components are not here**, and unlike a house this is not even a close
/// call: a boat's shape is a pure function of its multi id with no designed case
/// at all, so it is exactly what [`HouseRecord`]'s rule was written for. The
/// derived answer — which tiles are hull and which are deck, at what height — is
/// recomputed at boot from the same table the placement read.
///
/// No access lists and no age. A boat's own property rules are B4's, and adding
/// the columns before there is anything to put in them would be guessing at
/// their shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BoatRecord {
    /// Its item serial, so the ship comes back as the same thing to a client
    /// that had it on screen.
    #[serde(with = "serial")]
    pub serial: Serial,
    /// Which multi, `0x4000` below the graphic on the wire.
    pub multi: u16,
    /// Where its origin floats — not the corner of its box.
    pub x: u16,
    /// The same, south.
    pub y: u16,
    /// And its height, which for a moored ship is the waterline.
    pub z: i8,
    /// Which facet it is on.
    pub facet: u8,
    /// Who owns it.
    #[serde(with = "serial")]
    pub owner: Serial,
}

/// A player's house, as saved.
///
/// **The components are not here**, and that is the point: a multi's shape is a
/// pure function of its id and it lives in the client's files, so saving it would
/// be saving a copy of a file every client already has — one that goes stale the
/// day the operator updates their install. What is saved is where the house
/// stands and which multi it is, and the footprint is recomputed at boot.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HouseRecord {
    /// Its item serial, so the entity comes back as the same thing to a client
    /// that had it on screen.
    #[serde(with = "serial")]
    pub serial: Serial,
    /// Which multi, `0x4000` below the graphic on the wire.
    pub multi: u16,
    /// Where its origin stands — not the corner of its box. See
    /// `openshard_uofiles::multi::Multi::center`.
    pub x: u16,
    /// The same, south.
    pub y: u16,
    /// And its height.
    pub z: i8,
    /// Which facet it is on.
    pub facet: u8,
    /// Who owns it.
    #[serde(with = "serial")]
    pub owner: Serial,
    /// Everyone trusted short of owning it.
    #[serde(default)]
    pub co_owners: Vec<u32>,
    /// Everyone who may come in.
    #[serde(default)]
    pub friends: Vec<u32>,
    /// Everyone turned away.
    #[serde(default)]
    pub bans: Vec<u32>,
    /// How many ticks it has stood unrefreshed.
    ///
    /// The one timer in this engine saved as an elapsed count rather than as a
    /// deadline, because it is the one that has to cross a restart: the tick
    /// counter is not saved, so an absolute tick would read as zero on the way
    /// back in and every house on the shard would come up freshly refreshed.
    pub age: u64,
    /// How many items may be locked down here.
    ///
    /// Saved rather than recomputed, unlike the walls and the sign, and the
    /// difference is worth stating: those are a pure function of the multi id,
    /// and this is that function *times a shard's own tuning constant*.
    /// Recomputing it at boot would mean an operator who lowered
    /// `LOCKDOWNS_PER_TILE` finding half the shard's lockdowns over the new
    /// ceiling with nothing to say which ones, and one who raised it handing out
    /// the difference to houses placed before the decision.
    pub lockdowns: u32,
}

/// An account, as saved.
///
/// # The credential is a hash, not a password
///
/// [`credential`](Self::credential) is an argon2 PHC string
/// (`$argon2id$v=19$...`), never the plaintext. The UO login sends the password
/// in the clear and that cannot be fixed; what is fixed is that it is hashed
/// before it reaches here and the plaintext is dropped. See
/// `openshard_login::password`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AccountRecord {
    /// The login name. Unique; this is the key.
    #[serde(with = "account_name")]
    pub name: AccountName,
    /// The credential — an argon2 PHC hash of the password.
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
    #[serde(with = "serial")]
    pub serial: Serial,
    /// Which account it belongs to.
    #[serde(with = "account_name")]
    pub account: AccountName,
    /// The character's name.
    #[serde(with = "character_name")]
    pub name: CharacterName,
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
    /// Strength — caps hit points.
    #[serde(default = "default_stat")]
    pub strength: u16,
    /// Dexterity — the stamina pool and swing pace.
    #[serde(default = "default_stat")]
    pub dexterity: u16,
    /// Intelligence — caps mana.
    #[serde(default = "default_stat")]
    pub intelligence: u16,
    /// Every trained skill, as `(id, value in tenths, lock byte, cap)`. Empty for
    /// a character that has none yet.
    #[serde(default)]
    pub skills: Vec<SkillRecord>,
    /// Which way the three stats are set to train, and how long since each rose.
    #[serde(default)]
    pub stat_locks: StatLockRecord,
    /// Every timed effect working through it — poison, buffs, debuffs — so a
    /// relog cannot wash them off. Empty for a clean character.
    #[serde(default)]
    pub effects: Vec<EffectRecord>,
    /// Whether it logged out dead. A ghost that relogs comes back a ghost — the
    /// grey body and death shroud are re-derived on login; only the fact of death
    /// rides here. The `body`/`hue` above stay the *living* ones, so resurrection
    /// restores the character exactly. `false` for the living, the common case.
    #[serde(default)]
    pub dead: bool,
    /// How widely known the character is — ServUO's `Mobile.Fame`.
    #[serde(default)]
    pub fame: i32,
    /// Which way it is known — ServUO's `Mobile.Karma`.
    #[serde(default)]
    pub karma: i32,
    /// How many innocents this character has killed.
    ///
    /// Saved because it is a **standing**, and it was not: the fifth murder makes a
    /// character a murderer for good, and a count that lives only in memory washed
    /// every red blue at the next restart. Everything else about the flag — the decay
    /// clock, the notoriety it forces — was already right; the number underneath it
    /// simply went missing at the door.
    #[serde(default)]
    pub murders: u16,
    /// Every quest in progress, with how far each objective has got.
    #[serde(default)]
    pub quests: Vec<QuestRecord>,
    /// Every quest already finished, with the cooldown before it may be taken
    /// again. Kept separately from [`quests`](Self::quests) because a finished
    /// quest has no progress left to save — only a date.
    #[serde(default)]
    pub done_quests: Vec<DoneQuestRecord>,
    /// Which guild it belongs to, by [`GuildRecord::id`], or `None` for the
    /// unguilded — which is most characters.
    ///
    /// On the character rather than a roster on the guild, the same way
    /// `GuildMember` is a component: the question asked is "what guild is *this*
    /// one in", and a roster is the rare direction. An id naming a guild the
    /// store no longer has reads as no guild — see
    /// [`WorldState::guild_of`](openshard_state::WorldState::guild_of) — so a
    /// guild dropped by hand from the database orphans nobody.
    #[serde(default)]
    pub guild: Option<u32>,
    /// The title the guild knows it by — "Master of Arms". Free text a leader
    /// typed. Empty for a member the guild has not named and for anyone in no
    /// guild, and **not** the rank — see [`guild_rank`](Self::guild_rank).
    #[serde(default)]
    pub guild_title: String,
    /// Where it stands in the guild, as
    /// [`Rank::number`](openshard_state::Rank::number) — 0 Ronin through 4
    /// Leader.
    ///
    /// A number rather than the enum, for the reason every other saved
    /// discriminant here is one: the record is the format, and a variant
    /// reordered in the source must not silently repoint every saved row. A
    /// number outside the five reads as `Ronin` on the way back in
    /// (`Rank::from_number` answers `None` and the caller takes the floor),
    /// which is the safe direction — an unreadable rank should not be able to
    /// grant anything.
    #[serde(default)]
    pub guild_rank: u8,
    /// A guild that has asked it to join and is waiting on an answer.
    ///
    /// Saved, and worth saying why: an invitation left for a player who was
    /// offline is exactly the invitation that needs to survive a restart. One at
    /// a time, because there is one answer.
    #[serde(default)]
    pub guild_candidate: Option<u32>,
}

/// How one guild stands with another, as saved.
///
/// # `at_war` is the only thing it can say now
///
/// It used to distinguish a war from an alliance, because being allied was a
/// pairwise declaration. Alliances are named groups with their own record
/// ([`AllianceRecord`]), so every standing here is a war and the flag is
/// vestigial — kept rather than dropped because dropping it would move the JSON
/// shape for no gain, and `#[serde(default)]` would then read every saved war as
/// a peace.
///
/// A `bool` rather than a three-state, because absence *is* the neutral case:
/// two guilds with no declared relation are simply not in each other's lists.
/// The in-memory [`Relation`](openshard_state::Relation) makes the same choice
/// and for the same reason — a "neutral" variant would be a second way to spell
/// nothing, and a third state to keep in step.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GuildStanding {
    /// The other guild, by [`GuildRecord::id`].
    pub other: u32,
    /// At war, rather than allied.
    pub at_war: bool,
}

/// A guild, as saved.
///
/// The relations are written on **both** guilds, which is how they are held in
/// memory: a war stored on one side only would make the colour a mobile draws in
/// depend on which of the two a client happened to ask about. Reading them back
/// is therefore idempotent rather than additive — each side restores its own
/// list, and the two agree because both were written.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GuildRecord {
    /// Its id, which is the key every member record names it by. Never reused —
    /// see [`WorldRecord::guild_high_water`].
    pub id: u32,
    /// What it calls itself.
    pub name: String,
    /// The short form drawn in brackets after a member's name.
    pub abbreviation: String,
    /// Who leads it, by serial. A serial and not an entity: an entity id does not
    /// survive a restart, and the leader is the one member a guild cannot lose
    /// track of.
    #[serde(with = "serial")]
    pub leader: Serial,
    /// Every war and alliance it has declared.
    #[serde(default)]
    pub relations: Vec<GuildStanding>,
    /// Every one it has offered and the other has not yet matched. Saved because
    /// a declaration is *half* of a war, and losing it at a restart would quietly
    /// undo it — the other guild's leader would answer a declaration that no
    /// longer existed and start a fresh one nobody had answered.
    #[serde(default)]
    pub proposals: Vec<GuildStanding>,
    /// Which alliance it belongs to, by [`AllianceRecord::id`]. `None` for most
    /// guilds.
    #[serde(default)]
    pub alliance: Option<u32>,
}

/// A named alliance, as saved.
///
/// # Why the membership is written here and not on the guilds
///
/// Unlike a war — which is written on **both** guilds so that neither side can
/// be the only one that knows — an alliance has a body of its own to be written
/// on, and one list is one thing to keep in step instead of N. The guild's
/// [`alliance`](GuildRecord::alliance) is a back-pointer for the lookup, and a
/// guild naming an alliance whose record is gone reads as no alliance, exactly
/// as a membership naming a disbanded guild does.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AllianceRecord {
    /// Its id, never reused — see [`WorldRecord::alliance_high_water`].
    pub id: u32,
    /// What it calls itself.
    pub name: String,
    /// Which member guild leads it, by [`GuildRecord::id`].
    pub leader: u32,
    /// Every guild in it, the leader included.
    pub members: Vec<u32>,
    /// Every guild asked in that has not answered. Saved for the same reason a
    /// war declaration is: it is half of a decision, and losing it at a restart
    /// would quietly undo the asking.
    #[serde(default)]
    pub pending: Vec<u32>,
}

/// A quest in progress, as saved.
///
/// `progress` and `seconds` run parallel to the *definition's* objective list, so
/// this is only meaningful next to the pack's definition of `key`. That is the
/// same bargain ServUO's own save makes (it matches objectives positionally
/// against a freshly constructed quest), and it means adding an objective to the
/// end of an existing quest is safe while reordering is not.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct QuestRecord {
    /// Which quest, by the pack's key. A key the pack no longer defines is
    /// dropped on load rather than failing the character.
    pub key: String,
    /// How far each objective has got.
    pub progress: Vec<u16>,
    /// Seconds left on each timed objective; `0` on the untimed ones. A *remaining
    /// span*, not a deadline: the tick counter starts again from zero at boot, so
    /// a saved absolute tick would mean something different every restart — the
    /// same rule [`EffectRecord::remaining`] follows.
    #[serde(default)]
    pub seconds: Vec<u32>,
    /// Whether a timer ran out on it.
    #[serde(default)]
    pub failed: bool,
    /// The serial of the NPC that gave it, if it is still known.
    #[serde(default, with = "optional_serial")]
    pub giver: Option<Serial>,
}

/// A finished quest and its cooldown.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DoneQuestRecord {
    /// Which quest, by the pack's key.
    pub key: String,
    /// Seconds until it may be taken again — again a remaining span, not a
    /// deadline. [`u32::MAX`] means never (a once-only quest).
    pub restart_in_secs: u32,
}

/// A timed effect on a mobile that a relog must not wash off — poison today,
/// buffs and debuffs (bless, curse, a stat drain) as they land. Deliberately one
/// shape for all of them, so a new effect kind rides the same list and column
/// with no schema change: a `kind` tag, its `amount` (a poison level, a stat
/// offset), and how much is `remaining` (poison pulses, or a buff's seconds).
/// The remaining *count* is stored, not a tick (which resets to zero at boot) —
/// the effect re-derives its next fire from "now" on restore, the way a
/// spawner's remaining wait does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EffectRecord {
    /// What kind of effect: `0` poison, and future buffs/debuffs beyond it.
    pub kind: u8,
    /// Its magnitude — a poison level, or a stat offset (signed for a debuff).
    pub amount: i16,
    /// How much it has left — poison pulses, or a timed buff's seconds.
    pub remaining: u16,
}

/// The effect kind for poison — the first, and the pattern the rest follow.
pub const EFFECT_POISON: u8 = 0;

/// One skill a character has, as saved: which, how far trained, and the arrow
/// the player set it to train by.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SkillRecord {
    /// The skill id, zero-based.
    pub id: u8,
    /// The trained value, in tenths.
    pub value: u16,
    /// The lock arrow as its wire byte (0 up, 1 down, 2 locked).
    pub lock: u8,
    /// The ceiling on this skill, in tenths. Defaulted so a pre-v16 save reads as
    /// the ordinary 100.0 rather than as a skill capped at nothing.
    #[serde(default = "default_skill_cap")]
    pub cap: u16,
}

/// The cap a pre-v16 save (which stored none) loads each skill with — the classic
/// 100.0.
fn default_skill_cap() -> u16 {
    1000
}

/// Which way a character's three stats are set to train, and when each last rose.
///
/// Saved together because the gain reads them together, and because all four
/// numbers are worthless apart: an arrow with no timestamp lets a relog pour
/// points in, and a timestamp with no arrow has nothing to gate.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct StatLockRecord {
    /// Strength's arrow as its wire bits (0 up, 1 down, 2 locked).
    pub strength: u8,
    /// Dexterity's arrow.
    pub dexterity: u8,
    /// Intelligence's arrow.
    pub intelligence: u8,
    /// How many ticks ago strength last rose. Stored as an *age* and not as the
    /// absolute tick it happened on, because the tick counter restarts with the
    /// shard: an absolute stamp from the last run would sit in the future of this
    /// one and freeze the stat for ever.
    pub strength_age: u64,
    /// The same for dexterity.
    pub dexterity_age: u64,
    /// The same for intelligence.
    pub intelligence_age: u64,
}

/// The stat a pre-v6 save (which stored none) loads with — the flat hundred the
/// world handed out before stats were persisted.
fn default_stat() -> u16 {
    100
}

/// Where an item is, as saved. An item is in exactly one of three places, the
/// same three the live `Position`/`Contained`/`Equipped` components model — never
/// more than one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ItemLocation {
    /// Loose on the ground, at a world tile on a facet.
    Ground {
        /// Which facet.
        facet: u8,
        /// Where.
        x: u16,
        /// Where.
        y: u16,
        /// How high. Signed: UO has basements.
        z: i8,
    },
    /// Inside a container, by the container's serial and the slot in its gump.
    Contained {
        /// The container it is in, by serial.
        #[serde(with = "serial")]
        container: Serial,
        /// Column in the gump.
        x: u16,
        /// Row in the gump.
        y: u16,
        /// Slot in the grid view.
        grid: u8,
    },
    /// Worn on a mobile, at a layer.
    Equipped {
        /// The wearer's serial.
        #[serde(with = "serial")]
        mobile: Serial,
        /// The equipment layer.
        layer: u8,
    },
}

/// An item, as saved.
///
/// # Why the serial is here, like a character's
///
/// An item's serial is what a container's contents point at and what a worn item
/// is drawn under, so it is stable across restarts for the same reason a
/// character's is: change it and every reference to the old one dangles. It is
/// saved and reserved on the way back in.
///
/// `owner` is the character whose inventory this belongs to, or `None` for a
/// loose ground item that belongs to no one (`0` on disk) — the key a store
/// replaces a whole inventory by. `container_gump` is `Some` when the item is
/// *itself* a container, carrying the window the client opens for it, so a bag
/// inside a bag comes back openable.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ItemRecord {
    /// The wire serial. Stable across restarts; see the type docs.
    #[serde(with = "serial")]
    pub serial: Serial,
    /// The character whose inventory this is in, or `None` for a ground item.
    #[serde(with = "optional_serial")]
    pub owner: Option<Serial>,
    /// The item graphic.
    pub graphic: u16,
    /// The item hue.
    pub hue: u16,
    /// The stack amount; `1` for a single item. For a corpse marker, its body
    /// graphic instead — the historical on-disk representation of `CorpseBody`.
    pub amount: u16,
    /// Whether it stacks — a pile of gold merges with another, a sword does not.
    /// Saved so a restored pile still stacks; without it a lone gold coin would
    /// stop merging until re-lifted.
    pub stackable: bool,
    /// The container gump if this item is itself a container, else `None`.
    pub container_gump: Option<u16>,
    /// What one unit costs at a vendor, if this is priced stock. Defaulted so a
    /// v4 save loads.
    #[serde(default)]
    pub price: Option<u32>,
    /// The item's label, if it carries one — vendor stock names its wares.
    /// Defaulted so a v4 save loads.
    #[serde(default)]
    pub name: Option<String>,
    /// The learned-spell bitmask if this item is a spellbook, else `None`. Saved
    /// so a bought book still opens to its spells after a relog; without it a
    /// restored spellbook is a graphic with no `Spellbook` component and refuses
    /// to open. Defaulted so a pre-v8 save loads.
    #[serde(default)]
    pub spellbook: Option<u64>,
    /// How this corpse came to be one, if it is a corpse — what Forensic
    /// Evaluation reads. `None` for every other item, and defaulted so a pre-v17
    /// save loads.
    #[serde(default)]
    pub corpse: Option<CorpseData>,
    /// The poison on it, if any — `(level, charges)`. A bottled dose or a coating
    /// the Poisoning skill put on a blade; `None` for a clean item. Defaulted so a
    /// pre-v18 save loads.
    #[serde(default)]
    pub poison: Option<(u8, u16)>,
    /// The trap on it, if it is a trapped container.
    /// `None` for everything else, and defaulted so a pre-v19 save loads.
    #[serde(default, with = "trap")]
    pub trap: Option<TrapRecord>,
    /// How many uses are left in it, if it is a thing that wears out — a
    /// harvesting tool's swings or an instrument's tunes. One field for both,
    /// because ServUO gives both the one `IUsesRemaining` interface, and the
    /// *graphic* decides which component it comes back as. Defaulted so a pre-v20
    /// save loads.
    #[serde(default)]
    pub uses: Option<u16>,
    /// Whether it came out of a craft exceptional, and whose name is on it —
    /// `(exceptional, maker)`. `None` for everything a player did not make, which
    /// is nearly every item on a shard, so the column is empty far more often
    /// than not. Defaulted so a pre-v21 save loads.
    ///
    /// The maker is a **name and not a serial**, for the reason the corpse's
    /// killer is one: the smith logs out and the sword outlives the session.
    #[serde(default)]
    pub crafted: Option<(bool, Option<String>)>,
    /// Where a recall rune points — `(facet, x, y, z)`. `None` for a blank rune
    /// and for everything that is not one, and defaulted so a pre-v22 save
    /// loads.
    ///
    /// The absence *is* "unmarked": the world has no `marked` flag either, so
    /// there is no pair of halves to keep in step.
    #[serde(default)]
    pub rune: Option<(u8, u16, u16, i8)>,
    /// What a runebook holds. `None` for everything that is not one. One column
    /// for the whole book — the [`CorpseData`] shape — because its entries are a
    /// list and a list does not become sixteen columns.
    #[serde(default)]
    pub runebook: Option<RunebookData>,
    /// The house this item is locked down in, and the access level if it is a
    /// secure. `None` for everything loose, which is nearly everything.
    ///
    /// On the *item* rather than as a list on the house, mirroring the world's
    /// own `LockedDown` component, and for its reason: a lockdown is asked about
    /// one at a time, by a lift that already has the item in hand.
    #[serde(default)]
    pub locked_down: Option<LockdownData>,
    /// Where it is.
    pub location: ItemLocation,
}

/// An item pinned inside a house, as saved — a plain mirror of the world's
/// `LockedDown` component.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LockdownData {
    /// Which house, by its item serial.
    #[serde(with = "serial")]
    pub house: Serial,
    /// The least standing that may open it, if this is a secure container, as
    /// `Standing::code`. `None` for a plain lockdown.
    ///
    /// Numbered by hand at the world's end, like the pet's standing order and
    /// the effect kinds, so the on-disk meaning cannot drift when the enum is
    /// reordered — and it *is* ordered on purpose, which makes reordering it a
    /// live possibility rather than a hypothetical one.
    #[serde(default)]
    pub secure: Option<u8>,
}

/// A runebook's contents, as saved.
///
/// `next_use` is deliberately absent: it is the couple of seconds ServUO makes a
/// book rest between openings, and a restart that re-arms it at zero errs in the
/// player's favour over a column that would be stale by the time it was read.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RunebookData {
    /// The destinations bound, in order.
    pub entries: Vec<RunebookEntryData>,
    /// Charges left.
    pub charges: u8,
    /// The ceiling recharging fills to.
    pub max_charges: u8,
    /// Which entry is the default, if any.
    #[serde(default)]
    pub default_entry: Option<u8>,
}

/// One bound destination, as saved.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RunebookEntryData {
    /// Which facet it is on.
    pub facet: u8,
    /// East-west tile.
    pub x: u16,
    /// North-south tile.
    pub y: u16,
    /// Height.
    pub z: i8,
    /// What to call it in the window.
    pub description: String,
}

/// A pet's ownership and standing order, as saved — a plain mirror of the world's
/// `Pet` component.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PetData {
    /// Whose it is, by wire serial.
    #[serde(with = "serial")]
    pub owner: Serial,
    /// How many follower slots it fills.
    pub slots: FollowerSlots,
    /// What it was last told: 0 follow, 1 come, 2 stay, 3 guard, 4 attack, 5 stop.
    /// Numbered by hand, like the effect kinds, so the on-disk meaning cannot drift
    /// when the enum gains a variant.
    pub order: u8,
    /// Whom that order was about, for an attack.
    #[serde(default, with = "optional_serial")]
    pub order_target: Option<Serial>,
}

/// A corpse's story, as saved — a mirror of the world's `Corpse` component, plus
/// the one thing about the corpse's *picture* the item row has no column for.
///
/// Saved because a corpse lies for seven minutes and a shard restarts inside that
/// window: without it, the body of the character somebody was investigating comes
/// back anonymous, killed by nobody, disturbed by no one.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct CorpseData {
    /// Who this was.
    pub owner: String,
    /// Who struck the killing blow, by name.
    #[serde(default)]
    pub killer: Option<String>,
    /// The first forensicist to read it.
    #[serde(default)]
    pub examined_by: Option<String>,
    /// Everyone who has taken something off it.
    #[serde(default)]
    pub looters: Vec<String>,
    /// Which way it fell, as the wire's direction byte — `0` north, running
    /// clockwise, exactly `Direction::to_bits`.
    ///
    /// The other half of the picture, the body graphic, rides [`ItemRecord::amount`]
    /// like a stack size; this half rides here because the item row has nowhere
    /// else to put it and this column already exists on precisely the rows that
    /// need it. A save written before facings were saved has no field, and its
    /// corpses come back lying north — the same thing every corpse on the shard
    /// did before this was carried at all.
    #[serde(default)]
    pub facing: u8,
}

/// One creature kind a spawn region may put down, as saved — a plain mirror of
/// the world's creature template, kept here so the on-disk shape does not move
/// every time the simulation's does.
/// The serde default for [`CreatureData::aggression`]: aggressive.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CreatureData {
    /// The body graphic.
    pub body: u16,
    /// Its hue.
    pub hue: u16,
    /// Starting and maximum hit points.
    pub hits: u16,
    /// Health-bar colour, the notoriety wire value.
    pub notoriety: Notoriety,
    /// Melee damage before resistance.
    pub damage: u16,
    /// Physical resistance, a percentage.
    pub resistance: PhysicalResistance,
    /// How widely known it is — what its killer inherits. Defaulted, so an older
    /// saved region restores creatures that give up no standing.
    #[serde(default)]
    pub fame: i32,
    /// Which way it is known. Negative is evil.
    #[serde(default)]
    pub karma: i32,
    /// Swing cadence in ticks; `0` derives it from dexterity.
    pub swing: u64,
    /// How far it notices a target.
    pub sight: Sight,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    /// Defaults to aggressive, the only behaviour that existed before it.
    #[serde(default)]
    pub aggression: Aggression,
    /// Ticks between its beats while hunting; 0 takes the shard default.
    #[serde(default)]
    pub beat: u64,
    /// Its optional ranged attack reach. JSON `0` means no ranged attack.
    #[serde(default, with = "openshard_protocol::world::ranged")]
    pub ranged: Option<RangedRange>,
    /// The ranged attack's damage type.
    #[serde(default)]
    pub ranged_kind: DamageType,
    /// Whether it drifts when idle.
    pub wander: bool,
    /// Trained combat skills, `(skill id, value in tenths)`. Defaulted so an older
    /// save (no skills) restores as a skill-less creature.
    #[serde(default)]
    pub skills: Vec<(u8, u16)>,
}

/// A spawn region, as saved.
///
/// # Why the timer is *remaining seconds*, not a wall-clock time
///
/// The requirement is that a rare spawn killed shortly before a restart comes back
/// with the same wait ahead of it — killed with five hours left, five hours left
/// on load, whatever the shard was down for. So the timer is stored as the seconds
/// still to wait, not an absolute time: on load it counts down from there, and
/// downtime does not eat into it. Seconds, not ticks, so it survives the tick
/// counter resetting to zero at boot.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SpawnerRecord {
    /// Its stable id, the key it is replaced by.
    pub id: u32,
    /// Which facet.
    pub facet: u8,
    /// The region's north-west corner and size.
    pub x: u16,
    /// North-west corner y.
    pub y: u16,
    /// Region width.
    pub width: u16,
    /// Region height.
    pub height: u16,
    /// The most live creatures it keeps.
    pub max_count: u16,
    /// The respawn delay, in seconds.
    pub respawn_secs: u64,
    /// Seconds still to wait before the next spawn; `0` is ready now.
    pub remaining_secs: u64,
    /// The creatures it may put down.
    pub creatures: Vec<CreatureData>,
}

/// An NPC mobile, as saved — the townsperson, the vendor, the creature on the
/// ground. The Sphere/ServUO model: every live mobile is persisted, so a restart
/// restores the world exactly (a wounded rare comes back wounded; a killed one
/// stays gone, its region timer counting down). Deliberately a sibling of
/// [`CharacterRecord`], not a variant of it: an NPC has no account, and the two
/// change for different reasons.
///
/// Its worn gear and vendor stock ride the same [`Inventory`]/[`ItemRecord`]
/// machinery a character's do, keyed by this serial.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MobileRecord {
    /// The wire serial. Stable across restarts, like a character's.
    #[serde(with = "serial")]
    pub serial: Serial,
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
    /// Its name, if it has one — a named townsperson; `None` for a beast.
    pub name: Option<String>,
    /// Hit points as they stood at the save — a wounded creature stays wounded.
    pub hits_current: u16,
    /// Maximum hit points.
    pub hits_max: u16,
    /// Health-bar colour, the notoriety wire value.
    pub notoriety: Notoriety,
    /// Melee damage before resistance.
    pub damage: u16,
    /// Physical resistance, a percentage.
    pub resistance: PhysicalResistance,
    /// Swing cadence in ticks; `0` derives it from dexterity.
    pub swing: u64,
    /// How far it notices a target; `0` never picks a fight (and no brain).
    pub sight: Sight,
    /// Whether it starts fights (2), answers them (1), or only runs (0).
    pub aggression: Aggression,
    /// Ticks between its beats while hunting; 0 takes the shard default.
    pub beat: u64,
    /// Its optional ranged attack reach. JSON `0` means no ranged attack.
    #[serde(with = "openshard_protocol::world::ranged")]
    pub ranged: Option<RangedRange>,
    /// The ranged attack's damage type.
    pub ranged_kind: DamageType,
    /// Whether it drifts when idle.
    pub wander: bool,
    /// Whether it offers banking.
    pub banker: bool,
    /// Whether it keeps a shop.
    pub vendor: bool,
    /// Whether it offers a free resurrection to a ghost that comes near or
    /// double-clicks it. Defaulted, like `pet` and `night_home`, so an older save
    /// loads with no healers rather than failing to parse.
    #[serde(default)]
    pub healer: bool,
    /// The trade it plies, ServUO-style ("the blacksmith"). `None` for a creature.
    ///
    /// Saved because it is a *key*, not decoration: the speech table an NPC answers
    /// from is looked up by it on every word spoken nearby. Its generated outfit and
    /// name need it only once, at spawn, and those are already saved as worn items
    /// and a `Name` — this is what keeps it answering after a reboot.
    #[serde(default)]
    pub title: Option<String>,
    /// Whose pet it is and what it was told, if it is one — `(owner serial, slots,
    /// order, order target)`. A mobile record is a JSON blob, so this needs no
    /// column and no schema bump; it is defaulted, so an older save loads as a
    /// world of wild animals.
    ///
    /// Saved because a tamed creature is *property*: a restart that quietly
    /// released every pet on the shard is the `Murders` lesson again, and this time
    /// it would be released property somebody spent an hour taming.
    #[serde(default)]
    pub pet: Option<PetData>,
    /// A townsperson's post `(x, y, z)`, if it keeps one.
    pub npc_home: Option<(u16, u16, i8)>,
    /// Where it sleeps, for the optional daily routine. `None` on every NPC unless
    /// the pack gave it one, and read only when `gameplay.npc_schedule` is on.
    #[serde(default)]
    pub night_home: Option<(u16, u16, i8)>,
    /// A vendor's shelf when full, and the seconds until it next refills.
    ///
    /// The seconds and not a tick count, for the reason `SpawnerRecord` states: a
    /// tick counter restarts at boot, so a saved tick would come back either already
    /// due or an hour early.
    #[serde(default)]
    pub restock: Option<RestockRecord>,
    /// How far from its post a townsperson drifts; meaningful with `npc_home`.
    pub npc_wander: u8,
    /// The spawn region that maintains it, if one does — restored so the region
    /// counts it and does not spawn over it.
    ///
    /// Not a [`Serial`]: it is `SpawnedBy`'s index into the world's spawner
    /// list (`SpawnerRecord::id`), a namespace of its own that starts at `0` —
    /// a value `Serial::new` would reject outright. Converting this field
    /// alongside the others in the sweep was the wrong call; it stays a bare
    /// `u32`.
    pub spawned_by: Option<u32>,
    /// Every timed effect working through it — poison and the rest.
    #[serde(default)]
    pub effects: Vec<EffectRecord>,
    /// Trained combat skills, `(skill id, value in tenths)`. Defaulted so an older
    /// save restores a skill-less creature.
    #[serde(default)]
    pub skills: Vec<(u8, u16)>,
    /// The quests this NPC offers, by key. Empty for an ordinary mobile.
    ///
    /// Saved because the binding has nowhere else to live that survives: the
    /// script that placed the NPC only knows it is a giver during the run that
    /// placed it, so before this the shard's quests worked exactly once — on the
    /// boot where the world was populated — and every restart afterwards left a
    /// town full of NPCs that answered nothing, with no error anywhere to say so.
    #[serde(default)]
    pub quest_giver: Vec<String>,
    /// The region this NPC asks to be escorted to, if it is escortable. `None` for
    /// an ordinary mobile; an empty string means "wherever the quest decides",
    /// chosen when someone accepts.
    #[serde(default)]
    pub escort_destination: Option<String>,
}

/// A vendor's shelf when full, as saved. See [`MobileRecord::restock`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RestockRecord {
    /// How many seconds until the shelf next refills.
    pub in_seconds: u64,
    /// The full shelf: `(graphic, hue, amount, price, name)` per line.
    pub lines: Vec<(u16, u16, u16, u32, String)>,
}

/// A shut-and-openable door's live state, inside a [`DecorationRecord`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DoorState {
    /// The graphic it shows shut.
    pub closed_graphic: u16,
    /// The graphic it shows open.
    pub open_graphic: u16,
    /// How far the leaf swings east-west when opened.
    pub offset_x: i16,
    /// How far the leaf swings north-south when opened.
    pub offset_y: i16,
    /// Whether it stood open at the save — a door left open stays open.
    pub is_open: bool,
}

/// A placed decoration, as saved — the statics, doors and town containers a pack
/// lays over the map's art. Saved like everything else in the world; the pack is
/// the *seed* (a staff Populate/Decorate, once), the save is the truth thereafter.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DecorationRecord {
    /// The wire serial. Stable across restarts.
    #[serde(with = "serial")]
    pub serial: Serial,
    /// The graphic as it stands now (a door's current leaf).
    pub graphic: u16,
    /// Its hue.
    pub hue: u16,
    /// Which facet.
    pub facet: u8,
    /// Where it stands.
    pub x: u16,
    /// Where it stands.
    pub y: u16,
    /// How high. Signed: UO has basements.
    pub z: i8,
    /// Door state, if this decoration is a door.
    pub door: Option<DoorState>,
    /// The container gump if this decoration opens as one, else `None`.
    pub container_gump: Option<u16>,
    /// Which key opens it; `0` is unlocked. On the record rather than inside
    /// [`DoorState`] because a container locks too, and a lock is the same thing on
    /// either — ServUO's `ILockable`. Defaulted, so an older save reads as unlocked.
    #[serde(default)]
    pub key_value: u32,
}

/// One named area of a facet, as saved.
///
/// Regions are pure data, registered from `state/data/regions.json` when the
/// `regions:` verb is pressed — so saving them looks redundant, until a restart,
/// when nothing re-registers them until a game master clicks `.admin` again and
/// a shard silently loses its guards, its music and the dark in its dungeons.
/// The save is the truth here as everywhere else.
///
/// The rectangles ride as JSON for the same reason a spawner's creature list
/// does: a region holds a handful, and a table of them would be a join for no
/// gain.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RegionRecord {
    /// Which facet it belongs to.
    pub facet: u8,
    /// Its index on that facet, which is its id.
    pub id: u16,
    /// What the place is called.
    pub name: String,
    /// Which region wins where two overlap.
    pub priority: u8,
    /// Its boxes: `(x, y, width, height, z_min, z_max)`.
    pub rects: Vec<(u16, u16, u16, u16, i8, i8)>,
    /// Guards answer a call here.
    pub guarded: bool,
    /// No teleporting in, out or within.
    pub no_teleport: bool,
    /// No Recall or Gate.
    pub no_recall: bool,
    /// No house may be placed.
    pub no_housing: bool,
    /// No player may harm another.
    pub safe: bool,
    /// The client music track, as a `MusicName` index.
    pub music: Option<u16>,
    /// The light level inside, overriding the hour.
    pub light: Option<u8>,
}

/// The world's own scalars, as saved — one row, not one per anything.
///
/// Two things a restart cannot re-derive. The clock, because the tick counter
/// resets at boot by design (every restored timer is an offset from zero), so
/// without it every night starts over. And the roll generator's position, because
/// the alternative is not "slightly different rolls" but *the same rolls again*:
/// a shard that re-seeds at boot replays the sequence it played last run, which a
/// player can notice and then farm.
///
/// Both default to zero, which is a real value for each: midnight, and a seed the
/// generator replaces with its own non-zero constant — a store that has never
/// been written reads as a fresh world.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WorldRecord {
    /// The world clock, in UO minutes.
    pub clock_minutes: u64,
    /// Where the world's roll generator had got to. `openshard_state::rng::Rng`'s
    /// whole state, and the reason it is one `u64` rather than a seed plus a
    /// count: `xorshift64*` *is* its state, so resuming needs nothing else.
    ///
    /// Spans every `u64`, high bit included, while both stores keep it in a signed
    /// 64-bit column — the sign is reinterpreted, never clamped. See
    /// `SqliteStore::save`.
    #[serde(default)]
    pub rng_state: u64,
    /// The highest guild id ever handed out.
    ///
    /// Here rather than derived from the saved guilds at boot, and that is the
    /// whole point of the field: the maximum id *in the table* is not the maximum
    /// id ever issued, because a disbanded guild leaves no row. A shard that
    /// re-derived it would hand the next guild founded an id a disbanded one had
    /// used, and every character record still naming that id — a member who was
    /// offline when it disbanded, and so was never swept — would silently find
    /// itself in the new guild.
    #[serde(default)]
    pub guild_high_water: u32,
    /// The same for alliances, and for the same reason: a guild record names its
    /// alliance by id, and a restart that reissued one would put a guild into a
    /// body it never joined.
    #[serde(default)]
    pub alliance_high_water: u32,
}

/// A character's whole carried inventory, replaced as a unit.
///
/// A store saves a character's items by replacing everything under its `owner`
/// rather than tracking each item's comings and goings — see
/// [`crate::journal`]. `items` is every worn and contained item, at every depth.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Inventory {
    /// The character serial these items belong to.
    #[serde(with = "serial")]
    pub owner: Serial,
    /// Every item worn or contained under that character, at any nesting depth.
    pub items: Vec<ItemRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sight_keeps_its_numeric_saved_representation() {
        let json = serde_json::to_string(&Sight(8)).expect("sight serialises");
        assert_eq!(json, "8", "a named sight value must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<Sight>(&json).expect("sight deserialises"),
            Sight(8)
        );
    }

    #[test]
    fn physical_resistance_keeps_its_numeric_saved_representation() {
        let json = serde_json::to_string(&PhysicalResistance::new(35)).expect("resistance serialises");
        assert_eq!(json, "35", "a named resistance must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<PhysicalResistance>("255").expect("legacy resistance deserialises"),
            PhysicalResistance::new(100),
            "out-of-range legacy input keeps the runtime's historic 100% cap"
        );
    }

    #[test]
    fn poison_level_keeps_its_numeric_saved_representation() {
        let json = serde_json::to_string(&PoisonLevel::new(3)).expect("poison level serialises");
        assert_eq!(json, "3", "a named poison level must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<PoisonLevel>("255").expect("legacy poison level deserialises"),
            PoisonLevel::LETHAL,
            "out-of-range legacy input keeps the runtime's historic lethal cap"
        );
    }

    #[test]
    fn follower_slots_keep_their_numeric_saved_representation() {
        let json = serde_json::to_string(&FollowerSlots::new(3)).expect("follower slots serialise");
        assert_eq!(json, "3", "a named slot cost must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<FollowerSlots>("0").expect("legacy follower slots deserialise"),
            FollowerSlots::ONE,
            "a legacy zero keeps the runtime's historic minimum of one slot"
        );
    }

    #[test]
    fn notoriety_keeps_its_numeric_saved_representation() {
        let json = serde_json::to_string(&Notoriety::Enemy).expect("notoriety serialises");
        assert_eq!(json, "5", "a named notoriety value must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<Notoriety>("0").expect("an unset notoriety deserialises"),
            Notoriety::Innocent,
            "the persisted seam retains the protocol's safe default for unknown bytes"
        );
    }

    #[test]
    fn aggression_keeps_its_numeric_saved_representation() {
        let json = serde_json::to_string(&Aggression::Defensive).expect("aggression serialises");
        assert_eq!(json, "1", "a named aggression value must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<Aggression>("255").expect("an unknown aggression deserialises"),
            Aggression::Aggressive,
            "the persisted seam retains the longstanding aggressive fallback"
        );
    }

    #[test]
    fn damage_type_keeps_its_numeric_saved_representation() {
        let json = serde_json::to_string(&DamageType::Energy).expect("damage type serialises");
        assert_eq!(json, "4", "a named damage type must not change saved JSON");
        assert_eq!(
            serde_json::from_str::<DamageType>("255").expect("an unknown damage type deserialises"),
            DamageType::Physical,
            "the persisted seam retains the longstanding physical fallback"
        );
    }

    #[test]
    fn ranged_reach_keeps_its_numeric_saved_representation() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct RangedSeam {
            #[serde(with = "openshard_protocol::world::ranged")]
            ranged: Option<RangedRange>,
        }

        let melee = serde_json::to_string(&RangedSeam { ranged: None }).expect("melee range serialises");
        assert_eq!(melee, r#"{"ranged":0}"#, "no range must remain the legacy zero");

        let archer = RangedSeam {
            ranged: RangedRange::new(8),
        };
        let json = serde_json::to_string(&archer).expect("ranged reach serialises");
        assert_eq!(
            json, r#"{"ranged":8}"#,
            "a range must remain its numeric saved value"
        );
        assert_eq!(
            serde_json::from_str::<RangedSeam>(&melee)
                .expect("the legacy zero deserialises")
                .ranged,
            None
        );
        assert_eq!(
            serde_json::from_str::<RangedSeam>(&json)
                .expect("a numeric range deserialises")
                .ranged,
            RangedRange::new(8)
        );
    }

    #[test]
    fn an_item_record_round_trips_through_json() {
        // Every field reachable by name from outside — a skipped field comes back
        // as its default, and an item that loads with a default location is on the
        // ground at 0,0 instead of in the pack it was saved in.
        for location in [
            ItemLocation::Ground {
                facet: 0,
                x: 1400,
                y: 1600,
                z: -5,
            },
            ItemLocation::Contained {
                container: Serial::new(0x4000_0001).unwrap(),
                x: 40,
                y: 65,
                grid: 3,
            },
            ItemLocation::Equipped {
                mobile: Serial::new(0x0000_0001).unwrap(),
                layer: 0x15,
            },
        ] {
            let record = ItemRecord {
                serial: Serial::new(0x4000_0002).unwrap(),
                owner: Some(Serial::new(0x0000_0001).unwrap()),
                graphic: 0x0E75,
                hue: 0,
                amount: 1,
                stackable: false,
                container_gump: Some(0x003C),
                price: Some(11),
                name: Some("scissors".into()),
                spellbook: Some(0x0000_0000_00FF_00FF),
                corpse: Some(CorpseData {
                    owner: "Reginald".into(),
                    killer: Some("an orc".into()),
                    examined_by: Some("Mordred".into()),
                    looters: vec!["Vesper".into()],
                    facing: 6,
                }),
                poison: Some((2, 14)),
                trap: Some(TrapRecord {
                    kind: 3,
                    power: 40,
                    level: 2,
                }),
                uses: Some(37),
                crafted: Some((true, Some("Rowena".into()))),
                rune: Some((0, 1495, 1629, -20)),
                locked_down: Some(LockdownData {
                    house: Serial::new(0x4000_0001).unwrap(),
                    secure: Some(3),
                }),
                runebook: Some(RunebookData {
                    entries: vec![RunebookEntryData {
                        facet: 0,
                        x: 1336,
                        y: 1997,
                        z: 5,
                        description: "Britain".into(),
                    }],
                    charges: 4,
                    max_charges: 10,
                    default_entry: Some(0),
                }),
                location,
            };
            let json = serde_json::to_string(&record).expect("an item must serialise");
            assert!(
                json.contains("\"trap\":[3,40,2]"),
                "a named Rust trap keeps the pre-existing JSON array on disk"
            );
            let back: ItemRecord = serde_json::from_str(&json).expect("and come back");
            assert_eq!(back, record);
        }
    }

    #[test]
    fn a_character_record_round_trips_through_json() {
        // Not a test of serde. A test that every field is reachable by name from
        // outside the crate: a field that is private, skipped, or renamed by
        // accident is a field that comes back as its default, and a character
        // that loads with a default position is standing in the ocean.
        let record = CharacterRecord {
            serial: Serial::new(0x0000_0001).unwrap(),
            account: AccountName::new("admin"),
            name: CharacterName::new("Alpha"),
            body: 0x0190,
            hue: 0,
            facet: 0,
            x: 1363,
            y: 1600,
            z: 30,
            facing: 3,
            strength: 55,
            dexterity: 40,
            intelligence: 25,
            skills: vec![
                SkillRecord {
                    id: 25, // Magery
                    value: 501,
                    lock: 1, // down
                    cap: 1000,
                },
                SkillRecord {
                    id: 45, // Mining
                    value: 300,
                    lock: 0,
                    cap: 1200, // a raised cap: the field has to survive the trip
                },
            ],
            effects: vec![EffectRecord {
                kind: EFFECT_POISON,
                amount: 2,
                remaining: 5,
            }],
            dead: true,
            fame: 0,
            karma: 0,
            murders: 0,
            quests: vec![QuestRecord {
                key: "rat_cull".into(),
                progress: vec![3],
                seconds: vec![0],
                failed: false,
                giver: Some(Serial::new(0x4000_0001).unwrap()),
            }],
            done_quests: vec![DoneQuestRecord {
                key: "silk_gather".into(),
                restart_in_secs: 3600,
            }],
            stat_locks: StatLockRecord {
                strength: 0,     // up
                dexterity: 1,    // down
                intelligence: 2, // locked
                strength_age: 40,
                dexterity_age: 0,
                intelligence_age: 900,
            },
            // A member with a title, and an invitation from a second guild that
            // has not been answered. Both non-default, which is the point of the
            // test: a field that comes back as its default is a field nobody
            // saved.
            guild: Some(7),
            guild_title: "Warlord".to_owned(),
            guild_rank: 0,
            guild_candidate: Some(9),
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
            serial: Serial::new(1).unwrap(),
            account: AccountName::new("admin"),
            name: CharacterName::new("Alpha"),
            body: 0x0190,
            hue: 0,
            facet: 0,
            x: 5000,
            y: 500,
            z: -40,
            facing: 0,
            strength: 100,
            dexterity: 100,
            intelligence: 100,
            skills: Vec::new(),
            effects: Vec::new(),
            dead: false,
            fame: 0,
            karma: 0,
            murders: 0,
            quests: Vec::new(),
            done_quests: Vec::new(),
            guild: None,
            guild_title: String::new(),
            guild_rank: 0,
            guild_candidate: None,
            stat_locks: StatLockRecord::default(),
        };
        let json = serde_json::to_string(&record).expect("a record must serialise");
        let back: CharacterRecord = serde_json::from_str(&json).expect("and come back");
        assert_eq!(back.z, -40);
    }
}
