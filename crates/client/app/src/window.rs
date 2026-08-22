//! The window and its GPU surface: what a run needs before it can draw a
//! single frame, and nothing about what gets drawn on it.
//!
//! [`StartupError`] is why opening one can fail, [`Atlases`] and [`Wanted`]
//! are the art a frame's atlases are grown from, and [`Screen`] is
//! everything built once a window exists — the surface, the device, every
//! render pass. [`App::create_window`] is the one place all of it comes
//! together; [`App::wanted_now`], [`App::wanted_since`] and
//! [`App::wanted_in`] are the questions [`Screen::atlases`] is grown to
//! answer.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use openshard_client_render::animate::StaticAnimations;
use openshard_client_render::atlas::{
    AnimAtlas, AnimationKey, AtlasError, LandAtlas, StaticAtlas, StaticAtlasPages, TexmapAtlas, TtfAtlas,
};
use openshard_client_render::blit::{self, Blit};
use openshard_client_render::camera::{Camera, TileBounds};
use openshard_client_render::composite::{
    COMPOSITE_SOURCE_SIDE, CompositeCache, CompositeKey, CompositeQuarantineReason, CompositeRenderer,
    FlatGroundBlock,
};
use openshard_client_render::gbuffer::Gbuffer;
use openshard_client_render::gump::GumpRenderer;
use openshard_client_render::hue::HueRamp;
use openshard_client_render::items::{self, GroundItem};
use openshard_client_render::mobiles::{self, Mobile};
use openshard_client_render::outline::{self, Outline};
use openshard_client_render::radar_pass::{
    RADAR_CHUNK_CACHE_BUDGET, RadarChunkRenderer, RadarOverlayRenderer, radar_chunk_array_layers,
};
use openshard_client_render::renderer::{self, GroundRenderer, MeshFaceRenderer, SpriteRenderer};
use openshard_client_render::select::Select;
use openshard_client_render::solids::SolidsRenderer;
use openshard_client_render::{ground, light, statics};
use openshard_protocol::wire::Graphic;
use openshard_uofiles::anim::Anim;
use openshard_uofiles::art::Art;
use openshard_uofiles::equipconv::EquipConv;
use openshard_uofiles::map::Map;
use openshard_uofiles::texmaps::TexMaps;
use openshard_uofiles::tiledata::TileData;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::app::App;
use crate::crowd::Who;
use crate::{desk, graphics, profile, resources, shell, world};

/// Why the client could not start.
///
/// A binary can afford to print and exit, but the reasons are still types: a
/// `String` error loses which of these happened the moment it is formatted, and
/// "no GPU" and "no client files" want different answers from whoever hits them.
#[derive(Debug)]
pub(crate) enum StartupError {
    /// No window could be created.
    Window(winit::error::OsError),
    /// The window has no surface wgpu can draw to.
    Surface(wgpu::CreateSurfaceError),
    /// No adapter, or no device from it.
    NoDevice(String),
    /// The surface offers only sRGB formats, which would change the art's
    /// colours on their way to the screen.
    OnlySrgb,
    /// The land art would not pack.
    Atlas(openshard_client_render::atlas::AtlasError),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Window(source) => write!(f, "creating a window: {source}"),
            Self::Surface(source) => write!(f, "creating a surface: {source}"),
            Self::NoDevice(detail) => write!(f, "no GPU to draw with: {detail}"),
            Self::OnlySrgb => write!(
                f,
                "this surface offers only sRGB formats, which would alter the art's colours",
            ),
            Self::Atlas(source) => write!(f, "packing land art: {source}"),
        }
    }
}

/// Every picture a frame can sample, packed together.
///
/// One value rather than four fields because they are grown together and used
/// together: a frame drawn from a land atlas of one camera and a static atlas
/// of another is a frame with things standing on ground that is not there.
///
/// # They grow; they are not rebuilt
///
/// An atlas used to be thrown away and packed again the moment the camera asked
/// for a graphic it did not hold, which is a full re-read of the art plus three
/// new pipelines — during a scroll, every few tiles, because a scroll is exactly
/// what keeps introducing graphics. Now [`Atlases::grow`] adds what is new to
/// what is already there and [`Atlases::upload`] sends the rows that changed.
///
/// The rebuild survives as the answer to *full* — see [`Atlases::grow`]'s note —
/// which is the one thing growing cannot do for itself.
pub(crate) struct Atlases {
    pub(crate) land: LandAtlas,
    pub(crate) texmaps: TexmapAtlas,
    /// Bounded immutable pages. `StaticAtlas` remains available to tests and
    /// embedders that deliberately select the one-page baseline.
    pub(crate) statics: StaticAtlasPages,
    pub(crate) mobiles: AnimAtlas,
}

/// Atlas work paid by one frame.
///
/// Kept separate from the renderer's timing: an upload can be cheap while the
/// CPU packing that preceded it is not, and an overflow is the diagnosis that
/// makes a one-off long atlas phase actionable in the jank trace.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AtlasWork {
    /// Bytes submitted to atlas textures this frame. This counts the dirty row
    /// bands, including their deliberately conservative over-coverage.
    pub(crate) uploaded_bytes: u64,
    /// The atlas that ran out of room, when this frame had to rebuild it.
    pub(crate) overflow: Option<AtlasOverflow>,
}

/// The state at the point a growing atlas could no longer accept graphics.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AtlasOverflow {
    pub(crate) atlas: &'static str,
    pub(crate) packed_graphics: usize,
    pub(crate) newly_requested_graphics: usize,
}

/// What a frame wants packed, gathered before anything is read from disk.
///
/// Three sets rather than three arguments, because they travel together
/// everywhere and two of them are keyed by numbers that look alike: a land
/// graphic and a static graphic are both a `Graphic` and are different index
/// spaces, which is a mistake a positional argument list would accept in
/// silence.
#[derive(Default)]
pub(crate) struct Wanted {
    /// Land graphics, which feed the land atlas and the texture atlas both.
    pub(crate) land: BTreeSet<Graphic>,
    /// Static graphics: what the map has standing on the ground, and what the
    /// server has dropped on top of it.
    pub(crate) statics: BTreeSet<Graphic>,
    /// Body, group and stored direction for everyone on screen.
    pub(crate) animations: BTreeSet<AnimationKey>,
}

