//! The broad phase, as a tree of boxes instead of a grid of tiles —
//! `docs/render/design_occluders.md`'s D3.
//!
//! # Why a hierarchy and not the grid
//!
//! Not speed, and the plan is explicit about that. A uniform grid must list a
//! primitive in **every cell it spans** or it stops being a superset of what a
//! ray can meet, so the more a surface merges — which is D2b's whole point — the
//! worse a grid fits it. A grid likes many small primitives of one size; a
//! hierarchy likes few large ones of different sizes, which is what a merged
//! world is. The grid also cannot hold a primitive wider than its own
//! registration cell without either listing it twice (and then double-counting
//! it, `docs/render/design_occluders.md`'s backlog) or dropping the ray that crosses only the
//! overhang.
//!
//! # What this may not do
//!
//! **D4 — the broad phase may not change the answer.** What a traversal hands
//! back is a *superset* of the primitives a segment might meet, and the answer is
//! the per-primitive rules over that superset and nothing else. So every number
//! in here — [`LEAF_PRIMITIVES`], the split rule, the order a leaf's run is in —
//! is a cost knob that **cannot** move a pixel, and that is the property the
//! brute-force oracle gates rather than a claim this module makes about itself.
//!
//! # The three properties the build has to have
//!
//! - **A node's box contains every primitive under it**, in the *wire's* own
//!   `f32` and not the record's `f64`. See [`Node::space`]: a node built from
//!   coordinates a shader will never see could exclude a primitive the shader
//!   then walks straight past.
//! - **The layout is depth first, so a miss is one assignment.** Every node
//!   carries the index its own subtree ends at ([`Node::escape`]); a hit steps to
//!   `at + 1`, which is its first child, and a miss jumps to the escape. There is
//!   no stack anywhere, which is what WGSL can run at all — see
//!   `docs/render/design_occluders.md`'s S5.
//! - **The build is a pure function of the primitive list and its order.** The
//!   tick is deterministic and the two backends walk one uploaded tree; a build
//!   that depended on anything else — a hash order, a float tie broken by sort
//!   stability — would make them two trees that agree most of the time.

use crate::occlusion::{
    Solid,
    SolidId,
};

/// Which axis a node's box is measured or split on. A number here is a choice
/// of three where the reader has to remember which — see the split's own
/// tie-break, which used to be spelled `0`/`1`/`2` and is now a match arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
    X,
    Y,
    Z,
}

/// How many primitives one leaf holds at most.
///
/// A cost knob under D4 and therefore a number here rather than a question:
/// four is the usual place to stop splitting an axis-aligned tree, because below
/// it the node test costs more than testing the primitives outright. Turning it
/// may not change a pixel, and the oracle is what says so.
pub const LEAF_PRIMITIVES: usize = 4;

/// The number of primitives in one BVH leaf.
///
/// This is kept distinct from other byte-sized counts because a leaf count is
/// part of the tree's shape: it is always non-zero and never exceeds
/// [`LEAF_PRIMITIVES`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct LeafCount(u8);

impl LeafCount {
    fn new(value: usize) -> Self {
        assert!(
            value > 0 && value <= LEAF_PRIMITIVES,
            "a BVH leaf holds between one and {LEAF_PRIMITIVES} primitives"
        );
        Self(u8::try_from(value).expect("a leaf count fits in u8"))
    }

    const fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub(super) const fn as_u32(self) -> u32 {
        self.0 as u32
    }
}

impl std::fmt::Display for LeafCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Which node of a [`Bvh`], and never a place in the primitives — the two are
/// different lists and a traversal holds one of each.
///
/// [`Bvh::nodes`]'s own length is the "past the end" value every root-level
/// escape carries, which is what stops a traversal: see [`Node::escape`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NodeIdx(u32);

impl NodeIdx {
    /// The root, and where every traversal starts.
    pub const ROOT: Self = Self(0);

    /// The `n`th node of the tree, in the depth-first order the build lays down.
    pub fn new(at: u32) -> Self {
        Self(at)
    }

    /// Its zero-based position in the tree's depth-first node layout.
    ///
    /// This is the index into [`Bvh::nodes`] and the number written to the
    /// traversal's wire representation; it is never an index into the
    /// primitive permutation.
    pub fn depth_first_index(self) -> u32 {
        self.0
    }
}

