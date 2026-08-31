//! Guilds across the door: what goes to disk, and what comes back at boot.
//!
//! # Two halves, saved apart
//!
//! A guild is written here, as a record of its own. Who is *in* it is written
//! with the character, as a [`CharacterRecord`] field. That is not an accident of
//! layout: the two change on different schedules — a character is swept whenever
//! it is touched, a guild only on the full sweep — and a roster held in both
//! places would be two answers to one question.
//!
//! So the roster is derived on the way back in: every character restored with a
//! `guild` id names the guild, and nothing else does. A guild whose every member
//! is gone is a guild with no members, which is what it actually is.
//!
//! # What the high-water mark is for
//!
//! `Guilds::high_water` is restored from the world row, not re-derived from the
//! guilds themselves — see [`WorldRecord::guild_high_water`]. The maximum id in
//! the table is not the maximum ever issued, because a disbanded guild leaves no
//! row behind. A shard that re-derived it would hand the next guild founded an id
//! a disbanded one had already used, and every character record still naming that
//! id — a member who was offline when it disbanded, and so was never swept —
//! would silently find itself in the new guild.

use openshard_persistence::{
    AllianceRecord,
    GuildRecord,
    GuildStanding,
};
use openshard_state::guild::{
    Alliance,
    AllianceId,
    Guild,
    GuildId,
};
use tracing::info;

use super::World;

impl World {
    /// Every guild as a saveable record.
    ///
    /// A straight copy across the record seam, like a region: a guild is exactly
    /// its data, with no live timer to translate.
    pub(super) fn guild_records(&self) -> Vec<GuildRecord> {
        self.state
            .guilds
            .iter()
            .map(|guild| {
                GuildRecord {
                    id:           guild.id.0,
                    name:         guild.name.clone(),
                    abbreviation: guild.abbreviation.clone(),
                    leader:       guild.leader,
                    relations:    standings(guild.wars.iter()),
                    proposals:    standings(guild.war_offers.iter()),
                    alliance:     guild.alliance.map(|id| id.0),
                }
            })
            .collect()
    }

    /// Every alliance as a saveable record.
    pub(super) fn alliance_records(&self) -> Vec<AllianceRecord> {
        self.state
            .alliances
            .iter()
            .map(|alliance| {
                AllianceRecord {
                    id:      alliance.id.0,
                    name:    alliance.name.clone(),
                    leader:  alliance.leader.0,
                    members: alliance.members.iter().map(|guild| guild.0).collect(),
                    pending: alliance.pending.iter().map(|guild| guild.0).collect(),
                }
            })
            .collect()
    }

    /// Re-create the guilds from saved records at boot.
    ///
    /// Call once, before anyone connects, and **before** the characters are
    /// restored: a `GuildMember` component whose guild is not in the table yet
    /// reads as no membership, and while nothing at boot asks, the day something
    /// does the failure would be a whole shard of players quietly unguilded.
    pub fn restore_guilds(&mut self, records: Vec<GuildRecord>) {
        for record in &records {
            self.state.guilds.restore(Guild {
                id:           GuildId(record.id),
                name:         record.name.clone(),
                abbreviation: record.abbreviation.clone(),
                leader:       record.leader,
                wars:         wars(&record.relations),
                war_offers:   wars(&record.proposals),
                alliance:     record.alliance.map(AllianceId),
            });
        }
        if !records.is_empty() {
            info!(guilds = records.len(), "restored the shard's guilds");
        }
    }

    /// Re-create the alliances from saved records at boot.
    ///
    /// Order does not matter against [`restore_guilds`](Self::restore_guilds):
    /// each side names the other by id and neither validates the link at boot,
    /// for the reason that file's docs give — a guild naming an alliance that is
    /// gone reads as no alliance, which is the same rule a membership naming a
    /// disbanded guild has.
    pub fn restore_alliances(&mut self, records: Vec<AllianceRecord>) {
        for record in &records {
            self.state.alliances.restore(Alliance {
                id:      AllianceId(record.id),
                name:    record.name.clone(),
                leader:  GuildId(record.leader),
                members: record.members.iter().copied().map(GuildId).collect(),
                pending: record.pending.iter().copied().map(GuildId).collect(),
            });
        }
        if !records.is_empty() {
            info!(alliances = records.len(), "restored the shard's alliances");
        }
    }

    /// Restore the alliance id counter from the world row. See
    /// [`with_guild_high_water`](Self::with_guild_high_water).
    #[must_use]
    pub fn with_alliance_high_water(mut self, id: u32) -> Self {
        self.state.alliances.set_high_water(id);
        self
    }

    /// Restore the id counter from the world row.
    ///
    /// Separate from [`restore_guilds`](Self::restore_guilds) because the number
    /// is in a different row and arrives later in the boot — and it is safe in
    /// either order: `restore` has already raised the counter past every id it
    /// put back, and `set_high_water` never lowers it. So a store whose world row
    /// is missing or stale still cannot re-issue an id that is plainly in use;
    /// the row is the authority, the restored guilds are the floor.
    #[must_use]
    pub fn with_guild_high_water(mut self, id: u32) -> Self {
        self.state.guilds.set_high_water(id);
        self
    }
}

/// A guild's wars, as they go to disk.
///
/// `at_war` is always true now — it is the only standing there is, since being
/// allied became membership of a named group. Written rather than dropped for
/// the reason [`GuildStanding`]'s own doc gives.
fn standings<'a>(wars: impl Iterator<Item = &'a GuildId>) -> Vec<GuildStanding> {
    wars.map(|&other| {
        GuildStanding {
            other:  other.0,
            at_war: true,
        }
    })
    .collect()
}

/// And back again.
///
/// A standing that says `at_war: false` is one this engine no longer writes —
/// which cannot arrive, because the schema version refuses a database old enough
/// to hold one. Read as a war anyway rather than skipped: the row exists, the
/// two guilds meant *something* by it, and the version check is what actually
/// keeps them out.
fn wars(standings: &[GuildStanding]) -> std::collections::BTreeSet<GuildId> {
    standings.iter().map(|standing| GuildId(standing.other)).collect()
}
