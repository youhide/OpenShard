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
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::time::Instant;

use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;

use crate::footing::Footing;
use openshard_map::grid::Tile;

use crate::walk::steps_out_of;
use crate::{Effort, debug_enabled, find_path_toward_within, find_path_within, step_allowed};

/// The whole work one long query may do, in node expansions.
///
/// **The ceiling that used to be a clock.** A long query is two region floods
/// and up to nine refinement passes over a corridor, and what kept the sum of
/// them off the tick was 50 ms of wall clock read inside each — see
/// [`Effort`] for why that was the wrong instrument in three separate ways.
/// This is the same ceiling in the unit the searches under it are already
/// written in, so what a query may cost stops depending on how busy the
/// machine is.
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
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) regions: Vec<Region>,
    /// One bit per tile. Region ids are a regular 32×32 grid and are computed.
    pub(crate) walkable: Vec<u8>,
    pub(crate) nodes: Vec<Node>,
    pub(crate) region_offsets: Vec<u32>,
    pub(crate) region_nodes: Vec<u32>,
    pub(crate) edge_offsets: Vec<u32>,
    pub(crate) edge_targets: Vec<u32>,
    pub(crate) edge_costs: Vec<u16>,
    // Kept only while `build` is assembling the graph, then dropped.
    pub(crate) build_region_nodes: Vec<Vec<NodeId>>,
    pub(crate) build_edges: Vec<Vec<Edge>>,
    /// One node per standing place, however many entrances name it.
    ///
    /// A portal is directed now, so the two ways across one border are two
    /// logical entrances and both want a node at the same place. Interning them
    /// is what keeps a symmetric border costing what it always did — and it is
    /// the right identity anyway: two entrances meeting at one place are one
    /// place, not two.
    pub(crate) build_nodes: BTreeMap<(u16, u16, i8), NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) left: u16,
    pub(crate) top: u16,
    pub(crate) width: u16,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegionId(pub(crate) usize);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId(pub(crate) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) point: Point,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edge {
    pub(crate) to: NodeId,
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

/// Every place a body may stand on one facet, addressed by tile.
///
/// A column is a *list* of standing surfaces rather than one height, which is
/// what the span layer is for and what this bake had never read. One tile's
/// places are a contiguous run, highest first, and the census says 99.4% of the
/// runs on Britannia hold one or none — so this is about a per-cent more
/// entries than the one-per-tile array it replaces.
struct Places {
    /// `starts[tile]..starts[tile + 1]` is that tile's run.
    starts: Vec<u32>,
    /// The standing points themselves.
    points: Vec<Point>,
}

impl Places {
    /// One tile's run, by the graph's own row-major tile index.
    fn at(&self, index: usize) -> &[Point] {
        &self.points[self.starts[index] as usize..self.starts[index + 1] as usize]
    }

    /// How many places the facet holds.
    fn len(&self) -> usize {
        self.points.len()
    }

    /// The facet-wide number of the place a landing on tile `index` names, or
    /// `None` where nothing is listed at that height.
    ///
    /// A landing carries its own height and that is the whole identity — the
    /// same choice [`PathNodeKey`](crate::path) makes, and for the same reason:
    /// a span is a fact about the map file, and a place is where feet are. The
    /// run is one element long almost everywhere, so the walk is a comparison.
    fn id(&self, index: usize, at: Point) -> Option<usize> {
        let start = self.starts[index] as usize;
        self.at(index)
            .iter()
            .position(|place| place.z == at.z)
            .map(|offset| start + offset)
    }
}

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
/// The bake takes its places out of the facet-wide sampling it is holding
/// ([`Self::of`]); a query holds no such thing and samples the one region it
/// needs ([`Self::sampled`]).
struct RegionPlaces {
    region: Region,
    /// `offsets[cell]..offsets[cell + 1]`, in the region's row-major cell order.
    offsets: Vec<u32>,
    /// Every place of the region, in that order.
    points: Vec<Point>,
}

impl RegionPlaces {
    /// The bake's, out of the facet-wide sampling it already holds.
    fn of(graph: &NavigationGraph, places: &Places, region: Region) -> Self {
        let cells = usize::from(region.width) * usize::from(region.height);
        let mut offsets = Vec::with_capacity(cells + 1);
        let mut points = Vec::with_capacity(cells);
        offsets.push(0);
        for y in region.top..region.top + region.height {
            for x in region.left..region.left + region.width {
                points.extend_from_slice(places.at(graph.index(x, y)));
                offsets.push(points.len() as u32);
            }
        }
        Self {
            region,
            offsets,
            points,
        }
    }

    /// A query's, sampled out of the ground on the spot.
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

    /// These same places by their facet-wide numbers.
    ///
    /// The bake's alone: a label the component pass writes is read by the portal
    /// pass, and both speak the facet's numbering. Walked again rather than kept
    /// in the struct, because a query's region has no facet-wide sampling behind
    /// it to number against — and this walk asks the step rule nothing.
    fn facet_ids(&self, graph: &NavigationGraph, places: &Places) -> Vec<u32> {
        let mut ids = Vec::with_capacity(self.points.len());
        for y in self.region.top..self.region.top + self.region.height {
            for x in self.region.left..self.region.left + self.region.width {
                let index = graph.index(x, y);
                let start = places.starts[index] as usize;
                ids.extend((start..start + places.at(index).len()).map(|id| id as u32));
            }
        }
        ids
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

/// Every place a body may stand, one column at a time.
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
fn sample(footing: &Footing<'_>, width: u32, height: u32) -> Places {
    let cells = width as usize * height as usize;
    let mut starts = Vec::with_capacity(cells + 1);
    let mut points = Vec::with_capacity(cells);
    // The census caps a column at twelve spans on Britannia; the buffer is
    // reused rather than sized, because a base set is a world nobody has
    // counted.
    let mut column: Vec<i8> = Vec::with_capacity(16);
    starts.push(0);
    for y in 0..height as u16 {
        for x in 0..width as u16 {
            column_places(footing, x, y, &mut column, &mut points);
            starts.push(points.len() as u32);
        }
    }
    Places { starts, points }
}

/// One column's places, appended to `out` in the order its spans are stored.
///
/// The whole of what [`sample`] does to one column, and a *query* reads it again
/// — [`RegionPlaces::sampled`] samples one region on the spot, and what it
/// produces has to be exactly what the bake produced or a node of that region
/// would have no slot in it. One function is what says so.
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
        eprintln!("navigation graph: sampling {width}x{height} terrain");
        let cells = width as usize * height as usize;
        let places = sample(footing, width, height);
        eprintln!(
            "navigation graph +{:.3}s: terrain sampled, {} places over {cells} columns",
            started.elapsed().as_secs_f64(),
            places.len(),
        );

        let mut graph = Self {
            width,
            height,
            regions: Vec::new(),
            walkable: vec![0; cells.div_ceil(8)],
            nodes: Vec::new(),
            region_offsets: Vec::new(),
            region_nodes: Vec::new(),
            edge_offsets: Vec::new(),
            edge_targets: Vec::new(),
            edge_costs: Vec::new(),
            build_region_nodes: Vec::new(),
            build_edges: Vec::new(),
            build_nodes: BTreeMap::new(),
        };
        graph.partition(&places);
        eprintln!(
            "navigation graph +{:.3}s: partitioned into {} regions",
            started.elapsed().as_secs_f64(),
            graph.regions.len()
        );
        let components = graph.component_labels(footing, &places);
        graph.portals(footing, &places, &components);
        eprintln!(
            "navigation graph +{:.3}s: {} portal nodes found; calculating intra-region routes",
            started.elapsed().as_secs_f64(),
            graph.nodes.len()
        );
        graph.intra_edges(footing, &places);
        for edges in &mut graph.build_edges {
            edges.sort_unstable_by_key(|edge| (edge.to, edge.cost));
            edges.dedup_by_key(|edge| edge.to);
        }
        eprintln!(
            "navigation graph +{:.3}s: ready ({} nodes, {} edges)",
            started.elapsed().as_secs_f64(),
            graph.nodes.len(),
            graph.build_edges.iter().map(Vec::len).sum::<usize>()
        );
        graph.compact();
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

    fn region_at(&self, point: Point) -> Option<RegionId> {
        (u32::from(point.x) < self.width
            && u32::from(point.y) < self.height
            && self.is_walkable(point.x, point.y))
        .then(|| {
            RegionId(
                usize::from(point.y) / REGION_SIZE as usize * self.regions_across()
                    + usize::from(point.x) / REGION_SIZE as usize,
            )
        })
    }

    fn regions_across(&self) -> usize {
        (self.width as usize).div_ceil(REGION_SIZE as usize)
    }

    fn is_walkable(&self, x: u16, y: u16) -> bool {
        let index = self.index(x, y);
        self.walkable[index / 8] & (1 << (index % 8)) != 0
    }

    fn set_walkable(&mut self, x: u16, y: u16) {
        let index = self.index(x, y);
        self.walkable[index / 8] |= 1 << (index % 8);
    }

    fn partition(&mut self, places: &Places) {
        for top in (0..self.height).step_by(REGION_SIZE as usize) {
            for left in (0..self.width).step_by(REGION_SIZE as usize) {
                let width = REGION_SIZE.min(self.width - left) as u16;
                let height = REGION_SIZE.min(self.height - top) as u16;
                self.regions.push(Region {
                    left: left as u16,
                    top: top as u16,
                    width,
                    height,
                });
                self.build_region_nodes.push(Vec::new());
                for y in top as u16..top as u16 + height {
                    for x in left as u16..left as u16 + width {
                        // A tile is walkable when *something* stands on it,
                        // whatever height that is. The bit is what an endpoint
                        // is joined to the graph by, and an endpoint carries a z
                        // nobody promised — see `path::goal_node`.
                        let index = self.index(x, y);
                        if !places.at(index).is_empty() {
                            self.set_walkable(x, y);
                        }
                    }
                }
            }
        }
    }

    /// Mark strongly connected static components in each region.  These are
    /// bake-time scratch data: the labels never enter the artifact.
    ///
    /// **One label per place**, so a bridge deck and the road beneath it are two
    /// components of one region rather than one component of one tile.
    ///
    /// `u16` numbers a region's components, which is a whole region's worth of
    /// places and then some: the deepest column on Britannia holds twelve spans,
    /// so a 32×32 region holds at most twelve thousand of them. The counter
    /// checks rather than wraps, because a base set is a world nobody has
    /// counted.
    fn component_labels(&self, footing: &Footing<'_>, places: &Places) -> Vec<u16> {
        let mut labels = vec![NO_COMPONENT; places.len()];
        for &region in &self.regions {
            let local = RegionPlaces::of(self, places, region);
            let ids = local.facet_ids(self, places);
            let cells = local.len();
            if cells == 0 {
                continue;
            }
            // Out-degree is bounded by the eight directions and in-degree is
            // not: two places of one neighbouring column can land on the same
            // place — which is exactly what a stair does, and what a fixed
            // eight-slot array here used to make an out-of-bounds panic.
            let mut outgoing = vec![[NO_NEIGHBOR; Direction::ALL.len()]; cells];
            let mut outgoing_len = vec![0_u8; cells];

            for from_index in 0..cells {
                // The whole expansion at once. Eight `step_allowed` calls would
                // resolve the place being stepped off eight times over and each
                // cardinal neighbour twice — the same waste `find_path` stopped
                // paying in N3, on the pass that walks every place of the facet.
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

            // The same edges the other way round, counting-sorted into one run
            // per place rather than a vector per place: Kosaraju's second pass
            // walks them and a region has a thousand of them.
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

            // Kosaraju's algorithm keeps directed height transitions honest —
            // and since N4 there are some: a ledge a body steps off and cannot
            // climb back onto is an edge one way and no edge the other.
            let mut seen = vec![false; cells];
            let mut finish = Vec::with_capacity(cells);
            for root in 0..cells {
                if seen[root] {
                    continue;
                }
                seen[root] = true;
                let mut stack = vec![(root, 0_u8)];
                while let Some((at, next)) = stack.last_mut() {
                    if usize::from(*next) < usize::from(outgoing_len[*at]) {
                        let neighbor = outgoing[*at][usize::from(*next)] as usize;
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

            let mut component = NO_COMPONENT;
            for root in finish.into_iter().rev() {
                if labels[ids[root] as usize] != NO_COMPONENT {
                    continue;
                }
                component = component
                    .checked_add(1)
                    .expect("a region has at most one component per standing place");
                let mut stack = vec![root];
                while let Some(at) = stack.pop() {
                    let label = &mut labels[ids[at] as usize];
                    if *label != NO_COMPONENT {
                        continue;
                    }
                    *label = component;
                    let run = incoming_offsets[at] as usize..incoming_offsets[at + 1] as usize;
                    for &neighbor in &incoming[run] {
                        let neighbor = neighbor as usize;
                        if labels[ids[neighbor] as usize] == NO_COMPONENT {
                            stack.push(neighbor);
                        }
                    }
                }
            }
        }
        labels
    }

    /// Adjacent raw crossings share a logical entrance when they connect the
    /// same strong components.  This lets an isolated tree on a border remain
    /// a local obstacle instead of multiplying portal nodes.
    ///
    /// **Each way across is its own entrance**, which is the second half of N4:
    /// an entrance is now keyed by where a step *starts* as well as where it
    /// ends, so a border a body can cross one way and not the other is a portal
    /// rather than nothing at all. Where both ways exist — which is nearly
    /// everywhere — the two runs cover the same stretch of border, choose the
    /// same representatives, and intern the same pair of nodes, so a symmetric
    /// border costs exactly what it always did.
    fn portals(&mut self, footing: &Footing<'_>, places: &Places, components: &[u16]) {
        for x in
            ((REGION_SIZE - 1) as u16..(self.width as u16).saturating_sub(1)).step_by(REGION_SIZE as usize)
        {
            let mut entrances = Entrances::new();
            for y in 0..self.height as u16 {
                let side = Direction::East;
                self.crossings(footing, places, components, Tile::new(x, y), side, &mut entrances);
                let side = Direction::West;
                self.crossings(
                    footing,
                    places,
                    components,
                    Tile::new(x + 1, y),
                    side,
                    &mut entrances,
                );
            }
            for crossings in entrances.into_values() {
                self.add_portal(&crossings);
            }
        }
        for y in
            ((REGION_SIZE - 1) as u16..(self.height as u16).saturating_sub(1)).step_by(REGION_SIZE as usize)
        {
            let mut entrances = Entrances::new();
            for x in 0..self.width as u16 {
                let side = Direction::South;
                self.crossings(footing, places, components, Tile::new(x, y), side, &mut entrances);
                let side = Direction::North;
                self.crossings(
                    footing,
                    places,
                    components,
                    Tile::new(x, y + 1),
                    side,
                    &mut entrances,
                );
            }
            for crossings in entrances.into_values() {
                self.add_portal(&crossings);
            }
        }
    }

    /// Every step across a region border out of one tile, filed under the
    /// entrance it belongs to.
    ///
    /// One crossing per *place* on the tile, and one direction. The pair that
    /// used to stand here asked the step both ways and kept it only where both
    /// succeeded — which deleted every asymmetric border from the graph, and the
    /// step rule is asymmetric by design.
    ///
    /// A landing with no place listed is skipped rather than invented. Over the
    /// bare map that cannot happen — [`sample`] keeps a superset of every
    /// landing — and over a footing with a live world in it, skipping is the
    /// conservative answer a bake owes: a refusal, not a promise.
    fn crossings(
        &self,
        footing: &Footing<'_>,
        places: &Places,
        components: &[u16],
        tile: Tile,
        direction: Direction,
        out: &mut Entrances,
    ) {
        let index = self.index(tile.x, tile.y);
        for offset in 0..places.at(index).len() {
            let from = places.at(index)[offset];
            let Some(to) = step_allowed(footing, from, direction) else {
                continue;
            };
            let Some(landed) = places.id(self.index(to.x, to.y), to) else {
                continue;
            };
            let first = self
                .region_at(from)
                .expect("a place's own tile is walkable and on the map");
            let second = self
                .region_at(to)
                .expect("a landing's own tile is walkable and on the map");
            out.entry((
                first.0,
                components[places.starts[index] as usize + offset],
                second.0,
                components[landed],
            ))
            .or_default()
            .push((from, to));
        }
    }

    /// One logical entrance, as one or two directed edges.
    ///
    /// The representatives are what they always were — the middle of a narrow
    /// run, both ends of a wide one — and what changed is that a crossing buys
    /// **one** edge. Its reverse, where the ground allows one, arrives as its own
    /// entrance and its own edge over the same interned nodes.
    fn add_portal(&mut self, run: &[(Point, Point)]) {
        let ids: Vec<_> = match run.len() {
            0 => return,
            1..WIDE_PORTAL => vec![(run.len() - 1) / 2],
            _ => vec![0, run.len() - 1],
        };
        for index in ids {
            let first_id = self.intern_node(run[index].0);
            let second_id = self.intern_node(run[index].1);
            self.add_edge(first_id, second_id, 1);
        }
    }

    /// The node one standing place is, minted the first time an entrance names
    /// it.
    ///
    /// Interned rather than pushed, because a place is now named by up to two
    /// entrances — the way in and the way out — and by the entrances of a
    /// perpendicular border where they meet at a corner. Two nodes at one place
    /// would be two names for one thing, and would double the intra-region
    /// routing every one of them pays for.
    fn intern_node(&mut self, point: Point) -> NodeId {
        if let Some(&id) = self.build_nodes.get(&(point.x, point.y, point.z)) {
            return id;
        }
        let region = self
            .region_at(point)
            .expect("a portal endpoint is a place on the map");
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node { point });
        self.build_edges.push(Vec::new());
        self.build_region_nodes[region.0].push(id);
        self.build_nodes.insert((point.x, point.y, point.z), id);
        id
    }

    fn add_edge(&mut self, from: NodeId, to: NodeId, cost: u32) {
        self.build_edges[from.0].push(Edge { to, cost });
    }

    fn intra_edges(&mut self, footing: &Footing<'_>, places: &Places) {
        for region in 0..self.regions.len() {
            let nodes = self.build_region_nodes[region].clone();
            // A region with one entrance has nothing to route between, and a
            // facet is mostly such regions — the traversal below is the bake's
            // whole cost, so not starting it is worth the branch.
            if nodes.len() < 2 {
                continue;
            }
            let local = RegionPlaces::of(self, places, self.regions[region]);
            // The other nodes are the whole of what each traversal is for, so
            // they are resolved once and handed to it: a flood that has costed
            // all of them has nothing left to learn about this region.
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
                // question, and this one runs once at build time with a whole
                // facet's worth of time to do it in.
                let (costs, _) = region_costs(footing, &local, self.nodes[from.0].point, &slots);
                for (index, &to) in nodes.iter().enumerate() {
                    if from == to {
                        continue;
                    }
                    if let Some(cost) = costs[slots[index]] {
                        self.add_edge(from, to, cost);
                    }
                }
            }
        }
    }

    fn compact(&mut self) {
        self.region_offsets.push(0);
        for nodes in &self.build_region_nodes {
            self.region_nodes.extend(nodes.iter().map(|id| id.0 as u32));
            self.region_offsets.push(self.region_nodes.len() as u32);
        }
        self.edge_offsets.push(0);
        for edges in &self.build_edges {
            self.edge_targets
                .extend(edges.iter().map(|edge| edge.to.0 as u32));
            self.edge_costs.extend(
                edges
                    .iter()
                    .map(|edge| u16::try_from(edge.cost).expect("a 32×32 region route fits in u16")),
            );
            self.edge_offsets.push(self.edge_targets.len() as u32);
        }
        self.build_region_nodes = Vec::new();
        self.build_edges = Vec::new();
        self.build_nodes = BTreeMap::new();
    }

    fn node_region(&self, node: NodeId) -> RegionId {
        self.region_at(self.nodes[node.0].point)
            .expect("every node is walkable and inside the map")
    }

    fn nodes_in_region(&self, region: RegionId) -> impl Iterator<Item = NodeId> + '_ {
        let start = self.region_offsets[region.0] as usize;
        let end = self.region_offsets[region.0 + 1] as usize;
        self.region_nodes[start..end]
            .iter()
            .map(|&id| NodeId(id as usize))
    }

    fn edges_from(&self, node: NodeId) -> impl Iterator<Item = Edge> + '_ {
        let start = self.edge_offsets[node.0] as usize;
        let end = self.edge_offsets[node.0 + 1] as usize;
        self.edge_targets[start..end]
            .iter()
            .zip(&self.edge_costs[start..end])
            .map(|(&to, &cost)| Edge {
                to: NodeId(to as usize),
                cost: u32::from(cost),
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
        from: Point,
        to: Point,
        nodes: &[NodeId],
        budget: usize,
        effort: &mut Effort,
    ) -> Result<Vec<Direction>, NodeId> {
        let mut route = Vec::new();
        let mut at = from;
        let mut region = self
            .region_at(from)
            .expect("the query was checked before refinement");
        for &node in nodes {
            if effort.spent_out() {
                return Err(node);
            }
            let next = self.nodes[node.0];
            let next_region = self.node_region(node);
            let segment = match next_region == region {
                true => region_route(footing, self.regions[region.0], at, next.point, budget, effort),
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
        let Some(segment) = region_route(footing, self.regions[region.0], at, to, budget, effort) else {
            return Err(last);
        };
        append(footing, at, &segment, &mut route).ok_or(last)?;
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
    left: usize,
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

/// Why a long-path query ended, for diagnostics only.
///
/// The four refusals used to be one string — `unreachable_or_live_refinement`
/// — which is four different repairs wearing one word. Telling them apart is
/// what the facet-0 oracle needed to say *why* the router refuses a route past
/// one region, rather than only that it does. See `docs/map/terrain_seam.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LongExit {
    /// A route came back.
    Route,
    /// One or both endpoints are on a tile the static graph has no region for.
    OffGraph,
    /// The endpoint's own 32×32 region has no portal it can walk to. Nothing
    /// about the rest of the facet was even consulted.
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

/// The shortest failed search worth asking the graph about, in tiles.
///
/// A coarse graph is counterproductive for a short failed search: joining an
/// endpoint to the graph is [`NavigationGraph::local_costs`] — a flood over the
/// endpoint's own region, at *both* ends — and that costs more than the local
/// answer the caller has already been refused, especially around a house with
/// several doors. It used to be one exact search per node of that region, which
/// is what made the distance worth drawing at all; a flood is cheaper and the
/// threshold is still a real one, since the region is walked either way.
///
/// A property of this router rather than of any one caller, which is why it
/// lives beside [`find_long_path`] and not beside the budgets: the client's
/// click-to-walk and the shard's chase read the same number, and a fall-back
/// the two ends drew at different distances would be two answers to "how far
/// can a body plan".
pub const COARSE_MIN_DISTANCE: u32 = 8;

/// Refine a route proposed by a static navigation graph through live terrain.
#[must_use]
pub fn find_long_path(
    guide: &Footing<'_>,
    footing: &Footing<'_>,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    budget: usize,
) -> Option<Vec<Direction>> {
    let started = debug_enabled().then(Instant::now);
    // One wallet for the whole query, and the only ceiling over it. What used to
    // be here as well — a second, later reading of the clock, which threw away a
    // route that arrived just after the deadline — has nothing to do: a counted
    // ceiling cannot be passed *between* two reads of it, so a route that comes
    // back is a route that was paid for.
    let mut effort = Effort::of(LONG_PATH_EFFORT);
    let (result, exit) = find_long_path_inner(guide, footing, graph, from, to, budget, &mut effort);
    debug_long_path(
        from,
        to,
        budget,
        started,
        effort.spent(),
        result.as_ref().map(Vec::len),
        exit,
    );
    result
}

fn find_long_path_inner(
    guide: &Footing<'_>,
    footing: &Footing<'_>,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    budget: usize,
    effort: &mut Effort,
) -> (Option<Vec<Direction>>, LongExit) {
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
        true => region_route(footing, graph.regions[from_region.0], from, to, budget, effort),
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
    let source = graph.local_costs(guide, from_region, from, Join::OutOf, effort);
    let target = graph.local_costs(guide, to_region, to, Join::Into, effort);
    if effort.spent_out() {
        return (None, LongExit::Spent);
    }
    if source.is_empty() || target.is_empty() {
        return (None, LongExit::NoJoin);
    }
    for _ in 0..=LIVE_REROUTES {
        if effort.spent_out() {
            return (None, LongExit::Spent);
        }
        let Some(path) = graph.abstract_path(from, to, &forbidden, &source, &target) else {
            return (None, LongExit::NoCorridor);
        };
        match graph.refine(footing, from, to, &path, budget, effort) {
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
    budget: usize,
    effort: &mut Effort,
) -> Option<Vec<Direction>> {
    let hop = u16::try_from((budget / 2).max(1)).unwrap_or(u16::MAX);
    let mut route = Vec::new();
    let mut at = from;
    while distance(at, to) > u32::from(hop) {
        if effort.spent_out() {
            return None;
        }
        // Aim at the real destination and keep the closest result when the
        // bounded search runs out. A synthetic point exactly `hop` tiles away
        // can itself be a tree, which must not make a whole forest unroutable.
        let segment = find_path_toward_within(footing, at, to, budget, effort, Some(region))?;
        at = append(footing, at, &segment, &mut route)?;
    }
    let segment = find_path_within(footing, at, to, budget, effort, Some(region))?;
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
    use crate::find_path;

    use proptest::prelude::*;

    use super::*;
    use crate::scene::Scene;
    use openshard_map::overlay::{Cover, Doors, Overlay};

    /// A bounded open grid with some tiles blocked.
    ///
    /// A real map for the ground and the heights, and an overlay for what is in
    /// the way — which is what the shard and the client both are. It used to be
    /// a `Terrain` implementation of its own, and that is the whole of what
    /// node E took away: a fixture that reimplements the step rule agrees with
    /// itself and proves nothing.
    struct Grid {
        scene: Scene,
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
        let route = find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 100).unwrap();
        assert_eq!(end(&terrain.footing(), from, &route), to);
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
        let route = find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 100).unwrap();
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
            find_path(&footing, from, to, 64 * 64).is_some(),
            "the way round is walkable"
        );

        let route = find_long_path(&footing, &footing, &graph, from, to, 600).expect("a corridor answers");
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
        let route = find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 600).unwrap();
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
        let route = find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 600).unwrap();
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

        let route = find_long_path(&footing, &footing, &graph, high, low, 600)
            .expect("the drop off the walkway is a route");
        assert_eq!(end(&footing, high, &route), low);
        assert!(
            find_long_path(&footing, &footing, &graph, low, high, 600).is_none(),
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
        let down = find_long_path(&footing, &footing, &graph, high, low, 600).expect("the drop is a route");
        assert_eq!(end(&footing, high, &down), low);
        assert!(
            find_long_path(&footing, &footing, &graph, low, high, 600).is_none(),
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
        let route = find_long_path(&footing, &footing, &graph, plain, plateau, 600)
            .expect("the stair is a route onto the plateau");
        assert_eq!(end(&footing, plain, &route), plateau);
        let down =
            find_long_path(&footing, &footing, &graph, plateau, plain, 600).expect("and a route back down");
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
            let exact = find_path(&terrain.footing(), from, to, 20 * 14);
            let graph = NavigationGraph::build(&terrain.footing(), 20, 14).unwrap();
            let route = find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 20 * 14);
            prop_assert_eq!(route.is_some(), exact.is_some());
            if let Some(route) = route {
                prop_assert_eq!(end(&terrain.footing(), from, &route), to);
            }
        }
    }
}