/// A position in [`Bvh::order`], never an identity of a primitive or a node.
///
/// The distinction matters because `order` is a permutation: its positions are
/// rearranged while the [`SolidId`] values stored in it keep their meaning.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct OrderIndex(u32);

impl OrderIndex {
    /// The position at the start of a leaf's run.
    pub fn new(at: u32) -> Self {
        Self(at)
    }

    /// Its zero-based position in [`Bvh::order`]'s primitive permutation.
    ///
    /// This indexes the leaf's contiguous run in that permutation; it is
    /// neither a [`SolidId`] nor a position in the depth-first node layout.
    pub fn position(self) -> u32 {
        self.0
    }
}

/// The primitives one leaf holds: a run of [`Bvh::order`], never empty.
///
/// A run and not a list of indices because the build is free to permute the
/// primitives into whatever order makes leaves contiguous — that permutation
/// *is* [`Bvh::order`], and a leaf is then two numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Leaf {
    /// Where the run starts in [`Bvh::order`].
    pub first: OrderIndex,
    /// How many primitives it holds — `1..=`[`LEAF_PRIMITIVES`].
    pub count: LeafCount,
}

/// One node: the box, where a miss continues, and the primitives if it is a
/// leaf.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Node {
    /// Everything under this node is inside this box.
    ///
    /// **In the wire's own numbers**, which is why it is built out of
    /// [`Solid::wire_box`] rather than out of [`Solid::space`]: a node is tested
    /// by the same slab arithmetic a primitive is, and a node whose corners were
    /// the record's `f64` could round *inward* of a primitive's `f32` corner on
    /// the way to the shader. Inward by one bit is a primitive the traversal
    /// skips and the walk never sees.
    ///
    /// Conservative for **both** CPU walks by construction and not by luck:
    /// `walk_the_record` tests `space` and `walk_the_wire` tests
    /// `wire_box`, and the slab test converts either to `f32` before it does
    /// anything — `wire_box` is `space` through that same conversion, so the two
    /// walks test bit-identical boxes and this contains both.
    pub space:  crate::solid::Solid,
    /// Where a ray that misses this node continues: the first node **after**
    /// this whole subtree, in the depth-first order.
    ///
    /// A hit continues at `at + 1` instead, which is this node's first child —
    /// that is the whole of what "depth first" buys, and the reason there is no
    /// stack. The root's escape is [`Bvh::nodes`]'s length, which is how a
    /// traversal that misses everything ends.
    pub escape: NodeIdx,
    /// The primitives, for a leaf — and [`None`] for an inner node, which holds
    /// none at all.
    ///
    /// [`Option`] in the sense `docs/style.md` asks for: an inner node having no
    /// primitives is a fact about what it is, not a value the build has not got
    /// round to filling in.
    pub leaf:   Option<Leaf>,
}

/// A frame's primitives, arranged so a ray can skip most of them.
///
/// Built by [`Bvh::of`] over [`crate::occlusion::Occlusion`]'s own solid list
/// and read by the walks; the traversal itself lives beside them in
/// `light`, the way the DDA over the grid did, and is mirrored in `blit.wesl`.
#[derive(Clone, PartialEq, Debug)]
pub struct Bvh {
    /// Depth first, the root at zero. Empty exactly when the frame holds no
    /// primitives at all.
    nodes: Vec<Node>,
    /// Every primitive of the frame exactly once, permuted so that each leaf's
    /// own primitives are contiguous.
    ///
    /// The permutation and not a reordering of the solids themselves: a
    /// [`SolidId`] is what an instance row carries, what an aperture is indexed
    /// by and what the self-shadow rule compares, so the list those name may not
    /// be shuffled under them.
    order: Vec<SolidId>,
}

impl Bvh {
    /// A tree over no primitives at all: what [`crate::occlusion::Occlusion::EMPTY`]
    /// holds, and what a traversal ends on immediately.
    pub const EMPTY: Self = Self {
        nodes: Vec::new(),
        order: Vec::new(),
    };

