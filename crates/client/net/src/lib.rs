//! The client's side of the wire.
//!
//! Everything a program needs to talk to an OpenShard — or to any UO server —
//! as a client: split the incoming stream into packets, decompress the game
//! connection, and walk the login conversation from the first seed to standing
//! in the world.
//!
//! # The shape mirrors the gateway, on purpose
//!
//! [`Connection`](connection::Connection) is [`openshard_gateway::Connection`]
//! read the other way: bytes in, events out, no socket anywhere near it. The
//! reason is the same one the server has — what is hard here is byte
//! boundaries, and a real socket will not reproduce them on demand. A test can
//! hand this a packet split across three reads; a `TcpStream` cannot be asked
//! to.
//!
//! [`transport`] is the part that does touch tokio, and it decides nothing.
//!
//! # This crate is below any client
//!
//! It knows about packets and about the order they arrive in. It does not know
//! what a tile is, does not draw, and does not decide what to do about what it
//! read. A headless bot, a test harness and a renderer all sit on top of it.
//!
//! [`openshard_gateway::Connection`]: https://docs.rs/openshard-gateway

pub mod action;
pub mod casting;
pub mod combat;
pub mod connection;
pub mod doll;
pub mod drag;
pub mod interact;
pub mod party;
pub mod properties;
pub mod session;
pub mod skill;
pub mod talk;
pub mod target;
pub mod transport;
pub mod vendor;
pub mod view;
pub mod walk;
