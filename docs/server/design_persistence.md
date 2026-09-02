# Persistence: a snapshot taken inside the tick, written outside it

How a shard remembers, as built. The subject is `crates/server/persistence` and
the two places that drive it: `world/src/tick/persist.rs`, which takes what is to
be written, and `server/src/boot.rs`, which reads it back.

This document was written for the migration rather than moved into it: the model
existed only inside a phase checklist and three stages of an invariants sweep,
which is to say nowhere a reader would look. The records those came from are
[`evidence/2026-08-24-the-persistence-phase.md`](evidence/2026-08-24-the-persistence-phase.md)
and
[`evidence/2026-07-31-invariants-nothing-enforces.md`](evidence/2026-07-31-invariants-nothing-enforces.md).

## The split, and why the type is what enforces it

```text
  inside the tick             outside the tick
  ─────────────────           ────────────────
  Journal::drain(..)   ───>   Store::save(&snapshot).await
  a memcpy of what                 the slow part
  changed, taken at
  one instant
```

The database is never touched inside a tick. Not rarely — never: a tick that
waits on a disk is a tick that took however long the disk took, out of a 25 ms
budget for the whole world.

But the *data* can only be read honestly from inside a tick, because that is the
only moment nothing is half-applied. So the two halves are split by a signature
rather than by a comment asking nicely. `Journal::drain` is synchronous and takes
a closure over the world's own data, so it can only be called from where that
data lives; `Snapshot` is plain owned values, so it can go anywhere.

**A snapshot is taken all at once**, and that is not an optimisation. A character
snapshotted at tick 100 and the item in its pack at tick 140 describe a world
that never existed — the pack could have been dropped in between — so what the
store writes is one instant or it is a fiction.

## What is saved

The save is the whole world, the Sphere/ServUO model: every online character with
its full inventory, every live NPC mobile with its wounds and its `SpawnedBy`
link, vendors with their priced stock, placed decoration down to whether a door
is open, spawn regions with the seconds still to wait, named regions, guilds and
alliances, houses and their designs, boats, loose ground clutter, and the hour of
the day.

Two things about that list are decisions rather than coverage:

- **Every save writes every online character in full**, not only the ones that
  moved. Picking an item up takes no step, so it never marks a character dirty;
  a save that trusted the dirty marks for this would lose gold and gear, which is
  the one thing a player will not forgive.
- **A killed creature is simply absent from the sweep**, and nothing
  re-populates at boot. A staff `.admin` Populate or Decorate seeds a fresh world
  *once*, and the save is the truth thereafter.

The one mobile deliberately not swept is a ridden mount, which lives in limbo:
its existence persists through the saddle item on the rider, and `dismount`
reconstitutes the creature whole.

Three rules govern *when*:

- **The dirty marks come from the event bus.** Nothing calls `Journal::touch` by
  hand. A system that moves a mobile already emits `MobileMoved`, because that is
  how the client hears about it; persistence reads the same event. There is no
  line to forget.
- **Logout uses `Journal::keep`, not `touch`.** A touch is a promise to read the
  entity at the next save, and the entity is about to be despawned. Logout is
  when a save matters most, so the record is taken before the despawn.
- **A failed write costs a full sweep, not a rollback.** Re-writing the failed
  snapshot would put everyone back where they were when the write started. The
  save task signals the loop instead, and the loop calls `World::resweep` so the
  next save reads the world fresh.

`persistence.save_seconds` is the periodic cadence — `0` means only shutdown and
a staff `.save`. A save never stops the world, so the setting is only how much a
crash may cost. The staff `.save` announces "the world is being saved" the way
the old shards did, **without** their pause, because a snapshot here is an
instant memcpy rather than a synchronous walk of the world.

## One `Store`, three backends, chosen by what the setting looks like

`Store` is an enum over `MemoryStore`, `SqliteStore` and `PgStore` — an enum and
not a trait object, so every call site names which shard it is talking to and
`unsafe`-free static dispatch stays possible. `persistence.database` picks by
shape: a `postgres://` or `postgresql://` URL is PostgreSQL, anything else is a
SQLite file path, and an empty string keeps the world in memory.

