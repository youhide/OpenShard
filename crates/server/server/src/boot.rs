use super::*;

/// Load the config, writing the shipped default if there is none.
///
/// A fresh checkout should run. Writing the default rather than baking one in
/// means the first thing a new operator sees is the file they need to edit, with
/// the `advertise` warning in it, instead of a shard that works on their laptop
/// and nowhere else for reasons nobody wrote down.
pub fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
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
pub async fn open_store(config: &Config) -> Result<Arc<dyn Store>, Box<dyn std::error::Error>> {
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
///
/// Every field is named, and none is left to `Default`: a new `[gameplay]` knob
/// should fail to compile here until it is wired, rather than quietly run on its
/// default because nobody remembered this function.
pub(crate) fn gameplay_of(config: &Config) -> Gameplay {
    let g = &config.gameplay;
    Gameplay {
        combat_era: g.combat_era,
        speed_scale_factor: g.speed_scale_factor,
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
        // The same setting the `0xB9` mask is built from, as an ordinal: the
        // paperdoll the client draws and the content the shard runs read one
        // value, so they cannot disagree about which expansion this is.
        expansion: expansion_index(&g.expansion),
    }
}

/// The `0xB9` SupportedFeatures mask this shard advertises, from the tooltip and
/// context-menu config.
///
/// Zero when both are off — no `0xB9` is sent, and a modern client stays on the
/// classic single-click name label. Otherwise the AoS expansion set (ServUO's
/// `FeatureFlags` `T2A|UOR|UOTD|LBR|AOS` = `0x1F`), whose AOS bit is what turns on
/// object tooltips and context menus. The lower expansion bits ride along as
/// ServUO's core-expansion default; a 2D client ignores the ones it does not use.
/// The expansion name as the ordinal `Gameplay` compares against.
///
/// The same three names `supported_features_of` maps to `0xB9` masks, read once
/// more: one setting, two consumers, and `config` has already refused anything
/// else, so an unknown name is the ML default rather than an error here.
fn expansion_index(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "aos" => Gameplay::AOS,
        "se" => Gameplay::SE,
        _ => Gameplay::ML,
    }
}

pub(crate) fn supported_features_of(config: &Config) -> SupportedFeatures {
    let g = &config.gameplay;
    // The expansion the operator asked for. This is what the client builds its
    // paperdoll from: under AoS there is no Quest button to press, so the whole
    // `0xD7`/`0x32` path is unreachable however correctly it is implemented.
    let expansion = match g.expansion.trim().to_ascii_lowercase().as_str() {
        "aos" => SupportedFeatures::AOS,
        "se" => SupportedFeatures::SE,
        // `config` has already refused anything else; ML is the default.
        _ => SupportedFeatures::ML,
    };
    // With tooltips and context menus both off the shard advertises nothing at
    // all and a modern client falls back to the classic single-click name — the
    // pre-AoS feel, which is a choice an operator can still make.
    let aos = openshard_world::TooltipMode::parse(&g.tooltips) != openshard_world::TooltipMode::Off
        || g.context_menus;
    if aos { expansion } else { SupportedFeatures::NONE }
}

/// The `0xA9` character-list flags this shard advertises, from the tooltip and
/// context-menu config.
///
/// This is the packet ClassicUO actually reads to enable AoS object tooltips
/// (bit `0x20`) and context menus (bit `0x08`) — its `ClientFeatures.SetFlags`
/// keys on the character-list flags, not the `0xB9` SupportedFeatures. Without
/// the right bits here a modern client never sends a tooltip (`0xD6`) or
/// context-menu (`0xBF`) request, whatever its version.
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

