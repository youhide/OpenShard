//! The GPU passes a frame's already-collected geometry is recorded through:
//! [`encode_world_passes`] is the world — ground, statics, mobiles, the
//! masks and rings — and [`draw_gump_windows`] is this client's own dialogs,
//! drawn over it in the client's own art. Neither decides what is drawn;
//! `crate::frame_geometry` and `App::draw_from` do that, and hand the
//! answer here to be recorded.

use openshard_client_render::blit::{self, ViewportRect};
use std::collections::BTreeSet;

use openshard_client_render::camera::Camera;
use openshard_client_render::composite::{
    CompositeProducerJob, CompositeQuad, ImmutableRevision, MapBlock, MapBlockBounds,
};
use openshard_client_render::geometry::Rect;
use openshard_client_render::gump::{self as gump_art};
use openshard_client_render::lod::BlockLod;
use openshard_client_render::outline::{self, Ring};
use openshard_client_render::renderer::Target;
use openshard_client_render::select::{self, Selection};
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::{paperdoll, solids};
use std::time::{Duration, Instant};

use crate::frame_geometry::FrameGeometry;
use crate::picking::{self, SelectedIdentity};
use crate::window::Screen;

/// Facts from the one world-pass encoding that a GPU dump can later compare
/// with its attachments. Keeping these numbers beside the exact frame closes
/// the gap between a requested LOD policy and the list the ground renderer was
/// actually handed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorldPassAudit {
    pub(crate) requested_lod: BlockLod,
    pub(crate) composite_revision: ImmutableRevision,
    pub(crate) ready_blocks: usize,
    pub(crate) live_ground_quads: usize,
    pub(crate) full_ground_quads: usize,
    /// CPU command-recording costs within the world encoder. These are kept
    /// beside the frame's aggregate `encode` time so a far-zoom trace can
    /// distinguish bind/upload work from the later overlays and UI passes.
    pub(crate) cpu_ground: Duration,
    pub(crate) cpu_composites: Duration,
    pub(crate) cpu_ground_detail: Duration,
    pub(crate) ground_detail_cpu_uniforms: Duration,
    pub(crate) ground_detail_cpu_serialize: Duration,
    pub(crate) ground_detail_cpu_upload: Duration,
    pub(crate) ground_detail_cpu_pass: Duration,
    pub(crate) cpu_statics: Duration,
    pub(crate) cpu_items: Duration,
    pub(crate) composite_bindings_created: usize,
    pub(crate) composite_bindings_reused: usize,
    pub(crate) composite_cpu_upload: Duration,
    pub(crate) composite_cpu_bindings: Duration,
    pub(crate) composite_cpu_pass: Duration,
}
use crate::panes::Pane;
use crate::windows::{Drawn, WindowSubject};
use crate::{graphics, panes, profile, resources, shell, windows, world};