impl Atlases {
    /// Pack a set from nothing.
    ///
    /// The startup path, and the recovery path: an atlas that has filled up is
    /// replaced by one built for what is on screen *now*, which is where the
    /// eviction lives. Growing has no other way to reclaim a graphic the camera
    /// walked away from ten minutes ago, and rebuilding used to do it by
    /// accident on every miss.
    pub(crate) fn build(
        art: &Art,
        surfaces: Option<&openshard_client_render::arttable::ArtTable>,
        texmaps: &TexMaps,
        tiledata: &TileData,
        anim: &mut Anim,
        wanted: &Wanted,
    ) -> Result<Self, AtlasError> {
        Ok(Self {
            land: LandAtlas::build(art, wanted.land.iter().copied())?,
            texmaps: TexmapAtlas::build(texmaps, tiledata, wanted.land.iter().copied())?,
            // The table is cloned into the atlas rather than borrowed: an atlas
            // outlives the frame it was built in and packs more art on every
            // scroll, so it has to keep what it reads a graphic's surface out of.
            statics: StaticAtlasPages::build_from(art, wanted.statics.iter().copied(), surfaces.cloned())?,
            mobiles: AnimAtlas::build(anim, wanted.animations.iter().copied())?,
        })
    }

    /// Add whatever of `wanted` is not packed yet, reading only that.
    ///
    /// A graphic already offered costs a lookup in a `BTreeSet` and no file
    /// access at all — including one the client ships no art for, which is the
    /// case that used to make "is the atlas stale" answer yes for ever.
    ///
    /// [`AtlasError::Full`] leaves the atlases holding whatever fitted, and the
    /// caller is expected to throw them away and [`build`](Self::build) for the
    /// current frame. That is not a lost cause: it is the eviction, and it is
    /// the only thing that stops an atlas which only ever grows from filling up
    /// and staying full.
    pub(crate) fn grow(
        &mut self,
        art: &Art,
        texmaps: &TexMaps,
        tiledata: &TileData,
        anim: &mut Anim,
        wanted: &Wanted,
    ) -> Result<(), (&'static str, AtlasError)> {
        // Both halves of a ground quad from the same set, in the same growth: a
        // land graphic in one atlas and not the other draws a slope textured
        // with the terrain next door.
        self.land
            .add(art, wanted.land.iter().copied())
            .map_err(|error| ("land", error))?;
        self.texmaps
            .add(texmaps, tiledata, wanted.land.iter().copied())
            .map_err(|error| ("texmaps", error))?;
        self.statics
            .add(art, wanted.statics.iter().copied())
            .map_err(|error| ("statics", error))?;
        self.mobiles
            .add(anim, wanted.animations.iter().copied())
            .map_err(|error| ("mobiles", error))?;
        Ok(())
    }

    /// Send whatever grew to the textures already bound.
    ///
    /// Nothing at all on the ordinary frame, and a band of rows on the frame a
    /// camera crossed a tile — where this used to be three pipelines and 48MB.
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ground: &GroundRenderer,
        statics: &mut SpriteRenderer,
        items: &mut SpriteRenderer,
        mobiles: &SpriteRenderer,
    ) -> u64 {
        let mut uploaded = ground.upload_changes(queue, &mut self.land, &mut self.texmaps);
        statics.sync_static_pages(device, queue, &self.statics);
        items.sync_static_pages(device, queue, &self.statics);
        for dirty in self.statics.take_dirty() {
            uploaded += u64::from(dirty.rows.end - dirty.rows.start) * u64::from(StaticAtlas::side()) * 4;
            let page = self
                .statics
                .page(dirty.page)
                .expect("dirty page belongs to this static atlas family");
            statics.upload_page_rows(queue, dirty.page, page.pixels(), dirty.rows.clone());
            items.upload_page_rows(queue, dirty.page, page.pixels(), dirty.rows);
        }
        if let Some(rows) = self.mobiles.take_dirty() {
            uploaded += u64::from(rows.end - rows.start) * u64::from(AnimAtlas::side()) * 4;
            mobiles.upload_rows(queue, self.mobiles.pixels(), rows);
        }
        uploaded
    }
}

/// What a set of tile rectangles wants packed, gathered from field references.
///
/// Free rather than a method on `App` because the frame that needs it most is
/// the one holding a `&mut` borrow of the window, where no `&self` method can be
/// called — and threading the pieces explicitly is cheaper than splitting the
/// struct to please the borrow checker.
pub(crate) fn wanted_in(
    map: &Map,
    bands: impl IntoIterator<Item = TileBounds>,
    items: &[GroundItem],
    drawn: &[Mobile],
    animations: &StaticAnimations,
    equip_conv: &EquipConv,
) -> Wanted {
    let mut wanted = Wanted::default();
    for band in bands {
        ground::graphics_in(map, band, &mut wanted.land);
        // Every graphic of every cycle, and not the frame on screen: an atlas
        // grown for what a fire is showing this instant is an atlas grown again
        // when it stops showing it. See `StaticAnimations::cycle`.
        statics::graphics_in(map, band, animations, &mut wanted.statics);
    }
    wanted.statics.extend(items::needed_graphics(items, animations));
    wanted
        .animations
        .extend(mobiles::needed_animations(drawn, equip_conv));
    wanted
}

/// Append exactly one producer job's map art to the resident atlases.
///
/// This deliberately has no rebuild branch.  A far block can wait for a later
/// attempt when an atlas has reached its page limit, but it must never evict
/// the visible camera's art merely to make a background composite possible.
/// Existing atlas pages remain valid because growth only appends pixels/pages;
/// completed composites store final pixels and are not keyed to that growth.
pub(crate) fn prepare_composite_job(
    resources: &mut resources::Resources,
    window: &mut Screen,
    key: CompositeKey,
) -> Option<FlatGroundBlock> {
    let map_width = resources.map.map().width() as i32;
    let map_height = resources.map.map().height() as i32;
    if map_width <= 0 || map_height <= 0 {
        return None;
    }
    let Some(ground) = FlatGroundBlock::inspect(resources.map.map(), key.block) else {
        // This is a stable property of the immutable map, so treat it as a
        // completed LOD0 answer rather than retrying this producer request on
        // every camera frame.
        window
            .composites
            .reject_block(key, None, CompositeQuarantineReason::NonFlatGround);
        return None;
    };
    let (first_x, first_y) = openshard_client_render::composite::tile_origin(key.block);
    let owner = TileBounds {
        min_x: i32::from(first_x),
        max_x: i32::from(first_x) + openshard_uofiles::map::BLOCK_SIZE as i32 - 1,
        min_y: i32::from(first_y),
        max_y: i32::from(first_y) + openshard_uofiles::map::BLOCK_SIZE as i32 - 1,
    };
    let (owner_x, owner_y) = owner.clamp_to(map_width as u32, map_height as u32)?;
    let owner = TileBounds {
        min_x: i32::from(*owner_x.start()),
        max_x: i32::from(*owner_x.end()),
        min_y: i32::from(*owner_y.start()),
        max_y: i32::from(*owner_y.end()),
    };
    // Map statics (animated or otherwise) stay in the live pass, so background
    // LOD work cannot grow or mutate the static atlas and cannot bake a roof
    // outside its 8×8 source.
    let mut wanted = Wanted::default();
    ground::graphics_in(resources.map.map(), owner, &mut wanted.land);
    if window
        .atlases
        .grow(
            &resources.art,
            &resources.texmaps,
            &resources.tiledata,
            &mut resources.anim,
            &wanted,
        )
        .is_err()
    {
        return None;
    }
    window.atlases.upload(
        &window.device,
        &window.queue,
        &window.renderer,
        &mut window.statics,
        &mut window.items_pass,
        &window.mobile_pass,
    );
    Some(ground)
}

