# The route journal, on by default and turned off in F1

The journal exists ([`docs/world/reference/path_journal.md`](../../../docs/world/reference/path_journal.md)):
a session writes one line per click and per replan, and `path_replay` re-asks
those questions over the real facet. What this plan changes is **who turns it
on**.

## Why it may not stay an environment variable

`OPENSHARD_PATH_JOURNAL` has to be remembered *before* the session that needs
it, and nobody knows which session that is: a route walks into a wall once, in
the middle of playing, and the evidence exists only if somebody had already
guessed. A diagnostic that must be predicted is a diagnostic that is not there
the one time it matters.

It is also the wrong kind of switch. The client already has a place where a
person turns diagnostics on and off and where that choice survives a restart —
the F1 window and `client_ui.ron` — and every other one of them lives there. A
second mechanism, invisible in the UI, readable only from a shell, is a second
policy for the same question.

So: **written unless somebody says not to, and the saying-so is a checkbox.**

## Boundary

- The file format does not change. `record.rs` is untouched, and journals
  already written stay readable.
- Still no slice of the world. Settled in the previous session and unchanged:
  the replay opens the same facet and the *test* builds the door.
- The environment variable goes away entirely. Not kept as a path override —
  one mechanism, and the one that is visible.
- The journal is a **value with an owner**, not a process-wide sink. It moves
  onto `Steering`, which is what runs a search and what already holds the plan
  cache and the refusal beside it.
- No new window, no new tab, one checkbox and one status line in a tab that
  already exists.

## Decisions this plan pins

1. **Where the file goes.** `path-journal.jsonl` in the working directory —
   beside `client_ui.ron`, which is where this client already keeps the things
   that are one person's own. Added to `.gitignore` with its companion below.

2. **One file, and the one before it.** Opening rotates: an existing
   `path-journal.jsonl` becomes `path-journal.prev.jsonl` and the new session
   starts empty. So "the session I just closed" survives exactly one restart,
   which is the ordinary shape of *play, quit, ask about it*.

3. **Opened lazily, at the first line worth writing.** A session that never
   plans a route creates no file and rotates nothing — a client started to look
   at a gump does not push somebody's evidence out of `.prev`.

4. **The switch is `F1Settings::path_journal`, default on.** It persists in
   `client_ui.ron` like every other F1 setting; a file written by an older
   client has no such field and gets the default, which is on.

5. **Where the checkbox lives: the Tile tab.** It sits under the terrain
   overlay, which is the other control about *where a click would walk*. Its
   label names the file, and a status line under it says what the journal has
   done this session — `12 orders, 47 plans` — or why it is not writing.

6. **Turning it off stops the writing and keeps the file.** The lines already
   on disk are the report; discarding them because somebody unticked a box
   would throw away the very thing they are about to attach. Turning it back on
   in the same session appends, with a fresh `session` line so a reader can see
   where the gap was.

7. **A size cap of 64 MiB.** A journal that is always on outlives the session
   it was interesting for, and an unbounded one on a long night is a full disk.
   At the cap the journal closes itself, says so once on stderr and in the F1
   status line, and nothing further is written. 64 MiB is roughly a hundred
   thousand plans — two orders of magnitude past any session anybody has
   debugged.

8. **`path_replay` defaults to that same path** and loses its `env` binding.
   `--journal` still takes one for a file somebody has kept.

## Slices

1. **Ownership.** `Journal` stops being a `OnceLock` sink: `pathlog::write`
   exposes the value and its rotation, and drops `journal()` and `JOURNAL_VAR`.
   `Steering` gains `Option<Journal>`; `steer::plan` takes `Option<&Journal>`;
   `go_to`, `take` and `record_plan` write through the one it is given. The
   client opens it where it opens everything else and hands it over.

2. **The default.** Rotation, lazy open, and the `session` line written from the
   facet the client actually loaded. `.gitignore` gets both names.

3. **The switch.** `F1Settings::path_journal`, `Request::path_journal`, the
   `Hud` counters, the checkbox and the status line in the Tile tab, and
   `App::apply` turning the journal on and off between frames.

4. **The cap.** Counted as bytes written, checked per line; the closing line is
   itself written to the file so a reader knows it is truncated by policy rather
   than by a crash.

5. **The tool and the document.** `path_replay`'s default path; the reference
   page rewritten from "set this variable" to "it is already on, and here is
   where the file is".

## What proves it

- `pathlog`: a journal that is never written to creates no file; a second open
  rotates the first; the cap stops the writing and says so in the file.
- `client/app`: the setting round-trips through `client_ui.ron` and defaults to
  on for a file that has never heard of it; `Steering` with no journal writes
  nothing (the existing `steer.rs` tests already run that path, and stay green
  without a journal being a global).
- By hand, and this one is the operator's: play, click somewhere silly, quit,
  `path_replay --list`.