/// The shard's dialogs, in the client's own art, packed and drawn — a
/// container, a paperdoll, the skill sheet, all three through one machinery.
/// None of them is an egui window: their position, their drag, their
/// z-order and their hit test are this client's, in gump pixels, which is
/// decision 5 in `docs/client.md`. See `own_windows`, `crate::gump`,
/// `openshard_client_render::container` and
/// `openshard_client_render::paperdoll`.
///
/// A free function for the same reason [`encode_world_passes`] is one:
/// `resources.gump_atlas` grows and `windows.drawn_windows` is written here,
/// and both are named in the signature rather than reached through
/// `&mut self`. Does nothing when there is no gump file or no pass to draw
/// through — an offline run with neither.
///
/// `hover` is the shard's tooltip for whatever the pointer is on, first line
/// first, or empty. Handed in already resolved rather than worked out here:
/// deciding *what* the pointer is asking about needs the pick order and the
/// view, and answering it may put a `0xD6` on the wire — see
/// `App::hover_tooltip`, and this function is the part that only draws.
// Named individually on purpose — see the doc above: reaching them through
// `&mut self` is what this function exists to avoid.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_gump_windows(
    resources: &mut resources::Resources,
    world: &world::WorldState,
    windows: &mut windows::Windows,
    cursor: gump_art::GumpPixel,
    hover: &[String],
    shell: Option<&shell::Shell>,
    window: &mut Screen,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) {
    // `drawn_windows` is both the previous frame's hit-test map and the
    // source of the text overlay assembled later in `App::draw_from`.  It
    // must therefore describe *this* frame's art pass, including the frame in
    // which the pass cannot run.  Keeping yesterday's layout here used to
    // leave an invisible window intercepting clicks (and its labels still
    // eligible for the text pass) after gump assets or their GPU pass were
    // unavailable.
    if let (Some(files), Some(pass)) = (resources.gumps.as_ref(), window.gump_pass.as_mut()) {
        let mut pictures = Vec::new();
        // A dialog's art used to be packed here, in a loop over every gump in
        // the *view* — one rung above the windows, so a dialog this client had
        // closed and the view had not yet forgotten still had its pictures
        // packed. It is `DialogPane::art` now, asked of the window, and it goes
        // through the same three-phase order as every other kind's below: art
        // for all, pack once, layout for all.
        //
        // This client's own windows — a dialog, a container, a paperdoll —
        // all three through one machinery.
        //
        // Bottom to top, which is the list's own order: the pass has no
        // depth, so later is over.
        //
        // The layouts are built before the loop that packs them, so that
        // nothing borrows the view while the atlas is being grown.
        // Paired with their subjects rather than left parallel to
        // `own_windows`: a container whose entry has gone from the view is
        // skipped below, and an index into one list would then name the
        // wrong window in the other. This list is what the pointer is
        // tested against next frame — see `windows::Windows::drawn_windows`.
        let mut drawn_windows: Vec<(WindowSubject, Drawn)> = Vec::new();
        if let Some(view) = world.authoritative.view.as_ref() {
            // **Decision 6's order, and it is now every kind's:** every pane
            // says what art it needs, all of it is packed, and only then is
            // anything laid out. Collected into a list
            // first because a pane reads the whole of `Resources` while it
            // answers and the atlas is a field of it — packing inside the walk
            // would be growing the very thing the walk is holding.
            let hand = windows.hand;
            // The pointer, in each window's own gump pixels — see
            // `PaneFrame::cursor`'s doc for why this is the one arithmetic a
            // pane never has to do for itself.
            let local_cursor =
                |at: gump_art::GumpPixel| gump_art::GumpPixel::new(cursor.x - at.x, cursor.y - at.y);
            let wanted: Vec<(WindowSubject, Vec<gump_art::GumpArt>)> = windows
                .own_windows
                .iter()
                .map(|open| {
                    let frame = panes::PaneFrame {
                        view,
                        resources,
                        cursor: local_cursor(open.at),
                        hand,
                        has_keyboard: windows.keyboard == Some(open.subject),
                        has_prompt: windows.prompt == Some(crate::windows::Asking::Window(open.subject)),
                    };
                    (open.subject, open.pane.art(&frame))
                })
                .collect();
            for (subject, art) in wanted {
                let art_files = gump_art::ArtFiles {
                    gumps: files,
                    items: &resources.art,
                };
                // Said once per window and drawn anyway, for `gump::art_of`'s
                // reason above.
                if let Err(error) = resources.gump_atlas.add(art_files, art) {
                    eprintln!("packing window art for {subject:?}: {error}");
                }
            }
            // No container loop here any more: a bag says what art it needs in
            // `panes::container::ContainerPane::art`, packed by the sweep
            // above with every other kind's. It was the last window kind whose
            // pictures this function asked for on the window's behalf.
            for open in &windows.own_windows {
                // A kind that has moved into a pane lays itself out, and its
                // answer is the only one — the same three rungs the input side
                // has in `panes::route`, and the same reason: two places laying
                // one window out are two pictures waiting to disagree.
                let frame = panes::PaneFrame {
                    view,
                    resources,
                    cursor: local_cursor(open.at),
                    hand,
                    has_keyboard: windows.keyboard == Some(open.subject),
                    has_prompt: windows.prompt == Some(crate::windows::Asking::Window(open.subject)),
                };
                if let Some(drawn) = open.pane.layout(&frame) {
                    drawn_windows.push((open.subject, drawn));
                    continue;
                }
                match open.subject {
                    // Laid out by `panes::vendor::VendorPane` above. Reaching
                    // here is a shop whose catalogue has gone out of the view
                    // between the packet and the frame, which is nothing to
                    // draw and nothing to click.
                    WindowSubject::Vendor(_) => {}
                    // Laid out by `panes::dialog::DialogPane` above, and
                    // reaching here is the shop's case again: the view can drop
                    // a gump between the packet that opened the window and the
                    // frame that would have drawn it.
                    WindowSubject::Dialog(_) => {}
                    // Laid out by `panes::skills::SkillsPane` above, the same
                    // as a vendor's: reaching here is impossible, because a
                    // sheet always has a layout — it draws its own frame with
                    // nothing at all in the view.
                    WindowSubject::Skills => {}
                    // Laid out by `panes::status::StatusPane` above, and
                    // reaching here is the shop's case rather than the sheet's:
                    // the Status button opens the window and asks for the
                    // `0x11` in one press, so a frame or two can pass in which
                    // this client has none of the numbers to write on it.
                    WindowSubject::Status => {}
                    // Laid out by `panes::paperdoll::PaperdollPane` above, and
                    // reaching here is the sheet's case rather than the shop's:
                    // the pane answers `None` only for a client with no gump
                    // art at all, and this loop runs inside the `Some(files)`
                    // arm of exactly that question.
                    WindowSubject::Paperdoll(_) => {}
                    // Laid out by `panes::container::ContainerPane` above, and
                    // reaching here is the shop's case: the view can drop a
                    // `0x24` between the packet that took the bag away and the
                    // reconcile that will take the window with it.
                    //
                    // What used to stand here was the last window kind this
                    // function laid out itself — the icons, the lifted one
                    // subtracted, and a pending drop projected back in. All
                    // three are the pane's, and the list it drew travels with
                    // the pictures so that a click and the picture cannot
                    // disagree about which icon is which.
                    WindowSubject::Container(_) => {}
                }
            }
        }
        for (_, window) in &drawn_windows {
            let art_files = gump_art::ArtFiles {
                gumps: files,
                items: &resources.art,
            };
            // Everything the window will draw, packed before it is drawn —
            // a picture the atlas grew on the *next* frame would draw the
            // window with a hole in it once. Said and drawn anyway on a
            // failure, for `gump::art_of`'s reason above.
            if let Err(error) = resources
                .gump_atlas
                .add(art_files, paperdoll::art_of(window.pictures()))
            {
                eprintln!("packing window art: {error}");
            }
        }
        // The item follows the pointer above every window while the shard has
        // it on the cursor. It intentionally is not added to `drawn_windows`:
        // it is a cursor preview, not a window that can intercept a drop.
        if let Some(drag) = windows
            .hand
            .filter(|hand| hand.pending_drop().is_none())
            .map(crate::hand::Hand::drag)
        {
            let art_files = gump_art::ArtFiles {
                gumps: files,
                items: &resources.art,
            };
            if let Err(error) = resources.gump_atlas.add(
                art_files,
                [gump_art::GumpArt::Item(
                    openshard_client_render::items::displayed_graphic(drag.item.graphic, drag.item.amount),
                )],
            ) {
                eprintln!("packing dragged item art: {error}");
            }
            pictures.push(
                gump_art::Picture::plain(
                    gump_art::GumpArt::Item(openshard_client_render::items::displayed_graphic(
                        drag.item.graphic,
                        drag.item.amount,
                    )),
                    gump_art::GumpPixel::new(cursor.x - drag.grab.x, cursor.y - drag.grab.y),
                )
                .hued(drag.item.hue),
            );
        }
        // What the pointer is tested against from here on, and the atlas it
        // is tested in is the one just grown for it: the hit test and the
        // frame are now the same list. Kept even when it is empty — the
        // windows this frame drew none of are windows nothing can click.
        windows.drawn_windows = drawn_windows;
        if let Some(rows) = resources.gump_atlas.take_dirty() {
            pass.upload_rows(&window.queue, resources.gump_atlas.pixels(), rows);
        }
        let frame = gump_art::Frame {
            target: view,
            width: window.config.width,
            height: window.config.height,
            scale: shell.map(|shell| shell.pixels_per_point()).unwrap_or(1.0),
        };
        // A window is one painter layer: its frame and icons, then the text
        // belonging to that frame, before the next window is allowed to cover
        // it.  A global text pass cannot express this ordering.
        for (subject, drawn) in &windows.drawn_windows {
            // Every pane laid this window out window-local — see
            // `PaneFrame::cursor`'s doc — so this is the one place its
            // pictures and its text become the screen-space geometry a pass
            // draws: the window's own absolute placement, added back once.
            // Looked up in `own_windows` rather than carried on
            // `drawn_windows` itself, because it is the *current* position
            // that has to agree with what is on screen this frame — the same
            // one the layout above was just built with — and not a second
            // copy that could go stale relative to it.
            let at = windows
                .own_windows
                .iter()
                .find(|open| open.subject == *subject)
                .map(|open| open.at)
                .unwrap_or_default();
            let offset_pictures: Vec<gump_art::Picture> = drawn
                .pictures()
                .iter()
                .map(|picture| picture.offset(at))
                .collect();
            let art = gump_art::collect(&offset_pictures, &resources.gump_atlas);
            pass.render_layer(&window.device, &window.queue, encoder, frame, &art);
            let mut labels = Vec::new();
            let mut cut = Vec::new();
            match (subject, drawn) {
                // A shop's arm and a status frame's, now that a dialog's
                // captions are resolved by the pane that laid it out: this pass
                // reads what the layout produced and looks nothing up. It used
                // to reach into a `Dialogs` for the text table, the cliloc and
                // the typed contents of every field — the second half of the
                // window, worked out in a different place from the first.
                (WindowSubject::Dialog(_), Drawn::Dialog(laid_out)) => {
                    labels.extend(laid_out.lines.iter().map(|line| line.label().offset(at)));
                }
                // A dialog's arm and a shop's: the pane resolved the name and
                // the hover label when it laid the window out, and this pass
                // reads what the layout produced and looks nothing up.
                (WindowSubject::Paperdoll(_), Drawn::Paperdoll(window)) => {
                    labels.extend(window.lines.iter().map(|line| line.label().offset(at)));
                }
                (WindowSubject::Skills, Drawn::Skills(sheet)) => {
                    for line in &sheet.lines {
                        let mut quads = openshard_client_render::text::collect_gump(
                            &[line.label().offset(at)],
                            &resources.font_atlas,
                        );
                        if let Some(scissor) = line.scissor {
                            scissor.offset(at).cut(&mut quads);
                        }
                        cut.extend(quads);
                    }
                }
                (WindowSubject::Status, Drawn::Status(status)) => {
                    labels.extend(status.lines.iter().map(|line| line.label().offset(at)));
                }
                (WindowSubject::Vendor(_), Drawn::Vendor(vendor)) => {
                    labels.extend(vendor.lines.iter().map(|line| line.label().offset(at)));
                }
                // A bag's plate caption and its hover label, resolved by the
                // pane that laid the window out — the same shape as a
                // dialog's and a doll's. This arm used to work out both here:
                // which of the two plates a window has, tested twice against
                // the backpack and the vendor list, and the name of the icon
                // under the pointer, looked up in the view a second time. Both
                // are decided where they are drawn now.
                (WindowSubject::Container(_), Drawn::Container(window)) => {
                    labels.extend(window.lines.iter().map(|line| line.label().offset(at)));
                }
                _ => {}
            }
            let mut text = openshard_client_render::text::collect_gump(&labels, &resources.font_atlas);
            text.extend(cut);
            window
                .gump_text_pass
                .render_layer(&window.device, &window.queue, encoder, frame, &text);
        }
        let dragged = gump_art::collect(&pictures, &resources.gump_atlas);
        pass.render_layer(&window.device, &window.queue, encoder, frame, &dragged);
        // The shard's tooltip for whatever the pointer is on, last of all so it
        // stands over every window and over the dragged item both. Its own
        // layer and not one of the windows': the object it describes may be in
        // the world behind them, and a tooltip filed under a window would be
        // cut off with that window's frame.
        //
        // Under the cursor and offset down-right by the same step the container
        // hover uses, so a pointer never sits on top of the first line it is
        // asking about.
        if !hover.is_empty() {
            let step = tooltip_line_step(&resources.font_atlas);
            let labels: Vec<_> = hover
                .iter()
                .enumerate()
                .map(|(row, line)| openshard_client_render::text::GumpLabel {
                    at: cursor.offset(gump_art::GumpPixel::new(
                        TOOLTIP_OFFSET.x,
                        TOOLTIP_OFFSET.y + step * row as i32,
                    )),
                    text: line,
                    font: TOOLTIP_FONT,
                    hue: openshard_protocol::wire::Hue::LABEL,
                    clip: None,
                })
                .collect();
            let text = openshard_client_render::text::collect_gump(&labels, &resources.font_atlas);
            window
                .gump_text_pass
                .render_layer(&window.device, &window.queue, encoder, frame, &text);
        }
    } else {
        windows.drawn_windows.clear();
    }
}