/// What the character screen offers, from the config.
///
/// The cities are filtered to the facets this shard loaded, so every one offered
/// is a place a player can actually be put. The two masks are the same tooltip
/// and context-menu settings read once more — one setting, three consumers, and
/// they must agree or a modern client is told to expect tooltips it never gets.
pub(crate) fn character_screen_of(config: &Config) -> CharacterScreen {
    CharacterScreen {
        starts: crate::start_cities(&config.world.facets, (config.world.start.x, config.world.start.y)),
        flags: character_list_flags_of(config),
        features: supported_features_of(config),
    }
}

/// The world the config asks for, before a map or a save is laid over it.
///
/// Both of [`load_world`]'s paths — with a map and without — come through here, so
/// a knob added to `[world]` cannot be wired into one branch and forgotten in the
/// other. That drift is silent: the mapless mode is the one tests and a first run
/// use, so the branch that gets it right is not the branch anyone notices.
fn configured_world(config: &Config) -> World {
    let world = World::new((config.world.start.x, config.world.start.y))
        .with_gameplay(gameplay_of(config))
        .with_character_screen(character_screen_of(config))
        .with_save_seconds(config.persistence.save_seconds);
    // Only when the operator pinned one. There is no `u64` that means "no seed", so
    // an absent `world.seed` has to leave the world's own default in place rather
    // than pass a stand-in through.
    match config.world.seed {
        Some(seed) => {
            info!(
                seed,
                "world.seed is pinned: a fresh world's rolls are reproducible"
            );
            world.with_seed(seed)
        }
        None => world,
    }
}

/// Everything a shard needs before its first tick that has to be read off a
/// disk: the accounts, and a world with the last save laid over it.
///
/// Two values rather than one because they are two owners — the accounts go to
/// [`LoginServer`], the world to the tick — and the only thing they share is that
/// both are finished by the time the loop starts.
pub(crate) struct Restored {
    pub(crate) accounts: DevAccounts,
    pub(crate) world: World,
}

/// Fill a freshly built world from the store and the config, in the one order
/// that works.
///
/// The order is the whole reason this is a function and not seven calls in the
/// shard loop: characters before items, because the serials `restore_characters`
/// reserves are the owners the item records point at; items before mobiles,
/// because a mobile is equipped out of the inventories the items filed. Each
/// function's own doc says why it sits where it does.
///
/// Neither of those two is only said any more: `restore_items` takes the
/// [`RestoredCharacters`] that only `restore_characters` can hand back and
/// returns the [`RestoredItems`] that `restore_mobiles` will not compile
/// without, so neither pair can be swapped without a type error.
///
/// **The config's characters used to be the third of those and are not a rule at
/// all.** This doc claimed they had to come after the store's so that a name in
/// both kept the row that describes it — and the roster had never behaved
/// otherwise: `Roster::enrol` does not touch an entry that is there, and
/// `Roster::remember` describes an entry however late it was enrolled. The one
/// thing the order really decided was the *spelling* shown by `0xA9`, which the
/// roster now takes off the record rather than off whichever call ran first, so
/// that is order-independent too. `seed_configured_characters` sits here because
/// a stored character should hold the lower slot, and nothing worse than a slot
/// order rides on it. `unenforced.md` S6 has the argument.
///
/// Nothing here is fatal. A store that cannot be read is logged at each step and
/// the shard comes up with whatever it did get: a shard that refuses to start
/// because one table is unreadable helps nobody, and the alternative to a
/// partially restored world is no world at all.
pub(crate) async fn restore(store: &dyn Store, config: &Config, world: World) -> Restored {
    let accounts = load_accounts(store, config).await;
    let mut world = world;
    // Before the characters, whose records name a guild by id. Nothing at boot
    // resolves one — the id is copied onto a component — but the order is the one
    // that stays correct when something does.
    restore_guilds(store, &mut world).await;
    let characters = restore_characters(store, &mut world).await;
    seed_configured_characters(config, &mut world);
    let items = restore_items(store, &mut world, &characters).await;
    restore_mobiles(store, &mut world, &items).await;
    restore_decorations(store, &mut world).await;
    restore_spawners(store, &mut world).await;
    restore_regions(store, &mut world).await;
    let world = restore_world(store, world, config.world.seed).await;
    Restored { accounts, world }
}

