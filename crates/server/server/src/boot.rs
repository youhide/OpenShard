use openshard_map::grid::Tile;

use super::*;

mod gameplay;
#[allow(unused_imports)] // Kept as boot's crate-visible configuration API.
pub(crate) use gameplay::{
    character_list_flags_of,
    character_screen_of,
    gameplay_of,
    supported_features_of,
};

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
pub async fn open_store(config: &Config) -> Result<Arc<Store>, Box<dyn std::error::Error>> {
    let target = config.persistence.database.trim();
    if target.is_empty() {
        warn!(
            "no database configured: the world is kept in memory and lost at stop. \
             Set persistence.database to a file (SQLite) or a postgres:// URL to keep \
             characters across a restart."
        );
        return Ok(Arc::new(Store::memory()));
    }
    if is_postgres_url(target) {
        // The URL can carry a password, so it is never logged — only that this is
        // the PostgreSQL backend.
        let store = PgStore::connect(target)
            .await
            .map_err(|error| format!("could not connect to PostgreSQL: {error}"))?;
        info!("persisting to PostgreSQL");
        return Ok(Arc::new(Store::postgres(store)));
    }
    let store = SqliteStore::open(target)
        .map_err(|error| format!("could not open the database at {target:?}: {error}"))?;
    info!(path = target, "persisting to SQLite");
    Ok(Arc::new(Store::sqlite(store)))
}

/// Whether `persistence.database` names a PostgreSQL server rather than a SQLite
/// file. The two `postgres` spellings are the ones libpq itself accepts.
pub(crate) fn is_postgres_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("postgres://") || lower.starts_with("postgresql://")
}

/// The world the config asks for, before a map or a save is laid over it.
///
/// Both of [`load_world`]'s paths — with a map and without — come through here, so
/// a knob added to `[world]` cannot be wired into one branch and forgotten in the
/// other. That drift is silent: the mapless mode is the one tests and a first run
/// use, so the branch that gets it right is not the branch anyone notices.
fn configured_world(config: &Config) -> Result<World, Box<dyn std::error::Error>> {
    let world = World::new(Tile::new(config.world.start.x, config.world.start.y))
        .with_gameplay(gameplay_of(config))
        .with_character_screen(character_screen_of(config)?)
        .with_save_seconds(config.persistence.save_seconds);
    // Only when the operator pinned one. There is no `u64` that means "no seed", so
    // an absent `world.seed` has to leave the world's own default in place rather
    // than pass a stand-in through.
    Ok(match config.world.seed {
        Some(seed) => {
            info!(
                seed,
                "world.seed is pinned: a fresh world's rolls are reproducible"
            );
            world.with_seed(seed)
        }
        None => world,
    })
}

