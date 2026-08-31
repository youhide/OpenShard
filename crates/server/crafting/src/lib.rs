//! Making things: the craft systems, their recipes, and the window that drives
//! them.
//!
//! A port of ServUO's `Scripts/Services/Craft/` — `CraftSystem`, `CraftItem`,
//! `CraftRes`/`CraftSubRes` and `CraftGump` — as a gameplay system in the shape
//! every other one has: `fn(&mut WorldState)`, its own domain event, no calls to
//! a peer. It is what the harvest slice was the pillar for; until it landed,
//! nothing in the engine consumed a raw material.
//!
//! **The recipes are core data, like [`openshard_magic`]'s spells and
//! [`openshard_state::weapon`]'s speeds.** A bare shard has to be able to forge,
//! so the 492 shipped rows live in [`defs`] rather than in a pack, generated
//! once from ServUO by `tools/gen-craft-tables` and ordinary source from then on.
//! What a pack customises, it customises off [`ItemCrafted`] — the split skills,
//! magic and loot already use.
//!
//! The way in is a **double-click on the tool**, through the same `ItemUsed` seam
//! the bandage, the lockpick and the pickaxe come through. There is no craft
//! packet: the client sends an ordinary use, and everything after that is a gump.
//!
//! [`openshard_magic`]: https://docs.rs/openshard-magic

pub mod chance;
pub mod consume;
pub mod craft;
pub mod defs;
pub mod environment;
pub mod gump;
pub mod recipe;
pub mod smelt;
pub mod system;

pub use chance::{
    Chance,
    Roll,
    chance,
};
pub use consume::{
    Materials,
    Refusal,
    Share,
};
pub use craft::{
    ItemCrafted,
    advance_crafts,
    begin,
    tool_system,
    tool_system_for_kind,
};
pub use defs::{
    SYSTEMS,
    system,
};
pub use environment::{
    Facilities,
    around,
};
pub use gump::{
    CRAFT_GUMP,
    close,
    handle,
    open,
    open_catalogue,
    owns,
};
pub use recipe::{
    CraftRes,
    CraftSkillReq,
    Recipe,
    SubRes,
    SubResAxis,
};
pub use smelt::{
    INGOT_GRAPHIC,
    smelt,
};
pub use system::{
    CraftSystemDef,
    Eca,
    Needs,
    SystemId,
    Text,
};
