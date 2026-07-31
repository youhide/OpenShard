# Contributing to OpenShard

Thanks for looking. This is short on purpose; the reasoning behind the engine
lives in [`CLAUDE.md`](CLAUDE.md) — an index — and the docs it points at:
[`docs/style.md`](docs/style.md) for how the code reads,
[`docs/architecture.md`](docs/architecture.md) for its shape,
[`docs/findings.md`](docs/findings.md) for what the client actually does. Reading
the first two before a non-trivial change will save you an argument in review.

Two things sit beside this file: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
covers how we talk to each other, and [`SECURITY.md`](SECURITY.md) covers what
to do with something exploitable — report it privately rather than opening an
issue, because a shard is a server with a port open.

## The flow

`main` is protected — no direct pushes, no force-push. Everything lands through
a pull request:

1. Branch from `main`.
2. Make the change. Keep a PR to one subject; two subjects are two PRs.
3. Open the PR. CI runs `fmt`, `clippy` and the test suite.
4. A review, then merge. **Merge commits only** — squash and rebase are
   disabled, so the commits you push are the commits that land. Tidy them
   before asking for review.

## Before you push

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace          # includes doctests
```

All three are expected to be silent, and CI runs exactly these. They pass on
`main` today; a change that makes one of them noisy is not finished.

Where you need randomness inside a tick, spend the world's seeded `Rng` — the
tick is deterministic and replayable, and an OS or thread-local source quietly
breaks that. The same file explains the other rules that are easy to trip over
(never branch on `Era` for a protocol decision, no global mutable state, systems
emit events rather than calling each other).

## Commit messages

The message text only — no `Co-Authored-By:`, no `Claude-Session:`, no line
naming any model or tool. This applies to PR bodies too. Say what changed and
why; the why is the part that is not in the diff.

## No client files. Ever.

Ultima Online's client data (`map*.mul`, `statics*.mul`, `tiledata.mul`, the UOP
containers) is copyrighted and is not ours to redistribute. **None of it goes in
this repository, in any form, in any commit** — not as a test fixture, not as a
trimmed sample, not accidentally under `data/`.

The engine reads a *format*, not any particular shard's files. Point
`world.client_files` at whatever install you already have, and export
`OPENSHARD_CLIENT` for the tests that need one — they skip when it is unset.
Don't commit a path to your own machine either; those go in `CLAUDE.local.md`,
which is gitignored.

## Reference emulators

SphereServer and ServUO are **read**, never copied, never vendored, and never a
dependency. They are worth reading for observed protocol behaviour, which is two
decades of hard-won knowledge about which client breaks on what. They are not
worth reading for architecture — where the two of them agree about *engine*
design, that is usually the strongest available argument for doing it
differently here.

## Licence

The engine is GPL-3.0 ([`LICENSE`](LICENSE)) with one additional permission
([`LICENSE-EXCEPTION`](LICENSE-EXCEPTION)) that puts TypeScript content loaded
into the embedded runtime outside it. Unless you say otherwise in the pull
request, what you submit is offered under exactly those terms, with nothing
added — inbound equals outbound, so there is no CLA to sign and nothing to
countersign.

The practical consequence for a patch: code that ends up in the binary has to
arrive under terms GPL-3.0 can take. A snippet lifted from an MIT or BSD project
is fine and needs its attribution kept; a new dependency is worth a glance at
its licence field before it lands, because nothing in CI checks that yet.
