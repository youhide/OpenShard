//! The bus: one [`Events`] queue per event type, in one value.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

use crate::queue::{Cursor, Event, Events};

/// Type-erased view of a queue, so the bus can tick every queue it holds
/// without knowing any of their types.
trait AnyQueue: Send + Sync {
    fn update_erased(&mut self);
    fn clear_erased(&mut self);
    fn len_erased(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<E: Event> AnyQueue for Events<E> {
    fn update_erased(&mut self) {
        self.update();
    }

    fn clear_erased(&mut self) {
        self.clear();
    }

    fn len_erased(&self) -> usize {
        self.len()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Every event queue in one world, keyed by event type.
///
/// A bus is a plain value the world server owns — not a global, not a
/// singleton. Systems take `&mut EventBus` to send and `&EventBus` to read.
///
/// ```
/// use openshard_events::EventBus;
///
/// struct PlayerMoved { x: u16, y: u16 }
///
/// let mut bus = EventBus::new();
/// let mut cursor = bus.cursor::<PlayerMoved>();
///
/// bus.send(PlayerMoved { x: 10, y: 20 });
///
/// let moves: Vec<_> = bus.read(&mut cursor).map(|m| (m.x, m.y)).collect();
/// assert_eq!(moves, vec![(10, 20)]);
/// ```
///
/// # Ticking
///
/// The game loop calls [`EventBus::update`] exactly once per tick, after every
/// system has run. Anything sent during a tick is readable for that tick and the
/// next; see [`Events`] for why.
#[derive(Default)]
pub struct EventBus {
    queues: HashMap<TypeId, Box<dyn AnyQueue>>,
}

impl fmt::Debug for EventBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventBus")
            .field("event_types", &self.queues.len())
            .field("buffered", &self.buffered())
            .finish()
    }
}

impl EventBus {
    /// A bus with no queues. Queues appear as event types are used.
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit an event, creating its queue if this is the first one.
    pub fn send<E: Event>(&mut self, event: E) {
        self.queue_mut::<E>().send(event);
    }

    /// Emit many events of one type at once.
    pub fn extend<E: Event, I: IntoIterator<Item = E>>(&mut self, events: I) {
        self.queue_mut::<E>().extend(events);
    }

    /// Read everything `cursor` has not seen, advancing it.
    ///
    /// Yields nothing if no `E` has ever been sent — an unused event type is an
    /// empty read, not an error, so a system can read events no one emits yet.
    pub fn read<'a, E: Event>(
        &'a self,
        cursor: &mut Cursor<E>,
    ) -> Box<dyn Iterator<Item = &'a E> + 'a> {
        match self.queue::<E>() {
            Some(queue) => Box::new(queue.read(cursor)),
            None => Box::new(std::iter::empty()),
        }
    }

    /// A cursor for `E`, positioned at the oldest readable event.
    ///
    /// Take this once at startup and keep it; a fresh cursor every tick would
    /// re-read the previous tick's events.
    pub fn cursor<E: Event>(&self) -> Cursor<E> {
        self.queue::<E>()
            .map_or_else(Cursor::default, Events::cursor)
    }

    /// A cursor for `E` that skips everything already buffered.
    pub fn cursor_at_end<E: Event>(&self) -> Cursor<E> {
        self.queue::<E>()
            .map_or_else(Cursor::default, Events::cursor_at_end)
    }

    /// Borrow `E`'s queue, if any `E` has ever been sent.
    pub fn queue<E: Event>(&self) -> Option<&Events<E>> {
        self.queues
            .get(&TypeId::of::<E>())?
            .as_any()
            .downcast_ref::<Events<E>>()
    }

    /// Borrow `E`'s queue, creating it if needed.
    pub fn queue_mut<E: Event>(&mut self) -> &mut Events<E> {
        self.queues
            .entry(TypeId::of::<E>())
            .or_insert_with(|| Box::new(Events::<E>::new()))
            .as_any_mut()
            .downcast_mut::<Events<E>>()
            .expect("queue registered under a mismatched TypeId")
    }

    /// Advance every queue by one tick. Call once per tick, after all systems.
    ///
    /// Queues are ticked together so that "one tick" means the same thing for
    /// every event type. Ticking them piecemeal would make an event's lifetime
    /// depend on which systems happened to run.
    pub fn update(&mut self) {
        for queue in self.queues.values_mut() {
            queue.update_erased();
        }
    }

    /// Drop every buffered event of every type. Sequence numbers are kept, so
    /// existing cursors stay valid.
    pub fn clear(&mut self) {
        for queue in self.queues.values_mut() {
            queue.clear_erased();
        }
    }

    /// How many event types have a queue.
    pub fn event_types(&self) -> usize {
        self.queues.len()
    }

    /// How many events are buffered across every queue.
    pub fn buffered(&self) -> usize {
        self.queues.values().map(|q| q.len_erased()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(PartialEq, Debug)]
    struct Login(u32);

    #[derive(PartialEq, Debug)]
    struct Logout(u32);

    #[test]
    fn round_trips_one_event_type() {
        let mut bus = EventBus::new();
        let mut cursor = bus.cursor::<Login>();

        bus.send(Login(1));
        bus.send(Login(2));

        let seen: Vec<u32> = bus.read(&mut cursor).map(|l| l.0).collect();
        assert_eq!(seen, vec![1, 2]);
        assert_eq!(bus.read(&mut cursor).count(), 0);
    }

    #[test]
    fn event_types_do_not_interfere() {
        let mut bus = EventBus::new();
        let mut logins = bus.cursor::<Login>();
        let mut logouts = bus.cursor::<Logout>();

        bus.send(Login(1));
        bus.send(Logout(2));

        assert_eq!(bus.event_types(), 2);
        assert_eq!(
            bus.read(&mut logins).map(|l| l.0).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            bus.read(&mut logouts).map(|l| l.0).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn reading_an_unused_event_type_is_empty_not_a_panic() {
        // A system may read events that nothing emits yet — a plugin that has
        // not loaded, a subsystem not wired up.
        let bus = EventBus::new();
        let mut cursor = bus.cursor::<Login>();
        assert_eq!(bus.read(&mut cursor).count(), 0);
        assert_eq!(bus.event_types(), 0, "reading must not create a queue");
    }

    #[test]
    fn a_cursor_taken_before_the_queue_existed_still_works() {
        let mut bus = EventBus::new();
        // Cursor first, queue second: systems take cursors at startup, long
        // before the first event of that type is sent.
        let mut cursor = bus.cursor::<Login>();
        bus.send(Login(1));
        assert_eq!(
            bus.read(&mut cursor).map(|l| l.0).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn update_ticks_every_queue_together() {
        let mut bus = EventBus::new();
        bus.send(Login(1));
        bus.send(Logout(1));
        assert_eq!(bus.buffered(), 2);

        bus.update();
        assert_eq!(bus.buffered(), 2, "still readable one tick on");

        bus.update();
        assert_eq!(bus.buffered(), 0, "both dropped on the same tick");
    }

    #[test]
    fn independent_readers_across_ticks() {
        let mut bus = EventBus::new();
        let mut fast = bus.cursor::<Login>();
        let mut slow = bus.cursor::<Login>();

        bus.send(Login(1));
        assert_eq!(
            bus.read(&mut fast).map(|l| l.0).collect::<Vec<_>>(),
            vec![1]
        );

        bus.update();
        bus.send(Login(2));

        assert_eq!(
            bus.read(&mut fast).map(|l| l.0).collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            bus.read(&mut slow).map(|l| l.0).collect::<Vec<_>>(),
            vec![1, 2],
            "the slow reader catches up on both"
        );
    }

    #[test]
    fn clear_empties_every_queue() {
        let mut bus = EventBus::new();
        let mut cursor = bus.cursor::<Login>();
        bus.send(Login(1));
        bus.send(Logout(1));

        bus.clear();
        assert_eq!(bus.buffered(), 0);
        assert_eq!(bus.read(&mut cursor).count(), 0);

        bus.send(Login(2));
        assert_eq!(
            bus.read(&mut cursor).map(|l| l.0).collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn extend_goes_through_the_bus() {
        let mut bus = EventBus::new();
        let mut cursor = bus.cursor::<Login>();
        bus.extend([Login(1), Login(2), Login(3)]);
        assert_eq!(
            bus.read(&mut cursor).map(|l| l.0).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn buses_are_independent() {
        let mut a = EventBus::new();
        let b = EventBus::new();
        let mut ca = a.cursor::<Login>();
        let mut cb = b.cursor::<Login>();

        a.send(Login(1));
        assert_eq!(a.read(&mut ca).count(), 1);
        assert_eq!(b.read(&mut cb).count(), 0, "no global state between buses");
    }

    #[test]
    fn bus_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EventBus>();
        assert_send_sync::<Cursor<Login>>();
    }
}
