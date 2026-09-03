//! TOML configuration loading and validation.
//!
//! # Validation is the point
//!
//! Loading a TOML file is three lines of `serde`. The reason this is a crate is
//! everything after: a shard that starts with a subtly wrong config and *looks*
//! fine is worse than one that refuses to start. The failure mode this exists
//! to prevent is [`ServerConfig::advertise`] — get it wrong and every client
//! connects, logs in, picks a shard, and then silently fails to reach the game
//! server, with nothing in the log to say why.
//!
//! So `load` validates, and the errors say what to do about it.
//!
//! ```
//! use openshard_config::Config;
//!
//! let config: Config = toml::from_str(r#"
//!     [server]
//!     name = "OpenShard"
//!     listen = "0.0.0.0:2593"
//!     advertise = "203.0.113.10:2593"
//!
//!     [[accounts]]
//!     name = "admin"
//!     password = "hunter2"
//!     characters = ["Lord British"]
//!     access = "gamemaster"
//! "#).unwrap();
//!
//! config.validate().unwrap();
//! assert_eq!(config.server.name, "OpenShard");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::net::{
    IpAddr,
    SocketAddr,
    SocketAddrV4,
};
use std::path::{
    Path,
    PathBuf,
};

use openshard_protocol::identity::{
    AccountName,
    CharacterName,
    PlaintextPassword,
};
use openshard_protocol::world::{
    Facet,
    Season,
};
use serde::{
    Deserialize,
    Serialize,
};

/// A whole shard configuration.
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Network and identity.
    pub server:      ServerConfig,
    /// Where the client's map files live.
    #[serde(default)]
    pub world:       WorldConfig,
    /// Accounts, for as long as there is no database.
    #[serde(default)]
    pub accounts:    Vec<AccountConfig>,
    /// Where the world is kept between restarts.
    #[serde(default)]
    pub persistence: PersistenceConfig,
    /// The rules knobs — combat era, timers, ranges — an operator tunes without a
    /// rebuild. The Sphere `sphere.ini` equivalents, validated at load.
    #[serde(default)]
    pub gameplay:    GameplayConfig,
    /// What the shard does when it stops keeping its own declared tick rate.
    #[serde(default)]
    pub watchdog:    WatchdogConfig,
    /// Where the shard answers questions about itself, if anywhere.
    #[serde(default)]
    pub metrics:     MetricsConfig,
}

/// What the shard does when it stops being the thing it says it is.
///
/// # Why a shard that is merely slow is worth stopping
///
/// Every duration this shard puts on the wire is a tick count converted at the
/// declared rate — a bow's `impact_in`, an animation's stretch, a decay, a
/// criminal flag. A tick that is not 25ms long therefore does not make the shard
/// *late*: it makes every number the shard says **wrong**, in a way no packet in
/// the protocol lets a client notice and no tick-by-tick test can see, because
/// both sides of what such a test compares are counted in the unit that slipped.
///
/// That is what happened: an outfit lookup that walked the whole world dropped
/// the tick to 6.9 of its declared 40, a bow announced 1600ms and landed at
/// 6500ms, and the shard reported nothing at all because in its own units it was
/// perfectly correct. A shard in that state is not degraded, it is lying, and it
/// will go on lying for as long as nobody happens to read a log line.
///
/// # Why it stops rather than panics
///
/// A panic in the tick loop drops the task that runs it, and that task is the
/// one that writes the world on its way out — so a panic here would answer a
/// silent lie with silent data loss. Asking for the same stop that Ctrl-C asks
/// for gives the same fail-fast and keeps the world: the shard says why, saves,
/// and exits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchdogConfig {
    /// How many consecutive one-second windows the shard may run behind its
    /// declared tick rate before it stops itself. `0` turns the stop off, and
    /// the shard only says so in the log.
    ///
    /// **Consecutive windows and not one long tick**, because those are different
    /// events with different causes. A single overrun is the outside world — a
    /// disk that paused, a save that took nine seconds, a machine that had
    /// something else to do — and killing a shard for it would trade a hiccup
    /// for an outage. A minute of them is the shard itself, and no amount of
    /// waiting fixes it.
    #[serde(default = "default_tick_behind_windows")]
    pub tick_behind_windows: u32,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            tick_behind_windows: default_tick_behind_windows(),
        }
    }
}

/// Where the shard answers questions about itself.
///
/// # Why a shard publishes nothing until it is told to
///
/// Binding a port is a decision about the machine the shard runs on, and the
/// only safe default for one is *none*: a shard that opened a second port on
/// every interface the moment this crate learned how would be a shard that
/// started publishing its connection count to whoever could reach it, on an
/// upgrade nobody read the notes for. So the absence here is the domain's — a
/// shard with no endpoint is a working shard, and the common one — rather than a
/// value waiting to be filled in.
///
/// What it publishes has no authentication and is not meant to. See
/// `openshard_metrics::endpoint`, and prefer a loopback or a private interface;
/// the shard says so at boot when it is bound anywhere else.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    /// The socket serving `GET /metrics` and `GET /health`, or nothing at all.
    ///
    /// Not [`ServerConfig::listen`]: that is the game port, and the two cannot
    /// share a socket — see [`ConfigError::MetricsListenIsGamePort`], which
    /// refuses the mistake at load rather than letting the second bind fail
    /// after the shard is already up.
    #[serde(default)]
    pub listen: Option<SocketAddr>,
}

impl MetricsConfig {
    /// A shard that answers no questions about itself, which is what every
    /// config that does not mention the section asks for.
    ///
    /// Named rather than left to a derive, because "no endpoint" is a decision
    /// with a paragraph behind it — see the type — and a `#[derive(Default)]`
    /// says only that somebody wanted the field to compile.
    pub const fn publishing_nothing() -> Self {
        Self { listen: None }
    }
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self::publishing_nothing()
    }
}

/// A minute of windows.
///
/// Long enough that nothing an operator does on purpose trips it — a world save,
/// a base-set import, a `.populate` over a whole facet are all seconds and not
/// minutes — and short enough that nobody plays a whole session against a shard
/// whose every announced interval is wrong. The defect this was written for held
/// for as long as the shard was up.
const fn default_tick_behind_windows() -> u32 {
    60
}

