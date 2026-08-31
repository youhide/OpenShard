//! DefCarpentry's recipes, as core data.
//!
//! The table itself is [`data/carpentry.json`], generated once from ServUO by
//! `tools/gen-craft-tables` and edited as data from then on: edit the JSON, do
//! not regenerate it. `build.rs` turns it into the `const`s below before this
//! crate compiles, so what the gump reads is still `&'static [Recipe]`. Skills
//! are in tenths.
//!
//! [`data/carpentry.json`]: ../../../data/carpentry.json

use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_state::Skill;

use crate::recipe::{
    CraftRes,
    CraftSkillReq,
    Recipe,
    SubRes,
    SubResAxis,
};
use crate::system::{
    Needs,
    Text,
};

include!(concat!(env!("OUT_DIR"), "/carpentry.rs"));