/// Everything a shard needs before its first tick that has to be read off a
/// disk: the accounts, and a world with the last save laid over it.
///
/// Two values rather than one because they are two owners — the accounts go to
/// [`LoginServer`], the world to the tick — and the only thing they share is that
/// both are finished by the time the loop starts.
pub(crate) struct Restored {
    pub(crate) accounts: DevAccounts,
    pub(crate) world:    World,
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
/// order rides on it. S6 of
/// `docs/server/evidence/2026-07-31-invariants-nothing-enforces.md` has the
/// argument, and `docs/server/design_persistence.md` states the order as built.
///
/// Nothing here is fatal. A store that cannot be read is logged at each step and
/// the shard comes up with whatever it did get: a shard that refuses to start
/// because one table is unreadable helps nobody, and the alternative to a
/// partially restored world is no world at all.
pub(crate) async fn restore(store: &Store, config: &Config, world: World) -> Restored {
    let accounts = load_accounts(store, config).await;
    let world = restore_saved_world(store, config, world).await;
    Restored { accounts, world }
}

/// Lay the persisted world over a freshly loaded map without starting the shard.
///
/// This is the world half of [`restore`], exposed separately for read-only
/// diagnostic binaries. It deliberately does not load or seed accounts: a probe
/// that only asks the movement authority a question must not create an account
/// row as a side effect.
pub async fn restore_saved_world(store: &Store, config: &Config, world: World) -> World {
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
    restore_world(store, world, config.world.seed).await
}

/// Accounts come from the store first — their credentials are the argon2
/// hashes saved there — and config seeds the rest. The store is authoritative
/// for a password once it has one, so a config `[[accounts]]` line only
/// creates an account the store has never seen; changing a config password
/// after the first boot does nothing (the shard says as much in the docs).
async fn load_accounts(store: &Store, config: &Config) -> DevAccounts {
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
/// both that the character exists and where it was, and the roster is what holds
/// each — `docs/server/design_connection_state.md` D5. The accounts keep
/// credentials and authority — what a login is about — and nothing that a
/// character screen would read.
/// A store that cannot be read is not a reason to skip the items: the restore
/// still ran, with nothing in it, and the token says so. Returning an `Option`
/// here would put the ordering rule back in prose — the caller would have to know
/// that "no characters" still permits items.
async fn restore_characters(store: &Store, world: &mut World) -> RestoredCharacters {
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
async fn restore_items(store: &Store, world: &mut World, characters: &RestoredCharacters) -> RestoredItems {
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
async fn restore_mobiles(store: &Store, world: &mut World, items: &RestoredItems) {
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
async fn restore_decorations(store: &Store, world: &mut World) {
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
async fn restore_spawners(store: &Store, world: &mut World) {
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
async fn restore_guilds(store: &Store, world: &mut World) {
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
async fn restore_regions(store: &Store, world: &mut World) {
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
async fn restore_world(store: &Store, world: World, pinned_seed: Option<u64>) -> World {
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

/// Where one facet's world came from, and everything that follows from it.
///
/// The four fields travel together because they are one decision: a facet read
/// from a base set is stamped against the base set, its navigation artifact
/// lives beside the base set, and the command that rebuilds that artifact names
/// the base set. Splitting them would let a shard stamp one world and look for
/// the artifact of another — and both files exist, so it would not notice.
struct FacetSource {
    /// The facet, at the revision its source recorded.
    map:             openshard_map::snapshot::MapSnapshot,
    /// What the navigation artifact must have been built from.
    stamp:           openshard_movement::bake::Stamp,
    /// Where that artifact is.
    navigation_path: std::path::PathBuf,
    /// The patch log beside the base set, when there is one on disk.
    ///
    /// The artifact's one forgivable input: a graph baked before the last few
    /// edits was stamped over a shorter log than the world now has, and it is the
    /// log itself that says how to carry it forward. `None` is a facet out of the
    /// install, or a world of ours nobody has edited yet — neither can be behind.
    log:             Option<std::path::PathBuf>,
    /// The command that makes it, for the error that says it is missing.
    rebake:          String,
    /// Where this facet's world lives, when it is a world of ours.
    ///
    /// `None` for a facet read out of the install: there is nowhere beside it to
    /// keep a patch log, so it is a facet nothing can edit while the shard runs.
    /// See [`openshard_state::WorldHome`].
    home:            Option<openshard_state::WorldHome>,
}

/// The ruleset `facet` runs under: the operator's answer where there is one, and
/// the answer the facet's number meant in retail where there is not.
///
/// The fall-back is [`FacetRules::classic`] rather than any value spelled here,
/// because the same default has to be the one the *client* assumes — see its
/// doc, which is where that argument lives.
fn rules_of(config: &Config, facet: Facet) -> FacetRules {
    match config.world.free_movement.get(&openshard_config::FacetKey(facet)) {
        Some(&free_movement) => FacetRules { free_movement },
        None => FacetRules::classic(facet),
    }
}

/// Read one facet, from whichever source the config named for it.
///
/// The stamp is the half that matters. Before base sets there was one source,
/// so stamping "the install's map files" was the same statement as "what this
/// graph was built from". It is not any more: a facet read from a base set is
/// derived from that file, and the install's `map0LegacyMUL.uop` still sits
/// there with its old length and its old mtime — so a graph baked over the
/// install would validate happily against a world it has never seen. That is
/// `docs/world/evidence/2026-08-25-seven-directions.md`'s direction D arriving one caller
/// early, and it is the reason this function exists rather than an `if` at the
/// call site.
fn facet_source(
    config: &Config,
    dir: &Path,
    facet: Facet,
) -> Result<FacetSource, Box<dyn std::error::Error>> {
    let bake = "cargo run --release -p openshard-movement --bin openshard-navigation-bake";
    // A facet not in `world.base_sets` comes out of the install exactly as
    // before, so a shard converts one facet at a time.
    let base_set = config.world.base_set(facet);
    if let Some(base_set) = base_set {
        eprintln!("world load: reading facet {facet} from {}", base_set.display());
    }
    // The one resolution the navigation bake and the client also go through: it
    // reads the base set *and* the log beside it, refuses a file that turns out
    // to be another facet, and answers where things derived from this world
    // live. `tiledata.mul` is still the install's either way — a base set holds
    // the map, and what a tile *means* is the tile table's — which is why the
    // config refuses a base set without client files and `dir` is a real one
    // here.
    let source = base_set.map_or(
        openshard_movement::bake::WorldSource::Install,
        openshard_movement::bake::WorldSource::BaseSet,
    );
    let world = openshard_movement::bake::FacetWorld::read(dir, source, facet)?;
    if world.patches != 0 {
        eprintln!(
            "world load: {} patch(es) applied to facet {facet}; it is at revision {}",
            world.patches,
            world.snapshot.revision().get()
        );
    }
    let stamp = world.stamp(dir, facet)?;
    let navigation_path = world.navigation_path(dir);
    let rebake = match base_set {
        Some(base_set) => {
            format!(
                "OPENSHARD_CLIENT={dir:?} {bake} -- --facet {facet} --base-set {:?}",
                base_set.display()
            )
        }
        None => format!("OPENSHARD_CLIENT={dir:?} {bake} -- --facet {facet}"),
    };
    Ok(FacetSource {
        navigation_path,
        stamp,
        log: world.log,
        // The base set's own revision, not the world's: it is the log's header,
        // and a patch committed while the shard runs is appended to that log.
        // `None` for a facet read out of the install, which is a facet nothing
        // can edit while the shard runs.
        home: base_set
            .map(|base_set| {
                // The identity is taken from the file rather than from the world
                // in hand: what a client files its cache under has to be the
                // same number after a restart, and a hash of the *bytes on disk*
                // is that whether or not a patch has moved the world since.
                // See `openshard_basemap::identity_of`.
                openshard_basemap::identity_of(base_set).map(|identity| {
                    openshard_state::WorldHome {
                        base_set: base_set.to_owned(),
                        base: world.base.expect("a facet read from a base set has one"),
                        identity,
                    }
                })
            })
            .transpose()?,
        map: world.snapshot,
        rebake,
    })
}

/// What a navigation artifact missed while it sat on disk.
struct Missed {
    /// The revision it was built from.
    from:   openshard_map::snapshot::MapRevision,
    /// The chunks every patch committed since then touched, each one once.
    chunks: Vec<openshard_map::chunk::ChunkCoord>,
}

/// Which chunks a graph built at `from` has to be rebaked over to stand for the
/// world its log has since carried it to.
///
/// **The union, and one rebake.** `NavigationGraph::rebake_chunks` rebuilds the
/// regions a chunk set covers, their neighbours, and a ring beyond for edges, so
/// the set derived from a union contains every set derived from a member of it —
/// and the ground is at its final revision either way, because the world was
/// loaded by applying the whole log before anything here ran. Replaying the
/// patches one at a time would rebake the same regions n times over the same
/// map.
///
/// # Errors
///
/// A message, and every one of them ends the same way: bake the graph whole. The
/// log is the authority on ancestry that `bake::load_behind` deliberately does
/// not claim — a file can say it is *below* the world's revision and nothing
/// more — so a gap the log cannot cover is found here and nowhere else.
fn missed_chunks(
    log: &Path,
    facet: Facet,
    base: openshard_map::snapshot::MapRevision,
    from: openshard_map::snapshot::MapRevision,
) -> Result<Missed, String> {
    if from.get() < base.get() {
        return Err(format!(
            "the navigation artifact for facet {facet} was built from map revision {}, and the \
             log beside the base set starts at {}: nothing on disk reaches back that far",
            from.get(),
            base.get(),
        ));
    }
    let committed = openshard_basemap::patches::read(log, facet, base).map_err(|source| {
        format!(
            "the navigation artifact for facet {facet} is behind the world, and the log that \
             would carry it forward could not be read: {source}"
        )
    })?;
    let mut chunks: Vec<openshard_map::chunk::ChunkCoord> = committed
        .iter()
        .filter(|patch| patch.revision().get() > from.get())
        .flat_map(|patch| patch.touched_chunks())
        .collect();
    chunks.sort_unstable();
    chunks.dedup();
    Ok(Missed { from, chunks })
}

/// Load the world, if it is configured.
///
/// Each facet comes from whichever source `world.base_sets` names for it — our
/// own format, or the install — and `facet_source` is where that is decided.
/// Everything else here is per-shard rather than per-facet: the tile table and
/// the multis are the install's either way.
///
/// Blocking, and on purpose: this reads over a hundred megabytes and takes a
/// moment, and there is no sense accepting a client before the world it will
/// walk in exists.
pub fn load_world(config: &Config) -> Result<World, Box<dyn std::error::Error>> {
    let start = Tile::new(config.world.start.x, config.world.start.y);
    let dir = config.world.client_files.trim();
    if dir.is_empty() {
        // `Config::validate` already refuses this, and it is checked again here
        // because `load_world` also takes configs nobody wrote down — a test's,
        // the playground's — and the alternative is loading none of the base
        // sets an operator named while saying only that there is no map.
        if let Some((facet, path)) = config.world.base_sets.iter().next() {
            return Err(format!(
                "world.base_sets names {} for facet {}, but world.client_files is empty: a base \
                 set holds the map, and tiledata.mul still holds what a tile is",
                path.display(),
                facet.0,
            )
            .into());
        }
        warn!(
            "world.client_files is empty: running with no map. Every step will be allowed — \
             players walk through walls and across water. Set it to a client install."
        );
        return configured_world(config);
    }

    let dir = Path::new(dir);
    let started = Instant::now();
    // One tile table for the shard, owned by it: `tiledata.mul` describes tiles,
    // not a map, so it is read once and never copied. It used to be *cloned* into
    // each facet's terrain, then shared behind an `Arc` with those terrains; the
    // facets hold only maps now, so the world is the single holder.
    eprintln!("world load: reading tiledata.mul from {}", dir.display());
    let read = openshard_uofiles::tiledata::load(dir.join("tiledata.mul"))?;
    // The layout the file turned out to be in: a fact about the read rather
    // than about the table, and it goes no further than the log below.
    let tiledata_format = read.format;
    let tiles = read.tiles;
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
            multis
        }
        Err(error) => {
            warn!(%error, "could not read the client's multis; no house can be placed");
            openshard_uofiles::multi::Multis::default()
        }
    };

    let mut world = configured_world(config)?.with_tiles(tiles, multis);
    match openshard_housing::template::load_directory(&dir.join("openshard-houses")) {
        Ok(templates) => {
            if !templates.is_empty() {
                eprintln!(
                    "world load +{:.3}s: {} custom house template(s) read",
                    started.elapsed().as_secs_f64(),
                    templates.len()
                );
            }
            world = world.with_house_templates(templates);
        }
        Err(error) => warn!(%error, "could not read custom house templates; none can be placed"),
    }
    for &facet in &config.world.facets {
        let facet = openshard_protocol::world::Facet(facet);
        // The map before the navigation artifact, because the artifact is now
        // checked against the snapshot's revision as well as its input files:
        // there is nothing to check it against until the world is loaded.
        eprintln!(
            "world load +{:.3}s: reading facet {facet}",
            started.elapsed().as_secs_f64()
        );
        let source = facet_source(config, dir, facet)?;
        let FacetSource {
            map,
            stamp,
            navigation_path,
            log,
            rebake,
            home,
        } = source;
        // A world of ours with a log can be *ahead* of its own artifact, and that
        // is ordinary rather than broken: the graph follows a patch on the tick
        // that commits it, and nothing writes the file until something bakes.
        // So the artifact is read as far behind as the log can carry it, and
        // caught up below — once the facet is in the world, where the span index
        // the rebake reads already exists.
        let (coarse, behind) = match (&home, &log) {
            (Some(home), Some(log)) => {
                let loaded = openshard_movement::bake::load_behind(
                    &navigation_path,
                    &stamp,
                    &openshard_movement::bake::file_name_of(log),
                )
                .map_err(|error| format!("{error}\ncreate it with: {rebake}"))?;
                let behind = (loaded.revision != stamp.revision)
                    .then(|| missed_chunks(log, facet, home.base, loaded.revision))
                    .transpose()
                    .map_err(|reason| format!("{reason}\nrecreate it with: {rebake}"))?;
                (loaded.graph, behind)
            }
            _ => {
                (
                    openshard_movement::bake::load(&navigation_path, &stamp)
                        .map_err(|error| format!("{error}\ncreate it with: {rebake}"))?,
                    None,
                )
            }
        };
        if coarse.dimensions() != (map.map().width(), map.map().height()) {
            return Err(format!(
                "navigation artifact {} has dimensions {}x{}, but facet {facet} is {}x{}\n\
                 recreate it with: {rebake}",
                navigation_path.display(),
                coarse.dimensions().0,
                coarse.dimensions().1,
                map.map().width(),
                map.map().height(),
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
            match map.map().land(start.x, start.y) {
                Some(cell) => info!(x = start.x, y = start.y, z = cell.z, "start position"),
                None => {
                    warn!(
                        x = start.x,
                        y = start.y,
                        "world.start is off the map; characters will spawn in nowhere"
                    )
                }
            }
        }
        info!(
            facet = facet.0,
            name = map.map().facet_name(),
            size = format!("{}x{}", map.map().width(), map.map().height()),
            statics = map.map().static_count(),
            revision = map.revision().get(),
            source = %config.world.base_set(facet).map_or_else(
                || "client files".to_owned(),
                |path| path.display().to_string()
            ),
            "facet loaded"
        );
        world = world.with_facet(facet, map, Some(coarse), rules_of(config, facet), home);
        // Now the facet has its ground and its span index, which is what the
        // rebake reads — and the graph in hand is the one the file held, so this
        // is the last moment anything is behind.
        if let Some(missed) = behind {
            let began = Instant::now();
            let graph = world
                .catch_up(facet, &missed.chunks)
                .expect("a facet just loaded with a graph has one");
            let took = began.elapsed();
            // Written back, or the same chunks are rebaked at every start and the
            // file never moves. A failure here is not one the shard has to die of:
            // what is in memory is the world as it stands, and the next boot pays
            // this again rather than getting it wrong.
            match openshard_movement::bake::save(&navigation_path, graph, &stamp) {
                Ok(bytes) => {
                    info!(
                        facet = facet.0,
                        from = missed.from.get(),
                        to = stamp.revision.get(),
                        chunks = missed.chunks.len(),
                        ?took,
                        bytes,
                        "navigation artifact caught up with the patch log"
                    )
                }
                Err(error) => {
                    warn!(
                        facet = facet.0,
                        %error,
                        "the caught-up navigation artifact could not be written back; the shard runs \
                         on the graph in memory and the next start will catch it up again"
                    )
                }
            }
        }
    }
    info!(
        facets = config.world.facets.len(),
        tiledata = ?tiledata_format,
        took = ?started.elapsed(),
        "world loaded"
    );
    Ok(world)
}
