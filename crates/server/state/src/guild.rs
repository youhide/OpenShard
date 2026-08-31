//! Guilds: who belongs to one, and how two of them regard each other.
//!
//! Shared substrate, not rules. This is the membership and the relations the
//! packet path has to read; the *system* — founding a guild, invitations,
//! declaring a war, the roster gump — is `openshard-guilds` above it, the same
//! split [`Regions`](crate::Regions) and [`Dialogue`](crate::Dialogue) have.
//!
//! # Why it is here and not only in the guilds crate
//!
//! Because a `0x78` has a notoriety byte in it. What colour a mobile draws in
//! depends on who is looking — a guildmate is green, a mobile whose guild you are
//! at war with is orange — so the wire path itself has to be able to ask, and the
//! wire path is [`WorldState`](crate::WorldState)'s. See
//! [`WorldState::notoriety_toward`].
//!
//! # ServUO's rule, and its order
//!
//! `Scripts/Misc/Notoriety.cs`: a murderer is red and a criminal is grey
//! **before** any guild question is asked, and only then does the same guild or an
//! ally read green and a guild at war read orange. Guild colour loses to standing,
//! which is what stops a red hiding inside a guild tabard.

use std::collections::{
    BTreeMap,
    BTreeSet,
};

/// A guild's stable id — the key its members carry and its relations are named
/// by.
///
/// Distinct from every other `u32` in world state: it addresses a [`Guilds`]
/// entry and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct GuildId(pub u32);

/// An alliance's stable id.
///
/// Its own type rather than a [`GuildId`], because an alliance outlives the guild
/// that founded it: ServUO picks a new leader guild when the old one leaves, so
/// the founder's id is not a key the way a party's leader is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct AllianceId(pub u32);

/// A named alliance: several guilds under one banner.
///
/// # This replaced a pairwise "ally"
///
/// Until 2026-08-15 a guild could declare `Relation::Ally` on another, exactly
/// as it declares a war, and being allied was a fact about a *pair*. That was
/// this engine's own simplification and not the reference's, and it had a shape
/// nobody wanted: A allied with B and with C meant B and C were not allied with
/// each other, so "who is in my alliance" had no answer and alliance chat
/// reached a set that depended on who was speaking.
///
/// ServUO's alliance is this — a named object with a member list — so a war is
/// now the only thing two guilds declare at each other, and being allied is
/// membership. See [`Guild::wars`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Alliance {
    /// Its id, which is also the key it is stored under.
    pub id:      AllianceId,
    /// What it calls itself — "The Northern Compact".
    pub name:    String,
    /// Which member guild leads it.
    ///
    /// Replaced rather than dissolved when that guild leaves: ServUO's
    /// `CalculateAllianceLeader` picks another member, and only an alliance with
    /// fewer than two members left disbands.
    pub leader:  GuildId,
    /// Every guild in it, the leader included.
    pub members: BTreeSet<GuildId>,
    /// Every guild asked in that has not answered.
    ///
    /// On the alliance rather than on the guild, unlike a player's guild
    /// invitation: a guild is a thing several people act for, so the question
    /// belongs to the body that asked rather than to whoever happens to read it
    /// first.
    pub pending: BTreeSet<GuildId>,
}

impl Alliance {
    /// Whether `guild` is in it. Pending guilds are not members.
    #[must_use]
    pub fn contains(&self, guild: GuildId) -> bool {
        self.members.contains(&guild)
    }
}

/// Where a member stands inside their guild.
///
/// ServUO's `RankDefinition.Ranks` (`Scripts/Misc/Guild.cs`), the five of the new
/// guild system, in its order. What each rank may *do* is not here — see
/// `openshard_guilds::RankFlags`: this crate has to store which rank a member
/// holds because the component is saved with them, and the rules crate above is
/// the only thing that reads a permission out of it.
///
/// # It is an order, and it is not a permission order
///
/// [`Emissary`](Self::Emissary) may invite, dismiss, promote and set titles;
/// [`Warlord`](Self::Warlord) sits *above* it and may do none of those — it may
/// declare war, which an Emissary may not. Comparing two ranks answers "who
/// outranks whom", which is the question promote, demote and dismiss ask about
/// their target; it never answers "may this one do that".
///
/// # Nobody joins as a Member
///
/// A newcomer is a [`Ronin`](Self::Ronin), which holds no flag at all — not even
/// the vote. ServUO's `Guild.AddMember` is explicit about it (`RankDefinition.Lowest`)
/// and so is the demotion floor: a Ronin cannot be demoted further, only turned
/// out. It is a probationary rank, and a guild that wants a member has to promote
/// one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Rank {
    /// Cliloc `1062963`. In the guild and trusted with nothing yet.
    #[default]
    Ronin,
    /// Cliloc `1062962`. The ordinary member: votes, reaches guild items, and
    /// may turn out a Ronin.
    Member,
    /// Cliloc `1062961`. Recruits, dismisses, promotes and names titles.
    Emissary,
    /// Cliloc `1062960`. Declares and ends wars, and nothing else an ordinary
    /// member cannot do.
    Warlord,
    /// Cliloc `1062959`. Everything, including the alliances.
    Leader,
}

