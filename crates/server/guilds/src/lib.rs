//! Guilds: founding one, who is in it, and how two of them come to be at war.
//!
//! # What is here, and what is [`openshard_state::guild`]
//!
//! The substrate below holds what a guild *is* — its name, its roster key, and
//! the relations the packet path reads to colour a mobile. This crate holds the
//! rules: who may found a guild, who may invite, what a leader is allowed to do,
//! and the handshake that turns a declaration into a war. The same split
//! `openshard-quests` has over [`QuestDef`](openshard_state::QuestDef), and for
//! the same reason: `0x77` carries a notoriety byte, so state cannot ask a crate
//! above it what colour to draw.
//!
//! # The handshake, and why there is no "accept" function
//!
//! A war is two declarations, not a declaration and a consent. Guild A declares
//! war on B and nothing changes; B declares war on A and both are at war. That
//! is the guildstone's rule, and an alliance is the same shape, so one
//! [`propose`] serves both. There is no accept path to keep in step with the
//! declare path, and no way to be at war with a guild that has not said so.
//!
//! Membership is the other shape and deliberately so: an invitation *is* a
//! consent, because a guild may not conscript a player. [`invite`] leaves a
//! [`GuildCandidate`](openshard_state::GuildCandidate) and the player answers it.
//!
//! # Ranks
//!
//! ServUO's five, and its flag set per rank — see [`Rank`] for the ladder and
//! [`RankFlags`] for what each rung may do. Authority is asked in exactly two
//! ways and the difference matters:
//!
//! - [`may`] — "does this member hold this flag", which is what invite, dismiss,
//!   set-title, promote, demote and the war operations gate on.
//! - [`may_lead`] — "is this member *the* leader", for the two things no flag
//!   grants: disbanding the guild, and handing it to somebody else.
//!
//! A rank comparison is a third question again, and it is about the *target*
//! rather than the actor: a member with `RemovePlayers` may still only turn out
//! somebody below them. Every one of those comparisons is ServUO's own, copied
//! rather than reasoned about — see [`RankFlags`]' note on the Emissary and the
//! Warlord, which is where an intuition about rank order goes wrong.
//!
//! # What every change does to the screen
//!
//! Guild colour is relative — see
//! [`notoriety_toward`](openshard_state::WorldState::notoriety_toward) — so a
//! change here is a change to what several clients should be drawing, and none of
//! them will ask. Every operation that can move a colour ends by re-announcing
//! the mobiles it moved. That is what [`recolour_guild`] is for, and why joining
//! a guild re-announces the *whole* guild rather than only the joiner: the new
//! member's own screen has to turn green too.

mod chat;
mod diplomacy;
pub mod gump;
mod membership;
mod rank;
mod reply;
#[cfg(test)]
mod tests;

pub use chat::{say_to_alliance, say_to_guild, tell_guild};
pub use diplomacy::{
    ALLIANCE_NAME_LIMIT, Outcome, alliance_members, declare_war, invite_to_alliance, join_alliance,
    leave_alliance, make_peace,
};
pub use gump::GUILD_GUMP;
pub use membership::{
    ABBREVIATION_LIMIT, NAME_LIMIT, TITLE_LIMIT, accept_invitation, decline_invitation, demote, disband,
    dismiss, found, invite, leave, pass_leadership, promote, set_title,
};
pub use rank::RankFlags;
pub use reply::{handle, open};

use openshard_entities::EntityId;
use openshard_state::{GuildId, Rank, WorldState};

