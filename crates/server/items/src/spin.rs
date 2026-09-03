//! A spinning wheel: cotton, flax and wool into thread and yarn.
//!
//! ServUO's `ISpinningWheel` and the four fibres that target it — `Cotton`,
//! `Flax`, `Wool` and `TaintedWool`, each double-clicked in the pack, each
//! raising a cursor that wants a wheel. The wheel then turns for six seconds
//! and the yield goes into the spinner's pack.
//!
//! **The first half of the step without which Tailoring is unreachable except
//! by shopping.** Fifty-six tailoring rows eat cloth; the loom
//! ([`weave`](crate::weave)) makes the bolt that cuts into it, and the loom eats
//! what this module makes. Before the two of them, cloth had exactly one source
//! on the shard — a vendor's shelf.
//!
//! No skill, no roll, no workshop and no tool: like [`cut`](crate::cut), this is
//! an item action rather than a craft, which is why it lives in `items` and not
//! beside `crafting::smelt`. What it *does* need is a **house addon** to be
//! pointed at, and that is the one thing the ore's smelt does not.

use openshard_state::WorldTick;
use openshard_state::components::{
    AddonPart,
    Fibre,
    Spinning,
};

use super::*;

/// ServUO's `SpinTimer`: six seconds from the fibre going on to the yield coming
/// off. Nothing on the wheel is interruptible in that window — a spinner who
/// walks away still gets the thread.
const SPIN_TICKS: u64 = 6 * TICKS_PER_SECOND;

/// "What spinning wheel do you wish to spin this on?"
const WHICH_WHEEL: ClilocId = ClilocId(502_655);
/// "That spinning wheel is being used."
const WHEEL_BUSY: ClilocId = ClilocId(502_656);
/// "Use that on a spinning wheel."
const NOT_A_WHEEL: ClilocId = ClilocId(502_658);
/// "That must be in your pack for you to use it."
const NOT_IN_PACK: ClilocId = ClilocId(1_042_001);

/// A double-clicked pile of fibre raises the object cursor that asks which
/// wheel. Returns whether the item was a fibre at all, so the double-click
/// dispatch can fall through to everything else it knows.
pub fn use_fibre(state: &mut WorldState, spinner: EntityId, fibre: EntityId) -> bool {
    let Some(graphic) = state.registry.get::<Drawn>(fibre).map(|drawn| drawn.id) else {
        return false;
    };
    if Fibre::from_graphic(graphic).is_none() {
        return false;
    }
    // ServUO asks this *before* raising the cursor, and so does this: a pile on
    // the ground is refused with the line that says why, not with a cursor that
    // then refuses the click.
    if !carried_in_pack(state, spinner, fibre) {
        state.localized_message(spinner, NOT_IN_PACK, "");
        return true;
    }
    let (Some(&Client { connection, .. }), Some(serial)) = (
        state.registry.get::<Client>(spinner),
        state.registry.serial_of(spinner),
    ) else {
        return true;
    };
    state.raise_target(spinner, openshard_state::TargetPurpose::Spin { fibre });
    state.localized_message(spinner, WHICH_WHEEL, "");
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(serial.raw()),
            kind:      TargetKind::Object,
        }),
    );
    true
}

/// Put the fibre on the wheel the cursor came back with.
///
/// Everything is re-checked: the pile can be traded away and the wheel taken up
/// between the two packets, and the target serial is the client's word.
pub fn spin(state: &mut WorldState, spinner: EntityId, fibre: EntityId, target: Option<Serial>) {
    let Some(kind) = state
        .registry
        .get::<Drawn>(fibre)
        .copied()
        .and_then(|drawn| Fibre::from_graphic(drawn.id).map(|kind| (kind, drawn.hue)))
    else {
        return; // spent, or no longer a fibre
    };
    let (kind, hue) = kind;
    let Some(wheel) = target
        .and_then(|serial| state.registry.entity_of(serial))
        .and_then(|item| wheel_root(state, item))
    else {
        state.localized_message(spinner, NOT_A_WHEEL, "");
        return;
    };
    if !in_reach(state, wheel, spinner) {
        state.localized_message(spinner, NOT_A_WHEEL, "");
        return;
    }
    if !carried_in_pack(state, spinner, fibre) {
        state.localized_message(spinner, NOT_IN_PACK, "");
        return;
    }
    if state.registry.has::<Spinning>(wheel) {
        state.localized_message(spinner, WHEEL_BUSY, "");
        return;
    }
    let (Some(serial), Some(owner), Some(root)) = (
        state.registry.serial_of(fibre),
        state.registry.serial_of(spinner),
        state.registry.serial_of(wheel),
    ) else {
        return;
    };
    // ServUO's `BeginSpin` consumes first and starts the timer second, and the
    // order is the reservation: one pile cannot be put on two wheels, because
    // after this line there is no pile. The cost is the one the component's own
    // docs name — a restart inside the six seconds loses it.
    consume(state, serial, 1);
    state.registry.insert(
        wheel,
        Spinning {
            due: state.ticks + SPIN_TICKS,
            spinner: owner,
            fibre: kind,
            hue,
        },
    );
    draw_wheel(state, root, true);
}

