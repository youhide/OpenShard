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
    assert!(config.accounts[1].characters.is_empty());
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
    assert!(matches!(config.validate(), Err(ConfigError::AdvertisedPortZero)));
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
    assert_eq!(config.advertise_v4().unwrap().ip().octets(), [203, 0, 113, 10]);
}

#[test]
fn an_ipv6_advertise_has_nowhere_to_go_on_the_wire() {
    // The 0x8C relay has four bytes for an address. There is no v6 form, so
    // this is refused at validate() rather than left for advertise_v4() to
    // turn up None once the shard is already trying to serve a client.
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "[::]:2593"
            advertise = "[2001:db8::1]:2593"
            "#,
    );
    assert!(matches!(config.validate(), Err(ConfigError::AdvertisedNotIpv4)));
    assert_eq!(config.advertise_v4(), None);
}

#[test]
fn the_ipv6_advertise_error_says_what_to_do() {
    let message = ConfigError::AdvertisedNotIpv4.to_string();
    assert!(message.contains("server.advertise"), "names the field");
    assert!(message.contains("IPv6"), "names what is wrong");
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
    assert!(matches!(config.validate(), Err(ConfigError::EmptyAccountName)));
}

#[test]
fn the_shipped_default_parses_and_validates() {
    // It is written out for a fresh checkout, so it had better be usable.
    let config: Config = toml::from_str(DEFAULT_TOML).expect("DEFAULT_TOML must parse");
    config.validate().expect("DEFAULT_TOML must validate");
    assert_eq!(config.accounts.len(), 1);
    assert_eq!(config.accounts[0].name, "admin");
}

/// The account this file ships is a **player**, whatever it is called.
///
/// A decision, pinned here so that changing it is a decision too. The file is
/// written on first run with a password anyone can read, so authority in it
/// would mean a shard on defaults is owned by whoever guesses two words.
///
/// It costs something, and the cost is what the comment above the account in
/// `default.toml` is for: `.admin` from this account is not refused, it is
/// *spoken* — a player may say ".hello" out loud — so the words land in the chat
/// and no window opens, with nothing anywhere to say why. That is hours of
/// hunting through the server, the packet and the renderer, all of which are
/// working.
#[test]
fn the_shipped_account_has_no_authority_and_the_file_says_so() {
    let config: Config = toml::from_str(DEFAULT_TOML).expect("DEFAULT_TOML must parse");
    assert_eq!(
        config.accounts[0]
            .access
            .0
            .parse::<openshard_protocol::access::AccessLevel>()
            .unwrap_or_default(),
        openshard_protocol::access::AccessLevel::Player,
        "the shipped account gained authority: see this test's doc before changing it"
    );
    // And the knob is named where an operator will look for it. Absent from the
    // file, it is a knob nobody knows exists — which is how it went wrong.
    assert!(
        DEFAULT_TOML.contains("access = \"administrator\""),
        "default.toml no longer shows how to grant authority"
    );
}