/// The gameplay rules an operator tunes: the numbers that were compile-time
/// constants until an operator needed one different.
///
/// # Why these live in config and the packet lengths do not
///
/// A wire format is not a choice — get the `0x1A` layout wrong and no client
/// draws the item, so it is code, pinned by a test. These are choices: how fast a
/// blow lands, how long an item lies before it rots, how far a whisper carries.
/// SphereServer exposes exactly this set in `sphere.ini` (`CombatEra`,
/// `SpeedScaleFactor`, `DecayTimer`, `DistanceWhisper`…) for the same reason —
/// two shards running the same binary want different feels.
///
/// Times are in **seconds**, not ticks: an operator thinks in seconds, and the
/// world converts to its tick counter at construction, so the tick stays the only
/// place that knows the rate.
///
/// Note this is a different axis from a client's `Era` in `openshard-protocol`:
/// that is which *packets* a client version understands, never branched on for
/// rules; this is which *rules* the shard runs, never seen on the wire.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameplayConfig {
    /// Which swing-speed formula combat uses, Sphere's `m_iCombatSpeedEra`:
    /// `0` (Sphere custom), `1` (pre-AoS), `2` (AoS), `3` (SE) and `4` (ML) are all
    /// implemented — each turns dexterity and a weapon's era-appropriate speed
    /// (`old`/`aos`/`ml`) into a swing interval. Anything else is rejected rather
    /// than silently run as pre-AoS. Set `speed_scale_factor` to match the era
    /// (15000 classic pre-AoS, 40000 AoS, 80000 SE; ML ignores it).
    #[serde(default = "default_combat_era")]
    pub combat_era: CombatEra,
    /// Sphere's `SpeedScaleFactor`: the numerator of the swing formula. Larger is
    /// slower. OpenShard's quicker pre-AoS default is 10000 (the classic value
    /// is 15000); AoS uses 40000, SE 80000.
    #[serde(default = "default_speed_scale_factor")]
    pub speed_scale_factor: u64,
    /// Chance, in per-mille, that a landed weapon or ranged blow is critical.
    /// This is a shard-specific extension rather than a classic-UO rule: `50`
    /// is 5%.  Set zero for strictly classic damage rolls.
    #[serde(default = "default_critical_chance")]
    pub critical_chance: u16,
    /// Damage a critical blow deals, as a percentage of its normally scaled
    /// damage. `150` is one and a half times; values below 100 are rejected so
    /// enabling a critical can never make a landed hit weaker.
    #[serde(default = "default_critical_damage_percent")]
    pub critical_damage_percent: u16,
    /// The ceiling any one skill trains to, in tenths (so `1000` is 100.0).
    #[serde(default = "default_skill_cap")]
    pub skill_cap: u16,
    /// The ceiling on every skill added together, in tenths — the classic
    /// `7000` (700.0). ServUO's `PlayerCaps.TotalSkillCap`, and what makes a
    /// character a build: at the cap a skill only rises if one set to "down"
    /// gives ground.
    #[serde(default = "default_total_skill_cap")]
    pub total_skill_cap: u32,
    /// The ceiling on strength, dexterity and intelligence added together — the
    /// classic `225`. Read by the stat gain, and shown on the status bar.
    #[serde(default = "default_stat_cap")]
    pub stat_cap: u16,
    /// The ceiling on any one stat — the classic `125`.
    #[serde(default = "default_stat_cap_individual")]
    pub stat_cap_individual: u16,
    /// How long after a stat rises before it may rise again, in milliseconds.
    /// ServUO ships its fifteen-minute delay switched off, leaving the `500` its
    /// config falls back to; raise it for a shard that wants stat gain to be a
    /// long haul.
    #[serde(default = "default_stat_gain_ms")]
    pub stat_gain_ms: u64,
    /// The chance, in per-mille, that a skill gain also tries for a stat. Only
    /// the ML mechanic (`combat_era = 4`) reads it — ServUO's
    /// `PlayerChanceToGainStats`, 5%; below ML each stat rolls its own weight
    /// from the skill table instead.
    #[serde(default = "default_stat_gain_chance")]
    pub stat_gain_chance: u32,
    /// How long an item lies on the ground before it rots, in seconds. `0`
    /// disables cleanup of loose ground items and corpses.
    #[serde(default = "default_decay_seconds")]
    pub decay_seconds: u64,
    /// How long a house stands without being refreshed before it collapses, in
    /// seconds. ServUO's five days. `0` turns house decay off entirely, which is
    /// what a shard that never wants a plot to free up sets.
    #[serde(default = "default_house_decay_seconds")]
    pub house_decay_seconds: u64,
    /// How long a criminal flag lasts after a grey act, in seconds.
    #[serde(default = "default_criminal_seconds")]
    pub criminal_seconds: u64,
    /// How far normal speech carries, in tiles. Sphere's `DistanceTalk`.
    #[serde(default = "default_distance_talk")]
    pub distance_talk: u32,
    /// How far a whisper carries, in tiles. Sphere's `DistanceWhisper`.
    #[serde(default = "default_distance_whisper")]
    pub distance_whisper: u32,
    /// How far a yell carries, in tiles. Sphere's `DistanceYell`.
    #[serde(default = "default_distance_yell")]
    pub distance_yell: u32,
    /// Milliseconds between a hunting creature's steps. 400 is the classic
    /// base-monster pace — slower than a running player (250), so running away
    /// works, as it always has. Set 250 to let monsters keep pace with a
    /// runner. Idle creatures amble at twice this.
    #[serde(default = "default_creature_step_ms")]
    pub creature_step_ms: u64,
    /// How a spell is cast. `"servuo"` (the default) is the UO original: the
    /// caster stops, says the words over a cast delay, and the target cursor
    /// comes up only after — then it may move again. `"sphere"` is Sphere's feel:
    /// the spell resolves as it is cast, with no rooting, so the caster keeps
    /// walking. The Sphere-vs-ServUO knob the whole spell system reads.
    #[serde(default = "default_cast_style")]
    pub cast_style: String,
    /// Whether taking damage while casting disturbs the spell — UO's fizzle. Only
    /// bites in the `"servuo"` cast style, where there is a cast delay to
    /// interrupt. `true` is the UO/ServUO original; `false` lets a cast finish
    /// through the hits, Sphere-style.
    #[serde(default = "default_spell_disturb")]
    pub spell_disturb: bool,
    /// AoS object tooltips (the "cliloc" hover names), Sphere's `TOOLTIPMODE`.
    /// `"version"` (the default) sends only a revision when a thing is drawn and
    /// waits for the client to ask for the full list — the bandwidth-cheap
    /// standard. `"full"` sends the whole tooltip up front. `"off"` disables them
    /// and does not advertise AoS, so a modern client falls back to the classic
    /// single-click name label. The knob that picks the modern-vs-classic feel.
    #[serde(default = "default_tooltips")]
    pub tooltips: String,
    /// Whether the server offers right-click / single-click context menus (the
    /// `0xBF` popup). `true` answers a context-menu request with the object's
    /// default entries (open a container, a vendor's buy/sell, a paperdoll);
    /// `false` serves none, and — with `tooltips = "off"` — leaves the classic
    /// client on plain single-click names.
    #[serde(default = "default_context_menus")]
    pub context_menus: bool,
    /// Whether spells require and consume reagents at all. `true` (the default) is
    /// classic UO — a spell fizzles without its reagents in the pack, and a
    /// successful cast spends them. `false` casts from mana alone, Sphere's
    /// no-reagent shards. Independent of the cast style.
    #[serde(default = "default_true")]
    pub reagents: bool,
    /// Whether a *failed* cast still spends mana — Sphere's `ManaLossFail`, and
    /// the axis it confirmed: mana and reagents are spent at resolution, once
    /// success or failure is known, so this decides what a fizzle costs. `true`
    /// (the default) is the UO/ServUO original — a fizzle burns the mana;
    /// `false` refunds it. A successful cast always spends.
    #[serde(default = "default_true")]
    pub mana_loss_on_fail: bool,
    /// Whether a *failed* cast still consumes reagents — Sphere's `ReagentLossFail`.
    /// `true` (the default) is the UO/ServUO original; `false` keeps the reagents
    /// when the cast fizzles. Only meaningful when [`reagents`](Self::reagents) is
    /// on. A successful cast always consumes.
    #[serde(default = "default_true")]
    pub reagent_loss_on_fail: bool,
    /// Whether the status bar's gold field adds what is in the bank box. `false`
    /// (the default) is what UO does: ServUO marks the box a virtual item, so its
    /// gold never reaches the character's total — which is the reason a banker has
    /// to *say* your balance. `true` sums pack and bank, a convenience some shards
    /// prefer. Weight is never affected: banked goods are not carried whatever
    /// this says, or banking a pile would make you overweight.
    #[serde(default = "default_false")]
    pub bank_gold_in_status: bool,
    /// Whether a purchase from an NPC vendor falls back to the bank box when the
    /// backpack is short. `true` (the default) is UO and ServUO's `BaseVendor`,
    /// which tries the pack and then the bank and says which paid; `false` keeps
    /// the money strictly in hand, so a bank balance buys nothing.
    #[serde(default = "default_true")]
    pub vendor_bank_payment: bool,
    /// Whether Recall and Gate Travel may take you to another facet. `false`
    /// (the default) is the classic pre-AoS rule ServUO keeps — a rune marked in
    /// Ilshenar is a rune you walk to — and `true` is the behaviour from AoS on.
    /// The engine can move a mobile between facets either way; this decides only
    /// whether the two spells are allowed to.
    #[serde(default = "default_false")]
    pub cross_facet_travel: bool,
    /// Level-of-detail: when `true`, a creature with no player within
    /// [`lod_radius`](Self::lod_radius) stops paying for the full AI decision
    /// (line-of-sight, target scan, pathfinding) each beat — it dozes at a
    /// stretched beat instead. `false` (the default) simulates every creature at
    /// full rate, whether or not anyone is near. Opt-in: it trades a little
    /// off-screen liveliness for tick budget in a populated world.
    #[serde(default = "default_false")]
    pub lod: bool,
    /// How close (tiles, Chebyshev) a player must be for a creature to think at
    /// full rate under [`lod`](Self::lod). Kept comfortably above the view range
    /// (18) and the largest creature sight, so a creature a player can see is
    /// never dozed. Only meaningful when `lod` is on.
    #[serde(default = "default_lod_radius")]
    pub lod_radius: u32,
    /// How much to stretch a dozing creature's beat under [`lod`](Self::lod): its
    /// next think is pushed out this many times its normal beat. `8` is eight
    /// times slower. Only meaningful when `lod` is on; must be at least 1.
    #[serde(default = "default_lod_idle_factor")]
    pub lod_idle_factor: u64,
    /// How many real seconds one UO minute lasts — how fast the day/night cycle
    /// runs. `5` (the default) is ServUO's rate and puts a whole UO day in two
    /// real hours: dawn around 04:00 UO, dusk around 22:00. A larger number slows
    /// the sun down; `0` is refused, since a stopped clock is permanent midnight.
    #[serde(default = "default_uo_minute_seconds")]
    pub uo_minute_seconds: u64,
    /// Which season the client draws. Sent once, on world entry — there is no
    /// calendar turning it yet.
    #[serde(default = "default_season", with = "season")]
    pub season: Season,
    /// Whether guards answer in the regions marked guarded. `true` (the default)
    /// is a town where a criminal is punished; `false` is ServUO's per-region
    /// `Disabled` applied shard-wide, for a shard that wants no safe ground.
    #[serde(default = "default_true")]
    pub guards: bool,
    /// Which expansion the shard tells the client it is — `"aos"`, `"se"` or
    /// `"ml"` (the default).
    ///
    /// This is not decoration: the client draws its paperdoll from what it is
    /// told the shard supports, so a shard that says "AoS" has **no Quest button
    /// on the paperdoll** however well the server answers one. `"ml"` is the
    /// default because that is where the quest system this engine implements
    /// comes from; drop to `"aos"` for a pre-ML feel and reach the quest log
    /// through the context menu instead.
    #[serde(default = "default_expansion")]
    pub expansion: String,
    /// Whether townsfolk keep a daily routine — at their posts by day, at home by
    /// night. Off by default.
    ///
    /// Marked as ours rather than a port: neither reference ties an NPC to the
    /// clock. ServUO's nearest equivalent is a hand-placed `WayPoint` chain a
    /// builder walks an NPC along, with no notion of the hour. It also does nothing
    /// until the pack gives its NPCs a home to go to, so turning it on alone is
    /// safe.
    #[serde(default)]
    pub npc_schedule: bool,
    /// The hour townsfolk arrive at their posts, with `npc_schedule` on.
    #[serde(default = "default_npc_work_hour")]
    pub npc_work_hour: u8,
    /// The hour townsfolk leave for home, with `npc_schedule` on. Must be after
    /// `npc_work_hour` and under 24 — a working day that wraps midnight is
    /// rejected at load, so nothing downstream has to reason about one.
    #[serde(default = "default_npc_home_hour")]
    pub npc_home_hour: u8,
    /// What the world does to a combat action that is already running — the
    /// condition/effect table of `docs/combat/design_actions.md`'s D4.
    #[serde(default = "default_action_rules")]
    pub action_rules: ActionRulesConfig,
    /// How a running action's interval divides into the named stretches a
    /// watcher is told about — raising, loading, releasing.
    #[serde(default = "default_action_stages")]
    pub action_stages: ActionStagesConfig,
    /// How long each kind of action takes, as a percentage of what the weapon
    /// table and the swing formula come to on their own.
    #[serde(default = "default_action_speed")]
    pub action_speed: ActionSpeedsConfig,
}