/// Grows or, on eviction, wholly rebuilds `window`'s atlases so this frame's
/// picture has everywhere it needs already packed before anything reads them
/// — see `App::draw_from`'s Step three doc for where this call sits.
///
/// A free function and not a method on `App`: this is the one part of
/// presenting a frame that really does write `self` — `resources.anim`,
/// `graphics.covered`, `repacks` — and by taking exactly those fields rather
/// than `&mut self` it stays legible from its signature alone that nothing
/// else on `App` moves here. The same reasoning
/// [`crate::picking::SelectedIdentity::as_static`]'s doc gives for being a
/// free function rather than a method.
///
/// Returns whether a full rebuild ran plus the upload and overflow facts for
/// this frame's jank record.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ready_atlases(
    resources: &mut resources::Resources,
    graphics: &mut graphics::GraphicsSettings,
    world: &world::WorldState,
    repacks: &mut u64,
    window: &mut Screen,
    want: TileBounds,
    wanted: &Wanted,
    drawn: &[(Who, Mobile)],
) -> (bool, AtlasWork) {
    // Set only on a successful rebuild — the counter `docs/camera.md`
    // asks for, so the frame that stalled for one can be told apart from
    // one that is merely heavy. See [`Frame::repacked`](crate::frames::Frame).
    let mut repacked = false;
    let mut work = AtlasWork::default();
    // Full rebuild and not grow, either because the atlas just filled up
    // (the ordinary eviction) or because a debug edit changed a shape the
    // atlas already has packed and `grow` cannot see that on its own — it
    // only asks whether a graphic is packed *at all* (its own doc), so a
    // stair already on screen when its prism changes would never be
    // re-offered. Both land in the same rebuild, for the same reason: the
    // texture a bind group points at is the one the old atlas was
    // uploaded to.
    let evict = if resources.repack_forced {
        resources.repack_forced = false;
        true
    } else {
        // Grow rather than rebuild. What is new is added to the textures
        // already bound, a band of rows at a time, and a frame where the
        // camera stood still reads four `BTreeSet`s and touches no file
        // and no GPU.
        let newly_requested = [
            (
                "land",
                window.atlases.land.newly_requested(wanted.land.iter().copied()),
            ),
            (
                "texmaps",
                window
                    .atlases
                    .texmaps
                    .newly_requested(wanted.land.iter().copied()),
            ),
            (
                "statics",
                window
                    .atlases
                    .statics
                    .newly_requested(wanted.statics.iter().copied()),
            ),
            (
                "mobiles",
                window
                    .atlases
                    .mobiles
                    .newly_requested(wanted.animations.iter().copied()),
            ),
        ];
        let grown = window.atlases.grow(
            &resources.art,
            &resources.texmaps,
            &resources.tiledata,
            &mut resources.anim,
            wanted,
        );
        // Whatever was packed is uploaded, including on the way out of a
        // failure: a growth that stopped part way still wrote pixels, and
        // pixels the device has not been told about are sampled as
        // whatever was there before. Cheap to do unconditionally — the
        // band is empty when nothing grew — and it is one fewer path
        // where an atlas and its texture can disagree.
        work.uploaded_bytes += window.atlases.upload(
            &window.device,
            &window.queue,
            &window.renderer,
            &mut window.statics,
            &mut window.items_pass,
            &window.mobile_pass,
        );
        match grown {
            Ok(()) => {
                graphics.covered = Some(want);
                false
            }
            Err((atlas, AtlasError::Full { .. } | AtlasError::PageLimit { .. })) => {
                work.overflow = Some(AtlasOverflow {
                    atlas,
                    packed_graphics: match atlas {
                        "land" => window.atlases.land.len(),
                        "texmaps" => window.atlases.texmaps.len(),
                        "statics" => window.atlases.statics.len(),
                        "mobiles" => window.atlases.mobiles.len(),
                        _ => unreachable!("Atlas::grow names its own atlas"),
                    },
                    newly_requested_graphics: newly_requested
                        .iter()
                        .find_map(|(name, count)| (*name == atlas).then_some(*count))
                        .expect("Atlas::grow names an atlas counted above"),
                });
                true
            }
            Err((_, error)) => {
                eprintln!("growing the atlases: {error}");
                false
            }
        }
    };
    if evict {
        // Costly and rare on the ordinary path — where the old
        // arrangement paid it every few tiles — and it is the *only*
        // thing that reclaims space, so an atlas that only ever grew
        // would eventually stay full for ever. Cheap and deliberate on
        // the debug-edit path: one static's worth of texture, asked for
        // once per slider change.
        //
        // `covered` is cleared first: a rebuild forgets, so the next
        // frame may not assume anything about what the atlases hold. Set
        // again below only if the rebuild succeeds.
        graphics.covered = None;
        match Atlases::build(
            &resources.art,
            resources.surfaces.as_ref(),
            &resources.texmaps,
            &resources.tiledata,
            &mut resources.anim,
            &wanted_in(
                resources.map.map(),
                [want],
                &world.presentation.items,
                &drawn.iter().map(|(_, mobile)| mobile.clone()).collect::<Vec<_>>(),
                &world.presentation.tile_animations,
                &resources.equip_conv,
            ),
        ) {
            Ok(atlases) => {
                // `install_atlases` creates fresh textures and uploads every
                // byte once. Count that replacement upload as well as dirty
                // row uploads above, otherwise the trace would understate the
                // very hitch its overflow record identifies.
                work.uploaded_bytes += (atlases.land.pixels().len()
                    + atlases.texmaps.pixels().len()
                    + (0..atlases.statics.page_count())
                        .map(|index| {
                            atlases
                                .statics
                                .page(openshard_client_render::atlas::StaticAtlasPage(index as u8))
                                .expect("page_count only reports present pages")
                                .pixels()
                                .len()
                        })
                        .sum::<usize>()
                    + atlases.mobiles.pixels().len()) as u64;
                window.install_atlases(atlases, &resources.hue_ramp);
                graphics.covered = Some(want);
                repacked = true;
                *repacks += 1;
            }
            // One screen does not fit one atlas, which is a different
            // statement from "the atlas filled up": no eviction can help
            // and the frame draws with sprites missing. Named here rather
            // than hidden, and it is what the standing backlog item about
            // a failed repack is about.
            Err(error) => eprintln!("packing the art on screen: {error}"),
        }
    }
    if std::env::var_os("OPENSHARD_ATLAS_AUDIT").is_some() && work.uploaded_bytes != 0 {
        tracing::info!(
            ?want,
            covered = ?graphics.covered,
            uploaded_bytes = work.uploaded_bytes,
            repacked,
            overflow = ?work.overflow,
            "atlas upload during max-zoom audit"
        );
    }
    (repacked, work)
}