impl Rank {
    /// Every rank, lowest first — the array ServUO indexes by rank number.
    pub const ALL: [Self; 5] = [
        Self::Ronin,
        Self::Member,
        Self::Emissary,
        Self::Warlord,
        Self::Leader,
    ];

    /// Its number, 0 for [`Ronin`](Self::Ronin) through 4 for
    /// [`Leader`](Self::Leader). What is saved, and what the promote and demote
    /// comparisons are written in.
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::Ronin => 0,
            Self::Member => 1,
            Self::Emissary => 2,
            Self::Warlord => 3,
            Self::Leader => 4,
        }
    }

    /// The rank a number names, or `None` past the fifth.
    ///
    /// `None` rather than a clamp: a saved number outside the five is a record
    /// this engine did not write, and reading it as `Leader` because it was large
    /// would hand a guild away.
    #[must_use]
    pub const fn from_number(number: u8) -> Option<Self> {
        match number {
            0 => Some(Self::Ronin),
            1 => Some(Self::Member),
            2 => Some(Self::Emissary),
            3 => Some(Self::Warlord),
            4 => Some(Self::Leader),
            _ => None,
        }
    }

    /// The next rank up, or `None` at [`Leader`](Self::Leader).
    #[must_use]
    pub const fn above(self) -> Option<Self> {
        Self::from_number(self.number() + 1)
    }

    /// The next rank down, or `None` at [`Ronin`](Self::Ronin) — which is the
    /// demotion floor rather than an error to be worked around.
    #[must_use]
    pub const fn below(self) -> Option<Self> {
        match self.number() {
            0 => None,
            number => Self::from_number(number - 1),
        }
    }

    /// The localized string the client draws for this rank.
    #[must_use]
    pub const fn cliloc(self) -> openshard_protocol::wire::ClilocId {
        openshard_protocol::wire::ClilocId(match self {
            Self::Ronin => 1_062_963,
            Self::Member => 1_062_962,
            Self::Emissary => 1_062_961,
            Self::Warlord => 1_062_960,
            Self::Leader => 1_062_959,
        })
    }

    /// The English name, for a system message and for this engine's own gump —
    /// which draws its own text rather than sending cliloc numbers.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ronin => "Ronin",
            Self::Member => "Member",
            Self::Emissary => "Emissary",
            Self::Warlord => "Warlord",
            Self::Leader => "Leader",
        }
    }
}

/// One guild.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Guild {
    /// Its id, which is also the key it is stored under.
    pub id:           GuildId,
    /// What it calls itself — "The Order of the Silver Serpent".
    pub name:         String,
    /// The short form the client draws in brackets after a member's name, three
    /// or four letters by convention: "OSS".
    pub abbreviation: String,
    /// Who leads it. A guild always has one; disbanding is what happens when it
    /// would not.
    pub leader:       openshard_protocol::serial::Serial,
    /// Every guild it is at war with.
    ///
    /// A set and no longer a map to a relation: war is the only thing two guilds
    /// declare at each other now that an alliance is a named group rather than a
    /// pairwise fact. See [`Alliance`].
    pub wars:         BTreeSet<GuildId>,
    /// Every guild it has declared war on that has not declared back.
    ///
    /// Separate from [`wars`](Self::wars) because a declaration is not a war: a
    /// guild that has declared and been ignored is not at war, and its members
    /// must not turn orange on the strength of its own opinion. The war exists
    /// once both sides hold the declaration — see [`Guilds::declare_war`].
    pub war_offers:   BTreeSet<GuildId>,
    /// Which alliance it belongs to, if any. A guild is in at most one.
    pub alliance:     Option<AllianceId>,
}

