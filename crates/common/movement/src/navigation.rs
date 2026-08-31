//! A bounded, single-level navigation graph.
//!
//! Static terrain is represented by 32×32 regions.  Obstacles inside a region
//! stay local to live refinement; only crossings between region components
//! become abstract choices.
//!
//! # A node is a place to stand, and an edge goes one way
//!
//! Both halves of `docs/map/navigation_spans.md`'s N4, and they are one repair.
//! The graph used to sample **one height per tile** — `ground_z`, the land alone
//! — so a tile whose only surface is a static floor came back at the ground
//! beneath it. Britain's castle plateau is land at z=30 over a city at z=0 and
//! the stairs between them are statics the sampler never saw, so the plateau was
//! an island in a graph whose own map said otherwise. It samples [`Places`] now:
//! every standing surface the map's spans offer, so a bridge deck and the road
//! under it are two nodes rather than one.
//!
//! And a crossing used to become a portal only where the step succeeded in
//! **both** directions, while the step rule is asymmetric by design — a climb
//! reaches `start_top + 2` and a descent is unbounded. Every ledge a body may
//! step off and not climb back onto was therefore deleted from the graph. A
//! crossing is one direction now and its reverse is its own edge, which is what
//! makes a one-way drop representable at all.

use std::cmp::Reverse;
use std::collections::{
    BTreeMap,
    BinaryHeap,
    VecDeque,
};
use std::time::Instant;

use openshard_map::chunk::{
    CHUNK_TILES,
    ChunkCoord,
};
use openshard_map::grid::Tile;
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use rustc_hash::FxHashMap;

use crate::footing::Footing;
use crate::walk::steps_out_of;
use crate::{
    Effort,
    Rigour,
    Weight,
    can_stand,
    debug_enabled,
    destination_place,
    find_path_toward_within,
    find_path_within,
    step_allowed,
};

/// The whole work one long query may do, in node expansions.
///
/// **The ceiling that used to be a clock.** An ordinary long query is two
/// region floods and up to nine refinement passes over a corridor; an endpoint
/// on a runtime storey substitutes a bounded directed live flood for its region
/// flood. What kept the sum off the tick was 50 ms of wall clock read inside
/// each — see [`Effort`] for why that was the wrong instrument in three separate
/// ways. This is the same ceiling in the unit the searches under it are already
/// written in, so what a query may cost stops depending on how busy the machine
/// is.
///
/// **The number is measured, not converted.** Converting the old one would give
/// ~200,000 — 50 ms at the ~250 ns an expansion costs — and nothing comes near
/// even a fraction of that. Over `coarse_bench`'s six distance bands from two
/// origins on facet 0, 87 long queries out to a quarter of the facet:
///
/// | | castle (1363, 1600) | open country (1500, 1900) |
/// |---|---|---|
/// | cheapest query | 330 | 1,258 |
/// | median | 1,746 | 2,111 |
/// | p95 | 3,450 | 4,022 |
/// | dearest | 4,118 | 4,377 |
///
/// A ceiling is not a budget, so it is set well above the sample rather than at
/// it: **23× the dearest query measured**, which leaves room for ground denser
/// than any in it — a query is bounded by its corridor and its retries long
/// before this is reached, and reaching it at all means something is wrong. What
/// keeps it honest as a *ceiling* is the other end: at ~250 ns a node it is
/// ~25 ms, half of what the clock it replaces allowed, so the worst case it
/// permits is a tick a shard can still absorb.
const LONG_PATH_EFFORT: usize = 100_000;

const WIDE_PORTAL: usize = 6;
/// A region stays well inside the normal 600-cell refinement budget, while the
/// whole facet has only a few thousand regions. Obstacles inside one are live
/// terrain, not graph boundaries, so a forest does not emit a node per tree.
const REGION_SIZE: u32 = 32;
const NO_COMPONENT: u16 = 0;
/// The fill of an unused neighbour slot, never read past its own length.
///
/// A `u32` since N4, because the thing being numbered is a region's *places*
/// rather than its cells: a 32×32 rectangle has a thousand of the one and can
/// have twelve thousand of the other, and a base set is a world nobody has
/// counted.
const NO_NEIGHBOR: u32 = u32::MAX;

#[derive(Debug, PartialEq, Eq)]
pub struct NavigationGraph {
    pub(crate) width:             u32,
    pub(crate) height:            u32,
    pub(crate) regions:           Vec<Region>,
    /// One bit per tile. Region ids are a regular 32×32 grid and are computed.
    pub(crate) walkable:          Vec<u8>,
    pub(crate) nodes:             Vec<Node>,
    /// Which nodes stand in each region, one [`Run`] per region into
    /// [`region_nodes`](Self::region_nodes).
    pub(crate) region_runs:       Vec<Run>,
    pub(crate) region_nodes:      Vec<u32>,
    /// Which edges leave each node, one [`Run`] per node into
    /// [`edge_targets`](Self::edge_targets) and its parallel costs.
    pub(crate) edge_runs:         Vec<Run>,
    pub(crate) edge_targets:      Vec<u32>,
    pub(crate) edge_costs:        Vec<u16>,
    /// Entries of [`nodes`](Self::nodes) no region names any more.
    pub(crate) dead_nodes:        u32,
    /// Entries of [`region_nodes`](Self::region_nodes) no run points at.
    pub(crate) dead_region_nodes: u32,
    /// Entries of [`edge_targets`](Self::edge_targets) no run points at.
    ///
    /// The garbage rule is the span layer's verbatim — never compacted during a
    /// session, until the dead outweigh the live — with one difference:
    /// `SpanIndex` answers that by baking the facet whole, and 11.6 s is the
    /// thing this graph's own rebake exists to stop paying. See
    /// [`repack`](NavigationGraph::repack).
    pub(crate) dead_edges:        u32,
}

/// Where one owner's entries sit in a packed array: `base..base + count`.
///
/// **A table, and not a prefix sum**, which is the third time this repo has made
/// that change and the same reason each time: a prefix sum *is* the ordering, so
/// re-laying one owner's run moves every run after it and repairs every offset
/// behind it. The span index's `BlockTable` and `WorldMap`'s `blocks` are the
/// other two; `docs/map/navigation_graph.md`'s G1 is the argument in full, and
/// what it buys here is that a publish rebuilds the regions around it instead of
/// dropping a graph that costs 11.6 s to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) base:  u32,
    pub(crate) count: u32,
}

impl Run {
    /// An owner with nothing of its own: a region no portal reaches, a node no
    /// edge leaves.
    const NONE: Self = Self { base: 0, count: 0 };

    fn range(self) -> std::ops::Range<usize> {
        self.base as usize..self.base as usize + self.count as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) left:   u16,
    pub(crate) top:    u16,
    pub(crate) width:  u16,
    pub(crate) height: u16,
}

impl Region {
    pub(crate) fn contains(self, point: Point) -> bool {
        let x = u32::from(point.x);
        let y = u32::from(point.y);
        x >= u32::from(self.left)
            && x < u32::from(self.left) + u32::from(self.width)
            && y >= u32::from(self.top)
            && y < u32::from(self.top) + u32::from(self.height)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegionId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) point: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) to:   NodeId,
    pub(crate) cost: u32,
}

/// The crossings of one region border, gathered by the logical entrance they
/// belong to.
///
/// The key is *directed* — the region and component a step starts in, then the
/// region and component it ends in — so the two ways across one border are two
/// entrances. Ordered, because a representative is chosen by position along the
/// run and a run assembled in a different order would choose a different one.
type Entrances = BTreeMap<(usize, u16, usize, u16), Vec<(Point, Point)>>;

/// One region's places, numbered from zero.
///
/// The two per-region passes — the component flood and the intra-region routes
/// — want a dense index over the places of one 32×32 rectangle, and the
/// facet-wide runs are not one. This is that numbering, built as a prefix sum
/// over the region's own cells: turning "which place did that step land on" into
/// two array reads and a walk of a one-element run, rather than a hash lookup on
/// a path taken once per neighbour of every place on the facet.
///
/// **A query builds one too**, since the endpoint join stopped being a fan-out
/// of exact searches and became a flood — see [`NavigationGraph::local_costs`].
/// A bake holds one per region it is working over ([`Held`]) and a query samples
/// the one region it needs; both go through [`Self::sampled`], which is what
/// makes a node of a region findable in the sampling a query takes of it.
struct RegionPlaces {
    region:  Region,
    /// `offsets[cell]..offsets[cell + 1]`, in the region's row-major cell order.
    offsets: Vec<u32>,
    /// Every place of the region, in that order.
    points:  Vec<Point>,
}

impl RegionPlaces {
    /// Sampled out of the ground on the spot.
    ///
    /// One region is a thousand columns and [`column_places`] is what the bake
    /// asked of each of them, so this reproduces the bake's own places over the
    /// same footing — which is what lets a node of this region be found in it.
    /// It is the whole of what a join costs before the flood starts, and it is
    /// paid once per endpoint where the fan-out it replaced paid a bounded A\*
    /// per node.
    fn sampled(footing: &Footing<'_>, region: Region) -> Self {
        let cells = usize::from(region.width) * usize::from(region.height);
        let mut offsets = Vec::with_capacity(cells + 1);
        let mut points = Vec::with_capacity(cells);
        let mut column: Vec<i8> = Vec::with_capacity(16);
        offsets.push(0);
        for y in region.top..region.top + region.height {
            for x in region.left..region.left + region.width {
                column_places(footing, x, y, &mut column, &mut points);
                offsets.push(points.len() as u32);
            }
        }
        Self {
            region,
            offsets,
            points,
        }
    }

    fn len(&self) -> usize {
        self.points.len()
    }

    /// One column's places, as a run of the region's own numbers. Empty for a
    /// column outside the rectangle, which is how a flood stays inside it.
    fn column(&self, x: u16, y: u16) -> std::ops::Range<usize> {
        if !self.region.contains(Point::new(x, y, 0)) {
            return 0..0;
        }
        let cell = usize::from(y - self.region.top) * usize::from(self.region.width)
            + usize::from(x - self.region.left);
        self.offsets[cell] as usize..self.offsets[cell + 1] as usize
    }

    /// The region's own number for the place `at` names, or `None` when the step
    /// left the region.
    fn slot(&self, at: Point) -> Option<usize> {
        let run = self.column(at.x, at.y);
        let start = run.start;
        self.points[run]
            .iter()
            .position(|place| place.z == at.z)
            .map(|offset| start + offset)
    }

    /// The region's own number for the place `at`'s column offers nearest the
    /// height it names, with a tie going to the lower.
    ///
    /// **An endpoint is a point and a place is where feet are**, and the two
    /// are not the same thing: a body stands on the live world's surfaces and a
    /// join floods over the bare map, so the height a query arrives with is
    /// often a height this map lists nothing at. It is the same resolution
    /// [`goal_node`](crate::path) makes of a destination, and the same one the
    /// join's target side already went through — an exact search aimed at the
    /// endpoint aimed at the resolved place, not at the raw height. Where the
    /// two differ the body is not standing anywhere the bare map knows about,
    /// and what the join produces is a corridor the live refinement still has
    /// to approve.
    fn nearest_slot(&self, at: Point) -> Option<usize> {
        let run = self.column(at.x, at.y);
        let start = run.start;
        let wanted = i32::from(at.z);
        self.points[run]
            .iter()
            .enumerate()
            .min_by_key(|(_, place)| ((i32::from(place.z) - wanted).abs(), place.z))
            .map(|(offset, _)| start + offset)
    }
}

/// One region's places and the strong components they fall into — the unit both
/// a whole-facet bake and the rebake of a neighbourhood work in.
///
/// The two halves travel together because the portal pass wants both at once: a
/// crossing is filed under the components it joins, and the component on the far
/// side of a border belongs to the region on that side.
struct RegionBake {
    places: RegionPlaces,
    /// One label per place, in the region's own numbering. Bake-time scratch:
    /// the labels never enter the artifact.
    labels: Vec<u16>,
}

/// The regions a bake is holding sampled places for.
///
/// **A bake reads more regions than it rebuilds.** A portal is a fact about a
/// *border*, so every neighbour of a rebuilt region is sampled too, to be the
/// far side of one. For [`NavigationGraph::build`] that is every region of the
/// facet; for [`NavigationGraph::rebake_chunks`] it is the ring outside the ring
/// — `docs/map/navigation_graph.md`'s G1 calls it two rings and a half, and this
/// is the half.
struct Held {
    /// Which entry of [`bakes`](Self::bakes) a region is, or [`NOT_HELD`].
    slot:  Vec<u32>,
    bakes: Vec<RegionBake>,
}

/// The fill of [`Held::slot`] for a region nothing sampled.
const NOT_HELD: u32 = u32::MAX;

impl Held {
    /// Sample and label each region named, once however often it is named.
    fn of(footing: &Footing<'_>, graph: &NavigationGraph, regions: &[RegionId]) -> Self {
        let mut held = Self {
            slot:  vec![NOT_HELD; graph.regions.len()],
            bakes: Vec::with_capacity(regions.len()),
        };
        for &region in regions {
            if held.slot[region.0] != NOT_HELD {
                continue;
            }
            let places = RegionPlaces::sampled(footing, graph.regions[region.0]);
            let labels = component_labels(footing, &places);
            held.slot[region.0] = held.bakes.len() as u32;
            held.bakes.push(RegionBake { places, labels });
        }
        held
    }

    /// One held region. A caller that asks about a region nobody sampled is a
    /// caller that got its own area wrong, which is a bug rather than a state.
    fn at(&self, region: RegionId) -> &RegionBake {
        let slot = self.slot[region.0];
        debug_assert_ne!(slot, NOT_HELD, "a bake reads only the regions it sampled");
        &self.bakes[slot as usize]
    }
}

/// Which of the two borders a region owns is being walked.
///
/// Every border joins a region to the one east or south of it, so naming the
/// pair from its north-west side names each border exactly once however it was
/// reached — see [`NavigationGraph::borders_of`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    East,
    South,
}

/// One side of a border: which region, and what a bake sampled of it.
#[derive(Clone, Copy)]
struct Bank<'a> {
    id:   RegionId,
    bake: &'a RegionBake,
}

/// The fill of [`Rebuild::region_slot`] for a region whose node list stands.
const NOT_REBUILT: u32 = u32::MAX;
/// The fill of [`Rebuild::node_slot`] for a node whose edges are not being
/// rewritten.
const NOT_REWRITTEN: u32 = u32::MAX;

/// Everything one rebake is assembling, before any of it is written into the
/// graph's own arrays.
///
/// Assembled apart from the graph because a rebake has to be able to say
/// "these are the edges this node has now" rather than "these are edges this
/// node has as well": a border pass produces a node's portal edges from
/// scratch, and the ones it had before are exactly what must not survive.
struct Rebuild {
    /// Which entry of [`nodes`](Self::nodes) a region's new list is, or
    /// [`NOT_REBUILT`].
    region_slot: Vec<u32>,
    /// Whether the ground under a region is what moved. The rebuilt regions are
    /// these and their neighbours, and the difference is what the neighbours are
    /// allowed to keep: their places did not move, so their intra-region routes
    /// stand unless their node set came back different.
    moved:       Vec<bool>,
    nodes:       Vec<(RegionId, Vec<NodeId>)>,
    /// Which entry of [`edges`](Self::edges) a node's new list is, or
    /// [`NOT_REWRITTEN`].
    node_slot:   Vec<u32>,
    edges:       Vec<(NodeId, Vec<Edge>)>,
    /// One node per standing place, however many entrances name it — seeded
    /// with what the graph already holds, which is the whole of how a local
    /// rebake keeps a `NodeId` meaning what it meant.
    interned:    BTreeMap<(u16, u16, i8), NodeId>,
}