    /// Build one over a frame's primitives, in their own [`SolidId`] order.
    ///
    /// **Median split on the longest axis**, recursively, and the plan pins that
    /// rather than leaving it open: it is deterministic and free of a tuning
    /// constant, where a surface-area heuristic needs both a traversal cost and a
    /// primitive cost to weigh against each other. An SAH split is allowed later
    /// as an optimisation, gated on the cost harness and forbidden by D4 from
    /// changing a pixel.
    ///
    /// The median is of the primitives' **centres** along that axis, which is
    /// what makes a leaf's own box small: splitting the node's box at its
    /// midpoint instead would put every primitive of a wall's run on one side of
    /// it whenever the run is off-centre in its parent.
    pub fn of(solids: &[Solid]) -> Self {
        let mut order: Vec<SolidId> = (0..solids.len() as u32).map(SolidId::new).collect();
        let mut nodes = Vec::new();
        if !order.is_empty() {
            let whole = 0..order.len();
            split(&mut nodes, &mut order, whole, solids);
        }
        Self { nodes, order }
    }

    /// The nodes, depth first — what a traversal indexes with a [`NodeIdx`].
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// One node, by name.
    ///
    /// # Panics
    ///
    /// On an index from another frame's tree. Every [`NodeIdx`] a traversal
    /// holds came out of [`NodeIdx::ROOT`], a `+ 1` or a [`Node::escape`] of this
    /// same tree, so a stale one is a caller mixing two frames rather than a
    /// value to defend against.
    pub fn node(&self, at: NodeIdx) -> &Node {
        &self.nodes[at.depth_first_index() as usize]
    }

    /// The permutation itself: every primitive of the frame exactly once, in the
    /// order the build put them in.
    ///
    /// What [`crate::occlusion::Occlusion::order_bytes`] uploads. A leaf is a run
    /// of this, so a backend that walks the tree needs the whole of it and not
    /// one leaf at a time — which is exactly the difference between this and
    /// [`Bvh::primitives`].
    pub fn order(&self) -> &[SolidId] {
        &self.order
    }

    /// The primitives a leaf names, in the build's own permuted order.
    pub fn primitives(&self, leaf: Leaf) -> &[SolidId] {
        let from = leaf.first.position() as usize;
        &self.order[from..from + leaf.count.as_usize()]
    }

    /// Where a traversal is finished: one past the last node, which is what
    /// every escape at the root's own level carries.
    pub fn past_the_end(&self) -> NodeIdx {
        NodeIdx::new(self.nodes.len() as u32)
    }

