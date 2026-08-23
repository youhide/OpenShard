//! What the shard has put on the ground, as something a step can be refused by.
//!
//! # Why the map is not enough
//!
//! `MapTerrain` reads the client's own files — land and static art — and nothing
//! else. A barrel is not in those files: it is an *entity* the shard placed and
//! described to us in a `0x1A`, so every terrain check this end makes looks
//! straight through it. The server does not: `openshard-state`'s `Obstructions`
//! indexes exactly these placements and `LiveTerrain` lays them over the same
//! map, which is what actually decides the step.
//!
//! Two ends deciding the same step by different rules is the whole defect. The
//! client walks its held direction into a crate, `Steering::detour` sees an open
//! tile and offers no way round, the `0x02` goes out, the shard refuses it with a
//! `0x21`, and the body shudders against the barrel for as long as the key is
//! held — the same shape of bug the corner rule had (see `steer.rs`), one layer
//! down. Walls worked only because a wall is static art and therefore in the
//! files.
//!
//! So this is the client's half of the shard's own index, built from the one
//! thing this end has: the ground items in the [`WorldView`](openshard_client_net::view::WorldView),
//! and the same `tiledata.mul` the server read to decide they block. Same
//! predicate (the tiledata blocking flag), same z-span (base z, tiledata height), so
//! the two ends agree by construction rather than by resemblance — see
//! `world::tick::decor::place_decoration`, which is where the server's copy of
//! this decision is made.
//!
//! It is a *projection* of the view, like [`App::items`](crate::App::items) and
//! [`App::others`](crate::App::others): rebuilt whole whenever the view changes,
//! with nothing to keep in step by hand. Where it is written is the live layer
//! of the facet's [`World`](openshard_map::world::World), beside the ground it
//! is laid over — see [`fill`].
//!
//! # A door is not a crate, and the graphic does not say which
//!
//! Both stop a step; only one of them is something a player can open. So a
//! cover records *which* it is (`CoverKind::Blocks`'s `door`), and the tiles the shut ones
//! stand on are the list this module exists to keep: **potentially passable, and
//! currently closed**. A `Footing` over it can then be read either way — as the
//! world stands, or as it would be with every door open — which is what lets a
//! route be planned *through* a shut door to find out where the way would go, and
//! the walk stop in front of it. `steer::plan` is the reader; the server has the same pair
//! under the same name (`state::obstruct`'s `Obstacle::door`), and since node E
//! of `docs/map/terrain_seam.md` it is not merely the same name but the same
//! type: both ends build an `openshard_map::overlay::Overlay`.
//!
//! **The graphic is what says a door is open, and its own art disagrees.** This
//! module used to argue that no state had to be tracked at all — "a door's own
//! graphic changes when it swings, so the shut leaf blocks and the open one does
//! not". Measured against the real `tiledata.mul`, that is false: all 164 shut
//! leaves in `client/render`'s door table are impassable, and so are **132 of the
//! open ones**. A client trusting the flags alone therefore refuses to walk
//! through open doors — steps the shard allows, which is the mirror-image of the
//! bug this module was written for. What does say is the table itself, ported
//! from the reference emulator's own arithmetic
//! ([`openshard_client_render::doors`]): the shut leaves sit on a family's even
//! offsets and the open ones on the odd, and an open leaf is left out of the index
//! entirely.
//!
//! # A body is not clutter, and this is not where one goes
//!
//! A mobile used to be indexed here too — laid into the overlay as furniture
//! with a body's height, under a comment admitting it was "not a category the
//! shared type names". It is a category now: `openshard_movement::Bodies`, the
//! fourth field of a [`Footing`](openshard_movement::Footing), asked *last* and
//! at the height the body would arrive at. [`crowd`] is what builds one at this
//! end, and it is in this module because it answers the same question `fill`
//! does — what refuses a step here, before one is sent — off the same view.
//! [`project`] writes both halves in one call, which is where the argument for
//! that pairing lives.
//!
//! Two things were wrong with a body in the overlay, and neither is a matter of
//! taste:
//!
//! - **The height.** A cover blocks over `[z, z + height)` and a body was given
//!   `PLAYER_HEIGHT`, which is sixteen. The shard measures a body against
//!   another body with fifteen (`Bodies`' `MOBILE_OVERLAP` — ServUO's, and one
//!   short of what it measures against a ceiling on purpose). One unit, at
//!   exactly the boundary: this end refused a step the shard allows.
//! - **The exemptions.** Staff walk through bodies and so do the dead, and a
//!   cover cannot say so — the overlay has no idea who is asking. The rule
//!   belongs where the mover is known, which is [`crowd`].

