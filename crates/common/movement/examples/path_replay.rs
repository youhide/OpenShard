//! Ask again, over the real facet, what a live client was asked in the game —
//! and say what is different about the answer.
//!
//! ```sh
//! # in the game: one line per click and per replan, unless F1 says otherwise
//! cargo run -p openshard-playground
//!
//! # afterwards: what happened, and then the click that went wrong
//! cargo run --release -p openshard-movement --example path_replay -- --list
//! cargo run --release -p openshard-movement --example path_replay -- --episode 3
//!
//! # the session before this one
//! cargo run --release -p openshard-movement --example path_replay -- \
//!   --journal path-journal.prev.jsonl --list
//! ```
//!
//! `--release`, and it matters: this replays every plan of an episode, and a
//! debug build's A\* is roughly twenty times slower than the one the session
//! ran.
//!
//! # What it can and cannot tell you
//!
//! The journal holds the **question** — where the body stood, where the player
//! pointed, and what came back. This opens the same facet and asks it again over
//! the **bare map**: no doors, no crates, no houses anybody built, nobody
//! standing in the way. So the two answers agreeing means the live layer had no
//! part in the report, and the two disagreeing localises what did: the step
//! where they part is the tile something was standing on.
//!
//! That is the whole design, and it is why the journal carries no slice of the
//! world. A capture of the overlay would replay the same answer by construction
//! and prove nothing; what a person wants next is a *test*, and a test says
//! `overlay.set(tile, vec![Cover::door(z, 20)])` — a scene somebody can read.
//! This report's job is to say which tile that line goes on.
//!
//! # The three verdicts
//!
//! - **Same answer.** The map alone reproduces the session's route. Whatever is
//!   wrong is in the search or in the destination, and a test needs no live
//!   layer at all: `real_routes.rs` is where it goes.
//! - **The bare map refuses a step the session walked.** Something the shard had
//!   put there was carrying the body — a house floor, a ship's deck, a stair. A
//!   test builds that surface at the named tile.
//! - **The bare map walks further than the session did.** Something was in the
//!   way that this replay does not have — a shut door, a crate, somebody
//!   standing there. A test places it at the named tile.
//!
//! Under each of them the run also re-asks the search with a generous budget,
//! because "there is no way there" and "seven hundred nodes were not enough" are
//! one refusal in the file and two different bugs.

use std::path::PathBuf;

use clap::Parser;
use openshard_map::grid::Tile;
use openshard_map::overlay::{
    Doors,
    Overlay,
};
use openshard_movement::bake::{
    OpenFacet,
    WorldSource,
    open_facet,
};
use openshard_movement::{
    COARSE_MIN_DISTANCE,
    Footing,
    MapTerrain,
    NavigationGraph,
    PathSearch,
    SearchExit,
    Weight,
    destination_place,
    find_long_path,
    find_path,
    search_path,
    step_allowed,
};
use openshard_pathlog::read::{
    Ending,
    Episode,
};
use openshard_pathlog::record::{
    self,
    Place,
    Step,
};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::{
    Facet,
    Point,
};

/// What a search is given when the question is "would a bigger budget have
/// arrived" rather than "what does the client do".
///
/// Large enough that nothing on a facet is refused by it for want of nodes, and
/// small enough to answer in well under a second on the ground a person is
/// asking about.
const GENEROUS: usize = 50_000;

#[derive(Debug, Parser)]
#[command(about = "Replay the routes a live client planned, over the real facet")]
struct Cli {
    /// The journal a session wrote.
    ///
    /// A client writes `path-journal.jsonl` where it was started, unless the F1
    /// window says otherwise, and keeps the session before it as
    /// `path-journal.prev.jsonl` — so this is normally not named at all.
    #[arg(long, default_value = openshard_pathlog::write::DEFAULT_PATH)]
    journal:  PathBuf,
    /// The client install the facet is read from.
    ///
    /// Optional, because `--list` is a reading of the journal alone: what a
    /// session did is in the file, and only re-asking its questions needs a
    /// map.
    #[arg(long, env = "OPENSHARD_CLIENT")]
    client:   Option<PathBuf>,
    /// The base set the shard is running, when it is not the install's own map.
    ///
    /// The journal's session line names the file it was; this has to be the
    /// path to it, because a name is not a location.
    #[arg(long, env = "OPENSHARD_BASE_SET")]
    base_set: Option<PathBuf>,
    /// List the episodes and stop: one line per destination.
    #[arg(long)]
    list:     bool,
    /// Which episode to replay, from one. The last one by default — which is
    /// almost always the click somebody has just come to complain about.
    #[arg(long, value_name = "N")]
    episode:  Option<usize>,
    /// Replay one plan of that episode rather than all of them.
    #[arg(long, value_name = "N")]
    plan:     Option<usize>,
    /// How far around the interesting tile to draw the ground.
    #[arg(long, default_value_t = 10, value_name = "TILES")]
    radius:   u16,
}