impl Rebuild {
    fn of(graph: &NavigationGraph, held: &Held, rebuilt: &[RegionId], moved: &[RegionId]) -> Self {
        let mut build = Self {
            region_slot: vec![NOT_REBUILT; graph.regions.len()],
            moved:       vec![false; graph.regions.len()],
            nodes:       Vec::with_capacity(rebuilt.len()),
            node_slot:   vec![NOT_REWRITTEN; graph.nodes.len()],
            edges:       Vec::new(),
            interned:    BTreeMap::new(),
        };
        for &region in moved {
            build.moved[region.0] = true;
        }
        for &region in rebuilt {
            if build.region_slot[region.0] != NOT_REBUILT {
                continue;
            }
            build.region_slot[region.0] = build.nodes.len() as u32;
            build.nodes.push((region, Vec::new()));
        }
        for (index, &slot) in held.slot.iter().enumerate() {
            if slot == NOT_HELD {
                continue;
            }
            let region = RegionId(index);
            let rebuilt = build.region_slot[index] != NOT_REBUILT;
            for node in graph.nodes_in_region(region).collect::<Vec<_>>() {
                let point = graph.nodes[node.0].point;
                build.interned.insert((point.x, point.y, point.z), node);
                // What a node keeps depends on which of the three tiers its
                // region is in, and each answer is a claim about what this
                // rebake is entitled to have an opinion about:
                //
                // - **the ground moved under it**: nothing. The passes below
                //   mint its portals and its intra-region routes again.
                // - **rebuilt because a neighbour moved**: its own
                //   intra-region edges, because its places did not move. Every
                //   border it has is walked again, so its portals are minted
                //   again — and if its node set comes back the same, the routes
                //   kept here are the answer and the floods are not run.
                // - **sampled only**, to be the far side of a border:
                //   everything that does not point into the rebuilt area, which
                //   is its own routes and its portals to the world beyond.
                let kept: Vec<Edge> = match (rebuilt, build.moved[index]) {
                    (true, true) => Vec::new(),
                    (true, false) => {
                        graph
                            .edges_from(node)
                            .filter(|edge| graph.node_region(edge.to) == region)
                            .collect()
                    }
                    (false, _) => {
                        graph
                            .edges_from(node)
                            .filter(|edge| build.region_slot[graph.node_region(edge.to).0] == NOT_REBUILT)
                            .collect()
                    }
                };
                build.node_slot[node.0] = build.edges.len() as u32;
                build.edges.push((node, kept));
            }
        }
        build
    }

    /// The new node list of a region being rebuilt, or `None` for one that keeps
    /// the list it has.
    fn listing(&mut self, region: RegionId) -> Option<&mut Vec<NodeId>> {
        let slot = self.region_slot[region.0];
        (slot != NOT_REBUILT).then(|| &mut self.nodes[slot as usize].1)
    }

    /// The same list, to read.
    fn listed(&self, region: RegionId) -> Option<&[NodeId]> {
        let slot = self.region_slot[region.0];
        (slot != NOT_REBUILT).then(|| self.nodes[slot as usize].1.as_slice())
    }

    /// Whether the ground under this region is what a publish moved, as against
    /// a region rebuilt because it is next to one that was.
    fn has_moved(&self, region: RegionId) -> bool {
        self.moved[region.0]
    }

    /// Put a node on its region's new list, if that region has one. A place is
    /// named by up to four entrances and each of them interns it, so the list is
    /// checked rather than appended to blindly.
    fn list(&mut self, region: RegionId, node: NodeId) {
        let Some(nodes) = self.listing(region) else {
            return;
        };
        if !nodes.contains(&node) {
            nodes.push(node);
        }
    }

    /// Start an edge list for a node that did not exist a moment ago.
    fn open(&mut self, node: NodeId) {
        debug_assert_eq!(self.node_slot.len(), node.0, "nodes are minted in order");
        self.node_slot.push(self.edges.len() as u32);
        self.edges.push((node, Vec::new()));
    }

    fn add_edge(&mut self, from: NodeId, to: NodeId, cost: u32) {
        self.edges_of(from).push(Edge { to, cost });
    }

    /// The list a node's edges are being assembled in.
    fn edges_of(&mut self, node: NodeId) -> &mut Vec<Edge> {
        let slot = self.node_slot[node.0];
        debug_assert_ne!(slot, NOT_REWRITTEN, "an edge is minted only where its node is");
        &mut self.edges[slot as usize].1
    }
}

/// Every step across a region border out of one tile, filed under the entrance
/// it belongs to.
///
/// One crossing per *place* on the tile, and one direction. The pair that used
/// to stand here asked the step both ways and kept it only where both succeeded
/// — which deleted every asymmetric border from the graph, and the step rule is
/// asymmetric by design.
///
/// A landing with no place listed is skipped rather than invented. Over the bare
/// map that cannot happen — [`column_places`] keeps a superset of every landing
/// — and over a footing with a live world in it, skipping is the conservative
/// answer a bake owes: a refusal, not a promise.
fn crossings(
    footing: &Footing<'_>,
    from: Bank<'_>,
    to: Bank<'_>,
    tile: Tile,
    direction: Direction,
    out: &mut Entrances,
) {
    for slot in from.bake.places.column(tile.x, tile.y) {
        let at = from.bake.places.points[slot];
        let Some(landing) = step_allowed(footing, at, direction) else {
            continue;
        };
        let Some(landed) = to.bake.places.slot(landing) else {
            continue;
        };
        out.entry((from.id.0, from.bake.labels[slot], to.id.0, to.bake.labels[landed]))
            .or_default()
            .push((at, landing));
    }
}

/// One column's places, appended to `out` in the order its spans are stored.
///
/// **The map's spans and not `ground_z`**, which is the first half of N4. The
/// old sampler took one height per tile and that height was `average_land_z` —
/// the land alone, with everything standing on it ignored — so a tile whose only
/// surface is a static floor came back at the ground beneath it, and a
/// thirty-unit cliff then refused every step onto the castle plateau.
///
/// **The whole span list, and no attempt to guess which of them a body could
/// ever climb onto.** [`Spans::check`](crate::spans::Spans::check) only ever
/// answers with a span's own `stand_z`, so a column's spans are a *superset* of
/// every landing the step rule can produce over this map — and that is the
/// property the passes below need rather than a nicety: a flood that stepped
/// somewhere the graph had no place for would stop dead there and call the
/// ground unreachable. Keeping a surface nothing can reach costs nothing in
/// exchange, because the component pass is over **directed** steps: such a place
/// is its own strong component with no edge into it, and no route is ever
/// planned through one.
///
/// The filter is [`can_step`](crate::can_step) asked of the place itself, which
/// is what drops a column the live world has walled off. A production bake runs
/// over an empty overlay by design — a door that happened to be shut is not a
/// property of the ground, see `docs/map/navigation_graph_bake.md` — so on the
/// shard's own artifact this only ever drops what the map itself refuses.
///
/// One function, and a *query* reads it again — [`RegionPlaces::sampled`] is
/// what both a bake and a join take of a region, and what one produces has to be
/// exactly what the other does or a node of that region would have no slot in
/// it.
///
/// `column` is the caller's scratch buffer, reused rather than sized.
fn column_places(footing: &Footing<'_>, x: u16, y: u16, column: &mut Vec<i8>, out: &mut Vec<Point>) {
    column.clear();
    match footing.map {
        Some(map) => column.extend(map.spans().surfaces(x, y).map(|span| span.stand_z)),
        // No map at all: no floor and no walls, every step allowed and z
        // never changing, so a column holds exactly one place. See
        // `Footing::map`.
        None => column.push(0),
    }
    for offset in 0..column.len() {
        let z = column[offset];
        // Two spans of one column can share a height — the land and a
        // paving stone laid on it — and one height is one place.
        if column[..offset].contains(&z) {
            continue;
        }
        let place = Point::new(x, y, z);
        if crate::can_step(footing, place, place).is_some() {
            out.push(place);
        }
    }
}

/// Mark the strongly connected components of one region's places.
///
/// Bake-time scratch data: the labels never enter the artifact. **One label per
/// place**, so a bridge deck and the road beneath it are two components of one
/// region rather than one component of one tile.
///
/// `u16` numbers a region's components, which is a whole region's worth of
/// places and then some: the deepest column on Britannia holds twelve spans, so
/// a 32×32 region holds at most twelve thousand of them. The counter checks
/// rather than wraps, because a base set is a world nobody has counted.
fn component_labels(footing: &Footing<'_>, local: &RegionPlaces) -> Vec<u16> {
    let cells = local.len();
    let mut labels = vec![NO_COMPONENT; cells];
    if cells == 0 {
        return labels;
    }

    let edges = RegionEdges::of(footing, local);
    let mut component = NO_COMPONENT;
    for root in edges.finishing_order().into_iter().rev() {
        if labels[root] != NO_COMPONENT {
            continue;
        }
        component = component
            .checked_add(1)
            .expect("a region has at most one component per standing place");
        label_component(&edges, &mut labels, root, component);
    }
    labels
}

/// The directed step graph of one region and its transpose.
struct RegionEdges {
    // Out-degree is bounded by the eight directions and in-degree is not: two
    // places of one neighbouring column can land on the same place — which is
    // exactly what a stair does, and what a fixed eight-slot array here used to
    // make an out-of-bounds panic.
    outgoing:         Vec<[u32; Direction::ALL.len()]>,
    outgoing_len:     Vec<u8>,
    // Counting-sorted into one run per destination. Kosaraju's second pass
    // walks these, and a region has thousands of places.
    incoming_offsets: Vec<u32>,
    incoming:         Vec<u32>,
}

impl RegionEdges {
    fn of(footing: &Footing<'_>, local: &RegionPlaces) -> Self {
        let cells = local.len();
        let mut outgoing = vec![[NO_NEIGHBOR; Direction::ALL.len()]; cells];
        let mut outgoing_len = vec![0_u8; cells];

        for from_index in 0..cells {
            // The whole expansion at once. Eight `step_allowed` calls would resolve
            // the place being stepped off eight times over and each cardinal
            // neighbour twice — the same waste `find_path` stopped paying in N3, on
            // the pass that walks every place of the facet.
            for next in steps_out_of(footing, local.points[from_index])
                .into_iter()
                .flatten()
            {
                let Some(next_index) = local.slot(next) else {
                    continue;
                };
                let out_at = usize::from(outgoing_len[from_index]);
                outgoing[from_index][out_at] = next_index as u32;
                outgoing_len[from_index] += 1;
            }
        }

        let mut incoming_offsets = vec![0_u32; cells + 1];
        for from_index in 0..cells {
            for &to in &outgoing[from_index][..usize::from(outgoing_len[from_index])] {
                incoming_offsets[to as usize + 1] += 1;
            }
        }
        for index in 0..cells {
            incoming_offsets[index + 1] += incoming_offsets[index];
        }
        let mut filled = incoming_offsets.clone();
        let mut incoming = vec![0_u32; incoming_offsets[cells] as usize];
        for from_index in 0..cells {
            for &to in &outgoing[from_index][..usize::from(outgoing_len[from_index])] {
                incoming[filled[to as usize] as usize] = from_index as u32;
                filled[to as usize] += 1;
            }
        }

        Self {
            outgoing,
            outgoing_len,
            incoming_offsets,
            incoming,
        }
    }

    /// Nodes in the order Kosaraju's first pass finishes them.
    fn finishing_order(&self) -> Vec<usize> {
        let cells = self.outgoing.len();
        let mut seen = vec![false; cells];
        let mut finish = Vec::with_capacity(cells);
        for root in 0..cells {
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let mut stack = vec![(root, 0_u8)];
            while let Some((at, next)) = stack.last_mut() {
                if usize::from(*next) < usize::from(self.outgoing_len[*at]) {
                    let neighbor = self.outgoing[*at][usize::from(*next)] as usize;
                    *next += 1;
                    if !seen[neighbor] {
                        seen[neighbor] = true;
                        stack.push((neighbor, 0));
                    }
                } else {
                    finish.push(*at);
                    stack.pop();
                }
            }
        }
        finish
    }

    fn incoming(&self, at: usize) -> &[u32] {
        let run = self.incoming_offsets[at] as usize..self.incoming_offsets[at + 1] as usize;
        &self.incoming[run]
    }
}

fn label_component(edges: &RegionEdges, labels: &mut [u16], root: usize, component: u16) {
    let mut stack = vec![root];
    while let Some(at) = stack.pop() {
        let label = &mut labels[at];
        if *label != NO_COMPONENT {
            continue;
        }
        *label = component;
        for &neighbor in edges.incoming(at) {
            let neighbor = neighbor as usize;
            if labels[neighbor] == NO_COMPONENT {
                stack.push(neighbor);
            }
        }
    }
}

