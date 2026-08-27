//! The status bar (`0x11`) — built from what a character actually carries, and
//! re-sent when it changes.
//!
//! Four of its numbers used to be constants: gold `0`, armour `0`, weight a flat
//! body weight, followers `0`. A player read them every session, and every one of
//! them was a lie the client had no way to check. They are **read-site
//! derivations** now, in the shape `combat::equipped_weapon` established: gold and
//! weight come from `items::carried` walking the pack, armour from
//! `combat::armor` summing what is worn, followers from the pets you have and the
//! mount under
//! the rider. Nothing is mirrored onto the mobile, so an item moving needs no
//! bookkeeping — and none of the item code has to know the status bar exists.
//!
//! Which is the point of [`World::refresh_statuses`]. Putting a "re-send the bar"
//! call beside every `insert` of a `Contained` would work and would decay: the
//! first system that moves an item without knowing about it drops the update
//! silently, exactly the failure the persistence rule warns about. Instead one
//! pass recomputes the derived numbers for each *online player* — a handful of
//! entities, walked off `state.players`, not the world — and sends only what
//! changed. It reads `state.ticks` and components, never a clock, so it replays.

use super::*;
use openshard_config::CombatEra;
use openshard_state::components::{Riding, body_is_female};

/// How often the derived numbers are recomputed, in ticks: twice a second. Fast
/// enough that gold falling out of a purchase reads as immediate, slow enough
/// that the pack walk is nothing next to the rest of the tick.
///
/// Derived from the tick rate rather than written out, because "twice a second"
/// is the decision and the tick count is only how it is spelled: as a bare `10`
/// it quietly became four times a second when the tick halved.
pub(super) const STATUS_REFRESH_TICKS: u64 = TICKS_PER_SECOND / 2;

// What this pass remembers between runs is `StatusSnapshot`, and it lives on the
// connection's row in `openshard_state` rather than here: it is about what a
// particular client was last told, not about what the character is.
use openshard_state::connection::StatusSnapshot;

impl World {
    /// What a mobile's status bar says right now.
    ///
    /// The one place the packet is built, so the entry send and the periodic
    /// refresh can never disagree about a number.
    pub(super) fn status_of(&self, entity: EntityId) -> Option<MobileStatus> {
        let serial = self.state.registry.serial_of(entity)?;
        let name = self
            .state
            .registry
            .get::<Name>(entity)
            .map_or_else(String::new, |n| n.0.clone());
        let stats = self.state.registry.get::<Stats>(entity).copied();
        let hits = self.state.registry.get::<Hitpoints>(entity).copied();
        let mana = self.state.registry.get::<Mana>(entity).copied();
        let stamina = self.state.registry.get::<Stamina>(entity).copied();
        let (strength, dexterity, intelligence) = stats
            .map_or((DEFAULT_HITPOINTS, DEFAULT_DEXTERITY, DEFAULT_MANA), |s| {
                (s.strength, s.dexterity, s.intelligence)
            });
        let hits = hits.map_or(
            Vitals {
                current: DEFAULT_HITPOINTS,
                max: DEFAULT_HITPOINTS,
            },
            |h| Vitals {
                current: h.current,
                max: h.max,
            },
        );
        let mana = mana.map_or(
            Vitals {
                current: DEFAULT_MANA,
                max: DEFAULT_MANA,
            },
            |m| Vitals {
                current: m.current,
                max: m.max,
            },
        );
        // The real pool if the mobile carries one; otherwise dexterity, so an NPC
        // or a bare test mobile still reads as able to run.
        let stamina = stamina.map_or(
            Vitals {
                current: dexterity,
                max: dexterity,
            },
            |s| Vitals {
                current: s.current,
                max: s.max,
            },
        );
        let derived = self.derived_status(entity);

        Some(MobileStatus {
            serial,
            name,
            hits,
            // The body says which paperdoll the client draws; the bar should agree
            // with it rather than call every character male.
            female: self
                .state
                .registry
                .get::<Body>(entity)
                .is_some_and(|body| body_is_female(body.id)),
            strength,
            dexterity,
            intelligence,
            stamina,
            mana,
            gold: derived.gold,
            armor: derived.armor,
            weight: derived.weight,
            max_weight: max_weight(strength),
            stat_cap: STAT_CAP,
            followers: derived.followers,
            followers_max: MAX_FOLLOWERS,
        })
    }

    /// The four numbers that come from what a mobile carries, wears and rides.
    pub(super) fn derived_status(&self, entity: EntityId) -> StatusSnapshot {
        self.derived_status_with(&items::contents_index(&self.state), entity)
    }

