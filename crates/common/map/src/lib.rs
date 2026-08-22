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
//! Three modules, and the order they depend on each other in:
//!
//! - [`grid`] — the land, and the one type that owns the block order it is in.
//! - [`map`] — one facet: that land, and everything standing on it.
//! - [`snapshot`] — which facet a map is, and which published revision. What an
//!   owner holds, so that a reader can never be looking at half a change.
//!
//! No format, no patches, no publisher yet. See
//! `docs/map/new_map_representation/snapshot.md` for why a revision that cannot
//! yet change is still worth carrying.

pub mod grid;
pub mod map;
pub mod snapshot;
