//! The login conversation as a state machine.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Instant;

use openshard_protocol::{
    encode_character_list, encode_login_denied, encode_relay, encode_shard_list, AccountLogin,
    ClientVersion, ClientVersionReport, DenyReason, GameServerLogin, Seed, SelectShard, ShardEntry,
    StartLocation,
};
use tracing::{debug, warn};

use crate::accounts::Accounts;
use crate::auth::AuthKeys;

/// What the caller should do with a connection after a packet.
#[derive(Clone, PartialEq, Eq, Debug)]
#[must_use = "a login response is the whole point of handling the packet"]
pub enum Response {
    /// Nothing to send; keep reading.
    Idle,
    /// Send these bytes and keep reading.
    Send(Vec<u8>),
    /// Send these bytes, then close.
    ///
    /// Used for both refusals and the relay. The relay is a close because the
    /// client is about to open a new connection to the game server anyway; the
    /// old one has no further purpose.
    SendThenClose(Vec<u8>),
    /// Close without sending. The client broke the conversation.
    Close,
}

/// Where a login has got to.
#[derive(Clone, PartialEq, Eq, Debug)]
enum State {
    /// Nothing yet. Expecting 0x80 (login server) or 0x91 (game server).
    Fresh,
    /// The account checked out and the shard list went back. Expecting 0xA0.
    ShardListSent {
        /// Who is logging in.
        account: String,
    },
    /// The character list went back. The login crate's job is done.
    CharacterListSent,
    /// The conversation is over.
    Finished,
}

/// One client's progress through login.
///
/// # Sans-io
///
/// Packets in, [`Response`]s out. No sockets, no clock of its own — `now` is a
/// parameter. The whole conversation is testable as a sequence of byte slices.
#[derive(Debug)]
pub struct LoginSession {
    state: State,
    /// What the client claims to be, from the seed or `0xBD`.
    ///
    /// Defaults to the oldest possible client, which is the conservative
    /// choice: every feature gate is "since version X", so an unknown client
    /// gets the plainest dialect rather than packets it cannot parse.
    version: ClientVersion,
}

impl Default for LoginSession {
    fn default() -> Self {
        Self::new()
    }
}

impl LoginSession {
    /// A session expecting its first packet.
    pub const fn new() -> Self {
        Self {
            state: State::Fresh,
            version: ClientVersion::OLDEST,
        }
    }

    /// What the client claims to be.
    pub const fn version(&self) -> ClientVersion {
        self.version
    }

    /// Whether the conversation has run to its end.
    pub fn is_finished(&self) -> bool {
        matches!(self.state, State::Finished | State::CharacterListSent)
    }

    /// Record the version the seed carried, if any.
    pub fn on_seed(&mut self, seed: Seed) {
        if let Some(version) = seed.version {
            self.version = version;
        }
    }
}

/// Everything a login server needs that outlives one connection.
///
/// A plain value the caller owns. Nothing here is a static.
#[derive(Debug)]
pub struct LoginServer<A: Accounts> {
    /// Where accounts live.
    pub accounts: A,
    /// Keys issued at relay, redeemed at game login.
    pub keys: AuthKeys,
    /// The shard list to advertise.
    pub shards: Vec<ShardEntry>,
    /// Where to send a client after it picks a shard.
    pub game_address: SocketAddrV4,
    /// The starting cities offered at character creation.
    pub starts: Vec<StartLocation>,
    /// The client-capability mask for the 0xA9 list.
    pub character_list_flags: u32,
}

impl<A: Accounts> LoginServer<A> {
    /// A server with one shard and no starting cities.
    pub fn new(accounts: A, shard_name: &str, game_address: SocketAddrV4) -> Self {
        Self {
            accounts,
            keys: AuthKeys::new(),
            shards: vec![ShardEntry {
                name: shard_name.to_owned(),
                percent_full: 0,
                timezone: 0,
                address: *game_address.ip(),
            }],
            game_address,
            starts: Vec::new(),
            character_list_flags: 0,
        }
    }

