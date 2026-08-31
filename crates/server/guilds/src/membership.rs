//! Founding a guild, joining one, moving up and down inside it, and leaving.
//!
//! Authority is asked through [`may`](crate::may) — the flag the operation
//! needs — except for the two things no flag grants, [`disband`] and
//! [`pass_leadership`], which ask [`may_lead`](crate::may_lead). Where an
//! operation acts *on* another member there is a second question,
//! [`outranks`](crate::outranks), and it is a different one: holding
//! `REMOVE_PLAYERS` says you may dismiss, not that you may dismiss *them*.

use openshard_entities::EntityId;
use openshard_state::{
    GuildCandidate,
    GuildId,
    GuildMember,
    Rank,
    WorldState,
};

use crate::{
    RankFlags,
    Refusal,
    announce,
    may,
    may_lead,
    outranks,
    rank_of,
    recolour_guild,
    roster,
};

/// The longest a guild's name may be. ServUO's `GuildNamePrompt`.
pub const NAME_LIMIT: usize = 40;
/// The longest an abbreviation may be — the client draws it in brackets after
/// the name, and three letters is what the gump's field takes. ServUO's
/// `GuildAbbrvPrompt`.
pub const ABBREVIATION_LIMIT: usize = 3;
/// The longest a member's guild title may be. ServUO's `GuildTitlePrompt`.
pub const TITLE_LIMIT: usize = 20;

/// Trim a player's typed text and cut it to length.
///
/// Truncation rather than refusal, which is what ServUO's three prompts do: a
/// player who types forty-one characters gets forty, not an error and a lost
/// name. Cut on a character boundary — the limits are counts of characters, and
/// a client may send any UTF-8 it likes.
pub(crate) fn clip(text: &str, limit: usize) -> String {
    text.trim().chars().take(limit).collect()
}

/// Found a guild with `leader` at its head.
///
/// The name and abbreviation are clipped, not validated: see [`clip`]. What *is*
/// refused is an empty one and one another guild already answers to — the
/// abbreviation is drawn beside a name and two guilds sharing one would make the
/// bracket a lie.
pub fn found(
    state: &mut WorldState,
    leader: EntityId,
    name: &str,
    abbreviation: &str,
) -> Result<GuildId, Refusal> {
    let serial = state.registry.serial_of(leader).ok_or(Refusal::NotAMobile)?;
    if state.guild_of(leader).is_some() {
        return Err(Refusal::AlreadyInAGuild);
    }
    let name = clip(name, NAME_LIMIT);
    let abbreviation = clip(abbreviation, ABBREVIATION_LIMIT);
    if name.is_empty() || abbreviation.is_empty() {
        return Err(Refusal::NoName);
    }
    if state.guilds.by_name(&name).is_some() {
        return Err(Refusal::NameTaken);
    }
    if state.guilds.by_abbreviation(&abbreviation).is_some() {
        return Err(Refusal::AbbreviationTaken);
    }

    let guild = state.guilds.found(name, abbreviation, serial);
    // An invitation the founder was still holding is gone: they answered a
    // different question. Left standing, accepting it later would move the
    // guild's own leader into somebody else's.
    state.registry.remove::<GuildCandidate>(leader);
    state.registry.insert(
        leader,
        GuildMember {
            guild,
            title: String::new(),
            // The founder is the Leader, not a Ronin who happens to be first
            // through the door — ServUO sets `RankDefinition.Leader` on the
            // `Guild.Leader` setter, and a guild founded with nobody able to
            // invite would be one nobody could join.
            rank: Rank::Leader,
        },
    );
    state.broadcast_move(leader);
    Ok(guild)
}

/// Ask `candidate` to join the guild `inviter` leads.
///
/// Leaves the question with the candidate, who answers it with
/// [`accept_invitation`] or [`decline_invitation`]. Nothing is recorded on the
/// guild: an invitation is one player's to answer, and a guild holding a list of
/// people it has asked is a list that outlives them.
pub fn invite(state: &mut WorldState, inviter: EntityId, candidate: EntityId) -> Result<(), Refusal> {
    let (guild, _) = may(state, inviter, RankFlags::CAN_INVITE)?;
    if candidate == inviter {
        return Err(Refusal::Yourself);
    }
    if state.registry.serial_of(candidate).is_none() {
        return Err(Refusal::NotAMobile);
    }
    if state.guild_of(candidate).is_some() {
        return Err(Refusal::TheyAreInAGuild);
    }
    state.registry.insert(candidate, GuildCandidate { guild });
    Ok(())
}