/// How long each kind of combat action takes, as a percentage of the interval
/// the weapon's own row and [`speed_scale_factor`](GameplayConfig::speed_scale_factor)
/// produce.
///
/// `100` is the formula's own answer, `50` is twice as fast, `200` is half. It
/// is the third table keyed by *kind* rather than by weapon, beside
/// [`ActionRulesConfig`] and [`ActionStagesConfig`], and for the same reason
/// they are: *"archery on this shard is quicker than the reference makes it"* is
/// one line an operator can read and disagree with, where a column edited into
/// fifty weapon rows is fifty places for two of them to come apart.
///
/// **Why a percentage and not the interval itself.** The interval is derived
/// from dexterity and the weapon, and both of those still have to matter — a
/// shard that wrote `shot = 1600ms` would have made every bow, every archer and
/// every point of dexterity the same speed. This scales that answer; it does not
/// replace it.
///
/// ```toml
/// [gameplay.action_speed]
/// swing  = 100   # blows keep the formula's pace
/// shot   = 64    # a default-dexterity bow: 2.5s becomes 1.6s
/// breath = 100
/// ```
///
/// An explicit `SwingSpeed` on a mobile — a script pinning an exact cadence for
/// one creature — is **not** scaled by this. It is already the last word on that
/// mobile's pace, and a percentage applied on top of it would mean a script
/// could no longer say what it says.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSpeedsConfig {
    /// A blow's pace.
    #[serde(default = "default_swing_speed_percent")]
    pub swing:  u16,
    /// A shot's pace.
    #[serde(default = "default_shot_speed_percent")]
    pub shot:   u16,
    /// An innate ranged attack's pace.
    #[serde(default = "default_breath_speed_percent")]
    pub breath: u16,
}

/// Where one kind of action's named stretches begin, as percentages of the whole
/// interval.
///
/// Two numbers and not three: the release is whatever is left, so the shard can
/// never be configured into an action that ends before it lands. Each is the
/// *share* of the interval that stretch occupies, in order, and their sum must
/// not pass 100 — a file that oversubscribes the interval is rejected at load
/// rather than silently truncated.
///
/// ```toml
/// [gameplay.action_stages.shot]
/// ready = 10   # the bow comes up
/// load  = 70   # the string is drawn
/// # the remaining 20% is the loose
/// ```
///
/// **There is no `aim` share, and that is a decision rather than an omission.**
/// Aiming is *holding*, and an action that is running through an interval is not
/// holding anything: its impact is coming whether or not anybody waits for it. A
/// third share between the draw and the loose therefore read on screen as a
/// stretch where the archer had finished drawing and the arrow had not left —
/// which is a delay with no cause, and it was reported as one. Holding is what
/// [`ActionStage::Aim`](openshard_protocol::feedback::ActionStage::Aim) is for,
/// and an armed action is the only thing that does it.
///
/// A share left out of a row is `0`, which is a real answer — a kind whose whole
/// interval is one long release writes nothing at all. As with
/// [`ConditionRulesConfig`], a row an operator writes is the *whole* row: the
/// shipped shares are not merged back into it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StageSharesConfig {
    /// Bringing the weapon up.
    #[serde(default)]
    pub ready: u8,
    /// The effort — bending the bow, cocking the arm.
    #[serde(default)]
    pub load:  u8,
}

/// The whole stage table, keyed by what the action is.
///
/// Keyed by kind for [`ActionRulesConfig`]'s reason: what a *shot* is made of is
/// one line an operator can read, where a column on every weapon row is fifty
/// places for two of them to disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionStagesConfig {
    /// How a blow divides.
    #[serde(default = "default_swing_stages")]
    pub swing:  StageSharesConfig,
    /// How a shot divides.
    #[serde(default = "default_shot_stages")]
    pub shot:   StageSharesConfig,
    /// How an innate ranged attack divides.
    #[serde(default = "default_breath_stages")]
    pub breath: StageSharesConfig,
}

/// What happens to a running combat action when a condition catches it.
///
/// Written as the effect's own name, with its number where it needs one:
///
/// ```toml
/// [gameplay.action_rules.shot]
/// running = { sway = { penalty = 25 } }
/// struck  = "break"
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionEffectConfig {
    /// The action ends, interrupted, and the actor is told which condition
    /// spoiled it.
    Break,
    /// The impact is pushed out by this percentage of the time it still had to
    /// run. `100` doubles what is left.
    Slow {
        /// How much of the remaining time to add.
        percent: u16,
    },
    /// Taken off the hit roll, as a signed percentage of the base chance.
    /// Negative steadies rather than sways — that is how "an archer steadies on
    /// horseback" is written.
    Sway {
        /// What to take off the chance.
        penalty: i16,
    },
}

/// One kind of action's rules: a condition that is absent has **no rule**, which
/// is a real answer — walking is free for an archer on the shipped shard.
///
/// A row an operator writes is the *whole* row for that kind: conditions left
/// out of it are no rule, not the shipped default. Half a row silently merged
/// with a default is the `..Default::default()` hazard in another costume — the
/// operator would be reading a table that is not the one the shard runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionRulesConfig {
    /// What a step taken at a run does.
    #[serde(default)]
    pub running: Option<ActionEffectConfig>,
    /// What a step that was not a run does.
    #[serde(default)]
    pub walking: Option<ActionEffectConfig>,
    /// What being on a mount does, charged at the step.
    #[serde(default)]
    pub mounted: Option<ActionEffectConfig>,
    /// What a wound taken while the action runs does.
    #[serde(default)]
    pub struck:  Option<ActionEffectConfig>,
    /// What losing the line to the committed target does.
    #[serde(default)]
    pub blinded: Option<ActionEffectConfig>,
}

/// The whole table, keyed by what the action is.
///
/// Keyed by kind rather than by weapon: what a run does to *a shot* is one line
/// an operator can read, where a column on every ranged weapon row would be
/// fifty places for two of them to disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRulesConfig {
    /// A blow's rules.
    #[serde(default = "default_swing_rules")]
    pub swing:  ConditionRulesConfig,
    /// A shot's rules.
    #[serde(default = "default_shot_rules")]
    pub shot:   ConditionRulesConfig,
    /// An innate ranged attack's rules — a dragon's breath.
    #[serde(default = "default_breath_rules")]
    pub breath: ConditionRulesConfig,
}

/// The expansions a shard may advertise, in order.
///
/// The names only; the `0xB9` masks they map to live in the server, because this
/// crate deliberately knows nothing about the protocol. Not cosmetic either way:
/// the client builds its paperdoll from what it is told, so the Quest and Guild
/// buttons exist only under `"ml"`.
pub const EXPANSIONS: [&str; 3] = ["aos", "se", "ml"];

/// Whether an expansion name is one the shard can advertise.
#[must_use]
pub fn expansion_is_known(expansion: &str) -> bool {
    let name = expansion.trim().to_ascii_lowercase();
    EXPANSIONS.contains(&name.as_str())
}