/// Reusable offscreen attachments for one map-block producer.
///
/// The producer deliberately owns different attachments from the
/// camera frame.  They have one canonical 864×864 extent, are reused for one
/// bounded job at a time, and must never be resized with the window.  The
/// producer pass is connected only after it can render a complete map block;
/// keeping the targets here now makes that separation explicit rather than
/// letting an implementation accidentally sample [`Screen::world`].
pub(crate) struct CompositeProducerTargets {
    pub(crate) world: wgpu::Texture,
    pub(crate) depth: wgpu::Texture,
    pub(crate) gbuffer: Gbuffer,
}

impl CompositeProducerTargets {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            world: blit::world_texture(device, COMPOSITE_SOURCE_SIDE, COMPOSITE_SOURCE_SIDE),
            depth: renderer::depth_texture(device, COMPOSITE_SOURCE_SIDE, COMPOSITE_SOURCE_SIDE),
            gbuffer: Gbuffer::new(device, COMPOSITE_SOURCE_SIDE, COMPOSITE_SOURCE_SIDE),
        }
    }

    /// Begin a fresh producer image without borrowing any visible-frame
    /// attachment.  The map-only draw that follows must write every plane
    /// before the job is allowed to become `Ready`.
    pub(crate) fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        let world = self.world.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = self.depth.create_view(&wgpu::TextureViewDescriptor::default());
        let gbuffer = self.gbuffer.views();
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map block composite producer clear"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: &world,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &gbuffer.ids,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::gbuffer::IDS_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &gbuffer.position,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::gbuffer::POSITION_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &gbuffer.normal,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(openshard_client_render::gbuffer::NORMAL_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
}