/// Why a guild operation was refused.
///
/// An enum rather than a message string, so a caller can decide what to do about
/// it and a test can name the case without matching prose. [`Refusal::message`]
/// is the wording, in one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// The actor is in no guild.
    NotInAGuild,
    /// The actor is in a guild but does not lead it.
    NotTheLeader,
    /// The actor is already in a guild, and a mobile is in at most one.
    AlreadyInAGuild,
    /// The other mobile is already in a guild.
    TheyAreInAGuild,
    /// The name is empty once trimmed, or is all whitespace.
    NoName,
    /// Another guild already calls itself that.
    NameTaken,
    /// Another guild already draws as that abbreviation.
    AbbreviationTaken,
    /// The mobile named is not one a guild can hold — no serial, so nothing to
    /// record and nothing to draw.
    NotAMobile,
    /// The mobile named is not in the actor's guild.
    NotYourMember,
    /// The operation names the actor, and it is not one that may.
    Yourself,
    /// There is no invitation to answer.
    NotInvited,
    /// The guild named does not exist, or is the actor's own.
    NoSuchGuild,
    /// The leader is not the last member, so the guild would be left without
    /// one.
    PassLeadershipFirst,
    /// The actor's rank does not hold the flag the operation needs.
    NotYourPlaceTo,
    /// The actor holds the flag but the target outranks them, or stands level
    /// with them. A separate refusal from [`NotYourPlaceTo`](Self::NotYourPlaceTo)
    /// because it says something different to the player: not "you may not do
    /// this" but "not to *them*".
    TheyOutrankYou,
    /// The target is already a [`Rank::Leader`] and there is nowhere above it,
    /// or already a [`Rank::Ronin`] and nowhere below.
    NoFurtherRank,
    /// The guild is in no alliance.
    NoAllies,
    /// The guild is already in one, and a guild is in at most one.
    AlreadyAllied,
    /// The other guild is already in one.
    TheyAreAllied,
    /// Nobody has asked this guild into an alliance.
    NotAsked,
    /// The two guilds are at war, which is not a thing an alliance can hold.
    AtWarWithThem,
    /// The two guilds are allied, which is not a thing a war can be declared
    /// across.
    AlliedWithThem,
    /// The guild is in no alliance to leave.
    NotAllied,
}

impl Refusal {
    /// What to tell the player.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotInAGuild => "You are not in a guild.",
            Self::NotTheLeader => "You do not lead your guild.",
            Self::AlreadyInAGuild => "You are already in a guild.",
            Self::TheyAreInAGuild => "They are already in a guild.",
            Self::NoName => "That is not a name.",
            Self::NameTaken => "Another guild is already called that.",
            Self::AbbreviationTaken => "Another guild already uses that abbreviation.",
            Self::NotAMobile => "That cannot join a guild.",
            Self::NotYourMember => "They are not in your guild.",
            Self::Yourself => "You cannot do that to yourself.",
            Self::NotInvited => "Nobody has asked you to join a guild.",
            Self::NoSuchGuild => "There is no such guild.",
            Self::PassLeadershipFirst => "Pass leadership on before you leave.",
            Self::NotYourPlaceTo => "Your rank does not allow that.",
            Self::TheyOutrankYou => "They outrank you.",
            Self::NoFurtherRank => "There is no rank beyond that one.",
            Self::NoAllies | Self::NotAllied => "Your guild is in no alliance.",
            Self::AlreadyAllied => "Your guild is already in an alliance.",
            Self::TheyAreAllied => "They are already in an alliance.",
            Self::NotAsked => "Nobody has asked your guild into an alliance.",
            Self::AtWarWithThem => "You are at war with them.",
            Self::AlliedWithThem => "You are allied with them.",
        }
    }
}

/// The guild `actor` leads, or why it does not lead one.
///
/// **Not the general authority check** — [`may`] is. This is the narrower
/// question, for the two operations no rank flag grants because no rank but the
/// Leader may do them at all: [`disband`] and [`pass_leadership`]. Everything
/// else asks [`may`] for the flag it needs, so that a guild can delegate.
pub fn may_lead(state: &WorldState, actor: EntityId) -> Result<GuildId, Refusal> {
    let guild = state.guild_of(actor).ok_or(Refusal::NotInAGuild)?;
    let serial = state.registry.serial_of(actor).ok_or(Refusal::NotAMobile)?;
    if guild.leader == serial {
        Ok(guild.id)
    } else {
        Err(Refusal::NotTheLeader)
    }
}

/// The rank `actor` holds in the guild they are in.
///
/// [`Rank::Ronin`] for a member with no rank recorded, which is also what a
/// newcomer is — so a membership written before ranks existed reads as the
/// least trusted rank rather than the most.
#[must_use]
pub fn rank_of(state: &WorldState, actor: EntityId) -> Option<Rank> {
    state
        .registry
        .get::<openshard_state::GuildMember>(actor)
        .filter(|member| state.guilds.get(member.guild).is_some())
        .map(|member| member.rank)
}