use std::collections::HashMap;

use openshard_client_net::view::WorldView;
use openshard_client_render::doors;
use openshard_client_render::items::GroundItem;
use openshard_map::grid::Tile;
use openshard_map::overlay::{Cover, Overlay};
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

/// Both halves of what refuses a step here, written from one view.
///
/// The furniture goes into `live` and the bodies into `bodies`, and it is one
/// call because it is one question asked of one snapshot.
///
/// **Two halves used to be built on two clocks.** [`fill`] was called here, once
/// per view, and [`crowd`] was built afresh at each of the four places a step is
/// decided — and only the item half had a reason for a clock of its own. The
/// overlay is written *into* the [`Ground`](openshard_movement::ground::Ground)
/// that a [`Footing`](openshard_movement::Footing) borrows, so it cannot be
/// handed back at the question; and a route over hundreds of tiles asks it once
/// per tile, which would be a rescan of everything on screen per tile. The body
/// half has neither property, and what it had instead was `Steering::steer` on
/// the input path: a `Vec` and a sort per raw mouse-move for as long as the
/// right button is held.
///
/// **One call and not two**, for [`Ground::set_base`](openshard_movement::ground::Ground::set_base)'s
/// reason: a view change that refills the furniture and forgets the bodies is a
/// client planning through a crowd, and the way to make that unspellable is to
/// leave nowhere to write one half without the other.
///
/// `view` and `items` are the same snapshot reached two ways. The bodies come
/// straight off the view; the furniture comes off the *projected* item list,
/// which is the view's items with a house expanded into its pieces and a
/// dragged item shown at the tile it is being dropped on — see `App::entered`.
pub fn project(
    live: &mut Overlay,
    bodies: &mut Vec<Point>,
    view: Option<&WorldView>,
    items: &[GroundItem],
    tiles: &TileData,
) {
    fill(live, items, tiles);
    *bodies = crowd(view);
}

/// Index whatever in `items` is in the way, and say of each placed item whether
/// it is a door.
///
/// Items only. A mobile is not clutter and does not go in here — see the module
/// header's *a body is not clutter*, and [`crowd`], which reads the same view.
///
/// Two questions, two sources, and the module header is where the argument for
/// the second one lives:
///
/// - **Does it block?** The tiledata flags, which is the same predicate the
///   server decides with, so the two ends agree by construction rather than by
///   resemblance.
/// - **Is it a door, and has it swung?** `client/render`'s door table, which is
///   the reference emulator's own arithmetic over the art indices. The flags
///   cannot answer either half: an open leaf is impassable in tiledata just as
///   often as a shut one.
///
/// An open leaf is therefore left out altogether — nothing is in the way of a
/// body walking through an open door — and a shut one goes in marked, which is
/// what [`Doors::AllOpen`] reads.
///
/// **Written into `live` rather than returned.** The live layer is one half of
/// the facet's [`World`](openshard_map::world::World) and the map is the other;
/// handing back a value the caller then has to remember to put somewhere is
/// exactly the arrangement era R exists to end. It is still built whole and
/// thrown away whole — this end has no identities to address a finer edit to,
/// which is the same fact that keeps [`Cover`] free of an owner (see
/// `openshard_map::overlay`), so `live` is cleared first and every tile in the
/// view is written afresh.
pub fn fill(live: &mut Overlay, items: &[GroundItem], tiles: &TileData) {
    let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
    for item in items {
        // What is drawn is what is in the way — see `GroundItem::displayed`.
        if doors::is_open(item.displayed()) {
            continue;
        }
        // `Cover::of_static` and not this end's own reading of the flags: the
        // shard lays the same covers from the same table through the same
        // function, which is what "agree by construction" has to mean to be
        // worth saying. Up to two of them, because a platform — a house's
        // floor, its stairs, a placed table — is both a body in the way and
        // somewhere to stand.
        let laid = Cover::of_static(tiles.static_tile(item.displayed().0)).based_at(item.at.z);
        let laid = match doors::is_door(item.displayed()) {
            true => laid.as_door(),
            false => laid,
        };
        let at = Tile::new(item.at.x, item.at.y);
        covers.entry(at).or_default().extend(laid);
    }
    live.clear();
    for (tile, covers) in covers {
        live.set(tile, covers);
    }
}