impl NavigationGraph {
    /// Extract a static graph from one facet. Empty and unrepresentable facets
    /// cannot be addressed by `Point` and therefore have no graph.
    #[must_use]
    pub fn build(footing: &Footing<'_>, width: u32, height: u32) -> Option<Self> {
        let limit = u32::from(u16::MAX) + 1;
        if width == 0 || height == 0 || width >= limit || height >= limit {
            return None;
        }
        let started = Instant::now();
        let cells = width as usize * height as usize;
        let mut graph = Self {
            width,
            height,
            regions: Vec::new(),
            walkable: vec![0; cells.div_ceil(8)],
            nodes: Vec::new(),
            region_runs: Vec::new(),
            region_nodes: Vec::new(),
            edge_runs: Vec::new(),
            edge_targets: Vec::new(),
            edge_costs: Vec::new(),
            dead_nodes: 0,
            dead_region_nodes: 0,
            dead_edges: 0,
        };
        graph.partition();
        eprintln!(
            "navigation graph: {}x{} terrain, partitioned into {} regions",
            width,
            height,
            graph.regions.len()
        );
        // **A whole facet is every region rebaked**, and that is the one thing
        // this call says that the sequence it replaced did not: there is a single
        // construction, so a facet patched into shape and a facet baked whole are
        // not two implementations that have to be kept agreeing. See
        // [`rebake_regions`](Self::rebake_regions).
        let all: Vec<RegionId> = (0..graph.regions.len()).map(RegionId).collect();
        graph.rebake_regions(footing, &all, &all);
        eprintln!(
            "navigation graph +{:.3}s: ready ({} nodes, {} edges)",
            started.elapsed().as_secs_f64(),
            graph.nodes.len(),
            graph.edge_targets.len()
        );
        Some(graph)
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Counts useful to an offline builder and its progress report.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.regions.len(), self.nodes.len(), self.edge_targets.len())
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * self.width as usize + usize::from(x)
    }

    /// Which region a point is in, if a body could be standing there.
    ///
    /// The walkable bit is what an *endpoint* is joined to the graph by, which
    /// is why this question and [`region_containing`](Self::region_containing)
    /// are two questions: where a place is, is a fact about the grid, and
    /// whether anything stands there is a fact about the ground.
    fn region_at(&self, point: Point) -> Option<RegionId> {
        self.is_walkable(point.x, point.y)
            .then(|| self.region_containing(point))
            .flatten()
    }

    /// Which region a point is in, whatever stands there.
    ///
    /// A node keeps its region while a rebake is taking its place away — the
    /// tile's walkable bit is already the new world's by then, and asking this
    /// the other way round is how a dying node's edges become unfindable.
    fn region_containing(&self, point: Point) -> Option<RegionId> {
        (u32::from(point.x) < self.width && u32::from(point.y) < self.height).then(|| {
            RegionId(
                usize::from(point.y) / REGION_SIZE as usize * self.regions_across()
                    + usize::from(point.x) / REGION_SIZE as usize,
            )
        })
    }

    fn regions_across(&self) -> usize {
        (self.width as usize).div_ceil(REGION_SIZE as usize)
    }

    fn regions_down(&self) -> usize {
        (self.height as usize).div_ceil(REGION_SIZE as usize)
    }

    fn is_walkable(&self, x: u16, y: u16) -> bool {
        let index = self.index(x, y);
        self.walkable[index / 8] & (1 << (index % 8)) != 0
    }

    fn set_walkable(&mut self, x: u16, y: u16, walkable: bool) {
        let index = self.index(x, y);
        let bit = 1 << (index % 8);
        match walkable {
            true => self.walkable[index / 8] |= bit,
            false => self.walkable[index / 8] &= !bit,
        }
    }

    /// The 32×32 grid itself, and nothing standing on it.
    ///
    /// The regions are a *computation* over the facet's extent — `region_at`
    /// derives one from a point without reading anything here — so this is the
    /// one part of the graph a rebake never has to revisit.
    fn partition(&mut self) {
        for top in (0..self.height).step_by(REGION_SIZE as usize) {
            for left in (0..self.width).step_by(REGION_SIZE as usize) {
                self.regions.push(Region {
                    left:   left as u16,
                    top:    top as u16,
                    width:  REGION_SIZE.min(self.width - left) as u16,
                    height: REGION_SIZE.min(self.height - top) as u16,
                });
                self.region_runs.push(Run::NONE);
            }
        }
    }

    /// Rewrite one region's walkable bits from the places just sampled for it.
    ///
    /// A tile is walkable when *something* stands on it, whatever height that
    /// is. The bit is what an endpoint is joined to the graph by, and an endpoint
    /// carries a z nobody promised — see `path::goal_node`.
    ///
    /// **Written both ways**, unlike the pass this replaces: a whole-facet bake
    /// starts from a bitmap of zeroes and only ever sets, and a rebake starts
    /// from the bits of the world before the edit — where a tile whose last
    /// standing surface was just dug away has to lose its bit.
    fn region_walkable(&mut self, held: &Held, region: RegionId) {
        let rectangle = self.regions[region.0];
        let places = &held.at(region).places;
        for y in rectangle.top..rectangle.top + rectangle.height {
            for x in rectangle.left..rectangle.left + rectangle.width {
                self.set_walkable(x, y, !places.column(x, y).is_empty());
            }
        }
    }

    /// Bring the graph back in step with ground that moved under these chunks.
    ///
    /// `docs/map/navigation_graph.md`'s **G1**, and the third artefact of
    /// `what_a_change_costs.md`'s S3: the span index and `WorldMap`'s statics
    /// already follow a publish locally, and this is the one that used to be
    /// *dropped* instead — 11.6 s to build, on a tick an operator typed into.
    ///
    /// **The area is two rings and a half**, and each of the three is a different
    /// claim:
    ///
    /// 1. The regions the chunks cover, **grown by one tile west and north** —
    ///    a column's height is the average of the four cells meeting at its
    ///    north-west corner, so a cell that moved is read by the column before
    ///    it. Their places, components, portals and intra-region edges are all
    ///    rebuilt. This is the span layer's own seam, one scale up.
    /// 2. Their **neighbours**, because a portal is a fact about a *border*: the
    ///    node on the far side of A|B belongs to B, so a border that gained or
    ///    lost a crossing changes B's node set — and that changes what B's
    ///    intra-region routing is between.
    /// 3. And the ring beyond, for **edges only**: C's portal edges into B are
    ///    rebuilt because B's were, and C's own places, nodes and intra-region
    ///    edges are not, because nothing under C moved. That is where the
    ///    cascade stops, and saying so is half of what this is.
    ///
    /// **It trusts its caller to name every chunk that changed**, exactly as
    /// [`SpanIndex::rebake_chunks`](crate::spans::SpanIndex::rebake_chunks)
    /// does — a caller that named too few leaves a region routing over the world
    /// as it was.
    pub fn rebake_chunks(&mut self, footing: &Footing<'_>, chunks: &[ChunkCoord]) {
        let (moved, rebuilt) = self.regions_around(chunks);
        if rebuilt.is_empty() {
            return;
        }
        self.rebake_regions(footing, &rebuilt, &moved);
    }

    /// The two rings [`rebake_chunks`](Self::rebake_chunks) works over: the
    /// regions whose ground moved, and those plus every neighbour of one.
    ///
    /// Both, rather than the union alone, because they are not owed the same
    /// work — see [`rebake_regions`](Self::rebake_regions).
    fn regions_around(&self, chunks: &[ChunkCoord]) -> (Vec<RegionId>, Vec<RegionId>) {
        let across = self.regions_across();
        let down = self.regions_down();
        let mut moved = std::collections::BTreeSet::new();
        for chunk in chunks {
            let (x, y) = chunk.origin();
            if x >= self.width || y >= self.height {
                continue;
            }
            // The columns this chunk moved, in tiles: its own, and **one west
            // and one north** — a column's height is the average of the four
            // cells meeting at its north-west corner, so a cell that moved is
            // read by the column before it. Clipped to the facet at the far end,
            // which is what a chunk on its eastern or southern edge needs.
            let left = x.saturating_sub(1);
            let top = y.saturating_sub(1);
            let right = (x + CHUNK_TILES - 1).min(self.width - 1);
            let bottom = (y + CHUNK_TILES - 1).min(self.height - 1);
            for row in (top / REGION_SIZE) as usize..=(bottom / REGION_SIZE) as usize {
                for column in (left / REGION_SIZE) as usize..=(right / REGION_SIZE) as usize {
                    moved.insert((column, row));
                }
            }
        }
        // The second ring, which is every neighbour of a region whose ground
        // moved — eight rather than four, because saying "a corner shares no
        // border" is a claim about the portal pass that this set does not have
        // to make.
        let mut rebuilt = std::collections::BTreeSet::new();
        for &(column, row) in &moved {
            for row in row.saturating_sub(1)..=(row + 1).min(down - 1) {
                for column in column.saturating_sub(1)..=(column + 1).min(across - 1) {
                    rebuilt.insert(RegionId(row * across + column));
                }
            }
        }
        let moved = moved
            .into_iter()
            .map(|(column, row)| RegionId(row * across + column))
            .collect();
        (moved, rebuilt.into_iter().collect())
    }

    /// Rebuild the node set and the edges of every region named, and the borders
    /// they share with their neighbours.
    ///
    /// **The one construction**, whether it is a whole facet or the
    /// neighbourhood of a publish: [`build`](Self::build) calls it with every
    /// region of the facet, and [`rebake_chunks`](Self::rebake_chunks) with two
    /// rings around an edit. A facet patched into shape and a facet baked whole
    /// are then the same code over the same ground rather than two
    /// implementations somebody has to keep agreeing.
    ///
    /// `moved` is the subset whose *ground* changed, and it is what tells the
    /// outer ring from the inner: a region rebuilt only because its neighbour
    /// moved keeps its places, and therefore keeps its intra-region routes
    /// unless its node set came back different.
    fn rebake_regions(&mut self, footing: &Footing<'_>, rebuilt: &[RegionId], moved: &[RegionId]) {
        // What has to be sampled: the regions being rebuilt, and the far side of
        // every border they touch.
        let mut wanted = Vec::with_capacity(rebuilt.len() * 5);
        for &region in rebuilt {
            wanted.push(region);
            wanted.extend(self.beside(region));
        }
        let held = Held::of(footing, self, &wanted);

        // The walkable bits first: `region_at` reads them, and every pass below
        // asks it which region a place is in.
        for &region in rebuilt {
            self.region_walkable(&held, region);
        }

        let mut build = Rebuild::of(self, &held, rebuilt, moved);
        for (region, side) in self.borders_of(rebuilt) {
            self.border_portals(footing, &held, &mut build, region, side);
        }
        for &region in rebuilt {
            // **What a region's intra-region routes depend on**: its own places,
            // and which of them are nodes. A region in the outer of the two
            // rebuilt rings has the places it had — nothing under it moved — so
            // if its node set came back the same, so did every route between
            // them, and the floods below are the expensive half of a rebake.
            // Its edges were kept rather than cleared for exactly this.
            let unmoved_ring = !build.has_moved(region) && !self.node_set_changed(&build, region);
            if unmoved_ring {
                continue;
            }
            let nodes = self.nodes_of_either(&build, region);
            self.strip_intra(&mut build, region, &nodes);
            self.intra_edges(footing, &held, &mut build, region);
        }
        self.write_back(build);
        self.repack_if_mostly_dead();
    }

    /// Whether a region's node list came back different from the one it holds.
    fn node_set_changed(&self, build: &Rebuild, region: RegionId) -> bool {
        let Some(nodes) = build.listed(region) else {
            return false;
        };
        let mut now: Vec<u32> = nodes.iter().map(|node| node.0 as u32).collect();
        let mut was: Vec<u32> = self.region_nodes[self.region_runs[region.0].range()].to_vec();
        now.sort_unstable();
        was.sort_unstable();
        now != was
    }

    /// Every node this rebake has, or had, in one region — which is what has to
    /// be cleared of intra-region edges before they are worked out again.
    fn nodes_of_either(&self, build: &Rebuild, region: RegionId) -> Vec<NodeId> {
        let mut nodes: Vec<NodeId> = self.nodes_in_region(region).collect();
        if let Some(listed) = build.listed(region) {
            for &node in listed {
                if !nodes.contains(&node) {
                    nodes.push(node);
                }
            }
        }
        nodes
    }

    /// Take a region's own intra-region edges back out of the lists a rebake is
    /// assembling, leaving the portal edges the border passes just put in.
    fn strip_intra(&self, build: &mut Rebuild, region: RegionId, nodes: &[NodeId]) {
        for &node in nodes {
            let regions: Vec<RegionId> = build
                .edges_of(node)
                .iter()
                .map(|edge| self.node_region(edge.to))
                .collect();
            let mut at = 0;
            build.edges_of(node).retain(|_| {
                let keep = regions[at] != region;
                at += 1;
                keep
            });
        }
    }

    /// Every border with a rebuilt region on at least one side, each named once.
    ///
    /// A border belongs to the region on its north-west side, so the pair
    /// `(region, side)` names one and the same border however it is reached — a
    /// rebuilt region's own east border, and the east border of the region to its
    /// west.
    fn borders_of(&self, rebuilt: &[RegionId]) -> Vec<(RegionId, Side)> {
        let across = self.regions_across();
        let mut borders = std::collections::BTreeSet::new();
        for &region in rebuilt {
            let (column, row) = (region.0 % across, region.0 / across);
            if column + 1 < across {
                borders.insert((region.0, Side::East));
            }
            if column > 0 {
                borders.insert((region.0 - 1, Side::East));
            }
            if row + 1 < self.regions_down() {
                borders.insert((region.0, Side::South));
            }
            if row > 0 {
                borders.insert((region.0 - across, Side::South));
            }
        }
        borders
            .into_iter()
            .map(|(region, side)| (RegionId(region), side))
            .collect()
    }

    /// The regions sharing a border with this one.
    fn beside(&self, region: RegionId) -> Vec<RegionId> {
        let across = self.regions_across();
        let (column, row) = (region.0 % across, region.0 / across);
        let mut beside = Vec::with_capacity(4);
        if column + 1 < across {
            beside.push(RegionId(region.0 + 1));
        }
        if column > 0 {
            beside.push(RegionId(region.0 - 1));
        }
        if row + 1 < self.regions_down() {
            beside.push(RegionId(region.0 + across));
        }
        if row > 0 {
            beside.push(RegionId(region.0 - across));
        }
        beside
    }

    /// One border's portals, as the nodes and edges they mint.
    ///
    /// Adjacent raw crossings share a logical entrance when they connect the
    /// same strong components. This lets an isolated tree on a border remain a
    /// local obstacle instead of multiplying portal nodes.
    ///
    /// **Each way across is its own entrance**, which is the second half of N4:
    /// an entrance is keyed by where a step *starts* as well as where it ends, so
    /// a border a body can cross one way and not the other is a portal rather
    /// than nothing at all. Where both ways exist — which is nearly everywhere —
    /// the two runs cover the same stretch of border, choose the same
    /// representatives, and intern the same pair of nodes, so a symmetric border
    /// costs exactly what it always did.
    ///
    /// **One border at a time**, where this used to sweep a facet-long line: an
    /// entrance was always keyed by the pair of regions it joins, so a run never
    /// spanned more than one pair — and one pair at a time is the shape the
    /// rebake of a neighbourhood needs.
    fn border_portals(
        &mut self,
        footing: &Footing<'_>,
        held: &Held,
        build: &mut Rebuild,
        first: RegionId,
        side: Side,
    ) {
        let second = match side {
            Side::East => RegionId(first.0 + 1),
            Side::South => RegionId(first.0 + self.regions_across()),
        };
        let near = Bank {
            id:   first,
            bake: held.at(first),
        };
        let far = Bank {
            id:   second,
            bake: held.at(second),
        };
        let rectangle = self.regions[first.0];
        let mut entrances = Entrances::new();
        match side {
            Side::East => {
                let x = rectangle.left + rectangle.width - 1;
                for y in rectangle.top..rectangle.top + rectangle.height {
                    crossings(
                        footing,
                        near,
                        far,
                        Tile::new(x, y),
                        Direction::East,
                        &mut entrances,
                    );
                    crossings(
                        footing,
                        far,
                        near,
                        Tile::new(x + 1, y),
                        Direction::West,
                        &mut entrances,
                    );
                }
            }
            Side::South => {
                let y = rectangle.top + rectangle.height - 1;
                for x in rectangle.left..rectangle.left + rectangle.width {
                    crossings(
                        footing,
                        near,
                        far,
                        Tile::new(x, y),
                        Direction::South,
                        &mut entrances,
                    );
                    crossings(
                        footing,
                        far,
                        near,
                        Tile::new(x, y + 1),
                        Direction::North,
                        &mut entrances,
                    );
                }
            }
        }
        for run in entrances.into_values() {
            self.add_portal(build, &run);
        }
    }

    /// One logical entrance, as one or two directed edges.
    ///
    /// The representatives are what they always were — the middle of a narrow
    /// run, both ends of a wide one — and what changed is that a crossing buys
    /// **one** edge. Its reverse, where the ground allows one, arrives as its own
    /// entrance and its own edge over the same interned nodes.
    fn add_portal(&mut self, build: &mut Rebuild, run: &[(Point, Point)]) {
        let ids: Vec<_> = match run.len() {
            0 => return,
            1..WIDE_PORTAL => vec![(run.len() - 1) / 2],
            _ => vec![0, run.len() - 1],
        };
        for index in ids {
            let first_id = self.intern_node(build, run[index].0);
            let second_id = self.intern_node(build, run[index].1);
            build.add_edge(first_id, second_id, 1);
        }
    }

    /// The node one standing place is, minted the first time an entrance names
    /// it.
    ///
    /// Interned rather than pushed, because a place is named by up to two
    /// entrances — the way in and the way out — and by the entrances of a
    /// perpendicular border where they meet at a corner. Two nodes at one place
    /// would be two names for one thing, and would double the intra-region
    /// routing every one of them pays for.
    ///
    /// **The interning table is seeded with the nodes the graph already holds**,
    /// which is what makes a local rebake possible at all: a `NodeId` is an index
    /// *other regions' edges point at*, so a place that still has a node has to
    /// keep its number. A place that lost its node leaves a dead entry behind and
    /// a place that gained one takes a number at the end.
    fn intern_node(&mut self, build: &mut Rebuild, point: Point) -> NodeId {
        let region = self
            .region_at(point)
            .expect("a portal endpoint is a place on the map");
        if let Some(&id) = build.interned.get(&(point.x, point.y, point.z)) {
            build.list(region, id);
            return id;
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node { point });
        self.edge_runs.push(Run::NONE);
        build.open(id);
        build.interned.insert((point.x, point.y, point.z), id);
        // A portal endpoint outside the rebuilt regions is always a node the
        // graph already holds — that is exactly what makes the ring beyond them
        // "edges only", so a *new* node there would mean the area was chosen
        // wrong rather than that the world has one more place in it.
        build
            .listing(region)
            .expect("a new portal endpoint stands in a region this rebake is rebuilding")
            .push(id);
        id
    }

    /// What it costs to cross one region between its own portals.
    fn intra_edges(&mut self, footing: &Footing<'_>, held: &Held, build: &mut Rebuild, region: RegionId) {
        let nodes = build
            .listing(region)
            .expect("a rebuilt region has a node list")
            .clone();
        // A region with one entrance has nothing to route between, and a facet is
        // mostly such regions — the traversal below is the bake's whole cost, so
        // not starting it is worth the branch.
        if nodes.len() < 2 {
            return;
        }
        let local = &held.at(region).places;
        // The other nodes are the whole of what each traversal is for, so they
        // are resolved once and handed to it: a flood that has costed all of them
        // has nothing left to learn about this region.
        let slots: Vec<usize> = nodes
            .iter()
            .map(|&node| {
                local
                    .slot(self.nodes[node.0].point)
                    .expect("a node is a place of its own region")
            })
            .collect();
        for &from in &nodes {
            // The bake pays for nothing: what a flood cost is a *query's*
            // question, and this one runs with a whole region's worth of time to
            // do it in.
            let (costs, _) = region_costs(footing, local, self.nodes[from.0].point, &slots);
            for (index, &to) in nodes.iter().enumerate() {
                if from == to {
                    continue;
                }
                if let Some(cost) = costs[slots[index]] {
                    build.add_edge(from, to, cost);
                }
            }
        }
    }

    /// Write everything a rebake assembled into the graph's own arrays.
    ///
    /// **A run that did not change is not written at all**, which matters more
    /// here than it does one layer down: most of the second ring around an edit
    /// comes back byte for byte as it was, and rewriting it would manufacture
    /// garbage out of a publish that moved nothing there.
    fn write_back(&mut self, build: Rebuild) {
        for (node, mut edges) in build.edges {
            edges.sort_unstable_by_key(|edge| (edge.to, edge.cost));
            edges.dedup_by_key(|edge| edge.to);
            self.place_edges(node, &edges);
        }
        for (region, nodes) in build.nodes {
            let ids: Vec<u32> = nodes.iter().map(|node| node.0 as u32).collect();
            self.place_nodes(region, &ids);
        }
    }

    /// Repoint one region at its new node list.
    ///
    /// A list that fits where it stood is written there and a longer one goes to
    /// the end — the table's whole purpose, and what a prefix sum forbids.
    fn place_nodes(&mut self, region: RegionId, ids: &[u32]) {
        let was = self.region_runs[region.0];
        if self.region_nodes[was.range()] == *ids {
            return;
        }
        // A node no place named this time round: its edges are already cleared
        // (every node of a rebuilt region is written above, with whatever the
        // passes gave it, which for this one is nothing), and nothing points at
        // it, because every edge into a rebuilt region was rewritten too.
        for &node in &self.region_nodes[was.range()] {
            if !ids.contains(&node) {
                self.dead_nodes += 1;
            }
        }
        let count = ids.len() as u32;
        let base = match count <= was.count {
            true => was.base,
            false => self.region_nodes.len() as u32,
        };
        if count <= was.count {
            self.region_nodes[base as usize..base as usize + ids.len()].copy_from_slice(ids);
            self.dead_region_nodes += was.count - count;
        } else {
            self.region_nodes.extend_from_slice(ids);
            self.dead_region_nodes += was.count;
        }
        self.region_runs[region.0] = Run { base, count };
    }

    /// Repoint one node at its new edge list. [`place_nodes`](Self::place_nodes)
    /// one level down, over the two parallel arrays an edge is.
    fn place_edges(&mut self, node: NodeId, edges: &[Edge]) {
        let was = self.edge_runs[node.0];
        let unchanged = was.count as usize == edges.len()
            && self.edge_targets[was.range()]
                .iter()
                .zip(&self.edge_costs[was.range()])
                .zip(edges)
                .all(|((&to, &cost), edge)| to as usize == edge.to.0 && u32::from(cost) == edge.cost);
        if unchanged {
            return;
        }
        let count = edges.len() as u32;
        let base = match count <= was.count {
            true => was.base,
            false => self.edge_targets.len() as u32,
        };
        for (offset, edge) in edges.iter().enumerate() {
            let cost = u16::try_from(edge.cost).expect("a 32×32 region route fits in u16");
            match count <= was.count {
                true => {
                    self.edge_targets[base as usize + offset] = edge.to.0 as u32;
                    self.edge_costs[base as usize + offset] = cost;
                }
                false => {
                    self.edge_targets.push(edge.to.0 as u32);
                    self.edge_costs.push(cost);
                }
            }
        }
        self.dead_edges += match count <= was.count {
            true => was.count - count,
            false => was.count,
        };
        self.edge_runs[node.0] = Run { base, count };
    }

    /// The garbage rule, and it is the span layer's verbatim — *never compact
    /// during a session, until the dead outweigh the live* — with one difference
    /// that matters: `SpanIndex` answers it by baking the facet whole, and
    /// 11.6 s is the thing this file exists to stop paying. So the answer here is
    /// a **repack**: one walk of what is live, at no point asking the ground
    /// anything.
    fn repack_if_mostly_dead(&mut self) {
        let dead = self.dead_nodes as usize + self.dead_region_nodes as usize + self.dead_edges as usize;
        let live = self.nodes.len() + self.region_nodes.len() + self.edge_targets.len() - dead;
        if dead > live {
            self.repack();
        }
    }

    /// Drop every entry no run points at, and renumber what is left.
    ///
    /// A `NodeId` is an index and nothing outside this graph holds one — a query
    /// resolves its endpoints against the regions every time it is asked — so
    /// renumbering is a private matter, which is what makes this cheap enough to
    /// be the answer to garbage.
    fn repack(&mut self) {
        let mut renumbered = vec![NO_NEIGHBOR; self.nodes.len()];
        let mut nodes = Vec::with_capacity(self.nodes.len() - self.dead_nodes as usize);
        for run in &self.region_runs {
            for &node in &self.region_nodes[run.range()] {
                renumbered[node as usize] = nodes.len() as u32;
                nodes.push(self.nodes[node as usize]);
            }
        }
        let mut region_nodes = Vec::with_capacity(nodes.len());
        let mut region_runs = Vec::with_capacity(self.regions.len());
        for run in &self.region_runs {
            let base = region_nodes.len() as u32;
            region_nodes.extend(
                self.region_nodes[run.range()]
                    .iter()
                    .map(|&node| renumbered[node as usize]),
            );
            region_runs.push(Run {
                base,
                count: run.count,
            });
        }
        let mut edge_runs = Vec::with_capacity(nodes.len());
        let mut edge_targets = Vec::with_capacity(self.edge_targets.len());
        let mut edge_costs = Vec::with_capacity(self.edge_costs.len());
        let mut order: Vec<usize> = (0..self.nodes.len())
            .filter(|&node| renumbered[node] != NO_NEIGHBOR)
            .collect();
        order.sort_unstable_by_key(|&node| renumbered[node]);
        for node in order {
            let run = self.edge_runs[node];
            let base = edge_targets.len() as u32;
            for (&to, &cost) in self.edge_targets[run.range()]
                .iter()
                .zip(&self.edge_costs[run.range()])
            {
                // An edge into a node nothing lists any more is dropped rather
                // than renumbered. It cannot happen — every edge into a rebuilt
                // region is rewritten by the rebake that killed the node — and
                // dropping it is the only honest answer if it ever does.
                if renumbered[to as usize] == NO_NEIGHBOR {
                    continue;
                }
                edge_targets.push(renumbered[to as usize]);
                edge_costs.push(cost);
            }
            edge_runs.push(Run {
                base,
                count: (edge_targets.len() as u32) - base,
            });
        }
        self.nodes = nodes;
        self.region_nodes = region_nodes;
        self.region_runs = region_runs;
        self.edge_runs = edge_runs;
        self.edge_targets = edge_targets;
        self.edge_costs = edge_costs;
        self.dead_nodes = 0;
        self.dead_region_nodes = 0;
        self.dead_edges = 0;
    }

    fn node_region(&self, node: NodeId) -> RegionId {
        self.region_containing(self.nodes[node.0].point)
            .expect("every node is inside the map")
    }

    fn nodes_in_region(&self, region: RegionId) -> impl Iterator<Item = NodeId> + '_ {
        self.region_nodes[self.region_runs[region.0].range()]
            .iter()
            .map(|&id| NodeId(id as usize))
    }

    fn edges_from(&self, node: NodeId) -> impl Iterator<Item = Edge> + '_ {
        let run = self.edge_runs[node.0].range();
        self.edge_targets[run.clone()]
            .iter()
            .zip(&self.edge_costs[run])
            .map(|(&to, &cost)| {
                Edge {
                    to:   NodeId(to as usize),
                    cost: u32::from(cost),
                }
            })
    }

    fn abstract_path(
        &self,
        from: Point,
        to: Point,
        forbidden: &[bool],
        source: &[(NodeId, u32)],
        target: &[(NodeId, u32)],
    ) -> Option<Vec<NodeId>> {
        if source.is_empty() || target.is_empty() {
            return None;
        }
        let start = self.nodes.len();
        let goal = start + 1;
        let mut cost = vec![u32::MAX; goal + 1];
        let mut parent = vec![None; goal + 1];
        let mut target_cost = vec![None; self.nodes.len()];
        for &(node, cost) in target {
            if !forbidden[node.0] {
                target_cost[node.0] = Some(cost);
            }
        }
        let mut open = BinaryHeap::new();
        cost[start] = 0;
        open.push(Reverse((distance(from, to), 0, start)));
        while let Some(Reverse((_f, here_cost, here))) = open.pop() {
            if here_cost != cost[here] {
                continue;
            }
            if here == goal {
                break;
            }
            let mut relax = |next: usize, edge_cost: u32, point: Point| {
                let next_cost = here_cost.saturating_add(edge_cost);
                if next_cost < cost[next] {
                    cost[next] = next_cost;
                    parent[next] = Some(here);
                    open.push(Reverse((next_cost + distance(point, to), next_cost, next)));
                }
            };
            if here == start {
                for &(node, edge_cost) in source {
                    if !forbidden[node.0] {
                        relax(node.0, edge_cost, self.nodes[node.0].point);
                    }
                }
                continue;
            }
            for edge in self.edges_from(NodeId(here)) {
                if !forbidden[edge.to.0] {
                    relax(edge.to.0, edge.cost, self.nodes[edge.to.0].point);
                }
            }
            if let Some(edge_cost) = target_cost[here] {
                relax(goal, edge_cost, to);
            }
        }
        parent[goal]?;
        let mut path = Vec::new();
        let mut here = goal;
        while let Some(previous) = parent[here] {
            here = previous;
            if here < self.nodes.len() {
                path.push(NodeId(here));
            }
        }
        path.reverse();
        Some(path)
    }

    /// What it costs to join one endpoint to every node of its own region.
    ///
    /// **One flood, and not one search per node.** N4 filed this as a finding
    /// and this is the repair: joining an endpoint used to run a bounded exact
    /// search from it to *every* node of its region, at both ends of a query, so
    /// a node the endpoint could not reach cost the whole budget before saying
    /// so — and N4 had just made the regions that matter three times denser. A
    /// uniform-cost flood answers all of them at once. Every place of the region
    /// is expanded at most once however many nodes stand in it, and a node
    /// outside the endpoint's reach costs nothing at all, because the flood
    /// never arrives there. That reach *is* the component label the bake
    /// computes and throws away, recovered where it is wanted instead of stored.
    ///
    /// **The two directions are two questions.** The step rule is asymmetric by
    /// design — a body may drop off a ledge and not climb back — so what it
    /// costs to reach a portal from an endpoint and what it costs to reach the
    /// endpoint from that portal are different numbers, and a join that answered
    /// one with the other would propose corridors nothing can walk. [`Join`]
    /// says which was asked.
    ///
    /// **Nothing stops it part-way**, unlike the fan-out it replaces. A flood
    /// over one region is bounded work — one expansion per place of a 32×32
    /// rectangle, which the census puts at about a thousand and caps at twelve
    /// thousand for a base set nobody has counted — where a fan-out carried a
    /// whole node budget *per node* and could spend a whole query by itself. So
    /// the flood is not interrupted; it **pays** for what it expanded, and the
    /// query's wallet is read where it already was, once the join is done.
    fn local_costs(
        &self,
        footing: &Footing<'_>,
        region_id: RegionId,
        endpoint: Point,
        join: Join,
        effort: &mut Effort,
    ) -> Vec<(NodeId, u32)> {
        let local = RegionPlaces::sampled(footing, self.regions[region_id.0]);
        // A region whose walkable bit is set holds a place, and the endpoint's
        // own height need not be one of them — see `RegionPlaces::nearest_slot`.
        let Some(start) = local.nearest_slot(endpoint) else {
            return Vec::new();
        };
        let at = local.points[start];
        // The region's nodes are what the flood is for, and it stops once it has
        // answered for all of them — so a query over open ground does not walk
        // the far corners of a rectangle whose portals it has already reached.
        let joined: Vec<(NodeId, usize)> = self
            .nodes_in_region(region_id)
            .filter_map(|node| local.slot(self.nodes[node.0].point).map(|slot| (node, slot)))
            .collect();
        let wanted: Vec<usize> = joined.iter().map(|&(_, slot)| slot).collect();
        let (costs, expanded) = match join {
            Join::OutOf => region_costs(footing, &local, at, &wanted),
            Join::Into => region_costs_into(footing, &local, at, &wanted),
        };
        effort.spend(expanded);
        joined
            .into_iter()
            .filter_map(|(node, slot)| costs[slot].map(|cost| (node, cost)))
            .collect()
    }

    fn refine(
        &self,
        footing: &Footing<'_>,
        endpoints: (Point, Point),
        nodes: &[NodeId],
        joins: (&EndpointJoin, &EndpointJoin),
        rigour: Rigour,
        effort: &mut Effort,
    ) -> Result<Vec<Direction>, NodeId> {
        let (from, to) = endpoints;
        let (source, target) = joins;
        let mut route = Vec::new();
        let mut at = from;
        let mut region = self
            .region_at(from)
            .expect("the query was checked before refinement");
        let mut first = 0;
        if let Some((&node, prefix)) = nodes
            .first()
            .and_then(|node| source.route(*node).map(|route| (node, route)))
        {
            let Some(next_at) = append(footing, at, prefix, &mut route) else {
                return Err(node);
            };
            if next_at != self.nodes[node.0].point {
                return Err(node);
            }
            at = next_at;
            region = self.node_region(node);
            first = 1;
        }
        for &node in &nodes[first..] {
            if effort.spent_out() {
                return Err(node);
            }
            let next = self.nodes[node.0];
            let next_region = self.node_region(node);
            let segment = match next_region == region {
                true => region_route(footing, self.regions[region.0], at, next.point, rigour, effort),
                false => cross_portal(footing, at, next.point),
            };
            let Some(segment) = segment else {
                return Err(node);
            };
            let Some(next_at) = append(footing, at, &segment, &mut route) else {
                return Err(node);
            };
            at = next_at;
            region = next_region;
        }
        let last = *nodes
            .last()
            .expect("an abstract route always names at least one node");
        if effort.spent_out() {
            return Err(last);
        }
        if let Some(suffix) = target.route(last) {
            let Some(next_at) = append(footing, at, suffix, &mut route) else {
                return Err(last);
            };
            if next_at != to {
                return Err(last);
            }
        } else {
            let Some(segment) = region_route(footing, self.regions[region.0], at, to, rigour, effort) else {
                return Err(last);
            };
            append(footing, at, &segment, &mut route).ok_or(last)?;
        }
        Ok(route)
    }

    fn forbid_portal(&self, node: NodeId, forbidden: &mut [bool]) {
        forbidden[node.0] = true;
        for edge in self.edges_from(node) {
            if edge.cost == 1 {
                forbidden[edge.to.0] = true;
            }
        }
    }
}

