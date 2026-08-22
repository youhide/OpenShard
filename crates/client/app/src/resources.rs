//! The client's own files, read once and held for the run: [`Resources`].
//!
//! Pulled out of [`crate::App`] because every field here answers the same
//! question — what did the install on disk say — and none of them depends on
//! where the camera is standing or what the shard has said since. See
//! `App::resources`'s own doc for why the split stops there and does not
//! reach for a getter per field.

use std::sync::Arc;

use openshard_client_render::atlas::FontAtlas;
use openshard_client_render::gump::GumpAtlas;
use openshard_client_render::hue::HueRamp;
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::NavigationGraph;
use openshard_uofiles::anim::Anim;
use openshard_uofiles::art::Art;
use openshard_uofiles::cliloc::Cliloc;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::gumpart::Gumps;
use openshard_uofiles::radarcol::RadarColors;
use openshard_uofiles::skillgrp::SkillGroups;
use openshard_uofiles::skills::Skills as SkillNames;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::TileData;
use openshard_uofiles::ttf_font::TtfFont;

/// The client's own asset files, read once at startup and held for the run.
///
/// Nothing here changes because of a packet or a camera move — see
/// [`crate::graphics::GraphicsSettings`] and [`crate::world::WorldState`] for
/// the fields that do. `App::resources` is the one place these are reached
/// from; a field here that turns out to want its own invariant gets a method
/// on this struct rather than a getter that hands the field out raw.
pub struct Resources {
    /// The facet, shared with the shard thread — see [`crate::link::connect`].
    pub map: MapSnapshot,
    /// Static long-distance connectivity over [`Resources::map`]. It is built
    /// once, before the event loop starts, and only proposes a corridor; the
    /// live route still reads the map with the shard's clutter laid over it.
    pub coarse: Option<NavigationGraph>,
    /// Static positive building space, baked from the facet's wall catalogue.
    /// Unlike `coarse`, this is presentation topology: zero means the open
    /// world and a non-zero label is the whole house in the first pass.
    pub interiors: Option<openshard_client_render::interiors::BuildingMap>,
    pub art: Art,
    /// What was measured off that art off the clock, or `None` for a run with no
    /// table beside the install — see `run`, which says which it is and carries
    /// on either way.
    ///
    /// It lives here rather than in [`crate::Atlases`] because the atlases are
    /// thrown away and rebuilt when one fills up, and a measurement of an
    /// install does not become untrue when a texture runs out of shelf space.
    pub surfaces: Option<openshard_client_render::arttable::ArtTable>,
    /// A hand edit changed a graphic's shape in [`Resources::surfaces`] since
    /// the atlases were last packed — set by `App::apply`'s `authored_prism`.
    ///
    /// The ordinary grow/evict cycle cannot see this on its own: growing only
    /// asks whether a graphic is packed *at all* (`Atlases::grow`'s own doc),
    /// so a graphic already on screen when its shape changes is never
    /// re-offered. This is the one case that has to force the full rebuild
    /// eviction otherwise waits for a full atlas to trigger.
    pub repack_forced: bool,
    pub texmaps: TexMaps,
    /// Shared with the shard thread, the same way [`Resources::map`] is — see
    /// [`crate::link::connect`]: the walk prediction weighs a pier's or a
    /// bridge's deck now, not only the land, and that needs `tiledata.mul` on
    /// both ends of the channel.
    pub tiledata: Arc<TileData>,
    /// Every multi the client ships, or `None` for an install this build could
    /// not read them from.
    ///
    /// A house is one item on the wire and a hundred statics on screen, and the
    /// expansion happens where the view becomes a draw list — see
    /// `net_command`. `None` means houses do not draw; it must **not** mean the
    /// graphic falls through to the static art, where `0x4064` is a valid id for
    /// something that is not a house.
    pub multis: Option<Arc<openshard_uofiles::multi::Multis>>,
    /// Every hue the client ships, packed once: unlike the sprite atlases it
    /// tints, nothing about it depends on where the camera is standing.
    pub hue_ramp: HueRamp,
    /// Every glyph `fonts.mul` ships, packed once for the reason `hue_ramp` is:
    /// nothing about it depends on the camera, and unlike a graphic there is no
    /// "not currently visible" character to leave unpacked.
    pub font_atlas: FontAtlas,
    /// The client's gump art, or `None` when it could not be opened — see
    /// `run`, which says so once and carries on.
    pub gumps: Option<Gumps>,
    /// The gump pictures packed so far.
    ///
    /// Grown a window at a time rather than built up front, unlike
    /// [`Resources::font_atlas`]: `gumpartLegacyMUL.uop` is 5,556 entries and a
    /// session opens a handful of them, so "the whole file" is the one thing
    /// this must not be. It lives on `App` and not on `Screen` for the reason
    /// `Screen::atlases` documents from the other side — the CPU half of an
    /// atlas builds quads and outlives any one surface.
    pub gump_atlas: GumpAtlas,
    /// The client's own text table, keyed by cliloc number — `None` when
    /// `Cliloc.enu` could not be read (missing, or a newer BWT-compressed
    /// one). A dialog whose layout named a cliloc rather than a wire line
    /// (`gump::Element::Localized`) draws nothing for that line without it,
    /// the same tolerance a missing gump art or a missing font glyph gets —
    /// see `gump::Dialogs::lines`.
    pub cliloc: Option<Cliloc>,
    /// The operator-supplied TrueType face, when `run` was asked to draw
    /// through one instead — `None` is the ordinary, `fonts.mul`-only run. Held
    /// here rather than only on `Screen` because it does not depend on a
    /// window existing: it is what `Screen::ttf_atlas` is grown from, every
    /// frame `App::draw` sees new characters in what is being said.
    pub ttf_font: Option<TtfFont>,
    /// The animations, open but not read: `anim.mul` is 195MB and frames come
    /// out of it a body at a time. `&mut` because reading one seeks the file.
    pub anim: Anim,
    /// What a worn item's own graphic resolves to for drawing — see
    /// [`EquipConv`]. Read once at startup like [`Resources::hue_ramp`]: unlike
    /// `anim`, the whole table is small enough to hold rather than seek into.
    pub equip_conv: EquipConv,
    /// What the client's own files call each skill, and which heading each one
    /// is filed under: the two tables the skill window's rows are built from.
    ///
    /// Read at startup and held, like [`Resources::equip_conv`] and unlike
    /// [`Resources::anim`]: fifty-eight names and fifty-eight group numbers are
    /// under a kilobyte between them, and the window asks for all of both every
    /// frame it is open.
    pub skill_names: SkillNames,
    /// Which heading each skill is under — see [`Resources::skill_names`].
    pub skill_groups: SkillGroups,
    /// The one colour a land or static graphic draws as on the radar/minimap,
    /// or `None` for an install this build could not read `radarcol.mul`
    /// from — see `client/render/src/radar.rs`'s module doc. `None` means the
    /// minimap draws no terrain, the same tolerance a missing `gumps` or
    /// `cliloc` table gets.
    pub radar_colors: Option<RadarColors>,
}
