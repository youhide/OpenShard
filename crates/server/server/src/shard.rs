use std::sync::atomic::{AtomicUsize, Ordering};

use openshard_events::Cursor;

use super::*;

/// What the save task has been handed and has not finished writing.
///
/// # Why anything counts this
///
/// D2 of `docs/shutdown.md`: the second stop signal is a force-exit, and it owes
/// the operator a line naming what their impatience cost. Without this the line
/// can only say that the save did not finish, which is the one thing they
/// already know — they are the ones who did not wait.
///
/// # It is a number for a log line, and nothing branches on it
///
/// Hence [`Ordering::Relaxed`] throughout, and hence two counters read one after
/// the other rather than one lock: no data is published through these, and a
/// reader that catches the pair mid-update reports a count that was true a
/// moment earlier. That is the correct amount of care for a diagnostic, and
/// paying more for it would suggest something depends on it.
///
/// A write that is *in flight* is counted as unwritten, because at a force-exit
/// that is what it is: `store.save` has not returned, so whether the rows landed
/// is exactly the question nobody can answer.
#[derive(Debug, Clone, Default)]
pub struct Unwritten(Arc<UnwrittenCounts>);

#[derive(Debug, Default)]
struct UnwrittenCounts {
    writes: AtomicUsize,
    rows: AtomicUsize,
}

impl Unwritten {
    /// Nothing queued and nothing written yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many snapshots have been handed over and not yet written.
    ///
    /// One snapshot is one `Store::save`, which is one transaction — so this is
    /// the number of *writes* D2 promises to name.
    pub fn writes(&self) -> usize {
        self.0.writes.load(Ordering::Relaxed)
    }

    /// How many rows are inside those snapshots.
    ///
    /// Beside [`Unwritten::writes`] because "three writes" tells an operator
    /// nothing about what they lost and "three writes, 12,000 rows" tells them
    /// whether it was a quiet minute or a full sweep.
    pub fn rows(&self) -> usize {
        self.0.rows.load(Ordering::Relaxed)
    }

    /// One more snapshot is outstanding.
    fn queued(&self, rows: usize) {
        self.0.writes.fetch_add(1, Ordering::Relaxed);
        self.0.rows.fetch_add(rows, Ordering::Relaxed);
    }

    /// One fewer is: the store answered, or the snapshot never reached the queue
    /// at all. Both are "nothing is waiting on it any more", which is the only
    /// thing this counts.
    ///
    /// Always paired with a [`Unwritten::queued`] of the same `rows`, which is
    /// what keeps the subtraction from going below zero — and it is raised
    /// first, in [`SnapshotTx::send`], so a save task that finishes a snapshot
    /// before its sender returns cannot subtract from a count that has not been
    /// added to yet.
    fn cleared(&self, rows: usize) {
        self.0.writes.fetch_sub(1, Ordering::Relaxed);
        self.0.rows.fetch_sub(rows, Ordering::Relaxed);
    }
}

/// What the outside world holds of a running shard: the one word that stops it,
/// and the tally of what its save task still owes the disk.
///
/// # Why they travel together
///
/// They are already handed to the same two places. [`run_shard`] watches the
/// word and counts into the tally; `stop::watch` says the word on the first
/// signal and reads the tally on the second. Neither may live *inside* the
/// shard, and for one reason: the force-exit of `docs/shutdown.md` D2 has to
/// work at the moment `run_shard` is not going to return, so what reads the
/// tally cannot be owned by the thing that is stuck.
///
/// A caller with no way to force-exit — every test — builds one with
/// [`Reins::new`] or [`Reins::over`] and never looks at it. That is the point of
/// the type: it was two arguments passed blind, and a signature stops being
/// readable long before it stops compiling.
///
/// Cloning hands out another hold on the same shard, the way cloning a
/// [`Shutdown`] does; there is no owner among the clones.
#[derive(Debug, Clone)]
pub struct Reins {
    shutdown: Shutdown,
    unwritten: Unwritten,
}

impl Reins {
    /// A shard nobody has asked to stop, owing nothing.
    pub fn new() -> Self {
        Self::over(Shutdown::new())
    }

    /// The same, over a [`Shutdown`] that already exists.
    ///
    /// For a caller that made the stop before the shard — the binary, which
    /// binds the gateway with it, and the e2e harness, whose `Running` holds it
    /// so a test can stop a shard it handed away.
    pub fn over(shutdown: Shutdown) -> Self {
        Self {
            shutdown,
            unwritten: Unwritten::new(),
        }
    }

    /// The word that stops this shard. A clone, like every other hold on it.
    pub fn shutdown(&self) -> Shutdown {
        self.shutdown.clone()
    }

    /// The tally of what the save task has been handed and not written.
    pub fn unwritten(&self) -> Unwritten {
        self.unwritten.clone()
    }
}

impl Default for Reins {
    fn default() -> Self {
        Self::new()
    }
}

/// Sender half of the tick's outbound-snapshot channel. Only the tick loop in
/// `run_shard` ever has a snapshot to hand off, so this stays private to the
/// module rather than a bare `UnboundedSender` some unrelated `Snapshot`
/// producer could be handed by mistake.
///
/// It carries the [`Unwritten`] tally because this is the only place a snapshot
/// enters the queue: counting here rather than at each call site is the same
/// argument as marking persistence dirty from the event bus — a `queued()`
/// beside every `send` works, and then one is forgotten.
#[derive(Debug, Clone)]
pub(crate) struct SnapshotTx {
    snapshots: mpsc::UnboundedSender<Snapshot>,
    unwritten: Unwritten,
}