/// Which way round an endpoint is joined to the graph.
///
/// Not a flag on the flood but the question it is asked: the step rule is
/// directed, so *out of* the endpoint and *into* it are two different sets of
/// places at two different costs. See [`NavigationGraph::local_costs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    /// The endpoint is where the walk starts: what it costs to reach each node.
    OutOf,
    /// The endpoint is where the walk ends: what it costs to reach it from each
    /// node.
    Into,
}

/// An endpoint joined to the static graph, and the live route behind each
/// quoted cost when that endpoint stands on a runtime floor.
///
/// The ordinary join has no routes: refinement can reproduce it inside the
/// endpoint's region from the same static map. A live-storey join cannot be
/// reproduced that way — the graph has no node at a player house's upper-floor
/// height — so the prefix or suffix is kept until the abstract path chooses
/// which portal it actually uses.
struct EndpointJoin {
    costs:  Vec<(NodeId, u32)>,
    routes: FxHashMap<usize, Vec<Direction>>,
}

impl EndpointJoin {
    fn static_costs(costs: Vec<(NodeId, u32)>) -> Self {
        Self {
            costs,
            routes: FxHashMap::default(),
        }
    }

    fn route(&self, node: NodeId) -> Option<&[Direction]> {
        self.routes.get(&node.0).map(Vec::as_slice)
    }
}

