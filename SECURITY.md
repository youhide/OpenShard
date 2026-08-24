# Security Policy

## Supported versions

OpenShard is pre-1.0 and moves fast. Only the **latest release** and the current
`main` get fixes; there are no backports to earlier tags, and a `v0.0.x` number
promises nothing about compatibility.

| Version | Supported |
|---|---|
| latest release / `main` | yes |
| anything older | no — update |

## Reporting a vulnerability

**Report privately through GitHub.** Open the repository's
[Security tab](https://github.com/youhide/OpenShard/security/advisories/new) and
use *Report a vulnerability*. That opens a private advisory only the maintainers
can read.

Please do **not** open a public issue for something exploitable. A shard runs on
somebody's machine with a port open to the internet; a public issue is a working
recipe before there is a fix.

Useful in a report: what you sent (a packet dump or the bytes are ideal), what
happened, and — if it is protocol-related — which client and version, since much
of this engine's behaviour is version-gated.

This is a volunteer project. Reports are handled on a best-effort basis, and a
fix ships when it is correct rather than to a schedule. You will get an
acknowledgement that a human read it.

## What counts

The engine is a network server that parses hostile input by design: a shard
accepts connections from arbitrary clients, and everything a client sends is
attacker-controlled. In scope:

- **A malformed packet that crashes, hangs or exhausts memory on the server.**
  A length claimed off the wire that is never checked against the buffer, a
  panic in a decoder, an allocation sized by an attacker.
- **Authentication bypass** — logging in as another account, reusing or guessing
  an auth key, or crossing the login/game connection boundary as somebody else.
- **Anything a client can do to reach outside the game**: reading or writing
  files on the host, executing code, escaping the script sandbox's op surface,
  or injecting into the `Store` (SQLite or PostgreSQL).
- **Privilege escalation in-game** where it crosses a trust boundary — a player
  reaching a staff command they were never granted.

Ordinary gameplay bugs — a duplication exploit, a skill that trains too fast, a
mob that walks through a wall — are ordinary issues. Open them publicly; they
are not security reports and they get fixed faster in the open.

## What is deliberate, and not a vulnerability

Two of these look like findings and are documented decisions. Please read them
before reporting:

- **The connection is not encrypted, and login encryption is deferred on
  purpose.** The classic UO scheme uses a per-client-version key table extracted
  from the client binary — anyone can read the stream, so it is obfuscation, not
  security. ClassicUO connects with encryption off and that is what shards use
  in practice. See `docs/roadmap/01-protocol.md`. Do not mistake it for a
  security feature if it ever lands.
- **The UO protocol sends the password in plaintext.** There is no challenge and
  no nonce, and no server can fix that from this end. What a server can control
  is storage, and stored credentials are argon2 PHC hashes
  (`crates/server/login/src/password.rs`). The `DevAccounts` provider does keep
  plaintext and says so — it is for development.

Development modes that are documented as such are also not reports: an empty
`world.client_files` allows every step (no map is loaded to refuse one), and an
empty `persistence.database` keeps the world in memory and never saves. The
shard says so at startup rather than implying otherwise.

## Running a shard safely

Two things are on the operator, not on the engine:

- **Change the default account.** The config written on first run contains a
  known administrator login (`admin` / `hunter2`) in plaintext, so that a fresh
  clone can be played immediately. It is a development default. Change it before
  the port is reachable from anywhere you do not control.
- **Keep `openshard.toml` out of version control.** It holds credentials and
  your network layout. It is gitignored here for that reason.
