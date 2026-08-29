//! The GPU passes a frame's already-collected geometry is recorded through:
//! [`encode_world_passes`] is the world — ground, statics, mobiles, the
//! masks and rings — and [`draw_gump_windows`] is this client's own dialogs,
//! drawn over it in the client's own art. Neither decides what is drawn;
//! `crate::frame_geometry` and `App::draw_from` do that, and hand the
//! answer here to be recorded.

use openshard_client_render::blit::{self, ViewportRect};
use std::collections::BTreeSet;
use std::sync::OnceLock;

use openshard_client_render::camera::Camera;
use openshard_client_render::composite::{
    CompositeProducerJob, CompositeQuad, ImmutableRevision, MapBlockBounds,
};
use openshard_client_render::geometry::Rect;
use openshard_client_render::gump::{self as gump_art};
use openshard_client_render::lod::BlockLod;
use openshard_client_render::outline::{self, Ring};
use openshard_client_render::radar::{self, RadarCache, RadarLod, RadarView};
use openshard_client_render::radar_pass::{RadarChunkRenderer, RadarMarker, RadarOverlayRenderer};
use openshard_client_render::renderer::Target;
use openshard_client_render::select::{self, Selection};
use openshard_client_render::sprite::SpriteQuad;
use openshard_client_render::{paperdoll, solids};
use openshard_map::grid::BlockCoord;
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
use crate::windows::{Drawn, WindowSubject};
use crate::{graphics, panes, profile, resources, shell, windows, world};

/// Opt-in minimap geometry probe. The paired margin override is documented in
/// `panes::minimap`.
fn minimap_diagnostics() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OPENSHARD_MINIMAP_DIAGNOSTIC").is_some())
}

