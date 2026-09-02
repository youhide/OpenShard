//! DefInscription's recipes, as core data.
//!
//! The table itself is [`data/inscription.json`], generated once from ServUO by
//! `tools/gen-craft-tables` and edited as data from then on: edit the JSON, do
//! not regenerate it. `build.rs` turns it into the `const`s below before this
//! crate compiles, so what the gump reads is still `&'static [Recipe]`. Skills
//! are in tenths.
//!
//! Two things here are unlike every other trade, and both are in the rows rather
//! than in the header: a scroll row costs [`mana`](crate::recipe::Recipe::mana)
//! as well as reagents, and it is refused unless the scribe's own spellbook
//! holds the spell being written down. The first is data; the second is derived
//! from the row's own art, because a Magery scroll *is* `0x1F2D + spell`.
//!
//! [`data/inscription.json`]: ../../../data/inscription.json

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

include!(concat!(env!("OUT_DIR"), "/inscription.rs"));
