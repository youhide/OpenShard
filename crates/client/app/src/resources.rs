//! The client's own files, read once and held for the run: [`Resources`].
//!
//! Pulled out of [`crate::App`] because every field here answers the same
//! question — what did the install on disk say — and none of them depends on
//! where the camera is standing or what the shard has said since. See
//! `App::resources`'s own doc for why the split stops there and does not
//! reach for a getter per field.

use std::path::PathBuf;
use std::sync::Arc;

use openshard_client_render::atlas::FontAtlas;
use openshard_client_render::gump::GumpAtlas;
use openshard_client_render::hue::HueRamp;
use openshard_map::map::WorldMap;
use openshard_movement::NavigationGraph;
use openshard_movement::ground::Ground;
use openshard_tiles::TileData;
use openshard_uofiles::anim::Anim;
use openshard_uofiles::art::Art;
use openshard_uofiles::cliloc::Cliloc;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::gumpart::Gumps;
use openshard_uofiles::radarcol::RadarColors;
use openshard_uofiles::skillgrp::SkillGroups;
use openshard_uofiles::skills::Skills as SkillNames;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::ttf_font::TtfFont;

/// The client's own asset files, read once at startup and held for the run.
///
/// Nothing here changes because of a camera move — see
/// [`crate::graphics::GraphicsSettings`] and [`crate::world::WorldState`] for
/// the fields that do. `App::resources` is the one place these are reached
/// from; a field here that turns out to want its own invariant gets a method
/// on this struct rather than a getter that hands the field out raw.
///
/// **One field here does change on a packet**, and deliberately:
/// [`Resources::ground`]'s live layer, which is what the shard has put on the
/// ground. It is here because the ground is here — the two are one facet, and
/// splitting them across two structs is exactly the arrangement
/// `docs/map/realtime_map.md`'s era R exists to end. See
/// [`crate::clutter::project`], which is the only writer — and which writes the
/// bodies standing on that ground in the same call, because refreshing one
/// without the other is a step decided against two different moments.
impl Resources {
    /// Whether this client has ground under it yet.
    ///
    /// **The invariant [`map`](Self::map) and [`terrain`](Self::terrain) are
    /// stated in terms of**, and it became a real question at
    /// `to_the_client.md`'s E2: a client whose world comes over the connection
    /// has a window, a shard and every one of its own files before it has a
    /// facet. Under the other two [`WorldSource`](crate::WorldSource) arms this
    /// is true from the first line of `run` and never becomes false again.
    ///
    /// It is checked at the two doors a world question can come through —
    /// [`App::draw`](crate::App::draw) for the frame and
    /// `App::window_event` for the mouse and the keyboard — and at the
    /// one place a *packet* asks one, which is `App::cutaway`. Everything
    /// downstream of those three is inside a frame or inside an event, and can
    /// read the map without asking again.
    ///
    /// And at one door that is none of those: `App::create_window`, which packs
    /// the atlases for the frame that has not happened yet. It is the odd one
    /// out because it runs *once*, before any frame and before any event, so
    /// nothing above it has had the chance to ask — and it read the map to know
    /// what to pack. It packs nothing when there is no ground, and leaves
    /// `covered` unset so the first frame that has one packs the lot.
    #[must_use]
    pub fn grounded(&self) -> bool {
        self.ground.snapshot().is_some()
    }

    /// The ground this client is drawing.
    ///
    /// A method rather than a field because of the shape of [`Ground`]: its base
    /// is optional, for a shard that runs with no client files at all and — since
    /// E2 — for a client whose facet has not arrived yet. Forty readers below
    /// would otherwise carry the same `expect`, and each of them would be
    /// answering a question that has exactly one answer at the door they came
    /// through.
    ///
    /// # Panics
    ///
    /// If there is no ground. Reachable now, and held off by
    /// [`grounded`](Self::grounded) rather than by the shape of `run`: the frame
    /// and the window's events are gated on it, and the one packet that reads
    /// the map asks for itself. A caller reached from somewhere that is neither
    /// is a caller that has to say why it is safe.
    #[must_use]
    pub fn map(&self) -> &WorldMap {
        self.ground
            .snapshot()
            .expect("a client that got as far as drawing has been given a facet")
            .map()
    }

    /// The same ground read through the table that says what its graphics are,
    /// and the bake over the pair — which is what the interiors bake takes.
    ///
    /// The bake is [`Ground`]'s own, taken in the same statement the facet was:
    /// at startup for a world off the disk, and at
    /// [`Ground::set_base`](openshard_movement::ground::Ground::set_base) for one
    /// off the wire. It used to be built twice more inside `interiors.rs`, once
    /// per bake, because the three tables had no name to travel under;
    /// `MapTerrain` is that name.
    ///
    /// # Panics
    ///
    /// If there is no ground, and for [`map`](Self::map)'s reason.
    #[must_use]
    pub fn terrain(&self) -> openshard_movement::MapTerrain<'_> {
        self.ground
            .terrain(&self.tiledata)
            .expect("a client that got as far as drawing has been given a facet")
    }
}

pub struct Resources {
    /// The client install everything here was read from.
    ///
    /// Kept because two questions outlive the reading: `tiledata.mul` is an
    /// input to every artifact's stamp — a graph built under one tile table is
    /// not valid under another — and a world that arrives *after* startup has
    /// to be able to ask both. Before that world existed, every reader had `dir`
    /// on the stack in `run` and none of them needed it again.
    pub dir: PathBuf,
    /// The file this facet's ground came out of — a base set of ours, or the
    /// one a client keeps of the world a shard handed it — and `None` for a
    /// facet read out of the install's own `map*` files.
    ///
    /// What it is for is baking: an artifact is stamped against the world it was
    /// built from, and that world has to be a file for the stamp to name. It
    /// outlives startup because a rebake can be asked for at any time, and
    /// because under `WorldSource::Shard` there is no world at startup to have
    /// remembered one from.
    pub world_file: Option<PathBuf>,
    /// The facet: the ground read off the install, what the shard has laid over
    /// it, and where a body may stand on the two. Its base is shared with the
    /// shard thread — see [`crate::link::connect`].
    ///
    /// The live layer is rebuilt whole from the view whenever the view changes
    /// and is never diffed; this end has no identities to address a finer edit
    /// to. See [`crate::clutter::fill`].
    ///
    /// The span bake used to sit in a field of its own beside this one, with a
    /// comment on each saying the two had to agree; it is inside
    /// [`Ground`](openshard_movement::ground::Ground) now, which is that comment
    /// made into a value.
    pub ground: Ground,
    /// Static long-distance connectivity over [`Resources::ground`]. It is built
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
    /// Shared with the shard thread, the same way [`Resources::ground`]'s base is — see
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
