//! What the person looking is asking to see: [`GraphicsSettings`].
//!
//! Every field here is a debug-view or lighting switch — toggled by a
//! function key, read by `App::draw` and `App::hud`, and none of it a fact
//! about the world itself: the world is walked identically whichever of these
//! is set. Pulled out of [`crate::App`] for that reason, the same one
//! `docs/lighting.md` decision 8 gives for keeping `light_view` "beside
//! `night` rather than inside the renderer".

use std::path::PathBuf;

use openshard_client_render::camera::TileBounds;
use openshard_client_render::debug::View;
use openshard_client_render::frame;
use openshard_client_render::impostor::Fringe;
use openshard_client_render::occlusion;

/// What the cursor is allowed to light up.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HighlightTarget {
    #[default]
    Auto,
    Items,
    Tiles,
}

/// How an item or creature says it is the one under the cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HighlightStyle {
    Hue,
    #[default]
    Outline,
    Both,
}

impl HighlightStyle {
    pub fn hues(self) -> bool {
        matches!(self, Self::Hue | Self::Both)
    }

    pub fn rings(self) -> bool {
        matches!(self, Self::Outline | Self::Both)
    }
}

/// What this run draws and how, as a person's own choices rather than a fact
/// about the world — see the module docs.
pub struct GraphicsSettings {
    /// Whether the architectural cutaway is disabled. This is a diagnostic
    /// switch for comparing the transparency bug with the normal view.
    pub cutaway_disabled: bool,
    /// Whether sprites overlapping the player's screen/body mask are kept
    /// opaque. This isolates the neighbour-transparency diagnostic from the
    /// architectural height cutaway above.
    pub body_overlap_transparency_disabled: bool,
    /// Whether the world is drawn as if it were night: dark ambient, and the
    /// fires on the map lighting what is around them. Toggled with F10.
    ///
    /// A local switch and not the shard's clock, because there is no time of day
    /// on the wire yet. When there is, this is the field it writes to and
    /// nothing below it changes — the ambient is already a colour per frame
    /// rather than a constant read by the shader.
    pub night: bool,
    /// What a fragment whose ray met no box is answered with — the **fringe**,
    /// [`Fringe`]. Cycled with F2, and handed to the statics pass every frame.
    ///
    /// Here rather than owned by that pass because this is where the keyboard
    /// is, which is the whole reason it is a key: two of the three states are
    /// answers this renderer measured and refused, and what a refusal on this
    /// track answers to is a person looking at two pictures of one instant.
    /// `docs/lighting_state.md`'s fringe entry carries the numbers, and
    /// `OPENSHARD_FRINGE` is the same switch for a run that starts in one state.
    pub fringe: Fringe,
    /// Whether a tile's ambient depends on how much of the sky its column can
    /// see: a room under a roof darker than the road outside it, before anything
    /// burns. Toggled with F6.
    ///
    /// **Off by default**, and that is a decision rather than an oversight. The
    /// sky field is a plan of its own — `docs/lighting_world.md` — and what it
    /// does is change the ambient of every tile in the frame, which is exactly
    /// the thing that must hold still while the pools of the point lights are
    /// being judged. A torch that looks wrong indoors is otherwise two questions
    /// at once, and the flat ambient is the honest baseline to compare against:
    /// see [`openshard_client_render::light::Ambient::flattened`].
    pub sky_field: bool,
    /// Whether the day has a sun in it: a direction, a wall's shadow lying
    /// across the street, and a lit patch on the floor behind a window. Toggled
    /// with F8, and ignored at night.
    ///
    /// Off by default, and that is not shyness: the sun's ray is walked for
    /// *every* ground pixel of a daylit frame, where firelight is walked only
    /// inside a pool. Until there is a measurement on Britain at the widest zoom
    /// — step 6 of `docs/lighting.md`, which is still open — the sky is a key
    /// somebody turns on rather than a cost every frame pays.
    pub sunlit: bool,
    /// Whether the player is carrying a light: a torch in the hand, throwing a
    /// beam the way the character is facing. Toggled with F7, and it does nothing
    /// in plain daylight, where the whole lighting pass is a copy.
    ///
    /// On by default, unlike the sun, and the cost is the reason the two differ:
    /// this is one more flame in a loop that already runs sixty-four of them, and
    /// a beam leaves that loop on a dot product for every fragment it does not
    /// point at. It is here at all because it is what makes a dark room
    /// *navigable* — the ambient floor is deliberately small, and without
    /// something in the hand a windowless cellar is a black rectangle with a
    /// character somewhere in it.
    ///
    /// A client-side guess, in the way `light::flame` is: nothing on the wire
    /// says a mobile is holding a torch. When the equipment layers are read for
    /// one, this is the field that answers from them and nothing below changes.
    pub lantern: bool,
    /// Which of the lighting pass's own values the blit draws instead of the
    /// frame. Cycled with F11, and [`View::Lit`] is the picture.
    ///
    /// Beside `night` rather than inside the renderer because it is a property of
    /// the person looking: the world is walked identically whichever view is on,
    /// and the field is written onto the frame's `Lighting` on its way to the
    /// blit. `docs/lighting.md`, decision 8.
    pub light_view: View,
    /// Which of the world's producers this client is drawing — the World tab's
    /// own boxes, and [`frame::Draw::EVERYTHING`] until somebody unticks one.
    ///
    /// Beside `light_view` for the same reason it is: a property of the person
    /// looking rather than of the world. The G-buffer holds one answer per pixel,
    /// so a person who wants to look at a wall a body is standing in front of
    /// cannot get it out of the picture — the body's pixels *are* the body's.
    /// This is how they ask for a frame the body is not in, with everything still
    /// standing in the grid and still casting its own shadow.
    pub drawing: frame::Draw,
    /// Where the next frame writes itself out, when somebody has pressed F12.
    ///
    /// **`docs/parity.md`'s first backlog item.** The tools could always dump a
    /// picture and the client — the thing that is actually broken — could dump
    /// nothing, so every artefact a person reported was *searched for* in a
    /// tool's frame rather than compared against this one's. A defect visible in
    /// one and absent in the other says nothing about either until both can be
    /// laid side by side.
    ///
    /// Armed here and spent in `App::draw`, because what is dumped has to be a
    /// frame drawn the ordinary way through the ordinary passes. A dump
    /// assembled on the spot for the camera would be a fourteenth assembly, and
    /// a picture of one.
    pub frame_dump: Option<PathBuf>,
    /// How many have been written this run — the number in each dump's own
    /// directory name, so a second press does not overwrite the picture the
    /// first one was taken to compare against.
    pub frame_dumps: u32,
    /// Whether the HUD is drawing what `common/movement` thinks of the ground —
    /// see `App::terrain_overlay`.
    ///
    /// Off by default and paid for only while it is on: the overlay asks a
    /// walkability question of every tile in view and plans a route every frame,
    /// which is a bill worth a debugging picture and not worth a frame nobody is
    /// looking at.
    pub show_terrain: bool,
    /// Whether the development overlay draws the R1 interior index. It does
    /// not gate ordinary geometry until R2 explicitly makes it an input.
    pub show_interiors: bool,
    /// Whether the HUD is drawing the lighting's occlusion grid as boxes — see
    /// `shell::draw_occluders` and `docs/lighting.md`, step 14.
    ///
    /// Off by default and paid for only while it is on, like the terrain
    /// overlay: the grid is a second walk of the map's statics over the same
    /// bounds the frame's lighting walks a moment later.
    pub show_occluders: bool,
    /// Whether the HUD is drawing the same grid as *solids* — decision 39 and
    /// step 23.0 of `docs/lighting.md`, and `shell::draw_solids`.
    ///
    /// Beside the wireframe rather than instead of it, and that is the design: a
    /// solid hides what stands behind it and a wireframe shows it, so the two
    /// answer different halves of "is the geometry where I think it is". Both on
    /// at once is a legitimate reading and is what the outline over a filled face
    /// is for.
    pub show_solids: bool,
    /// Whether the world image is skipped while solids are drawn — boxes alone
    /// over a blank frame, with no sprite between their faces to compare them
    /// against. F5's own picture is deliberately the opposite of this
    /// (decision 39.2, "the wall's sprite is visible inside the box that
    /// claims to contain it"); this is for reading the box's own shape without
    /// the art arguing with it.
    pub solids_only: bool,
    /// Whether the solids view's fills are a straight overwrite instead of
    /// blended in — `solids::Style::opaque`, threaded through from
    /// [`shell::Request::solids_opaque`]. Off by default, matching the
    /// translucent fill the view always drew before this existed.
    pub solids_opaque: bool,
    /// Whether either of those two views draws the **whole** grid rather than
    /// what stands above the player's feet — the second datum, and the enum it
    /// resolves to is [`solid::Cut`](openshard_client_render::solid::Cut).
    ///
    /// A `bool` here and an enum there, and the split is deliberate: what a
    /// person picks is one of two questions and holds across frames, while the
    /// cut in force carries the player's own `z` and is a fact about the frame
    /// it is drawn in. `App::solid_cut` is the one place the two are joined,
    /// so a stale `z` cannot be stored anywhere.
    ///
    /// Off, because the whole grid over a town is unreadable — a pier is a slab
    /// on every plank — and the readable answer is the one a person should get
    /// without asking. What it costs to have it be a switch at all is the
    /// backlog entry it closes: a hole in a floor and a floor below the cut are
    /// the same picture, and no count beside the checkbox can tell them apart.
    pub solids_everything: bool,
    /// How many solids the last frame's pass was handed, and how many of those
    /// it drew — the rest fell outside the viewport.
    ///
    /// Kept and shown because a view that quietly draws a fraction of a grid
    /// looks exactly like a grid with little in it, which is
    /// [`Surface::stands`](openshard_client_render::occlusion::Surface::stands)'
    /// argument applied to the other end of the same picture. Zero on a frame
    /// the view was off, which the checkbox beside it already says.
    pub solids_held: usize,
    /// The half of that pair that reached the target.
    pub solids_drawn: usize,
    /// The blocks of the occlusion grid built for earlier frames — see
    /// [`occlusion::bake`] and `docs/lighting.md`'s step 21.5.
    ///
    /// Owned by the app because it is the app that has more than one frame. It
    /// is one map's, and this client has one map; a second facet would want a
    /// second bake, and the field being here rather than global is what would
    /// make that a change to one line.
    pub occlusion_bake: occlusion::bake::Bake,
    /// What the cursor is allowed to light up, and how an item says it is the
    /// one lit. Both are the HUD's to set.
    pub highlight: HighlightTarget,
    pub highlight_style: HighlightStyle,
    /// The tile rectangle whose land and statics have been offered to the
    /// atlases, or `None` when nothing has.
    ///
    /// The state the band walk in `App::draw` is built on, and the one thing
    /// here that is wrong in silence: an atlas rebuilt behind this field's back
    /// forgets graphics that this still claims were offered, and the tiles that
    /// needed them simply stop being drawn — along one edge, at one camera
    /// position. So it is set from exactly two places, both of which have just
    /// finished packing, and cleared before anything that forgets.
    pub covered: Option<TileBounds>,
}
