//! The world clock, and the light everyone sees by.
//!
//! Until now the ambient was permanent daylight: `0x4F` went out once at login
//! with a zero in it, and the only thing that ever changed it was Night Sight —
//! which is why that buff was documented as a visual no-op. This is the clock it
//! was waiting for.
//!
//! # The clock is the tick counter, not a wall clock
//!
//! Nothing inside a tick may read the time of day from the OS, or a replay stops
//! replaying. So the world's hour is *derived* from `state.ticks` plus a base
//! carried across restarts, at ServUO's rate (`Clocks.cs`: five real seconds to
//! the UO minute, so a UO day is two real hours). Two identical runs are at the
//! same hour on the same tick, and the light that falls out of it is the same
//! too.
//!
//! # One pass, both reasons
//!
//! A player's light changes for two unrelated reasons — the sun moved, or they
//! walked into a cave — and there is exactly one place that notices either: a
//! pass that computes the level each player should see and sends `0x4F` only when
//! it differs from what was last sent. That is deliberately *not* a call beside
//! every step and every buff expiry; the status bar makes the same argument in
//! `tick/status.rs`, and the persistence rule makes it about saving.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_magic as magic;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::{
    Light,
    LightLevel,
    Season,
    Weather,
    WeatherChange,
};
use openshard_state::components::Position;

use super::World;
use super::defaults::{
    LIGHT_DAY,
    LIGHT_NIGHT,
    LIGHT_NIGHTSIGHT,
};

impl World {
    /// The world clock, in UO minutes since this shard's epoch. Derived from the
    /// tick counter, so it is exact under replay; `clock_base` is what a restart
    /// restores so the night does not start over at every boot.
    #[must_use]
    pub fn clock_minutes(&self) -> u64 {
        let per_minute = self.state.gameplay.uo_minute_ticks.max(1);
        self.clock_base
            .saturating_add(self.state.ticks.raw() / per_minute)
    }

    /// Publish the world's hour on [`WorldState`] for the systems that read it.
    ///
    /// The longitude term is dropped here on purpose. The light *should* reach the
    /// east before the west, and `daylight_at` keeps that; but a rule — a shop's
    /// opening hours, a townsperson's routine — has to be one answer for the whole
    /// facet, or a shopkeeper and the customer standing in front of them can
    /// disagree about whether it is closing time.
    ///
    /// [`WorldState`]: openshard_state::WorldState
    pub(super) fn refresh_hour(&mut self) {
        self.state.hour = self.uo_time_at(0).0;
    }

    /// Start the clock from `minutes` rather than midnight — what the boot load
    /// hands back so a shard restarts at the hour it stopped.
    #[must_use]
    pub const fn with_clock_minutes(mut self, minutes: u64) -> Self {
        self.clock_base = minutes;
        self
    }

    /// The hour and minute at a longitude, as the client's own clock reckons it.
    ///
    /// The `x / 16` term is ServUO's (`Clock.GetTime`) and is not decoration: UO's
    /// world is wide enough that dawn reaches the east before the west, and a
    /// shard whose whole map flips to night in one instant reads as a light
    /// switch rather than a sunrise.
    #[must_use]
    pub fn uo_time_at(&self, x: u16) -> (u64, u64) {
        let total = self.clock_minutes() + u64::from(x) / 16;
        ((total / 60) % 24, total % 60)
    }

    /// The ambient light at a point, from the time of day alone.
    ///
    /// ServUO's `LightCycle.ComputeLevelFor`: night until 04:00, a two-hour climb
    /// to full day at 06:00, day until 22:00, and a two-hour fall back to night.
    /// The scale runs backwards — 0 is blinding, higher is darker — so the two
    /// ramps interpolate in opposite directions.
    #[must_use]
    pub fn daylight_at(&self, x: u16) -> Light {
        let (hours, minutes) = self.uo_time_at(x);
        let day = i64::from(LIGHT_DAY.0);
        let night = i64::from(LIGHT_NIGHT.0);
        let level = match hours {
            h if h < 4 => night,
            h if h < 6 => night + (((h - 4) * 60 + minutes) as i64 * (day - night)) / 120,
            h if h < 22 => day,
            h => day + (((h - 22) * 60 + minutes) as i64 * (night - day)) / 120,
        };
        let level = level.clamp(0, 0x1F);
        Light(u8::try_from(level).expect("a clamped light level is in the wire byte's range"))
    }