#[allow(clippy::too_many_arguments)]
fn draw_radar_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    chunks: &mut RadarChunkRenderer,
    overlay: &mut RadarOverlayRenderer,
    encoder: &mut wgpu::CommandEncoder,
    frame: gump_art::Frame<'_>,
    cache: &RadarCache,
    view: RadarView,
    lod: RadarLod,
    player: Option<openshard_protocol::world::Point>,
) {
    let region = view.region();
    let ready: Vec<_> = radar::region_chunks(region, lod)
        .filter_map(|chunk| cache.select_ready(cache.key(region.facet(), lod, chunk)))
        .map(|ready| ready.chunk())
        .collect();
    let map = view.map_placement();
    overlay.render_backdrop(device, queue, encoder, frame, view.placement, radar::UNKNOWN);
    chunks.render_region(device, queue, encoder, frame, region, map, view.placement, ready);
    if let Some(player) = player {
        overlay.render_markers(
            device,
            queue,
            encoder,
            frame,
            region,
            map,
            view.placement,
            &[RadarMarker {
                tile: radar::RadarTile::new(u32::from(player.x), u32::from(player.y)),
                color: radar::PLAYER_MARKER,
            }],
        );
    }
}

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
///
/// `scale` is how big every window here is drawn, from the desk — see
/// [`crate::desk::WindowScale`]. It reaches the surface through
/// `gump::place` and the pointer through
/// [`OwnWindow::local_cursor`](windows::OwnWindow::local_cursor), and the two
/// have to be the same number on the same frame or a click lands where the
/// picture is not.
// Named individually on purpose — see the doc above: reaching them through
// `&mut self` is what this function exists to avoid.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_gump_windows(
    resources: &mut resources::Resources,
    world: &world::WorldState,
    windows: &mut windows::Windows,
    radar_cache: &RadarCache,
    radar_views: &[(WindowSubject, RadarView, RadarLod)],
    cursor: gump_art::GumpPixel,
    hover: &[String],
    scale: crate::desk::WindowScale,
    fonts: crate::desk::FontSizes,
    ttf_active: bool,
    bitmap_font_override: Option<openshard_protocol::speech::Font>,
    shell: Option<&shell::Shell>,
    window: &mut Screen,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) {
    // Read once for the whole pass: every window on this frame is placed with
    // it, and a pane's own cursor was divided by it a moment ago.
    let magnify = scale.factor();
    // `drawn_windows` is both the previous frame's hit-test map and the
    // source of the text overlay assembled later in `App::draw_from`.  It
    // must therefore describe *this* frame's art pass, including the frame in
    // which the pass cannot run.  Keeping yesterday's layout here used to
    // leave an invisible window intercepting clicks (and its labels still
    // eligible for the text pass) after gump assets or their GPU pass were
    // unavailable.
    if let Some(files) = resources.gumps.as_ref().filter(|_| window.gump_pass.is_some()) {
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
            // first because a pane holds the atlas borrowed while it answers
            // (`panes::PaneFiles::gump_atlas`) — packing inside the walk would
            // be growing the very thing the walk is holding.
            let hand = windows.hand;
            let wanted: Vec<(WindowSubject, Vec<gump_art::GumpArt>)> = windows
                .own_windows
                .iter()
                .map(|open| {
                    let frame = panes::PaneFrame {
                        view,
                        // The install, narrowed to what a window may read — see
                        // `panes::PaneFiles`. Built per builder rather than once for
                        // the pass: it borrows `resources`, and the packing sweep
                        // between the two grows the atlas.
                        files: panes::PaneFiles::of(resources),
                        // The pointer, in this window's own gump pixels — see
                        // `PaneFrame::cursor`'s doc for why this is the one
                        // arithmetic a pane never has to do for itself, and
                        // `OwnWindow::local_cursor` for why all three callers
                        // ask the window rather than each subtracting for
                        // themselves.
                        cursor: open.local_cursor(cursor, scale),
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
                    // The install, narrowed to what a window may read — see
                    // `panes::PaneFiles`. Built per builder rather than once for
                    // the pass: it borrows `resources`, and the packing sweep
                    // between the two grows the atlas.
                    files: panes::PaneFiles::of(resources),
                    cursor: open.local_cursor(cursor, scale),
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
                    // Laid out by `panes::minimap::MinimapPane` above, the
                    // same as a sheet's: it always has a layout, so its
                    // terrain is drawn from the second loop below, keyed by
                    // `Drawn::Minimap` rather than reached from here.
                    WindowSubject::Minimap => {}
                    // Like the minimap, its generated terrain is recorded in
                    // the specialised window pass below rather than as gump art.
                    WindowSubject::WorldMap => {}
                    // Laid out by `SpellbookPane`; the book's membership is
                    // held in the view and its page is ordinary gump art.
                    WindowSubject::Spellbook(_) => {}
                    // Laid out by `panes::split::SplitPane` above, and
                    // **unreachable**: it is the one kind that draws nothing out
                    // of the view at all — the frame, the bar and the number are
                    // all its own — so there is nothing that can go away
                    // underneath it.
                    WindowSubject::Split { .. } => {}
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
                    // Laid out by `panes::confirm::ConfirmPane` above, and
                    // reaching here is the shop's case once more: the shard can
                    // settle the question — a roster for an accepted invitation
                    // — between the frame that drew the plate and the reconcile
                    // that takes the window with it.
                    WindowSubject::Confirm(_) => {}
                    // Laid out by `panes::party::PartyPane` above, and reaching
                    // here is the last member leaving between the frame that
                    // drew the roster and the reconcile that takes the window.
                    WindowSubject::Party => {}
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
            // At the *negative* of where the pointer took hold of it, and
            // placed at the cursor below with every window's own magnification
            // — see `gump::place`. `grab` is an offset inside the item's own
            // art (`hand::centre_of`), so it is in the same unmagnified pixels
            // a window is laid out in, and subtracting it here rather than
            // from the cursor is what makes the icon keep the same corner
            // under the hand at every scale. The icon is drawn as big as the
            // bag it came out of, which is the whole reason the scale is one
            // number for every window: an item that changed size in the
            // player's hand on the way between two of them would be this
            // preview disagreeing with both.
            pictures.push(
                gump_art::Picture::plain(
                    gump_art::GumpArt::Item(openshard_client_render::items::displayed_graphic(
                        drag.item.graphic,
                        drag.item.amount,
                    )),
                    gump_art::GumpPixel::new(-drag.grab.x, -drag.grab.y),
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
            window
                .gump_pass
                .as_mut()
                .expect("gump assets have their matching render pass")
                .upload_rows(&window.queue, resources.gump_atlas.pixels(), rows);
        }
        let frame = gump_art::Frame {
            target: view,
            width: window.config.width,
            height: window.config.height,
            scale: shell.map(|shell| shell.pixels_per_point()).unwrap_or(1.0),
        };
        // A window is one painter layer: its frame and icons, then the text
        // belonging to that frame, before the next window is allowed to cover
        // it.  `window_text` records the matching independent text pass below;
        // collecting every caption into the HUD's single buffer would put a
        // lower doll's name over every later gump.
        for (subject, drawn) in &windows.drawn_windows {
            // Every pane laid this window out window-local and at its art's
            // own size — see `PaneFrame::cursor`'s doc — so this is the one
            // place its pictures and its text become the screen-space geometry
            // a pass draws: magnified by the desk's scale and moved to the
            // window's own placement, both added back once, by `gump::place`.
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
            let mut art = gump_art::collect(drawn.pictures(), &resources.gump_atlas);
            gump_art::place(&mut art, at, magnify);
            window
                .gump_pass
                .as_mut()
                .expect("gump assets have their matching render pass")
                .render_layer(&window.device, &window.queue, encoder, frame, &art);
            // Each window's captions, in that window's own unmagnified gump
            // pixels, paired with the box the row is cut to when there is one.
            #[allow(clippy::type_complexity)]
            let mut labels: Vec<(
                openshard_client_render::text::GumpLabel<'_>,
                Option<gump_art::Scissor>,
            )> = Vec::new();
            // A server gump was authored for a fixed text face.  Its art and
            // coordinates follow the desk's window scale, but a TrueType
            // caption keeps the dedicated form size: stretching it again is
            // what made dense legacy layouts run out of their fields.
            let form = matches!(subject, WindowSubject::Dialog(_));
            match (subject, drawn) {
                // A shop's arm and a status frame's, now that a dialog's
                // captions are resolved by the pane that laid it out: this pass
                // reads what the layout produced and looks nothing up. It used
                // to reach into a `Dialogs` for the text table, the cliloc and
                // the typed contents of every field — the second half of the
                // window, worked out in a different place from the first.
                (WindowSubject::Dialog(_), Drawn::Dialog(laid_out)) => {
                    labels.extend(laid_out.lines.iter().map(|line| (line.label(), None)));
                }
                // A dialog's arm and a shop's: the pane resolved the name and
                // the hover label when it laid the window out, and this pass
                // reads what the layout produced and looks nothing up.
                (WindowSubject::Paperdoll(_), Drawn::Paperdoll(window)) => {
                    labels.extend(window.lines.iter().map(|line| (line.label(), None)));
                }
                (WindowSubject::Skills, Drawn::Skills(sheet)) => {
                    // The one kind whose rows are cut to a viewport, so the
                    // one kind that carries a scissor alongside its label.
                    // Both are in the sheet's own unmagnified pixels, and
                    // whichever face draws them puts the two into its own
                    // space together — see `window_text`.
                    labels.extend(sheet.lines.iter().map(|line| (line.label(), line.scissor)));
                }
                (WindowSubject::Spellbook(_), Drawn::Spellbook(book)) => {
                    labels.extend(book.lines.iter().map(|line| (line.label(), line.scissor)));
                }
                (WindowSubject::Status, Drawn::Status(status)) => {
                    labels.extend(status.lines.iter().map(|line| (line.label(), None)));
                }
                // One line, the number being chosen — the reference's own text
                // box, which is a control rather than a readout: it is where an
                // exact figure is typed into a pile the bar has no pixels for.
                (WindowSubject::Split { .. }, Drawn::Split(split)) => {
                    labels.extend(split.lines.iter().map(|line| (line.label(), None)));
                }
                (WindowSubject::Vendor(_), Drawn::Vendor(vendor)) => {
                    labels.extend(vendor.lines.iter().map(|line| (line.label(), None)));
                }
                // A bag's plate caption and its hover label, resolved by the
                // pane that laid the window out — the same shape as a
                // dialog's and a doll's. This arm used to work out both here:
                // which of the two plates a window has, tested twice against
                // the backpack and the vendor list, and the name of the icon
                // under the pointer, looked up in the view a second time. Both
                // are decided where they are drawn now.
                (WindowSubject::Container(_), Drawn::Container(window)) => {
                    labels.extend(window.lines.iter().map(|line| (line.label(), None)));
                }
                // The question itself, wrapped by the layout that placed the
                // plate — the same shape as every arm above: this pass reads
                // what the layout produced and looks nothing up.
                (WindowSubject::Confirm(_), Drawn::Confirm(window)) => {
                    labels.extend(window.lines.iter().map(|line| (line.label(), None)));
                }
                (WindowSubject::Party, Drawn::Party(window)) => {
                    labels.extend(window.lines.iter().map(|line| (line.label(), None)));
                }
                (WindowSubject::Minimap, Drawn::Minimap(bounds)) => {
                    if let Some(player) = world.authoritative.view.as_ref().map(|view| view.player.position) {
                        // The view this window's terrain was *requested* with,
                        // handed over rather than worked out again — see
                        // `radar_views` in the signature. A window opened this
                        // frame has none yet, and its first frame draws its rim
                        // with no terrain in it, for the same reason it is not
                        // pickable until it has been drawn once.
                        if let Some((_, view, lod)) = radar_views
                            .iter()
                            .find(|(subject, _, _)| *subject == WindowSubject::Minimap)
                        {
                            draw_radar_view(
                                &window.device,
                                &window.queue,
                                &mut window.radar_chunks,
                                &mut window.radar_overlay,
                                encoder,
                                frame,
                                radar_cache,
                                *view,
                                *lod,
                                Some(player),
                            );
                            if minimap_diagnostics() {
                                window.radar_overlay.render_debug_map_bounds(
                                    &window.device,
                                    &window.queue,
                                    encoder,
                                    frame,
                                    view.map_placement(),
                                    view.placement,
                                );
                            }
                        }
                        let mut rim = gump_art::collect(&[bounds.frame], &resources.gump_atlas);
                        gump_art::place(&mut rim, at, magnify);
                        window
                            .gump_pass
                            .as_mut()
                            .expect("gump assets have their matching render pass")
                            .render_layer(&window.device, &window.queue, encoder, frame, &rim);
                    }
                }
                (WindowSubject::WorldMap, Drawn::WorldMap(bounds)) => {
                    labels.extend(bounds.lines.iter().map(|line| (line.label(), None)));
                    // Handed over, not rebuilt — see the minimap's arm above.
                    if let Some((_, view, lod)) = radar_views
                        .iter()
                        .find(|(subject, _, _)| *subject == WindowSubject::WorldMap)
                    {
                        let player = world.authoritative.view.as_ref().map(|view| view.player.position);
                        draw_radar_view(
                            &window.device,
                            &window.queue,
                            &mut window.radar_chunks,
                            &mut window.radar_overlay,
                            encoder,
                            frame,
                            radar_cache,
                            *view,
                            *lod,
                            player,
                        );
                    }
                }
                _ => {}
            }
            // Most lines are window captions, but a pile's quantity is its
            // own text role.  In particular, a TrueType atlas keys glyphs by
            // size, so handing this one mixed list `fonts.window` would make
            // the quantity use the ordinary window face size despite its
            // `STACK_COUNT_FONT`.  Keep contiguous runs together: that
            // preserves the pane's painter order (a hover label still comes
            // after the count beneath it) while each role reaches the atlas
            // at the size its control edits.
            let mut first = 0;
            while first < labels.len() {
                let count = labels[first].0.font == openshard_client_render::items::STACK_COUNT_FONT;
                let mut end = first + 1;
                while end < labels.len()
                    && (labels[end].0.font == openshard_client_render::items::STACK_COUNT_FONT) == count
                {
                    end += 1;
                }
                window_text(
                    WindowText {
                        labels: &labels[first..end],
                        at,
                        magnify,
                        density: frame.scale,
                        size: if count {
                            fonts.stack_count
                        } else if form {
                            fonts.form
                        } else {
                            fonts.window
                        },
                        bitmap_scale: if count {
                            fonts.bitmap_stack_count_scale()
                        } else if form {
                            fonts.bitmap_form_scale()
                        } else {
                            fonts.bitmap_window_scale()
                        },
                        font_magnify: if form { 1.0 } else { magnify },
                        bitmap_font_override,
                    },
                    ttf_active.then_some(resources.ttf_font.as_ref()).flatten(),
                    &resources.font_atlas,
                    window,
                    encoder,
                    view,
                );
                first = end;
            }
        }
        let mut dragged = gump_art::collect(&pictures, &resources.gump_atlas);
        // Placed at the cursor rather than at a window's corner: the icon is
        // held by the pointer. Its own coordinate is the negative of where it
        // was grabbed — see where it was pushed — so this magnifies that grab
        // offset with the picture it belongs to.
        gump_art::place(&mut dragged, cursor, magnify);
        window
            .gump_pass
            .as_mut()
            .expect("gump assets have their matching render pass")
            .render_layer(&window.device, &window.queue, encoder, frame, &dragged);
        // How many are on the cursor, in the corner of the icon that is on it
        // — the third and last place a pile is counted, and the same rule and
        // the same corner as the two before it: `container::amount_label`
        // places all three. A partial lift is exactly when a person needs to
        // read this, and it is the one moment the bag they took it out of no
        // longer shows the number.
        //
        // Its own layer after the icon rather than inside `pictures`, because
        // digits are glyphs: the art pass draws pictures and the text pass
        // draws text, which is the split every window above already has.
        if let Some(drag) = windows
            .hand
            .filter(|hand| hand.pending_drop().is_none())
            .map(crate::hand::Hand::drag)
        {
            if let Some((at, count)) = openshard_client_render::container::amount_label(
                drag.item.graphic,
                drag.item.amount,
                gump_art::GumpPixel::new(-drag.grab.x, -drag.grab.y),
                &resources.tiledata,
                &resources.gump_atlas,
                &resources.font_atlas,
            ) {
                // Placed at the cursor the way the icon above is, and drawn
                // through the same `window_text` every caption goes through —
                // so a count on the pointer is the same face at the same real
                // size as a count in the bag it came out of.
                window_text(
                    WindowText {
                        labels: &[(
                            openshard_client_render::text::GumpLabel {
                                at,
                                text: &count,
                                font: openshard_client_render::items::STACK_COUNT_FONT,
                                hue: openshard_protocol::wire::Hue::STACK_COUNT,
                                clip: None,
                            },
                            None,
                        )],
                        at: cursor,
                        magnify,
                        density: frame.scale,
                        size: fonts.stack_count,
                        bitmap_scale: fonts.bitmap_stack_count_scale(),
                        font_magnify: magnify,
                        bitmap_font_override,
                    },
                    ttf_active.then_some(resources.ttf_font.as_ref()).flatten(),
                    &resources.font_atlas,
                    window,
                    encoder,
                    view,
                );
            }
        }
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
            let step = tooltip_line_step(
                &resources.font_atlas,
                ttf_active.then_some(fonts.tooltip),
                bitmap_font_override.unwrap_or(TOOLTIP_FONT),
                fonts.bitmap_tooltip_scale(),
            );
            let labels: Vec<_> = hover
                .iter()
                .enumerate()
                .map(|(row, line)| {
                    (
                        openshard_client_render::text::GumpLabel {
                            at: cursor.offset(gump_art::GumpPixel::new(
                                TOOLTIP_OFFSET.x,
                                TOOLTIP_OFFSET.y + step * row as i32,
                            )),
                            text: line.as_str(),
                            font: TOOLTIP_FONT,
                            hue: openshard_protocol::wire::Hue::LABEL,
                            clip: None,
                        },
                        None,
                    )
                })
                .collect();
            // At the cursor and **not magnified**: a tooltip is not part of any
            // window, so `desk::WindowScale` has nothing to say about it — the
            // lines are already laid out in gump pixels around the pointer, and
            // the display's density is the only thing left to apply.
            window_text(
                WindowText {
                    labels: &labels,
                    at: gump_art::GumpPixel::new(0, 0),
                    magnify: 1.0,
                    density: frame.scale,
                    size: fonts.tooltip,
                    bitmap_scale: fonts.bitmap_tooltip_scale(),
                    font_magnify: 1.0,
                    bitmap_font_override,
                },
                ttf_active.then_some(resources.ttf_font.as_ref()).flatten(),
                &resources.font_atlas,
                window,
                encoder,
                view,
            );
        }
    } else {
        windows.drawn_windows.clear();
    }
}

/// One window's captions, and everything needed to put them on the screen.
///
/// A struct rather than seven arguments, and the fields are what
/// `docs/text_sizes.md`'s D4 is made of: `magnify` and `density` are the two
/// things a caption is scaled by, and the whole point is that they reach the
/// *rasterizer* rather than the finished glyph.
struct WindowText<'a> {
    /// The captions in this window's own unmagnified gump pixels, each with
    /// the box its row is cut to when it has one.
    labels: &'a [(
        openshard_client_render::text::GumpLabel<'a>,
        Option<gump_art::Scissor>,
    )],
    /// Where the window's own corner sits, in gump pixels.
    at: gump_art::GumpPixel,
    /// How much bigger than its art the window draws — `desk::WindowScale`.
    magnify: f32,
    /// The factor applied to the TrueType raster size.  A server form keeps
    /// this at one while its coordinates still use `magnify` above.
    font_magnify: f32,
    /// The display's own density, which the gump pass would otherwise apply in
    /// its shader.
    density: f32,
    /// The size captions are drawn at, before either of the two above.
    size: openshard_client_render::atlas::TextSize,
    /// The matching `fonts.mul` scale for this role.
    bitmap_scale: f32,
    /// The active F1 classic-face override, if any.  Role classification is
    /// done before this replacement, so an overridden pile count keeps the
    /// count's scale even though it uses the selected glyphs.
    bitmap_font_override: Option<openshard_protocol::speech::Font>,
}

