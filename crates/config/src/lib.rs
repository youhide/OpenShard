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

use std::fmt;
use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A whole shard configuration.
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Network and identity.
    pub server: ServerConfig,
    /// Where the client's map files live.
    #[serde(default)]
    pub world: WorldConfig,
    /// Accounts, for as long as there is no database.
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
    /// Where the world is kept between restarts.
    #[serde(default)]
    pub persistence: PersistenceConfig,
    /// The gameplay script the shard runs.
    #[serde(default)]
    pub scripting: ScriptingConfig,
    /// The rules knobs — combat era, timers, ranges — an operator tunes without a
    /// rebuild. The Sphere `sphere.ini` equivalents, validated at load.
    #[serde(default)]
    pub gameplay: GameplayConfig,
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
    /// (15000 pre-AoS, 40000 AoS, 80000 SE; ML ignores it).
    #[serde(default = "default_combat_era")]
    pub combat_era: u8,
    /// Sphere's `SpeedScaleFactor`: the numerator of the swing formula. Larger is
    /// slower. The pre-AoS default is 15000; AoS uses 40000, SE 80000.
    #[serde(default = "default_speed_scale_factor")]
    pub speed_scale_factor: u64,
    /// The ceiling any one skill trains to, in tenths (so `1000` is 100.0).
    #[serde(default = "default_skill_cap")]
    pub skill_cap: u16,
    /// How long an item lies on the ground before it rots, in seconds.
    #[serde(default = "default_decay_seconds")]
    pub decay_seconds: u64,
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
}

/// Whether combat [`combat_era`](GameplayConfig::combat_era) is one the swing
/// formula implements: Sphere custom (`0`), pre-AoS (`1`), AoS (`2`), SE (`3`) or
/// ML (`4`).
const fn combat_era_is_implemented(era: u8) -> bool {
    matches!(era, 0..=4)
}

fn default_combat_era() -> u8 {
    1
}
fn default_speed_scale_factor() -> u64 {
    15000
}
fn default_skill_cap() -> u16 {
    1000
}
fn default_decay_seconds() -> u64 {
    20 * 60
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

impl Default for GameplayConfig {
    fn default() -> Self {
        Self {
            combat_era: default_combat_era(),
            speed_scale_factor: default_speed_scale_factor(),
            skill_cap: default_skill_cap(),
            decay_seconds: default_decay_seconds(),
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
            lod: default_false(),
            lod_radius: default_lod_radius(),
            lod_idle_factor: default_lod_idle_factor(),
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

    /// Which facets to load from `client_files`: 0 is Felucca, then Trammel,
    /// Ilshenar, Malas, Tokuno, Ter Mur. Defaults to just the first. A character
    /// stays on the facet it is on — there is no travel between them yet.
    #[serde(default = "default_facets")]
    pub facets: Vec<u8>,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            client_files: String::new(),
            start: StartConfig::default(),
            facets: default_facets(),
        }
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
            database: String::new(),
            save_seconds: default_save_seconds(),
        }
    }
}

/// The gameplay script the shard runs.
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptingConfig {
    /// The script to load and hot-reload — a path to a `.js`/`.ts` file, or a
    /// *directory* (a pack) whose `.js` files are concatenated into one script.
    ///
    /// Empty means no scripting: the shard runs, mobiles move when clients ask,
    /// and nothing reacts on its own. A real mode, not a broken one — the same
    /// bargain as an empty map or an empty database — and the seam gameplay (§6)
    /// hangs off, so it is here from the start rather than retrofitted. The path is
    /// watched, so saving any file under it reloads the hooks in the live shard.
    #[serde(default)]
    pub main: String,
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
            name: "OpenShard".to_owned(),
            listen: SocketAddr::from(([0, 0, 0, 0], 2593)),
            advertise: SocketAddr::from(([127, 0, 0, 1], 2593)),
        }
    }
}