/// Answer an invitation with yes.
///
/// Re-announces the whole guild rather than only the joiner, because the colour
/// moved on two sets of screens: the guild now sees a green newcomer, and the
/// newcomer now sees a guild.
pub fn accept_invitation(state: &mut WorldState, candidate: EntityId) -> Result<GuildId, Refusal> {
    let guild = state
        .registry
        .get::<GuildCandidate>(candidate)
        .map(|invitation| invitation.guild)
        .ok_or(Refusal::NotInvited)?;
    // The invitation outlived the guild — disbanded, or the leader gone. Clear it
    // rather than joining a guild that no longer exists.
    if state.guilds.get(guild).is_none() {
        state.registry.remove::<GuildCandidate>(candidate);
        return Err(Refusal::NoSuchGuild);
    }
    if state.guild_of(candidate).is_some() {
        return Err(Refusal::AlreadyInAGuild);
    }
    state.registry.remove::<GuildCandidate>(candidate);
    state.registry.insert(
        candidate,
        GuildMember {
            guild,
            title: String::new(),
            // A Ronin, holding nothing — ServUO's `Guild.AddMember`. A guild
            // that wants a member out of a newcomer has to promote one, which
            // is the point of the rank existing.
            rank: Rank::Ronin,
        },
    );
    recolour_guild(state, guild);
    Ok(guild)
}

/// Answer an invitation with no. Silent if there was none to answer.
pub fn decline_invitation(state: &mut WorldState, candidate: EntityId) {
    state.registry.remove::<GuildCandidate>(candidate);
}

/// Leave the guild you are in.
///
/// A leader may not walk out on a guild that still has members — the guild would
/// be left with a leader serial naming nobody, and no way to appoint another. A
/// leader who *is* the last member disbands it instead, which is the same thing
/// said honestly.
pub fn leave(state: &mut WorldState, member: EntityId) -> Result<(), Refusal> {
    let guild = state.guild_of(member).ok_or(Refusal::NotInAGuild)?.id;
    if may_lead(state, member).is_ok() {
        if roster(state, guild).len() > 1 {
            return Err(Refusal::PassLeadershipFirst);
        }
        return disband(state, member);
    }
    state.registry.remove::<GuildMember>(member);
    state.broadcast_move(member);
    recolour_guild(state, guild);
    Ok(())
}

/// Turn a member out of the guild.
///
/// Two ways to be allowed to, which is ServUO's condition verbatim
/// (`GuildMemberInfoGump`'s kick arm): `REMOVE_PLAYERS` and outranking them, or
/// `REMOVE_LOWEST_RANK` and a target who is a [`Rank::Ronin`]. The second is
/// what lets an ordinary member get rid of a newcomer without being able to
/// touch anybody else.
pub fn dismiss(state: &mut WorldState, actor: EntityId, member: EntityId) -> Result<(), Refusal> {
    let guild = state.guild_of(actor).ok_or(Refusal::NotInAGuild)?.id;
    if member == actor {
        return Err(Refusal::Yourself);
    }
    if state.guild_of(member).map(|g| g.id) != Some(guild) {
        return Err(Refusal::NotYourMember);
    }
    if !crate::may_dismiss(state, actor, member) {
        // Which of the two refusals is the more useful thing to say: somebody
        // who holds `REMOVE_PLAYERS` and was stopped was stopped by the target's
        // rank, and everyone else was stopped by their own.
        let holds = rank_of(state, actor)
            .map(crate::rank::flags_of)
            .is_some_and(|flags| flags.has(RankFlags::REMOVE_PLAYERS));
        return Err(match holds {
            true => Refusal::TheyOutrankYou,
            false => Refusal::NotYourPlaceTo,
        });
    }
    state.registry.remove::<GuildMember>(member);
    state.system_message(member, "You have been dismissed from your guild.");
    state.broadcast_move(member);
    recolour_guild(state, guild);
    Ok(())
}

