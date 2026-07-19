use super::*;

/// Write snapshots, forever, on a task nothing waits for.
///
/// # This is the only place that touches a disk
///
/// And it is deliberately somewhere the tick cannot reach. The world hands over
/// owned values and moves on; whatever happens here — a slow disk, a lock, a
/// database in another country — happens to this task and to nothing else. A
/// shard whose store is wedged saves late. It does not lag, and it does not stop
/// letting people play.
///
/// # A failed write is reported, not retried here
///
/// Retrying from here would write the same stale snapshot at a world that has
/// moved on. The failure goes back to the shard loop, which asks the world for a
/// full sweep — see `World::resweep`. The cost of a failure is a fat save, and
/// the recovery reads the world as it is now rather than as it was.
pub(crate) async fn save_loop(
    store: Arc<dyn Store>,
    mut snapshots: mpsc::UnboundedReceiver<Snapshot>,
    failures: mpsc::UnboundedSender<()>,
) {
    while let Some(snapshot) = snapshots.recv().await {
        let rows = snapshot.len();
        let started = Instant::now();
        match store.save(&snapshot).await {
            Ok(()) => debug!(
                tick = snapshot.tick,
                rows,
                took = ?started.elapsed(),
                "saved"
            ),
            Err(error) => {
                error!(tick = snapshot.tick, rows, %error, "save failed; the next one will be a full sweep");
                // If the shard loop is gone there is nobody to sweep and nothing
                // to do about it. The `let _` is that, not carelessness.
                let _ = failures.send(());
            }
        }
    }
}