/// The facet this replay walks, and the coarse graph beside it.
///
/// The graph is `None` on a real state and not a failure — a client without one
/// refuses every long destination — and the report says which of the two it
/// replayed under, because the session's own line says which the session had.
fn open(cli: &Cli, facet: Facet) -> (OpenFacet, Option<NavigationGraph>) {
    let source = match &cli.base_set {
        Some(path) => WorldSource::BaseSet(path),
        None => WorldSource::Install,
    };
    let client = cli
        .client
        .as_ref()
        .expect("replaying a plan needs the facet it was planned over: pass --client");
    let ground = open_facet(client, source, facet).expect("the facet should load");
    let coarse = ground
        .coarse()
        .map_err(|error| eprintln!("no coarse graph: {error}"))
        .ok();
    (ground, coarse)
}

/// The client's own plan, in the client's own order: the bounded search, and the
/// coarse graph after it for anything further than a few tiles.
///
/// `steer.rs`'s `Readings::path` written out — with one footing rather than two,
/// because nothing is placed on this ground and a live reading and a guide
/// reading of a bare facet are the same reading.
fn client_plan(
    footing: &Footing<'_>,
    coarse: Option<&NavigationGraph>,
    from: Point,
    to: Point,
    budget: usize,
) -> Option<Vec<Direction>> {
    if let Some(local) = find_path(footing, from, to, budget, Weight::PLANNING) {
        return Some(local);
    }
    let distance = u32::from(from.x.abs_diff(to.x)).max(u32::from(from.y.abs_diff(to.y)));
    if distance <= COARSE_MIN_DISTANCE {
        return None;
    }
    coarse.and_then(|graph| find_long_path(footing, footing, graph, from, to, budget, Weight::PLANNING))
}

/// Walk a recorded route over ground of this replay's own, and answer where it
/// stopped being walkable.
///
/// `Ok(end)` is a route every step of which the bare map allows; `Err((index,
/// at))` is the first step it refuses, and the place the body was standing when
/// it was refused. That index is the whole of the second verdict: it names the
/// tile something was standing on in the session and is not standing on here.
fn walk(footing: &Footing<'_>, from: Point, steps: &[Step]) -> Result<Point, (usize, Point)> {
    let mut at = from;
    for (index, step) in steps.iter().enumerate() {
        match step_allowed(footing, at, step.direction()) {
            Some(next) => at = next,
            None => return Err((index, at)),
        }
    }
    Ok(at)
}

/// Every place to stand in a square around a point, printed as a picture.
///
/// The highest surface alone, which is all this has to show: where the building
/// is, where the water starts, and which tiles have nothing at all.
fn picture(terrain: &MapTerrain<'_>, centre: Point, radius: u16) {
    println!(
        "  the ground around ({}, {}), '@' is the body, heights against its own {}:",
        centre.x, centre.y, centre.z
    );
    for y in centre.y.saturating_sub(radius)..=centre.y.saturating_add(radius) {
        let mut row = String::new();
        for x in centre.x.saturating_sub(radius)..=centre.x.saturating_add(radius) {
            let highest = terrain.surfaces(x, y).into_iter().max();
            row.push(match (x, y) {
                _ if (x, y) == (centre.x, centre.y) => '@',
                _ => {
                    match highest {
                        None => '#',
                        Some(z) if z > i32::from(centre.z) + 10 => 'X',
                        Some(z) if z > i32::from(centre.z) + 2 => '+',
                        Some(z) if z < i32::from(centre.z) - 10 => 'v',
                        Some(_) => '.',
                    }
                }
            });
        }
        println!("  {row}");
    }
    println!("  # nothing to stand on   v well below   . about level   + above   X a storey up");
}