/// Everything a window needs, built once the window exists.
pub(crate) struct Screen {
    pub(crate) window: Arc<Window>,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) renderer: GroundRenderer,
    /// The map-block producer's dedicated mutable ground stream.  It shares
    /// atlas textures with `renderer`, but never its uniform or instance
    /// buffer: producer jobs are submitted independently of camera frames.
    pub(crate) composite_ground: GroundRenderer,
    /// Immutable map-block textures completed by the bounded composite queue.
    /// Work 4 decides where their colour-only pass interleaves with depth and
    /// dynamic objects; keeping the cache here makes producer completion a
    /// device-owned operation without putting it in a camera frame.
    pub(crate) composites: CompositeCache,
    /// The one-quad renderer paired with [`Self::composites`].  It is built at
    /// window creation, not lazily when a block enters the camera.
    pub(crate) composite_pass: CompositeRenderer,
    /// The world target format used for cached colour pixels.  This is normally
    /// [`blit::WORLD_FORMAT`], but keeping the value makes a future format
    /// reconfiguration an explicit cache-invalidating event instead of a
    /// silent cross-format texture reuse.
    pub(crate) composite_output_format: wgpu::TextureFormat,
    /// The pass that draws what stands on the ground.
    pub(crate) statics: SpriteRenderer,
    /// Reusable offscreen attachments for one canonical, map-only composite
    /// producer job. Unlike the camera-frame targets below, these never follow
    /// the viewport and are never a source for visible-frame captures.
    pub(crate) composite_producer: CompositeProducerTargets,
    /// Server-owned ground items. Kept separate from immutable map rows so a
    /// cached block never needs a frame-local instance id.
    pub(crate) items_pass: SpriteRenderer,
    /// What the world is drawn into, at 1:1 and at the camera's render size —
    /// which is the viewport only at zoom 1. [`Screen::blit`] puts it on the
    /// surface.
    pub(crate) world: wgpu::Texture,
    /// The architecture held aside for alpha composition. It has the world's
    /// image size, but no independent depth: its draw reads [`Self::depth`]
    /// so it cannot reveal something the opaque world already hid.
    pub(crate) cutaway_world: wgpu::Texture,
    /// The pass that does that, and the only place a zoom exists.
    pub(crate) blit: Blit,
    /// The depth buffer the three world passes share, which is what decides
    /// whether a hillside covers the wall behind it. Recreated with
    /// [`Screen::world`]: it has to be exactly the size of the image it is
    /// tested against.
    pub(crate) depth: wgpu::Texture,
    /// What the same three passes wrote about each world pixel beside the
    /// picture — which tile it came from, and where its fragment is — read by
    /// the blit to light the frame in world coordinates. See
    /// `openshard_client_render::gbuffer`. Recreated with [`Screen::world`] for
    /// the reason [`Screen::depth`] is: these are attachments of the same passes
    /// and must be exactly that image's size.
    pub(crate) gbuffer: Gbuffer,
    /// Surface data for [`Self::cutaway_world`]. Kept distinct from the main
    /// G-buffer because the visible body must remain the opaque answer for
    /// picking and masks even when a wall is displayed over it translucently.
    pub(crate) cutaway_gbuffer: Gbuffer,
    /// The pass that draws the mobiles, which is the statics pass again with
    /// another atlas bound: a sprite is a sprite, and the two differ only in
    /// where the quad goes.
    pub(crate) mobile_pass: SpriteRenderer,
    /// `docs/gbuffer.md` step 4c's mesh-face pass — depth and place only, for
    /// a climbable static's honest per-face geometry. No atlas dependency, so
    /// unlike `statics`/`mobile_pass` it is never rebuilt when the atlases
    /// are.
    pub(crate) mesh_pass: MeshFaceRenderer,
    /// Everything currently packed, grown as the camera walks into ground it
    /// has not seen. Beside the passes rather than inside them because the CPU
    /// side of an atlas is what builds a quad and the texture is what draws it.
    pub(crate) atlases: Atlases,
    /// Bounded GPU residency for the minimap's terrain chunks — the immutable
    /// texture-array counterpart to [`Self::composites`]. Bound to the
    /// surface's own format, the same as [`Self::gump_pass`]: a minimap
    /// window is drawn on the finished picture, not into the world texture.
    pub(crate) radar_chunks: RadarChunkRenderer,
    /// The minimap's overlay pass: where the body stands, drawn over the
    /// terrain in the same window. It owns no residency at all — a marker is a
    /// handful of tile-sized rectangles, rebuilt every frame, which is what
    /// keeps a step off [`Self::radar_chunks`]'s upload path entirely.
    pub(crate) radar_overlay: RadarOverlayRenderer,
    /// The pass that draws overhead speech, bound to `App::font_atlas` once:
    /// unlike `statics` and `mobile_pass`, nothing ever rebuilds it — the
    /// glyph atlas it is bound to is the whole of `fonts.mul` and does not go
    /// stale the way a camera-scoped atlas does.
    pub(crate) text_pass: SpriteRenderer,
    /// The TrueType glyphs asked for so far, when `App::ttf_font` is set.
    /// Grown a line at a time — see [`App::draw`] — the way [`Screen::atlases`]
    /// grows as the camera walks, because a face with all of Unicode to answer
    /// for has no "whole file" to pack up front the way `fonts.mul` does.
    pub(crate) ttf_atlas: Option<TtfAtlas>,
    /// The pass bound to [`Screen::ttf_atlas`]'s texture, rebuilt whenever that
    /// atlas is — see [`Screen::sync_ttf_scale`], the Chat tab's `TtfScale`
    /// slider's own trigger for that. Bound to the *surface's* format,
    /// [`gump_text_pass`](Screen::gump_text_pass)'s
    /// own reason: overhead speech and the HUD's speech line and journal both
    /// draw through this, after the blit, rather than through a `SpriteRenderer`
    /// bound to [`blit::WORLD_FORMAT`] the way [`Screen::text_pass`] is — see
    /// `openshard_client_render::text::ScreenLabel`'s doc for why a TrueType
    /// face's glyphs cannot go through the world passes' own camera-zoom
    /// scaling the way `text_pass`'s `fonts.mul` glyphs do. `None` exactly
    /// when `ttf_atlas` is.
    pub(crate) ttf_gump_pass: Option<GumpRenderer>,
    /// Which outlined object each world pixel belongs to, or zero for none.
    ///
    /// Filled by the statics pass drawing silhouettes into it and read by
    /// [`Screen::outline`] after the blit. Recreated with [`Screen::world`] for
    /// the reason [`Screen::depth`] is: it is a colour attachment of a pass whose
    /// depth attachment is that buffer, and the two must be the same size.
    pub(crate) outline_mask: wgpu::Texture,
    /// The pass that turns that mask into a ring on the surface — see
    /// `openshard_client_render::outline`.
    pub(crate) outline: Outline,
    /// The same, for what a click is *holding*: the selected static's own
    /// silhouette, in a texture of its own.
    ///
    /// Not [`Screen::outline_mask`], and the separation is the point: the ring
    /// pass draws an edge round every id it finds, so a selection sharing that
    /// mask would come out ringed as well as washed — and the hover ring would
    /// then be two statements in one shape. Recreated with [`Screen::world`],
    /// like its neighbour and for the same reason.
    pub(crate) select_mask: wgpu::Texture,
    /// The pass that washes that silhouette, and the ground under it, after the
    /// blit — see `openshard_client_render::select`.
    pub(crate) select: Select,
    /// The combat target's own ring silhouette — or, when not fighting, a
    /// mobile or item a click named — drawn in [`Ring::SELECTED`] rather than
    /// [`Ring::SOFT`].
    ///
    /// Not [`Screen::outline_mask`]: that one is overwritten every frame by
    /// whatever the cursor is over *this* frame, hover or nothing, so a
    /// selection sharing it would vanish the moment the cursor left the thing
    /// — which is the bug this field exists to not have. Not
    /// [`Screen::select_mask`] either — that one drives the static's wash, and
    /// a mobile or item has no wash of its own to conflict with, but the two
    /// masks are kept apart for the same reason `select_mask` is kept apart
    /// from `outline_mask`: one texture, one question. Recreated with
    /// [`Screen::world`], like its neighbours.
    pub(crate) held_mask: wgpu::Texture,
    /// The pass that draws the lighting's occlusion grid as solids over the
    /// finished picture — `openshard_client_render::solids`, and step 23.0.
    ///
    /// Always built and only ever *used* while the view is on: it is one
    /// pipeline pair and an empty buffer, and the alternative — an `Option`
    /// filled on the frame somebody ticks the box — puts a shader compile in the
    /// middle of a frame a person is looking at.
    pub(crate) solids: SolidsRenderer,
    /// The interface's pass, bound to [`App::gump_atlas`]'s texture and to the
    /// *surface's* format: it draws over the finished frame, not into the world
    /// image. `None` exactly when `App::gumps` is.
    pub(crate) gump_pass: Option<GumpRenderer>,
    /// The interface's text, drawn the same way `gump_pass` draws its art —
    /// bound to the surface's format, over the finished frame — but through
    /// [`App::font_atlas`] instead of the gump atlas. Not an `Option`, and not
    /// tied to `App::gumps` the way `gump_pass` is: `font_atlas` is built at
    /// startup unconditionally (see `text_pass`, its world-space twin), so
    /// there is nothing this has to wait for. A gump dialog's own captions are
    /// its first caller; the speech line and the journal are too, except when
    /// `App::ttf_font` is set — see `ttf_gump_pass`, its TrueType twin — per
    /// `docs/client.md`'s "a third `GumpRenderer` bound to `App::font_atlas`".
    pub(crate) gump_text_pass: GumpRenderer,
    /// What the GPU spent on the last frame it finished, pass by pass — the one
    /// half of a frame's cost that no clock on this thread can see, since
    /// `queue.submit` returns without waiting. `None` when the adapter cannot
    /// write timestamp queries, which is a fact the panel prints rather than a
    /// reason to draw zeroes. See [`crate::profile`].
    ///
    /// Here and not on [`App`] for a borrow reason: `profile::Gpu::scope` takes
    /// `&self`, so a scope can be open on this frame's encoder while the pass it
    /// is timing takes `&mut` of a sibling field. On `App` it would be behind
    /// the `self.window.as_mut()` that has already borrowed the whole of it.
    pub(crate) gpu: Option<profile::Gpu>,
}