/// The combat-rule era selected by [`GameplayConfig`].
///
/// This remains a numeric value at the configuration boundary for compatibility
/// with Sphere's `CombatEra` setting, while making the field's meaning explicit
/// in Rust. Unknown values are retained so [`Config::validate`] can report the
/// same actionable configuration error it did when this was a bare `u8`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct CombatEra(u8);

impl CombatEra {
    /// Keep a numeric Sphere era, including an unknown value for validation to report.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Returns the numeric Sphere value used in the configuration file.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Whether combat [`combat_era`](GameplayConfig::combat_era) is one the swing
/// formula implements: Sphere custom (`0`), pre-AoS (`1`), AoS (`2`), SE (`3`) or
/// ML (`4`).
const fn combat_era_is_implemented(era: CombatEra) -> bool {
    matches!(era.0, 0..=4)
}

fn default_combat_era() -> CombatEra {
    CombatEra(1)
}
fn default_speed_scale_factor() -> u64 {
    10000
}
fn default_critical_chance() -> u16 {
    50
}
fn default_critical_damage_percent() -> u16 {
    150
}
fn default_skill_cap() -> u16 {
    1000
}
fn default_total_skill_cap() -> u32 {
    7000
}
fn default_stat_cap() -> u16 {
    225
}
fn default_stat_cap_individual() -> u16 {
    125
}
fn default_stat_gain_ms() -> u64 {
    500
}
fn default_stat_gain_chance() -> u32 {
    50
}
fn default_decay_seconds() -> u64 {
    20 * 60
}
fn default_house_decay_seconds() -> u64 {
    5 * 24 * 60 * 60
}
fn default_criminal_seconds() -> u64 {
    2 * 60
}
fn default_distance_talk() -> u32 {
    18
}
fn default_distance_whisper() -> u32 {
    3
}
fn default_creature_step_ms() -> u64 {
    400
}

fn default_distance_yell() -> u32 {
    31
}
fn default_cast_style() -> String {
    "servuo".to_owned()
}
fn default_spell_disturb() -> bool {
    true
}
fn default_tooltips() -> String {
    "version".to_owned()
}
fn default_context_menus() -> bool {
    true
}
/// The shared default for the spell-cost bools — reagents on, loss on fail on
/// (the UO/ServUO original).
fn default_true() -> bool {
    true
}
/// The shared default for opt-in flags that ship off — LOD.
fn default_false() -> bool {
    false
}
fn default_lod_radius() -> u32 {
    32
}
fn default_lod_idle_factor() -> u64 {
    8
}
/// ServUO's `Clock.SecondsPerUOMinute`.
fn default_uo_minute_seconds() -> u64 {
    5
}
/// Spring — the season a shard with no calendar sits in.
fn default_season() -> Season {
    Season::Spring
}

fn default_expansion() -> String {
    "ml".to_owned()
}

/// The hour a shop opens, with `npc_schedule` on.
fn default_npc_work_hour() -> u8 {
    7
}

/// The hour a shop closes, with `npc_schedule` on.
fn default_npc_home_hour() -> u8 {
    21
}

/// No rule for any condition — the base every shipped row is written from.
const fn no_rules() -> ConditionRulesConfig {
    ConditionRulesConfig {
        running: None,
        walking: None,
        mounted: None,
        struck:  None,
        blinded: None,
    }
}

/// A cut line ends the action. Today's behaviour written down: the sustain pass
/// used to end a swing on a bare loss of sight, with no table to route it
/// through and so no way for a shard to want anything else.
const fn default_swing_rules() -> ConditionRulesConfig {
    ConditionRulesConfig {
        blinded: Some(ActionEffectConfig::Break),
        ..no_rules()
    }
}

/// **Walking is free, running sways, a mount is neutral** — the three sentences
/// the shipped table is meant to read as, plus the cut line every kind breaks
/// on. Twenty-five is the same scale, and near enough the same size, as the
/// bonus an ambush from cover already carries.
const fn default_shot_rules() -> ConditionRulesConfig {
    ConditionRulesConfig {
        running: Some(ActionEffectConfig::Sway { penalty: 25 }),
        blinded: Some(ActionEffectConfig::Break),
        ..no_rules()
    }
}

/// A creature's own breath: nothing but the cut line stops it.
const fn default_breath_rules() -> ConditionRulesConfig {
    ConditionRulesConfig {
        blinded: Some(ActionEffectConfig::Break),
        ..no_rules()
    }
}

/// The most an `action_rules` row may push an impact out, as a percentage of
/// the time the action still had to run. Ten times over is already a swing that
/// takes half a minute; past it the setting stops being a slow and becomes a
/// silent cancellation.
pub const MAX_SLOW_PERCENT: u16 = 1000;

impl ConditionRulesConfig {
    /// Every effect in the row, for validation to walk.
    fn effects(&self) -> [Option<ActionEffectConfig>; 5] {
        [
            self.running,
            self.walking,
            self.mounted,
            self.struck,
            self.blinded,
        ]
    }
}

impl ActionRulesConfig {
    /// The table this build ships with, for a shard whose file says nothing
    /// about one. Named rather than a bare `default` because the systems crate
    /// holds the same three rows in its own vocabulary and a test compares them:
    /// two tables claiming to be the shipped one must be the shipped one.
    #[must_use]
    pub const fn shipped() -> Self {
        Self {
            swing:  default_swing_rules(),
            shot:   default_shot_rules(),
            breath: default_breath_rules(),
        }
    }
}

fn default_action_rules() -> ActionRulesConfig {
    ActionRulesConfig::shipped()
}

/// A blow: up, back, and through. The wind-up is the longest of the three
/// because it is the part a defender watches — a swing that were mostly *strike*
/// would be a telegraph nobody could read in time.
const fn default_swing_stages() -> StageSharesConfig {
    StageSharesConfig { ready: 15, load: 65 }
}

/// A bow: lift, draw, loose. The lift is short and the draw is nearly the whole
/// of it, which is the shape of the thing — an archer spends their interval
/// *bending the bow*, and the arrow leaves at the end of it.
const fn default_shot_stages() -> StageSharesConfig {
    StageSharesConfig { ready: 10, load: 70 }
}

/// A breath: rear back, fill, and let go. Mostly the filling, because that is
/// the part a creature is visibly doing.
const fn default_breath_stages() -> StageSharesConfig {
    StageSharesConfig { ready: 20, load: 60 }
}

impl StageSharesConfig {
    /// What the two shares add up to. The release is `100` minus this, so a sum
    /// past a hundred is the one way to write a row that cannot be run.
    #[must_use]
    pub const fn claimed(&self) -> u16 {
        self.ready as u16 + self.load as u16
    }
}

impl ActionStagesConfig {
    /// The table this build ships with, named for [`ActionRulesConfig::shipped`]'s
    /// reason: the systems crate holds the same three rows in its own vocabulary
    /// and a test compares them.
    #[must_use]
    pub const fn shipped() -> Self {
        Self {
            swing:  default_swing_stages(),
            shot:   default_shot_stages(),
            breath: default_breath_stages(),
        }
    }
}

fn default_action_stages() -> ActionStagesConfig {
    ActionStagesConfig::shipped()
}

/// A blow keeps the pace the weapon table and the era formula give it.
const fn default_swing_speed_percent() -> u16 {
    100
}

/// A shot does not. The pre-AoS column makes a bow at default dexterity take two
/// and a half seconds, and what that reads as on screen is a body standing still
/// between arrows — the reference's own archery is quicker than this table makes
/// it, and no integer in the `old_speed` column lands on a round interval. `64`
/// is the number that turns 2.5s into 1.6s, which is the pace a bow was asked to
/// have; every other dexterity and every other bow scales with it.
const fn default_shot_speed_percent() -> u16 {
    64
}

/// A breath keeps its creature's own pace: a `RangedAttack` is scripted per
/// creature already, so there is no weapon table making it wrong.
const fn default_breath_speed_percent() -> u16 {
    100
}

impl ActionSpeedsConfig {
    /// The table this build ships with.
    #[must_use]
    pub const fn shipped() -> Self {
        Self {
            swing:  default_swing_speed_percent(),
            shot:   default_shot_speed_percent(),
            breath: default_breath_speed_percent(),
        }
    }
}

fn default_action_speed() -> ActionSpeedsConfig {
    ActionSpeedsConfig::shipped()
}

impl Default for GameplayConfig {
    fn default() -> Self {
        Self {
            combat_era: default_combat_era(),
            speed_scale_factor: default_speed_scale_factor(),
            critical_chance: default_critical_chance(),
            critical_damage_percent: default_critical_damage_percent(),
            skill_cap: default_skill_cap(),
            total_skill_cap: default_total_skill_cap(),
            stat_cap: default_stat_cap(),
            stat_cap_individual: default_stat_cap_individual(),
            stat_gain_ms: default_stat_gain_ms(),
            stat_gain_chance: default_stat_gain_chance(),
            decay_seconds: default_decay_seconds(),
            house_decay_seconds: default_house_decay_seconds(),
            criminal_seconds: default_criminal_seconds(),
            distance_talk: default_distance_talk(),
            distance_whisper: default_distance_whisper(),
            distance_yell: default_distance_yell(),
            creature_step_ms: default_creature_step_ms(),
            cast_style: default_cast_style(),
            spell_disturb: default_spell_disturb(),
            tooltips: default_tooltips(),
            context_menus: default_context_menus(),
            reagents: default_true(),
            mana_loss_on_fail: default_true(),
            reagent_loss_on_fail: default_true(),
            bank_gold_in_status: default_false(),
            vendor_bank_payment: default_true(),
            cross_facet_travel: default_false(),
            lod: default_false(),
            lod_radius: default_lod_radius(),
            lod_idle_factor: default_lod_idle_factor(),
            uo_minute_seconds: default_uo_minute_seconds(),
            season: default_season(),
            guards: default_true(),
            npc_schedule: false,
            npc_work_hour: default_npc_work_hour(),
            npc_home_hour: default_npc_home_hour(),
            action_rules: default_action_rules(),
            action_stages: default_action_stages(),
            action_speed: default_action_speed(),
            expansion: default_expansion(),
        }
    }
}

/// Where to find the client's data files.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorldConfig {
    /// The client install directory: `map0LegacyMUL.uop`, `tiledata.mul` and so on.
    ///
    /// Empty means no map. The shard still runs and a player can still walk —
    /// on nothing, through everything. That is a development mode, not a
    /// feature, and the server says so at startup.
    #[serde(default)]
    pub client_files: String,