/// Accounts come from the store first — their credentials are the argon2
/// hashes saved there — and config seeds the rest. The store is authoritative
/// for a password once it has one, so a config `[[accounts]]` line only
/// creates an account the store has never seen; changing a config password
/// after the first boot does nothing (the shard says as much in the docs).
async fn load_accounts(store: &dyn Store, config: &Config) -> DevAccounts {
    let mut accounts = DevAccounts::new();
    match store.accounts().await {
        Ok(stored) => {
            for record in stored {
                accounts = accounts.with_credential(&record.name, &record.credential);
            }
        }
        Err(error) => error!(%error, "could not read saved accounts; config seeds them instead"),
    }
    for account in &config.accounts {
        // Seed a config account only if the store has never seen it, hashing the
        // plaintext once and writing that same hash both in memory and to the
        // store — never the plaintext.
        if !accounts.contains(&account.name) {
            let credential = openshard_login::password::hash(&account.password);
            accounts = accounts.with_credential(&account.name, &credential);
            let record = AccountRecord {
                name: account.name.clone(),
                credential,
            };
            if let Err(error) = store.put_account(&record).await {
                warn!(%account.name, %error, "could not persist a configured account");
            }
        }
        // Access comes from config every boot regardless: it is deliberately not
        // persisted, but re-derived at each login. The account's *characters* are
        // not the accounts' business at all any more — they go to the world's
        // roster, in `seed_configured_characters`.
        // An unparseable access level is logged and left a player — authority is
        // never granted by a typo.
        match account.access.0.parse::<AccessLevel>() {
            Ok(AccessLevel::Player) => {}
            Ok(level) => accounts = accounts.with_access(&account.name, level),
            Err(error) => {
                warn!(%account.name, %error, "unknown access level; treating as player")
            }
        }
    }
    accounts
}

/// Bring the world's characters back from the database.
///
/// All of it goes to the world and none of it to the accounts: a stored row says
/// both that the character exists and where it was, and since S5 of
/// `docs/connection_state.md` the roster is what holds each. The accounts keep
/// credentials and authority — what a login is about — and nothing that a
/// character screen would read.
/// A store that cannot be read is not a reason to skip the items: the restore
/// still ran, with nothing in it, and the token says so. Returning an `Option`
/// here would put the ordering rule back in prose — the caller would have to know
/// that "no characters" still permits items.
async fn restore_characters(store: &dyn Store, world: &mut World) -> RestoredCharacters {
    match store.characters().await {
        Ok(characters) => {
            let restored = world.restore_characters(characters);
            if restored.count() > 0 {
                info!(
                    characters = restored.count(),
                    "restored the world from the database"
                );
            }
            restored
        }
        Err(error) => {
            error!(%error, "could not read saved characters; starting with none");
            world.restore_characters(Vec::new())
        }
    }
}

/// Put the config's `[[accounts]] characters` on the world's lists.
///
/// The other half of who exists, beside the store's rows. A configured character
/// that has never been played has nothing saved anywhere — no serial, no
/// position — so all this records is that it exists; entering it spawns a fresh
/// one at the start city.
///
/// Called after [`restore_characters`] for the slots and nothing else: a name in
/// both halves is one entry either way, described either way, and spelled the way
/// it was created either way — see `World::enrol_character`. Running after means
/// the characters somebody has actually played hold the lower slots, which is the
/// order their player already knows.
fn seed_configured_characters(config: &Config, world: &mut World) {
    for account in &config.accounts {
        for character in &account.characters {
            world.enrol_character(&account.name, character);
        }
    }
}

