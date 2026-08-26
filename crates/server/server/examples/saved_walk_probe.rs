//! Ask the production world authority whether a saved character may take one step.
//!
//! This restores the configured map and persistence layers, enters the saved
//! character, and feeds one `0x02`-equivalent command through `World::tick`. It
//! never queues a save.

use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use openshard_events::Cursor;
use openshard_gateway::ConnectionId;
use openshard_protocol::access::AccessLevel;
use openshard_protocol::direction::Facing;
use openshard_protocol::identity::{AccountName, CharacterName};
use openshard_protocol::version::ClientVersion;
use openshard_protocol::world::{RawFastwalkKey, RawStepSequence, WalkRequest};
use openshard_server::boot::{load_config, load_world, open_store, restore_saved_world};
use openshard_world::{Character, Command, Entering, MobileMoved, PlayerEntered, PlayerRefused, StepRefused};

#[derive(Debug, Parser)]
#[command(about = "Probe one real server step from a saved character")]
struct Cli {
    /// Shard configuration whose map and persistence store are restored.
    #[arg(long, default_value = "openshard.toml")]
    config: String,

    /// Saved character to enter.
    #[arg(long, default_value = "Lord British")]
    character: String,

    /// Restrict the character lookup to this account.
    #[arg(long)]
    account: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("saved walk probe failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = load_config(&cli.config)?;
    let store = open_store(&config).await?;

    let mut matches = store
        .characters()
        .await?
        .into_iter()
        .filter(|record| record.name.0.eq_ignore_ascii_case(&cli.character))
        .filter(|record| {
            cli.account
                .as_ref()
                .is_none_or(|account| record.account.0.eq_ignore_ascii_case(account))
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(format!("no saved character named {:?}", cli.character).into());
    }
    if matches.len() != 1 {
        let accounts = matches
            .iter()
            .map(|record| record.account.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "character {:?} exists on more than one account ({accounts}); pass --account",
            cli.character
        )
        .into());
    }
    let saved = matches.pop().expect("one match was checked");
    let facing = Facing::from_bits(saved.facing);
    let access = config
        .accounts
        .iter()
        .find(|account| account.name.normalized() == saved.account.normalized())
        .and_then(|account| account.access.0.parse::<AccessLevel>().ok())
        .unwrap_or(AccessLevel::Player);

    println!("config: {}", cli.config);
    println!("store:  {}", config.persistence.database);
    println!(
        "saved:  {:?} {} / {} at ({}, {}, {}), facing {}",
        saved.serial, saved.account, saved.name, saved.x, saved.y, saved.z, facing
    );

    let world = load_world(&config)?;
    let mut world = restore_saved_world(&store, &config, world).await;
    let connection = ConnectionId::from_raw(u64::MAX);
    let now = Instant::now();
    let mut entered: Cursor<PlayerEntered> = world.bus().cursor_at_end();
    let mut entry_refused: Cursor<PlayerRefused> = world.bus().cursor_at_end();
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName::new(&saved.account.0),
        name: CharacterName::new(&saved.name.0),
        access,
        character: Character::Saved,
    }));
    world.tick(now);

    if let Some(refused) = world
        .bus()
        .read(&mut entry_refused)
        .find(|event| event.connection == connection)
    {
        return Err(format!("the restored world refused entry: {:?}", refused.reason).into());
    }
    let arrival = world
        .bus()
        .read(&mut entered)
        .find(|event| event.connection == connection)
        .copied()
        .ok_or("the restored world neither entered nor explicitly refused the character")?;
    println!(
        "entered: {:?} at {:?} (access {:?})",
        arrival.serial, arrival.position, access
    );
    let _ = world.drain_outbound().count();

    let mut moved: Cursor<MobileMoved> = world.bus().cursor_at_end();
    let mut refused: Cursor<StepRefused> = world.bus().cursor_at_end();
    world.queue(Command::Walk {
        connection,
        request: WalkRequest {
            facing,
            sequence: RawStepSequence(0),
            fastwalk_key: RawFastwalkKey(0),
        },
    });
    world.tick(now);

    let packet_ids = world
        .drain_outbound()
        .filter(|out| out.connection == connection)
        .filter_map(|out| out.packet.first().copied())
        .collect::<Vec<_>>();
    let movement = world
        .bus()
        .read(&mut moved)
        .find(|event| event.serial == saved.serial)
        .copied();
    let refusal = world
        .bus()
        .read(&mut refused)
        .find(|event| event.serial == saved.serial)
        .copied();

    println!(
        "step:   {:?} from {:?}, packets [{}]",
        facing,
        arrival.position,
        packet_ids
            .iter()
            .map(|id| format!("0x{id:02x}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(movement) = movement {
        println!(
            "result: WalkAck; server moved {:?} -> {:?}",
            movement.from, movement.to
        );
        if packet_ids.contains(&0x22) && !packet_ids.contains(&0x21) {
            return Ok(());
        }
        return Err("movement event and outbound walk response disagree".into());
    }
    if let Some(refusal) = refusal {
        return Err(format!("WalkReject: {:?}", refusal.reason).into());
    }
    if packet_ids.contains(&0x21) {
        return Err("WalkReject before movement authority (frozen or out of stamina)".into());
    }
    Err("the step produced neither movement nor a walk response".into())
}
