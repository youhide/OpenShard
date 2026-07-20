//! The skill window's wire side: filling it on login, following a gain, and
//! reading the lock arrows.
//!
//! The rules — the roll, the gain curve, the lock's meaning — are the `skills`
//! crate's. This is only the `0x3A` traffic that shows them to a client, kept
//! here in `world` because drawing a mobile's state on a screen is the world's
//! job, the same seam `send_status` (`0x11`) sits on.

use super::*;
use openshard_protocol::{
    encode_skill_update, encode_skills_full, skill_count, Feature, SkillEntry, SkillLock,
};
use openshard_skills::SkillRaised;
use openshard_state::components::Skills;

impl World {
    /// A client moved a skill's up/down/lock arrow: store it. ServUO's
    /// `SetLockNoRelay` — the client already redrew its own arrow, so nothing is
    /// sent back.
    pub(super) fn set_skill_lock(&mut self, connection: ConnectionId, skill: u8, lock: SkillLock) {
        let Some(&player) = self.state.players.get(&connection) else {
            return;
        };
        let mut skills = self
            .state
            .registry
            .get::<Skills>(player)
            .cloned()
            .unwrap_or_default();
        skills.set_lock(skill, lock);
        self.state.registry.insert(player, skills);
    }

    /// A mobile's whole skill line-up for the `0x3A` window: every skill the
    /// client of `version` knows, trained or not, at its value, lock and cap.
    /// `value` equals `base` until item/buff modifiers exist.
    fn skill_entries(&self, entity: EntityId, version: ClientVersion) -> Vec<SkillEntry> {
        let skills = self.state.registry.get::<Skills>(entity);
        let cap = self.state.gameplay.skill_cap;
        (0..skill_count(version) as u8)
            .map(|id| {
                let base = skills.map_or(0, |s| s.get(id));
                SkillEntry {
                    id,
                    value: base,
                    base,
                    lock: skills.map_or(SkillLock::Up, |s| s.lock(id)),
                    cap,
                }
            })
            .collect()
    }

    /// Send a player its whole skill list, to fill the window — on login, the
    /// way ServUO sends `SkillUpdate` on world entry.
    pub(super) fn send_skills(&mut self, connection: ConnectionId, entity: EntityId) {
        let Some(&Client { version, .. }) = self.state.registry.get::<Client>(entity) else {
            return;
        };
        let entries = self.skill_entries(entity, version);
        let packet = encode_skills_full(&entries, version.supports(Feature::SkillCaps));
        self.state.send(connection, packet);
    }

    /// Push the single-line `0x3A` update for each skill that rose this tick, so
    /// an open window follows the gain live. Reads the `SkillRaised` a gain emits.
    pub(super) fn send_skill_updates(&mut self) {
        let raised: Vec<SkillRaised> = self.state.bus.read(&mut self.raised).copied().collect();
        for event in raised {
            let Some(&Client {
                connection,
                version,
            }) = self.state.registry.get::<Client>(event.entity)
            else {
                continue; // a creature training a skill has no window to update
            };
            let entry = SkillEntry {
                id: event.skill,
                value: event.value,
                base: event.value,
                lock: self
                    .state
                    .registry
                    .get::<Skills>(event.entity)
                    .map_or(SkillLock::Up, |s| s.lock(event.skill)),
                cap: self.state.gameplay.skill_cap,
            };
            let packet = encode_skill_update(&entry, version.supports(Feature::SkillCaps));
            self.state.send(connection, packet);
        }
    }
}