/// **The commented rows in `default.toml` are the table the shard actually
/// runs.**
///
/// They are written as *"uncommenting this changes nothing"*, and that sentence
/// is either true or it is the worst kind of documentation: an operator who
/// uncomments a block to edit one number, and thereby silently changes three
/// others, has been lied to by the file they were reading. The claim is the same
/// one `action_stages`'s and `action_rules`'s own unit tests make about the two
/// vocabularies in code — this is the third copy, in the operator's file, and it
/// is the copy nobody would notice going stale.
///
/// Only the combat tables, and only the blocks that claim to be the shipped
/// ones: uncommenting is done by lifting every `# ` line that follows a
/// `# [gameplay.action_speed]` or `# [gameplay.action_stages.…]` header, which is
/// exactly the gesture a person makes with a block-comment key.
#[test]
fn the_commented_combat_tables_in_the_shipped_file_are_the_shipped_tables() {
    let mut uncommented = String::new();
    let mut inside = false;
    for line in DEFAULT_TOML.lines() {
        let Some(bare) = line.strip_prefix("# ").map(str::trim_end) else {
            // A blank line ends a block, the way it does for a person reading.
            inside = false;
            continue;
        };
        if bare.starts_with("[gameplay.action_speed]") || bare.starts_with("[gameplay.action_stages.") {
            inside = true;
        }
        if inside {
            uncommented.push_str(bare);
            uncommented.push('\n');
        }
    }
    assert!(
        uncommented.contains("[gameplay.action_speed]")
            && uncommented.contains("[gameplay.action_stages.shot]"),
        "default.toml no longer shows the combat pacing tables an operator would look for:\n{uncommented}"
    );
    // Only the two tables, because the lifted block is not a whole file — it has
    // no `[server]` and is not meant to. The field types are the real ones, so
    // this is still the shard's own parser reading the operator's own rows.
    #[derive(serde::Deserialize)]
    struct JustTheTables {
        gameplay: JustTheGameplay,
    }
    #[derive(serde::Deserialize)]
    struct JustTheGameplay {
        action_speed:  crate::ActionSpeedsConfig,
        action_stages: crate::ActionStagesConfig,
    }
    let parsed: JustTheTables = toml::from_str(&uncommented).expect("the commented rows must parse");
    assert_eq!(
        parsed.gameplay.action_speed,
        crate::ActionSpeedsConfig::shipped(),
        "the action_speed rows in default.toml are not the ones the shard runs"
    );
    assert_eq!(
        parsed.gameplay.action_stages,
        crate::ActionStagesConfig::shipped(),
        "the action_stages rows in default.toml are not the ones the shard runs"
    );
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
fn the_world_seed_is_absent_unless_an_operator_pins_it() {
    // Absent, not zero: zero is a seed like any other, and a default would read
    // exactly like a number somebody chose.
    assert_eq!(config(MINIMAL).world.seed, None);
    let default: Config = toml::from_str(DEFAULT_TOML).unwrap();
    assert_eq!(default.world.seed, None);
}

#[test]
fn a_pinned_world_seed_is_read_whole() {
    // At the top of what TOML can spell: its integers are signed 64-bit, so the
    // seeds an operator can write stop at `i64::MAX`. That is not a clamp to be
    // fixed — it is the whole expressible range, and it is 9.2e18 seeds wide. The
    // generator's *state*, which does use every `u64`, is never written by hand.
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [world]
            seed = 9223372036854775807
            "#,
    );
    assert_eq!(config.world.seed, Some(0x7FFF_FFFF_FFFF_FFFF));
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
    // A config from before [gameplay] existed still parses into the shipped
    // pre-AoS rules.
    let g = config(MINIMAL).gameplay;
    assert_eq!(g.combat_era, CombatEra::new(1));
    assert_eq!(g.speed_scale_factor, 10000);
    assert_eq!((g.critical_chance, g.critical_damage_percent), (50, 150));
    assert_eq!(g.skill_cap, 1000);
    assert_eq!(
        (g.distance_talk, g.distance_whisper, g.distance_yell),
        (18, 3, 31)
    );
}

#[test]
fn combat_era_keeps_numeric_toml_representation() {
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [gameplay]
            combat_era = 3
            "#,
    );
    assert_eq!(config.gameplay.combat_era, CombatEra::new(3));
    assert!(toml::to_string(&config).unwrap().contains("combat_era = 3"));
}

#[test]
fn the_shipped_config_names_the_gameplay_knobs_and_validates() {
    let default: Config = toml::from_str(DEFAULT_TOML).unwrap();
    default.validate().expect("the shipped config is valid");
    assert_eq!(default.gameplay.decay_seconds, 1200);
    assert_eq!(default.gameplay.criminal_seconds, 120);
    assert_eq!(default.gameplay.tooltips, "version");
    assert!(default.gameplay.context_menus);
}

#[test]
fn tooltips_and_context_menus_default_on() {
    // A config from before these knobs existed still parses, and means the modern
    // AoS feel: version-mode tooltips and context menus both on.
    let g = config(MINIMAL).gameplay;
    assert_eq!(g.tooltips, "version");
    assert!(g.context_menus);
}

#[test]
fn an_unknown_combat_era_is_refused() {
    let mut config = config(MINIMAL);
    config.gameplay.combat_era = CombatEra::new(5);
    assert!(matches!(
        config.validate(),
        Err(ConfigError::UnknownCombatEra { era: 5 })
    ));
}

