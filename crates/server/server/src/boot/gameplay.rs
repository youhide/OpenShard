//! Conversion from validated server configuration into runtime gameplay state.

use openshard_config::Config;
use openshard_map::grid::Tile;
use openshard_protocol::login::{CharacterListFlags, SupportedFeatures};
use openshard_world::Gameplay;
use openshard_world::tick::screen::CharacterScreen;

/// Turn the validated gameplay configuration into the world's tick-based rules.
pub(crate) fn gameplay_of(config: &Config) -> Gameplay {
    let g = &config.gameplay;
    Gameplay {
        combat_era: g.combat_era,
        speed_scale_factor: g.speed_scale_factor,
        action_rules: openshard_state::action_rules::ActionRules::from_config(&g.action_rules),
        action_stages: openshard_state::action_stages::ActionStages::from_config(&g.action_stages),
        action_speed: openshard_state::action_speeds::ActionSpeeds::from_config(&g.action_speed),
        critical_chance: g.critical_chance,
        critical_damage_percent: g.critical_damage_percent,
        skill_cap: g.skill_cap,
        total_skill_cap: g.total_skill_cap,
        stat_cap: g.stat_cap,
        stat_cap_individual: g.stat_cap_individual,
        stat_gain_ticks: Gameplay::ticks_from_ms(g.stat_gain_ms),
        stat_gain_chance: g.stat_gain_chance,
        decay_ticks: Gameplay::ticks(g.decay_seconds),
        house_decay_ticks: Gameplay::ticks(g.house_decay_seconds),
        criminal_ticks: Gameplay::ticks(g.criminal_seconds),
        distance_talk: g.distance_talk,
        distance_whisper: g.distance_whisper,
        distance_yell: g.distance_yell,
        creature_step_ticks: Gameplay::ticks_from_ms(g.creature_step_ms),
        cast_style: openshard_world::CastStyle::parse(&g.cast_style),
        spell_disturb: g.spell_disturb,
        tooltip_mode: openshard_world::TooltipMode::parse(&g.tooltips),
        context_menus: g.context_menus,
        reagents: g.reagents,
        mana_loss_on_fail: g.mana_loss_on_fail,
        reagent_loss_on_fail: g.reagent_loss_on_fail,
        bank_gold_in_status: g.bank_gold_in_status,
        vendor_bank_payment: g.vendor_bank_payment,
        cross_facet_travel: g.cross_facet_travel,
        lod: g.lod,
        lod_radius: g.lod_radius,
        lod_idle_factor: g.lod_idle_factor,
        uo_minute_ticks: Gameplay::ticks(g.uo_minute_seconds).max(1),
        season: g.season,
        guards: g.guards,
        npc_schedule: g.npc_schedule,
        npc_work_hour: g.npc_work_hour,
        npc_home_hour: g.npc_home_hour,
        expansion: expansion_index(&g.expansion),
    }
}

fn expansion_index(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "aos" => Gameplay::AOS,
        "se" => Gameplay::SE,
        _ => Gameplay::ML,
    }
}

/// Supported-feature packet flags derived from the gameplay setting.
pub(crate) fn supported_features_of(config: &Config) -> SupportedFeatures {
    let g = &config.gameplay;
    let expansion = match g.expansion.trim().to_ascii_lowercase().as_str() {
        "aos" => SupportedFeatures::AOS,
        "se" => SupportedFeatures::SE,
        _ => SupportedFeatures::ML,
    };
    let aos = openshard_world::TooltipMode::parse(&g.tooltips) != openshard_world::TooltipMode::Off
        || g.context_menus;
    if aos { expansion } else { SupportedFeatures::NONE }
}

/// Character-list flags derived from tooltip and context-menu settings.
pub(crate) fn character_list_flags_of(config: &Config) -> CharacterListFlags {
    let g = &config.gameplay;
    let mut flags = CharacterListFlags::NONE;
    if openshard_world::TooltipMode::parse(&g.tooltips) != openshard_world::TooltipMode::Off {
        flags = flags.with(CharacterListFlags::TOOLTIPS);
    }
    if g.context_menus {
        flags = flags.with(CharacterListFlags::CONTEXT_MENU);
    }
    flags
}

/// What the login character screen offers for this configured world.
pub(crate) fn character_screen_of(config: &Config) -> CharacterScreen {
    CharacterScreen {
        starts: crate::dispatch::start_cities(
            &config.world.facets,
            Tile::new(config.world.start.x, config.world.start.y),
        ),
        flags: character_list_flags_of(config),
        features: supported_features_of(config),
    }
}