/// Drive login and the world until the gateway stops.
///
/// One task owns both. That is not a limitation: the world is deliberately
/// single-threaded — a deterministic tick is the whole point — and login is a
/// state machine that does no work worth parallelising. Async lives in the
/// gateway's tasks, on the far side of the channel.
pub(crate) async fn run_shard(
    mut events: mpsc::UnboundedReceiver<ServerEvent>,
    config: &Config,
    advertised: SocketAddrV4,
    mut world: World,
    store: Arc<dyn Store>,
) {
    let (saves, snapshots) = mpsc::unbounded_channel();
    let (failed, mut failures) = mpsc::unbounded_channel();

    let mut accounts = DevAccounts::new();
    for account in &config.accounts {
        accounts = accounts.with_account(&account.name, &account.password);
        for character in &account.characters {
            accounts = accounts.with_character(&account.name, character);
        }
        // An unparseable access level is logged and left a player — authority is
        // never granted by a typo.
        match account.access.parse::<AccessLevel>() {
            Ok(AccessLevel::Player) => {}
            Ok(level) => accounts = accounts.with_access(&account.name, level),
            Err(error) => {
                warn!(account = account.name, %error, "unknown access level; treating as player")
            }
        }
    }

    // Bring the world back from the database: reserve every stored serial so a new
    // character cannot take one, list the stored characters so they show up to
    // play, and keep their records so playing one restores it where it was. This
    // borrows the store; the save task takes ownership after, so the load has to
    // come first.
    for account in &config.accounts {
        let record = AccountRecord {
            name: account.name.clone(),
            credential: account.password.clone(),
        };
        if let Err(error) = store.put_account(&record).await {
            warn!(account = account.name, %error, "could not persist a configured account");
        }
    }
    let mut saved: HashMap<(String, String), CharacterRecord> = HashMap::new();
    match store.characters().await {
        Ok(characters) => {
            for record in characters {
                world.reserve_serial(record.serial);
                let listed = accounts
                    .characters(&record.account)
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(&record.name));
                if !listed {
                    accounts = accounts.with_character(&record.account, &record.name);
                }
                saved.insert(
                    (record.account.to_lowercase(), record.name.to_lowercase()),
                    record,
                );
            }
            if !saved.is_empty() {
                info!(
                    characters = saved.len(),
                    "restored the world from the database"
                );
            }
        }
        Err(error) => error!(%error, "could not read saved characters; starting with none"),
    }

    // Bring back saved items: the world reserves their serials, drops the loose
    // ground clutter back where it lay, and files each character's carried
    // inventory to re-equip when it logs in. After the characters, so their
    // serials are already reserved and an item can point at the container it was in.
    match store.items().await {
        Ok(items) => {
            if !items.is_empty() {
                info!(items = items.len(), "restored saved items");
            }
            world.restore_items(items);
        }
        Err(error) => error!(%error, "could not read saved items; starting with none"),
    }

    // Bring back the world's NPC mobiles — townsfolk, vendors, creatures — each
    // exactly as saved. After the items, so each mobile's gear and stock is
    // already filed under its serial for `restore_mobiles` to equip. This is the
    // whole-world model: the pack seeds a fresh world once (a staff Populate),
    // and from then on the save is the truth — nothing respawns at boot.
    match store.mobiles().await {
        Ok(mobiles) => {
            if !mobiles.is_empty() {
                info!(mobiles = mobiles.len(), "restored the world's mobiles");
            }
            world.restore_mobiles(mobiles);
        }
        Err(error) => error!(%error, "could not read saved mobiles; starting with none"),
    }

    // And the placed decoration, door state and all.
    match store.decorations().await {
        Ok(decorations) => {
            if !decorations.is_empty() {
                info!(
                    decorations = decorations.len(),
                    "restored the world's decoration"
                );
            }
            world.restore_decorations(decorations);
        }
        Err(error) => error!(%error, "could not read saved decorations; starting with none"),
    }

    // Bring back the spawn regions with their respawn timers, so a populated area
    // stays populated across a restart and a rare spawn keeps its remaining wait
    // rather than popping again the moment the shard comes up.
    match store.spawners().await {
        Ok(spawners) => {
            if !spawners.is_empty() {
                info!(spawners = spawners.len(), "restored spawn regions");
            }
            world.restore_spawners(spawners);
        }
        Err(error) => error!(%error, "could not read saved spawners; starting with none"),
    }

    let mut login = LoginServer::new(accounts, &config.server.name, advertised);
    // The character-creation screen needs somewhere to start. Without it the
    // client refuses to create at all — "No city found. Something wrong with the
    // received cities." — because the list it was sent is empty. The list is
    // filtered to the facets this shard loaded, so every city offered is one a
    // player can actually be placed in.
    login.starts = start_cities(
        &config.world.facets,
        (config.world.start.x, config.world.start.y),
    );

    // Kept, not detached: shutdown hands it a final snapshot, closes the channel,
    // and awaits this task so every queued write lands before the process exits.
    let save_task = tokio::spawn(save_loop(store, snapshots, failed));

    // The gameplay script, if one is configured. Constructed after the world is
    // built and restored, before the first tick, so its cursors start clean.
    let mut scripts = Scripts::load(&config.scripting.main, &world);

    let mut sessions: HashMap<ConnectionId, Session> = HashMap::new();
    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    // A tick that ran late must not try to catch up by running several in a row:
    // that turns a hiccup into a stall, and a fixed timestep into a variable one.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Biased so the tick cannot be starved by a busy network. Without
            // this, a flood of packets would keep `recv` ready forever and the
            // world would stop simulating under exactly the load that needs it.
            biased;

            _ = ticker.tick() => {
                world.tick(Instant::now());
                for out in world.drain_outbound() {
                    if let Some(session) = sessions.get(&out.connection) {
                        // A connection reaches the world only after its game
                        // login, so this is always a game connection and every
                        // packet leaves compressed. `send_packet` gates on the
                        // flag anyway, so it stays correct if that ever changes.
                        let _ = session.send_packet(out.packet);
                    }
                }
                // Handed off, not awaited. The tick's job here is to stop
                // holding the only copy.
                for snapshot in world.drain_saves() {
                    let _ = saves.send(snapshot);
                }
                // Keep the in-memory character list current with logouts, so a
                // re-login this run finds a character where it left, not where it
                // was at boot. The store gets the same record via the snapshot
                // above; this is the copy a re-login can read before that lands.
                for record in world.drain_departed() {
                    let key = (record.account.to_lowercase(), record.name.to_lowercase());
                    saved.insert(key, record);
                }
                // Feed the script this tick's events and queue its commands for
                // the next one. After the drains, so a command a script emits is
                // applied by a tick and leaves through this same path.
                if let Some(scripts) = scripts.as_mut() {
                    scripts.pump(&mut world);
                }
            }

            // Before `events`: a store that is failing is worth hearing about
            // ahead of the next packet, and there is never a queue of these.
            Some(()) = failures.recv() => {
                warn!("a save failed; marking the world for a full sweep");
                world.resweep();
            }

            // Ctrl-C: leave the loop and save the world on the way out, rather than
            // dying with the last save cadence's worth of play unwritten.
            _ = tokio::signal::ctrl_c() => {
                info!("shutdown requested; saving the world");
                break;
            }

            event = events.recv() => {
                let Some(event) = event else {
                    error!("the gateway stopped; saving the world");
                    break;
                };
                handle(&mut sessions, &mut login, &mut world, advertised, &saved, event);
            }
        }

        // Reclaim keys from clients that selected a shard and never came back.
        login.keys.expire(Instant::now());
    }

    // Shutdown: one last full snapshot, then flush every queued write before the
    // process exits. This is the one moment a lost write costs a player real value,
    // so unlike the per-tick handoff it is *awaited*. Dropping the sender ends the
    // save task's receive loop once it has drained what is left.
    world.take_snapshot();
    for snapshot in world.drain_saves() {
        let _ = saves.send(snapshot);
    }
    drop(saves);
    if let Err(error) = save_task.await {
        error!(%error, "the save task did not finish cleanly on shutdown");
    }
    info!("world saved; shutting down");
}