/// One recorded plan, asked again.
fn replay_plan(
    ground: &OpenFacet,
    coarse: Option<&NavigationGraph>,
    footing: &Footing<'_>,
    seq: u64,
    plan: &record::Plan,
    radius: u16,
    budget: usize,
) {
    let from = plan.from.point();
    let to = plan.to.point();
    println!();
    println!(
        "plan at line {seq}: ({}, {}, {}) -> ({}, {}, {})",
        from.x, from.y, from.z, to.x, to.y, to.z
    );
    println!(
        "  recorded: resolved ({}, {}, {})  {}  {} µs",
        plan.resolved.x,
        plan.resolved.y,
        plan.resolved.z,
        match &plan.refusal {
            Some(refusal) => format!("refusal {refusal:?}"),
            None => "arrived".to_owned(),
        },
        plan.elapsed_us,
    );
    println!(
        "            live       arrived={} exit={:?} explored={} written={}{}",
        plan.live.arrived,
        plan.live.exit,
        plan.live.explored,
        plan.live.written,
        match &plan.live.long {
            Some(long) => format!(" long={long:?}"),
            None => String::new(),
        },
    );
    if let Some(open) = &plan.doors_open {
        println!(
            "            doors open arrived={} exit={:?} explored={}{}",
            open.arrived,
            open.exit,
            open.explored,
            match &open.long {
                Some(long) => format!(" long={long:?}"),
                None => String::new(),
            },
        );
    }
    println!(
        "            open   {} steps: {}",
        plan.open.len(),
        record::route_text(&plan.open)
    );
    if !plan.barred.is_empty() {
        println!(
            "            barred {} steps: {}",
            plan.barred.len(),
            record::route_text(&plan.barred)
        );
    }

    // The destination as *this* ground resolves it. A click carries a picture's
    // height and the search compares against a place to stand; a resolution that
    // has moved is a report about the map rather than about the search.
    let resolved = destination_place(footing, from, to);
    if resolved != plan.resolved.point() {
        println!(
            "  ! the destination resolves differently here: ({}, {}, {}) against the session's ({}, {}, {}) \
             — the live layer had a surface on that column",
            resolved.x, resolved.y, resolved.z, plan.resolved.x, plan.resolved.y, plan.resolved.z,
        );
    }

    // What the same client plan does over the bare facet.
    let bare = search_path(footing, from, to, budget, Weight::PLANNING);
    let planned = client_plan(footing, coarse, from, to, budget);
    println!(
        "  replayed: live       arrived={} exit={:?} explored={} written={}",
        bare.arrived, bare.exit, bare.explored, bare.written,
    );
    let replayed: Vec<Step> = planned
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|&step| Step::of(step))
        .collect();
    // Whether the *client's* plan arrived, which is not `bare.arrived`: a long
    // destination is answered by the coarse graph after the bounded search has
    // already given up, and that is an arrival the bounded reading does not
    // know about.
    let arrived = match &planned {
        Some(route) => {
            walk(
                footing,
                from,
                &route.iter().map(|&step| Step::of(step)).collect::<Vec<_>>(),
            )
            .is_ok_and(|end| end == resolved)
        }
        None => false,
    };
    println!(
        "            open   {} steps: {}{}",
        replayed.len(),
        record::route_text(&replayed),
        match arrived {
            true => " (arrives)",
            false => " (does not arrive)",
        },
    );

    verdict(
        ground,
        footing,
        plan,
        &replayed,
        Answers {
            from,
            to,
            arrived,
            radius,
            budget,
        },
    );
}

/// What the replay found, for the paragraph that compares it with the session.
///
/// Five values that travel to one place together — the shape `Rigour` is in the
/// search itself — rather than five more arguments on a function that already
/// takes four.
struct Answers {
    from:    Point,
    to:      Point,
    /// Whether the replayed client plan reaches the destination, coarse graph
    /// and all.
    arrived: bool,
    radius:  u16,
    budget:  usize,
}

