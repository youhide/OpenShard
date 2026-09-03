//! DefGlassblowing's recipes, as core data.
//!
//! The table itself is [`data/glassblowing.json`], written by hand from ServUO's
//! `DefGlassblowing.InitCraftList` rather than by `tools/gen-craft-tables` — it
//! is the one trade that was not in the generated set, because nothing on this
//! shard could open it. `build.rs` turns it into the `const`s below before this
//! crate compiles. Skills are in tenths.
//!
//! **The Mondain's Legacy rows only.** Upstream adds gargoyle mirrors, a
//! soulstone fragment and an empty venom vial under `Core.SA`, and a shard whose
//! harvest tables stop at Mondain's Legacy has no business shipping them.
//!
//! [`data/glassblowing.json`]: ../../../data/glassblowing.json

use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_state::Skill;

use crate::recipe::{
    CraftRes,
    CraftSkillReq,
    Recipe,
};
use crate::system::{
    Needs,
    Text,
};

include!(concat!(env!("OUT_DIR"), "/glassblowing.rs"));