/// The other bodies this client's own step has to get past, as
/// [`Bodies`](openshard_movement::Bodies) wants them: feet, sorted by tile.
///
/// **The shard's `WorldState::crowd_near` from the other end of the wire**, and the reason
/// it has to exist at all is that both ends decide the same step. The shard
/// refuses a step onto an occupied tile; a client that plans through one asks
/// for a step it has already been told will be refused, gets a `0x21` back,
/// and rubber-bands a hold at a time for as long as somebody is standing there.
/// The route the HUD draws is the same plan, so the green line was drawn through
/// a body too.
///
/// # The rule, and the halves of it that cannot be seen from here
///
/// The shard's is two questions (`WorldState::walks_through_bodies` and
/// `WorldState::body_blocks`), and this is both of them:
///
/// - **Does this body walk through others?** The wire says so —
///   [`Player::walks_through_bodies`](openshard_client_net::view::Player::walks_through_bodies),
///   which is `IGNORE_MOBILES` off the flag byte, which the shard sets from
///   exactly that rule: staff, and the dead. Not a second reading of it here
///   either: the bit *is* the answer, and the whole point of the shard sending
///   it is that this end does not have to guess. An empty crowd is what a mover
///   exempt from the rule gets, the same `Vec::new()` the shard's own answer is
///   for one.
/// - **Is this body in the way?** Everything the shard shows us, with no
///   predicate at all — and the two clauses left off are proofs rather than
///   omissions.
///
/// **A ghost is not filtered out, because one cannot get here.** The shard lets
/// the dead through (`body_blocks` is false for a ghost), so this end would seem
/// to owe the same answer — but a client with a ghost in its view is a client
/// that is itself dead or staff: a ghost is drawn only to another ghost and to
/// staff (`WorldState::can_see_mobile`, and `show` is the one draw path), and
/// both of those are exempt by the rule above, so the answer for such a client
/// is the empty crowd two lines up. A filter here could therefore never fire for
/// a ghost. It *did* fire for the only body that can reach here wearing a
/// ghost's graphic: a living mobile a shard gave one to — a spectral NPC — which
/// the shard blocks on and which this end walked straight through. `is_ghost`
/// stays where a body id is genuinely the fact, which is the drawing (its
/// translucency, and the sword it is not holding); a step is decided against
/// what the shard says, and it says nothing about a stranger's death.
///
/// **A hidden game master is in nobody's way, and cannot get here either.** A
/// hidden mobile is drawn only to a staff viewer (`can_see_mobile` again), and a
/// staff viewer's own crowd is empty by the first rule, so the case cannot
/// arise at this end.
///
/// One sliver of the shard's rule is deliberately not reproduced: it also lets a
/// creature worn down to no hit points out of the way, because such a body stands
/// in a doorway for the one tick before it is reaped. This end cannot answer that
/// — a stranger's `hits` arrive as a percentage and not a count — and a single
/// tick of a route planned round a creature that is about to be a corpse costs a
/// re-plan and never a wrong step.
///
/// # Where the reach went
///
/// `crowd_near` takes one, because the shard's sector grid holds a whole facet
/// and a step wants eight tiles of it. Here the *view* is the reach: this client
/// has been shown the mobiles near it and nothing else, so the answer is
/// everything it holds. A body further off is invisible to a plan, which costs a
/// re-plan when the route reaches it and never a wrong step — the same bargain,
/// with the bound arriving from the shard rather than being chosen.
///
/// The list is small for the same reason: a screenful, built whole and thrown
/// away whole every time the view moves. There is no removal anybody can forget
/// — the whole of the argument against a body in an *index*, made here as well —
/// and it is written where the furniture is, by the one call that writes both
/// (see [`project`]).
///
/// Positions are the shard's last word and not the glide `crowd.rs` draws them
/// along, because the shard's last word is what the shard will decide the step
/// against. The player's own body is not among them: [`WorldView::mobiles`] is
/// never keyed by our own serial, which is "a mobile may always step off the
/// tile it is standing on" needing no code at all here.
///
/// `None` is the offline map viewer — no shard, no view, and nobody to be in the
/// way.
///
/// [`crowd_near`]: openshard_state::WorldState::crowd_near
pub fn crowd(view: Option<&WorldView>) -> Vec<Point> {
    let Some(view) = view else {
        return Vec::new();
    };
    if view.player.walks_through_bodies {
        return Vec::new();
    }
    let mut crowd: Vec<Point> = view.mobiles.values().map(|mobile| mobile.position).collect();
    // `Bodies::standing` looks a tile up by binary search and debug-asserts this
    // order rather than imposing it, for the reason its own doc gives: the
    // caller has just built the storage and knows whether it is sorted.
    crowd.sort_unstable_by_key(|body| (body.x, body.y));
    crowd
}

