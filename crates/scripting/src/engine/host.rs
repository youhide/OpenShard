use super::*;

/// Where a mobile is, as far as a script can see — the read model the engine
/// keeps up to date from the events it is handed, so a hook reads it without a
/// round-trip into the world.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct View {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) z: i8,
}

/// The Rust state the ops reach, stored in the runtime's [`OpState`].
///
/// Reads come out of `entities`; writes go into `outbox`. That asymmetry is the
/// engine's whole contract with a script in one struct: look at the world
/// directly, change it only by asking.
#[derive(Default)]
pub(super) struct Host {
    pub(super) entities: HashMap<Serial, View>,
    pub(super) outbox: Vec<Command>,
}

impl Host {
    /// Fold a domain event into the read model. The same event the script's
    /// handler sees also keeps this current — there is no second bookkeeping
    /// path to forget.
    pub(super) fn apply(&mut self, event: &Event) {
        // By reference, not by value: an `Event` is no longer `Copy` (speech
        // carries a `String`), and the read model only needs the position-bearing
        // events anyway.
        match event {
            Event::PlayerEntered { serial, x, y, z } | Event::MobileSpawned { serial, x, y, z } => {
                self.entities.insert(
                    *serial,
                    View {
                        x: *x,
                        y: *y,
                        z: *z,
                    },
                );
            }
            Event::MobileMoved {
                serial, x, y, z, ..
            } => {
                self.entities.insert(
                    *serial,
                    View {
                        x: *x,
                        y: *y,
                        z: *z,
                    },
                );
            }
            Event::PlayerLeft { serial } => {
                self.entities.remove(serial);
            }
            _ => {}
        }
    }
}
