//! The login conversation as a state machine.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Instant;

use openshard_protocol::identity::AccountName;
use openshard_protocol::login::{
    AccountLogin, ClientVersionReport, DenyReason, GameServerLogin, LoginDenied, LoginStagePacket,
    PercentFull, Relay, SelectShard, ShardEntry, ShardList,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::{seed::Seed, version::ClientVersion};
use tracing::{debug, warn};

use crate::accounts::{CredentialCheck, DevAccounts, PasswordVerdict};
use crate::auth::AuthKeys;

/// Why a connection is closing, carried from the failure site to whoever logs
/// or accounts for the disconnect at the far end of the channel.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reason(pub &'static str);

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

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
    /// Close without sending. The client broke the conversation, for the
    /// carried [`Reason`].
    Close(Reason),
}

/// What handling a login packet asks of the caller.
///
/// Two things and not one, because a login has a slow half. Everything a store
/// knows is decided while the packet is in hand; comparing the password is
/// argon2 — 19 MiB and two passes, most of a 50 ms tick — and the shard's loop
/// is the one thread its whole simulation runs on. So the check comes back as
/// work rather than being done here, and the verdict returns through
/// [`LoginServer::resume`]. See [`Credential`](crate::Credential) and `docs/connection_state.md` S6.
///
/// A caller with nothing to stall can run it on the spot: `check.run()` and
/// straight into `resume` is two lines, and that is what the tests do.
#[derive(Debug)]
#[must_use = "a login outcome is the whole point of handling the packet"]
pub enum Outcome {
    /// Answer the client, or do not — see [`Response`].
    Reply(Response),
    /// Check a password, off whatever thread must not wait for it, and bring
    /// the [`PasswordVerdict`] back to [`LoginServer::resume`].
    ///
    /// The connection is *waiting* until then: the login state machine has
    /// recorded who is being verified and will accept no other packet on this
    /// socket in the meantime.
    Verify(CredentialCheck),
}

/// Where a login has got to.
///
/// # The fork is the point
///
/// A client dials twice, and the two sockets are different conversations: the
/// login socket does `0x80`/`0xA0` and hands out a key, the game socket does
/// `0x91` and everything after. Which one this is cannot be known until the
/// first packet arrives, and once it is known it never changes — so it is a
/// branch of this enum rather than a flag beside it. Everything that used to be
/// a field the caller had to keep in step (whose account it is, whether the
/// socket is compressed) is a payload or a variant here instead, because a state
/// machine with facts kept outside it is a state machine that can disagree with
/// itself.
#[derive(Clone, PartialEq, Eq, Debug)]
enum LoginSessionState {
    /// Nothing yet. Expecting 0x80 (this is the login socket) or 0x91 (the game
    /// socket).
    Fresh,
    /// Login socket: a `0x80` named an account that exists and is not blocked,
    /// and its password is being checked somewhere else. Expecting nothing —
    /// the next thing that moves this session is a verdict, not a packet.
    ///
    /// The account is here rather than travelling with the check because it is
    /// the answer to "who is being let in", and this is the thing that will let
    /// them in. See [`Credential::against`](crate::Credential::against).
    VerifyingAccount {
        /// Who the `0x80` named, if the password turns out to be theirs.
        account: AccountName,
    },
    /// Login socket: the account checked out and the shard list went back.
    /// Expecting 0xA0.
    ShardListSent {
        /// Who is logging in. Read back by the relay, which binds the auth key
        /// to it, and by nothing else — see [`LoginSession::account`] for why
        /// this is deliberately not the account the rest of the shard sees.
        account: AccountName,
    },
    /// Login socket: refused, or relayed to the game server. Either way this
    /// socket has nothing left to say.
    LoginDone,
    /// Game socket: a `0x91` arrived. See [`GameState`].
    Game(GameState),
}

/// Where a *game* socket has got to.
///
/// Every variant means the same thing about the wire: from the moment the `0x91`
/// was read, every server-to-client packet on this socket is Huffman-compressed
/// — refusals included. That is Sphere's `CONNECT_GAME`, which it sets during
/// the crypt handshake, before the password is so much as looked at.
#[derive(Clone, PartialEq, Eq, Debug)]
enum GameState {
    /// The `0x91` carried a good key for an account that exists, and its password
    /// is being checked somewhere else. The socket is already compressed — that
    /// was settled by reading the `0x91` — and the next thing that moves this
    /// session is a verdict.
    Verifying {
        /// Whose account this socket will be authenticated to, if the password
        /// turns out to be theirs.
        account: AccountName,
    },
    /// The `0x91` was refused: a bad or expired key, a key belonging to somebody
    /// else, or a bad password. Compressed, and going nowhere.
    Refused,
    /// The character list went back. The login crate's job is done — character
    /// select, creation and deletion all happen on this socket afterwards, and
    /// every one of them needs to know whose account it is, which is why the
    /// account rides here rather than in a field of its own.
    CharacterListSent {
        /// Whose account this socket is authenticated to play on.
        account: AccountName,
    },
}