#[test]
fn every_sphere_combat_era_is_accepted() {
    // 0 (custom), 1 (pre-AoS), 2 (AoS), 3 (SE), 4 (ML) all have a swing formula.
    for era in 0..=4 {
        let mut config = config(MINIMAL);
        config.gameplay.combat_era = CombatEra::new(era);
        assert!(config.validate().is_ok(), "era {era} should load");
    }
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

#[test]
fn an_impossible_critical_chance_is_refused() {
    let mut config = config(MINIMAL);
    config.gameplay.critical_chance = 1001;
    assert!(matches!(
        config.validate(),
        Err(ConfigError::CriticalChanceTooHigh { chance: 1001 })
    ));
}

#[test]
fn an_impossible_stat_gain_chance_is_refused() {
    let mut config = config(MINIMAL);
    config.gameplay.stat_gain_chance = 1001;
    assert!(matches!(
        config.validate(),
        Err(ConfigError::StatGainChanceTooHigh { chance: 1001 })
    ));
}

#[test]
fn a_critical_cannot_be_weaker_than_a_normal_hit() {
    let mut config = config(MINIMAL);
    config.gameplay.critical_damage_percent = 99;
    assert!(matches!(
        config.validate(),
        Err(ConfigError::CriticalDamageBelowNormal { percent: 99 })
    ));
}

#[test]
fn lod_is_off_by_default_with_sane_knobs() {
    // A config from before LOD existed still parses: the optimization is opt-in,
    // and its two knobs carry the shipped defaults.
    let g = config(MINIMAL).gameplay;
    assert!(!g.lod);
    assert_eq!(g.lod_radius, 32);
    assert_eq!(g.lod_idle_factor, 8);
}

#[test]
fn lod_knobs_are_only_checked_when_lod_is_on() {
    // With LOD off, degenerate knobs are inert, so they are not rejected.
    let mut config = config(MINIMAL);
    config.gameplay.lod = false;
    config.gameplay.lod_radius = 0;
    config.gameplay.lod_idle_factor = 0;
    config.validate().expect("off LOD ignores its knobs");
}

#[test]
fn a_zero_lod_radius_is_refused_when_lod_is_on() {
    // No creature is ever within zero tiles of a player, so every one would doze
    // forever — refuse it rather than run a frozen world.
    let mut config = config(MINIMAL);
    config.gameplay.lod = true;
    config.gameplay.lod_radius = 0;
    assert!(matches!(config.validate(), Err(ConfigError::ZeroLodRadius)));
}

#[test]
fn a_zero_lod_idle_factor_is_refused_when_lod_is_on() {
    // A factor of zero leaves a dozing creature's next-think unmoved, spinning
    // the gate every tick — refuse it.
    let mut config = config(MINIMAL);
    config.gameplay.lod = true;
    config.gameplay.lod_idle_factor = 0;
    assert!(matches!(config.validate(), Err(ConfigError::ZeroLodIdleFactor)));
}

#[test]
fn a_facet_can_name_a_base_set_instead_of_the_install() {
    // The keys of a TOML table are strings, and these are facet numbers: this
    // test is as much about that conversion holding as about the field.
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [world]
            client_files = "/uo"
            facets = [0, 1]

            [world.base_sets]
            0 = "felucca.osbase"
            "#,
    );
    assert_eq!(
        config
            .world
            .base_sets
            .get(&FacetKey(Facet(0)))
            .map(PathBuf::as_path),
        Some(Path::new("felucca.osbase"))
    );
    // Facet 1 is loaded, and says nothing about where from — so it comes from
    // the install, which is what makes a shard convertible one facet at a time.
    assert_eq!(config.world.base_sets.get(&FacetKey(Facet(1))), None);
    config
        .validate()
        .expect("a base set beside an install is the whole point");
}

#[test]
fn no_base_set_is_the_default_and_the_shipped_config_has_none() {
    assert!(config(MINIMAL).world.base_sets.is_empty());
    let default: Config = toml::from_str(DEFAULT_TOML).unwrap();
    assert!(default.world.base_sets.is_empty());
}

#[test]
fn a_base_set_without_client_files_is_refused() {
    // The failure this prevents is silent: the map loads, every tile flag is
    // whatever an empty tile table says, and the walk answers wrongly for ever.
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [world]
            facets = [0]

            [world.base_sets]
            0 = "felucca.osbase"
            "#,
    );
    let error = config
        .validate()
        .expect_err("tiledata has to come from somewhere");
    assert!(matches!(
        error,
        ConfigError::BaseSetWithoutClientFiles { facet: Facet(0) }
    ));
    assert!(
        error.to_string().contains("tiledata.mul"),
        "the message has to name the file that is missing: {error}"
    );
}

#[test]
fn a_base_set_for_a_facet_nobody_loads_is_refused() {
    // A mistyped facet number is the whole reason: `4` instead of `0` leaves
    // Felucca coming out of the install, with the new world sitting unread.
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [world]
            client_files = "/uo"
            facets = [0]

            [world.base_sets]
            4 = "tokuno.osbase"
            "#,
    );
    assert!(matches!(
        config.validate(),
        Err(ConfigError::BaseSetForUnloadedFacet { facet: Facet(4) })
    ));
}

#[test]
fn an_empty_base_set_path_is_refused() {
    // `client_files = ""` means "no map"; there is no such reading here, so an
    // empty path is a half-written setting rather than a mode.
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [world]
            client_files = "/uo"
            facets = [0]

            [world.base_sets]
            0 = ""
            "#,
    );
    assert!(matches!(
        config.validate(),
        Err(ConfigError::EmptyBaseSetPath { facet: Facet(0) })
    ));
}