impl Screen {
    /// Copy whatever [`Screen::ttf_atlas`] has newly packed this frame onto
    /// [`Screen::ttf_gump_pass`]'s texture.
    ///
    /// The single place both of `App::draw`'s callers route through — overhead
    /// speech's own `atlas.add` and the HUD's — rather than each calling
    /// [`TtfAtlas::take_dirty`] on its own: that method hands back the rows
    /// written since the *last* call and then forgets them (see its doc), so
    /// a second independent caller the same frame would find nothing to
    /// upload — not because the texture was already current, but because the
    /// first caller's `take_dirty` already took the only answer there was.
    /// No-op with nothing dirty, or with no `ttf_atlas` at all — the
    /// offline-map-viewer and no-`--ttf-font` cases both take this path
    /// harmlessly every frame.
    pub(crate) fn upload_ttf_dirty(&mut self) {
        let Some(atlas) = self.ttf_atlas.as_mut() else {
            return;
        };
        let Some(rows) = atlas.take_dirty() else {
            return;
        };
        if let Some(pass) = &self.ttf_gump_pass {
            pass.upload_rows(&self.queue, atlas.pixels(), rows);
        }
    }

    /// Recreate the three passes an atlas rebuild invalidates, and adopt the
    /// new atlas: the shared body of the eviction branch of the ordinary
    /// grow/evict cycle and of a forced rebuild the debug HUD asks for
    /// directly — see `App::apply`'s `authored_prism`. Both need the same
    /// thing for the same reason: the texture a bind group points at is the
    /// one the old atlas was uploaded to, so a pass that keeps the old one
    /// draws the old pixels under a bind group that now names a different
    /// texture.
    pub(crate) fn install_atlases(&mut self, atlases: Atlases, hue_ramp: &HueRamp) {
        self.renderer = GroundRenderer::new(
            &self.device,
            &self.queue,
            blit::WORLD_FORMAT,
            &atlases.land,
            &atlases.texmaps,
        );
        self.composite_ground = self
            .renderer
            .sibling(&self.device, &self.queue, blit::WORLD_FORMAT);
        self.statics = SpriteRenderer::new_static_pages(
            &self.device,
            &self.queue,
            blit::WORLD_FORMAT,
            &atlases.statics,
            hue_ramp,
        );
        self.items_pass = SpriteRenderer::new_static_pages(
            &self.device,
            &self.queue,
            blit::WORLD_FORMAT,
            &atlases.statics,
            hue_ramp,
        );
        self.mobile_pass = SpriteRenderer::new(
            &self.device,
            &self.queue,
            blit::WORLD_FORMAT,
            atlases.mobiles.pixels(),
            hue_ramp,
        );
        self.atlases = atlases;
    }
}

/// A fresh, empty [`TtfAtlas`], and the [`GumpRenderer`]
/// bound to its texture — the pair `App::ttf_font`, given, always has one of.
///
/// Built once per window and never rebuilt. It used to be built again whenever
/// the size changed, because the atlas baked one pixel height for the whole
/// client; it is keyed by `(char, size)` now, so a new size is a few more
/// glyphs packed beside the old ones rather than a texture thrown away. See
/// `docs/text_sizes.md`'s D2, and `TtfAtlas::reset` for the one case that
/// still empties it.
fn build_ttf(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    hue_ramp: &HueRamp,
) -> (TtfAtlas, GumpRenderer) {
    let atlas = TtfAtlas::empty();
    let pass = GumpRenderer::new(device, queue, format, atlas.pixels(), hue_ramp);
    (atlas, pass)
}