/// Whether the relay is about to send this client somewhere it cannot get back
/// from.
///
/// True when `advertise` is loopback and the client is not on this machine: the
/// relay will tell it to dial `127.0.0.1`, it will reach its own loopback, find
/// nothing, and give up.
///
/// # Why this is worth catching here and not only at startup
///
/// The startup warning fires before anyone has connected, and it scrolls away.
/// By the time the mistake is *made* — the moment a client that is not on this
/// machine picks the shard — the warning is a hundred lines up, and what the
/// operator is looking at is a client stuck on "logging into shard" and a server
/// log that says nothing is wrong.
///
/// And nothing here *is* wrong, which is the whole difficulty. This end sends a
/// perfectly good packet and never sees a second connection, because the failure
/// happens somewhere it cannot observe. This is the last moment the shard can
/// still see both addresses at once and say what is about to happen.
pub(crate) fn relay_is_unreachable(client: SocketAddr, advertised: SocketAddrV4) -> bool {
    advertised.ip().is_loopback() && !client.ip().is_loopback()
}

pub(crate) fn handle(
    sessions: &mut HashMap<ConnectionId, Session>,
    login: &mut LoginServer<DevAccounts>,
    world: &mut World,
    advertised: SocketAddrV4,
    saved: &HashMap<(String, String), CharacterRecord>,
    event: ServerEvent,
) {
    match event {
        ServerEvent::Connected {
            id,
            address,
            outbox,
            control,
        } => {
            info!(%id, %address, "connected");
            if relay_is_unreachable(address, advertised) {
                error!(
                    client = %address,
                    %advertised,
                    "this client is not on this machine and server.advertise is loopback. \
                     When it picks the shard it will be told to dial {advertised} — its own \
                     loopback — and will hang on \"logging into shard\" until it times out. \
                     Set server.advertise to the address this client can reach."
                );
            }
            sessions.insert(
                id,
                Session {
                    login: LoginSession::new(),
                    in_world: false,
                    game: false,
                    outbox,
                    control,
                },
            );
        }

        ServerEvent::Received { id, event } => {
            let Some(session) = sessions.get_mut(&id) else {
                // Disconnected arrived first. Possible: the gateway's tasks and
                // this loop are not synchronised.
                return;
            };
            match event {
                Event::Seeded(seed) => session.login.on_seed(seed),
                Event::Packet(packet) => {
                    // The game login is the seam Sphere calls CONNECT_GAME: from
                    // here on, this connection's every server->client packet is
                    // Huffman-compressed — starting with the character list this
                    // very packet triggers. Set the flag before the reply is
                    // built so that reply goes out compressed.
                    if packet.first().copied() == Some(GameServerLogin::ID) {
                        session.game = true;
                    }
                    // Character creation crosses the login/world line: it writes
                    // the new character onto the account and then enters the world
                    // with it. Handle it here, where both are in reach — dispatch
                    // sees only the world, and the login state machine is done.
                    if matches!(
                        packet.first().copied(),
                        Some(CreateCharacter::ID_CLASSIC | CreateCharacter::ID_HIGH_SEAS)
                    ) {
                        if !create_character(session, login, world, &packet, id) {
                            sessions.remove(&id);
                        }
                        return;
                    }
                    // The account's authority, looked up where the store is in
                    // reach and passed to the world so the GM command gate has it.
                    // A player by default; only the store grants more.
                    let access = session
                        .login
                        .account()
                        .map_or(AccessLevel::Player, |a| login.accounts.access_level(a));
                    if !dispatch(session, world, &packet, id, saved, access) {
                        sessions.remove(&id);
                        return;
                    }
                    let response = login.handle(&mut session.login, &packet, Instant::now());
                    if !session.apply(response, id) {
                        sessions.remove(&id);
                    }
                }
            }
        }

        ServerEvent::Disconnected { id, reason } => {
            match reason {
                Some(reason) => warn!(%id, %reason, "disconnected"),
                None => info!(%id, "disconnected"),
            }
            // The world learns on its own schedule. It owns the entity and the
            // serial, and tearing them down from here would be a write to the
            // world from outside the tick.
            world.queue(Command::Disconnect { connection: id });
            sessions.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(address: &str) -> SocketAddr {
        address.parse().expect("a client address")
    }

    fn advertise(address: &str) -> SocketAddrV4 {
        address.parse().expect("an advertised address")
    }

    #[test]
    fn a_loopback_advertise_is_unreachable_to_a_client_on_the_network() {
        // The bug this exists for: the client dials its own loopback and hangs
        // on "logging into shard" while this end sees one connection, a clean
        // disconnect, and nothing to explain either.
        assert!(relay_is_unreachable(
            client("192.168.11.163:51606"),
            advertise("127.0.0.1:2593")
        ));
    }

    #[test]
    fn a_loopback_advertise_is_fine_for_a_client_on_this_machine() {
        // And this is why the shard does not simply refuse to start on a
        // loopback advertise: a developer with the client on their own desk is
        // the common case, and it works.
        assert!(!relay_is_unreachable(
            client("127.0.0.1:51606"),
            advertise("127.0.0.1:2593")
        ));
    }

    #[test]
    fn a_real_advertise_is_fine_for_anyone() {
        for address in ["127.0.0.1:51606", "192.168.11.163:51606", "8.8.8.8:51606"] {
            assert!(
                !relay_is_unreachable(client(address), advertise("192.168.11.10:2593")),
                "{address} should be able to reach an advertised LAN address"
            );
        }
    }

    #[test]
    fn an_ipv6_loopback_client_is_still_on_this_machine() {
        // `::1` is loopback and the obvious check — comparing against the string
        // "127.0.0.1", or against Ipv4Addr::LOCALHOST — misses it, and fires a
        // scary error at a developer whose client happens to have resolved
        // localhost to v6.
        assert!(!relay_is_unreachable(
            client("[::1]:51606"),
            advertise("127.0.0.1:2593")
        ));
    }
}