/// Hand over the yield of every wheel whose six seconds are up, and let it come
/// to rest.
///
/// Beside `close_doors` in the tick and shaped like it: a scan over the wheels
/// that are turning, which is a handful on a shard, rather than a queue of
/// deadlines that would be a second copy of `due` to keep in step.
pub fn advance_spins(state: &mut WorldState) {
    let now = state.ticks;
    let due: Vec<(EntityId, Spinning)> = state
        .registry
        .query::<Spinning>()
        .filter(|(_, spin)| spin.due <= now)
        .map(|(entity, spin)| (entity, *spin))
        .collect();
    for (wheel, spin) in due {
        state.registry.remove::<Spinning>(wheel);
        if let Some(root) = state.registry.serial_of(wheel) {
            draw_wheel(state, root, false);
        }
        let (graphic, amount) = spin.fibre.spun_into();
        // The spinner's own pack, ServUO's `AddToBackpack`. Six seconds is long
        // enough to log out in, though, and this engine's logged-out character
        // has no entity to hand anything to — so the yield falls at the wheel's
        // feet rather than being quietly destroyed along with the fibre that
        // bought it.
        let pack = state
            .registry
            .entity_of(spin.spinner)
            .and_then(|mobile| backpack_of(state, spin.spinner).map(|pack| (mobile, pack)));
        match pack {
            Some((mobile, pack)) => {
                let made = give(state, pack, graphic, spin.hue, amount);
                if made.is_complete() {
                    state.localized_message(mobile, spin.fibre.stowed_message(), "");
                } else {
                    let short = amount - made.given;
                    drop_at_wheel(state, wheel, graphic, spin.hue, short);
                    state.system_message(mobile, "Your pack was too full, and the rest is by the wheel.");
                }
            }
            None => drop_at_wheel(state, wheel, graphic, spin.hue, amount),
        }
    }
}

/// Lay what a wheel made on the wheel's own tile — the fallback for a spinner
/// who logged out or whose pack filled up while the wheel turned.
fn drop_at_wheel(state: &mut WorldState, wheel: EntityId, graphic: Graphic, hue: Hue, amount: u32) {
    if amount == 0 {
        return;
    }
    let (Some(&Position(at)), facet) = (state.registry.get::<Position>(wheel), state.facet_of(wheel)) else {
        return;
    };
    // Clamped to one pile: `spawn_item` takes a `u16`, and no fibre pays more
    // than six of anything, so the clamp can never bite.
    let amount = u16::try_from(amount).unwrap_or(u16::MAX);
    spawn_item(state, graphic, hue, amount, true, at, facet);
}

/// The addon root of a spinning wheel the player clicked, or `None` when what
/// they clicked is not one.
///
/// A click lands on a *component*, which for every wheel this engine installs is
/// also the root; the walk through [`AddonPart::root`] is written anyway, so a
/// wheel that ever grows a second tile does not silently start refusing clicks
/// on it.
fn wheel_root(state: &WorldState, item: EntityId) -> Option<EntityId> {
    let part = state.registry.get::<AddonPart>(item)?;
    part.addon.wheel_arts()?;
    state.registry.entity_of(part.root)
}

/// Draw every tile of one wheel turning, or at rest.
///
/// ServUO swaps the component's `ItemID` in `BeginSpin`/`EndSpin`; the pair is
/// [`AddonKind::wheel_arts`](openshard_state::components::AddonKind::wheel_arts),
/// which is also what says the addon is a wheel at all. The in-place swap itself
/// is [`redraw_item`](crate::redraw_item)'s, once per tile.
fn draw_wheel(state: &mut WorldState, root: Serial, turning: bool) {
    let parts: Vec<(EntityId, Graphic)> = state
        .registry
        .query::<AddonPart>()
        .filter(|(_, part)| part.root == root)
        .filter_map(|(entity, part)| {
            part.addon
                .wheel_arts()
                .map(|(idle, spinning)| (entity, if turning { spinning } else { idle }))
        })
        .collect();
    for (part, graphic) in parts {
        redraw_item(state, part, graphic);
    }
}

/// The tick a wheel started now would finish on — the one place the six seconds
/// are a number, so a test can wait exactly as long as the wheel does.
#[must_use]
pub fn spin_finishes_at(now: WorldTick) -> WorldTick {
    now + SPIN_TICKS
}
