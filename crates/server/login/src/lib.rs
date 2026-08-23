//! Login/auth server: account verification, shard list, and hand-off to the game server.
//!
//! ```text
//!   0x80 login ──> verify ──> 0xA8 shard list ──> 0xA0 select ──> 0x8C relay
//!                     │                                              │
//!                     └──> 0x82 denied                          auth key
//!                                                                    │
//!   ── the client reconnects to the game server ──────────────────────
//!                                                                    │
//!   0x91 game login ──> redeem key ──> verify ──> 0xA9 character list
//! ```
//!
//! # Sans-io, like the gateway
//!
//! [`LoginServer::handle`] takes an already-decoded [`LoginStagePacket`] and
//! returns an [`Outcome`] — bytes to send, or a password to check. No sockets,
//! no packet buffers, no threads, and no clock of its own: `now` is a parameter,
//! so key expiry is tested with arithmetic rather than `sleep`. Decoding is the
//! caller's job: the `server` crate's `parse_packet` does it once, ahead of
//! routing to this crate or the world.
//!
//! ```
//! use std::net::Ipv4Addr;
//! use std::time::Instant;
//! use openshard_login::{single_shard, DevAccounts, LoginServer, LoginSession, Outcome, Response};
//! use openshard_protocol::identity::{AccountName, PlaintextPassword, RawAccountName, RawPlaintextPassword};
//! use openshard_protocol::login::{AccountLogin, LoginStagePacket};
//!
//! let mut server = LoginServer::new(
//!     DevAccounts::new().with_account(&AccountName::new("admin"), &PlaintextPassword::new("hunter2")),
//!     "OpenShard",
//!     single_shard(Ipv4Addr::new(127, 0, 0, 1), 2593),
//! );
//! let mut session = LoginSession::new();
//!
//! let login = AccountLogin {
//!     account: RawAccountName("admin".to_owned()),
//!     password: RawPlaintextPassword("hunter2".to_owned()),
//! };
//! let packet = LoginStagePacket::decode(&login.encode(), session.version()).unwrap();
//!
//! // The account exists and is not blocked, so what comes back is the slow half
//! // of the login: argon2, which the shard runs on a blocking task because it
//! // is most of a tick. A doctest has nothing to stall, so it runs it here.
//! let Outcome::Verify(check) = server.handle(&mut session, packet, Instant::now()) else {
//!     panic!("a 0x80 asks for a password check");
//! };
//! let response = server.resume(&mut session, check.run());
//!
//! // The shard list goes back.
//! assert!(matches!(response, Response::Send(bytes) if bytes[0] == 0xA8));
//! ```
//!
//! # The auth key
//!
//! Sphere skips the four key bytes in `0x91` and re-verifies the password.
//! OpenShard checks them. It costs nothing, and it means the game port cannot
//! be reached without going through the login server first — which closes off a
//! class of probing straight at 2593. The password is still checked either way:
//! the key is a session token, not the gate.
//!
//! Keys come from the OS entropy pool, are one-shot, expire after
//! [`auth::DEFAULT_TTL`], and are bound to the account that earned them.
//!
//! # Passwords
//!
//! The UO protocol sends them in plaintext. There is no challenge and no nonce,
//! and no server can fix that. What a server *can* do is refuse to store them —
//! see [`DevAccounts`]. It stores password hashes and is for
//! development only.

mod accounts;
pub mod auth;
pub mod password;
mod session;

pub use accounts::{Credential, CredentialCheck, DevAccount, DevAccounts, PasswordVerdict};
pub use auth::{AuthKeys, PendingLogin};
pub use session::{LoginServer, LoginSession, Outcome, Response, single_shard};
