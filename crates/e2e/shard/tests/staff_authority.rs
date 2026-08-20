//! The shard tells a client what it may command, and the client believes it.
//!
//! `openshard_protocol::access::AuthorityNotice` is this engine's own `0xBF`
//! subcommand — no reference client has one — and its whole job is to let the
//! speech line stop offering staff commands to people who may not run them
//! (`openshard_commands::StaffCommand::matching`). Both halves are tested where
//! they live: the packet round-trips in `openshard-protocol`, the filter in
//! `openshard-commands`, and the completer in `openshard-client-app`.
//!
//! What none of those can catch is the seam this file is about — that the shard
//! actually *sends* it, before the client is let into the world, and that the
//! level which comes out at the far end is the one the operator wrote in the
//! config. Every unit test on either side passed while nothing was sent at all.
//!
//! # The stock config grants nothing, on purpose
//!
//! `admin` ships with its `access` line commented out (see `default.toml`: the
//! file is written on first run with a password anyone can read), so the shard
//! [`shard`] starts has **no** staff on it. This test therefore hands the
//! authority out itself, which is also the honest shape of the question: what is
//! under test is that what an operator wrote reaches the client's completer, and
//! an account that was staff by accident would prove nothing about that.

use std::net::SocketAddr;
use std::time::Duration;

use openshard_client_net::transport::enter_world;
use openshard_commands::StaffCommand;
use openshard_config::{Config, RawAccessLevel};
use openshard_protocol::access::AccessLevel;

use openshard_e2e_shard::{ACCOUNT, NYSTUL, WITNESS, plan, plan_for, spawn, stock_config, version};

/// The stock config with `admin` promoted, the way an operator does it by
/// uncommenting one line.
fn config_with_a_game_master(address: SocketAddr) -> Config {
    let mut config = stock_config(address);
    let account = config
        .accounts
        .iter_mut()
        .find(|account| account.name == ACCOUNT)
        .expect("the stock config ships the admin account");
    account.access = RawAccessLevel("administrator".to_owned());
    config
}

#[tokio::test]
async fn a_staff_account_is_told_it_may_command() {
    // Held for the length of the test: dropping the handle stops the shard.
    let (address, _shard) = spawn(config_with_a_game_master);

    // A bound rather than a wait for ever: a mismatch in this conversation hangs,
    // it does not error — `enter_world`'s own tests make the same argument.
    let entered = tokio::time::timeout(Duration::from_secs(20), enter_world(address, plan(), version()))
        .await
        .expect("the login conversation finished inside the timeout");
    let (_socket, view) = entered.expect("the client reached the world");

    assert_eq!(
        view.authority,
        AccessLevel::Administrator,
        "the level the config gave the account has to survive the login, the \
         hand-off to the world, and the wire"
    );
    // The consequence, which is the only reason the packet exists: this client's
    // speech line has something to offer when a `.` is typed.
    assert!(
        !StaffCommand::matching("", view.authority).is_empty(),
        "an administrator is offered the vocabulary"
    );
}

/// The other half, on the same shard as the promotion above: an account the
/// operator said nothing about arrives as a player.
///
/// A notice that was never sent looks exactly like this one — which is why the
/// test above is what makes this one mean anything. Both are needed, and both
/// have to be on a shard where the *other* answer is possible.
#[tokio::test]
async fn an_ordinary_account_is_told_nothing_it_could_command_with() {
    let (address, _shard) = spawn(config_with_a_game_master);

    let entered = tokio::time::timeout(
        Duration::from_secs(20),
        enter_world(address, plan_for(WITNESS, NYSTUL), version()),
    )
    .await
    .expect("the login conversation finished inside the timeout");
    let (_socket, view) = entered.expect("the client reached the world");

    assert_eq!(view.authority, AccessLevel::Player);
    assert!(
        StaffCommand::matching("", view.authority).is_empty(),
        "and is offered no staff command at all"
    );
}