/// One client's progress through login.
///
/// # Sans-io
///
/// Packets in, [`Response`]s out. No sockets, no clock of its own — `now` is a
/// parameter. The whole conversation is testable as a sequence of byte slices.
#[derive(Debug)]
pub struct LoginSession {
    state: LoginSessionState,
    /// What the client claims to be, from the seed or `0xBD`.
    ///
    /// Defaults to the oldest possible client, which is the conservative
    /// choice: every feature gate is "since version X", so an unknown client
    /// gets the plainest dialect rather than packets it cannot parse.
    ///
    /// A field and not part of [`LoginSessionState`] because it is the other
    /// axis: what the client *is*, not where the conversation has got to. It is
    /// learned once, from the seed or a `0xBD`, and holds for every state.
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
            state: LoginSessionState::Fresh,
            version: ClientVersion::OLDEST,
        }
    }

    /// What the client claims to be.
    pub const fn version(&self) -> ClientVersion {
        self.version
    }

    /// Whose account this connection may play a character on, or `None` if it
    /// may not play one at all.
    ///
    /// Character creation, deletion and select all need it: those packets name a
    /// character but not the account it belongs to.
    ///
    /// Deliberately *only* the game socket's account, even though the login
    /// socket verifies one too. What this answers is "whose character may this
    /// connection play", and only a game login answers it — a `0x5D` arriving on
    /// a login socket that got as far as the shard list must find nothing here.
    pub fn account(&self) -> Option<&AccountName> {
        match &self.state {
            LoginSessionState::Game(GameState::CharacterListSent { account }) => Some(account),
            // A session being verified has an account *named* but not
            // authenticated, and this is the question the rest of the shard asks
            // to decide whether a connection may act. It answers no until the
            // password has actually matched.
            LoginSessionState::Fresh
            | LoginSessionState::VerifyingAccount { .. }
            | LoginSessionState::ShardListSent { .. }
            | LoginSessionState::LoginDone
            | LoginSessionState::Game(GameState::Verifying { .. })
            | LoginSessionState::Game(GameState::Refused) => None,
        }
    }

    /// Whether this is a game socket: a `0x91` was read on it, valid or not.
    ///
    /// "Read", not "accepted" — see [`GameState`]. The caller compresses on this,
    /// and reads it back off the state machine rather than peeking the packet's
    /// id byte itself, which would be [`LoginStagePacket`]'s decode redone by
    /// hand.
    pub const fn is_game_login(&self) -> bool {
        matches!(self.state, LoginSessionState::Game(_))
    }

    /// Whether the conversation has run to its end — this crate has nothing more
    /// to say on this socket, whether it ended in a relay, a refusal, or a
    /// character list.
    pub const fn is_finished(&self) -> bool {
        matches!(
            self.state,
            LoginSessionState::LoginDone | LoginSessionState::Game(_)
        )
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
pub struct LoginServer {
    /// Where accounts live.
    pub accounts: DevAccounts,
    /// Keys issued at relay, redeemed at game login.
    pub keys: AuthKeys,
    /// The shard list to advertise.
    pub shards: Vec<ShardEntry>,
    /// Where to send a client after it picks a shard.
    pub game_address: SocketAddrV4,
}

impl LoginServer {
    /// A server with one shard and no starting cities.
    pub fn new(accounts: DevAccounts, shard_name: &str, game_address: SocketAddrV4) -> Self {
        Self {
            accounts,
            keys: AuthKeys::new(),
            shards: vec![ShardEntry {
                name: shard_name.to_owned(),
                percent_full: PercentFull::EMPTY,
                timezone: 0,
                address: *game_address.ip(),
            }],
            game_address,
        }
    }

    /// Handle one already-decoded packet.
    ///
    /// Takes a [`LoginStagePacket`], not raw bytes: decoding is
    /// [`LoginStagePacket::decode`]'s job, called exactly once by whoever
    /// routes the connection's packets (the `server` crate's `parse_packet`).
    /// This crate never touches a packet buffer.
    ///
    /// Unknown packets are ignored rather than fatal: a client may send `0xBE`
    /// (assist version) or `0xA4` (system info) at any point in login, and
    /// dropping the connection over them would break real clients for no
    /// reason.
    pub fn handle(&mut self, session: &mut LoginSession, packet: LoginStagePacket, now: Instant) -> Outcome {
        match self.try_handle(session, packet, now) {
            Ok(outcome) => outcome,
            Err(reason) => Outcome::Reply(Response::Close(reason)),
        }
    }

    /// Take back the verdict on a password this session asked to have checked.
    ///
    /// The other half of [`Outcome::Verify`], and the only thing that moves a
    /// session out of a verifying state. Who was being verified comes off the
    /// state, not off the verdict — the caller is trusted to bring the answer
    /// back to the connection it took the check from, and nothing else.
    ///
    /// A verdict for a session that asked for none closes it. That is not a
    /// reachable client behaviour: a session accepts no packet while verifying,
    /// so there is never a second check outstanding, and a caller that has lost
    /// track of which connection a verdict belongs to is a caller whose next act
    /// would be to authenticate the wrong one.
    pub fn resume(&mut self, session: &mut LoginSession, verdict: PasswordVerdict) -> Response {
        let (response, next_state) = match (&session.state, verdict) {
            (LoginSessionState::VerifyingAccount { account }, PasswordVerdict::Matched) => {
                debug!(%account, "account verified");
                let list = ServerPacket::ShardList(ShardList {
                    shards: self.shards.clone(),
                })
                .encode(session.version);
                let account = account.clone();
                (Response::Send(list), LoginSessionState::ShardListSent { account })
            }
            (LoginSessionState::VerifyingAccount { account }, PasswordVerdict::Rejected) => {
                warn!(%account, "login refused: bad password");
                (
                    Self::denied(session.version, DenyReason::BadPassword),
                    LoginSessionState::LoginDone,
                )
            }
            (LoginSessionState::Game(GameState::Verifying { account }), PasswordVerdict::Matched) => {
                debug!(%account, "game login accepted; the world takes it from here");
                let account = account.clone();
                (
                    Response::Idle,
                    LoginSessionState::Game(GameState::CharacterListSent { account }),
                )
            }
            (LoginSessionState::Game(GameState::Verifying { account }), PasswordVerdict::Rejected) => {
                warn!(%account, "game login refused: bad password");
                (
                    Self::denied(session.version, DenyReason::BadPassword),
                    LoginSessionState::Game(GameState::Refused),
                )
            }
            (state, verdict) => {
                warn!(
                    ?state,
                    ?verdict,
                    "a password verdict for a login that asked for none"
                );
                return Response::Close(Reason("a password verdict nothing was waiting for"));
            }
        };
        session.state = next_state;
        response
    }

    /// The `0x82` a refusal goes back as, in this session's dialect. The client
    /// hears one of five codes; the real reason was logged where it was decided —
    /// see [`DenyReason::wire_code`].
    fn denied(version: ClientVersion, reason: DenyReason) -> Response {
        Response::SendThenClose(ServerPacket::LoginDenied(LoginDenied { reason }).encode(version))
    }

    /// [`Self::handle`]'s body. A `Result` so every fatal path carries why it
    /// is fatal, rather than each site warning ad hoc and returning a bare
    /// [`Response::Close`].
    ///
    /// The login conversation is a state machine, so this reads as one: peel
    /// off the packets that arrive independent of where the conversation has
    /// got to, then match the *pair* of (current state, packet) against the
    /// transitions the protocol defines. Each defined transition hands back
    /// both the response and the state that follows it — the state to move
    /// to is never a side effect buried in a handler, it is the arm's return
    /// value. Any pairing the protocol does not define — a stray `0xA0`
    /// before the shard list, a second `0x80` — has no arm, and falls into
    /// the catch-all as "out of order".
    fn try_handle(
        &mut self,
        session: &mut LoginSession,
        packet: LoginStagePacket,
        now: Instant,
    ) -> Result<Outcome, Reason> {
        // These arrive at any point in the conversation, so they are not
        // part of the state transition table below — handling them does not
        // change what packet the login state machine is waiting for next.
        let packet = match packet {
            LoginStagePacket::VersionReport(report) => {
                return Ok(Outcome::Reply(self.on_version_report(session, report)));
            }
            // Junk here is not fatal: the seed usually carried a version,
            // and 0xBD's string is free-form enough that clients put other
            // things in it.
            LoginStagePacket::MalformedVersionReport => return Ok(Outcome::Reply(Response::Idle)),
            LoginStagePacket::Unknown(id) => {
                debug!(id = format!("0x{id:02X}"), "ignoring packet during login");
                return Ok(Outcome::Reply(Response::Idle));
            }
            other => other,
        };

        // The login state machine: what a packet means depends on where the
        // conversation has got to, so the match is on the pair, not on the
        // packet alone. `#[non_exhaustive]` on `LoginStagePacket` and any
        // state the transition table does not cover both land in the same
        // catch-all, since both mean "not a defined transition".
        let (response, next_state) = match (&session.state, packet) {
            (LoginSessionState::Fresh, LoginStagePacket::AccountLogin(login)) => {
                self.on_account_login(session, login)
            }
            (LoginSessionState::Fresh, LoginStagePacket::GameServerLogin(login)) => {
                self.on_game_login(session, login, now)
            }
            (LoginSessionState::ShardListSent { account }, LoginStagePacket::SelectShard(select)) => {
                let account = account.clone();
                self.on_select_shard(session, account, select, now)?
            }
            (state, packet) => {
                warn!(?state, ?packet, "packet arrived out of order");
                return Err(Reason("packet arrived out of order"));
            }
        };
        session.state = next_state;
        Ok(response)
    }

    fn on_version_report(&self, session: &mut LoginSession, report: ClientVersionReport) -> Response {
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

    /// Handle `0x80` from [`LoginSessionState::Fresh`]. The caller has
    /// already established that this is where the state machine allows it;
    /// this only decides the response and the state that follows.
    fn on_account_login(&self, session: &LoginSession, login: AccountLogin) -> (Outcome, LoginSessionState) {
        let credential = match self.accounts.credential(&login.account, &login.password) {
            Ok(credential) => credential,
            Err(reason) => {
                // The real reason is logged; the client hears one of five codes.
                warn!(%login.account, ?reason, "login refused");
                return (
                    Outcome::Reply(Self::denied(session.version, reason)),
                    LoginSessionState::LoginDone,
                );
            }
        };
        // Everything the store knows is settled; what is left is argon2, and it
        // is the caller's to run somewhere nothing is waiting on it. The account
        // stays here — see `Credential::against`.
        let (account, check) = credential.against(login.password);
        (
            Outcome::Verify(check),
            LoginSessionState::VerifyingAccount { account },
        )
    }

    /// Handle `0xA0` from [`LoginSessionState::ShardListSent`]. `account` is
    /// that state's payload, already taken out by the caller.
    fn on_select_shard(
        &mut self,
        session: &LoginSession,
        account: AccountName,
        select: SelectShard,
        now: Instant,
    ) -> Result<(Outcome, LoginSessionState), Reason> {
        // One check, not two: the wire index is one-based and untrusted, and
        // `validate` refuses both zero (which would underflow) and anything
        // past the list this connection was actually sent.
        let shard = match select.index.validate(self.shards.len()) {
            Ok(shard) => shard,
            Err(error) => {
                warn!(%error, "shard index out of range");
                return Err(Reason("shard index out of range"));
            }
        };

        // The version goes with the key: the game connection has no other way
        // to learn it. See `PendingLogin::version`.
        let key = self.keys.issue(&account, session.version, now);
        debug!(%account, slot = shard.0, "relaying to the game server");
        Ok((
            Outcome::Reply(Response::SendThenClose(
                ServerPacket::Relay(Relay {
                    endpoint: self.game_address,
                    auth_key: key,
                })
                .encode(session.version),
            )),
            LoginSessionState::LoginDone,
        ))
    }

    /// Handle `0x91` from [`LoginSessionState::Fresh`]. Every path out of here
    /// returns a [`LoginSessionState::Game`], refusals included: reading the
    /// `0x91` is what makes this a game socket, and a game socket is compressed
    /// from that moment whatever the key and password turn out to be. That used
    /// to be a `game_login` flag set on the first line and hoped for; it is a
    /// property of the returned state now, so it cannot be forgotten on a path.
    ///
    /// `session` is still `&mut` for `version` alone — the dialect this socket
    /// adopts from the key, which is the other axis and not state-machine state.
    fn on_game_login(
        &mut self,
        session: &mut LoginSession,
        login: GameServerLogin,
        now: Instant,
    ) -> (Outcome, LoginSessionState) {
        // Sphere skips these four bytes entirely and re-verifies the password.
        // We check them: it costs nothing and it means the game port cannot be
        // reached without going through the login server first, which closes
        // off a whole class of "connect straight to 2593" probing. The password
        // is still checked below — the key is a session token, not the gate.
        //
        // It is also the cheap gate in front of the expensive one: a `0x91` with
        // no valid key is refused here, before anything has been hashed. Somebody
        // firing forged game logins at 2593 costs this shard a hash-map lookup
        // each.
        let Some(pending) = self.keys.redeem(login.auth_key, now) else {
            warn!(%login.account, "bad or expired auth key");
            return (
                Outcome::Reply(Self::denied(session.version, DenyReason::BadAuthId)),
                LoginSessionState::Game(GameState::Refused),
            );
        };

        // Adopt the dialect the client declared on the login connection. This
        // one told us nothing but a key, and guessing "oldest" here means
        // sending an ancient character list to a modern client, which it reads
        // past the end of.
        session.version = pending.version;

        // The key says who selected the shard. If the account on this packet is
        // a different one, someone is replaying a key they did not earn.
        if pending.account.normalized() != login.account.0.to_lowercase() {
            warn!(
                expected = %pending.account,
                got = %login.account,
                "auth key does not belong to this account"
            );
            return (
                Outcome::Reply(Self::denied(session.version, DenyReason::BadAuthId)),
                LoginSessionState::Game(GameState::Refused),
            );
        }

        let credential = match self.accounts.credential(&login.account, &login.password) {
            Ok(credential) => credential,
            Err(reason) => {
                warn!(%login.account, ?reason, "game login refused");
                return (
                    Outcome::Reply(Self::denied(session.version, reason)),
                    LoginSessionState::Game(GameState::Refused),
                );
            }
        };

        // The password is the caller's to check, off the loop — see `Outcome`.
        // What follows a matching one is in `resume`, and this crate is done at
        // that point: the `0xA9` character list that used to go back from here is
        // the world's since S5 of `docs/connection_state.md`, because which
        // characters exist is the roster's and a character being *played* is a
        // fact only the world holds. Answering from here meant a second list that
        // had to agree with the world's and could not see it.
        let (account, check) = credential.against(login.password);
        (
            Outcome::Verify(check),
            LoginSessionState::Game(GameState::Verifying { account }),
        )
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
    use openshard_protocol::feature::Feature;
    use openshard_protocol::identity::{
        AccountName, PlaintextPassword, RawAccountName, RawPlaintextPassword,
    };
    use openshard_protocol::login::RawShardIndex;
    use openshard_protocol::seed::RawSeedValue;
    use openshard_protocol::wire::AuthKey;

    fn server() -> LoginServer {
        let accounts = DevAccounts::new()
            .with_account(&AccountName::new("admin"), &PlaintextPassword::new("hunter2"))
            .with_account(&AccountName::new("banned"), &PlaintextPassword::new("x"))
            .blocked(&AccountName::new("banned"));
        LoginServer::new(
            accounts,
            "OpenShard",
            single_shard(Ipv4Addr::new(127, 0, 0, 1), 2593),
        )
    }

    /// Handle a packet the way the shard does, but run the password check on
    /// this thread instead of handing it to a task.
    ///
    /// Everything below is about the *conversation* — which packet is allowed
    /// where, what goes back, what the state becomes — and where argon2 runs is
    /// not part of that. The two-step path is exercised on its own by
    /// [`a_login_waits_for_a_verdict_before_it_is_anybody`] and, over a real
    /// channel and a real blocking task, by the server crate's `verify` module.
    fn drive(
        server: &mut LoginServer,
        session: &mut LoginSession,
        packet: LoginStagePacket,
        now: Instant,
    ) -> Response {
        match server.handle(session, packet, now) {
            Outcome::Reply(response) => response,
            Outcome::Verify(check) => server.resume(session, check.run()),
        }
    }

    fn modern_session() -> LoginSession {
        let mut session = LoginSession::new();
        session.on_seed(Seed {
            value: RawSeedValue(0x0A00_0001),
            version: Some(ClientVersion::TOL),
        });
        session
    }

    fn login(account: &str, password: &str) -> Vec<u8> {
        AccountLogin {
            account: RawAccountName(account.to_owned()),
            password: RawPlaintextPassword(password.to_owned()),
        }
        .encode()
    }

    fn game_login(key: AuthKey, account: &str) -> Vec<u8> {
        GameServerLogin {
            auth_key: key,
            account: RawAccountName(account.to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        }
        .encode()
    }

    /// Decode a fixture the way `parse_packet` in `shard.rs` would, so tests
    /// build packets the same way `handle`'s only real caller does. The
    /// version is fixed rather than threaded from a `LoginSession` because
    /// none of this module's `decode_body` impls read it — only encoding
    /// varies by version here.
    fn pkt(bytes: &[u8]) -> LoginStagePacket {
        LoginStagePacket::decode(bytes, ClientVersion::OLDEST).expect("test fixture is a valid encoding")
    }

    /// Take an already-authenticated session through shard select to the relay.
    fn relay_key_from(server: &mut LoginServer, session: &mut LoginSession, now: Instant) -> AuthKey {
        let Response::SendThenClose(relay) = drive(
            server,
            session,
            pkt(&SelectShard {
                index: RawShardIndex(1),
            }
            .encode()),
            now,
        ) else {
            panic!("expected a relay");
        };
        AuthKey(u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]))
    }

    /// Run the whole conversation and return the auth key from the relay.
    fn relay_key(server: &mut LoginServer, now: Instant) -> AuthKey {
        let mut session = modern_session();
        assert!(matches!(
            drive(server, &mut session, pkt(&login("admin", "hunter2")), now),
            Response::Send(_)
        ));
        let Response::SendThenClose(relay) = drive(
            server,
            &mut session,
            pkt(&SelectShard {
                index: RawShardIndex(1),
            }
            .encode()),
            now,
        ) else {
            panic!("expected a relay");
        };
        AuthKey(u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]))
    }

    #[test]
    fn the_happy_path_hands_the_connection_over() {
        let mut server = server();
        let now = Instant::now();

        // Login connection.
        let mut session = modern_session();
        let Response::Send(shards) = drive(&mut server, &mut session, pkt(&login("admin", "hunter2")), now)
        else {
            panic!("expected the shard list");
        };
        assert_eq!(shards[0], 0xA8);

        let Response::SendThenClose(relay) = drive(
            &mut server,
            &mut session,
            pkt(&SelectShard {
                index: RawShardIndex(1),
            }
            .encode()),
            now,
        ) else {
            panic!("expected a relay");
        };
        assert_eq!(relay[0], 0x8C);
        let key = AuthKey(u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]));

        // Game connection: a new session, as a real client would reconnect.
        // Nothing goes back from here — the character list is the world's since
        // S5 of `docs/connection_state.md`, and the caller queues
        // `Command::Authenticated` off exactly the transition asserted below.
        let mut session = modern_session();
        let game_login = GameServerLogin {
            auth_key: key,
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        assert_eq!(
            drive(&mut server, &mut session, pkt(&game_login.encode()), now),
            Response::Idle,
            "this crate has nothing left to say on an accepted game login"
        );
        assert_eq!(
            session.account().map(|a| a.0.as_str()),
            Some("admin"),
            "and the connection is authenticated, which is what the caller acts on"
        );
        assert!(session.is_finished());
    }

    #[test]
    fn the_game_login_remembers_whose_account_it_is() {
        // Character creation arrives later on this same connection and asks the
        // session whose account to create the character on.
        let mut server = server();
        let now = Instant::now();
        let key = relay_key(&mut server, now);

        let mut session = modern_session();
        assert_eq!(session.account(), None, "nothing is known before the login");
        let _ = drive(&mut server, &mut session, pkt(&game_login(key, "admin")), now);
        assert_eq!(session.account().map(|a| a.0.as_str()), Some("admin"));
    }

    #[test]
    fn a_game_login_is_nobody_until_the_verdict_comes_back() {
        // The two-step path itself, which `drive` hides everywhere else. The
        // window between the halves is a whole argon2 long — the shard is serving
        // every other client through it, which is the entire point of S6 — and
        // this connection must be nobody for all of it.
        let mut server = server();
        let now = Instant::now();
        let key = relay_key(&mut server, now);

        let mut session = modern_session();
        let Outcome::Verify(check) = server.handle(&mut session, pkt(&game_login(key, "admin")), now) else {
            panic!("a 0x91 with a good key asks for a password check");
        };
        assert!(
            session.is_game_login(),
            "the socket is compressed from the 0x91 on, verdict or not"
        );
        assert_eq!(session.account(), None, "and nobody is logged in yet");

        assert_eq!(server.resume(&mut session, check.run()), Response::Idle);
        assert_eq!(
            session.account().map(|a| a.0.as_str()),
            Some("admin"),
            "the account comes off the state machine, which held it all along"
        );
    }

    #[test]
    fn a_verdict_nothing_asked_for_authenticates_nobody() {
        // The failure this shape has to refuse. A verdict is yes-or-no about a
        // credential and carries no identity, so a caller that brought one back to
        // the wrong connection would otherwise be letting somebody in on a
        // password check they never sat: `Matched` on a fresh session must not be
        // an account.
        let mut server = server();
        let mut session = modern_session();
        assert!(
            matches!(
                server.resume(&mut session, PasswordVerdict::Matched),
                Response::Close(_)
            ),
            "a stray verdict closes the connection rather than moving it"
        );
        assert_eq!(session.account(), None);
    }

    #[test]
    fn nothing_but_a_verdict_moves_a_session_that_is_being_checked() {
        // A second `0x80` while the first is still being hashed would put two
        // argon2 runs and two verdicts on one connection. There is no transition
        // for it — the packet is out of order like any other — so a session can
        // only ever have one check outstanding.
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        let Outcome::Verify(_check) = server.handle(&mut session, pkt(&login("admin", "hunter2")), now)
        else {
            panic!("a 0x80 asks for a password check");
        };
        assert!(matches!(
            server.handle(&mut session, pkt(&login("admin", "hunter2")), now),
            Outcome::Reply(Response::Close(_))
        ));
    }

    #[test]
    fn a_bad_password_is_refused_and_closed() {
        let mut server = server();
        let mut session = modern_session();
        let response = drive(
            &mut server,
            &mut session,
            pkt(&login("admin", "wrong")),
            Instant::now(),
        );
        assert_eq!(
            response,
            Response::SendThenClose(vec![0x82, DenyReason::BadPassword.wire_code()])
        );
    }

    #[test]
    fn a_blocked_account_hears_blocked() {
        let mut server = server();
        let mut session = modern_session();
        let response = drive(
            &mut server,
            &mut session,
            pkt(&login("banned", "x")),
            Instant::now(),
        );
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
        let unknown = drive(
            &mut server,
            &mut LoginSession::new(),
            pkt(&login("nobody", "x")),
            Instant::now(),
        );
        let bad = drive(
            &mut server,
            &mut LoginSession::new(),
            pkt(&login("admin", "x")),
            Instant::now(),
        );
        assert_ne!(unknown, bad);
    }

    #[test]
    fn selecting_a_shard_before_logging_in_is_fatal() {
        let mut server = server();
        let mut session = modern_session();
        let response = drive(
            &mut server,
            &mut session,
            pkt(&SelectShard {
                index: RawShardIndex(1),
            }
            .encode()),
            Instant::now(),
        );
        assert!(matches!(response, Response::Close(_)));
    }

    #[test]
    fn logging_in_twice_is_fatal() {
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        assert!(matches!(
            drive(&mut server, &mut session, pkt(&login("admin", "hunter2")), now),
            Response::Send(_)
        ));
        assert!(
            matches!(
                drive(&mut server, &mut session, pkt(&login("admin", "hunter2")), now),
                Response::Close(_)
            ),
            "a second 0x80 means the client lost the plot"
        );
    }

    #[test]
    fn shard_index_zero_is_refused_rather_than_underflowing() {
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        let _ = drive(&mut server, &mut session, pkt(&login("admin", "hunter2")), now);
        assert!(matches!(
            drive(
                &mut server,
                &mut session,
                pkt(&SelectShard {
                    index: RawShardIndex(0),
                }
                .encode()),
                now
            ),
            Response::Close(_)
        ));
    }

    #[test]
    fn a_shard_index_past_the_list_is_refused() {
        let mut server = server();
        let mut session = modern_session();
        let now = Instant::now();
        let _ = drive(&mut server, &mut session, pkt(&login("admin", "hunter2")), now);
        assert!(matches!(
            drive(
                &mut server,
                &mut session,
                pkt(&SelectShard {
                    index: RawShardIndex(99),
                }
                .encode()),
                now
            ),
            Response::Close(_)
        ));
    }

    #[test]
    fn the_game_port_cannot_be_reached_without_the_login_server() {
        // The whole reason to check the auth key that Sphere ignores.
        let mut server = server();
        let mut session = modern_session();
        let forged = GameServerLogin {
            auth_key: AuthKey(0xDEAD_BEEF),
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        assert_eq!(
            drive(&mut server, &mut session, pkt(&forged.encode()), Instant::now()),
            Response::SendThenClose(vec![0x82, DenyReason::BadAuthId.wire_code()]),
            "a right password with a wrong key is still refused"
        );
    }

    #[test]
    fn a_refused_game_login_is_still_a_game_socket() {
        // Sphere flips CONNECT_GAME during the crypt handshake, before the
        // password is so much as looked at, so the *refusal* goes out compressed
        // too. Send those two bytes raw and ClassicUO Huffman-decodes them into
        // garbage and shows the player nothing at all.
        //
        // This was a `game_login` flag set on the handler's first line and relied
        // on by four early returns. It is the shape of the returned state now —
        // every path out of `on_game_login` is a `Game(..)` — so a new refusal
        // cannot forget it. All three refusals, because each is its own return.
        let mut server = server();
        let now = Instant::now();

        let forged = GameServerLogin {
            auth_key: AuthKey(0xDEAD_BEEF),
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        let mut session = modern_session();
        let _ = drive(&mut server, &mut session, pkt(&forged.encode()), now);
        assert!(session.is_game_login(), "a bad key is still a game socket");
        assert_eq!(session.account(), None, "but nothing to play as");

        let key = relay_key(&mut server, now);
        let wrong_account = GameServerLogin {
            auth_key: key,
            account: RawAccountName("banned".to_owned()),
            password: RawPlaintextPassword("x".to_owned()),
        };
        let mut session = modern_session();
        let _ = drive(&mut server, &mut session, pkt(&wrong_account.encode()), now);
        assert!(
            session.is_game_login(),
            "somebody else's key is still a game socket"
        );

        let key = relay_key(&mut server, now);
        let wrong_password = GameServerLogin {
            auth_key: key,
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("wrong".to_owned()),
        };
        let mut session = modern_session();
        let _ = drive(&mut server, &mut session, pkt(&wrong_password.encode()), now);
        assert!(session.is_game_login(), "a bad password is still a game socket");
    }

    #[test]
    fn the_login_socket_never_offers_an_account_to_play_on() {
        // `0x80` verifies an account too, and it is kept — in `ShardListSent`,
        // where the relay reads it to bind the key. But `account` answers a
        // narrower question: whose character may this connection play. A `0x5D`
        // arriving on the login socket must find nothing, or a client could skip
        // the game login and enter the world over the socket that never proved it
        // held a key.
        let mut server = server();
        let mut session = modern_session();
        let Response::Send(_) = drive(
            &mut server,
            &mut session,
            pkt(&login("admin", "hunter2")),
            Instant::now(),
        ) else {
            panic!("expected the shard list");
        };
        assert_eq!(session.account(), None, "verified, but not on the game socket");
        assert!(!session.is_game_login(), "and not compressed either");
    }

    #[test]
    fn an_auth_key_cannot_be_reused() {
        let mut server = server();
        let now = Instant::now();
        let key = relay_key(&mut server, now);

        let game_login = GameServerLogin {
            auth_key: key,
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        assert_eq!(
            drive(&mut server, &mut modern_session(), pkt(&game_login.encode()), now),
            Response::Idle,
            "the first redemption is accepted"
        );
        assert_eq!(
            drive(&mut server, &mut modern_session(), pkt(&game_login.encode()), now),
            Response::SendThenClose(vec![0x82, DenyReason::BadAuthId.wire_code()]),
            "someone who read the key off the wire gets nothing"
        );
    }

    #[test]
    fn an_auth_key_belongs_to_the_account_that_earned_it() {
        // Alice selects a shard; Bob presents her key with his own credentials.
        let mut server = LoginServer::new(
            DevAccounts::new()
                .with_account(&AccountName::new("alice"), &PlaintextPassword::new("a"))
                .with_account(&AccountName::new("bob"), &PlaintextPassword::new("b")),
            "OpenShard",
            single_shard(Ipv4Addr::new(127, 0, 0, 1), 2593),
        );
        let now = Instant::now();

        let mut session = modern_session();
        let _ = drive(&mut server, &mut session, pkt(&login("alice", "a")), now);
        let Response::SendThenClose(relay) = drive(
            &mut server,
            &mut session,
            pkt(&SelectShard {
                index: RawShardIndex(1),
            }
            .encode()),
            now,
        ) else {
            panic!("expected a relay");
        };
        let alices_key = AuthKey(u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]));

        let bob = GameServerLogin {
            auth_key: alices_key,
            account: RawAccountName("bob".to_owned()),
            password: RawPlaintextPassword("b".to_owned()),
        };
        assert_eq!(
            drive(&mut server, &mut modern_session(), pkt(&bob.encode()), now),
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
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        let too_late = issued + crate::auth::DEFAULT_TTL + std::time::Duration::from_secs(1);
        assert_eq!(
            drive(
                &mut server,
                &mut modern_session(),
                pkt(&game_login.encode()),
                too_late
            ),
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
            account: RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("wrong".to_owned()),
        };
        assert_eq!(
            drive(&mut server, &mut modern_session(), pkt(&game_login.encode()), now),
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
        // rides on the key. The list itself is the world's to encode now, and the
        // version reaches it on `Command::Authenticated` — read off this session,
        // which is why this still has to be right here.
        let mut server = server();
        let now = Instant::now();

        // Connection one: the client announces a modern version in the seed.
        let mut first = LoginSession::new();
        first.on_seed(Seed {
            value: RawSeedValue(1),
            version: Some(ClientVersion::TOL),
        });
        let Response::Send(_) = drive(&mut server, &mut first, pkt(&login("admin", "hunter2")), now) else {
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

        let _ = drive(&mut server, &mut second, pkt(&game_login(key, "admin")), now);
        assert_eq!(
            second.version(),
            ClientVersion::TOL,
            "the key must carry the dialect across the gap"
        );
    }

    #[test]
    fn a_key_from_an_ancient_client_does_not_promote_it() {
        // The other direction: the key carries whatever was declared, and an old
        // client must keep getting the old shape.
        let mut server = server();
        let now = Instant::now();

        let mut first = LoginSession::new();
        first.on_seed(Seed {
            value: RawSeedValue(1),
            version: Some(ClientVersion::new(2, 0, 0, 0)),
        });
        let Response::Send(_) = drive(&mut server, &mut first, pkt(&login("admin", "hunter2")), now) else {
            panic!("expected the shard list");
        };
        let key = relay_key_from(&mut server, &mut first, now);

        let mut second = LoginSession::new();
        let _ = drive(&mut server, &mut second, pkt(&game_login(key, "admin")), now);
        assert_eq!(second.version(), ClientVersion::new(2, 0, 0, 0));
    }

    #[test]
    fn the_seed_version_shapes_the_shard_list() {
        // What this actually protects is the wiring: the version arrives in the
        // seed, and the encoder cannot ask for it. If the seed stops reaching
        // `ShardList::encode_body` every client gets whatever the default is,
        // and half of them get an address backwards.
        //
        // Which order belongs to which client is `ShardList::encode_body`'s
        // business and is pinned there. This asserts only that the two differ
        // and that the boundary is where the seed says.
        let mut server = server();
        let now = Instant::now();

        let mut modern = LoginSession::new();
        modern.on_seed(Seed {
            value: RawSeedValue(1),
            version: Some(ClientVersion::new(4, 0, 0, 0)),
        });
        let Response::Send(list) = drive(&mut server, &mut modern, pkt(&login("admin", "hunter2")), now)
        else {
            panic!("expected the shard list");
        };
        assert_eq!(&list[42..46], &[1, 0, 0, 127], "reversed since 4.0.0");

        let mut ancient = LoginSession::new();
        ancient.on_seed(Seed {
            value: RawSeedValue(1),
            version: Some(ClientVersion::new(3, 255, 255, 255)),
        });
        let Response::Send(list) = drive(&mut server, &mut ancient, pkt(&login("admin", "hunter2")), now)
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
            value: RawSeedValue(0xC0A8_0001),
            version: None,
        });
        assert_eq!(session.version(), ClientVersion::OLDEST);

        let report = ClientVersionReport {
            raw: "7.0.45.65".to_owned(),
        };
        let _ = drive(&mut server, &mut session, pkt(&report.encode()), Instant::now());
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
        let _ = drive(&mut server, &mut session, pkt(&report.encode()), Instant::now());
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
            drive(&mut server, &mut session, pkt(&report.encode()), Instant::now()),
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
                drive(&mut server, &mut session, pkt(&packet), Instant::now()),
                Response::Idle,
                "0x{:02X} must not drop the connection",
                packet[0]
            );
        }
        // And login still works afterwards.
        assert!(matches!(
            drive(
                &mut server,
                &mut session,
                pkt(&login("admin", "hunter2")),
                Instant::now()
            ),
            Response::Send(_)
        ));
    }
}