/// Bring back saved items: the world reserves their serials, drops the loose
/// ground clutter back where it lay, and files each character's carried
/// inventory to re-equip when it logs in. It takes the characters' restore as an
/// argument rather than trusting a comment about call order — the serials that
/// restore reserved are the owners these records point at — and hands the same
/// kind of token on to the mobiles.
///
/// A store that cannot be read restores nothing and still returns the token, for
/// the reason [`restore_characters`] does: an `Option` here would make the caller
/// decide what "no items" permits, which is the ordering rule back in prose.
async fn restore_items(
    store: &dyn Store,
    world: &mut World,
    characters: &RestoredCharacters,
) -> RestoredItems {
    match store.items().await {
        Ok(items) => {
            if !items.is_empty() {
                info!(items = items.len(), "restored saved items");
            }
            world.restore_items(items, characters)
        }
        Err(error) => {
            error!(%error, "could not read saved items; starting with none");
            world.restore_items(Vec::new(), characters)
        }
    }
}

/// Bring back the world's NPC mobiles — townsfolk, vendors, creatures — each
/// exactly as saved. Takes the items' restore rather than a comment about call
/// order: each mobile's gear and stock is already filed under its serial for
/// `World::restore_mobiles` to equip, and the token is what says so. This is the
/// whole-world model: the pack seeds a fresh world once (a staff Populate), and
/// from then on the save is the truth — nothing respawns at boot.
async fn restore_mobiles(store: &dyn Store, world: &mut World, items: &RestoredItems) {
    match store.mobiles().await {
        Ok(mobiles) => {
            if !mobiles.is_empty() {
                info!(mobiles = mobiles.len(), "restored the world's mobiles");
            }
            world.restore_mobiles(mobiles, items);
        }
        Err(error) => error!(%error, "could not read saved mobiles; starting with none"),
    }
}

/// Bring back the placed decoration, door state and all.
async fn restore_decorations(store: &dyn Store, world: &mut World) {
    match store.decorations().await {
        Ok(decorations) => {
            if !decorations.is_empty() {
                info!(decorations = decorations.len(), "restored the world's decoration");
            }
            world.restore_decorations(decorations);
        }
        Err(error) => error!(%error, "could not read saved decorations; starting with none"),
    }
}

/// Bring back the spawn regions with their respawn timers, so a populated area
/// stays populated across a restart and a rare spawn keeps its remaining wait
/// rather than popping again the moment the shard comes up.
async fn restore_spawners(store: &dyn Store, world: &mut World) {
    match store.spawners().await {
        Ok(spawners) => {
            if !spawners.is_empty() {
                info!(spawners = spawners.len(), "restored spawn regions");
            }
            world.restore_spawners(spawners);
        }
        Err(error) => error!(%error, "could not read saved spawners; starting with none"),
    }
}

/// Bring back the guilds. Only the guilds: who is *in* one rides with the
/// character records, so a roster is derived from who names the guild and there
/// is no second list to fall out of step with it.
async fn restore_guilds(store: &dyn Store, world: &mut World) {
    match store.guilds().await {
        Ok(guilds) => world.restore_guilds(guilds),
        Err(error) => error!(%error, "could not read saved guilds; starting with none"),
    }
    // And the alliances they name. Read separately and failing separately: an
    // unreadable alliance table is a shard whose guilds are all unallied, which
    // is a great deal better than a shard with no guilds.
    // And the houses, after the facets exist — restoring one asks the terrain for
    // its footprint, and a terrain that is not there yet answers no walls.
    match store.houses().await {
        Ok(houses) => {
            // The designs come with them rather than in a pass of their own: a
            // design is only meaningful joined to its house, and a house whose
            // design failed to read must not come back wearing the foundation's
            // walls without anything saying so. An unreadable design table is
            // logged and the houses restore classic, which is visible.
            let designs = match store.designs().await {
                Ok(designs) => designs,
                Err(error) => {
                    error!(%error, "could not read saved house designs; those houses restore unshaped");
                    Vec::new()
                }
            };
            world.restore_houses(houses, designs);
        }
        Err(error) => error!(%error, "could not read saved houses; starting with none"),
    }
    // And the ships, on the same terms and for the same reason: a mooring asks
    // the terrain which of its tiles are hull and which are deck, so it has to
    // come after the facets. Separate from the houses because a fleet and a
    // village fail independently.
    match store.boats().await {
        Ok(boats) => world.restore_boats(boats),
        Err(error) => error!(%error, "could not read saved boats; starting with none"),
    }
    match store.alliances().await {
        Ok(alliances) => world.restore_alliances(alliances),
        Err(error) => error!(%error, "could not read saved alliances; starting with none"),
    }
}

