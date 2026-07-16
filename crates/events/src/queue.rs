//! The per-type event queue and its reader cursors.

use std::fmt;
use std::marker::PhantomData;

/// Anything that can be sent through the bus.
///
/// The blanket impl means you never write `impl Event for Foo` — any plain data
/// type that can cross threads is already an event.
pub trait Event: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> Event for T {}

/// A double-buffered queue of events of one type.
///
/// # Lifetime of an event
///
/// Events live for exactly two calls to [`Events::update`], which the game loop
/// makes once per tick. That gives every reader a full tick to see an event
/// regardless of system ordering: a system that runs *before* the emitter still
/// picks it up on the next tick instead of missing it forever.
///
/// The cost of that guarantee is bounded — buffers are swapped and reused, never
/// grown without limit — and it is why the bus does not need to know who is
/// listening. Nothing here is a subscription.
pub struct Events<E> {
    /// Events from the previous tick. Still readable, dropped on next update.
    older: Vec<E>,
    /// Events sent during the current tick.
    newer: Vec<E>,
    /// Sequence number of `older[0]`.
    older_start: u64,
    /// Sequence number of `newer[0]`.
    newer_start: u64,
    /// Sequence number the next sent event will get.
    next_sequence: u64,
}

impl<E> Default for Events<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> fmt::Debug for Events<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Events")
            .field("event", &std::any::type_name::<E>())
            .field("buffered", &self.len())
            .field("next_sequence", &self.next_sequence)
            .finish()
    }
}

impl<E> Events<E> {
    /// An empty queue.
    pub const fn new() -> Self {
        Self {
            older: Vec::new(),
            newer: Vec::new(),
            older_start: 0,
            newer_start: 0,
            next_sequence: 0,
        }
    }

    /// Emit an event. Every reader that has not yet caught up will see it.
    pub fn send(&mut self, event: E) {
        self.newer.push(event);
        self.next_sequence += 1;
    }

    /// Emit many events at once.
    pub fn extend<I: IntoIterator<Item = E>>(&mut self, events: I) {
        let before = self.newer.len();
        self.newer.extend(events);
        self.next_sequence += (self.newer.len() - before) as u64;
    }

    /// Retire the oldest buffer and start a new one. Call once per tick.
    pub fn update(&mut self) {
        // Swap rather than allocate: `newer` becomes `older`, and the buffer
        // that was `older` is cleared and reused for the coming tick.
        std::mem::swap(&mut self.older, &mut self.newer);
        self.newer.clear();
        self.older_start = self.newer_start;
        self.newer_start = self.next_sequence;
    }

    /// How many events are currently readable.
    pub fn len(&self) -> usize {
        self.older.len() + self.newer.len()
    }

    /// Whether any event is currently readable.
    pub fn is_empty(&self) -> bool {
        self.older.is_empty() && self.newer.is_empty()
    }

    /// Total number of events ever sent through this queue.
    pub const fn sent(&self) -> u64 {
        self.next_sequence
    }

    /// Drop every buffered event. Readers resume from the next one sent.
    pub fn clear(&mut self) {
        self.older.clear();
        self.newer.clear();
        self.older_start = self.next_sequence;
        self.newer_start = self.next_sequence;
    }

    /// A cursor positioned at the oldest readable event.
    pub fn cursor(&self) -> Cursor<E> {
        Cursor {
            next: self.older_start,
            _marker: PhantomData,
        }
    }

    /// A cursor that skips everything already buffered.
    pub fn cursor_at_end(&self) -> Cursor<E> {
        Cursor {
            next: self.next_sequence,
            _marker: PhantomData,
        }
    }

    /// Read everything `cursor` has not seen, advancing it.
    ///
    /// Each reader owns its cursor, so consuming events here does not hide them
    /// from anyone else — three systems can each read every `PlayerMove`.
    pub fn read<'a>(&'a self, cursor: &mut Cursor<E>) -> impl Iterator<Item = &'a E> + 'a {
        let from = cursor.next;
        cursor.next = self.next_sequence;

        // `saturating_sub` handles a cursor left behind by `update`: if `from`
        // predates the buffer, the skip clamps to 0 and the reader gets what
        // survives rather than panicking.
        let skip_older = from.saturating_sub(self.older_start) as usize;
        let skip_newer = from.saturating_sub(self.newer_start) as usize;
        let older = self.older.get(skip_older.min(self.older.len())..).unwrap_or(&[]);
        let newer = self.newer.get(skip_newer.min(self.newer.len())..).unwrap_or(&[]);
        older.iter().chain(newer.iter())
    }

    /// Everything currently buffered, oldest first, without touching a cursor.
    pub fn iter(&self) -> impl Iterator<Item = &E> + '_ {
        self.older.iter().chain(self.newer.iter())
    }

    /// How many events `cursor` has yet to read.
    ///
    /// A cursor stranded behind the buffer counts only what it can still reach —
    /// events dropped by [`Events::update`] are gone regardless of what the
    /// cursor says.
    pub fn unread(&self, cursor: &Cursor<E>) -> usize {
        let from = cursor.next.max(self.older_start);
        self.next_sequence.saturating_sub(from) as usize
    }
}

/// A reader's position in an [`Events`] queue.
///
/// Cursors are owned by whoever reads — a system, a plugin, the replay logger —
/// not by the queue. That is what keeps the bus free of subscription state and
/// makes every reader independent.
///
/// A cursor is bound to one queue by type. Reading a `Cursor<PlayerMove>` from a
/// different `Events<PlayerMove>` compiles but yields nonsense, so keep a cursor
/// next to the bus it came from.
pub struct Cursor<E> {
    next: u64,
    _marker: PhantomData<fn() -> E>,
}