impl SnapshotTx {
    // Boxed: a bare `SendError<Snapshot>` carries a whole `Snapshot` by value,
    // which is the failure case's problem alone — the caller here only ever
    // checks `is_err()` and never wants that weight on the stack.
    fn send(&self, snapshot: Snapshot) -> Result<(), Box<mpsc::error::SendError<Snapshot>>> {
        // Counted before the send, and only kept if the send took it: a snapshot
        // the channel refused was never handed over, so it is not outstanding
        // work — it is work that went nowhere, and the receiver that would have
        // decremented it is gone.
        let rows = snapshot.len();
        self.unwritten.queued(rows);
        self.snapshots.send(snapshot).map_err(|error| {
            self.unwritten.cleared(rows);
            Box::new(error)
        })
    }
}

/// Receiver half of [`SnapshotTx`], drained by [`save_loop`].
///
/// It carries the same [`Unwritten`] the sender does — one tally, counted up on
/// one side and down on the other.
#[derive(Debug)]
pub(crate) struct SnapshotRx {
    snapshots: mpsc::UnboundedReceiver<Snapshot>,
    unwritten: Unwritten,
}

impl SnapshotRx {
    async fn recv(&mut self) -> Option<Snapshot> {
        self.snapshots.recv().await
    }
}

fn snapshot_channel(unwritten: Unwritten) -> (SnapshotTx, SnapshotRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        SnapshotTx {
            snapshots: tx,
            unwritten: unwritten.clone(),
        },
        SnapshotRx {
            snapshots: rx,
            unwritten,
        },
    )
}

/// Sender half of the save task's failure-signal channel — a save failed and
/// the tick loop should mark the world for a full resweep. The unit payload
/// makes it easy to reach for a bare `mpsc::unbounded_channel::<()>()` at a
/// call site that also happens to touch snapshots; the newtype keeps the two
/// from being confused for each other by the compiler as well as the reader.
#[derive(Debug, Clone)]
pub(crate) struct FailureTx(mpsc::UnboundedSender<()>);

impl FailureTx {
    fn send(&self) -> Result<(), mpsc::error::SendError<()>> {
        self.0.send(())
    }
}

/// Receiver half of [`FailureTx`], read by the tick loop in `run_shard`.
#[derive(Debug)]
struct FailureRx(mpsc::UnboundedReceiver<()>);

impl FailureRx {
    async fn recv(&mut self) -> Option<()> {
        self.0.recv().await
    }
}

fn failure_channel() -> (FailureTx, FailureRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (FailureTx(tx), FailureRx(rx))
}

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
async fn save_loop(store: Arc<Store>, mut snapshots: SnapshotRx, failures: FailureTx) {
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
                let _ = failures.send();
            }
        }
        // After the store has answered, and whichever way it answered: a write
        // that failed is not going to be retried from here, so nothing is
        // waiting on it any more. It is out of the tally for the same reason it
        // is out of the queue.
        snapshots.unwritten.cleared(rows);
    }
}

/// How often abandoned auth keys are swept out of memory.
///
/// A key that is never redeemed is dead weight and nothing else — `AuthKeys::redeem`
/// checks expiry itself, so a stale key is unusable long before it is collected.
/// Sweeping on the key's own lifetime means one lives at most twice that, which is
/// a bound worth having and a cadence not worth tuning.
const KEY_SWEEP: Duration = openshard_login::auth::DEFAULT_TTL;

/// What every player is told when the shard stops.
///
/// A constant and not a setting, deliberately: a message nobody can vary is a
/// string, not a configuration. It becomes config on the day there is an operator
/// command to schedule a stop and therefore something to vary it *with* — S7 of
/// `docs/shutdown.md`.
///
/// It says the world is being saved because that is the part a player cares
/// about: the difference between this and a crash is whether the last half hour
/// still exists.
///
/// `pub` for one reason: the end-to-end test that a stopping shard says this
/// before it hangs up asserts against the constant rather than a copy of the
/// string, so changing the wording cannot quietly leave the test passing on text
/// nobody sends.
pub const SHUTDOWN_NOTICE: &str = "The shard is shutting down. Your character is being saved.";

/// Everything the shard loop owns between ticks.
///
/// One value rather than eight locals threaded through every helper. Each step of
/// `docs/connection_state.md` added one — the tick was up to seven parameters and
/// took another per step — and a signature stops being readable long before it
/// stops compiling.
///
/// It is the loop's *state*, not a place for the loop's rules: the packet handlers
/// below still take the pieces they need one at a time. That is not an oversight.
/// A handler holding `&mut Session` while it queues a command borrows two fields at
/// once, which the compiler allows across fields and not across a `&mut self`.
struct Shard {
    world: World,
    sessions: Sessions,
    /// Keeps the sessions' phases in step with what the world did — the world is
    /// the authority on every transition past `Entering`. See D4 in
    /// `docs/connection_state.md`.
    phases: PhaseSync,
    /// Credentials, keys and the relay. Everything after the `0x91` is the
    /// world's.
    login: LoginServer,
    /// Where the shard's own content reads the staff menu's buttons from — the
    /// tree's half of what a pack answers in `onEvent`. See [`content::verb`].
    verbs: Cursor<AdminMenuAction>,
    /// Where a login's argon2 goes, so that it is not here. See `verify`.
    verifier: Verifier,
    saves: SnapshotTx,
    /// Where the relay tells a client to dial. Read on every connect, to say so
    /// when it is an address that client cannot reach.
    advertised: SocketAddrV4,
}