#[derive(Clone, Copy)]
struct LiveVisit {
    /// The preceding place and the step from it to this one. `None` at the
    /// endpoint the flood started from.
    previous: Option<(Point, Direction)>,
    cost:     u32,
}

#[derive(Clone, Copy)]
struct ReverseLiveVisit {
    /// The next place toward the endpoint and the forward step that reaches it.
    /// `None` at the endpoint itself.
    next: Option<(Point, Direction)>,
    cost: u32,
}

impl NavigationGraph {
    /// Whether `endpoint` stands in a column whose topology the live layer
    /// changed by adding a surface.
    ///
    /// This is a player-house floor or stair, or a moored deck. It includes the
    /// map ground under a live floor: that point may exist in the guide too,
    /// but a house wall can separate it from every static portal in its region
    /// while the house's own exit lies across the next border. Joining it over
    /// the guide and leaving the difference to refinement would then forbid all
    /// of the region's portals without ever following the live storey out.
    fn needs_live_join(footing: &Footing<'_>, endpoint: Point) -> bool {
        let tile = Tile::new(endpoint.x, endpoint.y);
        let live_surface = footing.overlay.surfaces_at(tile).next().is_some();
        live_surface && can_stand(footing, tile, i32::from(endpoint.z), crate::PLAYER_HEIGHT)
    }

    /// Join a live-only endpoint to the first band of static portals it can
    /// actually walk to.
    ///
    /// This is the dynamic storey graph: nodes are `(x, y, z)` places reached
    /// by the production step rule, not tiles, so crossing a region boundary
    /// upstairs remains upstairs even though the baked graph has only the map
    /// below it. The walk ends once it has reached static portal nodes plus a
    /// bounded band beyond the first; every chosen edge is replayed through the
    /// live footing by [`NavigationGraph::refine`].
    fn live_join(
        &self,
        footing: &Footing<'_>,
        endpoint: Point,
        join: Join,
        effort: &mut Effort,
    ) -> EndpointJoin {
        match join {
            Join::OutOf => self.live_join_out(footing, endpoint, effort),
            Join::Into => self.live_join_into(footing, endpoint, effort),
        }
    }

    fn live_join_out(&self, footing: &Footing<'_>, endpoint: Point, effort: &mut Effort) -> EndpointJoin {
        let mut visited = FxHashMap::default();
        let mut open = VecDeque::new();
        let mut costs = Vec::new();
        let mut routes = FxHashMap::default();
        let mut first = None;
        visited.insert(
            endpoint,
            LiveVisit {
                previous: None,
                cost:     0,
            },
        );
        open.push_back(endpoint);

        while let Some(here) = open.pop_front() {
            if effort.spent_out() {
                break;
            }
            let cost = visited[&here].cost;
            if first.is_some_and(|first| cost > first + LIVE_JOIN_SLACK) {
                break;
            }
            effort.spend(1);
            for node in self.nodes_at(here) {
                if routes.contains_key(&node.0) {
                    continue;
                }
                first.get_or_insert(cost);
                costs.push((node, cost));
                routes.insert(node.0, reconstruct_live_out(&visited, endpoint, here));
            }
            for (&direction, landing) in Direction::ALL.iter().zip(steps_out_of(footing, here)) {
                let Some(landing) = landing else {
                    continue;
                };
                if visited.contains_key(&landing) {
                    continue;
                }
                visited.insert(
                    landing,
                    LiveVisit {
                        previous: Some((here, direction)),
                        cost:     cost + 1,
                    },
                );
                open.push_back(landing);
            }
        }
        EndpointJoin { costs, routes }
    }

    fn live_join_into(&self, footing: &Footing<'_>, endpoint: Point, effort: &mut Effort) -> EndpointJoin {
        let mut visited = FxHashMap::default();
        let mut open = VecDeque::new();
        let mut costs = Vec::new();
        let mut routes = FxHashMap::default();
        let mut first = None;
        visited.insert(endpoint, ReverseLiveVisit { next: None, cost: 0 });
        open.push_back(endpoint);

        while let Some(here) = open.pop_front() {
            if effort.spent_out() {
                break;
            }
            let cost = visited[&here].cost;
            if first.is_some_and(|first| cost > first + LIVE_JOIN_SLACK) {
                break;
            }
            effort.spend(1);
            for node in self.nodes_at(here) {
                if routes.contains_key(&node.0) {
                    continue;
                }
                first.get_or_insert(cost);
                costs.push((node, cost));
                routes.insert(node.0, reconstruct_live_into(&visited, here, endpoint));
            }

            // There is no inverse step rule: descent is intentionally not the
            // inverse of climbing. Enumerate every standable place in the eight
            // neighbouring columns and ask the real forward rule which of them
            // lands here, the same construction the static region join uses.
            for direction_from_here in Direction::ALL {
                let Some(column) = crate::step_from(here, direction_from_here) else {
                    continue;
                };
                for candidate in live_places_at(footing, Tile::new(column.x, column.y)) {
                    if visited.contains_key(&candidate) {
                        continue;
                    }
                    let direction = direction_from_here.opposite();
                    if step_allowed(footing, candidate, direction) != Some(here) {
                        continue;
                    }
                    visited.insert(
                        candidate,
                        ReverseLiveVisit {
                            next: Some((here, direction)),
                            cost: cost + 1,
                        },
                    );
                    open.push_back(candidate);
                }
            }
        }
        EndpointJoin { costs, routes }
    }

    fn nodes_at(&self, point: Point) -> impl Iterator<Item = NodeId> + '_ {
        self.region_at(point)
            .into_iter()
            .flat_map(|region| self.nodes_in_region(region))
            .filter(move |&node| self.nodes[node.0].point == point)
    }
}

fn live_places_at(footing: &Footing<'_>, tile: Tile) -> Vec<Point> {
    let mut heights: Vec<i8> = footing
        .map
        .into_iter()
        .flat_map(|map| map.spans().surfaces(tile.x, tile.y).map(|span| span.stand_z))
        .collect();
    heights.extend(
        footing
            .overlay
            .surfaces_at(tile)
            .filter_map(|cover| i8::try_from(cover.surface()).ok()),
    );
    heights.sort_unstable();
    heights.dedup();
    heights
        .into_iter()
        .filter_map(|z| {
            can_stand(footing, tile, i32::from(z), crate::PLAYER_HEIGHT)
                .then_some(Point::new(tile.x, tile.y, z))
        })
        .collect()
}

fn reconstruct_live_out(
    visited: &FxHashMap<Point, LiveVisit>,
    start: Point,
    mut here: Point,
) -> Vec<Direction> {
    let mut route = Vec::new();
    while here != start {
        let (previous, direction) = visited[&here]
            .previous
            .expect("a reached live-storey place has a predecessor");
        route.push(direction);
        here = previous;
    }
    route.reverse();
    route
}

fn reconstruct_live_into(
    visited: &FxHashMap<Point, ReverseLiveVisit>,
    mut here: Point,
    goal: Point,
) -> Vec<Direction> {
    let mut route = Vec::new();
    while here != goal {
        let (next, direction) = visited[&here]
            .next
            .expect("a reached reverse live-storey place has a successor");
        route.push(direction);
        here = next;
    }
    route
}

/// The places a flood is being run for, and how many of them are still
/// unanswered.
///
/// A uniform-cost flood finalises a place the first time it reaches it, so once
/// every place the caller asked about has a cost there is nothing left for the
/// flood to learn. Both callers ask about a handful of *nodes* in a rectangle
/// that holds a thousand places — the bake about the other portals of one
/// region, a query about the portals its endpoint might join — so stopping there
/// keeps the traversal proportional to the portals it has to answer for rather
/// than to the region it runs over.
struct Sought {
    marked: Vec<bool>,
    left:   usize,
}

impl Sought {
    fn of(wanted: &[usize], places: usize) -> Self {
        let mut marked = vec![false; places];
        let mut left = 0;
        for &slot in wanted {
            if !marked[slot] {
                marked[slot] = true;
                left += 1;
            }
        }
        Self { marked, left }
    }

    /// Whether the flood may stop, `slot` having just been given its cost.
    fn done(&mut self, slot: usize) -> bool {
        if self.marked[slot] {
            self.left -= 1;
        }
        self.left == 0
    }
}

/// Exact uniform-cost routes from one place to every place in one small region.
/// One traversal replaces the old one-A*-per-node-pair construction while
/// retaining directed movement and height resolution through the step rule.
///
/// **To every place and not to every tile**, so a route to a bridge deck is not
/// answered by the cost of reaching the road under it. The traversal is
/// one-directional and always was, which is what makes a one-way drop cost what
/// it really costs from this side and nothing at all from the other.
///
/// `wanted` is the places the answer is for, and the traversal stops once they
/// all have one — see [`Sought`]. Every other slot of the returned array is then
/// `None` because the flood stopped, which is not the same `None` as *no route*;
/// nothing may read a slot it did not ask about.
fn region_costs(
    footing: &Footing<'_>,
    local: &RegionPlaces,
    from: Point,
    wanted: &[usize],
) -> (Vec<Option<u32>>, usize) {
    let mut costs = vec![None; local.len()];
    let mut sought = Sought::of(wanted, local.len());
    let mut open = VecDeque::new();
    // What the flood cost the query, in the same unit a search charges: one per
    // place whose neighbours were asked for. See `Effort`.
    let mut expanded = 0;
    let start = local.slot(from).expect("a node is a place of its own region");
    costs[start] = Some(0);
    if sought.done(start) {
        return (costs, expanded);
    }
    open.push_back((from, start));
    while let Some((point, slot)) = open.pop_front() {
        let cost = costs[slot].expect("queued places have a cost");
        expanded += 1;
        for next in steps_out_of(footing, point).into_iter().flatten() {
            let Some(at) = local.slot(next) else {
                continue;
            };
            if costs[at].is_none() {
                costs[at] = Some(cost + 1);
                if sought.done(at) {
                    return (costs, expanded);
                }
                open.push_back((next, at));
            }
        }
    }
    (costs, expanded)
}

/// The same traversal read backwards: what it costs to reach `to` from every
/// place in one small region.
///
/// [`region_costs`] answers the other question, and on this step rule they are
/// not each other's mirror — a ledge a body drops off is an edge one way and no
/// edge the other, which is the whole of N4's second half. A join that asked
/// only the forward one would offer a corridor whose last hop nothing can walk.
///
/// **There is no reverse step rule to ask**, so a place's predecessors are found
/// by asking its neighbours where *they* land. A step lands on one of the eight
/// neighbouring columns, so every predecessor of a place stands in one of them,
/// and there are no others to consider. Each candidate's own expansion is
/// computed once and kept, so the traversal costs one
/// [`steps_out_of`] per place it *touches* — the flooded set and its border —
/// rather than one per pop.
fn region_costs_into(
    footing: &Footing<'_>,
    local: &RegionPlaces,
    to: Point,
    wanted: &[usize],
) -> (Vec<Option<u32>>, usize) {
    let mut costs = vec![None; local.len()];
    // Each place's landings by the region's own numbering, resolved the first
    // time the traversal asks a place where it goes. `NO_NEIGHBOR` is a landing
    // outside the region or no landing at all, and neither is a predecessor of
    // anything in here.
    let mut steps = vec![[NO_NEIGHBOR; Direction::ALL.len()]; local.len()];
    let mut asked = vec![false; local.len()];
    let mut sought = Sought::of(wanted, local.len());
    let mut open = VecDeque::new();
    // One per place this asks where it goes, which is what an expansion is here:
    // the traversal pops a place many times over and resolves each one once.
    let mut expanded = 0;
    let start = local.slot(to).expect("the endpoint is a place of its own region");
    costs[start] = Some(0);
    if sought.done(start) {
        return (costs, expanded);
    }
    open.push_back(start);
    while let Some(slot) = open.pop_front() {
        let cost = costs[slot].expect("queued places have a cost");
        let here = local.points[slot];
        for direction in Direction::ALL {
            let Some(neighbor) = crate::step_from(here, direction) else {
                continue;
            };
            for candidate in local.column(neighbor.x, neighbor.y) {
                if costs[candidate].is_some() {
                    continue;
                }
                if !asked[candidate] {
                    for (bits, landing) in steps_out_of(footing, local.points[candidate])
                        .into_iter()
                        .enumerate()
                    {
                        steps[candidate][bits] = landing
                            .and_then(|at| local.slot(at))
                            .map_or(NO_NEIGHBOR, |at| at as u32);
                    }
                    asked[candidate] = true;
                    expanded += 1;
                }
                if steps[candidate].contains(&(slot as u32)) {
                    costs[candidate] = Some(cost + 1);
                    if sought.done(candidate) {
                        return (costs, expanded);
                    }
                    open.push_back(candidate);
                }
            }
        }
    }
    (costs, expanded)
}

