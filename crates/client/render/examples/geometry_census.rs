//! How much of a neighbourhood's geometry was measured, and how much was invented.
//!
//! Every drawn static gets a box, and they are not all the same kind of claim.
//! Some are measured off the art — a fitted prism, a panel on the edge the
//! silhouette names. Some are what is left when the art would not say: a whole
//! tile, standing in for a shape nobody has. And some belong to pictures the
//! grid holds *nothing* for, which are given a box for the impostor anyway
//! (`docs/render/design_model.md` phase 6c's deliberate fallback) — a point of no
//! primitive with a measured face, which is where several defects on that
//! document's backlog came from.
//!
//! This counts them, on a real place, so that "what is still a crutch" is a
//! number rather than a memory. Reads the client's own files; no GPU.
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo run --release -p openshard-client-render \
//!     --example geometry_census -- 1501 1659 20
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use openshard_client_render::atlas::StaticAtlas;
use openshard_client_render::occlusion;
use openshard_protocol::wire::Graphic;
use openshard_uofiles::art::Art;

/// What kind of claim a picture's box is.
///
/// Ordered worst-to-best deliberately: the report reads top-down as "here is
/// what we are making up", which is the question this tool exists for.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Claim {
    /// The art would not say which edge it stands on, so the box is the whole
    /// tile. Nothing about this was measured.
    WholeTileUnread,
    /// A climbable the prism search could not fit — one whole-tile body, which
    /// `boxes_of` argues for by name.
    WholeTileClimbable,
    /// A body the art named no edge for, narrowed to the horizontal box its own
    /// base edge measured — `docs/render/design_footprints.md`'s S3. The height is still
    /// whatever `tiledata` gives, unmeasured: D1's other half.
    Footprint,
    /// A lid: the whole tile across, `LID_THICKNESS` deep. Measured, and a body
    /// like every other since `docs/render/design_frame_assembly.md`'s P4 step 1 — it has side faces
    /// with area and an interior a ray can be inside of. The one number it does
    /// not measure is that depth, which is `occlusion::LID_THICKNESS` — argued
    /// from the wire's resolution and the screen's, not from the art.
    Lid,
    /// One or two panels on the edges the silhouette named, each inset by
    /// `PANEL_THICKNESS` — a depth no art states.
    Panels,
    /// A fitted prism, one body a tread. The only box in this list whose shape
    /// came out of the picture.
    Prism,
}