**Two backends, one choice.** Neither is "the production one" and SQLite runs a
real shard perfectly well; which one an operator wants is theirs to decide. The
in-memory mode is a real choice too — the same bargain as running with no map —
but a shard that stayed quiet about it would be one an operator assumes is
saving, so it warns. Opening a database that was asked for and cannot be opened
is fatal, for the same reason.

PostgreSQL connects with `NoTls`. That is enough for a database on the same host
or a trusted network, which is where a first backend earns its keep; an encryptor
is a later, additive change that does not touch the shape.

### The schema is one number

`SCHEMA_VERSION` (`record.rs`) is 37 today, and it covers every table. A database
written by a build with a different number is not read and silently trimmed: the
last four versions are migrated in place with `ALTER TABLE`, and anything else is
refused with the version it found and the version it understands.

The version numbers scattered through the phase record — "schema v2", "v19" — are
the day each field landed, not a claim about today.

### Only one accessor promises an order

`Store::characters` returns rows in ascending serial, which is creation order and
the only key all three backends share. Everything else — `items`, `mobiles`, the
rest — is deliberately unordered, because nothing downstream shows them in a list
a player indexes.

That promise is not tidiness. The roster enrols in the order the store hands rows
over, `0xA9` draws that list and `0x83` picks by position in it, so an unordered
`characters` is a character list that reshuffles. It did: PostgreSQL returned
heap order, where an `UPDATE` writes the tuple at the end, so one logout moved
that character to the bottom of its own list on the next boot; `MemoryStore`
returned `HashMap` order, a fresh shuffle every process, on the backend a shard
with no database actually runs. SQLite happened to be right because a `serial
INTEGER PRIMARY KEY` is the rowid alias — which is why every gate in the
repository was green over a rule two of three implementations broke.

## Boot: the restore order is a signature, not a comment

`boot::restore` fills a freshly built world from the store in the one order that
works, and two of the three links in that order are types rather than prose:

```text
  restore_characters ──RestoredCharacters──> restore_items ──RestoredItems──> restore_mobiles
```

- **Characters before items**, because the serials `restore_characters` reserves
  are the owners the item records point at. Run them the other way round and a
  character's pack is filed under a serial the allocator is free to hand to
  something else — and nothing fails, then or later. The pack is simply somewhere
  else, and the first person to notice is a player.
- **Items before mobiles**, because a mobile is equipped out of the inventories
  the items filed under its serial. Run them the other way round and every NPC
  and vendor comes back naked, its gear bound and reachable by nobody.

Neither token is a marker. `RestoredCharacters` carries the reserved-serial set,
which is what lets the items' restore tell a player's pack from an NPC's gear;
`RestoredItems` carries the filed owners that are *not* among the restored
characters, so the boot log can say how many mobiles found gear and how many
inventories were filed for one — and their difference is gear nobody came for,
which is the failure the order exists to prevent.

There is no test-only constructor for either. An escape hatch inside the crate
that defines the order is the order back as a convention; a helper that *runs*
the order (`tests::nothing_restored_first`) is not.

**The config's characters are not a third link.** `boot::restore` used to say
they had to be seeded after the store's rows so that a name in both kept the row
describing it, and the roster had never behaved otherwise — `Roster::enrol` does
not touch an entry that is there, and `Roster::remember` describes an entry
however late it was enrolled. What the order really decided was the *spelling*
`0xA9` shows, and the roster now takes that off the record rather than off
whichever call ran first.

## Where the rest of it is

- The phase record, with the schema history and what each version added:
  [`evidence/2026-08-24-the-persistence-phase.md`](evidence/2026-08-24-the-persistence-phase.md).
- The invariants sweep that turned two of the boot links into signatures and
  found the character-order bug:
  [`evidence/2026-07-31-invariants-nothing-enforces.md`](evidence/2026-07-31-invariants-nothing-enforces.md).
- That a stop saves, and what it costs when it cannot:
  [`design_shutdown.md`](design_shutdown.md).
- What is still open is ranked in [`README.md`](README.md).