    /// Handle one framed packet.
    ///
    /// Unknown packets are ignored rather than fatal: a client may send `0xBE`
    /// (assist version) or `0xA4` (system info) at any point in login, and
    /// dropping the connection over them would break real clients for no
    /// reason. The gateway has already proved the packet is well-framed, so
    /// ignoring one is safe — the stream is still aligned.
    pub fn handle(&mut self, session: &mut LoginSession, packet: &[u8], now: Instant) -> Response {
        let Some(&id) = packet.first() else {
            return Response::Idle;
        };

        match id {
            ClientVersionReport::ID => self.on_version_report(session, packet),
            AccountLogin::ID => self.on_account_login(session, packet),
            SelectShard::ID => self.on_select_shard(session, packet, now),
            GameServerLogin::ID => self.on_game_login(session, packet, now),
            _ => {
                debug!(id = format!("0x{id:02X}"), "ignoring packet during login");
                Response::Idle
            }
        }
    }

    fn on_version_report(&self, session: &mut LoginSession, packet: &[u8]) -> Response {
        let Ok(report) = ClientVersionReport::decode(packet) else {
            return Response::Idle;
        };
        match report.version() {
            Some(version) => {
                // Sphere accepts the version once and ignores every later 0xBD.
                // Letting a client re-report mid-session would let it change the
                // dialect after the server had already committed to one.
                if session.version == ClientVersion::OLDEST {
                    debug!(%version, "client reported its version");
                    session.version = version;
                }
            }
            // Junk here is not fatal: the seed usually carried a version, and
            // this string is free-form enough that clients put other things in
            // it.
            None => debug!(raw = report.raw, "client reported an unparseable version"),
        }
        Response::Idle
    }

    fn on_account_login(&mut self, session: &mut LoginSession, packet: &[u8]) -> Response {
        if session.state != State::Fresh {
            warn!("0x80 arrived out of order");
            return Response::Close;
        }

        let login = match AccountLogin::decode(packet) {
            Ok(login) => login,
            Err(error) => {
                warn!(%error, "malformed 0x80");
                return Response::Close;
            }
        };

        if let Err(reason) = self.accounts.verify(&login.account, &login.password) {
            // The real reason is logged; the client hears one of five codes.
            warn!(account = login.account, ?reason, "login refused");
            session.state = State::Finished;
            return Response::SendThenClose(encode_login_denied(reason));
        }

        debug!(account = login.account, "account verified");
        let list = encode_shard_list(&self.shards, session.version);
        session.state = State::ShardListSent {
            account: login.account,
        };
        Response::Send(list)
    }

    fn on_select_shard(
        &mut self,
        session: &mut LoginSession,
        packet: &[u8],
        now: Instant,
    ) -> Response {
        let State::ShardListSent { account } = &session.state else {
            warn!("0xA0 arrived before the shard list");
            return Response::Close;
        };
        let account = account.clone();

        let Ok(select) = SelectShard::decode(packet) else {
            return Response::Close;
        };
        // The wire index is one-based and untrusted; `slot` refuses zero rather
        // than underflowing.
        let Some(slot) = select.slot() else {
            warn!(index = select.index, "shard index out of range");
            return Response::Close;
        };
        if slot >= self.shards.len() {
            warn!(slot, "shard index out of range");
            return Response::Close;
        }

        // The version goes with the key: the game connection has no other way
        // to learn it. See `PendingLogin::version`.
        let key = self.keys.issue(&account, session.version, now);
        debug!(account, slot, "relaying to the game server");
        session.state = State::Finished;
        Response::SendThenClose(encode_relay(
            *self.game_address.ip(),
            self.game_address.port(),
            key,
        ))
    }

    fn on_game_login(
        &mut self,
        session: &mut LoginSession,
        packet: &[u8],
        now: Instant,
    ) -> Response {
        if session.state != State::Fresh {
            warn!("0x91 arrived out of order");
            return Response::Close;
        }

        let login = match GameServerLogin::decode(packet) {
            Ok(login) => login,
            Err(error) => {
                warn!(%error, "malformed 0x91");
                return Response::Close;
            }
        };

        // Sphere skips these four bytes entirely and re-verifies the password.
        // We check them: it costs nothing and it means the game port cannot be
        // reached without going through the login server first, which closes
        // off a whole class of "connect straight to 2593" probing. The password
        // is still checked below — the key is a session token, not the gate.
        let Some(pending) = self.keys.redeem(login.auth_key, now) else {
            warn!(account = login.account, "bad or expired auth key");
            session.state = State::Finished;
            return Response::SendThenClose(encode_login_denied(DenyReason::BadAuthId));
        };

        // Adopt the dialect the client declared on the login connection. This
        // one told us nothing but a key, and guessing "oldest" here means
        // sending an ancient character list to a modern client, which it reads
        // past the end of.
        session.version = pending.version;

        // The key says who selected the shard. If the account on this packet is
        // a different one, someone is replaying a key they did not earn.
        if !pending.account.eq_ignore_ascii_case(&login.account) {
            warn!(
                expected = pending.account,
                got = login.account,
                "auth key does not belong to this account"
            );
            session.state = State::Finished;
            return Response::SendThenClose(encode_login_denied(DenyReason::BadAuthId));
        }

        if let Err(reason) = self.accounts.verify(&login.account, &login.password) {
            warn!(account = login.account, ?reason, "game login refused");
            session.state = State::Finished;
            return Response::SendThenClose(encode_login_denied(reason));
        }

        let characters = self.accounts.characters(&login.account);
        debug!(
            account = login.account,
            count = characters.len(),
            "sending character list"
        );
        session.state = State::CharacterListSent;
        Response::Send(encode_character_list(
            &characters,
            &self.starts,
            self.character_list_flags,
            session.version,
        ))
    }
}