/// [`fill`] into a fresh overlay, for the tests that are about what went in
/// rather than about where it was written.
#[cfg(test)]
fn of(items: &[GroundItem], tiles: &TileData) -> Overlay {
    let mut live = Overlay::default();
    fill(&mut live, items, tiles);
    live
}

/// Index blockers by hand, with no tiledata to read flags from.
///
/// For the tests that are about the *span* and the detour rather than about
/// which art is impassable — the flag half is [`fill`]'s and is covered against
/// the real `tiledata.mul` below.
///
/// Nothing placed this way is a door: which art is a door is the other half
/// [`fill`] answers, and a fixture that could claim it would be claiming the
/// very thing under test.
#[cfg(test)]
fn placed(blockers: &[(openshard_protocol::world::Point, u8)]) -> Overlay {
    let mut covers: HashMap<Tile, Vec<Cover>> = HashMap::new();
    for (at, height) in blockers {
        covers
            .entry(Tile::new(at.x, at.y))
            .or_default()
            .push(Cover::blocking(at.z, *height));
    }
    let mut overlay = Overlay::default();
    for (tile, covers) in covers {
        overlay.set(tile, covers);
    }
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_map::overlay::{Body, Doors};
    use openshard_movement::{Around, Detour, Footing, Heading, Lean, Leeway, Step, step_allowed};
    use openshard_protocol::direction::{Direction, Facing};
    use openshard_protocol::items::ItemAmount;
    use openshard_protocol::mobile::StatusFlags;
    use openshard_protocol::serial::Serial;
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_protocol::world::Point;

    /// A body with its feet at `z`, as the shard asks about one — a whole
    /// [`PLAYER_HEIGHT`](openshard_movement::PLAYER_HEIGHT) of it, because these
    /// tests are about what a *person* is refused by. Not the fifteen a body is
    /// measured against another body with: this is a person against a *crate*,
    /// which is the taller of the two questions. See `Bodies`' `MOBILE_OVERLAP`.
    fn standing_at(z: i32) -> Body {
        Body::new(z, openshard_movement::PLAYER_HEIGHT)
    }

    /// A barrel's tiledata height, so the span in these tests is a real one.
    const BARREL_HEIGHT: u8 = 12;

    /// Somewhere to stand, on flat open ground.
    const HERE: Point = Point::new(100, 100, 0);

    /// A held direction walked into a barrel is refused *here*, before it is
    /// ever sent — which is the whole point. Over `OpenWorld` nothing else can
    /// refuse a step, so this is the clutter and nothing but.
    #[test]
    fn a_step_into_a_barrel_is_refused_by_this_end() {
        let east = Point::new(HERE.x + 1, HERE.y, 0);
        let clutter = placed(&[(east, BARREL_HEIGHT)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
        assert!(
            step_allowed(&terrain, HERE, Direction::East).is_none(),
            "a barrel due east did not stop the step"
        );
        assert!(
            step_allowed(&terrain, HERE, Direction::West).is_some(),
            "a barrel due east stopped a step the other way"
        );
    }

    /// The living body's graphic, and the ghost the same player becomes.
    const LIVING: Graphic = Graphic(0x0190);
    const GHOST: Graphic = Graphic(0x0192);

    /// A world this client has entered and nobody else is in yet.
    fn viewed() -> WorldView {
        WorldView::entered(openshard_protocol::world::PlayerStart {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: LIVING,
            position: HERE,
            facing: Facing::walking(Direction::South),
            map: openshard_protocol::world::MapSize::BRITANNIA,
        })
    }

    /// Somebody else, as a `0x78` would have left them in the view.
    fn seen(view: &mut WorldView, serial: u32, at: Point, body: Graphic) {
        view.mobiles.insert(
            Serial::new(serial).unwrap(),
            openshard_client_net::view::Mobile {
                body,
                position: at,
                facing: Facing::walking(Direction::South),
                hue: Hue::NONE,
                flags: StatusFlags::NONE,
                notoriety: openshard_protocol::mobile::Notoriety::Innocent,
                hits: None,
                equipment: Vec::new(),
            },
        );
    }

    /// The ground with a crowd on it and nothing else — no map, no clutter — so
    /// the only thing that can refuse a step is a body.
    fn among(crowd: &[Point]) -> Footing<'_> {
        Footing::new(None, &NOTHING_PLACED, Doors::AsTheyStand)
            .among(openshard_movement::Bodies::standing(crowd))
    }

    static NOTHING_PLACED: std::sync::LazyLock<Overlay> = std::sync::LazyLock::new(Overlay::default);

    /// The rubber-band, from the end that causes it: a step into a body is
    /// refused *here*, so it is never sent and never answered with a `0x21`.
    #[test]
    fn a_step_into_an_npc_body_is_refused_by_this_end() {
        let mut view = viewed();
        seen(&mut view, 2, Point::new(HERE.x + 1, HERE.y, 0), LIVING);
        let crowd = crowd(Some(&view));
        assert!(
            step_allowed(&among(&crowd), HERE, Direction::East).is_none(),
            "an NPC due east did not stop the step"
        );
        assert!(
            step_allowed(&among(&crowd), HERE, Direction::North).is_some(),
            "an NPC due east stopped a step the other way"
        );
    }

    /// A body standing a storey up shares an `(x, y)` and is in nobody's way —
    /// the span the crowd carries, and the reason a mezzanine is walkable with
    /// somebody standing under it.
    #[test]
    fn a_body_a_storey_up_is_not_in_the_way() {
        let mut view = viewed();
        seen(&mut view, 2, Point::new(HERE.x + 1, HERE.y, 20), LIVING);
        let crowd = crowd(Some(&view));
        assert_eq!(crowd.len(), 1, "the body overhead is in the crowd");
        assert!(
            step_allowed(&among(&crowd), HERE, Direction::East).is_some(),
            "a body on the floor above sealed the ground under it"
        );
    }

    /// **A body wearing a ghost's graphic is still in the way**, because a body
    /// this client can see is a body the shard let it see — and the shard shows
    /// a ghost to nobody but another ghost and staff, both of which are exempt
    /// and hold no crowd at all (the test below). So the only thing that can
    /// reach here in a ghost's skin is a living one wearing it, and the shard
    /// blocks on that: a spectral NPC used to be walked into a hold at a time.
    ///
    /// This is the whole of the proof `crowd`'s doc states — the filter it
    /// replaces could never fire for a real ghost, and fired only for this.
    #[test]
    fn a_living_body_in_a_ghost_s_skin_is_in_the_way() {
        let mut view = viewed();
        seen(&mut view, 2, Point::new(HERE.x + 1, HERE.y, 0), GHOST);
        let crowd = crowd(Some(&view));
        assert_eq!(crowd.len(), 1, "a body the shard drew for us was left out");
        assert!(
            step_allowed(&among(&crowd), HERE, Direction::East).is_none(),
            "a spectral NPC the shard blocks on was walked into"
        );
    }

    /// **And the exemption arrives from the shard, not from a guess here.**
    /// `IGNORE_MOBILES` on our own body is the shard saying this mover walks
    /// through bodies — staff, or dead — so there is no crowd to get past. A
    /// client that ignored the bit would refuse steps the shard allows, which is
    /// the rubber-band with the ends swapped.
    ///
    /// It is also the other half of the test above: the viewer who *can* see a
    /// ghost is a ghost or is staff, and this is what such a viewer's crowd is.
    #[test]
    fn a_mover_the_shard_exempted_has_no_crowd_at_all() {
        let mut view = viewed();
        seen(&mut view, 2, Point::new(HERE.x + 1, HERE.y, 0), LIVING);
        assert_eq!(
            crowd(Some(&view)).len(),
            1,
            "the body is in the way to begin with"
        );
        view.player.walks_through_bodies = true;
        assert!(
            crowd(Some(&view)).is_empty(),
            "a mover the shard exempted was still held to the rule"
        );
    }

    /// `Bodies::standing` looks a tile up by binary search and debug-asserts the
    /// order rather than imposing it, so the sort is this end's contract to keep.
    /// A `FxHashMap`'s iteration order is not one.
    #[test]
    fn the_crowd_comes_back_sorted_by_tile() {
        let mut view = viewed();
        for (index, x) in [7_u16, 3, 9, 1, 5].into_iter().enumerate() {
            seen(&mut view, index as u32 + 2, Point::new(x, 100, 0), LIVING);
        }
        let crowd = crowd(Some(&view));
        assert!(
            crowd
                .windows(2)
                .all(|pair| (pair[0].x, pair[0].y) <= (pair[1].x, pair[1].y)),
            "the crowd came back in {crowd:?}, which Bodies::standing cannot search"
        );
    }

    /// The offline map viewer: no shard, no view, and nobody to be in the way.
    #[test]
    fn a_client_with_no_shard_has_no_crowd() {
        assert!(crowd(None).is_empty());
    }

    /// **One view, both halves, replaced together** — which is the whole reason
    /// [`project`] is one call. What it is written against is the half-refresh:
    /// a view that moved the furniture and left the bodies where the last one
    /// had them (or the other way round) is this client planning against two
    /// different moments, and the symptom is a step refused by a shard that can
    /// see nobody there.
    ///
    /// Both stale halves are put in first, so a call that quietly wrote only one
    /// of them fails here rather than in a walk somebody is watching.
    #[test]
    fn one_call_replaces_the_furniture_and_the_crowd_together() {
        let tiles = TileData::empty();
        let stale = Tile::new(1, 1);
        let mut live = Overlay::default();
        live.set(stale, vec![Cover::blocking(0, BARREL_HEIGHT)]);
        let mut bodies = vec![Point::new(1, 1, 0)];

        let mut view = viewed();
        let standing = Point::new(HERE.x + 1, HERE.y, 0);
        seen(&mut view, 2, standing, LIVING);
        project(&mut live, &mut bodies, Some(&view), &[], &tiles);

        assert!(
            live.at(stale).is_empty(),
            "the last view's furniture outlived the view that placed it"
        );
        assert_eq!(
            bodies,
            vec![standing],
            "the last view's crowd outlived the view that stood in it"
        );
    }

    /// The bug as it was reported: a body walking at a barrel diagonally went
    /// straight at it every hold and was rolled back by the shard every hold,
    /// because this end could see no obstacle to go around. It goes around.
    #[test]
    fn a_diagonal_held_at_a_barrel_rounds_it() {
        let north_east = Point::new(HERE.x + 1, HERE.y - 1, 0);
        let clutter = placed(&[(north_east, BARREL_HEIGHT)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
        let intent = Heading {
            direction: Direction::NorthEast,
            lean: Lean::Centred,
        };
        assert!(
            step_allowed(&terrain, HERE, Direction::NorthEast).is_none(),
            "the barrel on the diagonal was not seen at all"
        );
        let around = Around::read(&terrain, HERE, intent);

        // An eighth of a turn — the default, and the smallest one that rounds
        // anything: the flanks of a blocked diagonal are North and East, both
        // open, so the body keeps moving along the barrel instead of standing.
        let step = Detour::default().step(&around, Leeway::Eighth);
        assert!(
            matches!(step, Step::Aside(Direction::North | Direction::East)),
            "a barrel on the diagonal gave {step:?} instead of a way round"
        );
    }

    /// The same barrel one storey up is not in the way — the reason a blocker
    /// carries a z-span at all. Without it, a crate on a balcony would seal the
    /// street under it, which is the mirror-image bug (the server's own, once).
    #[test]
    fn a_barrel_overhead_stops_nothing() {
        let east = Point::new(HERE.x + 1, HERE.y, 40);
        let clutter = placed(&[(east, BARREL_HEIGHT)]);
        let terrain = Footing::new(None, &clutter, Doors::AsTheyStand);
        assert!(
            step_allowed(&terrain, HERE, Direction::East).is_some(),
            "a barrel two storeys up blocked the ground"
        );
    }

    /// A barrel: impassable in tiledata, and nothing to do with the map's own
    /// statics. Anything blocking would do — this is the one the report named.
    const BARREL: Graphic = Graphic(0x0FAE);

    fn item(x: u16, y: u16, z: i8, graphic: Graphic) -> GroundItem {
        GroundItem {
            amount: ItemAmount::ONE,
            at: Point::new(x, y, z),
            graphic,
            hue: Hue::NONE,
        }
    }

    /// The tiledata the real client ships, or nothing when no install is
    /// configured — the tests that need one skip, as everywhere else in this
    /// workspace. No client files live in this repository.
    fn client_tiledata() -> Option<TileData> {
        let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        let file = dir.join("tiledata.mul");
        file.exists()
            .then(|| openshard_uofiles::tiledata::load_tiles(file).expect("tiledata should load"))
    }

    #[test]
    fn a_barrel_is_impassable_in_the_client_s_own_tiledata() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        // The premise the whole module rests on: the server decided this barrel
        // blocks by reading this flag, and so does the index below. Pinned here
        // so a tiledata reader that started answering differently is a failing
        // test rather than a body walking through crates again.
        assert!(
            tiles.static_tile(BARREL.0).flags.is_blocking(),
            "a barrel's tiledata does not call it impassable"
        );
    }

    /// The one that reads as a bug and is not.
    ///
    /// `water barrel` looks exactly like a barrel on screen and is walked
    /// straight through — on Britain's docks, where two of them stand a tile
    /// apart and only one stops anybody. Its tiledata carries a single flag,
    /// `0x4000`, which is `ArticleA`: it says the item's name takes "a", and
    /// nothing else. No `Impassable`, no `Surface`, height zero. Read straight
    /// out of the file with the reader out of the loop, and ServUO decides with
    /// the same predicate (`ImpassableSurface = Impassable | Surface`,
    /// `Scripts/Services/Pathing/Movement.cs`), so the reference walks through
    /// it too.
    ///
    /// Pinned so the next person to notice it finds this test instead of
    /// re-deriving it, and so that a shard which *wants* it solid knows it is
    /// changing gameplay rather than fixing a defect.
    #[test]
    fn a_water_barrel_is_walked_through_because_the_client_says_so() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let water_barrel = Graphic(0x154D);
        let data = tiles.static_tile(water_barrel.0);
        assert_eq!(
            data.name.as_str(),
            "water barrel",
            "0x154D is not the tile this is about"
        );
        assert_eq!(
            data.flags.bits(),
            0x4000,
            "a water barrel's flags are no longer article-only"
        );
        assert!(!data.flags.is_blocking());
        let clutter = of(&[item(100, 100, 0, water_barrel)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AsTheyStand)
                .is_none(),
            "a water barrel was made solid here and the shard would still allow the step"
        );
    }

    #[test]
    fn a_placed_barrel_blocks_its_tile_at_ground_level() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 0, BARREL)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AsTheyStand)
                .is_some(),
            "a barrel underfoot is not in the way"
        );
        assert!(
            clutter
                .blocker_at(Tile::new(101, 100), standing_at(0), Doors::AsTheyStand)
                .is_none(),
            "a barrel blocked a tile it is not on"
        );
    }

    #[test]
    fn a_barrel_two_storeys_up_leaves_the_ground_floor_open() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 40, BARREL)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AsTheyStand)
                .is_none(),
            "a crate on an upper floor sealed the floor beneath it"
        );
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(40), Doors::AsTheyStand)
                .is_some(),
            "a crate did not block the floor it stands on"
        );
    }

    #[test]
    fn a_gold_coin_is_not_an_obstacle() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        // Gold on the floor is the case the impassable flag exists to tell
        // apart: a tile full of loot is still walked over.
        let gold = Graphic(0x0EED);
        assert!(
            !tiles.static_tile(gold.0).flags.is_blocking(),
            "gold's tiledata calls it impassable"
        );
        let clutter = of(&[item(100, 100, 0, gold)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AsTheyStand)
                .is_none(),
            "a coin on the floor stopped a step"
        );
    }

    /// `MetalDoor` facing 0: shut on the even graphic, open on the odd — the
    /// family `client/render`'s door table was ported off.
    const DOOR_SHUT: Graphic = Graphic(0x0675);
    const DOOR_OPEN: Graphic = Graphic(0x0676);

    /// **The measurement that killed this module's old argument.** It used to
    /// hold that no door state had to be tracked, because "the shut leaf blocks
    /// and the open one does not" by its flags alone. Both leaves are
    /// impassable, so a client reading the flags refuses to walk through an open
    /// door — a step the shard allows, which is the mirror-image of the bug this
    /// module was written to fix.
    ///
    /// Pinned because it is the whole reason the door table is consulted at all,
    /// and because it is the kind of fact that reads like a typo a year later.
    #[test]
    fn an_open_door_s_own_art_is_impassable_just_like_the_shut_one() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        assert!(
            tiles.static_tile(DOOR_SHUT.0).flags.is_blocking(),
            "a shut door is not impassable in tiledata"
        );
        assert!(
            tiles.static_tile(DOOR_OPEN.0).flags.is_blocking(),
            "an open door's art is no longer impassable — the flags may now say what the table is for"
        );
        assert!(!doors::is_open(DOOR_SHUT), "0x0675 is the shut leaf");
        assert!(doors::is_open(DOOR_OPEN), "0x0676 is the open one");
    }

    /// So the open leaf is left out of the index entirely: nothing is in the way
    /// of a body walking through an open door.
    #[test]
    fn an_open_door_is_not_in_the_way() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 0, DOOR_OPEN)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AsTheyStand)
                .is_none(),
            "an open door blocked its own doorway"
        );
    }

    /// And the shut one is in the way *and* known to be openable, which is the
    /// list this module keeps: blocked now, not blocked with the doors open.
    #[test]
    fn a_shut_door_is_in_the_way_now_and_not_with_the_doors_open() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 0, DOOR_SHUT)], &tiles);
        let doorway = Tile::new(100, 100);
        assert!(
            clutter
                .blocker_at(doorway, standing_at(0), Doors::AsTheyStand)
                .is_some(),
            "a shut door let a step through"
        );
        assert!(
            clutter
                .blocker_at(doorway, standing_at(0), Doors::AllOpen)
                .is_none(),
            "a shut door is what 'potentially passable' means; the plan must be able to look past it"
        );
    }

    /// A crate is in the way both ways round — opening a door does not move the
    /// furniture. Without this the two readings would differ by everything
    /// placed rather than by the doors, and a route would be planned through a
    /// stack of barrels nobody can shift.
    #[test]
    fn a_barrel_is_in_the_way_whatever_the_doors_are_doing() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 0, BARREL)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AllOpen)
                .is_some()
        );
    }

    /// A crate dragged into a doorway seals it: the tile is blocked in both
    /// readings even though a door stands there too, because the crate is still
    /// there once the door has swung. `any` over the blockers is what gets this
    /// right, and a naive "is there a door here" would get it wrong.
    #[test]
    fn a_shut_door_with_a_crate_in_it_is_not_potentially_passable() {
        let Some(tiles) = client_tiledata() else {
            return;
        };
        let clutter = of(&[item(100, 100, 0, DOOR_SHUT), item(100, 100, 0, BARREL)], &tiles);
        assert!(
            clutter
                .blocker_at(Tile::new(100, 100), standing_at(0), Doors::AllOpen)
                .is_some(),
            "a doorway with a barrel in it was called openable"
        );
    }
}