impl Shard {
    /// Run one tick: advance the world, flush its outbound packets, hand off its
    /// snapshots, and pump the gameplay script.
    fn tick(&mut self) {
        self.world.tick(Instant::now());
        // Before anything is sent: what the world did this tick decides which
        // connections are in it, and a refusal means there is nobody left to send
        // to.
        for connection in self.phases.apply(&self.world, &mut self.sessions) {
            self.sessions.close(connection);
        }
        self.flush_outbound();
        // Handed off, not awaited. The tick's job here is to stop
        // holding the only copy.
        for snapshot in self.world.drain_saves() {
            let _ = self.saves.send(snapshot);
        }
        self.answer_verbs();
    }

    /// Lay down whatever the tree has for the admin verbs pressed this tick.
    ///
    /// The counterpart of a pack's `onEvent` switch on the verb string, and the
    /// reason `world::admin` can stop saying the engine holds no content of its
    /// own. Collected before queueing because reading the bus borrows the world.
    fn answer_verbs(&mut self) {
        let actions: Vec<String> = self
            .world
            .bus()
            .read(&mut self.verbs)
            .map(|action| action.action.clone())
            .collect();
        for action in actions {
            let commands = content::verb(&action);
            if commands.is_empty() {
                continue;
            }
            debug!(
                action,
                commands = commands.len(),
                "laying the shard's own content"
            );
            for command in commands {
                self.world.queue(command);
            }
        }
    }

    /// Tell every player the shard is going, and get the line onto the wire.
    ///
    /// Two statements that must not be separated, so they are one call. See D6 in
    /// `docs/shutdown.md`: `announce` queues and `flush_outbound` sends, and a
    /// stop that does the first without the second is a stop that says nothing —
    /// silently, and in the one situation nobody re-runs by hand.
    ///
    /// Not a tick. The world is not advanced here: it has stopped, and what is
    /// wanted is one packet per player, not another 50 ms of simulation on the
    /// way out.
    fn announce_shutdown(&mut self) {
        self.world.announce(SHUTDOWN_NOTICE);
        self.flush_outbound();
    }

    /// Hand everything the world has queued to the sessions it is addressed to.
    ///
    /// Its own method because the shutdown path needs it too, and needs it to be
    /// the *same* one: the goodbye of `docs/shutdown.md` D6 is queued by the
    /// world like any other packet, and a second copy of this loop written beside
    /// the teardown would be a second thing to keep correct.
    ///
    /// A packet for a connection with no session is dropped, not an error: the
    /// world may have addressed a client that was closed between the tick and
    /// here.
    fn flush_outbound(&mut self) {
        for out in self.world.drain_outbound() {
            if let Some(session) = self.sessions.get(out.connection) {
                // A connection reaches the world only after its game
                // login, so this is always a game connection and every
                // packet leaves compressed. `send_packet` gates on the
                // flag anyway, so it stays correct if that ever changes.
                let _ = session.send_packet(out.packet);
            }
        }
    }

    /// Drop the keys of clients that selected a shard and never came back.
    ///
    /// On its own timer rather than riding along with the tick, which is where it
    /// used to be. Nothing about it belongs to the simulation: it is memory upkeep
    /// on a table the world has never heard of, it has no effect any client can
    /// observe, and at the tick's rate it ran twenty times a second to find nothing
    /// 599 times out of 600.
    fn expire_keys(&mut self) {
        self.login.keys.expire(Instant::now());
    }
}

/// Say what a closed window found, when it found something new.
///
/// # Why the line reads the way it does
///
/// The number an operator needs is not "the shard is busy" — it is *how far the
/// wire's arithmetic is now from the truth*, because every duration the shard
/// announces is a tick count converted at the declared rate. A shard at a
/// quarter of its rate tells a client a bow lands in 1600ms and lands it in
/// 6500ms, and no packet in the protocol carries anything that would let the
/// client notice. So the rate is named against the rate that was published, and
/// `busy` is beside it because that is what separates a tick this shard cannot
/// finish from a tick this shard was never given the chance to run.
///
/// Announcement lives here rather than in [`crate::pace`] so that the
/// measurement stays a measurement: `Pace` can be driven by a test with no
/// subscriber attached, and nothing it reports depends on having one. The stop
/// is here for the same reason — a type that measures a clock should not be able
/// to end a shard.
fn report_pace(pace: &mut crate::pace::Pace, window: crate::pace::Window, shutdown: &Shutdown) {
    match pace.verdict(window) {
        Some(crate::pace::Verdict::GivingUp { window, windows }) => {
            error!(
                behind_windows = windows,
                observed_ticks_per_second = window.observed_rate(),
                declared_ticks_per_second = openshard_state::TICKS_PER_SECOND,
                busy_share = window.busy_share(),
                worst_tick_ms = window.worst.as_secs_f32() * 1000.0,
                "the tick has been slower than its declared rate for {windows} seconds; every \
                 interval announced in that time was wrong. Stopping — the world is saved on the \
                 way out. Raise or clear `[watchdog] tick_behind_windows` to let it run on."
            );
            // The same stop Ctrl-C asks for, and deliberately not a panic: a
            // panic drops the task that writes the world, so it would answer a
            // silent lie with silent data loss. `run_shard` leaves its loop on
            // this and saves below it.
            shutdown.stop();
        }
        Some(crate::pace::Verdict::FellBehind(window)) => warn!(
            observed_ticks_per_second = window.observed_rate(),
            declared_ticks_per_second = openshard_state::TICKS_PER_SECOND,
            behind_ticks_per_second = window.behind_ticks(),
            busy_share = window.busy_share(),
            worst_tick_ms = window.worst.as_secs_f32() * 1000.0,
            "the tick is slower than the rate every announced duration is denominated in; \
             clients are being told intervals this shard will not keep"
        ),
        Some(crate::pace::Verdict::CaughtUp(window)) => info!(
            observed_ticks_per_second = window.observed_rate(),
            declared_ticks_per_second = openshard_state::TICKS_PER_SECOND,
            busy_share = window.busy_share(),
            worst_tick_ms = window.worst.as_secs_f32() * 1000.0,
            "the tick is keeping its declared rate"
        ),
        None => {}
    }
}