impl Guild {
    /// Whether this guild is at war with `other`.
    #[must_use]
    pub fn at_war_with(&self, other: GuildId) -> bool {
        self.wars.contains(&other)
    }

    /// Whether it has declared war on `other` and is still waiting.
    #[must_use]
    pub fn has_declared_on(&self, other: GuildId) -> bool {
        self.war_offers.contains(&other)
    }
}

/// Every guild on the shard.
///
/// A map rather than a `Vec`: unlike a region, a guild's id is not its position
/// in a list — guilds are founded and disbanded while the shard runs, and an id
/// must not be reused by the next one to be founded. `next_id` never goes
/// backwards for that reason.
#[derive(Clone, Default, Debug)]
pub struct Guilds {
    guilds:  BTreeMap<GuildId, Guild>,
    /// The next id to hand out. Monotonic, and saved with the world: a restart
    /// that restarted it would let a new guild inherit a disbanded one's id, and
    /// every member record still naming that id would silently join it.
    next_id: u32,
}

impl Guilds {
    /// Found a guild and return its id.
    pub fn found(
        &mut self,
        name: String,
        abbreviation: String,
        leader: openshard_protocol::serial::Serial,
    ) -> GuildId {
        self.next_id += 1;
        let id = GuildId(self.next_id);
        self.guilds.insert(
            id,
            Guild {
                id,
                name,
                abbreviation,
                leader,
                wars: BTreeSet::new(),
                war_offers: BTreeSet::new(),
                alliance: None,
            },
        );
        id
    }

    /// Put a guild back exactly as it was saved.
    ///
    /// Not [`found`](Self::found): that mints the next id, and a restore must
    /// keep the one already written on every member record. The counter is still
    /// raised past it, so a store whose world row went missing cannot re-issue an
    /// id that is plainly in use — the row is the authority, this is the floor.
    pub fn restore(&mut self, guild: Guild) {
        self.next_id = self.next_id.max(guild.id.0);
        self.guilds.insert(guild.id, guild);
    }

