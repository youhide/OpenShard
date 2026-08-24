//! The simulation: game loop, spatial index, and composition of every gameplay system.
//!
//! # What is here
//!
//! The tick, the components a character is made of, the sector grid that
//! answers "what is near this point", and the client's map files.
//!
//! [`World::tick`] is the deterministic half of the boundary the gateway's
//! channel draws: commands queue in from network tasks, are applied in a fixed
//! order at a fixed rate, and packets come out. Nothing inside it awaits, reads
//! a clock, or touches a socket.
//!
//! The gameplay systems are not written yet.
//!
//! # The client's files are the source of truth
//!
//! The server does not send map tiles — the client already has them, and has had
//! them since it was installed. What the server needs the map for is *deciding*:
//! how high the ground is, what blocks, what floats. If the two disagree the
//! client draws a wall the server lets you walk through, and the player watches
//! themselves rubber-band.
//!
//! So these parsers are not "reading a file format", they are agreeing with a
//! binary from 1997 about the shape of Britannia. Two things in them are not
//! stated anywhere in the files and will silently produce a plausible, wrong
//! world if guessed:
//!
//! - **Block order is column-major** — `bx * (height/8) + by`. See [`map`].
//! - **`tiledata.mul` has two layouts** and no version field. See [`tiledata`].
//!
//! Both are settled by arithmetic and pinned by tests against real files.

pub mod admin;
pub mod decoration;
mod doorgen;
pub mod events;
pub mod gm;
pub mod loot;
pub mod mapedit;
pub mod spawner;
pub mod terrain;
pub mod tick;
pub mod townsfolk;

// Components, the spatial index and the generator moved down into
// `openshard-state` so the gameplay systems can live in their own crates above
// it. Re-exported here so `openshard_world::Position` and friends keep resolving.
pub use events::{
    AdminMenuAction, CorpseCreated, GumpAnswered, MobileMoved, MobileRestored, MobileSpawned, MobileTurned,
    PlayerEntered, PlayerLeaving, PlayerLeft, PlayerRefused, RefusedEntry, RefusedReason, SpellRequested,
    StepRefused,
};
pub use openshard_chat::MobileSpoke;
pub use openshard_combat::{MobileDamaged, MobileDied};
pub use openshard_items::{ItemSpawned, ItemUsed, ItemsTaken, MobileUsed};
pub use openshard_magic::SpellCast;
pub use openshard_npc::StockLine;
pub use openshard_skills::{SkillChanged, SkillRequested, SkillUsed};
pub use openshard_state::Outbound;
pub use openshard_state::components;
pub use openshard_state::{
    Account, Amount, Body, Brain, Client, Combat, Contained, Container, CriminalUntil, DamageType, Decays,
    Drawn, Equipped, Heading, Hitpoints, Mana, MeleeDamage, Movement, MurderDecay, Murders, Name, Position,
    Resistance, Skills, Stackable, Stats, SwingSpeed,
};
pub use openshard_state::{CastStyle, Gameplay, StatLock, TooltipMode};
pub use openshard_state::{Dialogue, SpeechEntry, SpeechTable};
pub use openshard_state::{ObjectiveDef, ObjectiveKind, QuestDef, RewardDef, RewardKind};
pub use openshard_state::{Region, RegionFlags, RegionId, RegionRect};
pub use openshard_state::{SECTOR_SIZE, Sectors, VIEW_RANGE, distance, in_range, sectors};
pub use terrain::{MAX_STEP_UP, MapTerrain, PLAYER_HEIGHT};
pub use tick::{
    Appearance, Character, CharacterSheet, Command, DecorContainer, DecorDoor, Entering, FreshCharacter,
    RestoredCharacters, RestoredItems, TICK_INTERVAL, World,
};
