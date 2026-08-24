# 4. Persistence

- [x] Persistence queue, drained outside the tick
- [x] SQLite backend — `SqliteStore`, tested
- [x] Save and load accounts and characters
- [x] Serial reservation on load — `Registry::reserve_serial`, for load-on-play
- [x] Crash recovery — the boot load restores the world; a played character
  returns on its saved serial and spot
- [x] PostgreSQL backend — `PgStore`, the same `Store` trait, tested against a
  live server
- [x] **Item persistence** — a character's carried inventory (worn gear, the
  backpack and everything nested inside it) and loose ground clutter survive a
  restart. `ItemRecord` is the saved shape; `SCHEMA_VERSION` moved to 2. An
  inventory is saved as a unit — the store replaces everything under an owner
  rather than diffing item by item, walked live for an online character and kept
  at logout like the character record; the ground is a full sweep, decoration
  excluded (a pack re-lays that). On boot the item serials are reserved and ground
  items placed; a returning character re-equips its saved inventory instead of a
  starter backpack. Items keep their serials across a restart so a container's
  contents still point at it.
- [x] **A save is complete, and shutdown flushes it.** Consistency, because it is
  gold and gear: every save writes *every online character* in full — record and
  whole inventory — not only the ones that moved, because picking an item up takes
  no step and so never marks a character dirty; the ground is swept every save, not
  only when someone was active; and a logout re-fills the in-memory
  pending-inventory cache so a **re-login in the same run** re-equips what it
  carried (before the fix it lost the backpack). And the shard **saves on the way
  out**: Ctrl-C, or the gateway stopping, takes one last full snapshot, closes the
  save channel and *awaits* the writer so every queued transaction lands before the
  process exits — unlike the per-tick handoff, because the one moment a lost write
  costs a player real value is the last one.
- [x] **`Stackable` persists, the save interval is a config line, and `.save`
  forces one.** An item's `Stackable` flag is saved (`ItemRecord`, schema v3), so a
  restored gold pile still merges with more rather than losing the flag until
  re-lifted. `persistence.save_seconds` sets the periodic cadence (0 = only shutdown
  and `.save`; a save never stops the world, so this is only how much a crash could
  cost). And a staff **`.save`** (GM+) takes an immediate snapshot and tells every
  player "the world is being saved" — the old shards' announce **without** their
  pause, because OpenShard's snapshot is an instant memcpy, not a synchronous walk
  of the world.
- [x] **Spawn regions persist, timers and all.** A populated area stays populated
  across a restart without re-running `.admin`, and — the point — a rare spawn keeps
  its remaining wait: killed with hours to go, it comes back with those hours ahead
  of it, not popping again the moment the shard is up. `SpawnerRecord` (schema v4)
  saves the region, its creatures and the timer as the **seconds still to wait**,
  not a tick count (which resets at boot) or a wall-clock time (the tick reads no
  clock) — so downtime pauses the timer rather than eating it, the semantics chosen
  for a rare spawn. Registering the *same* region twice lays one rather than
  stacking a second, and after a restart the regions come from the store rather
  than being re-laid, so a re-populate is not needed and the timers hold. "The same
  region" is the whole region — box, creatures, ceiling and pace
  (`Spawner::is_the_same_region`), not the box alone: Britannia's regions overlap,
  and matching on the box read 120 of the 1,430 shipped regions as re-registrations
  of the region already there and dropped them, which is how the forest north-east
  of Britain came to hold orcs and no skeletons.
## A spawn region's id is its slot — and now it is that by construction

`maintain_spawners` tags each creature `SpawnedBy(id)`; the tag is saved with the
creature and read back against a list a later boot rebuilt. So the only id that
survives that trip is one the *list* defines. It used to be a counter beside the
list (`World::next_spawner_id`, starting at one), and the tag was the creature's
region's **index** — two numberings that agreed only by luck, and by luck of a
particular kind: a world laid once from empty has `id == index + 1`, so the tag and
the ceiling it was counted against lined up all the way through a restart. Nothing
enforced either half. `clear_spawners` emptied the list without rewinding the
counter, and neither store's `spawners()` had an `ORDER BY id`. Either one drifting
re-points every live creature at a neighbouring region: one region permanently at
its ceiling and never spawning again, its neighbour over its ceiling, and no error
anywhere — the same silence the box-shaped de-duplication had.

The counter is gone. `register_spawner` gives a region the slot it is about to
take, `restore_spawners` gives it the slot it lands in rather than the number in
the row, and `spawner_records` writes that number out — `a_regions_id_is_its_slot_
however_the_list_was_built` walks all three paths plus a Clear. There is no
migration and none is needed: the tags on disk were always indices, which is what
the ids now are. Clear stays safe because it takes the creatures with the regions,
so no tag outlives the numbering it was written against.

What this rules out, and it is worth naming because nothing in the type system
does: **a region may not be removed on its own.** The list is laid whole or cleared
whole. A future "delete this one region" renumbers every region after it and
re-points their creatures — that feature needs a real id and the migration this
did not.
## The playground lays the shipped content when asked — `--seed`

`e2e/shard`'s `in_process::spawn` took no verbs and handed `run_shard` an empty
slice, so `openshard-playground` opened exactly what its database held and had no
way to lay anything else; the module doc pointed at a `--seed` that was the server
binary's. It takes one now (`OPENSHARD_SEED`, comma-separated, the same verbs), and
the in-process shard passes it through. This is the difference between a restart
and a re-populate: content that has grown since a world was laid — a fixed dataset,
a region the engine used to drop — arrives on a seed or on the staff menu's
Populate, never on a boot, because a boot restores and lays nothing new.