/// Drive login and the world until the shard is stopped or the gateway goes.
///
/// One task owns both. That is not a limitation: the world is deliberately
/// single-threaded — a deterministic tick is the whole point — and login is a
/// state machine that does no work worth parallelising. Async lives in the
/// gateway's tasks, on the far side of the channel.
///
/// # Stopping
///
/// `reins` is what the caller keeps of this shard, and both halves of it are the
/// caller's deliberately — see [`Reins`]. The stop inside it is the same
/// [`Shutdown`] the gateway was built with, so that the door closes and the tick
/// ends on one word rather than two; what happens after that word is heard is
/// below the loop — the trades, the last snapshot, and the save task awaited to
/// the end. **This function returns only once the world is on disk**, which is
/// what makes it something a caller may wait for.
pub async fn run_shard(
    mut events: ServerEventRx,
    config: &Config,
    world: World,
    store: Arc<Store>,
    reins: Reins,
    seed: &[String],
) {
    let shutdown = reins.shutdown();
    // `Config::validate` (run by `Config::load`, which every `Config` reaching
    // here has been through) refuses an IPv6 `server.advertise`, so this is
    // always `Some` in practice.
    let advertised = config
        .advertise_v4()
        .expect("Config::validate rejects an IPv6 server.advertise");

    let (saves, snapshots) = snapshot_channel(reins.unwritten());
    let (failed, mut failures) = failure_channel();
    let (verifier, mut verdicts) = Verifier::new();

    // Everything that comes off a disk, in the one order that works — see
    // `boot::restore`. It borrows the store; the save task takes ownership
    // afterwards, so this has to come first.
    let boot::Restored { accounts, world } = boot::restore(&store, config, world).await;

    // Kept, not detached: shutdown hands it a final snapshot, closes the channel,
    // and awaits this task so every queued write lands before the process exits.
    let save_task = tokio::spawn(save_loop(store, snapshots, failed));

    let mut shard = Shard {
        // Taken here, beside the script's cursors and for the same reason: before
        // the first tick, so a `--seed` verb sent below is read exactly once.
        verbs: world.bus().cursor(),
        // Built after the world is restored for the same reason: the arrivals and
        // departures of the restore are not phase changes for connections that do
        // not exist yet.
        phases: PhaseSync::new(&world),
        // The login server is credentials, keys and the relay, and nothing else:
        // the starting cities and the two capability masks went to the world with
        // the character screen they configure.
        login: LoginServer::new(accounts, &config.server.name, advertised),
        sessions: Sessions::new(),
        world,
        verifier,
        saves,
        advertised,
    };

    // The shard's own content, queued between the restore above and the first
    // tick below — the world it lays down is furnished before anybody can walk
    // into it. Queued rather than applied: the tick is the only thing that writes
    // the world, and content is not an exception to that.
    let content = content::boot();
    debug!(commands = content.len(), "queueing the shard's own content");
    for command in content {
        shard.world.queue(command);
    }

    // The verbs this run was told to send itself, after the `verbs` cursor above
    // has been taken and before the first tick retires anything — the one window
    // where an event sent from outside a tick is read exactly once.
    for action in seed {
        info!(action, "seeding from the command line");
        shard.world.seed(action);
    }
    // A verb the tree has no content for lays nothing, and nothing else is
    // listening now, so it is worth saying out loud rather than leaving an
    // operator to wonder why the world is empty.
    let unanswered: Vec<&str> = seed
        .iter()
        .map(String::as_str)
        .filter(|action| content::verb(action).is_empty())
        .collect();
    if !unanswered.is_empty() {
        warn!(
            verbs = unanswered.join(", "),
            "--seed named verbs this shard has no content for; nothing will answer them"
        );
    }

    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    // A tick that ran late must not try to catch up by running several in a row:
    // that turns a hiccup into a stall, and a fixed timestep into a variable one.
    //
    // The cost of that choice is that an overrun becomes a **slower clock**
    // rather than a visible burst, and the shard goes on announcing durations
    // denominated in the rate it is no longer keeping. `pace` below is what makes
    // the slower clock sayable; see `crate::pace` for why the wall clock may be
    // read here and nowhere inside the tick.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pace = crate::pace::Pace::new(crate::pace::BehindWindows(config.watchdog.tick_behind_windows));
    let mut key_sweep = tokio::time::interval(KEY_SWEEP);
    key_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Biased so the tick cannot be starved by a busy network. Without
            // this, a flood of packets would keep `recv` ready forever and the
            // world would stop simulating under exactly the load that needs it.
            biased;

            _ = ticker.tick() => {
                let began = Instant::now();
                shard.tick();
                // Measured around the tick rather than inside it: the world is
                // never handed a wall clock, so replay is untouched and a run
                // with this measurement produces the same world as one without.
                if let Some(window) = pace.record(began, began.elapsed()) {
                    report_pace(&mut pace, window, &shutdown);
                }
            }

            // Before `events`: a store that is failing is worth hearing about
            // ahead of the next packet, and there is never a queue of these.
            Some(()) = failures.recv() => {
                warn!("a save failed; marking the world for a full sweep");
                shard.world.resweep();
            }

            // Before `events` as well: a client waiting on a password check is a
            // client that has been waiting for a hash, and answering it costs
            // nothing but the packet it was going to send anyway.
            Some(verdict) = verdicts.recv() => shard.resume_login(verdict),

            _ = key_sweep.tick() => shard.expire_keys(),

            // A stop was asked for — Ctrl-C in the binary, a handle in a test.
            // Leave the loop and save the world on the way out, rather than dying
            // with the last save cadence's worth of play unwritten.
            //
            // Nothing here has to be done first: the gateway heard the same word
            // and is hanging up on its own connections, and a packet that arrives
            // in this same moment is queued into a tick that will not run. What
            // matters is that the world below is written, and that is what
            // follows the loop.
            () = shutdown.requested() => {
                info!("shutdown requested; saving the world");
                break;
            }

            event = events.recv() => {
                let Some(event) = event else {
                    error!("the gateway stopped; saving the world");
                    break;
                };
                shard.handle_network(event);
            }
        }
    }

    // From here to the last line is everything a stop actually costs: the
    // goodbye, the sweep of a whole world, and however many writes were already
    // queued behind it. The log goes quiet across all of it, so it is timed — an
    // operator setting a stop timeout (systemd's `TimeoutStopSec`, and the
    // patience of whoever is holding Ctrl-C) has nothing else to set it from, and
    // the number they need is this one and not the wall clock of a whole run.
    let stopping = Instant::now();

    // The shard's last word, and then the hang-up — in that order, and the order
    // is the whole of it.
    //
    // A clean stop looks from the client exactly like a crash unless somebody
    // says otherwise, so the world tells every player what is happening while it
    // still has sessions to say it through. The flush is welded to the
    // announcement: `announce` only queues, and anything inserted between these
    // two lines swallows the notice without failing anything — which is why the
    // end-to-end test asserts the *order* and not merely the presence.
    shard.announce_shutdown();

    // The loop is over, so the state it owned goes back to being the two things
    // shutdown needs: the world to sweep, and the channel to send the sweep down.
    //
    // Below the announcement on purpose: this is what drops the sessions, and
    // dropping a session drops its outbox, which is what hangs the client up.
    // Move it back above and the goodbye is written to nothing.
    let Shard { mut world, saves, .. } = shard;

    // Shutdown: one last full snapshot, then flush every queued write before the
    // process exits. This is the one moment a lost write costs a player real value,
    // so unlike the per-tick handoff it is *awaited*. Dropping the sender ends the
    // save task's receive loop once it has drained what is left.
    //
    // End every trade first. A trade escrow is deliberately not saved — a
    // restored one would be a window nobody can close — so the goods inside one
    // have to be back in the two packs *before* the sweep reads them, or a clean
    // stop taken mid-trade costs both parties whatever they had offered.
    world.cancel_all_trades();
    world.take_snapshot();
    for snapshot in world.drain_saves() {
        let _ = saves.send(snapshot);
    }
    drop(saves);
    if let Err(error) = save_task.await {
        error!(%error, "the save task did not finish cleanly on shutdown");
    }
    info!(took = ?stopping.elapsed(), "world saved; shutting down");
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