    /// The guild that calls itself `name`, case-insensitively.
    ///
    /// A scan, and deliberately: it is asked once when a guild is founded and
    /// never on a hot path, so an index would be a second thing to keep in step
    /// for no gain.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Guild> {
        self.guilds.values().find(|g| g.name.eq_ignore_ascii_case(name))
    }

    /// The guild that draws as `abbreviation`, case-insensitively.
    #[must_use]
    pub fn by_abbreviation(&self, abbreviation: &str) -> Option<&Guild> {
        self.guilds
            .values()
            .find(|g| g.abbreviation.eq_ignore_ascii_case(abbreviation))
    }

    /// One guild, if it exists.
    #[must_use]
    pub fn get(&self, id: GuildId) -> Option<&Guild> {
        self.guilds.get(&id)
    }

    /// One guild, to change.
    pub fn get_mut(&mut self, id: GuildId) -> Option<&mut Guild> {
        self.guilds.get_mut(&id)
    }

    /// Every guild, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Guild> {
        self.guilds.values()
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.guilds.len()
    }

    /// Whether none are founded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.guilds.is_empty()
    }

    /// Record a war **both ways**.
    ///
    /// A war is not one-sided: ServUO's `IsEnemy` is asked of either guild and
    /// both answer yes, so storing it on one and not the other would make the
    /// colour depend on which of the two a client happened to ask about.
    pub fn declare(&mut self, from: GuildId, to: GuildId) {
        if from == to {
            return;
        }
        if let Some(guild) = self.guilds.get_mut(&from) {
            guild.wars.insert(to);
        }
        if let Some(guild) = self.guilds.get_mut(&to) {
            guild.wars.insert(from);
        }
    }

    /// Declare war on `to`, and begin it if they have declared on you.
    ///
    /// Returns whether it took effect. The classic guildstone handshake:
    /// declaring war on a guild that has not declared on you puts you on their
    /// list and changes nothing else, and the war begins when they declare in
    /// return.
    ///
    /// A matching declaration is consumed — the war is the record, and a
    /// declaration left standing beside it would be a second answer to the same
    /// question.
    ///
    /// # This used to serve alliances too
    ///
    /// One function did both, on the argument that "an alliance is the same
    /// shape". It is not: an alliance is a named group a guild is invited *into*
    /// by a member of it, not a thing two guilds declare at each other. Trying to
    /// keep them one is what made an alliance pairwise. See [`Alliance`].
    pub fn declare_war(&mut self, from: GuildId, to: GuildId) -> bool {
        if from == to || !self.guilds.contains_key(&from) || !self.guilds.contains_key(&to) {
            return false;
        }
        if self.guilds[&to].has_declared_on(from) {
            self.withdraw(to, from);
            self.declare(from, to);
            return true;
        }
        if let Some(guild) = self.guilds.get_mut(&from) {
            guild.war_offers.insert(to);
        }
        false
    }

    /// Take back a declaration. Silent if there was none.
    pub fn withdraw(&mut self, from: GuildId, to: GuildId) {
        if let Some(guild) = self.guilds.get_mut(&from) {
            guild.war_offers.remove(&to);
        }
    }

    /// Withdraw a declaration, both ways, and any offer either side was still
    /// holding.
    ///
    /// Peace clears the offers too: a guild that made peace while the other's
    /// war declaration still stood would go back to war the moment it declared
    /// anything, without either side deciding to.
    pub fn undeclare(&mut self, from: GuildId, to: GuildId) {
        if let Some(guild) = self.guilds.get_mut(&from) {
            guild.wars.remove(&to);
            guild.war_offers.remove(&to);
        }
        if let Some(guild) = self.guilds.get_mut(&to) {
            guild.wars.remove(&from);
            guild.war_offers.remove(&from);
        }
    }

    /// Disband a guild, and take every declaration about it with it.
    ///
    /// The sweep is the point: a relation left pointing at a disbanded id would
    /// make the *next* guild founded under a reused id inherit a war. Ids are not
    /// reused, so this cannot actually happen — and it is swept anyway, because
    /// the invariant it protects is one line away from being broken by a future
    /// change to `found`.
    pub fn disband(&mut self, id: GuildId) -> Option<Guild> {
        let gone = self.guilds.remove(&id)?;
        for guild in self.guilds.values_mut() {
            guild.wars.remove(&id);
            guild.war_offers.remove(&id);
        }
        Some(gone)
    }

    /// The ids in use, for the save and for a test.
    #[must_use]
    pub fn ids(&self) -> BTreeSet<GuildId> {
        self.guilds.keys().copied().collect()
    }

    /// The highest id handed out so far, saved and restored with the world.
    #[must_use]
    pub const fn high_water(&self) -> u32 {
        self.next_id
    }

    /// Restore the id counter after a load. Never lowers it: an id already handed
    /// out must not be handed out again.
    pub fn set_high_water(&mut self, id: u32) {
        self.next_id = self.next_id.max(id);
    }
}

/// Every alliance on the shard.
#[derive(Clone, Default, Debug)]
pub struct Alliances {
    alliances: BTreeMap<AllianceId, Alliance>,
    /// The next id to hand out. Monotonic and saved, for [`Guilds`]' reason: a
    /// guild record names its alliance by id, and a restart that reissued one
    /// would put a guild into a body it never joined.
    next_id:   u32,
}

impl Alliances {
    /// Found an alliance led by `leader`, with `partner` asked in.
    ///
    /// Two guilds at the start and never one: ServUO's constructor takes both,
    /// and an alliance of one is the state [`remove`](Self::remove) disbands. The
    /// partner starts **pending** — founding it is an invitation, not a
    /// conscription, the same rule a guild's own membership has.
    pub fn found(&mut self, name: String, leader: GuildId, partner: GuildId) -> AllianceId {
        self.next_id += 1;
        let id = AllianceId(self.next_id);
        self.alliances.insert(
            id,
            Alliance {
                id,
                name,
                leader,
                members: BTreeSet::from([leader]),
                pending: BTreeSet::from([partner]),
            },
        );
        id
    }

    /// Put one back exactly as it was saved. See [`Guilds::restore`].
    pub fn restore(&mut self, alliance: Alliance) {
        self.next_id = self.next_id.max(alliance.id.0);
        self.alliances.insert(alliance.id, alliance);
    }