- [x] **The save is the whole world (schema v5), the Sphere/ServUO model.** Every
  live NPC mobile — townsfolk, vendors with their priced stock, spawner creatures
  with their current wounds and `SpawnedBy` link (`MobileRecord`) — and every
  placed decoration, door open/shut state included (`DecorationRecord`), is swept
  into each snapshot and restored at boot exactly as it stood. A killed creature is
  simply absent from the sweep and stays dead, its region's saved timer counting
  down; nothing re-populates at boot, so a staff `.admin` Populate/Decorate seeds a
  fresh world **once** and the save is the truth thereafter. Both references walk
  every mobile and item to save (ServUO's `World.Save`, Sphere's `CWorld::SaveStage`)
  and never regenerate the world — this reaches the same end without stopping the
  world to do it. A ridden mount in limbo is the one mobile not swept: its ride
  persists through the saddle item on the rider, and `dismount` reconstitutes the
  creature whole.
- [x] **Stats and trained skills persist (schema v6).** A `CharacterRecord` carries
  str/dex/int and every trained skill with its lock arrow; character creation finally
  *applies* the stats and skills the player picked, threaded through
  `Command::Enter` as a `CharacterSheet` — for a new character from the create packet
  and for a played one from the save. The `0x3A` skills window follows a live gain.
- [x] **Regions and the world clock persist (schema v12).** A facet's named areas
  (`RegionRecord`) and the hour of the day ride in the same snapshot sweep as
  decoration and spawners. Both are things a player never changes and a restart
  would silently lose: no guards, no town music, daylight in every dungeon, and
  every night starting over at boot. The clock cannot ride the tick counter,
  which resets to zero by design — every restored timer is an offset from it.
- [x] **Active effects persist (schema v7).** Poison and the timed stat buffs are
  saved with their mobile as an `EffectRecord` list on the character or mobile row,
  so a relog cannot wash a debuff off — see the `magic` effects work in §6 for the
  shape (`World::effects_of`/`apply_effects`, the ledger-only restore for buffs).
- [x] **A container's trap persists (schema v19).** A restart that quietly disarms
  every chest on the shard is the same class of silent loss as one that forgets a
  lock — and the disarm is a skill somebody spent points on.
- [x] **The poison on an item persists (schema v18).** A bottled dose or the
  coating the Poisoning skill put on a blade. The same lesson as the spellbook mask:
  all four poison potions are one graphic, so an unsaved bottle comes back empty and
  a blade somebody spent a potion on comes back clean.
- [x] **A corpse's story persists (schema v17).** Who it was, who killed it, who
  has read it with Forensic Evaluation and who has rifled it, as one nullable JSON
  column on the item row. A corpse lies for seven minutes and a shard restarts
  inside that window, so without it the body a player was investigating comes back
  anonymous, killed by nobody and disturbed by no one. See the Forensics entry in
  §6 `skills`.

Two backends, one choice. A shard runs on SQLite or on PostgreSQL, and which is
the operator's to make: neither is "the production one", and SQLite runs a real
shard perfectly well. Some will want a text file or a Postgres cluster; the
`Store` trait is the seam that lets any of them sit behind the same simulation.

`persistence.database` picks the backend by what it looks like: a `postgres://`
URL connects to PostgreSQL, anything else is a SQLite file path, and empty keeps
the world in memory — the same bargain as running with no map, and the shard says
so. A logged-out character lives as a row, not an entity: its serial is reserved
at boot so nothing new can take it, and playing it (`0x5D`) spawns it back on that
serial, at its saved position, looking as it did. Characters save as they change
and on logout, through the same journal the tick already feeds.

**Three things it is worth knowing before touching this:**

- **The dirty marks come from the event bus.** Nothing calls `journal.touch()`
  by hand. A system that moves a mobile already emits `MobileMoved`, because
  that is how the client hears about it; persistence reads the same event. There
  is no line to forget.
- **Logout uses `Journal::keep`, not `touch`.** A touch is a promise to read the
  entity at the next save, and the entity is about to be despawned. Logout is
  when a save matters most, so the record is taken before the despawn. There is
  a test with that name.
- **A failed write costs a full sweep, not a rollback.** Re-writing the failed
  snapshot would put everyone back where they were when the write started. The
  world is marked dirty instead and the next save reads it fresh.

**Two things specific to the PostgreSQL backend:**

- **It connects with `NoTls`.** Enough for a database on the same host or a
  trusted network, which is where a first backend earns its keep. An encryptor is
  a later, additive change and does not touch the shape — `PgStore` is one
  connection behind an async mutex, the same shape as SQLite's, because a
  transaction borrows the client and saves are off the tick either way.
- **`tokio-postgres` used to be pinned, and no longer is.** From 0.7.13 it pulls
  a crypto stack (RustCrypto 0.11, `rand` 0.10) that wanted Rust 1.85 — above the
  1.82 MSRV of the time — so the lock held it at 0.7.12. The scripting spike (§5)
  raised the MSRV to 1.88, which cleared the constraint, and the pin was dropped;
  the crate floats on `"0.7"` again. See the `Cargo.lock` note in
  [`development.md`](../development.md).