/// Route a decoded login packet: the character screen's two, and everything the
/// login state machine still owns.
///
/// # The screen is not this crate's any more
///
/// Creating and deleting a character are world commands since S5 of
/// `docs/connection_state.md` — the world owns which characters exist, which of
/// them is being played, and where each one was — so both arms here are a
/// translation and nothing else, exactly like `dispatch_world_packet`. Neither
/// touches `login`, and neither answers the client: the reply comes out of the
/// tick that applies the command, which is what keeps the two ends in one order.
///
/// What is left of the login conversation is what is genuinely not simulation:
/// credentials, argon2, the auth key and the relay. It ends at
/// [`Command::Authenticated`], which is queued by [`Shard::resume_login`] — the
/// hand-off happens when the *password* checks out, and that answer arrives on a
/// channel rather than out of this call. See `verify`.
fn handle_login_packet(
    session: &mut Session,
    login: &mut LoginServer,
    world: &mut World,
    verifier: &Verifier,
    packet: LoginStagePacket,
    id: ConnectionId,
) -> bool {
    match packet {
        LoginStagePacket::CreateCharacter(create) => {
            world.queue(Command::CreateCharacter {
                connection: id,
                create,
            });
            // No `enter_world` here, deliberately. A `0x00` is not "let me in":
            // the world may refuse the name, and a refused creation keeps the
            // connection on the creation screen to try again. Moving the phase now
            // would strand it in `Entering` with no character behind it. The
            // `PlayerEntered` that follows a creation that worked is what moves it
            // — see `Session::entered_world`.
            true
        }
        LoginStagePacket::DeleteCharacter(delete) => {
            world.queue(Command::DeleteCharacter {
                connection: id,
                slot: delete.slot,
            });
            true
        }
        LoginStagePacket::PlayCharacter(play) => {
            // The screen's third packet, and the one that ends the screen.
            // Everything it needs beyond the name — whose account this is, what
            // authority it plays with, which client it is — is on the
            // connection's row in the world, put there at the hand-off.
            //
            // The phase moves here rather than on the `PlayerEntered` that
            // follows: this *is* a request to enter, and everything the client
            // sends next must be queued behind the entry rather than dropped by
            // the world gate. See `WorldPhase::Entering`.
            session.enter_world();
            // Tell the gateway framer this client's version now, before any
            // in-world packet whose length depends on it (the drop packet). The
            // game connection never stated its version; this is the
            // auth-key-linked one the login carried across. Character select is
            // the last quiet moment before world traffic starts.
            let _ = session.control.send(session.login.version());
            world.queue(Command::PlayCharacter {
                connection: id,
                name: play.name,
            });
            true
        }
        login_packet => {
            // The game login is the seam Sphere calls CONNECT_GAME: from here
            // on, this connection's every server->client packet is
            // Huffman-compressed — starting with the reply `handle` just built,
            // which is why `apply` follows the call rather than the state
            // machine sending anything itself. Nothing is copied out to say so:
            // `Session::send_packet` reads the state machine.
            match login.handle(&mut session.login, login_packet, Instant::now()) {
                Outcome::Reply(response) => session.apply(response, id),
                // A password to check. It goes to a blocking task and the
                // connection waits: the login session will accept no other packet
                // until the verdict lands in `resume_login`. Keeping the
                // connection is the whole point — dropping it here would be
                // refusing every login that has not been checked yet.
                Outcome::Verify(check) => {
                    verifier.spawn(id, check);
                    true
                }
            }
        }
    }
}