/// What the two answers, side by side, say about the world the session was in.
fn verdict(
    ground: &OpenFacet,
    footing: &Footing<'_>,
    plan: &record::Plan,
    replayed: &[Step],
    answers: Answers,
) {
    let Answers {
        from,
        to,
        arrived,
        radius,
        budget,
    } = answers;
    println!("  verdict:");
    // First: is the recorded route even walkable here? That is the sharpest
    // question, because its answer is a *tile*.
    match walk(footing, from, &plan.open) {
        Ok(end) => {
            let recorded_end = plan
                .open_points
                .last()
                .copied()
                .unwrap_or(Place::of(from))
                .point();
            if end == recorded_end {
                println!("    the bare map walks the recorded route exactly, and it ends where it ended.");
            } else {
                println!(
                    "    the bare map walks every recorded step but ends at ({}, {}, {}), where the \
                     session ended at ({}, {}, {}) — a surface under the route was the live layer's.",
                    end.x, end.y, end.z, recorded_end.x, recorded_end.y, recorded_end.z,
                );
            }
        }
        Err((index, at)) => {
            let onto = plan
                .open_points
                .get(index)
                .copied()
                .map(|place| format!("({}, {}, {})", place.x, place.y, place.z))
                .unwrap_or_else(|| "somewhere the journal did not record".to_owned());
            println!(
                "    the bare map REFUSES recorded step {} ({:?}) from ({}, {}, {}) onto {onto}.",
                index + 1,
                plan.open[index],
                at.x,
                at.y,
                at.z,
            );
            println!(
                "    Something the shard had placed there was carrying the body — a house floor, a \
                 deck, a stair. A test builds that surface at that tile."
            );
            picture(&ground.terrain(), at, radius);
        }
    }

    // Second: the two plans, compared — and *arriving* compared before the
    // steps are, because two routes that both arrive differing is an ordinary
    // fact about a map with two ways round a building, and one of them not
    // arriving is the report.
    let recorded_arrived = plan.refusal.is_none();
    match (recorded_arrived, arrived) {
        (true, true) if plan.open == replayed => {
            println!("    both arrive, by the same route, step for step.");
        }
        (true, true) => {
            parting(plan, replayed);
            println!(
                "    Both still arrive. Two ways round on one map are equally good answers, so \
                 this on its own is not a bug — unless the session's route is the longer one, \
                 which is a live obstacle pushing it off the short way."
            );
        }
        (true, false) => {
            println!(
                "    the session ARRIVED and the bare map does not: the route ran over something \
                 the map does not have — a house floor, a ship's deck, a placed stair. A test \
                 builds that surface."
            );
            picture(&ground.terrain(), from, radius);
        }
        (false, true) => {
            println!(
                "    the bare map ARRIVES where the session refused ({:?}): something was in the \
                 way that is not here — a shut door, a crate, somebody standing on the tile. A \
                 test places it. (Unless the two runs differ over the coarse graph — the session \
                 line above says which had one.)",
                plan.refusal
                    .expect("a plan that did not arrive carries its reason"),
            );
            picture(&ground.terrain(), from, radius);
        }
        (false, false) => {
            println!("    neither arrives: the refusal is the map's, and a test needs no live layer.");
            parting(plan, replayed);
        }
    }

    // Third: was the refusal the budget rather than the ground? One search
    // answers it, and it is the difference between "there is no way", "ask
    // again from closer", and "the ground was not this ground".
    if !recorded_arrived {
        let generous = search_path(footing, from, to, GENEROUS, Weight::PLANNING);
        println!(
            "    at {GENEROUS} nodes: arrived={} exit={:?} explored={} — {}",
            generous.arrived,
            generous.exit,
            generous.explored,
            budget_verdict(&generous, budget),
        );
    }
}

/// Where two routes stop agreeing, for a reader who wants the step number.
fn parting(plan: &record::Plan, replayed: &[Step]) {
    let first = plan
        .open
        .iter()
        .zip(replayed)
        .position(|(recorded, replayed)| recorded != replayed);
    match first {
        Some(index) => {
            println!(
                "    the routes part at step {}: the session went {:?}, the bare map goes {:?}.",
                index + 1,
                plan.open[index],
                replayed[index],
            );
        }
        None => {
            println!(
                "    one route is a prefix of the other: {} recorded steps against {} replayed.",
                plan.open.len(),
                replayed.len(),
            );
        }
    }
}

/// What a generous search says about a bounded one that refused.
///
/// **The node count is compared against the session's own budget**, which is
/// the whole of this: a way that costs fifteen nodes was never refused by a
/// budget of seven hundred, so a session that refused it was not walking on
/// this ground. Reporting that as "the budget refused it" — which is what
/// reading `arrived` alone says — sends a person to tune a number that was
/// never the problem.
fn budget_verdict(generous: &PathSearch, budget: usize) -> String {
    match (generous.arrived, generous.exit) {
        (true, _) if generous.explored <= budget => {
            format!(
                "the way costs {} nodes here, inside the session's own {budget} — so the budget is \
                 NOT what refused it, and the ground the session searched was not this ground",
                generous.explored
            )
        }
        (true, _) => {
            format!(
                "the way exists and costs {} nodes, past the session's {budget}: the budget is what \
                 refused it",
                generous.explored
            )
        }
        (false, SearchExit::Exhausted) => {
            "everything reachable was settled: there is genuinely no way there on this map".to_owned()
        }
        (false, SearchExit::Budget | SearchExit::Goal) => {
            "even this budget ran out — the destination is a long way off, and a coarse corridor is \
             the only thing that answers it"
                .to_owned()
        }
    }
}