    /// Where a new character appears.
    ///
    /// Only x and y: the height is taken from the map at spawn. A configured `z`
    /// would be a second source of truth for something the map already knows,
    /// and getting it wrong by three units leaves a character unable to take a
    /// single step, with nothing in the log to say why.
    #[serde(default)]
    pub start: StartConfig,

    /// Which facets to load: 0 is Felucca, then Trammel, Ilshenar, Malas,
    /// Tokuno, Ter Mur. Defaults to just the first. A character stays on the
    /// facet it is on — there is no travel between them yet.
    #[serde(default = "default_facets")]
    pub facets: Vec<u8>,

    /// Where a facet's world comes from, when it does not come from
    /// `client_files`.
    ///
    /// One entry per facet, keyed by the same number [`WorldConfig::facets`]
    /// lists: `0 = "felucca.osbase"`. A facet named here is read from that base
    /// set — our own format, written by `openshard-map-import` — and a facet
    /// not named here is read out of the install, exactly as before. Nothing is
    /// derived from a file name and nothing is derived from `client_files`: a
    /// path guessed from the other setting is a shard silently running the
    /// wrong world.
    ///
    /// # It does not replace `client_files`
    ///
    /// A base set holds the **map** — the ground and the statics. It does not
    /// hold `tiledata.mul`, which says what a tile *is*, or the multis, which
    /// say what a house is; both are still read from the install. A config that
    /// names a base set and leaves `client_files` empty is refused rather than
    /// run, because a world whose tiles have no flags answers every movement
    /// question wrongly and looks like a bug in the walk.
    #[serde(default)]
    pub base_sets: BTreeMap<FacetKey, PathBuf>,

    /// Which facets let bodies walk through each other, where that is not what
    /// the facet's number meant in retail.
    ///
    /// ServUO's `MapRules.FreeMovement`. Keyed by facet like
    /// [`base_sets`](WorldConfig::base_sets), and a facet not named here keeps
    /// the retail answer for its number: off on facet 0 (Felucca, where a body
    /// in the way costs ten stamina to get past) and on everywhere else.
    ///
    /// # Setting it is choosing a stutter
    ///
    /// The stock client decides the same question for itself, hardcoded, with
    /// `_world.Map.Index == 0` — it is not told. So a facet whose answer here
    /// disagrees with its number is a facet where the client predicts one thing
    /// and the shard answers another, which a player feels as being snapped
    /// back a tile. That is a legitimate thing to want (a Felucca-ruleset shard
    /// running its world in slot 3) and it is not free; hence a setting, and not
    /// a guess.
    ///
    /// A table of its own rather than one entry in a per-facet rules table,
    /// because `FreeMovement` is the only one of `MapRules`' four flags this
    /// engine has a reader for. The other three are named in
    /// `openshard_state::facet_rules`, where the second one to grow a reader is
    /// what decides whether these become one table.
    #[serde(default)]
    pub free_movement: BTreeMap<FacetKey, bool>,

    /// The seed the world's roll generator starts a *fresh* world from.
    ///
    /// Absent means the engine's own default seed stands — this is an override,
    /// the way a region's `music` is, not a value every config must carry.
    ///
    /// # It only applies to a world with no save behind it
    ///
    /// A shard that has saved once restores where its generator *got to*, not
    /// where it started, so changing this on a live shard changes nothing. That is
    /// the point: rewinding the stream at every boot would repeat the previous
    /// run's rolls. Set it to reproduce a fresh world — a bug that only shows up
    /// with one sequence of rolls, a benchmark that must be comparable between
    /// runs — and leave it alone otherwise.
    #[serde(default)]
    pub seed: Option<u64>,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            client_files:  String::new(),
            start:         StartConfig::default(),
            facets:        default_facets(),
            base_sets:     BTreeMap::new(),
            free_movement: BTreeMap::new(),
            seed:          None,
        }
    }
}

impl WorldConfig {
    /// The base set configured for `facet`, if the operator named one.
    ///
    /// A facet not in the table comes out of the install exactly as before, so
    /// `None` is *the install* rather than an answer nobody has given yet — and
    /// every caller turns it straight into
    /// `openshard_movement::bake::WorldSource`, which is the enum that says so.
    ///
    /// An accessor rather than three call sites reaching into the map with a
    /// `FacetKey` of their own: the shard's boot, the playground's window and
    /// anything else that has to agree with them are asking one question.
    #[must_use]
    pub fn base_set(&self, facet: Facet) -> Option<&Path> {
        self.base_sets.get(&FacetKey(facet)).map(PathBuf::as_path)
    }
}

/// A facet number, as the key of a `world.base_sets` table.
///
/// TOML has no integer keys — a table's keys are strings, always — and serde
/// will not turn `"0"` into a `u8` on the way past. The choice is between a map
/// keyed by a `String` that every reader has to parse again (and that can hold
/// `"felucca"` without anybody noticing until boot) and one conversion with a
/// type around it. This is that type: it parses once, at the edge, and what
/// comes out the other side is a facet number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FacetKey(pub Facet);

impl Serialize for FacetKey {
    /// Back out as the string it came in as, so a config round-trips.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0.0)
    }
}

impl<'de> Deserialize<'de> for FacetKey {
    /// A string, because that is what a TOML key is — and a plain integer too,
    /// for a format that has them, so this type is not a TOML quirk leaking
    /// into everything that ever reads a config.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visit;
        impl serde::de::Visitor<'_> for Visit {
            type Value = FacetKey;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a facet number: 0 is Felucca, then Trammel, Ilshenar, Malas, Tokuno, Ter Mur")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<FacetKey, E> {
                value
                    .parse()
                    .map(|number| FacetKey(Facet(number)))
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Str(value), &self))
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<FacetKey, E> {
                u8::try_from(value)
                    .map(|number| FacetKey(Facet(number)))
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Unsigned(value), &self))
            }
        }
        d.deserialize_any(Visit)
    }
}

/// The facets loaded when the config does not say which: just Felucca.
fn default_facets() -> Vec<u8> {
    vec![0]
}

/// Where a new character appears.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartConfig {
    /// East-west tile.
    pub x: u16,
    /// North-south tile.
    pub y: u16,
}

impl Default for StartConfig {
    /// Open ground north-west of Britain.
    ///
    /// A default, not a fact. Facets differ — the classic Britain centre at
    /// (1475, 1774) is open water on some maps — so this is only right for the
    /// files it was picked against, and it is in config precisely so it can be
    /// wrong for you and fixable without a rebuild.
    fn default() -> Self {
        Self { x: 1363, y: 1600 }
    }
}

/// Where the world is kept between restarts.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceConfig {
    /// Where the world is kept: a SQLite file path, a `postgres://` URL, or empty
    /// to keep it in memory.
    ///
    /// Empty is a real mode, not a broken one: the shard runs and loses the
    /// world at stop, the same bargain as running with no map. Give it a value
    /// and characters survive a restart. The shape picks the backend — a
    /// `postgres://` (or `postgresql://`) URL connects to PostgreSQL, anything
    /// else is a SQLite file such as `openshard.db`. SQLite or PostgreSQL is the
    /// operator's choice, and neither is a tier.
    #[serde(default)]
    pub database: String,

    /// How often the world is saved, in seconds. `0` turns the periodic save off —
    /// the world is then written only on a clean shutdown and on a staff `.save`.
    ///
    /// A save is cheap and never stops the world (an instant snapshot, written by a
    /// task nothing waits on), so this is only how much play a crash may cost, not a
    /// pause anyone feels. The default is a few minutes; a busy shard tightens it.
    #[serde(default = "default_save_seconds")]
    pub save_seconds: u64,
}

