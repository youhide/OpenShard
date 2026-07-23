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
//! [`LoginServer::handle`] takes a framed packet and returns a [`Response`]. No
//! sockets, and no clock of its own — `now` is a parameter, so key expiry is
//! tested with arithmetic rather than `sleep`.
//!
//! ```
//! use std::net::Ipv4Addr;
//! use std::time::Instant;
//! use openshard_login::{single_shard, DevAccounts, LoginServer, LoginSession, Response};
//! use openshard_protocol::AccountLogin;
//!
//! let mut server = LoginServer::new(
//!     DevAccounts::new().with_account("admin", "hunter2"),
//!     "OpenShard",
//!     single_shard(Ipv4Addr::new(127, 0, 0, 1), 2593),
//! );
//! let mut session = LoginSession::new();
//!
//! let login = AccountLogin {
//!     account: "admin".to_owned(),
//!     password: "hunter2".to_owned(),
//! };
//! let response = server.handle(&mut session, &login.encode(), Instant::now());
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
//! see the note on [`Accounts`]. [`DevAccounts`] does store them, and is for
//! development only.

mod accounts;
pub mod auth;
pub mod password;
mod session;

pub use accounts::{Accounts, DevAccount, DevAccounts};
pub use auth::{AuthKeys, PendingLogin};
pub use session::{single_shard, LoginServer, LoginSession, Response};