/// Give a member the title the guild knows them by — "Warlord", "Emissary".
///
/// An empty title is how one is taken away, and is not an error: a leader
/// clearing a field is saying something, and refusing it would leave no way to
/// undo a title at all.
pub fn set_title(
    state: &mut WorldState,
    actor: EntityId,
    member: EntityId,
    title: &str,
) -> Result<(), Refusal> {
    let (guild, _) = may(state, actor, RankFlags::CAN_SET_GUILD_TITLE)?;
    if state.guild_of(member).map(|g| g.id) != Some(guild) {
        return Err(Refusal::NotYourMember);
    }
    // Yourself, or somebody you outrank — ServUO's `playerRank.Rank >
    // targetRank.Rank || m_Member == player`. Retitling yourself is the arm
    // worth naming: an Emissary may not touch another Emissary's title, and
    // would otherwise be unable to change their own either.
    if member != actor && !outranks(state, actor, member) {
        return Err(Refusal::TheyOutrankYou);
    }
    let title = clip(title, TITLE_LIMIT);
    if let Some(entry) = state.registry.get_mut::<GuildMember>(member) {
        entry.title = title;
    }
    // No `broadcast_move`: a title is not a colour. It is read off the name, and
    // the name is asked for a click at a time — so the next single-click has it,
    // and a `0x77` would move nothing.
    state.system_message(member, "Your guild title has changed.");
    Ok(())
}

/// Move a member one rank up.
///
/// # Two ranks below, not one
///
/// ServUO's condition is `(playerRank - 1) > targetRank`, or `playerRank >
/// targetRank` for the Leader alone. So an Emissary may promote a Ronin to
/// Member and no further: promoting somebody to the rank directly below your own
/// is already too far, because that rank might hold a flag you do not. Only the
/// Leader may promote into the rank below theirs — and promoting *to* Leader is
/// not this function at all, it is [`pass_leadership`], because a guild has one.
pub fn promote(state: &mut WorldState, actor: EntityId, member: EntityId) -> Result<Rank, Refusal> {
    let (guild, actor_rank) = may(state, actor, RankFlags::CAN_PROMOTE_DEMOTE)?;
    if member == actor {
        return Err(Refusal::Yourself);
    }
    if state.guild_of(member).map(|g| g.id) != Some(guild) {
        return Err(Refusal::NotYourMember);
    }
    let target = rank_of(state, member).ok_or(Refusal::NotYourMember)?;
    let far_enough = match actor_rank {
        Rank::Leader => actor_rank > target,
        _ => actor_rank.number().saturating_sub(1) > target.number(),
    };
    if !far_enough {
        return Err(Refusal::TheyOutrankYou);
    }
    let next = target.above().ok_or(Refusal::NoFurtherRank)?;
    // The rung below the top is as far as a promotion goes. Reaching Leader is
    // handing the guild over, and that is a different act with a different
    // consequence for the person doing it — see `pass_leadership`.
    if next == Rank::Leader {
        return Err(Refusal::NoFurtherRank);
    }
    set_rank(state, member, next);
    state.system_message(member, &format!("You are now a {} of your guild.", next.name()));
    Ok(next)
}

/// Move a member one rank down.
///
/// Needs only that you outrank them — ServUO's `playerRank.Rank >
/// targetRank.Rank`, without the promotion's extra rung. A [`Rank::Ronin`] is
/// the floor: there is nothing below, and the way to be rid of one is
/// [`dismiss`].
pub fn demote(state: &mut WorldState, actor: EntityId, member: EntityId) -> Result<Rank, Refusal> {
    let (guild, _) = may(state, actor, RankFlags::CAN_PROMOTE_DEMOTE)?;
    if member == actor {
        return Err(Refusal::Yourself);
    }
    if state.guild_of(member).map(|g| g.id) != Some(guild) {
        return Err(Refusal::NotYourMember);
    }
    if !outranks(state, actor, member) {
        return Err(Refusal::TheyOutrankYou);
    }
    let target = rank_of(state, member).ok_or(Refusal::NotYourMember)?;
    let next = target.below().ok_or(Refusal::NoFurtherRank)?;
    set_rank(state, member, next);
    state.system_message(member, &format!("You are now a {} of your guild.", next.name()));
    Ok(next)
}

