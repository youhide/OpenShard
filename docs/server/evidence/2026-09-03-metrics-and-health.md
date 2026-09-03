# A shard that publishes itself

> **This is a record**, written on 2026-09-03. What a shard does today is
> [`../README.md`](../README.md); what is still not built about operating one is
> [`plans/server/operations/PLAN.md`](../../../plans/server/operations/PLAN.md),
> which lost its first entry to this work.

The shard measured two things it had nowhere to put. `pace.rs` closes a window
every second and knows the observed rate, the busy share, the worst tick in it
and the commands that tick applied; `Unwritten` knows how many writes and how
many rows the save task has been promised and has not delivered. Both were spent
on a log line and thrown away.

A log line is an **event**: it says that something changed at the moment it
changed. Both questions an operator actually asks are **samples** — what is it
doing right now, and what has it been doing for the last hour — and neither of
them can be answered by an edge. So `crates/common/metrics` stopped being a stub:
`ShardMetrics` holds the live values, `Reading` is one consistent instant of
them, `exposition` and `health` are two renderings of that instant, and
`endpoint` is the socket that serves both. `[metrics] listen` turns it on and
nothing turns it on by default.

## What is published

Thirteen families, and every one of them is a number the shard already had.

| | |
|---|---|
| `openshard_uptime_seconds` · `openshard_ticks_total` | the shard is up, and the world is running |
| `openshard_tick_age_seconds` | how long ago the last tick finished |
| `openshard_tick_rate_declared_per_second` · `..._observed_per_second` | what the shard promises, beside what it delivered |
| `openshard_tick_busy_ratio` · `openshard_tick_worst_seconds` · `openshard_tick_behind_ticks` | whose fault a slow window is, what an average hides, and how much time it lost |
| `openshard_connections` | clients the shard is holding |
| `openshard_saves_completed_total` · `openshard_saves_failed_total` | what the store answered |
| `openshard_save_backlog_writes` · `openshard_save_backlog_rows` | what a force-exit would cost right now |
| `openshard_stopping` | a stop has been asked for |

`/health` is the same instant as JSON, plus the one field a scrape may not carry:
the worst tick's command mix. `/` names the other two for whoever arrived without
knowing them.

## The four decisions

**D1 — No thresholds anywhere in this crate.** `serving` is the only verdict, and
it is a fact rather than a comparison: the shard has been asked to stop, or it has
not. *How slow is too slow* is the fudge constant `docs/style.md` forbids — it
could only be right on the machine it was tuned on — and the place it genuinely
belongs is the operator's alerting rules, where it differs per shard and changes
without a rebuild. The consequence is deliberate and worth stating: a shard whose
tick has wedged still answers `/health` with a 200. What it also answers is
`tick_age_seconds`, growing without bound, which is a thing no log line has ever
said.

**D2 — The status code answers one question.** 200 while the shard is taking
play, 503 once it is not. A shard that is merely slow is still serving, so it
gets a 200 and says how slow in the body; overloading the code with two questions
would make both unanswerable.

**D3 — The HTTP is written by hand.** Three routes, no request body, no
keep-alive, no compression, no TLS, no routing table. A framework would bring a
router, an extractor layer and several dozen crates — all of which have to pass a
licence gate this tree still owes itself — to render text shorter than the code
configuring it. The rules that buys are small and stated beside the code that
keeps them, and one test holds all three at once: a real client reading a real
response to the end.

The trade is named rather than hidden. It is right for a diagnostic port and it
is **not** right for the administration API of the plan's entry 3: a dozen routes,
bodies and tokens is where a framework starts paying for itself.

**D4 — One request at a time.** `docs/server/README.md` § what is open ranks two
unbounded queues as this domain's worst standing defects, and a diagnostic port
that spawned a task per connection — reachable by anything that can open a
socket — would be a third. A scrape arrives every few seconds from one collector,
so serialising costs nothing real. What it does cost is a client that connects and
says nothing, holding the port against the next scrape, and the request deadline
is the bound on that. It is the one thing the deadline exists for and the one
thing it is tested for.

## Two things found on the way

**The endpoint must not stop when the shard does.** The first wiring gave it the
shard's own `Shutdown`, which is exactly backwards: the span an operator most
wants to ask about is the one where the log goes quiet — after the loop, through
the trade cancellation, the full sweep and however many writes were queued behind
it — and an endpoint on the same word answers "connection refused" for precisely
that. It now outlives `run_shard` and says `serving: false` across it, which is
also what `run_shard`'s `metrics.stopping()` is for.

**A connection count kept on the edges is a count with five places to forget.**
`Sessions::close` is called by a disconnect, by a handler that refused a packet,
by a decode that failed, by a login verdict for a socket that has gone, and by the
phase sync. The table is the count, so it is read once a tick — a `HashMap::len`
beside three relaxed atomic writes, forty times a second — rather than
incremented and decremented in five places that must all agree.

## What it is proved by

- `openshard-metrics`, 21 unit tests: the registry keeps what is published and
  distinguishes "no window yet" from a window of zeroes; the exposition gives
  every sample its `HELP` and `TYPE`, omits what has not been measured, spells
  `+Inf` the way the parser reads it, and never lets the free-form work summary
  become a label; the health document keeps its shape on a shard that has not
  ticked and survives a quote in a command name; the endpoint routes, refuses a
  write, refuses a head it cannot read, and hangs up on a client that says
  nothing.
- `crates/e2e/shard/tests/metrics_endpoint.rs`: a real shard, started, waited on
  until it closes a real pace window, and scraped over a socket. Every unit test
  above feeds the registry by hand and would go on passing if `run_shard` stopped
  publishing; this one would not.
- `openshard-config`, 4 tests: the section is absent by default and absent from
  the shipped file, a named socket is read as one, the game port is refused, and
  the same port on another interface is not.
- Run by hand: `openshard` on a config naming `127.0.0.1:19598`, scraped with
  `curl`. 109 ticks in 2.7 seconds, an observed 40.005 against a declared 40, a
  busy ratio of 0.0046, a worst tick of 238µs.

## What it left open

Ranked as row 14 of [`../README.md`](../README.md) § what is open, and one of the
three is deliberate:

- **The port has no authentication**, by design. It publishes numbers, the answer
  today is a loopback bind, and the shard warns at boot when it is not one.
  Authority belongs to the admin API that does not exist.
- **Nothing decides that a tick age is too long** — see D1. It is visible for the
  first time and it is the operator's rule to write.
- **The client's two binaries still build their own subscriber.** The shard's
  binaries went through one call to `openshard_metrics::logging::install`; the
  playground takes its filter from `--log` and adds a jank directive, and
  `openshard-client-app` defaults to `warn`. So `RUST_LOG` still means two things
  in this workspace.