    /// The same against a containment index built once for several mobiles — one
    /// scan of the column for a whole refresh pass, rather than one per bag per
    /// player.
    fn derived_status_with(&self, contents: &items::Contents, entity: EntityId) -> StatusSnapshot {
        StatusSnapshot {
            // What is on the character, and — only if the operator asked for it —
            // what is in the bank as well. Off is UO's own answer (the box is
            // virtual, so its gold never reaches the total), which is why a banker
            // has to be asked for a balance.
            gold: items::total_gold_with(&self.state, contents, entity)
                + if self.state.gameplay.bank_gold_in_status {
                    items::banked_gold_with(&self.state, contents, entity)
                } else {
                    0
                },
            // Pre-AoS this field *is* the armour rating; from AoS the client
            // labels it physical resistance, which is the resistance component's
            // to answer (the AoS per-piece resist data is a separate port).
            armor: if self.state.gameplay.combat_era >= CombatEra::new(2) {
                self.state
                    .registry
                    .get::<Resistance>(entity)
                    .map_or(0, |r| u16::from(r.physical))
            } else {
                openshard_state::armor::worn_armor_rating(&self.state, entity)
            },
            weight: items::total_weight_with(&self.state, contents, entity, BODY_WEIGHT),
            // A mount takes a follower slot in both references. Real pet slots
            // wait on taming; this is the one follower the engine can have today,
            // and reporting it is truer than reporting none.
            // Pets plus the mount under you — a read-site derivation
            // (`skills::followers_of`), so a pet that dies or is released stops
            // counting the instant it does, with nothing to keep in step.
            followers: skills::followers_of(&self.state, entity),
        }
    }

    /// What one step costs this mobile in stamina, spent; `Some(message)` if it
    /// has none left to spend and the step must be refused.
    ///
    /// The weighing happens here because the two halves live in two crates:
    /// `items` knows what a pack weighs, `combat` knows what a pool is worth. The
    /// world holds the carry cap (`max_weight`, from strength) and the four-stone
    /// allowance, and hands `combat` the one number it needs — how far over the
    /// line the walker is.
    ///
    /// The weight comes from what the refresh pass last worked out, not from a
    /// fresh walk of the pack: a step happens up to ten times a second per player
    /// and weighing a pack is a scan of the containment column. Half a second of
    /// staleness costs at most one step's worth of fatigue in the wrong direction,
    /// which is a fair trade for not re-weighing a mule on every tile. Before the
    /// first pass has run there is nothing remembered, and it weighs once.
    pub(super) fn spend_step_stamina(&mut self, entity: EntityId, running: bool) -> Option<&'static str> {
        // Staff walk through everything, fatigue included.
        if self.state.is_staff(entity) {
            return None;
        }
        let strength = self
            .state
            .registry
            .get::<Stats>(entity)
            .map_or(DEFAULT_HITPOINTS, |stats| stats.strength);
        let carried = self
            .state
            .row_of(entity)
            .and_then(|row| row.last_status.as_ref())
            .map_or_else(
                || items::total_weight(&self.state, entity, BODY_WEIGHT),
                |remembered| remembered.weight,
            );
        let cap = max_weight(strength).saturating_add(combat::OVERLOAD_ALLOWANCE);
        let over = carried.saturating_sub(cap);
        let mounted = self.state.registry.has::<Riding>(entity);
        combat::spend_step_stamina(&mut self.state, entity, running, mounted, over)
    }

    /// Re-send the status bar to any online player whose derived numbers moved.
    ///
    /// Runs from the tick every [`STATUS_REFRESH_TICKS`]. The comparison is what
    /// keeps it quiet: a player standing still sends nothing, and a purchase
    /// sends one small packet within half a second of the gold changing.
    pub(super) fn refresh_statuses(&mut self) {
        if !self.state.ticks.is_multiple_of(STATUS_REFRESH_TICKS) {
            return;
        }
        let players: Vec<(ConnectionId, EntityId)> = self
            .state
            .players
            .iter()
            .map(|(&connection, &entity)| (connection, entity))
            .collect();
        // No sweep of the remembered numbers here any more: they live on the
        // connection's row and a connection that has gone took them with it. This
        // used to be a `retain` over the map, and it was the second hand-written
        // teardown of the same state — `disconnect` cleared it too.
        let contents = items::contents_index(&self.state);
        for (connection, entity) in players {
            let now = self.derived_status_with(&contents, entity);
            let Some(row) = self.state.connection_mut(connection) else {
                continue;
            };
            let unchanged = row.last_status == Some(now);
            // Remembered either way: the fatigue check reads this weight, and a
            // player whose numbers have not moved still needs one on file.
            row.last_status = Some(now);
            if !unchanged {
                self.send_status(connection, entity);
            }
        }
    }
}