/// Bring back the named regions — towns, dungeons, guarded zones. Saved like
/// everything else, so a restart keeps its guards, its music and the dark in
/// its caves without waiting for a staff `.admin`.
async fn restore_regions(store: &dyn Store, world: &mut World) {
    match store.regions().await {
        Ok(regions) => {
            if !regions.is_empty() {
                info!(regions = regions.len(), "restored the world's regions");
            }
            world.restore_regions(regions);
        }
        Err(error) => error!(%error, "could not read saved regions; starting with none"),
    }
}

/// Bring back the two things only the world itself knew: the hour of the day, and
/// where its roll generator had got to.
///
/// The tick counter restarts at zero by design, so without the clock every restart
/// would be a fresh midnight. The generator is the same shape of loss with a
/// sharper edge — re-seeded, it does not roll *differently*, it rolls the previous
/// run's sequence again, which is a thing a player who dislikes a roll can arrange
/// by getting the shard restarted.
///
/// A store with no such row yet — a world nobody has saved — is left exactly as
/// built, which is what keeps a configured `world.seed` from being overwritten on
/// the first boot. A store that cannot be *read* is logged and treated the same
/// way: this is cosmetic-to-annoying, not corrupting, and refusing to boot over it
/// would be worse.
///
/// `pinned_seed` is only here to be *complained* about: a saved world resumes its
/// stream, so a `world.seed` set on a shard that has saved does nothing, and a knob
/// that silently does nothing is the failure this whole config crate exists to
/// prevent. The operator hears it once, at boot.
async fn restore_world(store: &dyn Store, world: World, pinned_seed: Option<u64>) -> World {
    match store.world().await {
        Ok(Some(record)) => {
            if pinned_seed.is_some() {
                warn!(
                    "world.seed is set but this world has been saved before, so it is ignored: \
                     the shard resumes the roll stream its save recorded. Start from a fresh \
                     database to use it."
                );
            }
            world
                .with_clock_minutes(record.clock_minutes)
                .with_rng_state(record.rng_state)
                // The guild id counter, which is here and not derived from the
                // guilds themselves: a disbanded guild leaves no row, so the
                // maximum id in the table is not the maximum ever issued.
                .with_guild_high_water(record.guild_high_water)
                .with_alliance_high_water(record.alliance_high_water)
        }
        Ok(None) => world,
        Err(error) => {
            error!(%error, "could not read the saved world scalars; starting at midnight with a fresh roll stream");
            world
        }
    }
}