/// The default periodic save interval, in seconds — a few minutes, tightened by
/// the operator on a busy shard.
fn default_save_seconds() -> u64 {
    180
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            database:     String::new(),
            save_seconds: default_save_seconds(),
        }
    }
}

/// Network and identity.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// What the shard calls itself in the shard list.
    pub name: String,

    /// The socket to bind.
    ///
    /// `0.0.0.0:2593` is the usual answer: listen on every interface.
    pub listen: SocketAddr,

    /// The address handed to clients in the `0x8C` relay.
    ///
    /// # This is not `listen`
    ///
    /// `listen` is where the server binds. `advertise` is what the server *tells
    /// a client to dial*. Different questions, usually different answers.
    ///
    /// Getting this wrong is the most likely way to end up with a shard nobody
    /// can reach, and it fails silently: the login conversation completes, the
    /// client is told to connect somewhere it cannot, and it gives up without
    /// sending another packet. Nothing appears in the server log, because
    /// nothing reaches the server.
    ///
    /// - Behind NAT, this is the public IP, not the LAN one.
    /// - On a laptop, `127.0.0.1` is right, and right only for that laptop.
    /// - `0.0.0.0` is never right. See [`ConfigError::AdvertisedUnspecified`].
    pub advertise: SocketAddr,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name:      "OpenShard".to_owned(),
            listen:    SocketAddr::from(([0, 0, 0, 0], 2593)),
            advertise: SocketAddr::from(([127, 0, 0, 1], 2593)),
        }
    }
}

/// (De)serialize an [`AccountName`] as the bare TOML string, no wrapper
/// object — this crate does not depend on `serde` inside `openshard-protocol`,
/// so the impl lives here instead of on the type.
mod account_name {
    use serde::{
        Deserialize,
        Deserializer,
        Serializer,
    };

    use super::AccountName;

    pub fn serialize<S: Serializer>(value: &AccountName, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<AccountName, D::Error> {
        String::deserialize(d).map(AccountName)
    }
}

/// (De)serialize a [`PlaintextPassword`] as the bare TOML string. See
/// [`account_name`] for why this lives here rather than on the type.
mod plaintext_password {
    use serde::{
        Deserialize,
        Deserializer,
        Serializer,
    };

    use super::PlaintextPassword;

    pub fn serialize<S: Serializer>(value: &PlaintextPassword, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PlaintextPassword, D::Error> {
        String::deserialize(d).map(PlaintextPassword)
    }
}

/// (De)serialize a [`Season`] as its wire byte, rejecting one the client
/// cannot draw at deserialize time rather than letting a sixth season through
/// to be caught later — [`Season::from_bits`] is total and silently falls
/// back to spring, which is right for a packet off the wire but wrong for a
/// config typo that deserves to be refused. See [`account_name`] for why this
/// lives here rather than on the type.
mod season {
    use serde::{
        Deserialize,
        Deserializer,
        Serializer,
    };

    use super::Season;

    pub fn serialize<S: Serializer>(value: &Season, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(value.to_bits())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Season, D::Error> {
        let bits = u8::deserialize(d)?;
        Season::try_from_bits(bits).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "gameplay.season {bits} is not a season the client draws (0 spring, \
                 1 summer, 2 fall, 3 winter, 4 desolation)"
            ))
        })
    }
}

/// (De)serialize a `Vec<CharacterName>` as a TOML array of bare strings. See
/// [`account_name`] for why this lives here rather than on the type.
mod character_names {
    use serde::{
        Deserialize,
        Deserializer,
        Serialize,
        Serializer,
    };

    use super::CharacterName;

    pub fn serialize<S: Serializer>(value: &[CharacterName], s: S) -> Result<S::Ok, S::Error> {
        let names: Vec<&str> = value.iter().map(|name| name.0.as_str()).collect();
        names.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<CharacterName>, D::Error> {
        Vec::<String>::deserialize(d).map(|names| names.into_iter().map(CharacterName).collect())
    }
}

/// The staff authority text an operator wrote in `[[accounts]] access = "..."`,
/// not yet validated against [`openshard_protocol::access::AccessLevel`].
///
/// Distinct from an already-parsed `AccessLevel` on purpose: an unrecognised
/// value here is a config typo the binary logs and treats as `player`, not a
/// deserialize-time error, so this type stays a total wrapper around whatever
/// text was in the file — see [`AccountConfig::access`].
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct RawAccessLevel(pub String);

/// One account.
///
/// # Plaintext here, hashed once inside
///
/// The password sits in a file on disk. That is what a dev config is; it is not
/// a model for production. The binary hashes it (argon2) on the way into the
/// store, and never keeps the plaintext. See `openshard-login`'s `DevAccounts`
/// type and its `password` module.
///
/// # It seeds, it does not override
///
/// A config account creates a store row only the first time the shard sees it.
/// After that the store is authoritative for the password: changing this line
/// does *not* change an existing account's password (there is no re-hash of a
/// row that already has one). To rotate a password, clear the account from the
/// store, not the config.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    /// The account name. Case-insensitive at login.
    #[serde(with = "account_name")]
    pub name:       AccountName,
    /// The password, in plaintext. Hashed on first boot and then ignored; see
    /// the type docs.
    #[serde(with = "plaintext_password")]
    pub password:   PlaintextPassword,
    /// Character names on this account.
    #[serde(default, with = "character_names")]
    pub characters: Vec<CharacterName>,
    /// The staff authority this account plays with: `"player"` (the default),
    /// `"gamemaster"`/`"gm"`, or `"administrator"`/`"admin"`.
    ///
    /// Kept as text, not parsed here: [`RawAccessLevel`] only distinguishes
    /// "an operator wrote this in the config" from an already-validated
    /// `AccessLevel`. The binary parses it into an `AccessLevel`; an
    /// unrecognised value there is logged and treated as `player`, never a
    /// silent grant.
    #[serde(default)]
    pub access:     RawAccessLevel,
}

/// The widest a shard name can be. The 0xA8 field is 32 bytes.
const MAX_SHARD_NAME: usize = 32;

