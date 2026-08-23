//! Waking a sector when a player walks into it.
//!
//! # The bug this exists for
//!
//! LOD is what lets a full facet be simulated at all: a mobile no player is near
//! skips its beat and pushes the next one out by `lod_idle_factor` — sixteen
//! seconds at the defaults. The saving is real and the mechanism is right. What
//! was missing is the other half of it.
//!
//! Nothing woke a dozing mobile. It was never told a player had arrived; it
//! simply finished a long timer that had been set while nobody was there. So a
//! player walking into a town found a still tableau, and up to sixteen seconds
//! later it burst into life — all at once, because mobiles that doze together
//! wake together. "The NPCs only start acting when I get close" is what a missing
//! wake looks like from the client, and no amount of shortening the doze fixes it
//! without giving the saving back.
//!
//! # Sphere's answer, which is the one taken here
//!
//! Sphere sleeps whole sectors and wakes them as an *event*. `CSector::_CanSleep`
//! reasons about a sector and its eight neighbours (`fCheckAdjacents`), so a
//! sector is alive before a player crosses into it; and `CChar::_GoAwake` re-arms
//! each woken NPC to a random short delay, with the comment saying exactly why:
//! *"make it tick randomly in the next sector, so all awaken NPCs get a different
//! tick time."* The wake is therefore both halves at once — prompt, and staggered.
//!
//! # Why a crossing and not a per-tick scan
//!
//! A pass that asked "is a player near?" for every dozing mobile every tick would
//! undo the saving LOD exists for. A player crosses a sector boundary once every
//! sixty-four tiles, so diffing the sector each player stands in costs one lookup
//! per player per tick and the wake itself runs on the rare tick it changes. The
//! same find-it-by-diffing shape as `tick/regions.rs` and `tick/status.rs`, and
//! for the same reason: a call beside every mover is a call somewhere it is
//! forgotten.

use openshard_entities::EntityId;
use openshard_protocol::world::Facet;
use openshard_state::components::{Brain, Npc, Position};

use super::World;

impl World {
    /// Wake the sector block around any player who has just entered one.
    pub(super) fn sector_wakes(&mut self) {
        if !self.state.gameplay.lod {
            // Nothing dozes, so nothing needs waking.
            return;
        }
        let mut crossed: Vec<(EntityId, Facet)> = Vec::new();
        for &player in self.state.players.values() {
            let Some(&Position(at)) = self.state.registry.get::<Position>(player) else {
                continue;
            };
            let facet = self.state.facet_of(player);
            let sector = self.state.facet_state(facet).sectors().sector_of(at);
            // A player with no remembered sector has just arrived — logged in,
            // been resurrected, been teleported — and that is a crossing too.
            // `.0` because `player_sectors` remembers the pair as raw numbers.
            if self.player_sectors.insert(player, (facet.0, sector)) != Some((facet.0, sector)) {
                crossed.push((player, facet));
            }
        }

        for (player, facet) in crossed {
            let Some(&Position(at)) = self.state.registry.get::<Position>(player) else {
                continue;
            };
            let sleepers: Vec<EntityId> = self
                .state
                .facet_state(facet)
                .sectors()
                .mobiles_in_block(at)
                .map(|(entity, _)| entity)
                .collect();
            for entity in sleepers {
                self.wake(entity);
            }
        }
    }

    /// Pull a dozing mobile's next beat forward, if it is dozing.
    ///
    /// Only ever *forward*: a mobile already beating at its live rate keeps the
    /// beat it has. Re-arming everything on sight would reset the whole block's
    /// timers to one instant every time a player crossed a boundary — which is
    /// the lockstep this and `npc::next_beat` are both here to prevent.
    fn wake(&mut self, entity: EntityId) {
        let now = self.state.ticks;
        if let Some(npc) = self.state.registry.get::<Npc>(entity).copied() {
            let awake = openshard_npc::BEAT_TICKS;
            if npc.next_beat > now + awake {
                let armed = openshard_npc::first_beat(&mut self.state.rng, now, awake);
                self.state.registry.insert(
                    entity,
                    Npc {
                        next_beat: armed,
                        ..npc
                    },
                );
            }
        }
        let awake = self.brain_beat(entity);
        if let Some(&Brain { next_think, .. }) = self.state.registry.get::<Brain>(entity) {
            if next_think > now + awake {
                let armed = openshard_npc::first_beat(&mut self.state.rng, now, awake);
                if let Some(brain) = self.state.registry.get_mut::<Brain>(entity) {
                    brain.next_think = armed;
                }
            }
        }
    }

    /// Forget a player's remembered sector, so their next tick reads as an
    /// arrival. For logout, and for anything that moves someone a long way
    /// without walking them there.
    pub(super) fn forget_sector(&mut self, player: EntityId) {
        self.player_sectors.remove(&player);
    }
}