/// Gate a decoded world packet on the connection's phase and queue what the
/// dispatcher makes of it.
///
/// # The one gate
///
/// This `if` is the whole of what thirty arms of `dispatch_world_packet` used to
/// repeat — see that function's doc, and `docs/connection_state.md` S3. It is
/// here rather than there because this is the last place that still holds the
/// session: past it, a packet is only a packet.
///
/// Every packet reaching this function is a world packet and nothing else. The
/// character screen's three, `0x5D` included, are [`LoginStagePacket`]s and go to
/// [`handle_login_packet`]; that used to be a fourth arm here, matched out before
/// the gate, with an `unreachable!` in the dispatcher standing in for the
/// invariant. The split at the decode seam is the same statement, made where it
/// cannot be forgotten.
///
/// The dropped packet is not named in the log. `ClientPacket`'s `Debug` carries
/// bodies — a `0x03` would put whatever the player typed in the log — and a
/// per-variant name table would be a second list to keep in step with the enum.
/// The connection is what a reader needs; which packet it was, the client knows.
fn handle_world_packet(
    session: &mut Session,
    world: &mut World,
    packet: ClientPacket,
    id: ConnectionId,
) -> bool {
    if !session.in_world() {
        debug!(%id, "a world packet from a connection that is not in the world");
        return true;
    }
    if let Some(command) = dispatch_world_packet(packet, id) {
        world.queue(command);
    }
    true
}

impl Shard {
    /// Act on one thing the gateway said: a connection opened, a packet arrived,
    /// a connection went away.
    ///
    /// The fields are reached one at a time rather than through a `&mut self`
    /// helper: a handler holds a `&mut Session` while it queues into the world,
    /// and the compiler splits that borrow across fields where it would refuse it
    /// across a method call.
    /// A password check came back: tell the login session, answer the client,
    /// and — if this was the login that authenticates the connection — hand it to
    /// the world.
    ///
    /// # This is the only place a connection becomes somebody
    ///
    /// A `LoginSession` reaches `CharacterListSent` on one transition and one
    /// only: a matching verdict on a game login. So `account()` turning `Some`
    /// *is* the hand-off, with no flag here to keep in step with the state machine
    /// that owns the fact, and no "was it already authenticated?" to compare —
    /// `resume` refuses a second verdict on a session that is no longer waiting
    /// for one.
    fn resume_login(&mut self, verdict: Verdict) {
        let id = verdict.connection;
        let Some(session) = self.sessions.get_mut(id) else {
            // The socket closed while its password was being hashed. Nothing to
            // answer and nothing to clean up: the session went with the
            // `Disconnected` that removed it.
            debug!(%id, "a password verdict for a connection that has gone");
            return;
        };
        let response = self.login.resume(&mut session.login, verdict.verdict);
        if let Some(account) = session.login.account() {
            // The account's authority is re-derived at every login and never saved
            // with a character, so this is where it is looked up: the last moment
            // the accounts and the connection are both in reach.
            let access = self.login.accounts.access_level(account);
            self.world.queue(Command::Authenticated {
                connection: id,
                version: session.login.version(),
                account: account.clone(),
                access,
            });
        }
        if !session.apply(response, id) {
            self.sessions.close(id);
        }
    }