/// Load the client's map, if it is configured.
///
/// Blocking, and on purpose: this reads over a hundred megabytes and takes a
/// moment, and there is no sense accepting a client before the world it will
/// walk in exists.
pub fn load_world(config: &Config) -> Result<World, Box<dyn std::error::Error>> {
    let start = (config.world.start.x, config.world.start.y);
    let dir = config.world.client_files.trim();
    if dir.is_empty() {
        warn!(
            "world.client_files is empty: running with no map. Every step will be allowed — \
             players walk through walls and across water. Set it to a client install."
        );
        return Ok(configured_world(config));
    }

    let dir = Path::new(dir);
    let started = Instant::now();
    // One tile table for the shard: `tiledata.mul` describes tiles, not a map, so
    // it is read once and shared. It used to be *cloned* into each facet's
    // terrain, which copied the whole table per facet to answer questions that
    // never depended on the facet.
    eprintln!("world load: reading tiledata.mul from {}", dir.display());
    let tiles = std::sync::Arc::new(TileData::load(dir.join("tiledata.mul"))?);
    eprintln!(
        "world load +{:.3}s: tile data ready; loading {} facet(s)",
        started.elapsed().as_secs_f64(),
        config.world.facets.len()
    );

    // Every multi the client knows: the houses, the ships, the boats. Read once
    // and shared by every facet, since a multi is a shape rather than a place.
    //
    // A failure here is a shard with no *houses*, not a shard with no world — an
    // install can predate the format, and the alternative is refusing to boot
    // over a feature nobody may be using. Said out loud for that reason.
    let multis = match openshard_uofiles::multi::Multis::load(dir) {
        Ok(multis) => {
            eprintln!(
                "world load +{:.3}s: {} multis read",
                started.elapsed().as_secs_f64(),
                multis.len()
            );
            Some(multis)
        }
        Err(error) => {
            warn!(%error, "could not read the client's multis; no house can be placed");
            None
        }
    };

    let mut world = configured_world(config).with_tiles(tiles.clone(), multis);
    for &facet in &config.world.facets {
        let facet = openshard_protocol::world::Facet(facet);
        // The map before the navigation artifact, because the artifact is now
        // checked against the snapshot's revision as well as its input files:
        // there is nothing to check it against until the world is loaded.
        eprintln!(
            "world load +{:.3}s: reading facet {facet}",
            started.elapsed().as_secs_f64()
        );
        let map = openshard_map::MapSnapshot::load_facet(dir, facet)?;
        let stamp = openshard_movement::bake::stamp_of(dir, facet, map.revision())?;
        let navigation_path = openshard_movement::bake::artifact_path(dir, facet);
        let coarse = openshard_movement::bake::load(&navigation_path, &stamp).map_err(|error| {
            format!(
                "{error}\ncreate it with: OPENSHARD_CLIENT={:?} cargo run --release -p \
                 openshard-movement --bin openshard-navigation-bake -- --facet {facet}",
                dir
            )
        })?;
        if coarse.dimensions() != (map.map().width(), map.map().height()) {
            return Err(format!(
                "navigation artifact {} has dimensions {}x{}, but facet {facet} is {}x{}\n\
                 recreate it with: OPENSHARD_CLIENT={:?} cargo run --release -p openshard-movement \
                 --bin openshard-navigation-bake -- --facet {facet}",
                navigation_path.display(),
                coarse.dimensions().0,
                coarse.dimensions().1,
                map.map().width(),
                map.map().height(),
                dir,
            )
            .into());
        }
        eprintln!(
            "world load +{:.3}s: facet {facet} ready",
            started.elapsed().as_secs_f64()
        );
        // The start is only checked against facet 0, where new characters spawn.
        // A start off the map, or in the sea, is worth saying out loud: the shard
        // still runs and every player spawns somewhere useless.
        if facet.0 == 0 {
            match map.map().land(start.0, start.1) {
                Some(cell) => info!(x = start.0, y = start.1, z = cell.z, "start position"),
                None => warn!(
                    x = start.0,
                    y = start.1,
                    "world.start is off the map; characters will spawn in nowhere"
                ),
            }
        }
        info!(
            facet = facet.0,
            name = map.map().facet_name(),
            size = format!("{}x{}", map.map().width(), map.map().height()),
            statics = map.map().static_count(),
            "facet loaded"
        );
        world = world.with_facet(facet, MapTerrain::new(map, tiles.clone()), Some(coarse));
    }
    info!(
        facets = config.world.facets.len(),
        tiledata = ?tiles.format(),
        took = ?started.elapsed(),
        "world loaded"
    );
    Ok(world)
}