/// The guild `actor` may exercise `flag` in, and the rank they hold — or why
/// not.
///
/// **The one authority check.** Every operation a member's rank might or might
/// not permit goes through here, so "what an Emissary may do" is a list of
/// callers rather than a rule restated at each of them.
///
/// The rank comes back with the guild because most callers need it immediately
/// afterwards for the *other* question: whether they outrank the member they are
/// acting on. See [`outranks`].
pub fn may(state: &WorldState, actor: EntityId, flag: RankFlags) -> Result<(GuildId, Rank), Refusal> {
    let guild = state.guild_of(actor).ok_or(Refusal::NotInAGuild)?.id;
    let rank = rank_of(state, actor).ok_or(Refusal::NotInAGuild)?;
    if rank::flags_of(rank).has(flag) {
        Ok((guild, rank))
    } else {
        Err(Refusal::NotYourPlaceTo)
    }
}

/// Whether `actor` may turn `target` out of the guild.
///
/// The two ways there are, as one predicate: `REMOVE_PLAYERS` and outranking
/// them, or `REMOVE_LOWEST_RANK` and a target who is a [`Rank::Ronin`]. A
/// predicate rather than the condition written twice, because
/// [`dismiss`] enforces it and the roster window decides whether to draw the
/// button by it — and a window that offered what the operation refuses is worse
/// than one that offers nothing.
///
/// Says nothing about *membership*: both are assumed to be in the same guild,
/// which is [`dismiss`]'s own check.
#[must_use]
pub fn may_dismiss(state: &WorldState, actor: EntityId, target: EntityId) -> bool {
    let Some(flags) = rank_of(state, actor).map(rank::flags_of) else {
        return false;
    };
    (flags.has(RankFlags::REMOVE_PLAYERS) && outranks(state, actor, target))
        || (flags.has(RankFlags::REMOVE_LOWEST_RANK) && rank_of(state, target) == Some(Rank::Ronin))
}

/// Whether `actor` stands strictly above `target` in the same guild.
///
/// Strictly: two members of the same rank do not outrank each other, so an
/// Emissary cannot dismiss or retitle another Emissary. That is ServUO's
/// comparison (`playerRank.Rank > targetRank.Rank`) and it is what stops a rank
/// from being able to unmake itself.
#[must_use]
pub fn outranks(state: &WorldState, actor: EntityId, target: EntityId) -> bool {
    match (rank_of(state, actor), rank_of(state, target)) {
        (Some(actor), Some(target)) => actor > target,
        _ => false,
    }
}

/// Everyone currently in `guild`.
///
/// A scan of the [`GuildMember`](openshard_state::GuildMember) column, which is
/// the rare direction — see the component's own note. Collected rather than
/// returned lazily because every caller goes on to change the world with it, and
/// an iterator borrowing the registry cannot.
#[must_use]
pub fn roster(state: &WorldState, guild: GuildId) -> Vec<EntityId> {
    state
        .registry
        .query::<openshard_state::GuildMember>()
        .filter(|(_, member)| member.guild == guild)
        .map(|(entity, _)| entity)
        .collect()
}

/// Re-announce every member of `guild` to everyone watching them.
///
/// The colour a mobile draws in is the *watcher's* answer, and nothing on a
/// client will ask again on its own: a war declared while two members stand
/// facing each other would show blue until one of them took a step. This is the
/// step, sent on the shard's initiative.
///
/// Over-sending on purpose. It re-announces to every watcher rather than working
/// out which ones the change could have moved, because the set that *could* move
/// is "every member of every guild with a relation to this one, plus everyone who
/// has ever been in one" — and a `0x77` a client already agrees with costs a
/// packet and changes nothing on screen.
pub fn recolour_guild(state: &mut WorldState, guild: GuildId) {
    for member in roster(state, guild) {
        state.broadcast_move(member);
    }
}

/// Tell every member of `guild` who is online something that happened to it.
pub fn announce(state: &mut WorldState, guild: GuildId, text: &str) {
    for member in roster(state, guild) {
        state.system_message(member, text);
    }
}