    fn handle_network(&mut self, event: ServerEvent) {
        let (sessions, login, world, verifier, advertised) = (
            &mut self.sessions,
            &mut self.login,
            &mut self.world,
            &self.verifier,
            self.advertised,
        );
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
                sessions.open(id, Session::new(outbox, control));
            }

            ServerEvent::Received { id, event } => {
                // Read what decoding needs and let the borrow go, rather than
                // holding the session across the routing below. `0x83` is routed
                // with the whole table in hand — see `delete_character` — and a
                // `&mut Session` taken here would still be alive at that point.
                let Some(version) = sessions.get(id).map(|session| session.login.version()) else {
                    // Disconnected arrived first. Possible: the gateway's tasks and
                    // this loop are not synchronised.
                    return;
                };
                // Every arm below looked the session up a line ago and nothing
                // between here and there can remove it: this loop is the only thing
                // that touches the table, and it is not concurrent with itself.
                const PRESENT: &str = "the session was looked up at the top of this arm";
                match event {
                    Event::Seeded(seed) => sessions.get_mut(id).expect(PRESENT).login.on_seed(seed),
                    Event::Packet(packet) => match packet.parse_packet(version) {
                        // Ok: hand the decoded packet to whichever side it belongs
                        // to. Every handler returns the same "keep the connection?"
                        // bool, so there is one place that acts on it.
                        Ok(packet) => {
                            let session = sessions.get_mut(id).expect(PRESENT);
                            let keep = match packet {
                                Packet::Login(login_packet) => {
                                    handle_login_packet(session, login, world, verifier, login_packet, id)
                                }
                                Packet::World(world_packet) => {
                                    handle_world_packet(session, world, world_packet, id)
                                }
                            };
                            if !keep {
                                sessions.close(id);
                            }
                        }
                        // Err: every case here only logs and drops. Nothing decoded,
                        // so there is nothing to route.
                        Err(error) => {
                            match error {
                                PacketError::Login(ClientLoginDecodeError::CreateCharacter(error)) => {
                                    warn!(%id, %error, "malformed create-character");
                                }
                                PacketError::Login(ClientLoginDecodeError::DeleteCharacter(error)) => {
                                    warn!(%id, %error, "malformed delete-character");
                                }
                                PacketError::Login(ClientLoginDecodeError::PlayCharacter(error)) => {
                                    warn!(%id, %error, "malformed character-select");
                                }
                                PacketError::Login(other) => {
                                    warn!(%id, ?other, "malformed login packet");
                                }
                                PacketError::World(error) => {
                                    warn!(%id, ?error, "malformed packet");
                                }
                            }
                            sessions.close(id);
                        }
                    },
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
                sessions.close(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::direction::{Direction, Facing};
    use openshard_protocol::identity::RawCharacterName;
    use openshard_protocol::wire::{RawCharacterSlot, RawClientIp};
    use openshard_protocol::world::{RawFastwalkKey, RawStepSequence, WalkRequest};

    use openshard_protocol::world::CharacterPlay;

    use openshard_persistence::SCHEMA_VERSION;

    use super::*;
    use crate::testing::{at_character_screen, login_server, lord_british};

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

    /// One step north — any in-world packet would do; this is the smallest.
    fn a_step() -> ClientPacket {
        ClientPacket::Walk(WalkRequest {
            facing: Facing::walking(Direction::North),
            sequence: RawStepSequence(0),
            fastwalk_key: RawFastwalkKey(0),
        })
    }

    /// The `0x5D` a client sends when it picks [`lord_british`] off the list.
    fn picking_lord_british() -> LoginStagePacket {
        LoginStagePacket::PlayCharacter(CharacterPlay {
            name: RawCharacterName(lord_british().0),
            slot: RawCharacterSlot(0),
            client_ip: RawClientIp(0),
        })
    }

    #[test]
    fn a_world_packet_from_outside_the_world_becomes_no_command() {
        // The gate, from the side it refuses. A connection sitting on the
        // character screen has no entity, so a `0x02` from it would be queued
        // into a tick that drops it on a `players` miss — work created out of a
        // packet nobody can act on. The queue length is the assertion because
        // "nothing happened" has nothing else to look at; see `World::queued`.
        let mut login = login_server();
        let mut world = World::new(openshard_map::grid::Tile::new(1363, 1600));
        let id = ConnectionId::from_raw(1);
        let (mut session, _wire) = at_character_screen(&mut login, Instant::now());

        assert!(
            handle_world_packet(&mut session, &mut world, a_step(), id),
            "a packet that is merely early is not a reason to close"
        );
        assert_eq!(world.queued(), 0, "and it did not become work");
    }

    #[test]
    fn the_same_packet_becomes_a_command_once_the_connection_is_in() {
        // The other direction, so the test above cannot pass by a gate that
        // refuses everything. Nothing changes but the phase — the same session,
        // the same packet.
        let mut login = login_server();
        let mut world = World::new(openshard_map::grid::Tile::new(1363, 1600));
        let id = ConnectionId::from_raw(1);
        let (mut session, _wire) = at_character_screen(&mut login, Instant::now());
        session.enter_world();

        assert!(handle_world_packet(&mut session, &mut world, a_step(), id));
        assert_eq!(world.queued(), 1, "the step is queued for the next tick");
    }

    #[test]
    fn the_character_screen_packet_never_meets_the_gate() {
        // `0x5D` arrives from a connection that is by definition outside the
        // world — it is what puts it in — so it is not a world packet at all: the
        // decode seam hands it to `handle_login_packet` beside the screen's other
        // two. Route it past the gate instead and a shard accepts no logins at
        // all, silently: the client waits on "logging into shard" and this end
        // says nothing.
        let mut login = login_server();
        let mut world = World::new(openshard_map::grid::Tile::new(1363, 1600));
        let (verifier, _verdicts) = Verifier::new();
        let id = ConnectionId::from_raw(1);
        let (mut session, _wire) = at_character_screen(&mut login, Instant::now());
        assert!(!session.in_world(), "the fixture is on the character screen");

        assert!(handle_login_packet(
            &mut session,
            &mut login,
            &mut world,
            &verifier,
            picking_lord_british(),
            id
        ));
        assert_eq!(world.queued(), 1, "the entry is queued");
        assert!(session.in_world(), "and the gate is open for what follows");
    }

    /// A snapshot worth three rows, built out of removals alone.
    ///
    /// Deliberately not a character: what is being counted is `Snapshot::len`,
    /// and three serials say that in one line where three `CharacterRecord`
    /// fixtures would say it in thirty and invite a reader to look for meaning
    /// in them.
    fn three_rows(tick: u64) -> Snapshot {
        Snapshot {
            tick,
            schema: SCHEMA_VERSION,
            characters: Vec::new(),
            removed: vec![1, 2, 3],
            inventories: Vec::new(),
            ground: None,
            spawners: None,
            mobiles: None,
            decorations: None,
            regions: None,
            guilds: None,
            alliances: None,
            houses: None,
            designs: None,
            boats: None,
            world: None,
        }
    }

    #[tokio::test]
    async fn a_snapshot_is_unwritten_until_the_store_has_answered_for_it() {
        // The number D2's force-exit names. Without it the second signal can
        // only say that the save did not finish — which the operator knows,
        // being the one who did not wait — and the difference between a stop
        // that cost nothing and one that dropped a full sweep is invisible.
        let unwritten = Unwritten::new();
        let (saves, snapshots) = snapshot_channel(unwritten.clone());
        let (failed, _failures) = failure_channel();

        // Handed over with nothing draining the queue: this is exactly the
        // state a shard is in when its store is slow and the operator is
        // impatient.
        saves.send(three_rows(1)).expect("the receiver is alive");
        saves.send(three_rows(2)).expect("the receiver is alive");
        assert_eq!(unwritten.writes(), 2, "two transactions are outstanding");
        assert_eq!(unwritten.rows(), 6, "and six rows are inside them");

        // Dropping the sender is what ends `save_loop`, so awaiting it here
        // runs it to the end of the queue rather than forever.
        drop(saves);
        let store = Arc::new(Store::memory());
        save_loop(store, snapshots, failed).await;

        assert_eq!(unwritten.writes(), 0, "nothing is owed once the store answered");
        assert_eq!(unwritten.rows(), 0, "and no rows are left behind in the count");
    }

    #[test]
    fn a_snapshot_the_channel_refused_is_not_owed_by_anybody() {
        // A send that fails means the save task is gone — it panicked, or the
        // shutdown tail already dropped the receiver — so nothing will ever
        // clear that snapshot from the tally. Counting it would make the
        // force-exit line grow a permanent phantom backlog, and an operator
        // reading "3 writes abandoned" every time would learn to ignore it.
        let unwritten = Unwritten::new();
        let (saves, snapshots) = snapshot_channel(unwritten.clone());
        drop(snapshots);

        assert!(saves.send(three_rows(1)).is_err(), "the receiver is gone");
        assert_eq!(
            unwritten.writes(),
            0,
            "a snapshot nobody took is not outstanding work"
        );
        assert_eq!(unwritten.rows(), 0);
    }

    #[test]
    fn reins_over_a_stop_hold_that_stop_and_not_a_new_one() {
        // The one thing `Reins::over` exists for: the caller already made the
        // `Shutdown` — it went to the gateway, and a test's `Running` keeps a
        // clone to stop the shard with — so the reins must be another hold on
        // *that* stop. A `Shutdown::new()` inside here would compile, run, and
        // leave every caller with a stop word the shard cannot hear.
        let shutdown = Shutdown::new();
        let reins = Reins::over(shutdown.clone());

        shutdown.stop();
        assert!(
            reins.shutdown().is_stopping(),
            "the reins hold the caller's stop, not one of their own"
        );

        // And a clone is a hold on the same tally, which is what makes it safe
        // for the signal watcher and the shard to be handed one each.
        let elsewhere = reins.clone();
        reins.unwritten().queued(3);
        assert_eq!(elsewhere.unwritten().writes(), 1, "one write is outstanding");
        assert_eq!(elsewhere.unwritten().rows(), 3, "and it carries three rows");
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