/// One line per episode, for choosing which one to look at.
fn list(episodes: &[Episode]) {
    println!("{} episodes", episodes.len());
    for episode in episodes {
        let (from, to) = match &episode.order {
            Some(order) => (order.from, order.to),
            // An episode whose click the journal missed is named by the plans
            // under it, which is what a replay would open anyway.
            None => {
                match episode.plans.first() {
                    Some((_, plan)) => (plan.from, plan.to),
                    None => continue,
                }
            }
        };
        println!(
            "  {:>3}  lines {:>4}..{:<4}  ({:>5},{:>5},{:>4}) -> ({:>5},{:>5},{:>4})  {} plans  {}",
            episode.number,
            episode.seq_from,
            episode.seq_to,
            from.x,
            from.y,
            from.z,
            to.x,
            to.y,
            to.z,
            episode.plans.len(),
            match &episode.ending {
                Some(Ending::Arrived(_)) => "arrived".to_owned(),
                Some(Ending::Abandoned(abandonment)) => {
                    format!("abandoned after {} stalled steps", abandonment.stalled)
                }
                None => "still walking when the session ended".to_owned(),
            },
        );
    }
}

fn main() -> Result<(), openshard_pathlog::read::ReadError> {
    let cli = Cli::parse();
    let entries = openshard_pathlog::read::read(&cli.journal)?;
    let episodes = openshard_pathlog::read::episodes(&entries);
    if episodes.is_empty() {
        println!("{}: no route was planned in that session", cli.journal.display());
        return Ok(());
    }
    if cli.list {
        list(&episodes);
        return Ok(());
    }

    let number = cli.episode.unwrap_or(episodes.len());
    let episode = episodes
        .iter()
        .find(|episode| episode.number == number)
        .unwrap_or_else(|| panic!("there is no episode {number}; there are {}", episodes.len()));

    // The session line **this episode** was planned under, which is not always
    // the file's first: a client that bakes a graph when the world arrives says
    // so with a second one, and every episode after that moment had a corridor
    // to ask. Replaying an early episode under the late line — or the other way
    // round — is exactly the wrong guess the field exists to prevent.
    let session = openshard_pathlog::read::session_at(&entries, episode.seq_from);

    let facet = Facet(session.map_or(0, |session| session.facet));
    let budget = session.map_or(700, |session| session.budget);
    if let Some(session) = session {
        println!(
            "session: facet {} {} coarse graph, budget {}, weight {}{}",
            session.facet,
            match session.coarse {
                true => "with a",
                false => "WITHOUT a",
            },
            session.budget,
            session.weight,
            match &session.world {
                Some(world) => format!(", world {world}"),
                None => String::new(),
            },
        );
    }
    let (ground, coarse) = open(&cli, facet);
    if session.is_some_and(|session| session.coarse) && coarse.is_none() {
        println!(
            "! the session had a coarse graph and this replay has none: every long destination \
             will be refused here for a reason the session did not have"
        );
    }
    let nothing_placed = Overlay::default();
    let terrain = ground.terrain();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);

    println!();
    println!(
        "episode {} of {}, lines {}..{}: {} plans, {}",
        episode.number,
        episodes.len(),
        episode.seq_from,
        episode.seq_to,
        episode.plans.len(),
        match &episode.ending {
            Some(Ending::Arrived(arrival)) => {
                format!(
                    "arrived at ({}, {}, {})",
                    arrival.at.x, arrival.at.y, arrival.at.z
                )
            }
            Some(Ending::Abandoned(abandonment)) => {
                format!(
                    "ABANDONED at ({}, {}, {}) after {} stalled steps",
                    abandonment.at.x, abandonment.at.y, abandonment.at.z, abandonment.stalled
                )
            }
            None => "still walking when the session ended".to_owned(),
        },
    );
    if let Some(order) = &episode.order {
        println!(
            "  the click: from ({}, {}, {}) to ({}, {}, {})",
            order.from.x, order.from.y, order.from.z, order.to.x, order.to.y, order.to.z
        );
        let tile = Tile::new(order.to.x, order.to.y);
        println!(
            "  what stands on the destination column here: {:?}",
            ground.terrain().surfaces(tile.x, tile.y)
        );
    }

    for (index, (seq, plan)) in episode.plans.iter().enumerate() {
        if cli.plan.is_some_and(|wanted| wanted != index + 1) {
            continue;
        }
        replay_plan(&ground, coarse.as_ref(), &footing, *seq, plan, cli.radius, budget);
    }
    Ok(())
}
