//! The world: one facet of it, and which published version of it you hold.
//!
//! **This crate is the map.** Every reader in the workspace — the renderer, the
//! step check, the pathfinder, the building flood, the minimap — reads a [`Map`]
//! from here, and there is no second representation of the world anywhere else.
//!
//! [`Map`]: crate::map::Map
//!
//! It reads no files. UO's own `map*.mul` is *an* importer, not the source: it
//! lives in `openshard_uofiles::map`, hands back one of these, and is the only
//! thing in the workspace that has heard of a `.mul`. A world that never came
//! from an install is the point of the split, and it is what
//! `docs/map/new_map_representation/` is building towards.
//!
//! Five modules, and the order they depend on each other in:
//!
//! - [`grid`] — the land, and the one type that owns the block order it is in.
//! - [`map`] — one facet: that land, and everything standing on it.
//! - [`snapshot`] — which facet a map is, and which published revision. What an
//!   owner holds, so that a reader can never be looking at half a change.
//! - [`chunk`] — the square the world is stored, cached, invalidated and
//!   transferred in, cut out of a [`Map`] and assembled back into one.
//! - [`codec`] — that square as canonical bytes, and bytes back into one.
//!
//! Bytes are not a file: nothing here opens one, and where a base set lives on
//! disk is a caller's business. No patches and no publisher yet — see
//! `docs/map/new_map_representation/plan.md`.

pub mod chunk;
pub mod codec;
pub mod grid;
pub mod map;
pub mod snapshot;