fn distance(from: Point, to: Point) -> u32 {
    i32::from(from.x)
        .abs_diff(i32::from(to.x))
        .max(i32::from(from.y).abs_diff(i32::from(to.y)))
}

/// Why a long-path query ended.
///
/// The four refusals used to be one string — `unreachable_or_live_refinement`
/// — which is four different repairs wearing one word. Telling them apart is
/// what the facet-0 oracle needed to say *why* the router refuses a route past
/// one region, rather than only that it does. See `docs/map/terrain_seam.md`.
///
/// **It was diagnostics-only until a player asked.** A client that cannot plan
/// a route owes the person who clicked a reason, and the reasons here are the
/// only honest ones there are: a goal nothing reaches is not the same answer as
/// a query that ran out of effort, and telling a player the wrong one of those
/// sends them looking for a way round a wall that has none — or standing still
/// where one more click would have worked. See [`search_long_path`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongExit {
    /// A route came back.
    Route,
    /// One or both endpoints are on a tile the static graph has no region for.
    OffGraph,
    /// An ordinary endpoint's own 32×32 region, or a live endpoint's bounded
    /// storey connector, has no portal it can walk to. Nothing about the rest
    /// of the facet was even consulted.
    NoJoin,
    /// Endpoints join the graph, and no corridor of portals connects them.
    NoCorridor,
    /// A corridor existed and live refinement failed every hop it was given,
    /// until [`LIVE_REROUTES`] retries ran out.
    PortalsExhausted,
    /// [`LONG_PATH_EFFORT`] was spent before the query finished. Its predecessor
    /// was `Deadline`, and the difference is the whole point: this one is a fact
    /// about the ground the query was asked over, and the same query over the
    /// same ground spends it again.
    Spent,
}

/// How many corridors refinement may reject before the query gives up.
///
/// A constant, while the number of hops a corridor has grows with the distance
/// asked for — so a long route has more chances to spend a retry than a short
/// one has, on the same ground.
const LIVE_REROUTES: usize = 8;

/// How far past the first static portal a live-storey join keeps flooding.
///
/// A player house can carry a route through several baked regions before its
/// staircase reaches the map again: every upper floor is in the live layer,
/// while every portal in [`NavigationGraph`] is at a height the static map
/// owns. Stopping at the first portal would make that portal a single point of
/// failure under a crate or a shut door. Sixty-four more steps covers two whole
/// regions and gives the abstract search alternatives without turning an
/// isolated live platform into a facet-wide flood.
const LIVE_JOIN_SLACK: u32 = 64;

/// The shortest failed search worth asking the graph about, in tiles.
///
/// A coarse graph is counterproductive after a short search has exhausted the
/// reachable component: joining an endpoint is another flood, at *both* ends,
/// and cannot invent an exit. A budget refusal is not that answer. Several live
/// storeys can put hundreds of places inside eight tiles, so callers let
/// `Budget` fall through to the dynamic storey join while `Exhausted` remains a
/// final short refusal.
///
/// A property of this router rather than of any one caller, which is why it
/// lives beside [`find_long_path`] and not beside the budgets: the client's
/// click-to-walk and the shard's chase read the same number, and a fall-back
/// the two ends drew at different distances would be two answers to "how far
/// can a body plan".
pub const COARSE_MIN_DISTANCE: u32 = 8;

/// Refine a route proposed by a static navigation graph through live terrain.
///
/// The weight is the refinement's, and it reaches every exact search this
/// query makes — a corridor hop is a body's own walking, so it is planned the
/// way a body's walking is. It does **not** reach the graph: an edge cost is
/// baked, and what it says about the facet is what the corridor picks by.
/// The same query as [`find_long_path`], reported rather than answered.
///
/// [`search_path`](crate::search_path) is to [`find_path`](crate::find_path)
/// what this is to `find_long_path`, and it exists for the same kind of caller:
/// one that has to say *why*. A client whose click cannot be routed tells the
/// person what stopped it, and the difference between [`LongExit::NoCorridor`]
/// — the facet has no way there — and [`LongExit::Spent`] — this query ran out
/// of effort — is the difference between "you cannot get there" and "ask again
/// from closer".
#[must_use]
pub fn search_long_path(
    guide: &Footing<'_>,
    footing: &Footing<'_>,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    budget: usize,
    weight: Weight,
) -> (Option<Vec<Direction>>, LongExit) {
    let started = debug_enabled().then(Instant::now);
    // One wallet for the whole query, and the only ceiling over it. What used to
    // be here as well — a second, later reading of the clock, which threw away a
    // route that arrived just after the deadline — has nothing to do: a counted
    // ceiling cannot be passed *between* two reads of it, so a route that comes
    // back is a route that was paid for.
    let mut effort = Effort::of(LONG_PATH_EFFORT);
    let rigour = Rigour { budget, weight };
    let (result, exit) = find_long_path_inner(guide, footing, graph, from, to, rigour, &mut effort);
    debug_long_path(
        from,
        to,
        budget,
        started,
        effort.spent(),
        result.as_ref().map(Vec::len),
        exit,
    );
    (result, exit)
}

#[must_use]
pub fn find_long_path(
    guide: &Footing<'_>,
    footing: &Footing<'_>,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    budget: usize,
    weight: Weight,
) -> Option<Vec<Direction>> {
    search_long_path(guide, footing, graph, from, to, budget, weight).0
}

fn find_long_path_inner(
    guide: &Footing<'_>,
    footing: &Footing<'_>,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    rigour: Rigour,
    effort: &mut Effort,
) -> (Option<Vec<Direction>>, LongExit) {
    // A click names the art it hit; the route is to the standing place that art
    // resolves to. The bounded search already makes this resolution internally,
    // but a live-storey join keeps a suffix to the exact point and therefore
    // must name the same destination before it starts.
    let to = destination_place(footing, from, to);
    let (Some(from_region), Some(to_region)) = (graph.region_at(from), graph.region_at(to)) else {
        return (None, LongExit::OffGraph);
    };
    // One region is the exact search's own case, so it is asked first — and its
    // refusal is not the answer, because a region is a rectangle and not a
    // component. Two points inside one whose only connection leaves it and comes
    // back are joined by a corridor and by nothing else, and the graph is what
    // holds that corridor: a local refusal falls through to it rather than
    // standing as the verdict. What that costs is `local_costs` over one region
    // twice, which is the price of an answer where there used to be none.
    let local = match from_region == to_region {
        true => region_route(footing, graph.regions[from_region.0], from, to, rigour, effort),
        false => None,
    };
    if let Some(route) = local {
        return (Some(route), LongExit::Route);
    }
    let mut forbidden = vec![false; graph.nodes.len()];
    // Refinement can forbid several portals and retry the abstract route, but
    // the endpoint's own flood does not change between those retries — a portal
    // the corridor may not use is one `abstract_path` skips, not one the ground
    // stopped reaching. Joined once, read every retry.
    let source = match NavigationGraph::needs_live_join(footing, from) {
        true => graph.live_join(footing, from, Join::OutOf, effort),
        false => EndpointJoin::static_costs(graph.local_costs(guide, from_region, from, Join::OutOf, effort)),
    };
    let target = match NavigationGraph::needs_live_join(footing, to) {
        true => graph.live_join(footing, to, Join::Into, effort),
        false => EndpointJoin::static_costs(graph.local_costs(guide, to_region, to, Join::Into, effort)),
    };
    if effort.spent_out() {
        return (None, LongExit::Spent);
    }
    if source.costs.is_empty() || target.costs.is_empty() {
        return (None, LongExit::NoJoin);
    }
    for _ in 0..=LIVE_REROUTES {
        if effort.spent_out() {
            return (None, LongExit::Spent);
        }
        let Some(path) = graph.abstract_path(from, to, &forbidden, &source.costs, &target.costs) else {
            return (None, LongExit::NoCorridor);
        };
        match graph.refine(footing, (from, to), &path, (&source, &target), rigour, effort) {
            Ok(route) => return (Some(route), LongExit::Route),
            Err(node) if !forbidden[node.0] => graph.forbid_portal(node, &mut forbidden),
            Err(_) => return (None, LongExit::PortalsExhausted),
        }
    }
    (None, LongExit::PortalsExhausted)
}

/// `started` is `None` unless diagnostics were asked for, and `spent` is what
/// the query's wallet paid out — the reading [`LONG_PATH_EFFORT`] is argued
/// from, and the one number here that is the same on every machine.
fn debug_long_path(
    from: Point,
    to: Point,
    budget: usize,
    started: Option<Instant>,
    spent: usize,
    route_len: Option<usize>,
    exit: LongExit,
) {
    let Some(started) = started else {
        return;
    };
    let elapsed = started.elapsed();
    let threshold = std::env::var("OPENSHARD_PATH_DEBUG_MS")
        .ok()
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or(10);
    if elapsed.as_millis() < threshold {
        return;
    }
    eprintln!(
        "path-debug kind=find_long_path from=({}, {}, {}) to=({}, {}, {}) budget={budget} elapsed_ms={:.3} nodes={spent} exit={exit:?} route_steps={route_len:?}",
        from.x,
        from.y,
        from.z,
        to.x,
        to.y,
        to.z,
        elapsed.as_secs_f64() * 1_000.0,
    );
}

fn cross_portal(footing: &Footing<'_>, from: Point, to: Point) -> Option<Vec<Direction>> {
    let direction = match (to.x.cmp(&from.x), to.y.cmp(&from.y)) {
        (std::cmp::Ordering::Greater, std::cmp::Ordering::Equal) => Direction::East,
        (std::cmp::Ordering::Less, std::cmp::Ordering::Equal) => Direction::West,
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Greater) => Direction::South,
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Less) => Direction::North,
        _ => return None,
    };
    step_allowed(footing, from, direction)
        .filter(|landing| landing.x == to.x && landing.y == to.y)
        .map(|_| vec![direction])
}

fn region_route(
    footing: &Footing<'_>,
    region: Region,
    from: Point,
    to: Point,
    rigour: Rigour,
    effort: &mut Effort,
) -> Option<Vec<Direction>> {
    let hop = u16::try_from((rigour.budget / 2).max(1)).unwrap_or(u16::MAX);
    let mut route = Vec::new();
    let mut at = from;
    while distance(at, to) > u32::from(hop) {
        if effort.spent_out() {
            return None;
        }
        // Aim at the real destination and keep the closest result when the
        // bounded search runs out. A synthetic point exactly `hop` tiles away
        // can itself be a tree, which must not make a whole forest unroutable.
        let segment = find_path_toward_within(footing, at, to, rigour, effort, Some(region))?;
        at = append(footing, at, &segment, &mut route)?;
    }
    let segment = find_path_within(footing, at, to, rigour, effort, Some(region))?;
    append(footing, at, &segment, &mut route)?;
    Some(route)
}

