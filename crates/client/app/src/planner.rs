//! Planning a route on a thread that is not the one drawing.
//!
//! # What this is for
//!
//! A long plan is ~25 ms in `release` and a body takes a step every ~200 ms, so
//! for as long as anybody is walking, planning is a large and permanent fraction
//! of the frame — `docs/world/README.md`'s finding 28, and
//! `plans/world/pathfinding/PLAN.md`'s P3. None of the repairs beside it moves
//! that number: `without_folds` shortens the *route*, not the search that
//! proposed it.
//!
//! # What a worker is given, and why it is not a lock
//!
//! The search reads two grounds and they move on different clocks.
//!
//! - The **guide** — the bare facet, the span bake over it, the install's tile
//!   table and the coarse graph — does not change while a body walks. It is
//!   140-odd megabytes, so it cannot be copied per query, and the thread that
//!   draws reads the same map every frame, so it cannot be moved. It is
//!   **shared**: [`Ground::share`](openshard_movement::ground::Ground::share)
//!   carries the argument for the `Arc`, which `docs/style.md` otherwise
//!   refuses.
//! - The **live overlay** and the crowd on it are rewritten whole as the world
//!   arrives (`clutter::project`). They are **copied**, entire, per question.
//!
//! That second half was measured before it was built, because the plan named a
//! cheaper-sounding alternative — cut the region the query can reach and hand
//! only that over — and the measurement inverted it. A castle in view is 311
//! tiles of live layer and copying all of it is **four microseconds, 0.02% of
//! the plan it rides with**; cutting a window around the endpoints is four times
//! dearer and gets dearer with distance, because the overlay is O(what is
//! placed) and a window is O(area). And a window is a *guess* about where the
//! answer will go, where a copy is not a guess at all. See
//! `coarse_bench --handover`, which is where those numbers come from.
//!
//! So there is nothing here for a lock to protect: the shared half is read-only
//! while it is shared, and the half that changes is not shared at all.
//!
//! # The one thing the frame thread owes
//!
//! **A facet's ground is written while nothing is planning over it.** The map,
//! the bake and the coarse graph are all taken back exclusively when they are
//! rebaked or patched, so whoever is about to write one settles this first —
//! [`Planner::settle`], and `Steering::settle_plans` above it. It is bounded by
//! one query and it happens on events that already cost far more than one:
//! chunks arriving from a publish, a facet being replaced, a graph rebaked.
//!
//! Replacing a facet outright is the exception and needs no settling, because
//! nothing is taken back: a plan under way finishes over the facet it started
//! on, and the answer is discarded for being about the wrong pair.
//!
//! # Latency is not the obstacle
//!
//! A walk holds its last plan while the next is asked for — that is what the
//! plan cache is — and an answer that arrives a frame late is a plan from a tile
//! the body has just left, which is the case every replan already handles.

use std::sync::Arc;
use std::sync::mpsc::{
    Receiver,
    Sender,
    TryRecvError,
};

use openshard_map::overlay::{
    Doors,
    Overlay,
};
use openshard_movement::ground::{
    Bedrock,
    Ground,
};
use openshard_movement::{
    Bodies,
    Footing,
    NavigationGraph,
};
use openshard_protocol::world::Point;
use openshard_tiles::TileData;

use crate::steer::{
    Planned,
    Readings,
    plan,
};

/// One query's ground, owned, and the pair it is about.
///
/// Everything [`plan`] reads, in the two forms the decision above split it
/// into: the slow half shared and the fast half copied.
#[derive(Debug)]
pub(crate) struct Question {
    /// Where the body stands.
    pub from:    Point,
    /// The place it was told to go — a place and not a tile, height and all.
    pub goal:    Point,
    /// Which way the shut doors are read for the *live* half, which is the
    /// walk's own reading — see `world::walking_doors`.
    pub doors:   Doors,
    /// The facet's ground and the bake over it, shared.
    pub bedrock: Arc<Bedrock>,
    /// The install's tile table, shared.
    pub tiles:   Arc<TileData>,
    /// The coarse graph, shared. `None` is a client still building one.
    pub coarse:  Option<Arc<NavigationGraph>>,
    /// What the shard has laid over the ground, copied at the moment the
    /// question was asked.
    pub live:    Overlay,
    /// Who else is standing on it, copied the same way and **sorted by
    /// `(x, y)`** — [`Bodies::standing`]'s contract, which the copy inherits
    /// from the slice it was taken from.
    pub bodies:  Vec<Point>,
}

/// One question, answered.
#[derive(Debug)]
pub(crate) struct Answer {
    /// The pair it is about, so a stale answer can be told from a current one:
    /// the body may have walked on, or the destination changed, while this was
    /// being worked out.
    pub from:    Point,
    pub goal:    Point,
    /// The route, and the line the journal owes about it.
    pub planned: Planned,
}