impl<E> Clone for Cursor<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for Cursor<E> {}

impl<E> Default for Cursor<E> {
    /// A cursor at sequence zero: it will read everything still buffered.
    fn default() -> Self {
        Self {
            next: 0,
            _marker: PhantomData,
        }
    }
}

impl<E> fmt::Debug for Cursor<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cursor")
            .field("event", &std::any::type_name::<E>())
            .field("next", &self.next)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(PartialEq, Debug, Clone, Copy)]
    struct Ping(u32);

    fn drain(events: &Events<Ping>, cursor: &mut Cursor<Ping>) -> Vec<u32> {
        events.read(cursor).map(|p| p.0).collect()
    }

    #[test]
    fn reads_in_order_then_stops() {
        let mut events = Events::new();
        let mut cursor = events.cursor();

        events.send(Ping(1));
        events.send(Ping(2));
        assert_eq!(drain(&events, &mut cursor), vec![1, 2]);
        assert_eq!(drain(&events, &mut cursor), Vec::<u32>::new(), "no re-reads");
    }

    #[test]
    fn events_survive_exactly_one_update() {
        let mut events = Events::new();
        events.send(Ping(1));

        events.update();
        let mut cursor = events.cursor();
        assert_eq!(drain(&events, &mut cursor), vec![1], "readable one tick later");

        events.update();
        let mut cursor = events.cursor();
        assert_eq!(
            drain(&events, &mut cursor),
            Vec::<u32>::new(),
            "dropped after the second update"
        );
    }

    #[test]
    fn a_reader_that_runs_before_the_sender_still_sees_the_event() {
        // The ordering guarantee that makes the double buffer worth its cost.
        let mut events = Events::new();
        let mut early = events.cursor();

        // Tick 1: the reader runs first and sees nothing, then the event lands.
        assert_eq!(drain(&events, &mut early), Vec::<u32>::new());
        events.send(Ping(7));
        events.update();

        // Tick 2: the same reader picks it up rather than losing it.
        assert_eq!(drain(&events, &mut early), vec![7]);
    }

    #[test]
    fn readers_are_independent() {
        let mut events = Events::new();
        let mut a = events.cursor();
        let mut b = events.cursor();

        events.send(Ping(1));
        assert_eq!(drain(&events, &mut a), vec![1]);
        assert_eq!(drain(&events, &mut b), vec![1], "one reader cannot consume for another");

        events.send(Ping(2));
        assert_eq!(drain(&events, &mut a), vec![2]);
        assert_eq!(drain(&events, &mut b), vec![2]);
    }

    #[test]
    fn spans_the_buffer_boundary() {
        let mut events = Events::new();
        let mut cursor = events.cursor();

        events.send(Ping(1));
        events.update();
        events.send(Ping(2));

        assert_eq!(
            drain(&events, &mut cursor),
            vec![1, 2],
            "one read spans both buffers, oldest first"
        );
    }

    #[test]
    fn cursor_at_end_skips_history() {
        let mut events = Events::new();
        events.send(Ping(1));
        let mut cursor = events.cursor_at_end();
        assert_eq!(drain(&events, &mut cursor), Vec::<u32>::new());

        events.send(Ping(2));
        assert_eq!(drain(&events, &mut cursor), vec![2]);
    }

    #[test]
    fn a_lagging_cursor_gets_what_survives_instead_of_panicking() {
        let mut events = Events::new();
        let mut cursor = events.cursor();

        events.send(Ping(1));
        events.update();
        events.send(Ping(2));
        events.update(); // Ping(1) is gone now.
        events.send(Ping(3));

        assert_eq!(drain(&events, &mut cursor), vec![2, 3], "missed events, no panic");
    }

    #[test]
    fn default_cursor_reads_everything_buffered() {
        let mut events = Events::new();
        events.send(Ping(1));
        let mut cursor = Cursor::default();
        assert_eq!(drain(&events, &mut cursor), vec![1]);
    }

    #[test]
    fn extend_numbers_events_like_send() {
        let mut events = Events::new();
        let mut cursor = events.cursor();
        events.extend([Ping(1), Ping(2)]);
        events.send(Ping(3));

        assert_eq!(events.sent(), 3);
        assert_eq!(drain(&events, &mut cursor), vec![1, 2, 3]);
    }

    #[test]
    fn unread_counts_what_a_cursor_would_get() {
        let mut events = Events::new();
        let mut cursor = events.cursor();
        assert_eq!(events.unread(&cursor), 0);

        events.send(Ping(1));
        events.send(Ping(2));
        assert_eq!(events.unread(&cursor), 2);

        let _ = drain(&events, &mut cursor);
        assert_eq!(events.unread(&cursor), 0);

        // A cursor stranded behind the buffer reports only what it can reach.
        let stale = events.cursor();
        events.update();
        events.send(Ping(3));
        events.update();
        events.send(Ping(4));
        assert_eq!(events.unread(&stale), 2, "Ping(1) and Ping(2) are unreachable");
    }

    #[test]
    fn clear_drops_history_without_rewinding_sequence() {
        let mut events = Events::new();
        let mut cursor = events.cursor();
        events.send(Ping(1));
        events.clear();

        assert!(events.is_empty());
        assert_eq!(drain(&events, &mut cursor), Vec::<u32>::new());
        events.send(Ping(2));
        assert_eq!(drain(&events, &mut cursor), vec![2], "the queue still works after clear");
    }

    #[test]
    fn buffers_do_not_grow_without_bound() {
        let mut events = Events::new();
        for tick in 0..1_000 {
            events.send(Ping(tick));
            events.update();
            assert!(events.len() <= 2, "at most two ticks of events are retained");
        }
    }
}