fn append(
    footing: &Footing<'_>,
    from: Point,
    route: &[Direction],
    out: &mut Vec<Direction>,
) -> Option<Point> {
    let mut at = from;
    for &direction in route {
        at = step_allowed(footing, at, direction)?;
        out.push(direction);
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use openshard_map::overlay::{
        Cover,
        Doors,
        Overlay,
    };
    use proptest::prelude::*;

    use super::*;
    use crate::find_path;
    use crate::scene::Scene;

    /// A bounded open grid with some tiles blocked.
    ///
    /// A real map for the ground and the heights, and an overlay for what is in
    /// the way — which is what the shard and the client both are. It used to be
    /// a `Terrain` implementation of its own, and that is the whole of what
    /// node E took away: a fixture that reimplements the step rule agrees with
    /// itself and proves nothing.
    struct Grid {
        scene:   Scene,
        blocked: Overlay,
    }

    impl Grid {
        fn open(width: u16, height: u16) -> Self {
            // A scene is a whole number of 8×8 blocks, and a fixture asks for
            // the rectangle it wants: 20 by 14 is 24 by 16 of map. What is left
            // over is fenced off, so the grid's edge refuses a step the way the
            // double's bounds check used to.
            let scene = Scene::flat_holding(width - 1, height - 1, 0);
            let mut grid = Self {
                scene,
                blocked: Overlay::default(),
            };
            for y in 0..grid.scene.height() {
                for x in 0..grid.scene.width() {
                    if x >= width || y >= height {
                        grid.block(x, y);
                    }
                }
            }
            grid
        }

        fn block(&mut self, x: u16, y: u16) {
            self.blocked.set(Tile::new(x, y), vec![Cover::blocking(0, 20)]);
        }

        fn footing(&self) -> Footing<'_> {
            Footing::new(Some(self.scene.terrain()), &self.blocked, Doors::AsTheyStand)
        }
    }

    /// A long upper floor laid over the map, fenced on both sides and reached
    /// by ten two-unit treads at its east end.
    ///
    /// The floor crosses the x=32 baked-region boundary while the only way back
    /// to static ground is in the next region. That is the shape a placed house
    /// adds which a map-only endpoint join cannot represent: its portal at the
    /// boundary exists at z=0, while the body crosses it at z=20.
    fn live_storey() -> Grid {
        let mut grid = Grid::open(96, 32);
        let floor = |z| Cover::standing(z, 0);
        for x in 4..=40 {
            grid.blocked.set(Tile::new(x, 10), vec![floor(20)]);
        }
        for x in 41..=50 {
            let z = 20 - i8::try_from((x - 40) * 2).unwrap();
            grid.blocked.set(Tile::new(x, 10), vec![floor(z)]);
        }
        // Whole-tile wall art on the two neighbouring rows. The standing half
        // is what prevents `landing` from treating the blocked upper edge as an
        // unguarded drop to the ground under it.
        for x in 3..=50 {
            for y in [9, 11] {
                grid.blocked
                    .set(Tile::new(x, y), vec![Cover::blocking(0, 40), floor(20)]);
            }
        }
        grid.blocked
            .set(Tile::new(3, 10), vec![Cover::blocking(0, 40), floor(20)]);
        // A ground-floor arch: somebody downstairs can leave the house here,
        // while the wall still keeps the upper floor from becoming an
        // unguarded drop. The staircase remains the only way *up*.
        grid.blocked
            .set(Tile::new(4, 9), vec![Cover::blocking(16, 24), floor(20)]);
        grid
    }

    fn end(footing: &Footing<'_>, from: Point, route: &[Direction]) -> Point {
        route.iter().fold(from, |at, &direction| {
            step_allowed(footing, at, direction).unwrap()
        })
    }

    #[test]
    fn an_open_facet_has_only_bounded_coarse_regions() {
        let terrain = Grid::open(704, 32);
        let graph = NavigationGraph::build(&terrain.footing(), 704, 32).unwrap();
        assert_eq!(graph.regions.len(), 22);
        assert_eq!(graph.nodes.len(), 84);
        let from = Point::new(1, 1, 0);
        let to = Point::new(702, 30, 0);
        let route = find_long_path(
            &terrain.footing(),
            &terrain.footing(),
            &graph,
            from,
            to,
            100,
            Weight::EXACT,
        )
        .unwrap();
        assert_eq!(end(&terrain.footing(), from, &route), to);
    }

    /// A placed house is absent from the baked graph. Its upper floor crosses a
    /// region border at a height the graph has no node for, and its staircase
    /// is further away than the exact search's deliberately tiny budget.
    ///
    /// Both directions matter: climbing is bounded while descending is not, so
    /// a reverse join made by reversing the outward edges would pass this first
    /// assertion and fail the second.
    #[test]
    fn a_live_storey_joins_the_static_graph_in_both_directions() {
        let terrain = live_storey();
        let empty = Overlay::default();
        let guide = Footing::new(Some(terrain.scene.terrain()), &empty, Doors::AsTheyStand);
        let graph = NavigationGraph::build(&guide, 96, 32).unwrap();
        let live = terrain.footing();
        let upstairs = Point::new(4, 10, 20);
        let street = Point::new(90, 10, 0);

        let local = crate::search_path(&live, upstairs, street, 20, Weight::EXACT);
        assert_eq!(
            local.exit,
            crate::SearchExit::Budget,
            "the fixture stopped needing the graph"
        );
        let out = find_long_path(&guide, &live, &graph, upstairs, street, 20, Weight::EXACT)
            .expect("the live upper floor should join the static route outside");
        assert_eq!(end(&live, upstairs, &out), street);

        let into = find_long_path(&guide, &live, &graph, street, upstairs, 20, Weight::EXACT)
            .expect("the static route should join the live staircase upward");
        assert_eq!(end(&live, street, &into), upstairs);
    }

    /// The old tile-keyed answer to this query was an empty route: both places
    /// have `(4, 10)`. The place-keyed exact search fixed that for a small
    /// house; the live-storey join keeps the answer when a larger house spends
    /// the exact budget before reaching its stairs.
    #[test]
    fn a_long_house_route_reaches_another_floor_of_the_same_column() {
        let terrain = live_storey();
        let empty = Overlay::default();
        let guide = Footing::new(Some(terrain.scene.terrain()), &empty, Doors::AsTheyStand);
        let graph = NavigationGraph::build(&guide, 96, 32).unwrap();
        let live = terrain.footing();
        let downstairs = Point::new(4, 10, 0);
        let upstairs = Point::new(4, 10, 20);

        let local = crate::search_path(&live, downstairs, upstairs, 20, Weight::EXACT);
        assert_eq!(
            local.exit,
            crate::SearchExit::Budget,
            "the fixture stopped needing the graph"
        );
        let route = find_long_path(&guide, &live, &graph, downstairs, upstairs, 20, Weight::EXACT)
            .expect("the route should leave the column, climb the house, and return upstairs");
        assert!(
            !route.is_empty(),
            "two floors of one column became an empty route again"
        );
        assert_eq!(end(&live, downstairs, &route), upstairs);
    }

    #[test]
    fn a_wall_opening_becomes_a_portal_between_derived_regions() {
        let mut terrain = Grid::open(96, 64);
        for y in 0..64 {
            if y != 40 {
                terrain.block(48, y);
            }
        }
        let graph = NavigationGraph::build(&terrain.footing(), 96, 64).unwrap();
        let from = Point::new(2, 2, 0);
        let to = Point::new(93, 2, 0);
        let route = find_long_path(
            &terrain.footing(),
            &terrain.footing(),
            &graph,
            from,
            to,
            100,
            Weight::EXACT,
        )
        .unwrap();
        assert_eq!(end(&terrain.footing(), from, &route), to);
        let mut at = from;
        for direction in route {
            at = step_allowed(&terrain.footing(), at, direction).unwrap();
            assert!(at.x != 48 || at.y == 40);
        }
    }

    /// A region is a rectangle and not a component, so one of them may need a
    /// corridor out of itself.
    ///
    /// `find_long_path` used to answer a query whose endpoints share a region
    /// with `region_route` alone, and that search is *confined to the 32×32
    /// rectangle* — so two points joined only by a way that leaves the region
    /// and comes back were refused outright, over ground the exact search
    /// walks, and the graph beside them was never consulted. The local route is
    /// a first attempt now rather than the verdict.
    #[test]
    fn two_points_in_one_region_route_by_leaving_it() {
        let mut terrain = Grid::open(64, 64);
        // A wall the length of region 0 and no further, so the only way from
        // its west half to its east half is south into the region below,
        // across, and back north.
        for y in 0..32 {
            terrain.block(16, y);
        }
        let footing = terrain.footing();
        let graph = NavigationGraph::build(&footing, 64, 64).unwrap();
        let from = Point::new(4, 4, 0);
        let to = Point::new(28, 4, 0);
        assert_eq!(
            graph.region_at(from),
            graph.region_at(to),
            "the case is two endpoints of one region"
        );
        // The oracle that the ground joins them at all, and it is the search
        // this router is the fall-back for: exhaustive A* over the whole grid.
        assert!(
            find_path(&footing, from, to, 64 * 64, Weight::EXACT).is_some(),
            "the way round is walkable"
        );

        let route = find_long_path(&footing, &footing, &graph, from, to, 600, Weight::EXACT)
            .expect("a corridor answers");
        assert_eq!(end(&footing, from, &route), to);
        // And it is the corridor that answered rather than the local search:
        // `region_route` cannot leave the rectangle, so a step outside it is
        // the graph's own work.
        let mut at = from;
        let left_the_region = route.iter().any(|&direction| {
            at = step_allowed(&footing, at, direction).unwrap();
            at.y >= 32
        });
        assert!(left_the_region, "the way through is outside region 0");
    }

    #[test]
    fn a_forest_does_not_put_portals_around_every_tree() {
        let mut terrain = Grid::open(128, 128);
        for y in (4..124).step_by(4) {
            for x in (4..124).step_by(4) {
                terrain.block(x, y);
            }
        }
        let graph = NavigationGraph::build(&terrain.footing(), 128, 128).unwrap();
        assert_eq!(graph.regions.len(), 16);
        assert!(
            graph.nodes.len() < 500,
            "{} nodes for 900 trees",
            graph.nodes.len()
        );
        let from = Point::new(1, 1, 0);
        let to = Point::new(126, 126, 0);
        let route = find_long_path(
            &terrain.footing(),
            &terrain.footing(),
            &graph,
            from,
            to,
            600,
            Weight::EXACT,
        )
        .unwrap();
        assert_eq!(end(&terrain.footing(), from, &route), to);
    }

    #[test]
    fn border_trees_share_one_logical_entrance() {
        let mut terrain = Grid::open(64, 32);
        // Every remaining crossing is isolated by trees on both sides of the
        // border, but the two region interiors are still each one component.
        for y in (1..32).step_by(2) {
            terrain.block(31, y);
            terrain.block(32, y);
        }
        let graph = NavigationGraph::build(&terrain.footing(), 64, 32).unwrap();
        // Two maximally separated representatives make four directed nodes;
        // raw contiguous runs would have made 32.
        assert_eq!(graph.nodes.len(), 4);
        let from = Point::new(2, 2, 0);
        let to = Point::new(61, 29, 0);
        let route = find_long_path(
            &terrain.footing(),
            &terrain.footing(),
            &graph,
            from,
            to,
            600,
            Weight::EXACT,
        )
        .unwrap();
        assert_eq!(end(&terrain.footing(), from, &route), to);
    }

    #[test]
    fn component_pairs_keep_separate_gates() {
        let mut terrain = Grid::open(64, 32);
        // This wall divides only the left region.  Both halves have broad
        // crossings to the single component in the right region, so they must
        // remain two logical entrances rather than being merged by proximity.
        for x in 0..32 {
            terrain.block(x, 16);
        }
        let graph = NavigationGraph::build(&terrain.footing(), 64, 32).unwrap();
        assert_eq!(graph.nodes.len(), 8);
    }

    /// A raised walkway of statics, one tile wide, running west from `(x, 12)`.
    ///
    /// **The shape the graph could not hold.** Land cannot make a ledge — a land
    /// climb is not bounded by [`MAX_STEP_UP`](crate::MAX_STEP_UP), see
    /// `MapTerrain::check`'s land branch, and land heights interpolate between
    /// neighbouring cells, so even a cliff is a ramp walked both ways. A floor's
    /// top *is* a hard edge, and the ground under a floor laid flat on it is not
    /// somewhere a body fits — so a walkway of floors is a surface that exists
    /// only as a static, at a height `ground_z` does not report.
    fn walkway(scene: &mut Scene, xs: std::ops::RangeInclusive<u16>, height: u8) {
        for x in xs {
            scene.floor(x, 12, 0, height);
        }
    }

    /// A body may step off a ledge and not climb back onto it, and the graph is
    /// what used to lose that.
    ///
    /// A portal was minted only where the step succeeded **both** ways, so a
    /// one-way border was not a portal at all — a refusal rather than a lie,
    /// which is the right side of the error and still a refusal. Since N4 a
    /// crossing is one direction and its reverse is its own edge, so a route off
    /// the walkway exists and a route back up does not.
    ///
    /// A test stood here over a `OneWayGrid` double whose `can_step` refused one
    /// direction outright, and it was deleted in the terrain-seam work because
    /// no *ground* does that — the builder's own input could not contain the
    /// situation, since it sampled `ground_z` and a walkway of floors came back
    /// unwalkable. This is that test owed back, over ground.
    #[test]
    fn a_ledge_is_a_portal_one_way_and_no_portal_the_other() {
        let mut scene = Scene::flat_holding(63, 31, 0);
        // The walkway crosses the region border at x=31/32 and stops there, so
        // the only way off it is the drop east.
        walkway(&mut scene, 0..=31, 5);
        let footing = scene.footing();
        let graph = NavigationGraph::build(&footing, 64, 32).unwrap();

        let high = Point::new(2, 12, 5);
        let low = Point::new(61, 12, 0);
        // The step itself, so the test says what ground it is about before it
        // says what the graph did with it.
        assert_eq!(
            step_allowed(&footing, Point::new(31, 12, 5), Direction::East),
            Some(Point::new(32, 12, 0)),
            "a body steps off the walkway"
        );
        assert_eq!(
            step_allowed(&footing, Point::new(32, 12, 0), Direction::West),
            None,
            "and cannot climb back onto it"
        );

        let route = find_long_path(&footing, &footing, &graph, high, low, 600, Weight::EXACT)
            .expect("the drop off the walkway is a route");
        assert_eq!(end(&footing, high, &route), low);
        assert!(
            find_long_path(&footing, &footing, &graph, low, high, 600, Weight::EXACT).is_none(),
            "nothing climbs back onto a five-unit ledge"
        );

        // And the graph says it in its own terms: a crossing edge with no
        // reverse. `edge_targets` is directed storage, so this is a statement
        // about the bake rather than about the router reading it.
        let one_way = graph.nodes.iter().enumerate().any(|(from, _)| {
            graph
                .edges_from(NodeId(from))
                .any(|edge| edge.cost == 1 && !graph.edges_from(edge.to).any(|back| back.to.0 == from))
        });
        assert!(one_way, "a one-way ledge is a one-way edge");
    }

    /// Joining an endpoint is a directed question, and the two answers differ.
    ///
    /// The endpoint join stopped being a fan-out of exact searches and became a
    /// flood, and a flood is the shape that could quietly lose this: reading the
    /// step rule forwards for both ends is one line shorter and answers "can the
    /// endpoint reach that portal" where the target side asked "can that portal
    /// reach the endpoint". On a step rule where a body drops off a ledge and
    /// cannot climb back, those are different sets — so a target joined forwards
    /// would offer corridors whose last hop nothing can walk.
    ///
    /// The walkway here stops well short of the region border, so no node stands
    /// on it: a body up there can leave and nothing can arrive, and the join has
    /// to say both.
    #[test]
    fn a_one_way_drop_joins_an_endpoint_one_way() {
        let mut scene = Scene::flat_holding(63, 31, 0);
        walkway(&mut scene, 2..=20, 5);
        let footing = scene.footing();
        let graph = NavigationGraph::build(&footing, 64, 32).unwrap();

        let high = Point::new(10, 12, 5);
        let region = graph.region_at(high).expect("the walkway is walkable ground");
        // The ground first, so the test says what it is about before it says
        // what the graph did with it.
        assert_eq!(
            step_allowed(&footing, high, Direction::North),
            Some(Point::new(10, 11, 0)),
            "a body steps off the walkway"
        );
        assert_eq!(
            step_allowed(&footing, Point::new(10, 11, 0), Direction::South),
            None,
            "and cannot climb back onto it"
        );

        let mut effort = Effort::of(LONG_PATH_EFFORT);
        let out_of = graph.local_costs(&footing, region, high, Join::OutOf, &mut effort);
        assert_eq!(
            out_of.len(),
            graph.nodes_in_region(region).count(),
            "a body that drops off the walkway reaches every portal of its region"
        );
        assert!(
            graph
                .local_costs(&footing, region, high, Join::Into, &mut effort)
                .is_empty(),
            "and no portal of it reaches the walkway"
        );

        // And the router says the same thing, from a region away.
        let low = Point::new(60, 12, 0);
        let down = find_long_path(&footing, &footing, &graph, high, low, 600, Weight::EXACT)
            .expect("the drop is a route");
        assert_eq!(end(&footing, high, &down), low);
        assert!(
            find_long_path(&footing, &footing, &graph, low, high, 600, Weight::EXACT).is_none(),
            "nothing climbs back onto a five-unit ledge"
        );
    }

    /// A plateau reached only by a flight of steps is not an island.
    ///
    /// **N4's headline, in one scene.** The sampler took `ground_z` — the land
    /// alone — so every tile of the stair and of the plateau over it came back
    /// at the ground beneath, where a body does not fit; the whole climb was
    /// invisible and the plateau had no portal to anywhere. It is Britain's
    /// castle in miniature: land at one height, a raised place at another, and
    /// nothing but statics between them.
    #[test]
    fn a_plateau_reached_by_stairs_is_not_an_island() {
        let mut scene = Scene::flat_holding(63, 31, 0);
        // Fifteen steps of two, from the plain at x=38 up to the plateau at
        // x=24, and the plateau itself west of that. The border at x=31/32 falls
        // half way up, so the portal that joins the two regions is sixteen units
        // above the land the old sampler would have found there.
        for x in 24..=38u16 {
            scene.floor(x, 12, 0, 2 * (39 - x) as u8);
        }
        walkway(&mut scene, 0..=23, 30);
        let footing = scene.footing();
        let graph = NavigationGraph::build(&footing, 64, 32).unwrap();

        assert!(
            graph.nodes.iter().any(|node| node.point.z > 0),
            "a graph that samples the land alone has every node at z=0"
        );
        let plain = Point::new(60, 12, 0);
        let plateau = Point::new(2, 12, 30);
        let route = find_long_path(&footing, &footing, &graph, plain, plateau, 600, Weight::EXACT)
            .expect("the stair is a route onto the plateau");
        assert_eq!(end(&footing, plain, &route), plateau);
        let down = find_long_path(&footing, &footing, &graph, plateau, plain, 600, Weight::EXACT)
            .expect("and a route back down");
        assert_eq!(end(&footing, plateau, &down), plain);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// The graph may choose a different corridor, but it must agree with
        /// exhaustive A* about whether the two fixed endpoints connect at all.
        #[test]
        fn randomized_static_maps_keep_a_star_reachability(
            blocked in prop::collection::vec(any::<bool>(), 20 * 14),
        ) {
            let mut terrain = Grid::open(20, 14);
            for (index, blocked) in blocked.into_iter().enumerate() {
                let x = (index % 20) as u16;
                let y = (index / 20) as u16;
                if blocked && (x, y) != (1, 1) && (x, y) != (18, 12) {
                    terrain.block(x, y);
                }
            }
            let from = Point::new(1, 1, 0);
            let to = Point::new(18, 12, 0);
            let exact = find_path(&terrain.footing(), from, to, 20 * 14, Weight::EXACT);
            let graph = NavigationGraph::build(&terrain.footing(), 20, 14).unwrap();
            let route =
                find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 20 * 14, Weight::EXACT);
            prop_assert_eq!(route.is_some(), exact.is_some());
            if let Some(route) = route {
                prop_assert_eq!(end(&terrain.footing(), from, &route), to);
            }
        }
    }
}