/// Draw one window's captions, through whichever face this client is running.
///
/// **The two faces are placed differently on purpose**, and that is the whole
/// of this function:
///
/// - `fonts.mul` is a bitmap face. Its glyphs are magnified the way the art
///   beside them is — a caption drawn at a different scale from the plate it
///   sits on would slide off it by a pixel per pixel of magnification — so the
///   quads are collected in the window's own pixels and `gump::place` scales
///   both position and size, exactly as the art pass did a moment ago.
/// - A TrueType face has a real size. It is rasterized at
///   `size × magnify × density` and drawn one texel to one pixel: the
///   *position* still moves with the window's magnification, the glyph does
///   not stretch with it. Blowing up a 14-pixel glyph to 28 is the soft,
///   sawtoothed line `text::collect_gump_ttf`'s doc describes; asking
///   `fontdue` for 28 is a 28-pixel glyph. See `docs/text_sizes.md`.
///
/// A row's scissor follows its label into whichever space that is, so a skill
/// sheet's list is cut to its viewport either way.
fn window_text(
    text: WindowText<'_>,
    font: Option<&openshard_uofiles::ttf_font::TtfFont>,
    font_atlas: &openshard_client_render::atlas::FontAtlas,
    window: &mut Screen,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) {
    if text.labels.is_empty() {
        return;
    }
    // `fonts.mul` face 9 is the quantity face: compact and sans-serif, unlike
    // the ordinary caption face. A supplied TrueType face has only one family
    // and cannot represent that choice, so never let its all-text shortcut
    // replace the quantity labels in a container.
    let stack_count = text
        .labels
        .iter()
        .all(|(label, _)| label.font == openshard_client_render::items::STACK_COUNT_FONT);
    if stack_count || font.is_none() || window.ttf_atlas.is_none() {
        let mut quads = Vec::new();
        for (label, scissor) in text.labels {
            let label = openshard_client_render::text::GumpLabel {
                font: text.bitmap_font_override.unwrap_or(label.font),
                ..*label
            };
            let mut line =
                openshard_client_render::text::collect_gump(std::slice::from_ref(&label), font_atlas);
            // Cut in the window's own pixels, before the placement below
            // magnifies what survived: a row and its viewport are both the
            // sheet's own unmagnified coordinates, and cutting after would be
            // cutting a glyph whose texel grid has already been stretched.
            if let Some(scissor) = scissor {
                scissor.cut(&mut line);
            }
            if text.bitmap_scale != 1.0 {
                for quad in &mut line {
                    quad.rect.x = label.at.x as f32 + (quad.rect.x - label.at.x as f32) * text.bitmap_scale;
                    quad.rect.y = label.at.y as f32 + (quad.rect.y - label.at.y as f32) * text.bitmap_scale;
                    quad.rect.width *= text.bitmap_scale;
                    quad.rect.height *= text.bitmap_scale;
                }
            }
            // `gump::place` below must still magnify the label's anchor with
            // the form, but a fixed-size form face must not inherit that same
            // enlargement.  Shrink the glyph offsets and dimensions in local
            // space first; placement restores the anchor's scale and leaves
            // the glyph itself at `font_magnify`.
            let relative_scale = text.font_magnify / text.magnify;
            if relative_scale != 1.0 {
                for quad in &mut line {
                    quad.rect.x = label.at.x as f32 + (quad.rect.x - label.at.x as f32) * relative_scale;
                    quad.rect.y = label.at.y as f32 + (quad.rect.y - label.at.y as f32) * relative_scale;
                    quad.rect.width *= relative_scale;
                    quad.rect.height *= relative_scale;
                }
            }
            quads.extend(line);
        }
        gump_art::place(&mut quads, text.at, text.magnify);
        window.gump_text_pass.render_layer(
            &window.device,
            &window.queue,
            encoder,
            gump_art::Frame {
                target: view,
                width: window.config.width,
                height: window.config.height,
                scale: text.density,
            },
            &quads,
        );
        return;
    }
    let (Some(font), Some(atlas)) = (font, window.ttf_atlas.as_mut()) else {
        unreachable!("the bitmap branch returned unless both TrueType resources exist")
    };
    let size = text.size.scaled(text.font_magnify * text.density);
    if let Err(error) = atlas.add_or_reset(
        font,
        size,
        text.labels.iter().flat_map(|(label, _)| label.text.chars()),
    ) {
        // `eprintln!` and a window drawn anyway, the same corner every other
        // atlas cuts on a failure — see `docs/client.md`.
        eprintln!("packing ttf glyphs: {error}");
    }
    // Window pixels to real ones, once: the window's own magnification and
    // then the display's density, both applied to the *position* only.
    let real = |point: gump_art::GumpPixel| {
        gump_art::GumpPixel::new(
            ((point.x as f32 * text.magnify + text.at.x as f32) * text.density).round() as i32,
            ((point.y as f32 * text.magnify + text.at.y as f32) * text.density).round() as i32,
        )
    };
    let mut quads = Vec::new();
    for (label, scissor) in text.labels {
        let mut line = openshard_client_render::text::collect_gump_ttf(
            &[openshard_client_render::text::GumpLabel {
                at: real(label.at),
                ..*label
            }],
            atlas,
            size,
        );
        if let Some(scissor) = scissor {
            // The same box in the same space the glyphs are now in, so a row
            // is cut where the list's edge actually is on the screen.
            gump_art::Scissor {
                at: real(scissor.at),
                width: (scissor.width as f32 * text.magnify * text.density).round() as i32,
                height: (scissor.height as f32 * text.magnify * text.density).round() as i32,
            }
            .cut(&mut line);
        }
        quads.extend(line);
    }
    window.upload_ttf_dirty();
    window
        .ttf_gump_pass
        .as_mut()
        .expect("a TrueType atlas has its matching gump pass")
        .render_layer(
            &window.device,
            &window.queue,
            encoder,
            gump_art::Frame {
                target: view,
                width: window.config.width,
                height: window.config.height,
                // The TrueType quads were already collected in real pixels.
                scale: 1.0,
            },
            &quads,
        );
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
/// tooltip was drawn in another one. A capital `M`'s actual *ink* height is
/// the measure — transparent padding in a glyph cell does not count — plus two
/// pixels so consecutive lines do not touch. The fallback is only reachable
/// with a font atlas that packed no `M`, which is a broken `fonts.mul` rather
/// than a case to be right about.
fn tooltip_line_step(
    fonts: &openshard_client_render::atlas::FontAtlas,
    truetype: Option<openshard_client_render::atlas::TextSize>,
    bitmap_font: openshard_protocol::speech::Font,
    bitmap_scale: f32,
) -> i32 {
    // A TrueType face has a requested size, and the step is that size plus the
    // same two pixels of air. Bitmap faces have no continuous size, so their
    // real `M` ink below is the only honest measure.
    if let Some(size) = truetype {
        return size.pixels().round() as i32 + 2;
    }
    (fonts
        .glyph_ink_height(bitmap_font, b'M')
        .map_or(16, |height| i32::from(height) + 2) as f32
        * bitmap_scale)
        .round() as i32
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
        BlockCoord,
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
        &geometry.item_boxes,
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
    let selected_item_rings: Vec<&[SpriteQuad]> = geometry
        .selected_item_outline
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
        &selected_item_rings,
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
    // draws after the blit in the world-text layer instead — see
    // `presentation::WorldText` and its render call immediately after this
    // world layer.
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
            Selection::DEFAULT.on(openshard_map::grid::Tile::new(picked.at.x, picked.at.y)),
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
    if !geometry.selected_item_outline.is_empty() || !geometry.held_mobile_outline.is_empty() {
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
