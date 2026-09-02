//! Fixtures shared by this crate's unit tests.
//!
//! What lives here is the one thing several test modules cannot build for
//! themselves: a [`Session`] that has actually been through a login. Nothing
//! shorter reaches one — a game socket and the account behind it are both
//! decided inside `LoginSession`'s state machine, and the only way in is the
//! real conversation. Writing that conversation out per test module is how the
//! copies drift apart, so it is written once.

use std::net::Ipv4Addr;
use std::time::Instant;

use openshard_gateway::{
    OutboxRx,
    outbox_channel,
    version_channel,
};
use openshard_login::{
    DevAccounts,
    LoginServer,
    LoginSession,
    Outcome,
    Response,
    single_shard,
};
use openshard_protocol::identity::{
    AccountName,
    CharacterName,
    PlaintextPassword,
    RawAccountName,
    RawPlaintextPassword,
};
use openshard_protocol::login::{
    AccountLogin,
    GameServerLogin,
    LoginStagePacket,
    RawShardIndex,
    SelectShard,
};
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::AuthKey;

use crate::session::Session;

/// The account every fixture logs in as.
pub(crate) fn admin() -> AccountName {
    AccountName::new("admin")
}

/// The one character that account starts with.
pub(crate) fn lord_british() -> CharacterName {
    CharacterName::new("Lord British")
}

/// A login server holding [`admin`]. Its characters are not here — which
/// characters an account has is the world's roster since S5 of
/// `docs/server/evidence/2026-07-30-the-connection-state-machine.md`, and an
/// account store that still answered would be the second list that whole sweep
/// deletes.
pub(crate) fn login_server() -> LoginServer {
    LoginServer::new(
        DevAccounts::new().with_account(&admin(), &PlaintextPassword::new("hunter2")),
        "OpenShard",
        single_shard(Ipv4Addr::LOCALHOST, 2593),
    )
}

/// Decode a fixture the way `parse_packet` does, so tests build packets the same
/// way `LoginServer::handle`'s only real caller does.
fn pkt(bytes: &[u8]) -> LoginStagePacket {
    LoginStagePacket::decode(bytes, ClientVersion::OLDEST).expect("a valid fixture encoding")
}

/// Handle a login packet, running the password check on this thread.
///
/// The shard hands it to a blocking task and picks the verdict up on a channel —
/// argon2 is most of a tick, see `crate::verify` — but a fixture has no loop to
/// stall and no channel to wait on, and what it wants is the conversation's end
/// state.
fn drive(
    login: &mut LoginServer,
    session: &mut LoginSession,
    packet: LoginStagePacket,
    now: Instant,
) -> Response {
    match login.handle(session, packet, now) {
        Outcome::Reply(response) => response,
        Outcome::Verify(check) => login.resume(session, check.run()),
    }
}

/// Take a connection all the way to the character screen the way a real client
/// does: `0x80` and `0xA0` on the login socket, then `0x91` on a second one
/// carrying the key the relay handed out.
///
/// The returned session is a *game* socket — compressed, and authenticated to
/// [`admin`]. The returned outbox is empty: `LoginServer::handle` gives back a
/// `Response` and it is `Session::apply` that writes, which this does not call,
/// so the first thing on that channel is whatever the test provokes.
pub(crate) fn at_character_screen(login: &mut LoginServer, now: Instant) -> (Session, OutboxRx) {
    let mut auth = LoginSession::new();
    let account_login = AccountLogin {
        account:  RawAccountName::new("admin"),
        password: RawPlaintextPassword::new("hunter2"),
    };
    let Response::Send(_) = drive(login, &mut auth, pkt(&account_login.encode()), now) else {
        panic!("expected the shard list");
    };
    let Response::SendThenClose(relay) = drive(
        login,
        &mut auth,
        pkt(&SelectShard {
            index: RawShardIndex(1),
        }
        .encode()),
        now,
    ) else {
        panic!("expected a relay");
    };
    let key = AuthKey(u32::from_be_bytes([relay[7], relay[8], relay[9], relay[10]]));

    let (outbox, wire) = outbox_channel();
    let (control, _control) = version_channel();
    let mut session = Session::new(outbox, control);
    let game_login = GameServerLogin {
        auth_key: key,
        account:  RawAccountName::new("admin"),
        password: RawPlaintextPassword::new("hunter2"),
    };
    assert_eq!(
        drive(login, &mut session.login, pkt(&game_login.encode()), now),
        Response::Idle,
        "the login crate ends here: the character list comes out of a tick"
    );
    assert_eq!(
        session.login.account(),
        Some(&admin()),
        "the fixture is authenticated"
    );
    assert!(session.login.is_game_login(), "and it is a game socket");
    (session, wire)
}
