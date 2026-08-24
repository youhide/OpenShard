# Development

The environment, not the code. What lands and how it is reviewed is
[`../CONTRIBUTING.md`](../CONTRIBUTING.md); how the code should read is
[`style.md`](style.md).

## The three commands

```sh
cargo test --workspace          # includes doctests
cargo clippy --workspace --all-targets
cargo fmt --all
```

All three are expected to be silent. They are today; keep them that way. CI runs
exactly these on every pull request, so a red build is one of the three and
nothing subtler.

For a quick compile check, `--all-targets` is not optional: without it the test
and example targets are not built at all, and a broken test file passes
`cargo check` in silence.

```sh
cargo check --workspace --all-targets
```

`rustfmt.toml` is deliberately thin — `rust-toolchain.toml` pins stable, and
stable rustfmt warns once per unstable key and then ignores it, which would make
`cargo fmt` noisy for everybody. The intended nightly settings sit commented in
that file. See [`style.md`](style.md).

Running the shard: `cargo run -p openshard-server`.

A shard with an empty world draws its map and nothing else — the engine ships no
spawn or decoration data, so the townsfolk, the doors and the shop signs all come
from the script pack under the verbs the `.admin` menu's buttons send. `--seed`
sends those verbs at boot instead, so a world can be laid without a client
attached:

```sh
cargo run -p openshard-server -- --seed regions:felucca,decorate:felucca,populate:felucca
```

The verbs are the pack's, not a list the binary checks, and they are sent in the
order given — regions before what stands in them. They are sent *every* run the
flag is passed: with `persistence.database` empty the world starts bare each
time, which is what makes this convenient, but against a real database a second
seeded start lays everything a second time. The `.admin` menu's clear verbs are
how that is undone.

### `OPENSHARD_PACK` — the migration off the script pack

Content is moving out of the pack and into this repository as `data/*.json` (see
[`architecture.md`](architecture.md) § Scripting). Each dataset that moves is
checked against the pack it came from by one test, which compares the world
`Command`s both sources produce. The pack is a separate repository and not a
dependency, so the test skips unless `OPENSHARD_PACK` names its directory —
the same bargain the client-file tests make with `OPENSHARD_CLIENT`, and the
reason `cargo test --workspace` stays green on a checkout that has neither.

```sh
OPENSHARD_PACK=/path/to/OpenShard-Community-Pack cargo test -p openshard-server content
```

Run it before every one of those PRs. When the last dataset has moved, the test
and the pack go together.

Running both ends at once — a shard and our own client logged in to it, in one
process, with no port bound and no socket opened:

```sh
cargo run -p openshard-playground -- --client "/path/to/Ultima Online Classic"
```

Both client binaries take their options from the command line or from the
environment, whichever is there — `--help` lists the two spellings side by side
— and they read a `.env` from the workspace root before parsing, so the install
can be named once. Copy [`.env.example`](../.env.example) to `.env` and fill it
in; `.env` is ignored by git and stays that way, because a path to somebody's
client install is not anyone else's. Then the command above is just

```sh
cargo run -p openshard-playground
```

The two ends are joined by a pair of in-memory pipes. Everything above the
transport is the code that runs against ClassicUO — the transport itself is a
parameter on both sides, `transport::Dial` for the client and any stream for
`gateway::Gate`. It keeps nothing: the world is in memory and goes away with the
process, which is what makes it a playground rather than a way to run a shard.

### Reproduce a static-atlas overflow while scrolling

The playground can drive its logged-in player around a fixed, expanding route.
The ordinary player-follow camera then crosses fresh Felucca map tiles without
mouse input, which is the repeatable path for the static-atlas repack hitch:

```sh
cargo run -p openshard-playground -- --atlas-scroll
```

`OPENSHARD_ATLAS_SCROLL=1` is the equivalent environment setting. The route
uses the in-process shard's normal movement commands (not teleports), turns
when a local obstacle refuses a step, and takes roughly six minutes to cross
more than 7,000 tiles. Leave the window open until
`target/openshard-playground-jank.log` contains
`atlas_overflowed=Some("statics")`; that is the static atlas boundary. The
option replaces a configured gameplay script for that run and cannot be paired
with `--mailbox-load`.

The art table and navigation graph are separate, explicit preparation steps.
Run both after installing or updating the client files; normal shard and client
startup never rebuilds the navigation graph:

```sh
cargo run --release -p openshard-client-artscan
cargo run --release -p openshard-movement --bin openshard-navigation-bake -- --facet 0
```

`OPENSHARD_ART_TABLE` and `OPENSHARD_NAVIGATION` move the outputs when the
client install is read-only. The bake command also accepts `--out FILE` (for a
single facet), repeatable `--facet N`, and `--dry-run`.

### A facet that comes from a base set

A shard can read a facet out of OpenShard's own map format instead of out of the
install — `world.base_sets` in `openshard.toml`, one entry per facet. Import it
once, then bake a navigation graph **over the base set**, which is what the
shard will check its artifact against:

```sh
cargo run --release -p openshard-uofiles --bin openshard-map-import -- \
    --facet 0 --out felucca.osbase --verify
cargo run --release -p openshard-movement --bin openshard-navigation-bake -- \
    --facet 0 --base-set felucca.osbase
```

The graph lands beside the base set rather than beside the install, because that
is where a shard reading a base set looks for it. `--client` is still required
for both commands: a base set holds the map, and `tiledata.mul` still holds what
a tile is. A shard configured with a base set and no `client_files` is refused
at startup for that reason.

### Changing a world of our own

A base set is immutable; what changes is the **patch log** beside it, at the
same name with `.ospatch` for an extension. One command commits one change:

```sh
cargo run --release -p openshard-basemap --bin openshard-map-patch -- \
    --base-set felucca.osbase --author yourname \
    set-land --x 1495 --y 1629 --tile 1004 --z 25
cargo run --release -p openshard-basemap --bin openshard-map-patch -- \
    --base-set felucca.osbase show
```

`add-static` and `remove-static` are the other two operations there are;
`list --x N --y N` prints what stands on a tile with the ordinal `remove-static`
addresses it by, and `--dry-run` says what would be committed. The world the
shard runs is the base set **plus** the log, so a committed change survives a
restart and needs no other setting.

Every bake over the facet is stale the moment a patch lands, and the navigation
graph is the one that stops a shard booting — so rebake it, over the same base
set, with the command in the section above. The tool prints it.

## No Rust toolchain? Install one without root

`rustup` is unreachable from some sandboxes — `static.rust-lang.org` is blocked —
but Ubuntu ships versioned toolchain debs that `apt-get download` can fetch and
`dpkg -x` can unpack anywhere:

```sh
cd /tmp && mkdir -p rdl88 r88 && cd rdl88
apt-get download rustc-1.88 cargo-1.88 libstd-rust-1.88 libstd-rust-1.88-dev \
                 rust-1.88-clippy rustfmt-1.88 libssh2-1 libhttp-parser2.9
for d in *.deb; do dpkg -x "$d" /tmp/r88; done
export PATH=/tmp/r88/usr/lib/rust-1.88/bin:$PATH
export LD_LIBRARY_PATH=/tmp/r88/usr/lib/x86_64-linux-gnu:/tmp/r88/usr/lib:$LD_LIBRARY_PATH
export CARGO_HOME=/tmp/cargohome CARGO_TARGET_DIR=/tmp/os-target
cargo test --workspace
```

crates.io itself is reachable, so dependencies download fine. Only `rustup`'s
host is blocked. Nothing is excluded from the test run any more: `openshard-scripting`
used to be, because `deno_core` pulled a prebuilt V8 from GitHub release assets
that such a sandbox blocks (`403`), and that crate is gone.

## Building in a small sandbox? Watch `target/`

It reached 2.7GB and filled the disk hard enough that the sandbox could no longer
start a shell to clean itself — a wedge with no way out from inside.
`[profile.dev.package."*"] debug = false` in the workspace manifest is most of the
fix and helps everyone. On top of that, in a container and not in the repo,
because they trade away things a human working locally wants:

```sh
export CARGO_INCREMENTAL=0            # the incremental cache is per-crate and large
export CARGO_PROFILE_DEV_DEBUG=0      # no symbols at all, if backtraces are not needed
du -sh "$CARGO_TARGET_DIR"            # check it before it checks you
```

## Profiling: build `profiling`, and set two sysctls first

