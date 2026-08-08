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
cargo test --workspace --exclude openshard-scripting
```

crates.io itself is reachable, so dependencies download fine. Only `rustup`'s
host is blocked. `openshard-scripting` is excluded because `deno_core` pulls a
prebuilt V8 from GitHub release assets, which such a sandbox blocks (`403`) — that
crate builds on a normal dev machine, not there. It is also what holds the
workspace MSRV at 1.88: `deno_core`'s tree does not build below it.

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

## `Cargo.lock` is committed and that is load-bearing

`rust-version = "1.88"` only holds because the lock pins dependency versions that
respect it — a bare `cargo update` will happily pull a transitive dep that wants a
newer MSRV or a newer edition and break the build on the stated one. If that
happens, pin it: `cargo update -p <crate> --precise <older>`.

There is no live pin today. There was one — `tokio-postgres` held at 0.7.12,
because from 0.7.13 it pulls a crypto stack (RustCrypto 0.11, `rand` 0.10) that
wanted Rust 1.85, above the old 1.82 MSRV. The scripting spike raised the MSRV to
1.88, which dissolved the reason for the pin, so it was dropped: the crate floats
on its declared `"0.7"` again (currently 0.7.18, `postgres-protocol` 0.6.12). The
mechanism above is what to reach for if a future update pulls something past 1.88.

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
