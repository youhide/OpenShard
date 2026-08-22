//! A bounded, single-level navigation graph.
//!
//! Static terrain is represented by 32×32 regions.  Obstacles inside a region
//! stay local to live refinement; only crossings between region components
//! become abstract choices.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;

use crate::{Terrain, Tile, find_path_toward_until, find_path_until, step_allowed};

const MAX_LONG_PATH_TIME: Duration = Duration::from_millis(50);

const WIDE_PORTAL: usize = 6;
/// A region stays well inside the normal 600-cell refinement budget, while the
/// whole facet has only a few thousand regions. Obstacles inside one are live
/// terrain, not graph boundaries, so a forest does not emit a node per tree.
const REGION_SIZE: u32 = 32;
const NO_COMPONENT: u16 = 0;
const NO_NEIGHBOR: u16 = u16::MAX;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) left: u16,
    pub(crate) top: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

impl Region {
    fn contains(self, point: Point) -> bool {
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

struct InRegion<'a> {
    terrain: &'a dyn Terrain,
    region: Region,
}

impl Terrain for InRegion<'_> {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        (self.region.contains(from) && self.region.contains(to))
            .then(|| self.terrain.can_step(from, to))
            .flatten()
    }
}

impl NavigationGraph {
    /// Extract a static graph from one facet. Empty and unrepresentable facets
    /// cannot be addressed by `Point` and therefore have no graph.
    #[must_use]
    pub fn build(terrain: &dyn Terrain, width: u32, height: u32) -> Option<Self> {
        let limit = u32::from(u16::MAX) + 1;
        if width == 0 || height == 0 || width >= limit || height >= limit {
            return None;
        }
        let started = Instant::now();
        eprintln!("navigation graph: sampling {width}x{height} terrain");
        let cells = width as usize * height as usize;
        let mut points = vec![None; cells];
        for y in 0..height as u16 {
            for x in 0..width as u16 {
                let tile = Tile::new(x, y);
                let near = terrain.ground_z(tile).unwrap_or(0);
                let point = Point::new(x, y, near);
                points[usize::from(y) * width as usize + usize::from(x)] = terrain.can_step(point, point);
            }
        }
        eprintln!(
            "navigation graph +{:.3}s: terrain sampled",
            started.elapsed().as_secs_f64()
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
        };
        graph.partition(&points);
        eprintln!(
            "navigation graph +{:.3}s: partitioned into {} regions",
            started.elapsed().as_secs_f64(),
            graph.regions.len()
        );
        let components = graph.component_labels(terrain, &points);
        graph.portals(terrain, &points, &components);
        eprintln!(
            "navigation graph +{:.3}s: {} portal nodes found; calculating intra-region routes",
            started.elapsed().as_secs_f64(),
            graph.nodes.len()
        );
        graph.intra_edges(terrain);
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

    fn partition(&mut self, points: &[Option<Point>]) {
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
                        let index = self.index(x, y);
                        if points[index].is_some() {
                            self.set_walkable(x, y);
                        }
                    }
                }
            }
        }
    }

    /// Mark strongly connected static components in each region.  These are
    /// bake-time scratch data: `u16` is enough because a region has at most
    /// 1,024 cells, and the labels never enter the artifact.
    fn component_labels(&self, terrain: &dyn Terrain, points: &[Option<Point>]) -> Vec<u16> {
        let mut labels = vec![NO_COMPONENT; points.len()];
        for &region in &self.regions {
            let width = usize::from(region.width);
            let cells = width * usize::from(region.height);
            let mut outgoing = vec![[NO_NEIGHBOR; Direction::ALL.len()]; cells];
            let mut outgoing_len = vec![0_u8; cells];
            let mut incoming = vec![[NO_NEIGHBOR; Direction::ALL.len()]; cells];
            let mut incoming_len = vec![0_u8; cells];
            let local_index =
                |point: Point| usize::from(point.y - region.top) * width + usize::from(point.x - region.left);

            for y in region.top..region.top + region.height {
                for x in region.left..region.left + region.width {
                    let Some(from) = points[self.index(x, y)] else {
                        continue;
                    };
                    let from_index = local_index(from);
                    for direction in Direction::ALL {
                        let Some(next) = step_allowed(terrain, from, direction) else {
                            continue;
                        };
                        if !region.contains(next) || points[self.index(next.x, next.y)].is_none() {
                            continue;
                        }
                        let next_index = local_index(next);
                        let out_at = usize::from(outgoing_len[from_index]);
                        outgoing[from_index][out_at] = next_index as u16;
                        outgoing_len[from_index] += 1;
                        let in_at = usize::from(incoming_len[next_index]);
                        incoming[next_index][in_at] = from_index as u16;
                        incoming_len[next_index] += 1;
                    }
                }
            }

            // Kosaraju's algorithm keeps directed height transitions honest.
            let mut seen = vec![false; cells];
            let mut finish = Vec::with_capacity(cells);
            for root in 0..cells {
                let point = Point::new(
                    region.left + (root % width) as u16,
                    region.top + (root / width) as u16,
                    0,
                );
                if seen[root] || points[self.index(point.x, point.y)].is_none() {
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
                if labels[self.index(
                    region.left + (root % width) as u16,
                    region.top + (root / width) as u16,
                )] != NO_COMPONENT
                {
                    continue;
                }
                component = component
                    .checked_add(1)
                    .expect("a region has at most 1,024 components");
                let mut stack = vec![root];
                while let Some(at) = stack.pop() {
                    let x = region.left + (at % width) as u16;
                    let y = region.top + (at / width) as u16;
                    let label = &mut labels[self.index(x, y)];
                    if *label != NO_COMPONENT {
                        continue;
                    }
                    *label = component;
                    for &neighbor in &incoming[at][..usize::from(incoming_len[at])] {
                        let neighbor = usize::from(neighbor);
                        let x = region.left + (neighbor % width) as u16;
                        let y = region.top + (neighbor / width) as u16;
                        if labels[self.index(x, y)] == NO_COMPONENT {
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
    fn portals(&mut self, terrain: &dyn Terrain, points: &[Option<Point>], components: &[u16]) {
        for x in
            ((REGION_SIZE - 1) as u16..(self.width as u16).saturating_sub(1)).step_by(REGION_SIZE as usize)
        {
            let mut entrances = BTreeMap::<(usize, u16, usize, u16), Vec<(Point, Point)>>::new();
            for y in 0..self.height as u16 {
                let Some(pair) = self.vertical_pair(terrain, points, x, y) else {
                    continue;
                };
                let first = self
                    .region_at(pair.0)
                    .expect("a valid crossing starts in a region");
                let second = self.region_at(pair.1).expect("a valid crossing ends in a region");
                entrances
                    .entry((
                        first.0,
                        components[self.index(pair.0.x, pair.0.y)],
                        second.0,
                        components[self.index(pair.1.x, pair.1.y)],
                    ))
                    .or_default()
                    .push(pair);
            }
            for crossings in entrances.into_values() {
                let first = self
                    .region_at(crossings[0].0)
                    .expect("a valid crossing starts in a region");
                let second = self
                    .region_at(crossings[0].1)
                    .expect("a valid crossing ends in a region");
                self.add_portal(first, second, &crossings);
            }
        }
        for y in
            ((REGION_SIZE - 1) as u16..(self.height as u16).saturating_sub(1)).step_by(REGION_SIZE as usize)
        {
            let mut entrances = BTreeMap::<(usize, u16, usize, u16), Vec<(Point, Point)>>::new();
            for x in 0..self.width as u16 {
                let Some(pair) = self.horizontal_pair(terrain, points, x, y) else {
                    continue;
                };
                let first = self
                    .region_at(pair.0)
                    .expect("a valid crossing starts in a region");
                let second = self.region_at(pair.1).expect("a valid crossing ends in a region");
                entrances
                    .entry((
                        first.0,
                        components[self.index(pair.0.x, pair.0.y)],
                        second.0,
                        components[self.index(pair.1.x, pair.1.y)],
                    ))
                    .or_default()
                    .push(pair);
            }
            for crossings in entrances.into_values() {
                let first = self
                    .region_at(crossings[0].0)
                    .expect("a valid crossing starts in a region");
                let second = self
                    .region_at(crossings[0].1)
                    .expect("a valid crossing ends in a region");
                self.add_portal(first, second, &crossings);
            }
        }
    }

    fn vertical_pair(
        &self,
        terrain: &dyn Terrain,
        points: &[Option<Point>],
        x: u16,
        y: u16,
    ) -> Option<(Point, Point)> {
        let left = points[self.index(x, y)]?;
        points[self.index(x + 1, y)]?;
        let right = step_allowed(terrain, left, Direction::East)?;
        let left = step_allowed(terrain, right, Direction::West)?;
        (left.x == x && left.y == y && right.x == x + 1 && right.y == y).then_some((left, right))
    }

    fn horizontal_pair(
        &self,
        terrain: &dyn Terrain,
        points: &[Option<Point>],
        x: u16,
        y: u16,
    ) -> Option<(Point, Point)> {
        let top = points[self.index(x, y)]?;
        points[self.index(x, y + 1)]?;
        let bottom = step_allowed(terrain, top, Direction::South)?;
        let top = step_allowed(terrain, bottom, Direction::North)?;
        (top.x == x && top.y == y && bottom.x == x && bottom.y == y + 1).then_some((top, bottom))
    }

    fn add_portal(&mut self, first: RegionId, second: RegionId, run: &[(Point, Point)]) {
        let ids: Vec<_> = match run.len() {
            0 => return,
            1..WIDE_PORTAL => vec![(run.len() - 1) / 2],
            _ => vec![0, run.len() - 1],
        };
        for index in ids {
            let first_id = self.add_node(first, run[index].0);
            let second_id = self.add_node(second, run[index].1);
            self.add_edge(first_id, second_id, 1);
            self.add_edge(second_id, first_id, 1);
        }
    }

    fn add_node(&mut self, region: RegionId, point: Point) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node { point });
        self.build_edges.push(Vec::new());
        self.build_region_nodes[region.0].push(id);
        id
    }

    fn add_edge(&mut self, from: NodeId, to: NodeId, cost: u32) {
        self.build_edges[from.0].push(Edge { to, cost });
    }

    fn intra_edges(&mut self, terrain: &dyn Terrain) {
        for region in 0..self.regions.len() {
            let nodes = self.build_region_nodes[region].clone();
            let region = self.regions[region];
            let local = InRegion { terrain, region };
            for &from in &nodes {
                let costs = region_costs(&local, region, self.nodes[from.0].point);
                for &to in &nodes {
                    if from == to {
                        continue;
                    }
                    let point = self.nodes[to.0].point;
                    let x = usize::from(point.x - region.left);
                    let y = usize::from(point.y - region.top);
                    if let Some(cost) = costs[y * usize::from(region.width) + x] {
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

    fn local_costs(
        &self,
        terrain: &dyn Terrain,
        region_id: RegionId,
        endpoint: Point,
        forbidden: &[bool],
        toward_endpoint: bool,
        deadline: Instant,
    ) -> Vec<(NodeId, u32)> {
        let region = self.regions[region_id.0];
        let local = InRegion { terrain, region };
        let budget = usize::from(region.width) * usize::from(region.height);
        self.nodes_in_region(region_id)
            .filter(|node| !forbidden[node.0])
            .filter_map(|node| {
                if Instant::now() >= deadline {
                    return None;
                }
                let (from, to) = match toward_endpoint {
                    true => (self.nodes[node.0].point, endpoint),
                    false => (endpoint, self.nodes[node.0].point),
                };
                find_path_until(&local, from, to, budget, deadline).map(|route| (node, route.len() as u32))
            })
            .collect()
    }

    fn refine(
        &self,
        terrain: &dyn Terrain,
        from: Point,
        to: Point,
        nodes: &[NodeId],
        budget: usize,
        deadline: Instant,
    ) -> Result<Vec<Direction>, NodeId> {
        let mut route = Vec::new();
        let mut at = from;
        let mut region = self
            .region_at(from)
            .expect("the query was checked before refinement");
        for &node in nodes {
            if Instant::now() >= deadline {
                return Err(node);
            }
            let next = self.nodes[node.0];
            let next_region = self.node_region(node);
            let segment = match next_region == region {
                true => region_route(terrain, self.regions[region.0], at, next.point, budget, deadline),
                false => cross_portal(terrain, at, next.point),
            };
            let Some(segment) = segment else {
                return Err(node);
            };
            let Some(next_at) = append(terrain, at, &segment, &mut route) else {
                return Err(node);
            };
            at = next_at;
            region = next_region;
        }
        let last = *nodes
            .last()
            .expect("different regions always need graph transitions");
        if Instant::now() >= deadline {
            return Err(last);
        }
        let Some(segment) = region_route(terrain, self.regions[region.0], at, to, budget, deadline) else {
            return Err(last);
        };
        append(terrain, at, &segment, &mut route).ok_or(last)?;
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

/// Exact uniform-cost routes from one point to every tile in one small region.
/// One traversal replaces the old one-A*-per-node-pair construction while
/// retaining directed movement and height resolution through `step_allowed`.
fn region_costs(terrain: &dyn Terrain, region: Region, from: Point) -> Vec<Option<u32>> {
    let width = usize::from(region.width);
    let cells = width * usize::from(region.height);
    let index = |point: Point| usize::from(point.y - region.top) * width + usize::from(point.x - region.left);
    let mut costs = vec![None; cells];
    let mut open = VecDeque::new();
    costs[index(from)] = Some(0);
    open.push_back(from);
    while let Some(point) = open.pop_front() {
        let cost = costs[index(point)].expect("queued points have a cost");
        for direction in Direction::ALL {
            let Some(next) = step_allowed(terrain, point, direction) else {
                continue;
            };
            if !region.contains(next) {
                continue;
            }
            let at = index(next);
            if costs[at].is_none() {
                costs[at] = Some(cost + 1);
                open.push_back(next);
            }
        }
    }
    costs
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
    /// Both endpoints are in one region and no route inside it joins them —
    /// the graph was never consulted.
    NoLocalRoute,
    /// A corridor existed and live refinement failed every hop it was given,
    /// until [`LIVE_REROUTES`] retries ran out.
    PortalsExhausted,
    /// [`MAX_LONG_PATH_TIME`] passed.
    Deadline,
}

/// How many corridors refinement may reject before the query gives up.
///
/// A constant, while the number of hops a corridor has grows with the distance
/// asked for — so a long route has more chances to spend a retry than a short
/// one has, on the same ground.
const LIVE_REROUTES: usize = 8;

/// Refine a route proposed by a static navigation graph through live terrain.
#[must_use]
pub fn find_long_path(
    guide: &dyn Terrain,
    terrain: &dyn Terrain,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    budget: usize,
) -> Option<Vec<Direction>> {
    let started = Instant::now();
    let (mut result, mut exit) = find_long_path_inner(guide, terrain, graph, from, to, budget);
    let elapsed = started.elapsed();
    // The inner loops observe the same deadline, but an individual live A*
    // call can finish just after it.  Do not hand an interactive caller a
    // late route; the next terrain/frame snapshot may try again.
    if elapsed >= MAX_LONG_PATH_TIME {
        result = None;
        exit = LongExit::Deadline;
    }
    debug_long_path(from, to, budget, elapsed, result.as_ref().map(Vec::len), exit);
    result
}

fn find_long_path_inner(
    guide: &dyn Terrain,
    terrain: &dyn Terrain,
    graph: &NavigationGraph,
    from: Point,
    to: Point,
    budget: usize,
) -> (Option<Vec<Direction>>, LongExit) {
    let deadline = Instant::now() + MAX_LONG_PATH_TIME;
    let (Some(from_region), Some(to_region)) = (graph.region_at(from), graph.region_at(to)) else {
        return (None, LongExit::OffGraph);
    };
    if from_region == to_region {
        let route = region_route(terrain, graph.regions[from_region.0], from, to, budget, deadline);
        let exit = match route {
            Some(_) => LongExit::Route,
            None => LongExit::NoLocalRoute,
        };
        return (route, exit);
    }
    let mut forbidden = vec![false; graph.nodes.len()];
    // Refinement can forbid several portals and retry the abstract route, but
    // the endpoint-to-portal searches do not change between those retries.
    // Compute them once instead of repeating the whole portal fan-out.
    let no_forbidden = vec![false; graph.nodes.len()];
    let source = graph.local_costs(guide, from_region, from, &no_forbidden, false, deadline);
    let target = graph.local_costs(guide, to_region, to, &no_forbidden, true, deadline);
    if Instant::now() >= deadline {
        return (None, LongExit::Deadline);
    }
    if source.is_empty() || target.is_empty() {
        return (None, LongExit::NoJoin);
    }
    for _ in 0..=LIVE_REROUTES {
        if Instant::now() >= deadline {
            return (None, LongExit::Deadline);
        }
        let Some(path) = graph.abstract_path(from, to, &forbidden, &source, &target) else {
            return (None, LongExit::NoCorridor);
        };
        match graph.refine(terrain, from, to, &path, budget, deadline) {
            Ok(route) => return (Some(route), LongExit::Route),
            Err(node) if !forbidden[node.0] => graph.forbid_portal(node, &mut forbidden),
            Err(_) => return (None, LongExit::PortalsExhausted),
        }
    }
    (None, LongExit::PortalsExhausted)
}

fn debug_long_path(
    from: Point,
    to: Point,
    budget: usize,
    elapsed: std::time::Duration,
    route_len: Option<usize>,
    exit: LongExit,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OPENSHARD_PATH_DEBUG").is_some()) {
        return;
    }
    let threshold = std::env::var("OPENSHARD_PATH_DEBUG_MS")
        .ok()
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or(10);
    if elapsed.as_millis() < threshold {
        return;
    }
    eprintln!(
        "path-debug kind=find_long_path from=({}, {}, {}) to=({}, {}, {}) budget={budget} elapsed_ms={:.3} exit={exit:?} route_steps={route_len:?}",
        from.x,
        from.y,
        from.z,
        to.x,
        to.y,
        to.z,
        elapsed.as_secs_f64() * 1_000.0,
    );
}

fn cross_portal(terrain: &dyn Terrain, from: Point, to: Point) -> Option<Vec<Direction>> {
    let direction = match (to.x.cmp(&from.x), to.y.cmp(&from.y)) {
        (std::cmp::Ordering::Greater, std::cmp::Ordering::Equal) => Direction::East,
        (std::cmp::Ordering::Less, std::cmp::Ordering::Equal) => Direction::West,
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Greater) => Direction::South,
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Less) => Direction::North,
        _ => return None,
    };
    step_allowed(terrain, from, direction)
        .filter(|landing| landing.x == to.x && landing.y == to.y)
        .map(|_| vec![direction])
}

fn region_route(
    terrain: &dyn Terrain,
    region: Region,
    from: Point,
    to: Point,
    budget: usize,
    deadline: Instant,
) -> Option<Vec<Direction>> {
    let local = InRegion { terrain, region };
    let hop = u16::try_from((budget / 2).max(1)).unwrap_or(u16::MAX);
    let mut route = Vec::new();
    let mut at = from;
    while distance(at, to) > u32::from(hop) {
        if Instant::now() >= deadline {
            return None;
        }
        // Aim at the real destination and keep the closest result when the
        // bounded search runs out. A synthetic point exactly `hop` tiles away
        // can itself be a tree, which must not make a whole forest unroutable.
        let segment = find_path_toward_until(&local, at, to, budget, deadline)?;
        at = append(terrain, at, &segment, &mut route)?;
    }
    let segment = find_path_until(&local, at, to, budget, deadline)?;
    append(terrain, at, &segment, &mut route)?;
    Some(route)
}

fn append(
    terrain: &dyn Terrain,
    from: Point,
    route: &[Direction],
    out: &mut Vec<Direction>,
) -> Option<Point> {
    let mut at = from;
    for &direction in route {
        at = step_allowed(terrain, at, direction)?;
        out.push(direction);
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::find_path;

    use proptest::prelude::*;

    use super::*;

    #[derive(Clone, Default)]
    struct Grid {
        width: u16,
        height: u16,
        blocked: BTreeSet<(u16, u16)>,
    }

    impl Grid {
        fn open(width: u16, height: u16) -> Self {
            Self {
                width,
                height,
                blocked: BTreeSet::new(),
            }
        }

        fn block(&mut self, x: u16, y: u16) {
            self.blocked.insert((x, y));
        }
    }

    impl Terrain for Grid {
        fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
            (to.x < self.width && to.y < self.height && !self.blocked.contains(&(to.x, to.y))).then_some(to)
        }
    }

    fn end(terrain: &dyn Terrain, from: Point, route: &[Direction]) -> Point {
        route.iter().fold(from, |at, &direction| {
            step_allowed(terrain, at, direction).unwrap()
        })
    }

    #[test]
    fn an_open_facet_has_only_bounded_coarse_regions() {
        let terrain = Grid::open(704, 32);
        let graph = NavigationGraph::build(&terrain, 704, 32).unwrap();
        assert_eq!(graph.regions.len(), 22);
        assert_eq!(graph.nodes.len(), 84);
        let from = Point::new(1, 1, 0);
        let to = Point::new(702, 30, 0);
        let route = find_long_path(&terrain, &terrain, &graph, from, to, 100).unwrap();
        assert_eq!(end(&terrain, from, &route), to);
    }

    #[test]
    fn a_wall_opening_becomes_a_portal_between_derived_regions() {
        let mut terrain = Grid::open(96, 64);
        for y in 0..64 {
            if y != 40 {
                terrain.block(48, y);
            }
        }
        let graph = NavigationGraph::build(&terrain, 96, 64).unwrap();
        let from = Point::new(2, 2, 0);
        let to = Point::new(93, 2, 0);
        let route = find_long_path(&terrain, &terrain, &graph, from, to, 100).unwrap();
        assert_eq!(end(&terrain, from, &route), to);
        let mut at = from;
        for direction in route {
            at = step_allowed(&terrain, at, direction).unwrap();
            assert!(at.x != 48 || at.y == 40);
        }
    }

    #[test]
    fn a_forest_does_not_put_portals_around_every_tree() {
        let mut terrain = Grid::open(128, 128);
        for y in (4..124).step_by(4) {
            for x in (4..124).step_by(4) {
                terrain.block(x, y);
            }
        }
        let graph = NavigationGraph::build(&terrain, 128, 128).unwrap();
        assert_eq!(graph.regions.len(), 16);
        assert!(
            graph.nodes.len() < 500,
            "{} nodes for 900 trees",
            graph.nodes.len()
        );
        let from = Point::new(1, 1, 0);
        let to = Point::new(126, 126, 0);
        let route = find_long_path(&terrain, &terrain, &graph, from, to, 600).unwrap();
        assert_eq!(end(&terrain, from, &route), to);
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
        let graph = NavigationGraph::build(&terrain, 64, 32).unwrap();
        // Two maximally separated representatives make four directed nodes;
        // raw contiguous runs would have made 32.
        assert_eq!(graph.nodes.len(), 4);
        let from = Point::new(2, 2, 0);
        let to = Point::new(61, 29, 0);
        let route = find_long_path(&terrain, &terrain, &graph, from, to, 600).unwrap();
        assert_eq!(end(&terrain, from, &route), to);
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
        let graph = NavigationGraph::build(&terrain, 64, 32).unwrap();
        assert_eq!(graph.nodes.len(), 8);
    }

    #[derive(Clone)]
    struct OneWayGrid(Grid);

    impl Terrain for OneWayGrid {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            self.0.can_step(from, to).filter(|_| {
                // The left region has a south-only height transition across
                // this row.  It splits strong components without making the
                // open region on the other side of the border one-way.
                !(from.x < 32 && from.y >= 16 && to.y < 16)
            })
        }
    }

    #[test]
    fn one_way_region_transitions_do_not_merge_component_pairs() {
        let terrain = OneWayGrid(Grid::open(64, 32));
        let graph = NavigationGraph::build(&terrain, 64, 32).unwrap();
        // The top and bottom crossings touch different strong components in
        // the left region, despite being connected when movement is treated
        // as undirected.
        assert_eq!(graph.nodes.len(), 8);
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
            let exact = find_path(&terrain, from, to, 20 * 14);
            let graph = NavigationGraph::build(&terrain, 20, 14).unwrap();
            let route = find_long_path(&terrain, &terrain, &graph, from, to, 20 * 14);
            prop_assert_eq!(route.is_some(), exact.is_some());
            if let Some(route) = route {
                prop_assert_eq!(end(&terrain, from, &route), to);
            }
        }
    }
}