/// Write a member's rank. Silent if they hold no membership — every caller has
/// already established that they do.
fn set_rank(state: &mut WorldState, member: EntityId, rank: Rank) {
    if let Some(entry) = state.registry.get_mut::<GuildMember>(member) {
        entry.rank = rank;
    }
}

/// Hand the guild to one of its members.
///
/// The one promotion that reaches [`Rank::Leader`], and it is a trade rather
/// than a gift: the outgoing leader becomes a [`Rank::Member`], which is
/// ServUO's `Guild.Leader` setter exactly. Leaving them at Leader would give the
/// guild two, and dropping them to Ronin would turn a founder out of their own
/// decisions on the way past.
pub fn pass_leadership(state: &mut WorldState, leader: EntityId, member: EntityId) -> Result<(), Refusal> {
    let guild = may_lead(state, leader)?;
    if member == leader {
        return Err(Refusal::Yourself);
    }
    if state.guild_of(member).map(|g| g.id) != Some(guild) {
        return Err(Refusal::NotYourMember);
    }
    let serial = state.registry.serial_of(member).ok_or(Refusal::NotAMobile)?;
    if let Some(entry) = state.guilds.get_mut(guild) {
        entry.leader = serial;
    }
    set_rank(state, leader, Rank::Member);
    set_rank(state, member, Rank::Leader);
    announce(state, guild, "Your guild has a new leader.");
    Ok(())
}

/// Disband the guild, and take its roster and its diplomacy with it.
///
/// Every member loses the component here rather than discovering it later:
/// [`guild_of`](WorldState::guild_of) already reads a membership naming a dead
/// guild as none, so a member who was offline is safe — but a member who is
/// standing here has watchers to be told about, and they can only be told while
/// the roster still names them.
pub fn disband(state: &mut WorldState, leader: EntityId) -> Result<(), Refusal> {
    let guild = may_lead(state, leader)?;
    let members = roster(state, guild);
    // Every guild that had a declaration about this one, so their members'
    // screens can be told. Taken before the disband, because the sweep inside it
    // is what removes them.
    let entangled: Vec<GuildId> = state
        .guilds
        .get(guild)
        .map(|g| g.wars.iter().chain(g.war_offers.iter()).copied().collect())
        .unwrap_or_default();
    // And the alliance, which has to let go of a guild that no longer exists —
    // otherwise its members read green to a roster that is not there.
    let alliance = state.guilds.get(guild).and_then(|g| g.alliance);

    state.guilds.disband(guild);
    for member in &members {
        state.registry.remove::<GuildMember>(*member);
        state.system_message(*member, "Your guild has been disbanded.");
        state.broadcast_move(*member);
    }
    if let Some(alliance) = alliance {
        if let openshard_state::Removal::Disbanded(gone) = state.alliances.remove(alliance, guild) {
            for member in gone.members.iter().chain(gone.pending.iter()) {
                if let Some(entry) = state.guilds.get_mut(*member) {
                    entry.alliance = None;
                }
                recolour_guild(state, *member);
            }
        } else {
            for member in crate::alliance_members(state, alliance) {
                recolour_guild(state, member);
            }
        }
    }
    // A guild that was at war with this one has members whose screens still show
    // orange. They are no longer in the roster above — the declaration is gone
    // with the guild — so they are told here or not at all.
    for other in entangled {
        recolour_guild(state, other);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ABBREVIATION_LIMIT,
        clip,
    };

    #[test]
    fn a_name_is_trimmed_and_cut_on_a_character_boundary() {
        assert_eq!(clip("  The Silver Serpent  ", 40), "The Silver Serpent");
        assert_eq!(clip("OSSA", ABBREVIATION_LIMIT), "OSS");
        // Cut by characters, not bytes: `&str[..3]` on this would panic, and the
        // client is free to send any UTF-8 it likes.
        assert_eq!(clip("ÆØÅX", ABBREVIATION_LIMIT), "ÆØÅ");
        assert_eq!(clip("   ", 40), "");
    }
}
