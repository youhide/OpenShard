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
