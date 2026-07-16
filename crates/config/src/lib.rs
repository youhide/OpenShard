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
}

/// Where to find the client's data files.
#[derive(Clone, PartialEq, Eq, Debug, Default, Deserialize, Serialize)]
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

# Where a new character appears. The height comes from the map, not from here.
#
# This default is open ground on one facet and may be open water on yours.
[world.start]
x = 1363
y = 1600

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
    fn a_missing_file_says_which_one() {
        let error = Config::load("/nonexistent/openshard.toml").unwrap_err();
        assert!(error.to_string().contains("openshard.toml"));
        assert!(matches!(error, ConfigError::Read { .. }));
    }
}