    /// Whether there is no tree at all — a frame with no occluder in it.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// The box holding every primitive of `run`, in the wire's own `f32`.
///
/// See [`Node::space`] for why it is the wire's and not the record's.
fn bounds(run: &[SolidId], solids: &[Solid]) -> crate::solid::Solid {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for id in run {
        let wire = solids[id.raw() as usize].wire_box();
        for (axis, (lo, hi)) in [
            (wire.min.x, wire.max.x),
            (wire.min.y, wire.max.y),
            (wire.min.z, wire.max.z),
        ]
        .into_iter()
        .enumerate()
        {
            min[axis] = min[axis].min(lo);
            max[axis] = max[axis].max(hi);
        }
    }
    crate::solid::Solid {
        min: crate::camera::WorldSpot {
            x: min[0],
            y: min[1],
            z: min[2],
        },
        max: crate::camera::WorldSpot {
            x: max[0],
            y: max[1],
            z: max[2],
        },
    }
}

/// Where a primitive sits along one axis, for the median: the middle of its own
/// box, in the wire's numbers like everything else here.
///
/// The middle rather than either corner, so that a flat primitive — a lid, whose
/// box has no `z` extent at all — sorts by where it actually is instead of by
/// which end of it the split happens to read.
fn centre(id: SolidId, axis: Axis, solids: &[Solid]) -> f64 {
    let wire = solids[id.raw() as usize].wire_box();
    let (lo, hi) = match axis {
        Axis::X => (wire.min.x, wire.max.x),
        Axis::Y => (wire.min.y, wire.max.y),
        Axis::Z => (wire.min.z, wire.max.z),
    };
    (lo + hi) * 0.5
}

/// One node and everything under it, appended to `nodes` in depth-first order.
///
/// `range` is the part of `order` this node holds, and the recursion permutes
/// exactly that part. The node's own escape is filled in on the way back out,
/// when the length of `nodes` is what "after this subtree" means.
fn split(nodes: &mut Vec<Node>, order: &mut [SolidId], range: std::ops::Range<usize>, solids: &[Solid]) {
    let at = nodes.len();
    let space = bounds(&order[range.clone()], solids);
    nodes.push(Node {
        space,
        // Filled in below. Not a sentinel anybody may read: the node is not
        // reachable from a traversal until this function returns, since a
        // traversal only ever walks a finished tree.
        escape: NodeIdx::ROOT,
        leaf: None,
    });
    match range.len() <= LEAF_PRIMITIVES {
        true => {
            nodes[at].leaf = Some(Leaf {
                first: OrderIndex::new(range.start as u32),
                count: LeafCount::new(range.len()),
            });
        }
        false => {
            // The longest axis of this node's own box, which is the axis a split
            // shortens most.
            let extent = [
                space.max.x - space.min.x,
                space.max.y - space.min.y,
                space.max.z - space.min.z,
            ];
            // Ties go to the earlier axis, which is what `>=` says, so that a
            // cube — a whole-tile body is one on two of its three axes — splits
            // the same way in every frame it appears in.
            let axis = match (
                extent[0] >= extent[1] && extent[0] >= extent[2],
                extent[1] >= extent[2],
            ) {
                (true, _) => Axis::X,
                (false, true) => Axis::Y,
                (false, false) => Axis::Z,
            };
            // **The tie-break is the primitive's own name, and it is what makes
            // this sort unstable-safe.** Two primitives with the same centre on
            // one axis are the ordinary case rather than a curiosity — a corner
            // is two panels of one tile, a flight is a tread over a tread — so
            // without a tie-break the arrangement of a whole leaf would be
            // whatever the sort happened to do with equal keys. With it the
            // comparator is a total order over distinct names, so the cheaper
            // unstable sort answers identically and the build stays a pure
            // function of the list. See
            // `coincident_primitives_are_leafed_in_their_own_name_order`, which
            // is that stated as a gate.
            order[range.clone()].sort_unstable_by(|a, b| {
                centre(*a, axis, solids)
                    .total_cmp(&centre(*b, axis, solids))
                    .then(a.cmp(b))
            });
            let middle = range.start + range.len() / 2;
            split(nodes, order, range.start..middle, solids);
            split(nodes, order, middle..range.end, solids);
        }
    }
    nodes[at].escape = NodeIdx::new(nodes.len() as u32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::occlusion::{
        Edges,
        Owner,
        Part,
    };

    /// A body filling one tile, from `z` 0 to 10 — the shape a built scene's
    /// occluders mostly are, and enough to give a build something to sort.
    fn body(x: i32, y: i32, z: i32) -> Solid {
        Solid {
            space:    crate::occlusion::Solid::box_of(x, y, z, z + 10, Edges::ANY),
            opacity:  crate::occlusion::OPAQUE,
            edges:    Edges::ANY,
            aperture: None,
            roof:     false,
            owner:    Owner::new(z as i8, openshard_protocol::wire::Graphic(1)),
            part:     Part::ONLY,
        }
    }

    /// A row of `count` of them along `x`.
    fn row(count: i32) -> Vec<Solid> {
        (0..count).map(|x| body(x, 0, 0)).collect()
    }

    #[test]
    fn a_tree_over_nothing_is_no_tree_at_all() {
        let bvh = Bvh::of(&[]);
        assert!(bvh.is_empty());
        assert_eq!(bvh.past_the_end(), NodeIdx::ROOT);
        assert_eq!(bvh, Bvh::EMPTY);
    }

    /// Up to a leaf's worth of primitives is one node, and it is the root.
    #[test]
    fn a_frame_that_fits_in_one_leaf_is_one_node() {
        let solids = row(LEAF_PRIMITIVES as i32);
        let bvh = Bvh::of(&solids);
        assert_eq!(bvh.nodes().len(), 1);
        let root = bvh.node(NodeIdx::ROOT);
        assert_eq!(
            root.leaf,
            Some(Leaf {
                first: OrderIndex::new(0),
                count: LeafCount::new(LEAF_PRIMITIVES),
            })
        );
        assert_eq!(root.escape, bvh.past_the_end());
    }

    /// **Every primitive is under exactly one leaf**, which is the property that
    /// makes the tree a superset at all: a primitive no leaf names is a
    /// primitive the walk can never meet, however correct the traversal is.
    #[test]
    fn every_primitive_is_named_by_exactly_one_leaf() {
        for count in [1, 2, 5, 9, 33, 64, 100] {
            let solids = row(count);
            let bvh = Bvh::of(&solids);
            let mut seen: Vec<SolidId> = bvh
                .nodes()
                .iter()
                .filter_map(|node| node.leaf)
                .flat_map(|leaf| bvh.primitives(leaf).to_vec())
                .collect();
            seen.sort();
            let all: Vec<SolidId> = (0..count as u32).map(SolidId::new).collect();
            assert_eq!(seen, all, "over {count} primitives");
        }
    }

    /// A leaf holds at most [`LEAF_PRIMITIVES`] and never nothing: an empty leaf
    /// would be a node a traversal pays for and gets no answer from.
    #[test]
    fn a_leaf_is_between_one_primitive_and_the_cap() {
        let solids = row(37);
        let bvh = Bvh::of(&solids);
        for leaf in bvh.nodes().iter().filter_map(|node| node.leaf) {
            assert!(
                leaf.count.as_usize() >= 1 && leaf.count.as_usize() <= LEAF_PRIMITIVES,
                "a leaf of {} primitives",
                leaf.count
            );
        }
    }

    /// **The escape indices are what a stackless traversal is**, so they are
    /// checked as a shape rather than trusted from the recursion: a node's
    /// subtree is the contiguous run between it and its own escape, an inner
    /// node's first child is the very next node, and the second child's escape
    /// is its parent's.
    ///
    /// The root's own escape is the whole tree, and that is asserted here rather
    /// than left to follow from the rest: `blit.wesl` reads it as the number of
    /// nodes this frame has — its buffer is grown and never shrunk, so what ends
    /// its traversal is the root's escape and not the buffer's length. A root
    /// escaping short of the end would leave the shader walking a subtree.
    #[test]
    fn a_nodes_escape_is_the_end_of_its_own_subtree() {
        let solids = row(29);
        let bvh = Bvh::of(&solids);
        let end = bvh.past_the_end().depth_first_index();
        assert_eq!(bvh.node(NodeIdx::ROOT).escape, bvh.past_the_end());
        for (at, node) in bvh.nodes().iter().enumerate() {
            let at = at as u32;
            assert!(
                node.escape.depth_first_index() > at && node.escape.depth_first_index() <= end,
                "node {at} escapes to {}",
                node.escape.depth_first_index()
            );
            match node.leaf {
                // A leaf holds nothing under it, so the node after it is where a
                // miss and a hit both continue.
                Some(_) => assert_eq!(node.escape.depth_first_index(), at + 1),
                None => {
                    // The first child is the next node and the second child ends
                    // where the parent does — the whole of the depth-first
                    // layout, said as an assertion.
                    let left = bvh.node(NodeIdx::new(at + 1));
                    let right = bvh.node(left.escape);
                    assert_eq!(right.escape, node.escape, "node {at}'s second child");
                }
            }
        }
    }

    /// **A node's box contains every primitive under it**, in the wire's own
    /// numbers — [`Node::space`]'s own claim, over the whole tree rather than
    /// over the root.
    #[test]
    fn a_nodes_box_holds_every_primitive_of_its_own_subtree() {
        let solids: Vec<Solid> = (0..7)
            .flat_map(|x| (0..5).map(move |y| body(x, y, x * 3)))
            .collect();
        let bvh = Bvh::of(&solids);
        for (at, node) in bvh.nodes().iter().enumerate() {
            // The subtree is the run of nodes from this one to its escape, which
            // is the fact the test above pins; every leaf in that run is under
            // this node.
            let subtree = at as u32..node.escape.depth_first_index();
            for under in subtree.map(NodeIdx::new) {
                let Some(leaf) = bvh.node(under).leaf else {
                    continue;
                };
                for id in bvh.primitives(leaf) {
                    let wire = solids[id.raw() as usize].wire_box();
                    assert!(
                        wire.min.x >= node.space.min.x
                            && wire.min.y >= node.space.min.y
                            && wire.min.z >= node.space.min.z
                            && wire.max.x <= node.space.max.x
                            && wire.max.y <= node.space.max.y
                            && wire.max.z <= node.space.max.z,
                        "node {at} does not hold {id:?}"
                    );
                }
            }
        }
    }

    /// **The build is a pure function of the list**, and the only part of that
    /// which can actually fail is the tie-break: `Bvh::of` takes nothing but the
    /// solids, so "built twice, equal twice" is true by signature and would be a
    /// test that cannot go red.
    ///
    /// What can go red is this, and the fixture had to be built twice to get
    /// there. Forty primitives standing in **two** columns, alternating in the
    /// input: twenty share one centre and twenty share another, so the sort has
    /// real work to do *and* every comparison inside a column is a tie. The
    /// answer is then the two columns in order and each column in [`SolidId`]
    /// order, which is exactly what the tie-break decides.
    ///
    /// ⚠ **The obvious fixture — forty primitives all at one place — is
    /// vacuous**, and it was written that way first. An unstable sort handed a
    /// slice of nothing but equal keys leaves it exactly as it found it, so the
    /// test passed with the tie-break deleted. A gate that cannot go red is not
    /// a gate; what makes this one fire is that the ties are *inside* a list the
    /// sort genuinely partitions.
    #[test]
    fn coincident_primitives_are_leafed_in_their_own_name_order() {
        // Lids and not bodies, so that the axis the split reads is the one the
        // ties are on: a ten-tall body's longest axis is `z`, where all forty of
        // these sit at one height and the sort has nothing to partition at all.
        // That is the fixture's second version — the first one split on `z`,
        // failed for that reason, and would have been read as the tie-break
        // working.
        let lid = |x: i32| {
            Solid {
                space: crate::occlusion::Solid::box_of(x, 0, 0, 0, Edges::NONE),
                edges: Edges::NONE,
                ..body(x, 0, 0)
            }
        };
        let solids: Vec<Solid> = (0..40).map(|at| lid(at % 2)).collect();
        let bvh = Bvh::of(&solids);
        let order: Vec<SolidId> = bvh
            .nodes()
            .iter()
            .filter_map(|node| node.leaf)
            .flat_map(|leaf| bvh.primitives(leaf).to_vec())
            .collect();
        let expected: Vec<SolidId> = (0..40)
            .filter(|at| at % 2 == 0)
            .chain((0..40).filter(|at| at % 2 == 1))
            .map(SolidId::new)
            .collect();
        assert_eq!(order, expected);
    }

    /// **A median split cannot leave a thin leaf**, and that is what bounds the
    /// tree rather than any argument about depth: a range longer than
    /// [`LEAF_PRIMITIVES`] is at least five, so halving it by *count* gives two
    /// parts of at least two each. Every leaf therefore holds two primitives or
    /// more, a frame of `n` has at most `n / 2` leaves, and a binary tree with
    /// that many leaves has fewer than `n` nodes — which is the number a
    /// traversal pays and the reason a budget can be stated at all.
    ///
    /// It is a gate and not an observation: a split that cut the node's *box* at
    /// its midpoint instead of its list at the median puts every primitive on
    /// one side whenever they are not evenly spread, and this goes red on the
    /// empty half it leaves.
    #[test]
    fn a_median_split_leaves_no_thin_leaf_and_fewer_nodes_than_primitives() {
        for count in [5_usize, 9, 37, 100] {
            let bvh = Bvh::of(&row(count as i32));
            for leaf in bvh.nodes().iter().filter_map(|node| node.leaf) {
                assert!(
                    leaf.count.as_usize() >= 2,
                    "a leaf of {} over {count} primitives",
                    leaf.count
                );
            }
            assert!(
                bvh.nodes().len() < count,
                "{} nodes over {count} primitives",
                bvh.nodes().len()
            );
        }
    }
}