#[test]
fn a_base_set_table_survives_being_written_back_out() {
    // The keys go out as strings because that is the only kind TOML has, and a
    // config the shard rewrites has to be one it can read again.
    let mut config = Config::default();
    config.world.client_files = "/uo".into();
    config
        .world
        .base_sets
        .insert(FacetKey(Facet(0)), PathBuf::from("felucca.osbase"));
    let text = toml::to_string(&config).expect("a config should serialise");
    let back: Config = toml::from_str(&text).expect("and parse again");
    assert_eq!(back.world.base_sets, config.world.base_sets);
}

/// The free-movement table, read and written like the base-set one beside it.
///
/// **`false` is the value worth carrying**, and it is the one a table keyed by
/// facet makes easy to lose: absence means "whatever this number meant in
/// retail", so an operator turning the rule *on* for facet 0 and an operator
/// saying nothing about facet 0 must not round-trip to the same config. Serde
/// would drop a `false` written as a bare `bool` field with `skip_serializing_if`
/// on it; here the distinction is the key's presence, which is why both a `true`
/// and a `false` entry are asserted.
#[test]
fn a_free_movement_table_survives_being_written_back_out() {
    let mut config = Config::default();
    config.world.client_files = "/uo".into();
    // Felucca's rules in slot 3, and Trammel's in slot 0 — the two overrides
    // that exist, one each way round.
    config.world.free_movement.insert(FacetKey(Facet(3)), false);
    config.world.free_movement.insert(FacetKey(Facet(0)), true);
    let text = toml::to_string(&config).expect("a config should serialise");
    let back: Config = toml::from_str(&text).expect("and parse again");
    assert_eq!(back.world.free_movement, config.world.free_movement);
    assert_eq!(
        back.world.free_movement.get(&FacetKey(Facet(3))),
        Some(&false),
        "a facet turned off survived as an entry rather than as an absence"
    );
}

/// A config from before the setting existed loads, and says nothing about any
/// facet — which is what leaves every facet on [`FacetRules::classic`].
///
/// [`FacetRules::classic`]: openshard_state::facet_rules::FacetRules::classic
#[test]
fn free_movement_defaults_to_saying_nothing() {
    let config = config(MINIMAL);
    assert!(
        config.world.free_movement.is_empty(),
        "an unset table is empty, not a table of answers"
    );
}

/// The condition rules as an operator writes them — the exact shape documented
/// in `openshard.toml` and in `docs/combat_actions.md`'s D4, parsed rather than
/// described.
#[test]
fn the_action_rules_table_reads_as_its_effects_own_names() {
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [gameplay.action_rules.shot]
            running = { sway = { penalty = 40 } }
            walking = { slow = { percent = 50 } }
            struck = "break"
            "#,
    );
    config.validate().unwrap();
    let shot = config.gameplay.action_rules.shot;
    assert_eq!(shot.running, Some(ActionEffectConfig::Sway { penalty: 40 }));
    assert_eq!(shot.walking, Some(ActionEffectConfig::Slow { percent: 50 }));
    assert_eq!(shot.struck, Some(ActionEffectConfig::Break));
    assert_eq!(
        shot.blinded, None,
        "a row an operator writes is the whole row: what it leaves out is no rule, \
         not the shipped default quietly merged back in"
    );
    assert_eq!(
        config.gameplay.action_rules.swing,
        ActionRulesConfig::shipped().swing,
        "and a kind it says nothing about keeps the shipped row entire"
    );
}

/// A shard with no `[gameplay.action_rules]` at all runs the shipped table,
/// which is the one thing every other file in the repo describes.
#[test]
fn an_unwritten_action_rules_table_is_the_shipped_one() {
    assert_eq!(
        config(MINIMAL).gameplay.action_rules,
        ActionRulesConfig::shipped()
    );
}

/// A slow big enough to be a cancellation is refused at load rather than run: an
/// impact pushed a hundred times further out reads as a shard swallowing the
/// blow, not as a setting doing its job.
#[test]
fn an_absurd_slow_is_refused_at_load() {
    let config = config(
        r#"
            [server]
            name = "OpenShard"
            listen = "0.0.0.0:2593"
            advertise = "127.0.0.1:2593"

            [gameplay.action_rules.swing]
            struck = { slow = { percent = 10000 } }
            "#,
    );
    assert!(matches!(
        config.validate(),
        Err(ConfigError::SlowPercentTooHigh {
            kind:    "swing",
            percent: 10000,
        })
    ));
}