impl Claim {
    fn name(&self) -> &'static str {
        match self {
            Claim::WholeTileUnread => "whole tile, the art would not say",
            Claim::WholeTileClimbable => "whole tile, a climbable that would not fit",
            Claim::Footprint => "a measured footprint, narrower than the whole tile",
            Claim::Lid => "a lid — measured, LID_THICKNESS deep",
            Claim::Panels => "panels on the named edges, PANEL_THICKNESS deep",
            Claim::Prism => "a fitted prism, one body a tread",
        }
    }

    fn measured(&self) -> bool {
        matches!(self, Claim::Prism | Claim::Panels | Claim::Lid | Claim::Footprint)
    }
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let mut args = std::env::args().skip(1);
    let cx: i32 = args.next().expect("x").parse().expect("x is a number");
    let cy: i32 = args.next().expect("y").parse().expect("y is a number");
    let radius: i32 = args.next().map_or(20, |v| v.parse().expect("radius"));

    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("Felucca");
    let tiledata = openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata.mul");
    let art = Art::open(&dir).expect("artLegacyMUL.uop");

    let mut graphics: Vec<Graphic> = Vec::new();
    for x in cx - radius..=cx + radius {
        for y in cy - radius..=cy + radius {
            for item in map.statics_at(x as u16, y as u16) {
                graphics.push(item.tile);
            }
        }
    }
    graphics.sort_unstable_by_key(|g| g.0);
    graphics.dedup();
    let packed = graphics.len();
    let atlas = StaticAtlas::build(&art, graphics.iter().copied()).expect("a quarter fits");

    let mut claims: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut measured = 0u32;
    let mut invented = 0u32;
    // A picture the grid stands nothing for, which still gets a box from
    // `boxes_of` for the impostor to meet. Counted apart from the claim, because
    // it is orthogonal: a CLEAR piece can have a beautifully fitted prism and
    // still be a point of no primitive.
    let mut clear = 0u32;
    let mut clear_with_height = 0u32;
    let mut total = 0u32;
    // Front A's own question: how much of the whole-tile stand-in is a roof,
    // which is not a box a height alone would fix — a sloped plate is not an
    // AABB. Counted apart from `claims` because it cuts across both whole-tile
    // variants, not just the unread one.
    let mut whole_tile = 0u32;
    let mut whole_tile_roof = 0u32;

    for x in cx - radius..=cx + radius {
        for y in cy - radius..=cy + radius {
            for item in map.statics_at(x as u16, y as u16) {
                let graphic = item.tile;
                let tile = tiledata.static_tile(graphic.0);
                let shape = occlusion::shape_of(
                    Some(openshard_client_render::atlas::StaticArt::Single(&atlas)),
                    graphic,
                );

                // `boxes_of`'s own order, and it has to stay its order: a
                // climbable or a **platform** whose art fits a prism is a body
                // (a stair, a table), a climbable that fits none is a whole
                // tile, a `BACKGROUND` piece is a lid, and everything else is
                // read off its silhouette.
                let body =
                    tile.flags.is_climbable() || (tile.flags.is_platform() && !tile.flags.is_background());
                let claim = if body && shape.prism.is_some() {
                    Claim::Prism
                } else if tile.flags.is_climbable() {
                    Claim::WholeTileClimbable
                } else if tile.flags.is_background() {
                    Claim::Lid
                } else {
                    match (shape.facing.is_some(), shape.footprint.is_some()) {
                        (true, _) => Claim::Panels,
                        (false, true) => Claim::Footprint,
                        (false, false) => Claim::WholeTileUnread,
                    }
                };

                total += 1;
                *claims.entry(claim.name()).or_default() += 1;
                match claim.measured() {
                    true => measured += 1,
                    false => invented += 1,
                }
                if matches!(claim, Claim::WholeTileUnread | Claim::WholeTileClimbable) {
                    whole_tile += 1;
                    whole_tile_roof += u32::from(tile.flags.is_roof());
                }

                if occlusion::opacity(graphic, tile) == occlusion::CLEAR {
                    clear += 1;
                    // The combination that produced this session's specks: no
                    // name in the grid, and a box with side faces to be met on.
                    // **Deeper than the invented minimum, not merely non-flat.**
                    // Since `docs/render/design_frame_assembly.md`'s P4 step 1 a lid is a box too, one
                    // `LID_THICKNESS` deep — a quarter of a real pixel at the
                    // deepest zoom — so `min.z != max.z` would now be true of
                    // every floor in the world and this share would jump without
                    // a single box having grown a face anybody can meet.
                    let mut has_height = false;
                    occlusion::boxes_of(x, y, item.z, tile, &shape, |_, _, solid| {
                        has_height |= solid.max.z - solid.min.z > occlusion::LID_THICKNESS;
                    });
                    clear_with_height += u32::from(has_height);
                }
            }
        }
    }

    let pct = |n: u32| 100.0 * f64::from(n) / f64::from(total.max(1));
    println!(
        "{total} statics on {}x{} tiles around ({cx}, {cy}), {packed} distinct graphics\n",
        radius * 2 + 1,
        radius * 2 + 1,
    );
    for (name, n) in &claims {
        println!("  {n:>6}  {:>5.1}%  {name}", pct(*n));
    }
    println!(
        "\n  {measured:>6}  {:>5.1}%  measured off the art in some way\n  {invented:>6}  {:>5.1}%  \
         a whole tile standing in for a shape nobody has",
        pct(measured),
        pct(invented),
    );
    println!(
        "\n  {clear:>6}  {:>5.1}%  a point of NO primitive — the grid holds nothing for them, and\n\
           {clear_with_height:>6}  {:>5.1}%  of those are handed a box with real height anyway,\n\
         \x20         which is a side face a fragment can be answered with and no id to excuse it",
        pct(clear),
        pct(clear_with_height),
    );
    let whole_tile_pct = |n: u32| 100.0 * f64::from(n) / f64::from(whole_tile.max(1));
    println!(
        "\n  {whole_tile:>6}  {:>5.1}%  whole-tile stand-ins (of {total}), of which\n\
           {whole_tile_roof:>6}  {:>5.1}%  ({:>5.1}% of the whole-tile share) are ROOF — a sloped\n\
         \x20         plate, not an AABB, so height alone would not retire them",
        pct(whole_tile),
        whole_tile_pct(whole_tile_roof),
        pct(whole_tile_roof),
    );
}