/// The config could not be loaded, or is not usable.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigError {
    /// The file could not be read.
    Read {
        /// Which file.
        path:   PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file is not valid TOML, or does not match the schema.
    Parse {
        /// Which file.
        path:   PathBuf,
        /// Why.
        source: toml::de::Error,
    },
    /// `advertise` is a wildcard address.
    ///
    /// Its own variant because it is the mistake people actually make: copying
    /// `listen` into `advertise`. `0.0.0.0` means "every interface" to a server
    /// binding a socket, and means nothing at all to a client dialling one.
    AdvertisedUnspecified,
    /// `advertise` has no port.
    AdvertisedPortZero,
    /// `advertise` is IPv6.
    ///
    /// The `0x8C` relay packet has four bytes for an address and no way to
    /// carry more, so an IPv6 `advertise` can never reach a client. Caught
    /// here rather than left to `advertise_v4()` returning `None` at the
    /// point the packet is built, which is a boot-time mistake and deserves
    /// a boot-time error.
    AdvertisedNotIpv4,
    /// `metrics.listen` is the socket the game itself binds.
    ///
    /// Its own variant because the mistake is silent in the worst way: the
    /// second bind fails, and whichever of the two lost the race is the one an
    /// operator finds missing — a shard with no metrics, or a shard nobody can
    /// log in to, depending on the order things started in that run.
    MetricsListenIsGamePort {
        /// The socket both were pointed at.
        address: SocketAddr,
    },
    /// The shard name is empty, or will not fit its wire field.
    BadShardName {
        /// How long the name is.
        length: usize,
    },
    /// Two accounts share a name.
    DuplicateAccount {
        /// The name that appears twice.
        name: AccountName,
    },
    /// An account has no name.
    EmptyAccountName,
    /// `gameplay.combat_era` names an era the swing formula does not implement.
    UnknownCombatEra {
        /// The value given.
        era: u8,
    },
    /// `gameplay.speed_scale_factor` is zero, which the swing formula divides by.
    ZeroSpeedScaleFactor,
    /// `gameplay.critical_chance` names more than every landed blow.
    CriticalChanceTooHigh {
        /// The chance given, in per-mille.
        chance: u16,
    },
    /// `gameplay.stat_gain_chance` names more than every eligible skill gain.
    StatGainChanceTooHigh {
        /// The chance given, in per-mille.
        chance: u32,
    },
    /// A `gameplay.action_rules` row slows an action by so much that it would
    /// never land.
    SlowPercentTooHigh {
        /// Which kind of action the row is for.
        kind:    &'static str,
        /// The percentage given.
        percent: u16,
    },
    /// A `gameplay.action_stages` row claims more of the interval than there is,
    /// leaving the release — which is the part that lands — nothing at all.
    StageSharesOversubscribed {
        /// Which kind of action the row is for.
        kind:    &'static str,
        /// What the two shares add up to.
        claimed: u16,
    },
    /// A `gameplay.action_speed` row is zero, which would make that kind of
    /// action land on the tick it was committed on — no wind-up, no bar, no
    /// frame in which anything could spoil it.
    ZeroActionSpeed {
        /// Which kind of action the row is for.
        kind: &'static str,
    },
    /// `gameplay.critical_damage_percent` would make a critical weaker than a
    /// normal landed hit.
    CriticalDamageBelowNormal {
        /// The multiplier given, as a percentage.
        percent: u16,
    },
    /// `gameplay.lod` is on but `lod_radius` is zero, so no creature would ever
    /// think — a player is never within zero tiles of one.
    ZeroLodRadius,
    /// `gameplay.lod` is on but `lod_idle_factor` is zero, which would leave a
    /// dozing creature's next-think unmoved and busy-loop the gate.
    ZeroLodIdleFactor,
    /// `gameplay.uo_minute_seconds` is zero, which stops the world clock — a
    /// shard frozen at midnight, with no error to say why.
    ZeroUoMinuteSeconds,
    /// `gameplay.expansion` is not an expansion the shard can advertise.
    UnknownExpansion {
        /// The value given.
        expansion: String,
    },
    /// `gameplay.npc_work_hour`/`npc_home_hour` do not describe a working day that
    /// starts and ends on the same date. Rejected rather than wrapped, so the one
    /// comparison that reads them stays a comparison.
    BadNpcHours {
        /// The hour given for opening.
        work: u8,
        /// The hour given for closing.
        home: u8,
    },
    /// `gameplay.skill_cap` or `total_skill_cap` is zero. The gain chance reads
    /// the headroom under both as a fraction, so a zero divides by nothing.
    ZeroSkillCap,
    /// `gameplay.stat_cap` or `stat_cap_individual` is zero, which would leave
    /// every character unable to hold a single point of anything.
    ZeroStatCap,
    /// `gameplay.stat_cap_individual` is above `stat_cap`, so one stat is allowed
    /// more than all three together — a ceiling nothing can reach.
    StatCapBelowIndividual {
        /// The cap on all three stats.
        total:      u16,
        /// The cap given for one.
        individual: u16,
    },
    /// `world.base_sets` names a facet, and `world.client_files` is empty.
    ///
    /// A base set holds the map and nothing else: `tiledata.mul` is what says a
    /// tile is water, or a wall, or a stair, and without it every one of them is
    /// an unremarkable nothing. The shard would run, and every question about
    /// the ground would be answered wrongly — which reads as a broken walk
    /// rather than as a missing setting.
    BaseSetWithoutClientFiles {
        /// The first facet named, in facet order.
        facet: Facet,
    },
    /// `world.base_sets` names a facet `world.facets` does not load.
    ///
    /// A mistyped facet number would otherwise do nothing at all: the entry is
    /// ignored, the facet it was meant for loads out of the install, and the
    /// shard runs the world the operator was replacing.
    BaseSetForUnloadedFacet {
        /// The facet named.
        facet: Facet,
    },
    /// A `world.base_sets` entry has an empty path.
    ///
    /// `client_files` reads empty as "no map at all"; a base set has no such
    /// meaning, and an empty path here is a setting somebody started writing.
    EmptyBaseSetPath {
        /// The facet named.
        facet: Facet,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "cannot parse {}: {source}", path.display()),
            Self::AdvertisedUnspecified => {
                f.write_str(
                    "server.advertise is a wildcard address; it must be the address clients dial \
                 (your public IP behind NAT, or 127.0.0.1 for a local-only shard) — it is not \
                 the same as server.listen",
                )
            }
            Self::AdvertisedPortZero => f.write_str("server.advertise needs a real port"),
            Self::AdvertisedNotIpv4 => {
                f.write_str(
                    "server.advertise is IPv6; the UO relay packet has four bytes for an address \
                 and no way to carry one",
                )
            }
            Self::MetricsListenIsGamePort { address } => {
                write!(
                    f,
                    "metrics.listen and server.listen are both {address}; the metrics endpoint \
                 needs a port of its own, and it is not the one clients dial",
                )
            }
            Self::BadShardName { length } => {
                write!(
                    f,
                    "server.name is {length} bytes; it must be 1 to {MAX_SHARD_NAME} to fit the \
                 0xA8 packet",
                )
            }
            Self::DuplicateAccount { name } => {
                write!(
                    f,
                    "two accounts are named {:?}; names are case-insensitive",
                    name.0
                )
            }
            Self::EmptyAccountName => f.write_str("an account has an empty name"),
            Self::UnknownCombatEra { era } => {
                write!(
                    f,
                    "gameplay.combat_era is {era}; only Sphere's 0 (custom), 1 (pre-AoS), \
                 2 (AoS), 3 (SE) and 4 (ML) are implemented",
                )
            }
            Self::ZeroSpeedScaleFactor => f.write_str("gameplay.speed_scale_factor must not be zero"),
            Self::CriticalChanceTooHigh { chance } => {
                write!(
                    f,
                    "gameplay.critical_chance is {chance}; it must be at most 1000 per-mille"
                )
            }
            Self::StatGainChanceTooHigh { chance } => {
                write!(
                    f,
                    "gameplay.stat_gain_chance is {chance}; it must be at most 1000 per-mille"
                )
            }
            Self::SlowPercentTooHigh { kind, percent } => {
                write!(
                    f,
                    "gameplay.action_rules.{kind} slows by {percent}%; it must be at most \
                 {MAX_SLOW_PERCENT}% — beyond that the impact is pushed so far out that \
                 nobody watching will see the action land, which reads as a shard that \
                 swallowed the blow rather than as the setting doing its job"
                )
            }
            Self::StageSharesOversubscribed { kind, claimed } => {
                write!(
                    f,
                    "gameplay.action_stages.{kind} gives its stages {claimed}% of the interval; \
                 ready + load must be at most 100 — the release is what is left over, \
                 and it is the stretch the impact happens in"
                )
            }
            Self::ZeroActionSpeed { kind } => {
                write!(
                    f,
                    "gameplay.action_speed.{kind} is 0; it is a percentage of the interval \
                 the weapon formula gives, and zero is not a fast action but no action — \
                 the impact would land on the tick it was committed on, with nothing \
                 drawn, nothing measured and no moment in which it could be spoiled"
                )
            }
            Self::CriticalDamageBelowNormal { percent } => {
                write!(
                    f,
                    "gameplay.critical_damage_percent is {percent}; it must be at least 100"
                )
            }
            Self::ZeroLodRadius => {
                f.write_str("gameplay.lod_radius must not be zero when gameplay.lod is on")
            }
            Self::ZeroLodIdleFactor => {
                f.write_str("gameplay.lod_idle_factor must be at least 1 when gameplay.lod is on")
            }
            Self::ZeroUoMinuteSeconds => f.write_str("gameplay.uo_minute_seconds must be at least 1"),
            Self::UnknownExpansion { expansion } => {
                write!(f, "gameplay.expansion \"{expansion}\" is not one of aos, se, ml")
            }
            Self::BadNpcHours { work, home } => {
                write!(
                    f,
                    "gameplay.npc_work_hour {work} and npc_home_hour {home} must both be under 24 \
                 with work before home; a working day that wraps midnight is not supported",
                )
            }
            Self::ZeroSkillCap => {
                f.write_str(
                    "gameplay.skill_cap and total_skill_cap must not be zero; the skill gain \
                 chance is a fraction of the headroom under each",
                )
            }
            Self::ZeroStatCap => f.write_str("gameplay.stat_cap and stat_cap_individual must not be zero"),
            Self::StatCapBelowIndividual { total, individual } => {
                write!(
                    f,
                    "gameplay.stat_cap_individual {individual} is above stat_cap {total}; one \
                 stat cannot be allowed more than all three together",
                )
            }
            Self::BaseSetWithoutClientFiles { facet } => {
                write!(
                    f,
                    "world.base_sets names facet {}, but world.client_files is empty: a base set \
                 holds the map, and tiledata.mul still holds what a tile is",
                    facet.0,
                )
            }
            Self::BaseSetForUnloadedFacet { facet } => {
                write!(
                    f,
                    "world.base_sets names facet {}, which world.facets does not load; the entry \
                 would do nothing and the facet it was meant for would come from the install",
                    facet.0,
                )
            }
            Self::EmptyBaseSetPath { facet } => {
                write!(
                    f,
                    "world.base_sets has an empty path for facet {}; it must name the file \
                 openshard-map-import wrote",
                    facet.0,
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Config {
    /// Read and validate a config file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| {
            ConfigError::Read {
                path: path.to_owned(),
                source,
            }
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| {
            ConfigError::Parse {
                path: path.to_owned(),
                source,
            }
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Check everything that would otherwise fail silently at runtime.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_server(&self.server)?;
        validate_metrics(&self.metrics, &self.server)?;
        validate_accounts(&self.accounts)?;
        validate_gameplay(&self.gameplay)?;
        validate_world(&self.world)
    }

    /// The IPv4 address to advertise, which is all the `0x8C` packet can carry.
    ///
    /// `None` for an IPv6 `advertise` — but `validate()` already refuses one
    /// (see [`ConfigError::AdvertisedNotIpv4`]), so a `Config` that has been
    /// validated (as `load()` always does) never turns this `None` in
    /// practice. The `Option` stays because this reads the raw `server`
    /// field directly and does not itself know whether validation ran.
    pub fn advertise_v4(&self) -> Option<SocketAddrV4> {
        match self.server.advertise.ip() {
            IpAddr::V4(address) => Some(SocketAddrV4::new(address, self.server.advertise.port())),
            IpAddr::V6(_) => None,
        }
    }
}

fn validate_server(server: &ServerConfig) -> Result<(), ConfigError> {
    if server.advertise.ip().is_unspecified() {
        return Err(ConfigError::AdvertisedUnspecified);
    }
    if server.advertise.port() == 0 {
        return Err(ConfigError::AdvertisedPortZero);
    }
    if server.advertise.is_ipv6() {
        return Err(ConfigError::AdvertisedNotIpv4);
    }

    let length = server.name.len();
    if length == 0 || length > MAX_SHARD_NAME {
        return Err(ConfigError::BadShardName { length });
    }
    Ok(())
}

/// The one way the two listeners can be wrong about each other.
///
/// Only the exact socket is refused. A metrics endpoint on the same *port* as
/// the game but a different interface is a real configuration — `127.0.0.1:2593`
/// beside `192.168.1.10:2593` binds twice without conflict — and refusing it
/// would be this crate guessing at an operator's network rather than checking a
/// contradiction.
fn validate_metrics(metrics: &MetricsConfig, server: &ServerConfig) -> Result<(), ConfigError> {
    match metrics.listen {
        Some(address) if address == server.listen => Err(ConfigError::MetricsListenIsGamePort { address }),
        _ => Ok(()),
    }
}

fn validate_accounts(accounts: &[AccountConfig]) -> Result<(), ConfigError> {
    let mut seen: Vec<String> = Vec::new();
    for account in accounts {
        if account.name.0.is_empty() {
            return Err(ConfigError::EmptyAccountName);
        }
        // Login lowercases names, so two accounts differing only in case
        // would collide at runtime, with one silently shadowing the other.
        let key = account.name.normalized();
        if seen.contains(&key) {
            return Err(ConfigError::DuplicateAccount {
                name: account.name.clone(),
            });
        }
        seen.push(key);
    }
    Ok(())
}

fn validate_gameplay(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    validate_combat(gameplay)?;
    validate_lod(gameplay)?;
    validate_world_clock(gameplay)?;
    validate_expansion(gameplay)?;
    validate_caps(gameplay)
}

fn validate_combat(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // An unimplemented era would silently fall through to era 1, giving a feel
    // the operator did not ask for; name it instead.
    if !combat_era_is_implemented(gameplay.combat_era) {
        return Err(ConfigError::UnknownCombatEra {
            era: gameplay.combat_era.value(),
        });
    }
    // The swing formula divides by this; zero would panic mid-tick.
    if gameplay.speed_scale_factor == 0 {
        return Err(ConfigError::ZeroSpeedScaleFactor);
    }
    if gameplay.critical_chance > 1000 {
        return Err(ConfigError::CriticalChanceTooHigh {
            chance: gameplay.critical_chance,
        });
    }
    // The roll has exactly 1000 outcomes; a larger threshold would silently
    // behave exactly like 100%, hiding the operator's invalid percentage.
    if gameplay.stat_gain_chance > 1000 {
        return Err(ConfigError::StatGainChanceTooHigh {
            chance: gameplay.stat_gain_chance,
        });
    }
    validate_action_effects(gameplay)?;
    validate_action_stage_shares(gameplay)?;
    validate_action_speeds(gameplay)?;
    if gameplay.critical_damage_percent < 100 {
        return Err(ConfigError::CriticalDamageBelowNormal {
            percent: gameplay.critical_damage_percent,
        });
    }
    Ok(())
}

fn validate_action_effects(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // A slow is the one effect in the table with a number big enough to turn
    // the rule into a cancellation nobody asked for.
    for (kind, row) in [
        ("swing", &gameplay.action_rules.swing),
        ("shot", &gameplay.action_rules.shot),
        ("breath", &gameplay.action_rules.breath),
    ] {
        for effect in row.effects().into_iter().flatten() {
            if let ActionEffectConfig::Slow { percent } = effect {
                if percent > MAX_SLOW_PERCENT {
                    return Err(ConfigError::SlowPercentTooHigh { kind, percent });
                }
            }
        }
    }
    Ok(())
}

fn validate_action_stage_shares(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // The release is the remainder, so a row that spends the whole interval
    // on getting ready leaves the impact nowhere to happen.
    for (kind, shares) in [
        ("swing", &gameplay.action_stages.swing),
        ("shot", &gameplay.action_stages.shot),
        ("breath", &gameplay.action_stages.breath),
    ] {
        let claimed = shares.claimed();
        if claimed > 100 {
            return Err(ConfigError::StageSharesOversubscribed { kind, claimed });
        }
    }
    Ok(())
}

fn validate_action_speeds(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // A percentage of zero is not "instant", it is "no action at all": the
    // impact would fall on the commit tick, so nothing would ever be drawn,
    // measured or interrupted. Refused at load rather than run.
    for (kind, percent) in [
        ("swing", gameplay.action_speed.swing),
        ("shot", gameplay.action_speed.shot),
        ("breath", gameplay.action_speed.breath),
    ] {
        if percent == 0 {
            return Err(ConfigError::ZeroActionSpeed { kind });
        }
    }
    Ok(())
}

fn validate_lod(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // LOD's two knobs only bite when it is on; a zero either freezes every
    // creature or spins the gate, so reject them rather than run them.
    if gameplay.lod {
        if gameplay.lod_radius == 0 {
            return Err(ConfigError::ZeroLodRadius);
        }
        if gameplay.lod_idle_factor == 0 {
            return Err(ConfigError::ZeroLodIdleFactor);
        }
    }
    Ok(())
}

fn validate_world_clock(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // A UO minute of zero divides the tick counter by nothing and leaves the
    // world at midnight for ever.
    if gameplay.uo_minute_seconds == 0 {
        return Err(ConfigError::ZeroUoMinuteSeconds);
    }
    // A routine whose day wraps midnight would leave every NPC permanently at
    // one end of it, which reads as the setting doing nothing.
    if gameplay.npc_work_hour >= gameplay.npc_home_hour || gameplay.npc_home_hour > 23 {
        return Err(ConfigError::BadNpcHours {
            work: gameplay.npc_work_hour,
            home: gameplay.npc_home_hour,
        });
    }
    Ok(())
}

fn validate_expansion(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // An expansion the shard cannot name would silently advertise nothing,
    // and the client would quietly drop half its paperdoll.
    if !expansion_is_known(&gameplay.expansion) {
        return Err(ConfigError::UnknownExpansion {
            expansion: gameplay.expansion.clone(),
        });
    }
    // `gameplay.season`'s own deserialize already refuses a sixth season —
    // see the `season` module — so there is nothing left to check here.
    Ok(())
}

fn validate_caps(gameplay: &GameplayConfig) -> Result<(), ConfigError> {
    // The gain chance divides by the total skill cap, and a per-stat cap above
    // the total one is a ceiling that can never be reached — both read as the
    // caps "not working" rather than as a bad setting.
    if gameplay.total_skill_cap == 0 || gameplay.skill_cap == 0 {
        return Err(ConfigError::ZeroSkillCap);
    }
    if gameplay.stat_cap == 0 || gameplay.stat_cap_individual == 0 {
        return Err(ConfigError::ZeroStatCap);
    }
    if gameplay.stat_cap_individual > gameplay.stat_cap {
        return Err(ConfigError::StatCapBelowIndividual {
            total:      gameplay.stat_cap,
            individual: gameplay.stat_cap_individual,
        });
    }
    Ok(())
}

fn validate_world(world: &WorldConfig) -> Result<(), ConfigError> {
    // A base set replaces the *map* files and nothing else, and an entry
    // that names a facet nobody loads is a typo that leaves the old world
    // running. Both are checked here rather than at boot so that a config
    // this wrong never reaches a running shard — see each variant for what
    // it would otherwise look like.
    for (&FacetKey(facet), path) in &world.base_sets {
        if path.as_os_str().is_empty() {
            return Err(ConfigError::EmptyBaseSetPath { facet });
        }
        if world.client_files.trim().is_empty() {
            return Err(ConfigError::BaseSetWithoutClientFiles { facet });
        }
        if !world.facets.contains(&facet.0) {
            return Err(ConfigError::BaseSetForUnloadedFacet { facet });
        }
    }
    Ok(())
}

/// The config shipped with the project, as text.
///
/// Written out by the binary when there is no config file, so a fresh checkout
/// runs without anyone having to read the docs first.
pub const DEFAULT_TOML: &str = include_str!("default.toml");

#[cfg(test)]
mod tests;