/// One account.
///
/// # Plaintext here, hashed once inside
///
/// The password sits in a file on disk. That is what a dev config is; it is not
/// a model for production. The binary hashes it (argon2) on the way into the
/// store, and never keeps the plaintext. See `openshard-login`'s `Accounts`
/// trait and its `password` module.
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
    pub name: String,
    /// The password, in plaintext. Hashed on first boot and then ignored; see
    /// the type docs.
    pub password: String,
    /// Character names on this account.
    #[serde(default)]
    pub characters: Vec<String>,
    /// The staff authority this account plays with: `"player"` (the default),
    /// `"gamemaster"`/`"gm"`, or `"administrator"`/`"admin"`. Parsed into an
    /// `AccessLevel` by the binary; an unrecognised value there is logged and
    /// treated as `player`, never a silent grant.
    #[serde(default)]
    pub access: String,
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
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file is not valid TOML, or does not match the schema.
    Parse {
        /// Which file.
        path: PathBuf,
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
    /// The shard name is empty, or will not fit its wire field.
    BadShardName {
        /// How long the name is.
        length: usize,
    },
    /// Two accounts share a name.
    DuplicateAccount {
        /// The name that appears twice.
        name: String,
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
    /// `gameplay.lod` is on but `lod_radius` is zero, so no creature would ever
    /// think — a player is never within zero tiles of one.
    ZeroLodRadius,
    /// `gameplay.lod` is on but `lod_idle_factor` is zero, which would leave a
    /// dozing creature's next-think unmoved and busy-loop the gate.
    ZeroLodIdleFactor,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "cannot parse {}: {source}", path.display()),
            Self::AdvertisedUnspecified => f.write_str(
                "server.advertise is a wildcard address; it must be the address clients dial \
                 (your public IP behind NAT, or 127.0.0.1 for a local-only shard) — it is not \
                 the same as server.listen",
            ),
            Self::AdvertisedPortZero => f.write_str("server.advertise needs a real port"),
            Self::BadShardName { length } => write!(
                f,
                "server.name is {length} bytes; it must be 1 to {MAX_SHARD_NAME} to fit the \
                 0xA8 packet",
            ),
            Self::DuplicateAccount { name } => write!(
                f,
                "two accounts are named {name:?}; names are case-insensitive"
            ),
            Self::EmptyAccountName => f.write_str("an account has an empty name"),
            Self::UnknownCombatEra { era } => write!(
                f,
                "gameplay.combat_era is {era}; only Sphere's 0 (custom), 1 (pre-AoS), \
                 2 (AoS), 3 (SE) and 4 (ML) are implemented",
            ),
            Self::ZeroSpeedScaleFactor => {
                f.write_str("gameplay.speed_scale_factor must not be zero")
            }
            Self::ZeroLodRadius => {
                f.write_str("gameplay.lod_radius must not be zero when gameplay.lod is on")
            }
            Self::ZeroLodIdleFactor => {
                f.write_str("gameplay.lod_idle_factor must be at least 1 when gameplay.lod is on")
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
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Check everything that would otherwise fail silently at runtime.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server.advertise.ip().is_unspecified() {
            return Err(ConfigError::AdvertisedUnspecified);
        }
        if self.server.advertise.port() == 0 {
            return Err(ConfigError::AdvertisedPortZero);
        }

        let length = self.server.name.len();
        if length == 0 || length > MAX_SHARD_NAME {
            return Err(ConfigError::BadShardName { length });
        }

        let mut seen: Vec<String> = Vec::new();
        for account in &self.accounts {
            if account.name.is_empty() {
                return Err(ConfigError::EmptyAccountName);
            }
            // Login lowercases names, so two accounts differing only in case
            // would collide at runtime, with one silently shadowing the other.
            let key = account.name.to_lowercase();
            if seen.contains(&key) {
                return Err(ConfigError::DuplicateAccount {
                    name: account.name.clone(),
                });
            }
            seen.push(key);
        }

        // An unimplemented era would silently fall through to era 1, giving a feel
        // the operator did not ask for; name it instead.
        if !combat_era_is_implemented(self.gameplay.combat_era) {
            return Err(ConfigError::UnknownCombatEra {
                era: self.gameplay.combat_era,
            });
        }
        // The swing formula divides by this; zero would panic mid-tick.
        if self.gameplay.speed_scale_factor == 0 {
            return Err(ConfigError::ZeroSpeedScaleFactor);
        }
        // LOD's two knobs only bite when it is on; a zero either freezes every
        // creature or spins the gate, so reject them rather than run them.
        if self.gameplay.lod {
            if self.gameplay.lod_radius == 0 {
                return Err(ConfigError::ZeroLodRadius);
            }
            if self.gameplay.lod_idle_factor == 0 {
                return Err(ConfigError::ZeroLodIdleFactor);
            }
        }
        Ok(())
    }

    /// The IPv4 address to advertise, which is all the `0x8C` packet can carry.
    ///
    /// `None` for an IPv6 `advertise`. The UO protocol has no way to express
    /// one — the relay packet has four bytes for an address, and that is the
    /// whole of it.
    pub fn advertise_v4(&self) -> Option<SocketAddrV4> {
        match self.server.advertise.ip() {
            IpAddr::V4(address) => Some(SocketAddrV4::new(address, self.server.advertise.port())),
            IpAddr::V6(_) => None,
        }
    }
}

/// The config shipped with the project, as text.
///
/// Written out by the binary when there is no config file, so a fresh checkout
/// runs without anyone having to read the docs first.
pub const DEFAULT_TOML: &str = include_str!("default.toml");

#[cfg(test)]
mod tests;
