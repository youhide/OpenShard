//! Accepts client TCP connections and pumps encoded packets to and from the world server.
//!
//! # Sans-io
//!
//! The connection logic is a pure state machine — [`Connection`] — that has
//! never heard of a socket. Bytes go in, events come out. [`Server`] is a thin
//! Tokio adapter over it.
//!
//! The split is deliberate. Everything that is actually hard about a UO gateway
//! is about byte boundaries: a seed split across three TCP segments, four
//! packets in one read, a client that claims a 60KB length and then goes quiet.
//! A real socket will not reproduce any of those on demand, so testing through
//! one means testing the easy path and hoping. As a pure state machine each is
//! a deterministic unit test with no ports, no sleeps and no flakes — and what
//! is left in [`Server`] is small enough to read in one sitting.
//!
//! ```
//! use openshard_gateway::{Connection, Event};
//!
//! let mut connection = Connection::new();
//! connection.receive(&[192, 168, 0, 1]); // legacy seed
//! connection.receive(&[0x73, 0x00]);     // ping
//!
//! assert!(matches!(connection.poll().unwrap(), Some(Event::Seeded(_))));
//! assert_eq!(connection.poll().unwrap(), Some(Event::Packet(vec![0x73, 0x00])));
//! ```
//!
//! # The boundary
//!
//! [`Server`] hands events to a channel, not a callback. A callback would run
//! world code inside a network task, on whatever thread Tokio picked, at
//! whatever moment bytes happened to arrive — which is the end of a
//! deterministic simulation. The channel is where "async everywhere" stops and
//! the tick begins.
//!
//! # What this crate does not do
//!
//! It does not know what a packet means. It finds packet boundaries and passes
//! them on; login, movement and the rest read them. That is the whole job, and
//! it should stay that small.

mod connection;
mod server;

pub use connection::{Connection, ConnectionError, Event};
pub use server::{ConnectionId, Server, ServerEvent};
