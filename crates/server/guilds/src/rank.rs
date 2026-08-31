//! What each of a guild's five ranks is allowed to do.
//!
//! [`Rank`](openshard_state::Rank) itself lives in the substrate, because a
//! member's rank is saved with them. The permissions are here, because nothing
//! below this crate ever asks one — the packet path needs to know a mobile's
//! guild to pick a notoriety byte, and never needs to know whether they may
//! invite.
//!
//! Ported from ServUO's `RankFlags` and `RankDefinition.Ranks`
//! (`Scripts/Misc/Guild.cs`). The bit values are the reference's, kept rather
//! than renumbered: nothing puts them on the wire, but a table copied with its
//! own numbers is one an auditor can diff against the source.

use openshard_state::Rank;

/// What a rank may do, as a set of bits.
///
/// The same newtype-over-an-integer shape
/// [`StatusFlags`](openshard_protocol::mobile::StatusFlags) has, and for the same
/// reason: a named constant per bit, and `const` combinators, so a table of
/// ranks reads as a table.
///
/// # The Emissary and the Warlord are the trap
///
/// The ranks are ordered and the permissions are **not nested**. An Emissary
/// (rank 2) recruits, dismisses, promotes and titles; a Warlord (rank 3) sits
/// above it and does none of those — it declares wars, which the Emissary
/// cannot. So "a higher rank may do everything a lower one may" is false, and
/// any check written as a rank comparison instead of a flag test gets one of the
/// two wrong. The comparison has exactly one job — deciding who outranks whom,
/// which is a question about the *target* of a promote, demote, dismiss or
/// title.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RankFlags(pub u32);

impl RankFlags {
    /// Nothing. What a [`Rank::Ronin`] holds.
    pub const NONE: Self = Self(0);
    /// May ask a player to join.
    pub const CAN_INVITE: Self = Self(0x0000_0001);
    /// May use items the guild owns. No consumer yet — the guildstone is not an
    /// item on this shard, which is what would own one.
    pub const ACCESS_GUILD_ITEMS: Self = Self(0x0000_0002);
    /// May turn out a member of the lowest rank, and only that rank. What lets
    /// an ordinary member get rid of a Ronin.
    pub const REMOVE_LOWEST_RANK: Self = Self(0x0000_0004);
    /// May turn out any member they outrank.
    pub const REMOVE_PLAYERS: Self = Self(0x0000_0008);
    /// May move a member up and down the ladder.
    pub const CAN_PROMOTE_DEMOTE: Self = Self(0x0000_0010);
    /// May declare a war and make a peace.
    pub const CONTROL_WAR_STATUS: Self = Self(0x0000_0020);
    /// May propose and dissolve an alliance. The Leader's alone.
    pub const ALLIANCE_CONTROL: Self = Self(0x0000_0040);
    /// May give a member a guild title.
    pub const CAN_SET_GUILD_TITLE: Self = Self(0x0000_0080);
    /// May vote. No consumer yet — this engine has no guild vote, which in
    /// ServUO is how a guildmaster is elected.
    pub const CAN_VOTE: Self = Self(0x0000_0100);

    /// What an ordinary [`Rank::Member`] holds — ServUO's `RankFlags.Member`.
    pub const MEMBER: Self = Self::REMOVE_LOWEST_RANK
        .with(Self::ACCESS_GUILD_ITEMS)
        .with(Self::CAN_VOTE);

    /// Everything — ServUO's `RankFlags.All`, which is [`MEMBER`](Self::MEMBER)
    /// and every other bit.
    pub const ALL: Self = Self::MEMBER
        .with(Self::CAN_INVITE)
        .with(Self::REMOVE_PLAYERS)
        .with(Self::CAN_PROMOTE_DEMOTE)
        .with(Self::CONTROL_WAR_STATUS)
        .with(Self::ALLIANCE_CONTROL)
        .with(Self::CAN_SET_GUILD_TITLE);

    /// Both sets of bits.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is set here.
    #[must_use]
    pub const fn has(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// What `rank` may do — ServUO's `RankDefinition.Ranks` table, row for row.
#[must_use]
pub const fn flags_of(rank: Rank) -> RankFlags {
    match rank {
        Rank::Ronin => RankFlags::NONE,
        Rank::Member => RankFlags::MEMBER,
        Rank::Emissary => {
            RankFlags::MEMBER
                .with(RankFlags::REMOVE_PLAYERS)
                .with(RankFlags::CAN_INVITE)
                .with(RankFlags::CAN_SET_GUILD_TITLE)
                .with(RankFlags::CAN_PROMOTE_DEMOTE)
        }
        Rank::Warlord => RankFlags::MEMBER.with(RankFlags::CONTROL_WAR_STATUS),
        Rank::Leader => RankFlags::ALL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table, asserted against the reference row by row. Worth spelling out
    /// rather than trusting the constructor above to read correctly: this is a
    /// port of a literal table, and a port of a literal table is exactly the
    /// kind of thing that is wrong in one cell.
    #[test]
    fn each_rank_holds_what_servuo_gives_it() {
        assert_eq!(flags_of(Rank::Ronin), RankFlags::NONE);
        assert_eq!(flags_of(Rank::Leader), RankFlags::ALL);

        let member = flags_of(Rank::Member);
        assert!(member.has(RankFlags::CAN_VOTE));
        assert!(member.has(RankFlags::REMOVE_LOWEST_RANK));
        assert!(member.has(RankFlags::ACCESS_GUILD_ITEMS));
        assert!(!member.has(RankFlags::CAN_INVITE));
        assert!(!member.has(RankFlags::REMOVE_PLAYERS));
    }

    /// The one this file's doc is about. Written as its own test because the
    /// intuition it refutes is the one somebody will act on.
    #[test]
    fn rank_order_is_not_permission_order() {
        let emissary = flags_of(Rank::Emissary);
        let warlord = flags_of(Rank::Warlord);
        assert!(Rank::Warlord > Rank::Emissary, "the Warlord is the higher rank");

        assert!(emissary.has(RankFlags::CAN_INVITE));
        assert!(!warlord.has(RankFlags::CAN_INVITE), "and may not invite");
        assert!(emissary.has(RankFlags::CAN_PROMOTE_DEMOTE));
        assert!(!warlord.has(RankFlags::CAN_PROMOTE_DEMOTE));

        assert!(warlord.has(RankFlags::CONTROL_WAR_STATUS));
        assert!(!emissary.has(RankFlags::CONTROL_WAR_STATUS), "nor declare a war");
    }

    /// Only the Leader. Kept apart from the table test because it is the one
    /// permission a guild cannot delegate, and a change to the table that
    /// quietly handed it to a Warlord would let a subordinate ally the guild.
    #[test]
    fn an_alliance_is_the_leaders_alone() {
        for rank in Rank::ALL {
            assert_eq!(
                flags_of(rank).has(RankFlags::ALLIANCE_CONTROL),
                rank == Rank::Leader,
                "{} and alliance control",
                rank.name()
            );
        }
    }

    #[test]
    fn the_ladder_ends_where_it_should() {
        assert_eq!(Rank::Ronin.below(), None, "a Ronin is the demotion floor");
        assert_eq!(Rank::Leader.above(), None);
        assert_eq!(Rank::Member.above(), Some(Rank::Emissary));
        assert_eq!(Rank::Warlord.below(), Some(Rank::Emissary));
        for rank in Rank::ALL {
            assert_eq!(Rank::from_number(rank.number()), Some(rank));
        }
        assert_eq!(Rank::from_number(5), None, "not clamped to Leader");
    }
}
