use super::*;

/// Load the config, writing the shipped default if there is none.
///
/// A fresh checkout should run. Writing the default rather than baking one in
/// means the first thing a new operator sees is the file they need to edit, with
/// the `advertise` warning in it, instead of a shard that works on their laptop
/// and nowhere else for reasons nobody wrote down.
pub(crate) fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    if !Path::new(path).exists() {
        std::fs::write(path, DEFAULT_TOML)?;
        info!(path, "no config found; wrote the default");
    }
    Ok(Config::load(path)?)
}

/// Where the world is kept.
///
/// `persistence.database` picks the backend by what it looks like: a
/// `postgres://` (or `postgresql://`) URL connects to PostgreSQL, anything else
/// is a SQLite file path, and an empty string keeps everything in memory and says
/// so. The two databases are equal choices, not a dev-and-prod pair — SQLite runs
/// a live shard perfectly well, and which one an operator wants is theirs to
/// decide.
///
/// The in-memory mode is a real choice too, not a broken one — the same bargain
/// as running with no map — but a shard that stays quiet about it is one an
/// operator assumes is saving, so it warns.
///
/// Opening the database can fail, and that is fatal: a shard told to persist that
/// cannot is not a shard anyone wants started in memory by surprise, losing
/// everything at the next stop.
pub(crate) async fn open_store(
    config: &Config,
) -> Result<Arc<dyn Store>, Box<dyn std::error::Error>> {
    let target = config.persistence.database.trim();
    if target.is_empty() {
        warn!(
            "no database configured: the world is kept in memory and lost at stop. \
             Set persistence.database to a file (SQLite) or a postgres:// URL to keep \
             characters across a restart."
        );
        return Ok(Arc::new(MemoryStore::new()));
    }
    if is_postgres_url(target) {
        // The URL can carry a password, so it is never logged — only that this is
        // the PostgreSQL backend.
        let store = PgStore::connect(target)
            .await
            .map_err(|error| format!("could not connect to PostgreSQL: {error}"))?;
        info!("persisting to PostgreSQL");
        return Ok(Arc::new(store));
    }
    let store = SqliteStore::open(target)
        .map_err(|error| format!("could not open the database at {target:?}: {error}"))?;
    info!(path = target, "persisting to SQLite");
    Ok(Arc::new(store))
}

/// Whether `persistence.database` names a PostgreSQL server rather than a SQLite
/// file. The two `postgres` spellings are the ones libpq itself accepts.
pub(crate) fn is_postgres_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("postgres://") || lower.starts_with("postgresql://")
}

/// Turn the validated `[gameplay]` config into the world's runtime rules,
/// converting the operator's seconds into the tick counts the systems run on.
pub(crate) fn gameplay_of(config: &Config) -> Gameplay {
    let g = &config.gameplay;
    Gameplay::new(
        g.combat_era,
        g.speed_scale_factor,
        g.skill_cap,
        g.decay_seconds,
        g.criminal_seconds,
        g.distance_talk,
        g.distance_whisper,
        g.distance_yell,
        g.creature_step_ms,
        openshard_world::CastStyle::parse(&g.cast_style),
        g.spell_disturb,
        openshard_world::TooltipMode::parse(&g.tooltips),
        g.context_menus,
        g.reagents,
        g.mana_loss_on_fail,
        g.reagent_loss_on_fail,
        g.lod,
        g.lod_radius,
        g.lod_idle_factor,
    )
}

/// The `0xB9` SupportedFeatures mask this shard advertises, from the tooltip and
/// context-menu config.
///
/// Zero when both are off — no `0xB9` is sent, and a modern client stays on the
/// classic single-click name label. Otherwise the AoS expansion set (ServUO's
/// `FeatureFlags` `T2A|UOR|UOTD|LBR|AOS` = `0x1F`), whose AOS bit is what turns on
/// object tooltips and context menus. The lower expansion bits ride along as
/// ServUO's core-expansion default; a 2D client ignores the ones it does not use.
pub(crate) fn supported_features_of(config: &Config) -> u32 {
    let g = &config.gameplay;
    let aos = openshard_world::TooltipMode::parse(&g.tooltips) != openshard_world::TooltipMode::Off
        || g.context_menus;
    if aos {
        openshard_protocol::AOS_FEATURE_FLAGS
    } else {
        0
    }
}

/// The `0xA9` character-list flags this shard advertises, from the tooltip and
/// context-menu config.
///
/// This is the packet ClassicUO actually reads to enable AoS object tooltips
/// (bit `0x20`) and context menus (bit `0x08`) — its `ClientFeatures.SetFlags`
/// keys on the character-list flags, not the `0xB9` SupportedFeatures. Without
/// the right bits here a modern client never sends a tooltip (`0xD6`) or
/// context-menu (`0xBF`) request, whatever its version.
pub(crate) fn character_list_flags_of(config: &Config) -> u32 {
    let g = &config.gameplay;
    let mut flags = 0;
    if openshard_world::TooltipMode::parse(&g.tooltips) != openshard_world::TooltipMode::Off {
        flags |= openshard_protocol::CLF_TOOLTIPS;
    }
    if g.context_menus {
        flags |= openshard_protocol::CLF_CONTEXT_MENU;
    }
    flags
}

/// Load the client's map, if it is configured.
///
/// Blocking, and on purpose: this reads over a hundred megabytes and takes a
/// moment, and there is no sense accepting a client before the world it will
/// walk in exists.
pub(crate) fn load_world(config: &Config) -> Result<World, Box<dyn std::error::Error>> {
    let start = (config.world.start.x, config.world.start.y);
    let gameplay = gameplay_of(config);
    let dir = config.world.client_files.trim();
    if dir.is_empty() {
        warn!(
            "world.client_files is empty: running with no map. Every step will be allowed — \
             players walk through walls and across water. Set it to a client install."
        );
        return Ok(World::new(start)
            .with_gameplay(gameplay)
            .with_save_seconds(config.persistence.save_seconds));
    }

    let dir = Path::new(dir);
    let started = Instant::now();
    // One tile table, shared by every facet: `tiledata.mul` describes tiles, not
    // a map, so it is read once and each facet's terrain gets a copy.
    let tiles = TileData::load(dir.join("tiledata.mul"))?;

    let mut world = World::new(start)
        .with_gameplay(gameplay)
        .with_save_seconds(config.persistence.save_seconds);
    for &facet in &config.world.facets {
        let map = Map::load_facet(dir, facet)?;
        // The start is only checked against facet 0, where new characters spawn.
        // A start off the map, or in the sea, is worth saying out loud: the shard
        // still runs and every player spawns somewhere useless.
        if facet == 0 {
            match map.land(start.0, start.1) {
                Some(cell) => info!(x = start.0, y = start.1, z = cell.z, "start position"),
                None => warn!(
                    x = start.0,
                    y = start.1,
                    "world.start is off the map; characters will spawn in nowhere"
                ),
            }
        }
        info!(
            facet,
            name = map.facet_name(),
            size = format!("{}x{}", map.width(), map.height()),
            statics = map.static_count(),
            "facet loaded"
        );
        world = world.with_facet(facet, MapTerrain::new(map, tiles.clone()));
    }
    info!(
        facets = config.world.facets.len(),
        tiledata = ?tiles.format(),
        took = ?started.elapsed(),
        "world loaded"
    );
    Ok(world)
}