/// The address a shard advertises, for the common single-shard case.
pub fn single_shard(address: Ipv4Addr, port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(address, port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::DevAccounts;
    use openshard_protocol::Feature;

    fn server() -> LoginServer<DevAccounts> {
        let accounts = DevAccounts::new()
            .with_account("admin", "hunter2")
            .with_character("admin", "Lord British")
            .with_account("banned", "x")
            .blocked("banned");
        LoginServer::new(
            accounts,
            "OpenShard",
            single_shard(Ipv4Addr::new(127, 0, 0, 1), 2593),
        )
    }

    fn modern_session() -> LoginSession {
        let mut session = LoginSession::new();
        session.on_seed(Seed {
            value: 0x0A00_0001,
            version: Some(ClientVersion::TOL),
        });
        session
    }

    fn login(account: &str, password: &str) -> Vec<u8> {
        AccountLogin {
            account: account.to_owned(),
            password: password.to_owned(),
        }
        .encode()
    }

    fn game_login(key: u32, account: &str) -> Vec<u8> {
        GameServerLogin {
            auth_key: key,
            account: account.to_owned(),
            password: "hunter2".to_owned(),
        }
        .encode()
    }

    /// Take an already-authenticated session through shard select to the relay.
    fn relay_key_from(
        server: &mut LoginServer<DevAccounts>,
        session: &mut LoginSession,
        now: Instant,
    ) -> u32 {
        let Response::SendThenClose(relay) =
            server.handle(session, &SelectShard { index: 1 }.encode(), now)
        else {
            panic!("expected a relay");
        };
        u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]])
    }

    /// Run the whole conversation and return the auth key from the relay.
    fn relay_key(server: &mut LoginServer<DevAccounts>, now: Instant) -> u32 {
        let mut session = modern_session();
        assert!(matches!(
            server.handle(&mut session, &login("admin", "hunter2"), now),
            Response::Send(_)
        ));
        let Response::SendThenClose(relay) =
            server.handle(&mut session, &SelectShard { index: 1 }.encode(), now)
        else {
            panic!("expected a relay");
        };
        u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]])
    }

    #[test]
    fn the_happy_path_reaches_a_character_list() {
        let mut server = server();
        let now = Instant::now();

        // Login connection.
        let mut session = modern_session();
        let Response::Send(shards) = server.handle(&mut session, &login("admin", "hunter2"), now)
        else {
            panic!("expected the shard list");
        };
        assert_eq!(shards[0], 0xA8);

        let Response::SendThenClose(relay) =
            server.handle(&mut session, &SelectShard { index: 1 }.encode(), now)
        else {
            panic!("expected a relay");
        };
        assert_eq!(relay[0], 0x8C);
        let key = u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]);

        // Game connection: a new session, as a real client would reconnect.
        let mut session = modern_session();
        let game_login = GameServerLogin {
            auth_key: key,
            account: "admin".to_owned(),
            password: "hunter2".to_owned(),
        };
        let Response::Send(characters) = server.handle(&mut session, &game_login.encode(), now)
        else {
            panic!("expected the character list");
        };
        assert_eq!(characters[0], 0xA9);
        assert_eq!(&characters[4..16], b"Lord British");
        assert!(session.is_finished());
    }

    #[test]
    fn a_bad_password_is_refused_and_closed() {
        let mut server = server();
        let mut session = modern_session();
        let response = server.handle(&mut session, &login("admin", "wrong"), Instant::now());
        assert_eq!(
            response,
            Response::SendThenClose(vec![0x82, DenyReason::BadPassword.wire_code()])
        );
    }

    #[test]
    fn a_blocked_account_hears_blocked() {
        let mut server = server();
        let mut session = modern_session();
        let response = server.handle(&mut session, &login("banned", "x"), Instant::now());
        assert_eq!(
            response,
            Response::SendThenClose(vec![0x82, DenyReason::Blocked.wire_code()])
        );
    }

    #[test]
    fn an_unknown_account_and_a_bad_password_are_told_apart_only_in_the_log() {
        // Both are refused; the codes differ because the client renders them
        // differently and Sphere has always done this. The enumeration oracle
        // this creates is the protocol's, not ours to fix here.
        let mut server = server();
        let unknown = server.handle(
            &mut LoginSession::new(),
            &login("nobody", "x"),
            Instant::now(),
        );
        let bad = server.handle(
            &mut LoginSession::new(),
            &login("admin", "x"),
            Instant::now(),
        );
        assert_ne!(unknown, bad);
    }

    #[test]
    fn selecting_a_shard_before_logging_in_is_fatal() {
        let mut server = server();
        let mut session = modern_session();
        let response = server.handle(
            &mut session,
            &SelectShard { index: 1 }.encode(),
            Instant::now(),
        );
        assert_eq!(response, Response::Close);
    }

    #[test]
    fn logging_in_twice_is_fatal() {
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        assert!(matches!(
            server.handle(&mut session, &login("admin", "hunter2"), now),
            Response::Send(_)
        ));
        assert_eq!(
            server.handle(&mut session, &login("admin", "hunter2"), now),
            Response::Close,
            "a second 0x80 means the client lost the plot"
        );
    }

    #[test]
    fn shard_index_zero_is_refused_rather_than_underflowing() {
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        let _ = server.handle(&mut session, &login("admin", "hunter2"), now);
        assert_eq!(
            server.handle(&mut session, &SelectShard { index: 0 }.encode(), now),
            Response::Close
        );
    }

    #[test]
    fn a_shard_index_past_the_list_is_refused() {
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        let _ = server.handle(&mut session, &login("admin", "hunter2"), now);
        assert_eq!(
            server.handle(&mut session, &SelectShard { index: 99 }.encode(), now),
            Response::Close
        );
    }

    #[test]
    fn the_game_port_cannot_be_reached_without_the_login_server() {
        // The whole reason to check the auth key that Sphere ignores.
        let mut server = server();
        let mut session = modern_session();
        let forged = GameServerLogin {
            auth_key: 0xDEAD_BEEF,
            account: "admin".to_owned(),
            password: "hunter2".to_owned(),
        };
        assert_eq!(
            server.handle(&mut session, &forged.encode(), Instant::now()),
            Response::SendThenClose(vec![0x82, DenyReason::BadAuthId.wire_code()]),
            "a right password with a wrong key is still refused"
        );
    }

    #[test]
    fn an_auth_key_cannot_be_reused() {
        let mut server = server();
        let now = Instant::now();
        let key = relay_key(&mut server, now);

        let game_login = GameServerLogin {
            auth_key: key,
            account: "admin".to_owned(),
            password: "hunter2".to_owned(),
        };
        assert!(matches!(
            server.handle(&mut modern_session(), &game_login.encode(), now),
            Response::Send(_)
        ));
        assert_eq!(
            server.handle(&mut modern_session(), &game_login.encode(), now),
            Response::SendThenClose(vec![0x82, DenyReason::BadAuthId.wire_code()]),
            "someone who read the key off the wire gets nothing"
        );
    }

    #[test]
    fn an_auth_key_belongs_to_the_account_that_earned_it() {
        // Alice selects a shard; Bob presents her key with his own credentials.
        let mut server = LoginServer::new(
            DevAccounts::new()
                .with_account("alice", "a")
                .with_account("bob", "b"),
            "OpenShard",
            single_shard(Ipv4Addr::new(127, 0, 0, 1), 2593),
        );
        let now = Instant::now();

        let mut session = modern_session();
        let _ = server.handle(&mut session, &login("alice", "a"), now);
        let Response::SendThenClose(relay) =
            server.handle(&mut session, &SelectShard { index: 1 }.encode(), now)
        else {
            panic!("expected a relay");
        };
        let alices_key = u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]);

        let bob = GameServerLogin {
            auth_key: alices_key,
            account: "bob".to_owned(),
            password: "b".to_owned(),
        };
        assert_eq!(
            server.handle(&mut modern_session(), &bob.encode(), now),
            Response::SendThenClose(vec![0x82, DenyReason::BadAuthId.wire_code()]),
            "a valid key plus valid credentials for a different account is not a login"
        );
    }

    #[test]
    fn an_expired_auth_key_is_refused() {
        let mut server = server();
        let issued = Instant::now();
        let key = relay_key(&mut server, issued);

        let game_login = GameServerLogin {
            auth_key: key,
            account: "admin".to_owned(),
            password: "hunter2".to_owned(),
        };
        let too_late = issued + crate::auth::DEFAULT_TTL + std::time::Duration::from_secs(1);
        assert_eq!(
            server.handle(&mut modern_session(), &game_login.encode(), too_late),
            Response::SendThenClose(vec![0x82, DenyReason::BadAuthId.wire_code()])
        );
    }

    #[test]
    fn a_valid_key_with_a_wrong_password_is_still_refused() {
        // The key is a session token, not the gate.
        let mut server = server();
        let now = Instant::now();
        let key = relay_key(&mut server, now);

        let game_login = GameServerLogin {
            auth_key: key,
            account: "admin".to_owned(),
            password: "wrong".to_owned(),
        };
        assert_eq!(
            server.handle(&mut modern_session(), &game_login.encode(), now),
            Response::SendThenClose(vec![0x82, DenyReason::BadPassword.wire_code()])
        );
    }

    #[test]
    fn the_dialect_survives_the_reconnect_to_the_game_server() {
        // The game connection is a different socket and the client says nothing
        // on it: four bytes of key, then 0x91. No seed, no version.
        //
        // So a session that only knows its own socket knows nothing, falls back
        // to the oldest dialect, and sends a 1997 character list to a modern
        // client — no padding, narrow city names, no trailing flags. The client
        // reads the fields it expects, runs off the end, and desynchronises. It
        // surfaces as a garbage packet id hundreds of bytes later and looks
        // nothing like a version problem.
        //
        // The key is the only thing linking the two connections, so the version
        // rides on the key.
        let mut server = server();
        let now = Instant::now();

        // Connection one: the client announces a modern version in the seed.
        let mut first = LoginSession::new();
        first.on_seed(Seed {
            value: 1,
            version: Some(ClientVersion::TOL),
        });
        let Response::Send(_) = server.handle(&mut first, &login("admin", "hunter2"), now) else {
            panic!("expected the shard list");
        };
        let key = relay_key_from(&mut server, &mut first, now);

        // Connection two: a brand new session that has been told nothing.
        let mut second = LoginSession::new();
        assert_eq!(
            second.version(),
            ClientVersion::OLDEST,
            "the game socket carries no version of its own"
        );

        let Response::Send(list) = server.handle(&mut second, &game_login(key, "admin"), now)
        else {
            panic!("expected the character list");
        };
        assert_eq!(
            second.version(),
            ClientVersion::TOL,
            "the key must carry the dialect across the gap"
        );

        // And the list is in the modern shape, which is the thing the client
        // actually chokes on: five padded slots and a trailing flags dword.
        let modern = encode_character_list(
            &server.accounts.characters("admin"),
            &server.starts,
            server.character_list_flags,
            ClientVersion::TOL,
        );
        assert_eq!(list, modern, "the client must get its own dialect");
    }

    #[test]
    fn a_key_from_an_ancient_client_does_not_promote_it() {
        // The other direction: the key carries whatever was declared, and an old
        // client must keep getting the old shape.
        let mut server = server();
        let now = Instant::now();

        let mut first = LoginSession::new();
        first.on_seed(Seed {
            value: 1,
            version: Some(ClientVersion::new(2, 0, 0, 0)),
        });
        let Response::Send(_) = server.handle(&mut first, &login("admin", "hunter2"), now) else {
            panic!("expected the shard list");
        };
        let key = relay_key_from(&mut server, &mut first, now);

        let mut second = LoginSession::new();
        let Response::Send(_) = server.handle(&mut second, &game_login(key, "admin"), now) else {
            panic!("expected the character list");
        };
        assert_eq!(second.version(), ClientVersion::new(2, 0, 0, 0));
    }

    #[test]
    fn the_seed_version_shapes_the_shard_list() {
        // What this actually protects is the wiring: the version arrives in the
        // seed, and the encoder cannot ask for it. If the seed stops reaching
        // `encode_shard_list` every client gets whatever the default is, and
        // half of them get an address backwards.
        //
        // Which order belongs to which client is `encode_shard_list`'s business
        // and is pinned there. This asserts only that the two differ and that
        // the boundary is where the seed says.
        let mut server = server();
        let now = Instant::now();

        let mut modern = LoginSession::new();
        modern.on_seed(Seed {
            value: 1,
            version: Some(ClientVersion::new(4, 0, 0, 0)),
        });
        let Response::Send(list) = server.handle(&mut modern, &login("admin", "hunter2"), now)
        else {
            panic!("expected the shard list");
        };
        assert_eq!(&list[42..46], &[1, 0, 0, 127], "reversed since 4.0.0");

        let mut ancient = LoginSession::new();
        ancient.on_seed(Seed {
            value: 1,
            version: Some(ClientVersion::new(3, 255, 255, 255)),
        });
        let Response::Send(list) = server.handle(&mut ancient, &login("admin", "hunter2"), now)
        else {
            panic!("expected the shard list");
        };
        assert_eq!(&list[42..46], &[127, 0, 0, 1], "in order below it");
    }

    #[test]
    fn a_client_with_no_version_gets_the_plainest_dialect() {
        // A legacy seed carries no version. Defaulting to OLDEST means every
        // feature gate says no, which is the only safe guess: sending a packet
        // the client cannot parse gets silence, not an error.
        let session = LoginSession::new();
        assert_eq!(session.version(), ClientVersion::OLDEST);
        assert!(!session.version().supports(Feature::ReversedShardIp));
    }

    #[test]
    fn a_version_report_fills_in_a_legacy_seed() {
        let mut server = server();
        let mut session = LoginSession::new();
        session.on_seed(Seed {
            value: 0xC0A8_0001,
            version: None,
        });
        assert_eq!(session.version(), ClientVersion::OLDEST);

        let report = ClientVersionReport {
            raw: "7.0.45.65".to_owned(),
        };
        let _ = server.handle(&mut session, &report.encode(), Instant::now());
        assert_eq!(session.version(), ClientVersion::new(7, 0, 45, 65));
    }

    #[test]
    fn a_second_version_report_is_ignored() {
        // Sphere accepts the version once. Letting a client re-report would let
        // it change dialect after the server had committed to one.
        let mut server = server();
        let mut session = modern_session();
        assert_eq!(session.version(), ClientVersion::TOL);

        let report = ClientVersionReport {
            raw: "3.0.7b".to_owned(),
        };
        let _ = server.handle(&mut session, &report.encode(), Instant::now());
        assert_eq!(session.version(), ClientVersion::TOL, "unchanged");
    }

    #[test]
    fn an_unparseable_version_report_is_not_fatal() {
        let mut server = server();
        let mut session = LoginSession::new();
        let report = ClientVersionReport {
            raw: "garbage".to_owned(),
        };
        assert_eq!(
            server.handle(&mut session, &report.encode(), Instant::now()),
            Response::Idle
        );
    }

    #[test]
    fn packets_that_do_not_belong_to_login_are_ignored_not_fatal() {
        // Real clients send 0xBE and 0xA4 during login. Closing on them would
        // break every one of them for no reason, and the gateway has already
        // proved the stream is still aligned.
        let mut server = server();
        let mut session = modern_session();
        for packet in [
            vec![0xBE, 0x00, 0x04, 0x00], // assist version
            vec![0xA4; 149],              // system info
            vec![0x73, 0x00],             // ping
        ] {
            assert_eq!(
                server.handle(&mut session, &packet, Instant::now()),
                Response::Idle,
                "0x{:02X} must not drop the connection",
                packet[0]
            );
        }
        // And login still works afterwards.
        assert!(matches!(
            server.handle(&mut session, &login("admin", "hunter2"), Instant::now()),
            Response::Send(_)
        ));
    }

    #[test]
    fn an_empty_packet_is_ignored() {
        let mut server = server();
        assert_eq!(
            server.handle(&mut LoginSession::new(), &[], Instant::now()),
            Response::Idle
        );
    }

    #[test]
    fn a_malformed_login_is_fatal() {
        let mut server = server();
        assert_eq!(
            server.handle(&mut LoginSession::new(), &[0x80, 0x01], Instant::now()),
            Response::Close
        );
    }
}