/// The face a tooltip is drawn in — `fonts.mul`'s face 1, the same one every
/// other label in this client uses.
const TOOLTIP_FONT: openshard_protocol::speech::Font = openshard_protocol::speech::Font(1);

/// Where the first line sits relative to the pointer, in gump pixels. The same
/// step [`container_hover_text`]'s label uses, so the two hovers read as one
/// behaviour rather than two that happen to both be near the cursor.
const TOOLTIP_OFFSET: gump_art::GumpPixel = gump_art::GumpPixel { x: 14, y: 18 };

/// The vertical step between two tooltip lines, in gump pixels.
///
/// Read off the face rather than written down: `fonts.mul` holds ten faces of
/// different heights, and a number fixed here would be wrong the moment a
/// tooltip was drawn in another one. A capital `M` is the measure — full height
/// in every face and no descender — plus two pixels so consecutive lines do not
/// touch. The fallback is only reachable with a font atlas that packed no `M`,
/// which is a broken `fonts.mul` rather than a case to be right about.
fn tooltip_line_step(fonts: &openshard_client_render::atlas::FontAtlas) -> i32 {
    fonts
        .glyph(TOOLTIP_FONT, b'M')
        .map_or(16, |sprite| i32::from(sprite.height) + 2)
}

/// Records every world-space pass into `encoder`, from the ground up to the
/// hover and held rings — the one part of presenting a frame that is
/// **only** drawing, in the sense the module docs on
/// [`crate::graphics::GraphicsSettings`] and [`crate::picking::Picking`]
/// argue for: a free function taking `&mut GraphicsSettings` for the one
/// pair of fields it writes (`solids_held`, `solids_drawn`, this frame's own
/// count of what the solids view was handed and drew) and `&Picking` for the
/// one it only reads. See `crate::frame_geometry::assemble_geometry`'s doc
/// for the same shape one step earlier in the frame.
///
/// `encoder` and `window` are threaded through rather than returned: the
/// gump windows, the chat line and egui all record into the same encoder
/// after this call returns, and `App::draw_from` is what owns that sequence.
///
/// `geometry` is taken whole and not exploded into its fields: it is
/// `assemble_geometry`'s own output, still in the shape that function built
/// it in, and every field this pass reads (all but `asked_for`, which is the
/// F12 dump's) comes straight off it. `text_quads` is the one thing drawn
/// here that is not part of it — the overhead speech quads, collected
/// separately after the geometry is assembled — so it stays its own
/// parameter rather than a field something would have to graft onto
/// [`FrameGeometry`] just to be threaded through in the same breath.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_world_passes(
    graphics: &mut graphics::GraphicsSettings,
    picking: &picking::Picking,
    window: &mut Screen,
    encoder: &mut wgpu::CommandEncoder,
    target: Target<'_>,
    view: &wgpu::TextureView,
    world_view: &wgpu::TextureView,
    gbuffer_views: &openshard_client_render::gbuffer::Views,
    cutaway_world_view: &wgpu::TextureView,
    cutaway_gbuffer_views: &openshard_client_render::gbuffer::Views,
    viewport: ViewportRect,
    camera: Camera,
    solid_cut: openshard_client_render::solid::Cut,
    geometry: &FrameGeometry,
    text_quads: &[SpriteQuad],
    render_width: u32,
    render_height: u32,
    composite_lod: BlockLod,
    composite_revision: ImmutableRevision,
    composite_visible: Option<MapBlockBounds>,
) -> WorldPassAudit {
    // Ground first, because it clears; statics after, into what it left.
    // Which covers which is decided by the depth they share, not by this
    // order — the order only decides who clears.
    //
    // Every pass from here to the submit is bracketed by `profile::begin`
    // and `profile::end`: the GPU's own timestamps, which are the one half
    // of a frame's cost no clock on this thread can see. See [`profile`] for
    // why that is so and why the bracket is a pair of calls rather than a
    // scope guard. Nothing when the adapter has no timestamp queries.
    let ready: Vec<(
        MapBlock,
        Rect,
        &openshard_client_render::composite::CompositeTexture,
    )> = (geometry.cutaway_instances.drawn == 0)
        .then_some(composite_visible)
        .flatten()
        .into_iter()
        .flat_map(MapBlockBounds::blocks)
        .filter_map(|block| {
            let texture =
                window
                    .composites
                    .selected_or_more_detailed(block, composite_lod, composite_revision)?;
            if !texture.has_deferred() {
                return None;
            }
            debug_assert_eq!(texture.ground().block(), block);
            let rect = texture.rect_in(camera);
            debug_assert_eq!(
                rect,
                CompositeProducerJob::for_flat_ground(texture.key(), texture.ground()).rect_in(camera),
                "the cached texture must restore through its producer transform"
            );
            Some((block, rect, texture))
        })
        .collect();
    let cached_blocks: BTreeSet<_> = ready.iter().map(|(block, _, _)| *block).collect();
    let ground = geometry.detail_ground(&cached_blocks);
    let mut audit = WorldPassAudit {
        requested_lod: composite_lod,
        composite_revision,
        ready_blocks: ready.len(),
        live_ground_quads: ground.len(),
        full_ground_quads: geometry.quads.len(),
        cpu_ground: Duration::ZERO,
        cpu_composites: Duration::ZERO,
        cpu_ground_detail: Duration::ZERO,
        ground_detail_cpu_uniforms: Duration::ZERO,
        ground_detail_cpu_serialize: Duration::ZERO,
        ground_detail_cpu_upload: Duration::ZERO,
        ground_detail_cpu_pass: Duration::ZERO,
        cpu_statics: Duration::ZERO,
        cpu_items: Duration::ZERO,
        composite_bindings_created: 0,
        composite_bindings_reused: 0,
        composite_cpu_upload: Duration::ZERO,
        composite_cpu_bindings: Duration::ZERO,
        composite_cpu_pass: Duration::ZERO,
    };
    if composite_lod == BlockLod::Lod0 && !ready.is_empty() {
        tracing::error!(?audit, "LOD0 world pass selected cached map blocks");
    }
    let (map_static_rows, map_statics_drawn) = geometry.detail_map_statics(&cached_blocks);
    // The ordinary all-live path clears and draws at once. With a cache, clear
    // first but defer the live slope/rim rows until after restore: their exact
    // current-frame depth must be able to beat a neighbouring cached flat tile
    // where the two diamonds overlap.
    let ground_started = Instant::now();
    if ready.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "ground", encoder);
        window
            .renderer
            .render(&window.device, &window.queue, encoder, target, &ground);
        profile::end(window.gpu.as_ref(), encoder, timed);
    } else {
        window
            .renderer
            .render(&window.device, &window.queue, encoder, target, &[]);
    }
    let cpu_ground = ground_started.elapsed();
    // Cached flat ground is restored before all live sprite passes. Map statics
    // deliberately remain live: their art can overhang an 8×8 ground block by
    // more than the bounded composite margin, while this shared depth buffer
    // still orders them against every cached ground pixel.
    let composite_blocks: Vec<_> = ready
        .iter()
        .map(|(_, rect, texture)| CompositeQuad { texture, rect: *rect })
        .collect();
    let (eye_x, eye_y) = camera.eye_tile();
    let current_depth_base = openshard_client_render::depth::base_for(eye_x, eye_y);
    let composites_started = Instant::now();
    let timed = profile::begin(window.gpu.as_ref(), "map composites", encoder);
    // `render_deferred` keeps the depth-base correction per block instance, so
    // the full far-zoom set is one upload/call. Its own per-call batch remains
    // immutable until this encoder is submitted, preserving the artifact fix
    // for multiple deferred calls in one command buffer.
    window.composite_pass.begin_frame();
    window.composite_pass.render_deferred_rebased(
        &window.device,
        &window.queue,
        encoder,
        target,
        current_depth_base,
        &composite_blocks,
    );
    (audit.composite_bindings_created, audit.composite_bindings_reused) =
        window.composite_pass.deferred_binding_stats();
    let composite_cpu = window.composite_pass.deferred_cpu_costs();
    audit.composite_cpu_upload = composite_cpu.upload;
    audit.composite_cpu_bindings = composite_cpu.bindings;
    audit.composite_cpu_pass = composite_cpu.pass;
    profile::end(window.gpu.as_ref(), encoder, timed);
    let ground_detail_started = Instant::now();
    if !ready.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "ground detail", encoder);
        window
            .renderer
            .render_loaded(&window.device, &window.queue, encoder, target, &ground);
        let ground_detail_cpu = window.renderer.last_cpu_costs();
        audit.ground_detail_cpu_uniforms = ground_detail_cpu.uniforms;
        audit.ground_detail_cpu_serialize = ground_detail_cpu.serialize;
        audit.ground_detail_cpu_upload = ground_detail_cpu.upload;
        audit.ground_detail_cpu_pass = ground_detail_cpu.pass;
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    audit.cpu_ground_detail = ground_detail_started.elapsed();
    let cpu_composites = composites_started.elapsed();
    // Handed over every frame rather than on the key, because the key does
    // not have the window: `graphics.fringe` is the switch and the pass is where
    // it is read, and a state pushed once at start-up would leave F2 silent.
    window.statics.set_fringe(graphics.fringe);
    window.items_pass.set_fringe(graphics.fringe);
    let statics_started = Instant::now();
    let timed = profile::begin(window.gpu.as_ref(), "statics", encoder);
    window.statics.render(
        &window.device,
        &window.queue,
        encoder,
        target,
        map_static_rows,
        &geometry.mesh.boxes,
        Some(map_statics_drawn),
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    let cpu_statics = statics_started.elapsed();
    // Composite entries are potentially eight RGBA-sized planes each.  Keep a
    // bounded LRU tail, but protect one block outside the current viewport so
    // a small pan cannot turn into an upload/pan-back loop.  This maintenance
    // never builds pixels and therefore cannot add a synchronous camera-frame
    // composition cost.
    let _evicted_composites = window.composites.evict_lru_outside_viewport(composite_visible);
    // Server items intentionally run after immutable map statics.  The shared
    // depth buffer preserves their historical interleaving, while keeping this
    // buffer free of map rows is what lets a cached map composite keep a stable
    // G-buffer identity without making a dynamic item point at a stale row.
    let items_started = Instant::now();
    let timed = profile::begin(window.gpu.as_ref(), "items", encoder);
    window.items_pass.render_with_id_bits(
        &window.device,
        &window.queue,
        encoder,
        target,
        &geometry.item_instances.rows,
        &[],
        Some(geometry.item_instances.drawn),
        openshard_client_render::gbuffer::IDS_DYNAMIC_ITEM,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    let cpu_items = items_started.elapsed();
    audit.cpu_ground = cpu_ground;
    audit.cpu_composites = cpu_composites;
    audit.cpu_statics = cpu_statics;
    audit.cpu_items = cpu_items;
    // Right after statics, into the same static's own pixels its
    // billboard sprite just drew — `docs/gbuffer.md` step 4c. Depth and
    // place only, never colour: this only gives a climbable static's
    // pixels a more honest per-face normal than one blended stance could.
    let timed = profile::begin(window.gpu.as_ref(), "mesh faces", encoder);
    window.mesh_pass.render(
        &window.device,
        &window.queue,
        encoder,
        target,
        &geometry.mesh.mesh_vertices,
        &geometry.mesh.mesh_rows,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    let timed = profile::begin(window.gpu.as_ref(), "mobiles", encoder);
    window.mobile_pass.render(
        &window.device,
        &window.queue,
        encoder,
        target,
        &geometry.mobile_quads,
        // A mobile has no volume — `docs/lighting_rebuild.md` says so in as
        // many words, and phase 7 is what gives a billboard a normal.
        &[],
        None,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    // The architectural cutaway comes after mobiles so its depth test sees the
    // settled opaque world. It writes its *own* picture and G-buffer but reads
    // the main depth without writing it; its separately lit colour is composed
    // after the main deferred blit below.
    if geometry.cutaway_instances.drawn != 0 {
        let timed = profile::begin(window.gpu.as_ref(), "cutaway geometry", encoder);
        let cutaway_target = Target {
            view: cutaway_world_view,
            depth: target.depth,
            gbuffer: cutaway_gbuffer_views,
            width: target.width,
            height: target.height,
            projection: target.projection,
        };
        window.statics.render_cutaway(
            &window.device,
            &window.queue,
            encoder,
            cutaway_target,
            &geometry.cutaway_instances.rows,
            &geometry.cutaway_boxes,
            geometry.cutaway_instances.drawn,
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    // The silhouettes, here and not later: the mask is depth-tested against
    // what the three world passes have drawn, so a barrel behind a wall is
    // kept out of it — and the text pass below writes depth at the near
    // plane over everything, which would punch the mask through.
    let mask_view = window
        .outline_mask
        .create_view(&wgpu::TextureViewDescriptor::default());
    // One item is one ring; the pass numbers groups, so each quad is a group
    // of its own — see `SpriteRenderer::render_mask`.
    let item_rings: Vec<&[SpriteQuad]> = geometry.outline_quads.iter().map(std::slice::from_ref).collect();
    let timed = profile::begin(window.gpu.as_ref(), "outline mask: items", encoder);
    window.statics.render_mask(
        &window.device,
        &window.queue,
        encoder,
        target,
        &mask_view,
        &item_rings,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    // And a creature through its own atlas, in *one* group: a body and
    // everything it wears is one thing being pointed at, and one ring goes
    // round the lot. This pass clears the mask too, which is why it is
    // skipped when nothing is ringed — the items' pass above has already
    // written the frame's answer, and a second clear would erase it.
    if !geometry.mobile_outline.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "outline mask: mobiles", encoder);
        window.mobile_pass.render_mask(
            &window.device,
            &window.queue,
            encoder,
            target,
            &mask_view,
            &[&geometry.mobile_outline],
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    // And the held selection into its own mask, through the same pass and
    // the same depth buffer: what is washed is what is *visible* of the
    // selected static, so a wall the player has walked behind is not painted
    // over the thing now in front of it. One group, because a selection is
    // one thing — the pass numbers groups for the ring's sake and the wash
    // reads only "is this texel nought".
    let select_view = window
        .select_mask
        .create_view(&wgpu::TextureViewDescriptor::default());
    if !geometry.select_quads.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "select mask", encoder);
        window.statics.render_mask(
            &window.device,
            &window.queue,
            encoder,
            target,
            &select_view,
            &[&geometry.select_quads],
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    // And the combat target (or, with no target, the held mobile/item) into
    // `Screen::held_mask` — the same two-pass shape as the hover ring
    // above (items first, unconditionally, so an empty frame still clears
    // the mask; the mobile pass gated because it clears the mask too).
    // Not folded into the hover mask above: a click's ring must survive
    // the cursor moving off the thing, and that mask is overwritten fresh
    // every frame from whatever the cursor is over *this* frame alone.
    let held_view = window
        .held_mask
        .create_view(&wgpu::TextureViewDescriptor::default());
    let held_item_rings: Vec<&[SpriteQuad]> = geometry
        .held_item_outline
        .iter()
        .map(std::slice::from_ref)
        .collect();
    let timed = profile::begin(window.gpu.as_ref(), "held mask: items", encoder);
    window.statics.render_mask(
        &window.device,
        &window.queue,
        encoder,
        target,
        &held_view,
        &held_item_rings,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    if !geometry.held_mobile_outline.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "held mask: mobiles", encoder);
        window.mobile_pass.render_mask(
            &window.device,
            &window.queue,
            encoder,
            target,
            &held_view,
            &[&geometry.held_mobile_outline],
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    // Always `text_pass`, `fonts.mul`'s own: `text_quads` is empty
    // whenever `App::ttf_font` is set, since a TrueType face's speech
    // draws after the blit instead — see `screen_speech`'s own comment
    // above and the render call after it, below.
    let timed = profile::begin(window.gpu.as_ref(), "overhead text", encoder);
    window.text_pass.render(
        &window.device,
        &window.queue,
        encoder,
        target,
        text_quads,
        &[],
        None,
    );
    profile::end(window.gpu.as_ref(), encoder, timed);
    // And the world image onto the surface, into the rect the panels left
    // free. Magnified this is a copy — the image is already the viewport's
    // size and the magnification happened in the vertex transform — and
    // minified it is where the shrinking happens, which is why the zoom is
    // still what picks the sampler.
    //
    // The lighting — the flames, the sun, the lantern in the player's hand
    // and which of the pass's own values is drawn — was assembled at the top
    // of the frame, out of `frame::Inputs`. Nothing between there and here
    // may touch it: a frame this client draws and a frame a tool dumps are
    // the same frame only for as long as neither of them has an adjustment
    // of its own afterwards. `docs/parity.md`.
    //
    // **Solids alone**, `App::solids_only`: the surface is cleared and the
    // world image is not drawn onto it at all, so the boxes below stand
    // over nothing that could be mistaken for their own shape. `lighting`
    // is unaffected either way — it is what the solids pass reads its grid
    // from, and it was already built above whichever branch runs here.
    if graphics.solids_only && graphics.show_solids {
        let timed = profile::begin(window.gpu.as_ref(), "solids-only clear", encoder);
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("solids-only clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(openshard_client_render::renderer::CLEAR),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        profile::end(window.gpu.as_ref(), encoder, timed);
    } else {
        // **The pass to watch.** Deferred shading over the whole viewport:
        // every light in range walked per fragment, the sun, the occlusion
        // grid. `tests/cost.rs` measures it offline; this is the same pass
        // on the frame as played.
        let timed = profile::begin(window.gpu.as_ref(), "blit: lighting", encoder);
        window.blit.render(
            &window.device,
            &window.queue,
            encoder,
            blit::Frame {
                target: view,
                world: world_view,
                gbuffer: gbuffer_views,
                face_instances: window.statics.instances_buffer(),
                item_instances: window.items_pass.instances_buffer(),
                mobile_instances: window.mobile_pass.instances_buffer(),
                mesh_instances: window.mesh_pass.rows_buffer(),
                ground_instances: window.renderer.instances_buffer(),
                zoom: camera.zoom(),
                rect: viewport,
            },
            &geometry.lighting,
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
        if geometry.cutaway_instances.drawn != 0 {
            let timed = profile::begin(window.gpu.as_ref(), "cutaway lighting", encoder);
            window.blit.render_cutaway(
                &window.device,
                &window.queue,
                encoder,
                blit::Frame {
                    target: view,
                    world: cutaway_world_view,
                    gbuffer: cutaway_gbuffer_views,
                    face_instances: window.statics.cutaway_instances_buffer(),
                    item_instances: window.items_pass.instances_buffer(),
                    mobile_instances: window.mobile_pass.instances_buffer(),
                    mesh_instances: window.mesh_pass.rows_buffer(),
                    ground_instances: window.renderer.instances_buffer(),
                    zoom: camera.zoom(),
                    rect: viewport,
                },
                &geometry.lighting,
                1.0,
            );
            profile::end(window.gpu.as_ref(), encoder, timed);
        }
    }
    // The occlusion grid as solids, when somebody asked for it — step 23.0.
    // First of what is drawn over the lit picture, so the highlights stay on
    // top of it: a diagnostic must not hide the thing the cursor is naming.
    //
    // The grid drawn is the frame's **own** — `lighting.occlusion`, which is
    // the list the shader is walking this same frame — and not a second walk
    // of the map. A picture of a grid rebuilt beside the one in force would
    // be a claim about a grid nothing rendered.
    if graphics.show_solids {
        let standing = openshard_client_render::solid::standing(&geometry.lighting.occlusion, solid_cut);
        graphics.solids_held = standing.len();
        let timed = profile::begin(window.gpu.as_ref(), "solids", encoder);
        graphics.solids_drawn = window.solids.render(
            &window.device,
            &window.queue,
            encoder,
            solids::Frame {
                target: view,
                size: (window.config.width, window.config.height),
                rect: viewport,
            },
            &camera,
            &standing,
            solids::Style {
                opaque: graphics.solids_opaque,
                ..solids::Style::default()
            },
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    // The held selection's wash, first of the two things drawn over the lit
    // picture: the wall the click named and the ground it stands on. Under
    // the ring rather than over it, because they answer different questions
    // — the wash is what is *held* and the ring is what the cursor is on —
    // and the live one has to stay readable while it passes over the held
    // one.
    //
    // Skipped when nothing is selected, and the whole cost of a frame with
    // nothing selected is that comparison: the mask is not drawn either.
    if let Some(picked) = picking
        .selected
        .and_then(SelectedIdentity::as_static)
        .filter(|_| !geometry.select_quads.is_empty())
    {
        let timed = profile::begin(window.gpu.as_ref(), "selection wash", encoder);
        window.select.render(
            &window.device,
            &window.queue,
            encoder,
            select::Frame {
                target: view,
                mask: &select_view,
                ids: &gbuffer_views.ids,
                face_instances: window.statics.instances_buffer(),
                ground_instances: window.renderer.instances_buffer(),
                size: (render_width, render_height),
                rect: viewport,
            },
            // The tile the *static* stands on, and not `selected_tile`: the
            // ground being washed is the ground under the thing that was
            // picked, which is the whole of "and the tile it stands on". The
            // two are usually different tiles — a wall's picture stands up
            // the screen from its own cell, so the ground under the cursor is
            // the cell behind it.
            Selection::DEFAULT.on(openshard_movement::Tile::new(picked.at.x, picked.at.y)),
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    // And the ring on top of that, over the same rectangle — after the blit
    // so it is drawn in screen pixels and unlit: a highlight that dimmed at
    // night would stop working exactly when the picture is hardest to read.
    // Skipped entirely on the ordinary frame, where nothing is under the
    // cursor and the mask is empty. **Both silhouette lists**, or a ringed
    // creature draws its mask into a texture no pass ever reads and the
    // highlight is simply absent — which is what an item-only test of this
    // condition looked like from the outside.
    // The held ring, drawn first of the two so the live hover ring stays
    // on top and readable when the cursor is over the very thing that is
    // selected — the same ordering the wash and the hover ring keep,
    // and for the same reason. `Ring::SELECTED`'s own pipeline call: one
    // [`Ring`] per `Outline::render`, so the held ring's colour cannot be
    // the hover ring's even for one frame.
    if !geometry.held_item_outline.is_empty() || !geometry.held_mobile_outline.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "held ring", encoder);
        window.outline.render(
            &window.device,
            &window.queue,
            encoder,
            outline::Frame {
                target: view,
                mask: &held_view,
                mask_size: (render_width, render_height),
                rect: viewport,
            },
            Ring::SELECTED.for_zoom(camera.zoom()),
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    if !geometry.outline_quads.is_empty() || !geometry.mobile_outline.is_empty() {
        let timed = profile::begin(window.gpu.as_ref(), "outline ring", encoder);
        window.outline.render(
            &window.device,
            &window.queue,
            encoder,
            outline::Frame {
                target: view,
                mask: &mask_view,
                mask_size: (render_width, render_height),
                rect: viewport,
            },
            // The soft ring — an edge with a glow behind it — widened when
            // the world is minified, where one mask texel is less than one
            // screen pixel and a hairline breaks into a dashed line. See
            // `Ring::for_zoom`.
            Ring::SOFT.for_zoom(camera.zoom()),
        );
        profile::end(window.gpu.as_ref(), encoder, timed);
    }
    audit
}

// No tests here: the one that stood in this file measured a container's hover
// label, and both the label and its test moved into `panes::container` with
// the window that draws it. What is left in this module is recording, which
// is exercised by running the client.