impl App {
    pub(crate) fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<Screen, StartupError> {
        // Physical pixels, not logical: a `LogicalSize` here would ask for the
        // same *point* size on every monitor and come out small on a dense
        // one, exactly backwards from what "respect the density" means. Sized
        // off the monitor rather than the `Camera` default (1024x768, meant as
        // a viewport floor, not a window request) so the window opens large on
        // whatever screen it is on.
        let attributes = Window::default_attributes().with_title("OpenShard");
        // Where the last run left it, when there was one and when it still names
        // a screen that exists. The monitors are asked *now*, from the event
        // loop, because a laptop undocked since the last run has a saved frame
        // that opens the window on a monitor nobody has — offscreen, which looks
        // exactly like a client that failed to start. See `Desk::fits`.
        let monitors: Vec<_> = event_loop
            .available_monitors()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                desk::Monitor {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }
            })
            .collect();
        let restored = self
            .desk
            .window
            .filter(|frame| desk::Desk::fits(frame, &monitors));
        let attributes = match restored {
            Some(frame) => attributes
                .with_position(winit::dpi::PhysicalPosition::new(frame.x, frame.y))
                .with_inner_size(winit::dpi::PhysicalSize::new(
                    frame.width.max(1),
                    frame.height.max(1),
                ))
                .with_maximized(frame.maximized),
            // No saved frame: the first run, or one whose screen is gone.
            None => match event_loop.primary_monitor().map(|monitor| monitor.size()) {
                Some(size) if size.width > 0 && size.height > 0 => {
                    attributes.with_inner_size(winit::dpi::PhysicalSize::new(
                        (size.width as f32 * 0.9) as u32,
                        (size.height as f32 * 0.9) as u32,
                    ))
                }
                _ => attributes.with_inner_size(winit::dpi::LogicalSize::new(
                    self.control.camera().width,
                    self.control.camera().height,
                )),
            },
        };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(StartupError::Window)?,
        );
        // Without this, the compositor never starts an IME session for this
        // window, and on Wayland that is what feeds `egui-winit` composed
        // text: a layout that needs one (Cyrillic under a caps-lock layout
        // switch, an East Asian input method) either loses every keystroke or
        // the raw keysym instead of the composed character, silently, while a
        // plain Latin layout still works because it needs no composition —
        // the shell's "say" box looked fine to type in for exactly that
        // reason and nothing else.
        window.set_ime_allowed(true);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(StartupError::Surface)?;

        // Blocking here is fine on the desktop and would not be in a browser,
        // where this whole function becomes an `async` one driven by the event
        // loop. Nothing below cares which way it was awaited.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|error| StartupError::NoDevice(error.to_string()))?;
        // The defaults, plus the one thing the renderer asks for above them: a
        // G-buffer's planes and the picture beside them are past WebGPU's
        // guaranteed `maxColorAttachmentBytesPerSample`, and an adapter that
        // reports only the floor cannot draw this frame. It surfaces here as
        // `NoDevice` with wgpu's own message, which names the limit — see
        // `openshard_client_render::gbuffer::required_limits` for why it is
        // asked for and what brings it back down.
        //
        // And the timestamp queries, when the adapter has them — the frames
        // panel's GPU column, `profile::Gpu`. Asked for **both or neither**: a
        // feature required that the adapter has not got fails the whole
        // `request_device`, which would cost this client its window over a
        // diagnostic, and half the pair measures nothing anyway (see
        // `profile::Gpu::REQUIRED` for why it takes two).
        let timers = match adapter.features().contains(profile::Gpu::REQUIRED) {
            true => profile::Gpu::REQUIRED,
            false => wgpu::Features::empty(),
        };
        // The default WebGPU request exposes only 256 texture-array layers,
        // even where the adapter supports many more.  A fully zoomed-out,
        // rotated minimap needs 729 64×64 radar pages; leaving that default in
        // place made the renderer deliberately drop its outer tiles. The radar
        // pass owns the matching cache budget and layer count; `min` keeps the
        // request within each adapter's limit.
        let mut required_limits = openshard_client_render::gbuffer::required_limits();
        required_limits.max_texture_array_layers =
            radar_chunk_array_layers(adapter.limits().max_texture_array_layers);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: timers,
            required_limits,
            ..Default::default()
        }))
        .map_err(|error| StartupError::NoDevice(error.to_string()))?;

        let capabilities = surface.get_capabilities(&adapter);
        // A non-sRGB format, deliberately: `client/render` writes the art's own
        // bytes and an sRGB surface would gamma-correct them into something
        // else. See that crate's docs.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .ok_or(StartupError::OnlySrgb)?;

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            // `Auto` is the only value guaranteed for every format, and it means
            // "whatever the format says" — which for a non-sRGB format is the
            // pass-through this renderer needs.
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            // Named, and not `present_modes[0]`. This is the loop's pacer: a
            // frame is drawn, `request_redraw` asks for the next one at once,
            // and what makes that a rate rather than a spin is `get_current_texture`
            // blocking here until the display has taken the last one. Whatever
            // the adapter happened to offer first is `Mailbox` on some drivers
            // and `Immediate` on others — neither of which blocks, so the same
            // code is a 60Hz walk on one machine and a busy loop at a thousand
            // frames a second on the next. `Fifo` is the one mode `wgpu`
            // guarantees on every backend, which is why it can be asked for
            // outright rather than searched for.
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // How far the zoom may be walked out. Asked once, because it is a
        // property of the device and not of the frame.
        self.control
            .set_max_texture(device.limits().max_texture_dimension_2d);
        self.control.resize(config.width, config.height);

        let wanted = self.wanted_now();
        let atlases = Atlases::build(
            &self.resources.art,
            self.resources.surfaces.as_ref(),
            &self.resources.texmaps,
            &self.resources.tiledata,
            &mut self.resources.anim,
            &wanted,
        )
        .map_err(StartupError::Atlas)?;
        // What the atlases were built for, which is what the band walk in
        // `draw` subtracts from on the next frame.
        self.graphics.covered = Some(light::lit_tiles(self.control.camera(), &self.tuning()));
        // The world passes draw into the world texture, so they take *its*
        // format and not the surface's — the two differ on an HDR display,
        // where the first non-sRGB surface format is `Rgba16Float`.
        let renderer = GroundRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            &atlases.land,
            &atlases.texmaps,
        );
        let composite_ground = renderer.sibling(&device, &queue, blit::WORLD_FORMAT);
        let composites = CompositeCache::default();
        let composite_pass = CompositeRenderer::new(&device);
        let statics = SpriteRenderer::new_static_pages(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            &atlases.statics,
            &self.resources.hue_ramp,
        );
        let items_pass = SpriteRenderer::new_static_pages(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            &atlases.statics,
            &self.resources.hue_ramp,
        );
        let mobile_pass = SpriteRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            atlases.mobiles.pixels(),
            &self.resources.hue_ramp,
        );
        // No atlas and no format: this pass writes only place and the shared
        // depth buffer, so it does not need rebuilding here on every atlas
        // repack the way `statics`/`mobile_pass` do.
        let mesh_pass = MeshFaceRenderer::new(&device);
        // Built once, unlike `statics` and `mobile_pass`: `font_atlas` is never
        // rebuilt, so neither is what draws it.
        let text_pass = SpriteRenderer::new(
            &device,
            &queue,
            blit::WORLD_FORMAT,
            self.resources.font_atlas.pixels(),
            &self.resources.hue_ramp,
        );
        // Empty, and with no size baked into it: a glyph is packed the first
        // frame something asks for that character *at that size*, and the
        // sizes are the player's own `desk::FontSizes` read where each kind of
        // text is drawn. See `docs/text_sizes.md`.
        let (ttf_atlas, ttf_gump_pass) = match &self.resources.ttf_font {
            Some(_) => {
                let (atlas, pass) = build_ttf(&device, &queue, format, &self.resources.hue_ramp);
                (Some(atlas), Some(pass))
            }
            None => (None, None),
        };
        // The world is drawn at 1:1 into a texture of the camera's render size,
        // which is the viewport only at zoom 1 — see `client/render`'s `blit`.
        let world = blit::world_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let cutaway_world = blit::world_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let depth = renderer::depth_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let outline_mask = outline::mask_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        // The selection's own, at the same size and in the same format: it is a
        // colour attachment of the same silhouette pass, sharing the same depth
        // buffer, so it can be neither larger nor smaller than the world image.
        let select_mask = outline::mask_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        // The held selection's ring silhouette, kept apart from both of the
        // above for `Screen::held_mask`'s own reason.
        let held_mask = outline::mask_texture(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let gbuffer = Gbuffer::new(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let cutaway_gbuffer = Gbuffer::new(
            &device,
            self.control.camera().render_width(),
            self.control.camera().render_height(),
        );
        let composite_producer = CompositeProducerTargets::new(&device);
        let blit = Blit::new(&device, format);
        // The surface's format and not the world's: the ring is drawn over the
        // blit's output, so that a highlight is not dimmed by the night the way
        // the picture under it is.
        let outline = Outline::new(&device, format);
        // And the selection's wash, over the same finished picture and for the
        // same reason: what is held must stay legible after dark.
        let select = Select::new(&device, format);
        // The occlusion grid as solids — `docs/lighting.md` step 23.0. Over the
        // lit picture for the third time and for the third statement of the same
        // reason: a diagnostic that dimmed at night would stop working exactly
        // when the picture is hardest to read.
        let solids = SolidsRenderer::new(&device, format);
        // And the interface's, bound to the surface's format for the same
        // reason: a gump is drawn on the finished picture, and the night that
        // dimmed the world has already been applied to it.
        let gump_pass = self.resources.gumps.as_ref().map(|_| {
            GumpRenderer::new(
                &device,
                &queue,
                format,
                self.resources.gump_atlas.pixels(),
                &self.resources.hue_ramp,
            )
        });
        // The interface's text, built once and unconditionally for the same
        // reason `text_pass` is: `font_atlas` is the whole of `fonts.mul`,
        // packed at startup, and never goes stale.
        let gump_text_pass = GumpRenderer::new(
            &device,
            &queue,
            format,
            self.resources.font_atlas.pixels(),
            &self.resources.hue_ramp,
        );
        // One owner names both this byte budget and the texture-array request
        // above. Keeping them paired prevents an otherwise invisible 256-page
        // device default from truncating a larger cache.
        let radar_chunks = RadarChunkRenderer::new(&device, format, RADAR_CHUNK_CACHE_BUDGET);
        let radar_overlay = RadarOverlayRenderer::new(&device, format);
        // The HUD, with the surface's own format: egui picks its fragment entry
        // point from whether that format is sRGB, and this one deliberately is
        // not.
        self.shell = Some(shell::Shell::new(&device, format, &window, self.desk.clone()));

        // Before `device` is moved into the screen below, and the only reason
        // this line is here rather than beside the passes: it reads the device's
        // features, not any of them.
        let gpu = profile::Gpu::new(&device);

        Ok(Screen {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
            composite_ground,
            composites,
            composite_pass,
            composite_output_format: blit::WORLD_FORMAT,
            statics,
            items_pass,
            composite_producer,
            world,
            cutaway_world,
            blit,
            depth,
            gbuffer,
            cutaway_gbuffer,
            mobile_pass,
            mesh_pass,
            atlases,
            radar_chunks,
            radar_overlay,
            text_pass,
            ttf_atlas,
            ttf_gump_pass,
            outline_mask,
            outline,
            select_mask,
            select,
            held_mask,
            solids,
            gump_pass,
            gump_text_pass,
            gpu,
        })
    }

    /// Ask the platform for another frame, if there is a window to ask.
    ///
    /// The one place that spells `self.window.as_ref()`: every event-loop arm
    /// that decided a frame is stale used to open the `Option` itself, twenty
    /// times over — mechanical, and mechanical is exactly what a repeated `if
    /// let` should not stay. There is no window during the interval between
    /// `resumed` handing one back and the first frame reaching this call, and
    /// asking into that gap is simply a redraw nobody was there to want.
    pub(crate) fn ask_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.window.request_redraw();
        }
    }

    /// Everything on screen right now, whatever the atlases already hold.
    ///
    /// The whole-viewport walk, which is what a rebuild needs and what an
    /// ordinary frame must not do: [`App::wanted_since`] is the frame's version
    /// of the same question and walks only the band the camera crossed.
    ///
    /// [`light::lit_tiles`], not `camera.visible_tiles`: the occlusion grid
    /// `light::collect` builds is grown by the widest flame's own reach, and
    /// reads this same static atlas for an occluder's facing. A wall standing
    /// only in the margin between the two bounds fell back to the whole-tile
    /// shape whenever nothing else had put its graphic in the atlas first —
    /// see `docs/parity.md`'s backlog.
    pub(crate) fn wanted_now(&self) -> Wanted {
        self.wanted_in([light::lit_tiles(self.control.camera(), &self.tuning())])
    }

    /// What the camera has walked onto since `covered` was the lit rectangle,
    /// plus everything that is not a question about the map at all.
    ///
    /// The saving this whole arrangement is for. A frame used to walk the
    /// visible rectangle twice — once for the land graphics and once for the
    /// statics — purely to ask whether the atlases were still good for it, which
    /// is ~9,800 cells at 1080p against a camera that had moved one tile. The
    /// bands [`TileBounds::difference`] hands back are that tile's worth of
    /// cells.
    ///
    /// The invariant it rests on: every cell inside `covered` has already been
    /// offered to the atlases, and an atlas never forgets what it was offered —
    /// not even a graphic the client ships no art for. So a graphic can only be
    /// new outside `covered`, and anything that *does* make an atlas forget has
    /// to set `covered` back to `None` in the same breath.
    ///
    /// `camera` is the frame's snapshot — see [`App::hud`]. What the atlases are
    /// grown for has to be what the passes below then draw, or a band is packed
    /// for one rectangle and sampled for another — which is why `bounds` is
    /// [`light::lit_tiles`] and not `camera.visible_tiles`: `light::collect`
    /// reads this atlas over the wider bound, and `covered` has to name
    /// whichever rectangle was actually packed.
    pub(crate) fn wanted_since(
        &self,
        camera: Camera,
        tuning: &light::Tuning,
        covered: Option<TileBounds>,
    ) -> Wanted {
        let bounds = light::lit_tiles(&camera, tuning);
        let bands = match covered {
            Some(covered) => bounds.difference(covered),
            None => [Some(bounds), None, None, None],
        };
        self.wanted_in(bands.into_iter().flatten())
    }

    /// The graphics on some set of tiles, and everything that is on screen
    /// regardless of where the camera is.
    ///
    /// Items the server has dropped and the bodies walking about are short lists
    /// held in memory, so they are asked in full however small the bands are —
    /// an item that arrives while the camera stands still is on no band at all.
    /// They go into the *static* set deliberately: one atlas serves the map's
    /// statics and the server's items, because a floor tile packed twice is a
    /// floor tile twice.
    pub(crate) fn wanted_in(&self, bands: impl IntoIterator<Item = TileBounds>) -> Wanted {
        let drawn: Vec<Mobile> = self
            .drawn_mobiles()
            .into_iter()
            .map(|(_, mobile)| mobile)
            .collect();
        wanted_in(
            self.resources.map.map(),
            bands,
            &self.world.presentation.items,
            &drawn,
            &self.world.presentation.tile_animations,
            &self.resources.equip_conv,
        )
    }
}