    /// The alliance that calls itself `name`, case-insensitively.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Alliance> {
        self.alliances
            .values()
            .find(|alliance| alliance.name.eq_ignore_ascii_case(name))
    }

    /// One alliance, if it exists.
    #[must_use]
    pub fn get(&self, id: AllianceId) -> Option<&Alliance> {
        self.alliances.get(&id)
    }

    /// One alliance, to change.
    pub fn get_mut(&mut self, id: AllianceId) -> Option<&mut Alliance> {
        self.alliances.get_mut(&id)
    }

    /// Every alliance, in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Alliance> {
        self.alliances.values()
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.alliances.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.alliances.is_empty()
    }

    /// Ask `guild` into `alliance`. Silent if it is already in or already asked.
    pub fn ask(&mut self, alliance: AllianceId, guild: GuildId) {
        if let Some(entry) = self.alliances.get_mut(&alliance) {
            if !entry.members.contains(&guild) {
                entry.pending.insert(guild);
            }
        }
    }

    /// Turn a pending guild into a member. `false` if it was not asked.
    pub fn accept(&mut self, alliance: AllianceId, guild: GuildId) -> bool {
        let Some(entry) = self.alliances.get_mut(&alliance) else {
            return false;
        };
        if !entry.pending.remove(&guild) {
            return false;
        }
        entry.members.insert(guild);
        true
    }

    /// Take a guild out, whether member or pending.
    ///
    /// Returns the alliance if it survived, and `None` if this emptied it —
    /// ServUO disbands below two members, and the leader leaving picks another
    /// rather than dissolving. The caller unhooks the guilds either way, which
    /// is why the *whole* membership comes back on a disband.
    pub fn remove(&mut self, alliance: AllianceId, guild: GuildId) -> Removal {
        let Some(entry) = self.alliances.get_mut(&alliance) else {
            return Removal::Gone;
        };
        entry.pending.remove(&guild);
        if !entry.members.remove(&guild) {
            return Removal::Stood;
        }
        // The leader leaving is not the end of it. ServUO picks another member,
        // and only an alliance that cannot field two disbands — which is what
        // makes an alliance outlive the guild that founded it, and why its id is
        // not that guild's.
        if entry.leader == guild {
            if let Some(&next) = entry.members.iter().next() {
                entry.leader = next;
            }
        }
        if entry.members.len() >= 2 {
            return Removal::Stood;
        }
        let gone = self.alliances.remove(&alliance).expect("just read");
        Removal::Disbanded(gone)
    }

    /// The highest id handed out so far, saved and restored with the world.
    #[must_use]
    pub const fn high_water(&self) -> u32 {
        self.next_id
    }

    /// Restore the id counter after a load. Never lowers it.
    pub fn set_high_water(&mut self, id: u32) {
        self.next_id = self.next_id.max(id);
    }
}

/// What taking a guild out of an alliance did to it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Removal {
    /// The alliance is still there.
    Stood,
    /// It fell below two members and is gone. Every guild it held comes back,
    /// members and pending both, because each has a link to unhook and the
    /// alliance is no longer there to be asked.
    Disbanded(Alliance),
    /// There was no such alliance.
    Gone,
}

#[cfg(test)]
mod tests {
    use openshard_protocol::serial::Serial;

    use super::*;

    fn leader() -> Serial {
        Serial::new(0x0000_0001).expect("a mobile serial")
    }

    fn two() -> (Guilds, GuildId, GuildId) {
        let mut guilds = Guilds::default();
        let a = guilds.found("A".to_owned(), "A".to_owned(), leader());
        let b = guilds.found("B".to_owned(), "B".to_owned(), leader());
        (guilds, a, b)
    }

    #[test]
    fn a_declaration_binds_both_guilds() {
        // The colour must not depend on which of the two a client asks about.
        let (mut guilds, a, b) = two();
        guilds.declare(a, b);
        assert!(guilds.get(a).unwrap().at_war_with(b));
        assert!(guilds.get(b).unwrap().at_war_with(a));
    }

    #[test]
    fn a_guild_is_not_at_war_with_itself() {
        let (mut guilds, a, _) = two();
        guilds.declare(a, a);
        assert!(!guilds.get(a).unwrap().at_war_with(a));
    }

    /// The guildstone's rule: one declaration is not a war.
    #[test]
    fn a_war_takes_two_declarations() {
        let (mut guilds, a, b) = two();
        assert!(!guilds.declare_war(a, b), "a war with one side");
        assert!(!guilds.get(a).unwrap().at_war_with(b));
        assert!(!guilds.get(b).unwrap().at_war_with(a));
        assert!(guilds.get(a).unwrap().has_declared_on(b));

        assert!(guilds.declare_war(b, a));
        assert!(guilds.get(a).unwrap().at_war_with(b));
        assert!(guilds.get(b).unwrap().at_war_with(a));
        assert!(
            !guilds.get(a).unwrap().has_declared_on(b),
            "the declaration is consumed by the war it made"
        );
    }