/// G1's oracle, on scenes: **a facet patched into shape holds the graph the same
/// facet baked whole holds.**
///
/// Compared as *places and costs* rather than as bytes, because a `NodeId` is an
/// index and two constructions are free to number the same places differently —
/// see [`NavigationGraph::shape`]. What that leaves is the only claim worth
/// making: the two graphs offer the same crossings between the same standing
/// places at the same prices, so every route either of them proposes, the other
/// proposes too.
#[cfg(test)]
mod rebake {
    use std::collections::{
        BTreeMap,
        BTreeSet,
    };

    use openshard_map::map::LandCell;
    use openshard_map::overlay::{
        Doors,
        Overlay,
    };
    use openshard_map::patch::{
        Patch,
        PatchAuthor,
        PatchOp,
        PatchTime,
    };
    use openshard_protocol::world::Facet;
    use openshard_tiles::TileData;

    use super::*;
    use crate::ground::Ground;
    use crate::scene::Scene;

    /// A place, and every place it can be crossed to, with what the crossing
    /// costs.
    type Shape = BTreeMap<(u16, u16, i8), BTreeSet<((u16, u16, i8), u32)>>;

    impl NavigationGraph {
        /// The graph as the world it describes rather than as the numbers it
        /// holds: which places are nodes, and what it costs to get between them.
        ///
        /// Dead entries fall out on their own — a node no region lists is not
        /// walked here, which is exactly what makes a repack invisible to
        /// everything above.
        fn shape(&self) -> Shape {
            let mut shape = Shape::new();
            for region in 0..self.regions.len() {
                for node in self.nodes_in_region(RegionId(region)) {
                    let at = self.nodes[node.0].point;
                    let edges = self
                        .edges_from(node)
                        .map(|edge| {
                            let to = self.nodes[edge.to.0].point;
                            ((to.x, to.y, to.z), edge.cost)
                        })
                        .collect();
                    let was = shape.insert((at.x, at.y, at.z), edges);
                    assert!(was.is_none(), "a place is one node and one region's");
                }
            }
            shape
        }
    }

    /// A facet three regions square, which is what it takes to have a region
    /// with neighbours on every side — and two chunks square, so a chunk's edge
    /// and a region's are two different seams.
    fn scene() -> Scene {
        let mut scene = Scene::flat_holding(95, 95, 0);
        // Something for the portal pass to have an opinion about: a wall down
        // the middle with one gap in it, and a plateau a body can walk up onto
        // from one side only.
        for y in 0..96 {
            if y != 40 {
                scene.wall(48, y, 0, 20);
            }
        }
        for y in 8..24 {
            for x in 8..24 {
                scene.ground(x, y, 10);
            }
        }
        for y in 8..24 {
            scene.stair(24, y, 0, 10);
        }
        scene
    }

    fn shard(scene: Scene) -> (Ground, TileData) {
        let (base, tiles) = scene.into_shard(Facet(0));
        (Ground::new(Some(base), &tiles), tiles)
    }

    /// The graph a whole-facet bake gives this ground.
    fn baked(ground: &Ground, tiles: &TileData) -> NavigationGraph {
        let nothing_placed = Overlay::default();
        let footing = Footing::new(ground.terrain(tiles), &nothing_placed, Doors::AsTheyStand);
        let (width, height) = match ground.snapshot() {
            Some(base) => (base.map().width(), base.map().height()),
            None => unreachable!("the fixture has ground"),
        };
        NavigationGraph::build(&footing, width, height).expect("a facet the graph can address")
    }

    /// Raise these cells by forty, published the way a `.setland` is — and hand
    /// back the chunks the patch moved, which is what the caller owes a rebake.
    ///
    /// **Cells rather than tiles, and never one of them alone.** A tile stands
    /// at the average of the four cells meeting at its north-west corner, and
    /// that average is taken over whichever *diagonal* of the four is the flatter
    /// — so raising a single cell raises no tile at all, which would make a
    /// fixture that looks like an edit and is not one.
    fn raise(ground: &mut Ground, tiles: &TileData, cells: &[(u16, u16)]) -> Vec<ChunkCoord> {
        let base = ground.snapshot().expect("the fixture has ground");
        let ops = cells
            .iter()
            .map(|&(x, y)| {
                let was = base.map().land(x, y).expect("a cell on this facet");
                PatchOp::set_land(
                    base.map(),
                    x,
                    y,
                    LandCell {
                        tile: was.tile,
                        z:    was.z.saturating_add(40),
                    },
                )
                .expect("a cell on this facet")
            })
            .collect();
        let patch = Patch::new(
            Facet(0),
            base.revision(),
            PatchAuthor("the rebake oracle".to_owned()),
            PatchTime(0),
            ops,
        );
        ground.publish(&patch, tiles).expect("the sample patch applies");
        patch.touched_chunks()
    }

    /// The four cells a tile stands on the average of.
    fn under(x: u16, y: u16) -> [(u16, u16); 4] {
        [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]
    }

    /// Put a wall where a body used to walk, which is the edit that takes a
    /// crossing *away* rather than moving one.
    ///
    /// The graphic is the fixture's own wall, copied out of the block it stands
    /// in: a static of a graphic the tile table has never heard of would be an
    /// item with no height, which is a weaker edit than the one this is for.
    fn wall_at(ground: &mut Ground, tiles: &TileData, x: u16, y: u16) -> Vec<ChunkCoord> {
        let base = ground.snapshot().expect("the fixture has ground");
        // Block (6, 1) is where the fixture's wall crosses x = 48, y = 8.
        let standing = *base
            .map()
            .statics_in_block(6, 1)
            .first()
            .expect("the fixture's wall stands in this block");
        let op = PatchOp::add_static(
            base.map(),
            openshard_map::map::StaticItem {
                x,
                y,
                z: 0,
                ..standing
            },
        )
        .expect("a tile on this facet");
        let patch = Patch::new(
            Facet(0),
            base.revision(),
            PatchAuthor("the rebake oracle".to_owned()),
            PatchTime(0),
            vec![op],
        );
        ground.publish(&patch, tiles).expect("the sample patch applies");
        patch.touched_chunks()
    }

    /// Rebake the graph over the ground as it now stands.
    fn follow(graph: &mut NavigationGraph, ground: &Ground, tiles: &TileData, chunks: &[ChunkCoord]) {
        let nothing_placed = Overlay::default();
        let footing = Footing::new(ground.terrain(tiles), &nothing_placed, Doors::AsTheyStand);
        graph.rebake_chunks(&footing, chunks);
    }

    /// The oracle itself, over the positions an edit can land on: inside a
    /// region, on a region's own first column and row, on the corner where both
    /// meet, and on a chunk's edge — which is a different seam from a region's,
    /// because a chunk is two regions wide.
    ///
    /// The edges are the point. A column's height is the average of the four
    /// cells meeting at its north-west corner, so an edit at `x % 32 == 0` is
    /// read by a column in the region to the west, and a rebake that took only
    /// the edited region would leave that column answering for the world as it
    /// was — with nothing about the edited region itself showing it.
    #[test]
    fn a_patched_graph_holds_what_a_whole_bake_holds() {
        let mut moved = 0;
        for (x, y) in [(40, 40), (32, 40), (40, 32), (32, 32), (64, 64), (63, 63)] {
            let (mut ground, tiles) = shard(scene());
            let mut graph = baked(&ground, &tiles);
            let before = graph.shape();

            let chunks = wall_at(&mut ground, &tiles, x, y);
            follow(&mut graph, &ground, &tiles, &chunks);

            assert_eq!(
                graph.shape(),
                baked(&ground, &tiles).shape(),
                "an edit at ({x}, {y}) left a seam"
            );
            moved += usize::from(graph.shape() != before);
        }
        assert!(moved > 0, "no edit moved the graph, so this oracle is asleep");
    }

    /// The same, for an edit to the *ground* rather than to what stands on it —
    /// the `.setland` an operator actually types, and the op whose read area is
    /// wider than the cell it names.
    #[test]
    fn a_raised_cell_is_followed_the_same_way() {
        for (x, y) in [(40, 40), (32, 40), (40, 32), (64, 64)] {
            let (mut ground, tiles) = shard(scene());
            let mut graph = baked(&ground, &tiles);

            let chunks = raise(&mut ground, &tiles, &under(x, y));
            follow(&mut graph, &ground, &tiles, &chunks);

            assert_eq!(
                graph.shape(),
                baked(&ground, &tiles).shape(),
                "a raise at ({x}, {y}) left a seam"
            );
        }
    }

    /// A crossing taken away, which is the edit a rebake can get wrong in a way
    /// a rebake of *more* ground cannot: a place that should not be a node any
    /// more, with edges still pointing at it.
    #[test]
    fn a_way_across_that_was_walled_off_leaves_nothing_behind() {
        let (mut ground, tiles) = shard(scene());
        let mut graph = baked(&ground, &tiles);
        // The fixture's wall runs the height of the facet with one gap in it, so
        // "an edge from one side of x = 48 to the other" is a fact about that gap
        // and about nothing else.
        let across = |shape: &Shape| {
            shape
                .iter()
                .any(|(&(x, _, _), edges)| x < 48 && edges.iter().any(|&((to, _, _), _)| to > 48))
        };
        assert!(
            across(&graph.shape()),
            "the gap is a way across before it is walled"
        );

        let chunks = wall_at(&mut ground, &tiles, 48, 40);
        follow(&mut graph, &ground, &tiles, &chunks);

        let whole = baked(&ground, &tiles);
        assert_eq!(graph.shape(), whole.shape());
        assert!(!across(&graph.shape()), "and nothing crosses it after");
        // The nodes the edit orphaned are still entries in `nodes`, which is the
        // point of the accounting: garbage is unreachable rather than absent,
        // until it outweighs the living.
        assert!(
            graph.nodes.len() >= whole.nodes.len(),
            "a rebake leaves its garbage where it is until a repack"
        );
    }

    /// The locality claim itself: a publish leaves the runs of a region it did
    /// not name exactly where they stood. A prefix sum could not do this, and it
    /// is the whole reason the addressing changed.
    #[test]
    fn a_publish_leaves_a_distant_regions_run_where_it_was() {
        let (mut ground, tiles) = shard(scene());
        let mut graph = baked(&ground, &tiles);
        let far = RegionId(graph.regions.len() - 1);
        let was = graph.region_runs[far.0];
        let nodes: Vec<_> = graph.nodes_in_region(far).collect();
        let edges: Vec<_> = nodes.iter().map(|&node| graph.edge_runs[node.0]).collect();

        let chunks = wall_at(&mut ground, &tiles, 8, 8);
        follow(&mut graph, &ground, &tiles, &chunks);

        assert_eq!(graph.region_runs[far.0], was, "a far region's list did not move");
        assert_eq!(
            nodes
                .iter()
                .map(|&node| graph.edge_runs[node.0])
                .collect::<Vec<_>>(),
            edges,
            "and neither did its nodes' edges"
        );
    }

    /// And a publish that changed no crossing at all writes nothing, which is
    /// the other half of the same rule: a run is rewritten because it differs,
    /// not because a pass looked at it. Without that, a brush stroke over open
    /// ground would manufacture garbage out of edits that moved nothing.
    #[test]
    fn a_publish_that_moved_no_crossing_leaves_no_garbage() {
        let (mut ground, tiles) = shard(scene());
        let mut graph = baked(&ground, &tiles);
        let before = graph.shape();

        let chunks = raise(&mut ground, &tiles, &under(40, 40));
        follow(&mut graph, &ground, &tiles, &chunks);

        assert_eq!(
            graph.shape(),
            before,
            "the fixture's open ground routes as it did"
        );
        assert_eq!((graph.dead_edges, graph.dead_region_nodes), (0, 0));
    }

    /// The garbage rule. Publishing the same tile up and down leaves orphaned
    /// runs behind every time, and once they outweigh the living the graph packs
    /// itself — which has to be invisible to everything above it.
    #[test]
    fn garbage_is_repacked_once_it_outweighs_the_living() {
        let (mut ground, tiles) = shard(scene());
        let mut graph = baked(&ground, &tiles);
        let mut packed = false;
        for round in 0..40_u16 {
            let x = 40 + round % 2;
            let chunks = raise(&mut ground, &tiles, &under(x, 40));
            follow(&mut graph, &ground, &tiles, &chunks);
            packed |= graph.dead_edges == 0 && graph.dead_region_nodes == 0 && round > 0;
            assert_eq!(
                graph.shape(),
                baked(&ground, &tiles).shape(),
                "round {round} disagrees with a whole bake"
            );
        }
        assert!(packed, "forty publishes never reached the repack rule");
    }

    /// An edit whose read area crosses a chunk's edge, on a facet wide enough
    /// for the rings to be smaller than it.
    ///
    /// The five-region facet is what makes this different from the tests above:
    /// on a three-region one the second ring is the whole world, so "the region
    /// after next" is not a place and nothing here could be missed.
    ///
    /// **On the widening, honestly.** The first ring is taken over the chunks'
    /// tiles *grown one west and north*, and no scene here turns that growth into
    /// a wrong answer — over bare land this step rule climbs any height at all, a
    /// slope being walkable and the two-unit limit being a rule about statics, so
    /// a land edit does not disconnect ground and the component split that would
    /// carry a change two regions out cannot be built out of one. What the growth
    /// buys is the argument rather than a caught failure: with it, every region
    /// whose *places* moved is inside the rebuilt set, which is what makes a
    /// border between a rebuilt region and a merely-sampled one unable to mint a
    /// node — and that is the claim [`NavigationGraph::intern_node`] fails loudly
    /// on. It is also the rule the span layer below already follows for the same
    /// reason, one scale down.
    #[test]
    fn an_edit_across_a_chunk_edge_is_followed_on_a_facet_with_room() {
        // Five regions square, so that "one region further out" is a place
        // rather than the edge of the facet.
        let mut scene = Scene::flat_holding(159, 159, 0);
        // A region cut in half but for one gap, and the gap is on the column an
        // edit in the chunk to its east moves.
        for x in 32..63 {
            scene.wall(x, 48, 0, 20);
        }
        // A ceiling over the gap, so that ground coming up under it is a change
        // to what stands there rather than only to how high it is.
        scene.floor(63, 48, 20, 2);
        let (mut ground, tiles) = shard(scene);
        let mut graph = baked(&ground, &tiles);
        let before = graph.shape();

        // The two cells of tile (63, 48) that lie east of the chunk edge — the
        // other diagonal stays where it is, and a tile stands on the average of
        // the flatter of the two.
        let chunks = raise(&mut ground, &tiles, &[(64, 48), (64, 49)]);
        follow(&mut graph, &ground, &tiles, &chunks);

        let whole = baked(&ground, &tiles);
        assert_eq!(
            graph.shape(),
            whole.shape(),
            "the edit reached further than the rebuilt regions"
        );
        assert_ne!(graph.shape(), before, "the ground under the ceiling moved");
    }

    /// A facet with no graph is a facet a publish leaves without one: nothing on
    /// a tick can bake one out of nothing, and a graph that appeared halfway
    /// through a session would be a graph nobody stamped.
    #[test]
    fn a_rebake_of_nothing_is_nothing() {
        let (ground, tiles) = shard(scene());
        let mut graph = baked(&ground, &tiles);
        let before = graph.shape();
        follow(&mut graph, &ground, &tiles, &[]);
        assert_eq!(graph.shape(), before, "no chunks moved, so nothing was rebaked");
    }
}
