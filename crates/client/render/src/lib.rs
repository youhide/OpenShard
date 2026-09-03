//! The client's renderer: ground first, everything else after it.
//!
//! # What this crate is not allowed to do
//!
//! It does not open files, create windows, pick an adapter, spawn threads or
//! read a clock. Everything it needs arrives as an argument: decoded art, a
//! `wgpu::Device` and `Queue` somebody else asked for, and a texture view to
//! draw into. That is what lets the same code back a window on the desktop, a
//! canvas in a browser, and an offscreen texture in a test — and the last one is
//! the point. A renderer with no way to read its own output back has no oracle,
//! and `crates/common/uofiles` has already shown what tests without an oracle
//! are worth: every reader in it was green against fixtures its own
//! understanding had written, and the first real file broke two of them.
//!
//! # Browser-shaped from the start
//!
//! The web is a target, so every device request is `async` because a browser
//! cannot be blocked on — that is honoured here from the first triangle rather
//! than discovered later. The ceiling itself is **WebGPU, not WebGL2**
//! (`docs/archive/render/lighting.md` decision 30.5): real compute shaders and storage
//! buffers, because WebGPU is broadly shipped now and was only a flag behind
//! Chromium when the older, stricter ceiling was written. What that older
//! ceiling produced is not deleted — `Occlusion`'s texture-folded lookup
//! (decision 38.4) still runs and still works — it is simply no longer what
//! new code here has to route around.
//!
//! # Colour is not converted
//!
//! Textures and targets are `Rgba8Unorm`, not `Rgba8UnormSrgb`. The client's
//! files hold 5 bits per channel with no colour space attached, and the moment a
//! gamma conversion enters, a pixel that went into the atlas is not the pixel
//! that comes out of the frame — which would make an exact assertion in a test
//! impossible and replace it with a tolerance nobody can justify.

pub mod animate;
pub mod animation;
pub mod arttable;
pub mod atlas;
pub mod bench;
pub mod blit;
pub mod camera;
pub mod chart;
pub mod chunk_cache;
pub mod composite;
pub mod confirm;
pub mod container;
pub mod control;
pub mod cutaway;
pub mod debug;
pub mod depth;
pub mod doors;
pub mod dump;
pub mod effects;
pub mod facing;
pub mod follow;
pub mod frame;
pub mod gbuffer;
pub mod geometry;
pub mod ground;
pub mod gump;
pub mod hue;
pub mod impostor;
pub mod interiors;
pub mod items;
pub mod light;
pub mod lock;
pub mod lod;
pub mod mesh;
pub mod mesh_face;
pub mod mobiles;
pub mod occlusion;
pub mod outline;
pub mod paperdoll;
pub mod party;
pub mod place;
pub mod plan;
pub mod png;
pub mod radar;
pub mod radar_pass;
pub mod renderer;
pub mod scene;
pub mod select;
pub mod skills;
pub mod solid;
pub mod solids;
pub mod spellbook;
pub mod split;
pub mod sprite;
pub mod statics;
pub mod status;
pub mod text;
pub mod tonemap;
pub mod tooltip;
pub mod vendor;