    /// The shard-wide weather at the current part of its simulated day.
    ///
    /// It is a pure clock-and-season function for the same reason daylight is:
    /// a replay must recreate the same sky, and a wall-clock or an unrecorded
    /// random roll would make two identical replays visibly disagree. A period
    /// is six UO hours — long enough to be weather rather than a flicker, short
    /// enough that a normal session sees it change. Season biases the choices
    /// without needing a second calendar before the existing season one turns.
    #[must_use]
    pub fn weather(&self) -> WeatherChange {
        let quarter = (self.clock_minutes() / (6 * 60)) % 4;
        let weather = match (self.state.gameplay.season, quarter) {
            (Season::Spring, 0 | 2) => Weather::Clear,
            (Season::Spring, 1) => Weather::Rain,
            (Season::Spring, _) => Weather::StormBrewing,
            (Season::Summer, 2) => Weather::StormBrewing,
            (Season::Summer, _) => Weather::Clear,
            (Season::Fall, 0 | 2) => Weather::Rain,
            (Season::Fall, 1) => Weather::Storm,
            (Season::Fall, _) => Weather::Clear,
            (Season::Winter, 2) => Weather::Clear,
            (Season::Winter, _) => Weather::Snow,
            (Season::Desolation, 0 | 2) => Weather::Storm,
            (Season::Desolation, _) => Weather::Clear,
        };
        let intensity = match weather {
            Weather::Rain => 112,
            Weather::StormBrewing => 144,
            Weather::Snow => 104,
            Weather::Storm => 208,
            Weather::Temperature | Weather::Clear => 0,
        };
        let temperature = match self.state.gameplay.season {
            Season::Spring => 12,
            Season::Summer => 24,
            Season::Fall => 8,
            Season::Winter => 0,
            Season::Desolation => 16,
        };
        WeatherChange {
            weather,
            intensity,
            temperature,
        }
    }

    /// The light level one mobile should be seeing right now.
    ///
    /// Precedence, brightest override first:
    ///
    /// 1. **Night Sight** — the buff exists to beat the dark, so it beats both the
    ///    hour and the cave.
    /// 2. **The region** — a dungeon is dark at noon, and says so in its own data
    ///    rather than in a rule here.
    /// 3. **The hour**, at this mobile's longitude.
    fn light_for(&self, entity: EntityId) -> Light {
        if magic::behaviour_buff(
            &self.state,
            entity,
            openshard_state::BehaviourBuffKind::NIGHT_SIGHT,
        )
        .is_some()
        {
            return LIGHT_NIGHTSIGHT;
        }
        if let Some(light) = self.state.region_of(entity).and_then(|region| region.light) {
            return Light(light);
        }
        let x = self
            .state
            .registry
            .get::<Position>(entity)
            .map_or(0, |Position(point): &Position| point.x);
        self.daylight_at(x)
    }

    /// Send every player whose light level has changed the new one, and nobody
    /// else. The one place `0x4F` goes out after login.
    pub(super) fn refresh_light(&mut self) {
        let changed: Vec<(ConnectionId, Light)> = self
            .state
            .players
            .iter()
            .filter_map(|(&connection, &entity)| {
                let level = self.light_for(entity);
                let remembered = self.state.connection(connection).and_then(|row| row.last_light);
                (remembered != Some(level)).then_some((connection, level))
            })
            .collect();
        for (connection, level) in changed {
            self.remember_light(connection, level);
            self.state
                .send_packet(connection, &ServerPacket::LightLevel(LightLevel { level }));
        }
    }

    /// Broadcast the next weather quarter once, at its exact simulated
    /// boundary. There is no remembered copy per connection: the condition is
    /// a pure function, and every connected player receives the same whole
    /// replacement packet. A player who logs in between boundaries gets that
    /// same answer in [`World::try_enter`](super::World::try_enter).
    pub(super) fn refresh_weather(&mut self) {
        let per_minute = self.state.gameplay.uo_minute_ticks.max(1);
        if !self.state.ticks.raw().is_multiple_of(per_minute) || !self.clock_minutes().is_multiple_of(6 * 60)
        {
            return;
        }
        let weather = self.weather();
        let connections: Vec<ConnectionId> = self.state.players.keys().copied().collect();
        for connection in connections {
            self.state
                .send_packet(connection, &ServerPacket::WeatherChange(weather));
        }
    }

    /// The light a player entering the world is told about, remembered so the
    /// refresh pass does not immediately send it again.
    pub(super) fn initial_light(&mut self, connection: ConnectionId) -> Light {
        let level = self
            .state
            .players
            .get(&connection)
            .map_or(LIGHT_DAY, |&entity| self.light_for(entity));
        self.remember_light(connection, level);
        level
    }

    /// Note what a connection has been told, so the refresh pass does not say it
    /// again. Nothing to remember for a connection the world is not holding — and
    /// nothing was sent to it either, `send_packet` having dropped it for the same
    /// reason.
    fn remember_light(&mut self, connection: ConnectionId, level: Light) {
        if let Some(row) = self.state.connection_mut(connection) {
            row.last_light = Some(level);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: u64) -> Light {
        super::super::tests::world()
            .with_clock_minutes(minute)
            .daylight_at(0)
    }

    #[test]
    fn daylight_changes_at_the_exact_dawn_and_dusk_boundaries() {
        assert_eq!(at(4 * 60 - 1), LIGHT_NIGHT, "night lasts through 03:59");
        assert_eq!(at(4 * 60), LIGHT_NIGHT, "the dawn ramp begins at night");
        assert!(at(6 * 60 - 1) > LIGHT_DAY, "the dawn ramp lasts through 05:59");
        assert_eq!(at(6 * 60), LIGHT_DAY, "full day begins at 06:00");

        assert_eq!(at(22 * 60 - 1), LIGHT_DAY, "full day lasts through 21:59");
        assert_eq!(at(22 * 60), LIGHT_DAY, "the dusk ramp begins at day");
        assert!(at(24 * 60 - 1) < LIGHT_NIGHT, "the dusk ramp lasts through 23:59");
        assert_eq!(at(24 * 60), LIGHT_NIGHT, "midnight is full night");
    }
}