/// What came of asking for a plan.
///
/// Three answers and not a `bool`, because the third sends a caller somewhere
/// else entirely: a worker that is merely busy will answer in a moment, and one
/// that is gone never will.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asking {
    /// The worker has the question.
    Working,
    /// It is still on the last one; this question was not sent. The ordinary
    /// mid-walk state, and not a failure — a plan for a pair the body has
    /// already left is still being worked out.
    Busy,
    /// There is no worker any more.
    Gone,
}

/// A thread that plans, and the two ends of the conversation with it.
///
/// **One question at a time.** A route is replanned when what is left of the
/// last one runs out, which is every few steps, and the walk and the picture of
/// it share one answer — so a queue would only ever hold questions whose pair
/// had already moved on. What a second question does while one is outstanding is
/// nothing: the frame thread walks the plan it has.
#[derive(Debug)]
pub(crate) struct Planner {
    asked:       Sender<Question>,
    answered:    Receiver<Answer>,
    /// The pair the worker is working on, if it is working.
    outstanding: Option<(Point, Point)>,
}

impl Planner {
    /// Start the worker.
    ///
    /// The thread lives as long as this does and stops when the sender is
    /// dropped, which is what closing a window does.
    ///
    /// # Errors
    ///
    /// The operating system refusing a thread. A client that gets one plans on
    /// the frame thread instead, which is what it did before this existed —
    /// slower, and not broken.
    pub(crate) fn start() -> std::io::Result<Self> {
        let (asked, questions) = std::sync::mpsc::channel::<Question>();
        let (answers, answered) = std::sync::mpsc::channel::<Answer>();
        std::thread::Builder::new()
            .name("planner".to_owned())
            .spawn(move || {
                for question in questions {
                    let (from, goal) = (question.from, question.goal);
                    let answer = Answer {
                        from,
                        goal,
                        planned: answer(question),
                    };
                    if answers.send(answer).is_err() {
                        // The window has gone. Nothing here outlives it.
                        return;
                    }
                }
            })?;
        Ok(Self {
            asked,
            answered,
            outstanding: None,
        })
    }

    /// Ask for a plan for this pair, unless something is already being worked
    /// out.
    ///
    /// The three answers are three different things for a caller to do, which
    /// is why they are not a `bool` — see [`Asking`].
    pub(crate) fn ask(&mut self, question: Question) -> Asking {
        if self.outstanding.is_some() {
            return Asking::Busy;
        }
        let pair = (question.from, question.goal);
        match self.asked.send(question) {
            Ok(()) => {
                self.outstanding = Some(pair);
                Asking::Working
            }
            // The worker is gone — its thread panicked, which is the one way
            // this happens. Nothing is outstanding and nothing more will be, so
            // the caller stops waiting on it and plans on its own thread.
            Err(_) => Asking::Gone,
        }
    }

    /// Whatever the worker has finished, without waiting for it.
    pub(crate) fn collect(&mut self) -> Option<Answer> {
        match self.answered.try_recv() {
            Ok(answer) => {
                self.outstanding = None;
                Some(answer)
            }
            Err(TryRecvError::Empty) => None,
            // The worker is gone. Whatever was outstanding is never coming, and
            // saying so is what lets the next `ask` be attempted (and fail
            // honestly) rather than waiting on it forever.
            Err(TryRecvError::Disconnected) => {
                self.outstanding = None;
                None
            }
        }
    }

    /// Wait for the plan being worked out, so the ground under it may be
    /// written.
    ///
    /// **The rendezvous this module's header is about.** It blocks for at most
    /// one query, and only on the events that move a facet's ground — which
    /// each cost more than one query on their own. `None` is a worker with
    /// nothing to finish, which is the ordinary case.
    pub(crate) fn settle(&mut self) -> Option<Answer> {
        self.outstanding?;
        self.outstanding = None;
        self.answered.recv().ok()
    }
}

/// Plan one question, on whichever thread is running.
///
/// The facet the worker holds is a real [`Ground`]: somebody else's bedrock and
/// this question's own copy of what is live on it. That is what keeps the two
/// readings assembled the one way — [`Footing::of`] and [`Footing::guide`] over
/// one value — rather than by a second construction here that could pair them
/// differently.
fn answer(question: Question) -> Planned {
    let Question {
        from,
        goal,
        doors,
        bedrock,
        tiles,
        coarse,
        live,
        bodies,
    } = question;
    let ground = Ground::shared(bedrock, live);
    let readings = Readings {
        live:   Footing::of(&ground, &tiles, doors).among(Bodies::standing(&bodies)),
        guide:  Footing::guide(&ground, &tiles),
        coarse: coarse.as_deref(),
        // Nowhere further to send it: this *is* the thread the question came to.
        // A worker that could pass a question on would be a worker that could
        // pass it back to itself.
        shared: None,
    };
    plan(readings, from, goal)
}
