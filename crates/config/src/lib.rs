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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameplayConfig {
    /// Which swing-speed formula combat uses, Sphere's `m_iCombatSpeedEra`:
    /// `0` Sphere-custom pre-AoS, `1` pre-AoS, `2` AoS, `3` SE, `4` ML. The eras
    /// differ in how dexterity and weapon speed become a swing interval.
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
}

/// The highest combat era [`GameplayConfig::combat_era`] accepts — Sphere's ML.
const MAX_COMBAT_ERA: u8 = 4;

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
fn default_distance_yell() -> u32 {
    31
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
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
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
}

/// The gameplay script the shard runs.
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptingConfig {
    /// The script to load and hot-reload — a path to a `.js`/`.ts` file.
    ///
    /// Empty means no scripting: the shard runs, mobiles move when clients ask,
    /// and nothing reacts on its own. A real mode, not a broken one — the same
    /// bargain as an empty map or an empty database — and the seam gameplay (§6)
    /// hangs off, so it is here from the start rather than retrofitted. The file
    /// is watched, so saving it reloads the hooks in the live shard.
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
/// # Plaintext, and knowingly so
///
/// The password sits in a file on disk. That is what a dev config is; it is not
/// a model for production, where accounts belong in a database behind a slow
/// hash. See `openshard-login`'s `Accounts` trait.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    /// The account name. Case-insensitive at login.
    pub name: String,
    /// The password, in plaintext.
    pub password: String,
    /// Character names on this account.
    #[serde(default)]
    pub characters: Vec<String>,
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
    /// `gameplay.combat_era` is outside the range of formulas Sphere defines.
    UnknownCombatEra {
        /// The value given.
        era: u8,
    },
    /// `gameplay.speed_scale_factor` is zero, which the swing formula divides by.
    ZeroSpeedScaleFactor,
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
                "gameplay.combat_era is {era}; it must be 0 to {MAX_COMBAT_ERA} \
                 (0 Sphere pre-AoS, 1 pre-AoS, 2 AoS, 3 SE, 4 ML)",
            ),
            Self::ZeroSpeedScaleFactor => {
                f.write_str("gameplay.speed_scale_factor must not be zero")
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

        // A combat era outside the table would silently fall through to a default
        // formula, giving a feel the operator did not ask for; name it instead.
        if self.gameplay.combat_era > MAX_COMBAT_ERA {
            return Err(ConfigError::UnknownCombatEra {
                era: self.gameplay.combat_era,
            });
        }
        // The swing formula divides by this; zero would panic mid-tick.
        if self.gameplay.speed_scale_factor == 0 {
            return Err(ConfigError::ZeroSpeedScaleFactor);
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
pub const DEFAULT_TOML: &str = r#"# OpenShard configuration.

[server]
name = "OpenShard"

# Where to bind. 0.0.0.0 listens on every interface.
listen = "0.0.0.0:2593"

# What to TELL CLIENTS to dial, in the 0x8C relay packet.
#
# NOT the same as `listen`, and the single most likely reason a client hangs.
# `listen` is where this shard answers; `advertise` is the address it hands out.
# Get it wrong and the failure is silent from here: the client logs in fine,
# picks the shard, is told to dial somewhere it cannot reach, and sits on
# "logging into shard" until it times out. This end sees one connection and a
# disconnect, and nothing that looks like an error, because nothing here failed.
#
# The default only works if the client runs on THIS MACHINE. A client anywhere
# else — another PC, a VM, a phone — will dial its own 127.0.0.1 and find
# nothing.
#
#   client on this machine   127.0.0.1:2593
#   client on your LAN       this machine's LAN address, e.g. 192.168.1.10:2593
#   client over the internet your PUBLIC address, with 2593 forwarded
advertise = "127.0.0.1:2593"

[world]
# The client install directory, holding map0LegacyMUL.uop and tiledata.mul.
#
# Leave empty and the shard runs with no map at all: every step is allowed and
# players walk through walls and across water. Useful for testing the protocol,
# useless as a game.
client_files = ""

# Which facets to load: 0 is Felucca, then Trammel, Ilshenar, Malas, Tokuno,
# Ter Mur. A character stays on its facet; there is no travel between them yet.
facets = [0]

# Where a new character appears. The height comes from the map, not from here.
#
# This default is open ground on one facet and may be open water on yours.
[world.start]
x = 1363
y = 1600

[persistence]
# Where the world is kept. Leave empty to keep it in memory and lose it at stop;
# give it a value to have characters survive a restart. A "postgres://" URL uses
# PostgreSQL; anything else is a SQLite file path. Neither backend is a tier.
#
#   database = "openshard.db"
#   database = "postgres://user:pass@localhost/openshard"
database = ""

[scripting]
# The gameplay script the shard loads and hot-reloads. A path to a .js/.ts file.
#
# Leave empty and nothing reacts on its own: mobiles move when clients ask and
# no more. Point it at a script and the shard delivers domain events to it every
# tick and applies the commands it enqueues. The file is watched — save it and
# the hooks reload without a restart.
#
#   main = "scripts/main.js"
main = ""

[gameplay]
# The rules knobs — the SphereServer sphere.ini equivalents. Every value here has
# a working default; uncomment one only to change the shard's feel.

# Which swing-speed formula combat uses (Sphere's CombatEra):
#   0 Sphere pre-AoS   1 pre-AoS   2 AoS   3 SE   4 ML
# The eras differ in how dexterity and weapon speed become a swing interval.
combat_era = 1

# The swing formula's numerator (Sphere's SpeedScaleFactor). Larger is slower.
# Pre-AoS is 15000; AoS uses 40000, SE 80000.
speed_scale_factor = 15000

# The ceiling any one skill trains to, in tenths (1000 = 100.0).
skill_cap = 1000

# How long an item lies on the ground before it rots, in seconds.
decay_seconds = 1200

# How long a criminal flag lasts after a grey act, in seconds.
criminal_seconds = 120

# How far speech carries, in tiles: normal, a whisper, a yell.
distance_talk = 18
distance_whisper = 3
distance_yell = 31

# Development accounts, in plaintext, until there is a database.
[[accounts]]
name = "admin"
password = "hunter2"
characters = ["Lord British"]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        toml::from_str(toml).expect("the test TOML should parse")
    }

    const MINIMAL: &str = r#"
        [server]
        name = "OpenShard"
        listen = "0.0.0.0:2593"
        advertise = "127.0.0.1:2593"
    "#;

    #[test]
    fn parses_a_full_config() {
        let config = config(
            r#"
            [server]
            name = "Britannia"
            listen = "0.0.0.0:2593"
            advertise = "203.0.113.10:2593"

            [[accounts]]
            name = "admin"
            password = "hunter2"
            characters = ["Lord British", "Dupre"]

            [[accounts]]
            name = "guest"
            password = ""
            "#,
        );

        assert_eq!(config.server.name, "Britannia");
        assert_eq!(config.server.listen.port(), 2593);
        assert_eq!(config.accounts.len(), 2);
        assert_eq!(config.accounts[0].characters, ["Lord British", "Dupre"]);
        assert_eq!(config.accounts[1].characters, Vec::<String>::new());
        config.validate().unwrap();
    }

    #[test]
    fn accounts_are_optional() {
        let config = config(MINIMAL);
        assert!(config.accounts.is_empty());
        config.validate().unwrap();
    }

    #[test]
    fn a_typo_in_a_key_is_an_error_not_a_default() {
        // `deny_unknown_fields` earns its place here: `advertize` quietly
        // falling back to a default is exactly the silent misconfiguration this
        // crate exists to prevent.
        let result: Result<Config, _> = toml::from_str(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertize = "127.0.0.1:2593"
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_wildcard_advertise_is_refused() {
        // The mistake people actually make: copying `listen` into `advertise`.
        // 0.0.0.0 means "every interface" to a server and nothing at all to a
        // client being told where to dial.
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "0.0.0.0:2593"
            "#,
        );
        assert!(matches!(
            config.validate(),
            Err(ConfigError::AdvertisedUnspecified)
        ));
    }

    #[test]
    fn the_wildcard_error_says_what_to_do() {
        // A validation error nobody can act on is barely better than no check.
        let message = ConfigError::AdvertisedUnspecified.to_string();
        assert!(message.contains("server.advertise"), "names the field");
        assert!(message.contains("clients dial"), "says what it is for");
        assert!(message.contains("server.listen"), "names the confusion");
    }

    #[test]
    fn an_ipv6_wildcard_advertise_is_refused_too() {
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "[::]:2593"
            advertise = "[::]:2593"
            "#,
        );
        assert!(matches!(
            config.validate(),
            Err(ConfigError::AdvertisedUnspecified)
        ));
    }

    #[test]
    fn a_listen_wildcard_is_fine() {
        // Only `advertise` is constrained. Binding every interface is normal.
        let config = config(MINIMAL);
        assert!(config.server.listen.ip().is_unspecified());
        config.validate().unwrap();
    }

    #[test]
    fn advertise_needs_a_port() {
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:0"
            "#,
        );
        assert!(matches!(
            config.validate(),
            Err(ConfigError::AdvertisedPortZero)
        ));
    }

    #[test]
    fn advertise_may_differ_from_listen() {
        // The whole reason they are separate fields: bind everything, tell
        // clients the one public address.
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "203.0.113.10:2593"
            "#,
        );
        config.validate().unwrap();
        assert_eq!(
            config.advertise_v4().unwrap().ip().octets(),
            [203, 0, 113, 10]
        );
    }

    #[test]
    fn an_ipv6_advertise_has_nowhere_to_go_on_the_wire() {
        // The 0x8C relay has four bytes for an address. There is no v6 form.
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "[::]:2593"
            advertise = "[2001:db8::1]:2593"
            "#,
        );
        config.validate().unwrap();
        assert_eq!(config.advertise_v4(), None);
    }

    #[test]
    fn a_shard_name_that_would_not_fit_is_refused() {
        // The 0xA8 field is 32 bytes; a longer name is silently truncated by the
        // encoder, so catch it where it can still be explained.
        let long = "x".repeat(MAX_SHARD_NAME + 1);
        let config = config(&format!(
            r#"
            [server]
            name = "{long}"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"
            "#
        ));
        assert!(matches!(
            config.validate(),
            Err(ConfigError::BadShardName { length: 33 })
        ));
    }

    #[test]
    fn a_shard_name_at_the_limit_is_fine() {
        let name = "x".repeat(MAX_SHARD_NAME);
        let config = config(&format!(
            r#"
            [server]
            name = "{name}"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"
            "#
        ));
        config.validate().unwrap();
    }

    #[test]
    fn an_empty_shard_name_is_refused() {
        let config = config(
            r#"
            [server]
            name = ""
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"
            "#,
        );
        assert!(matches!(
            config.validate(),
            Err(ConfigError::BadShardName { length: 0 })
        ));
    }

    #[test]
    fn accounts_differing_only_in_case_are_refused() {
        // Login lowercases names, so these two would collide at runtime with one
        // silently shadowing the other.
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [[accounts]]
            name = "Admin"
            password = "a"

            [[accounts]]
            name = "admin"
            password = "b"
            "#,
        );
        assert!(matches!(
            config.validate(),
            Err(ConfigError::DuplicateAccount { .. })
        ));
    }

    #[test]
    fn an_empty_account_name_is_refused() {
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [[accounts]]
            name = ""
            password = "a"
            "#,
        );
        assert!(matches!(
            config.validate(),
            Err(ConfigError::EmptyAccountName)
        ));
    }

    #[test]
    fn the_shipped_default_parses_and_validates() {
        // It is written out for a fresh checkout, so it had better be usable.
        let config: Config = toml::from_str(DEFAULT_TOML).expect("DEFAULT_TOML must parse");
        config.validate().expect("DEFAULT_TOML must validate");
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].name, "admin");
    }

    #[test]
    fn a_config_round_trips_through_toml() {
        let original = config(MINIMAL);
        let text = toml::to_string(&original).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn persistence_defaults_to_no_database() {
        // A config with no [persistence] section — every config written before
        // this option existed — must still parse and mean "keep it in memory".
        assert_eq!(config(MINIMAL).persistence.database, "");
    }

    #[test]
    fn facets_default_to_just_felucca() {
        // A config from before facets existed, and the shipped default, both mean
        // "load map0 only".
        assert_eq!(config(MINIMAL).world.facets, vec![0]);
        let default: Config = toml::from_str(DEFAULT_TOML).unwrap();
        assert_eq!(default.world.facets, vec![0]);
    }

    #[test]
    fn facets_are_read_as_a_list() {
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [world]
            facets = [0, 1, 4]
            "#,
        );
        assert_eq!(config.world.facets, vec![0, 1, 4]);
    }

    #[test]
    fn a_database_path_is_read() {
        let config = config(
            r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [persistence]
            database = "openshard.db"
            "#,
        );
        assert_eq!(config.persistence.database, "openshard.db");
    }

    #[test]
    fn a_missing_file_says_which_one() {
        let error = Config::load("/nonexistent/openshard.toml").unwrap_err();
        assert!(error.to_string().contains("openshard.toml"));
        assert!(matches!(error, ConfigError::Read { .. }));
    }

    #[test]
    fn gameplay_defaults_to_the_pre_aos_feel() {
        // A config from before [gameplay] existed still parses and means the same
        // numbers the constants used to hold.
        let g = config(MINIMAL).gameplay;
        assert_eq!(g.combat_era, 1);
        assert_eq!(g.speed_scale_factor, 15000);
        assert_eq!(g.skill_cap, 1000);
        assert_eq!(
            (g.distance_talk, g.distance_whisper, g.distance_yell),
            (18, 3, 31)
        );
    }

    #[test]
    fn the_shipped_config_names_the_gameplay_knobs_and_validates() {
        let default: Config = toml::from_str(DEFAULT_TOML).unwrap();
        default.validate().expect("the shipped config is valid");
        assert_eq!(default.gameplay.decay_seconds, 1200);
        assert_eq!(default.gameplay.criminal_seconds, 120);
    }

    #[test]
    fn an_unknown_combat_era_is_refused() {
        let mut config = config(MINIMAL);
        config.gameplay.combat_era = 5;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::UnknownCombatEra { era: 5 })
        ));
    }

    #[test]
    fn a_zero_speed_scale_factor_is_refused() {
        // The swing formula divides by it — a zero would panic mid-tick, so the
        // shard refuses to start instead.
        let mut config = config(MINIMAL);
        config.gameplay.speed_scale_factor = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::ZeroSpeedScaleFactor)
        ));
    }
}