`release` has no debug info, so a profile of it reports addresses inside one
inlined blob; `dev` is a different program. `[profile.profiling]` in the
workspace manifest is the third option — `release` plus `debug = 1`, the
file-and-line map and nothing else — so a sampling profiler names frames in the
code that actually ships.

```sh
cargo build --profile profiling -p openshard-movement --examples
perf record -F 999 -o perf.data -- ./target/profiling/examples/map_path_probe --client "$OPENSHARD_CLIENT"
perf report -i perf.data --stdio --no-children --percent-limit 0.4
```

`samply record --save-only -o profile.json.gz -- <cmd>` records the same run for
the Firefox Profiler UI (`samply load profile.json.gz`), which is the one to
reach for when the call tree matters more than the flat list.

**Both want two kernel settings, and neither survives a reboot:**

```sh
sudo sysctl -w kernel.perf_event_paranoid=1    # 2 is the usual default; perf and samply both refuse it
sudo sysctl -w kernel.perf_event_mlock_kb=4096 # 516 is the usual default; samply fails with a bare `mmap failed`
```

The second one is worth knowing about because its symptom names nothing: samply
prints `Failed to start profiling: mmap failed` and stops.

## `Cargo.lock` is committed and that is load-bearing

`rust-version` only holds because the lock pins dependency versions that respect
it — a bare `cargo update` will happily pull a transitive dep that wants a newer
MSRV or a newer edition and break the build on the stated one. If that happens,
pin it: `cargo update -p <crate> --precise <older>`.

There is no live pin today. There was one — `tokio-postgres` held at 0.7.12,
because from 0.7.13 it pulls a crypto stack (RustCrypto 0.11, `rand` 0.10) that
wanted Rust 1.85, above the then-current 1.82 MSRV. A later raise dissolved the
reason for the pin, so it was dropped: the crate floats on its declared `"0.7"`
again (currently 0.7.18, `postgres-protocol` 0.6.12).

## What holds the MSRV, measured

`rust-version = "1.96"`, and **the client is what sets it**. Measured rather than
assumed, by reading `rust_version` off every package in `cargo metadata`: the
highest demand in the tree is `wesl` 0.4.2 at **1.96.0**, a build-dependency of
`crates/client/render` (`puffin` and `egui` follow at 1.92, also the client). The
server crates build on **1.95**, the oldest toolchain to hand; the true server
floor is at or below that and was not bisected further.

It used to say 1.88, and `deno_core` was the reason. That reason is gone, and it
did not free anything — the client's shader toolchain had quietly overtaken it, so
the declared 1.88 had already stopped being true. Whoever wants the workspace back
below 1.96 should start at `wesl`.

**A live instance of exactly this, found 2026-08-07 and not yet pinned.**
`crates/client/render`'s build-dependency `wesl = "0.4"` (the WESL-to-WGSL
shader compiler, see [`lighting_raymarch.md`](lighting_raymarch.md)) resolves
today to `wesl` 0.4.2 in `Cargo.lock`, and 0.4.2's own `Cargo.toml` states
`rust-version = "1.96.0"` — above this workspace's `1.88`, confirmed by
reading the crate's manifest directly
(`~/.cargo/registry/src/*/wesl-0.4.2/Cargo.toml`). `wesl` 0.4.0, the version
current when it was first vendored, declared `1.87.0`. Not caught by CI
because the toolchain actually in use here is newer than both numbers — the
drift is latent, not a build failure yet. Not fixed here (found while
rewriting the lighting docs, out of scope for that pass): if a future
`cargo update` or a clean environment on an older toolchain hits this, pin
`wesl` the way `tokio-postgres` was pinned above.

## The toolchain is newer than the lints the tree was written against

`cargo clippy --workspace --all-targets` is expected to be silent — it is what
CI runs — and on the local toolchain it is not, as of 2026-08-11. The warnings
are all one shape: lints that did not exist when the code was written, firing on
code nobody has touched.

```
chunks_exact_to_as_chunks   crates/common/protocol/src/{codec,speech}.rs,
                            crates/client/render/src/png.rs, and several tests
```

Nothing here is a defect, and none of it is a reason to edit a file a session is
not otherwise in: a sweep like this belongs to one pass over the workspace with
one commit, not to whichever session happens to run clippy next. **What matters
while it lasts is reading the output rather than the exit code** — a session's
own warning is easy to miss in a list of a dozen inherited ones, so check the
paths, not the count.