    /// Peace clears the declarations too. A guild that made peace while the
    /// other's still stood would go back to war the moment it declared anything.
    #[test]
    fn peace_clears_what_either_side_was_still_holding() {
        let (mut guilds, a, b) = two();
        guilds.declare_war(a, b);
        guilds.declare_war(b, a);
        guilds.undeclare(a, b);
        assert!(!guilds.get(a).unwrap().at_war_with(b));
        assert!(!guilds.get(b).unwrap().at_war_with(a));
        assert!(!guilds.get(a).unwrap().has_declared_on(b));
        assert!(!guilds.get(b).unwrap().has_declared_on(a));
    }

    #[test]
    fn disbanding_takes_every_declaration_about_it() {
        let (mut guilds, a, b) = two();
        guilds.declare(a, b);
        guilds.declare_war(b, a);
        guilds.disband(a);
        assert!(!guilds.get(b).unwrap().at_war_with(a));
        assert!(!guilds.get(b).unwrap().has_declared_on(a));
    }

    #[test]
    fn an_id_is_never_reissued() {
        let (mut guilds, a, _) = two();
        guilds.disband(a);
        let c = guilds.found("C".to_owned(), "C".to_owned(), leader());
        assert_ne!(c, a, "a disbanded id must not come back");
    }

    /// Founding is an invitation: the partner is pending, not a member. An
    /// alliance that conscripted would be one a guild could be put into by
    /// somebody else naming it.
    #[test]
    fn founding_an_alliance_asks_the_partner_rather_than_adding_them() {
        let (_, a, b) = two();
        let mut alliances = Alliances::default();
        let id = alliances.found("The Compact".to_owned(), a, b);
        let alliance = alliances.get(id).expect("just founded");
        assert_eq!(alliance.leader, a);
        assert!(alliance.contains(a));
        assert!(!alliance.contains(b), "asking joined them");
        assert!(alliance.pending.contains(&b));

        assert!(alliances.accept(id, b));
        assert!(alliances.get(id).unwrap().contains(b));
        assert!(!alliances.accept(id, b), "and only once");
    }

    /// The leader leaving picks another rather than dissolving — which is what
    /// makes an alliance outlive its founder, and why its id is not that guild's.
    #[test]
    fn the_leader_leaving_hands_the_alliance_on() {
        let mut guilds = Guilds::default();
        let a = guilds.found("A".to_owned(), "A".to_owned(), leader());
        let b = guilds.found("B".to_owned(), "B".to_owned(), leader());
        let c = guilds.found("C".to_owned(), "C".to_owned(), leader());
        let mut alliances = Alliances::default();
        let id = alliances.found("The Compact".to_owned(), a, b);
        alliances.accept(id, b);
        alliances.ask(id, c);
        alliances.accept(id, c);

        assert_eq!(alliances.remove(id, a), Removal::Stood);
        let alliance = alliances.get(id).expect("it stood");
        assert_ne!(alliance.leader, a, "somebody else leads it now");
        assert!(alliance.contains(alliance.leader), "and leads from inside it");
    }

    /// Below two members it is gone, and the whole membership comes back so the
    /// caller can unhook every guild that named it.
    #[test]
    fn an_alliance_of_one_disbands() {
        let (_, a, b) = two();
        let mut alliances = Alliances::default();
        let id = alliances.found("The Compact".to_owned(), a, b);
        alliances.accept(id, b);

        let Removal::Disbanded(gone) = alliances.remove(id, b) else {
            panic!("two members less one is not an alliance");
        };
        assert_eq!(gone.members, BTreeSet::from([a]));
        assert!(alliances.get(id).is_none());
        assert!(alliances.is_empty());
    }

    #[test]
    fn a_pending_guild_that_leaves_does_not_disband_it() {
        // The pending one was never a member, so the count never moved.
        let (_, a, b) = two();
        let mut alliances = Alliances::default();
        let id = alliances.found("The Compact".to_owned(), a, b);
        assert_eq!(alliances.remove(id, b), Removal::Stood);
        assert!(alliances.get(id).is_some());
    }
}
