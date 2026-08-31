//! Free resurrection from a town healer — ServUO's `BaseHealer.OfferResurrection`.
//!
//! A spell or a bandage already resurrects (`death.rs`); this is the third door,
//! the one that costs nothing but the walk. A ghost that double-clicks a healer,
//! or simply comes near one, is asked "wouldst thou like to be resurrected?" and
//! — unlike the other two — comes back at full health, ServUO's own number for
//! this path.

use openshard_protocol::gump::{
    ButtonId,
    CloseGump,
    GUMP_WHITE,
    GumpAnswer,
    GumpButton,
    GumpDisplay,
    GumpId,
    GumpKey,
    GumpLayout,
    GumpPoint,
    GumpResponse,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::components::{
    Client,
    Ghost,
    Healer,
    Position,
};
use openshard_state::sectors::in_range;

use super::*;

/// The healer's confirm dialog. Its own number, distinct from every other
/// window's — see `gates.rs`'s `MOONGATE_GUMP` for the scheme.
pub(super) const HEALER_GUMP: GumpId = openshard_protocol::gump::id::HEALER;

/// "Wouldst thou like to be resurrected?" — ServUO's CONTINUE. Anything else
/// (CANCEL, or the close box) is `GumpAnswer::Closed`, both `ButtonId(0)`.
const HEALER_CONTINUE: ButtonId = ButtonId(1);

/// How near a healer must stand for it to matter — ServUO's `InRange(m, 2)`.
const HEALER_RANGE: u32 = 2;

impl World {
    /// A double-click on a healer. **Not a ServUO behaviour** — `BaseHealer`
    /// overrides no `OnDoubleClick` at all there, only `OnMovement`
    /// ([`offer_resurrection_nearby`]) — added because waiting for a ghost to
    /// take a *step* into range when it may already be standing in it (killed
    /// beside the healer, or arrived by any path other than walking) is a
    /// worse door than a click.
    ///
    /// A no-op on anything but a ghost clicking a healer — the ordinary
    /// paperdoll/shop dispatch already answers a double-click on a healer that
    /// is not that. Out of range says so rather than doing nothing silently:
    /// a click that reached a real healer and still failed should not read
    /// the same as a click on the wrong NPC (the guildmaster standing beside
    /// it, say) or on empty ground.
    pub(super) fn click_healer(&mut self, player: EntityId, target: EntityId) {
        if !self.state.registry.has::<Healer>(target) {
            debug!(?target, "click_healer: target has no Healer component");
            return;
        }
        if !self.state.registry.has::<Ghost>(player) {
            debug!(?player, "click_healer: clicker is not a ghost");
            return;
        }
        if self.healer_in_range(player, target) {
            self.open_healer_gump(player, target);
        } else {
            self.notify_self(player, "You are too far away to be healed.");
        }
    }

    /// Offer every ghost a resurrection the moment a healer comes within reach —
    /// ServUO's `BaseHealer.OnMovement`. Called after a step, so it reads as the
    /// healer noticing arrival rather than a standing poll.
    ///
    /// Throttled by the confirm itself: a ghost already holding this healer's
    /// gump is not asked again on every tile it takes inside the range, and the
    /// pending offer clears the moment it steps back out — so walking away and
    /// returning asks again, and the window never nags.
    pub(super) fn offer_resurrection_nearby(&mut self, ghost: EntityId) {
        if !self.state.registry.has::<Ghost>(ghost) {
            return;
        }
        let Some(&Position(at)) = self.state.registry.get::<Position>(ghost) else {
            return;
        };
        let facet = self.state.facet_of(ghost);
        let pending = self.state.row_of(ghost).and_then(|row| row.healer_gump);
        // Still in range of the healer already asking: leave the gump as it is.
        if let Some(healer) = pending {
            if self.healer_in_range(ghost, healer) {
                return;
            }
        }
        let total_healers = self.state.registry.query::<Healer>().count();
        let nearest = self.state.registry.query::<Healer>().find_map(|(healer, _)| {
            let &Position(there) = self.state.registry.get::<Position>(healer)?;
            (self.state.facet_of(healer) == facet && in_range(at, there, HEALER_RANGE)).then_some(healer)
        });
        debug!(
            ?ghost,
            total_healers,
            found = nearest.is_some(),
            "offer_resurrection_nearby"
        );
        match nearest {
            Some(healer) => self.open_healer_gump(ghost, healer),
            None if pending.is_some() => {
                // Walked out of every healer's range: clear the remembered offer
                // so a later approach asks again, and take the stale window down
                // rather than leave an unanswerable gump on screen.
                if let Some(row) = self.state.row_of_mut(ghost) {
                    row.healer_gump = None;
                }
                self.close_healer_gump(ghost);
            }
            None => {}
        }
    }

    /// Whether `healer` stands within [`HEALER_RANGE`] of `ghost`, on the same
    /// facet.
    fn healer_in_range(&self, ghost: EntityId, healer: EntityId) -> bool {
        match (
            self.state.registry.get::<Position>(ghost),
            self.state.registry.get::<Position>(healer),
        ) {
            (Some(&Position(here)), Some(&Position(there))) => {
                self.state.facet_of(ghost) == self.state.facet_of(healer)
                    && in_range(here, there, HEALER_RANGE)
            }
            _ => false,
        }
    }

    /// Draw the confirm — ServUO's `ResurrectGump(owner, healer, ResurrectMessage.Healer)`,
    /// laid out in plain lines rather than ServUO's cliloc text: `client/render`'s
    /// gump pass has nowhere yet to look a cliloc number up (`gump.rs`'s `window`
    /// drops `Element::Localized`/`Element::Html` — "text this crate cannot lay
    /// out yet"), so a window built the way `ResurrectGump` is would draw a
    /// background and two blank buttons and nothing anyone could read. `label`
    /// is the one text element the client renders today, the same one the public
    /// moongate list already uses for exactly this reason.
    fn open_healer_gump(&mut self, ghost: EntityId, healer: EntityId) {
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(ghost) else {
            debug!(?ghost, "open_healer_gump: ghost has no Client component");
            return;
        };
        let Some(serial) = self.state.registry.serial_of(ghost) else {
            debug!(?ghost, "open_healer_gump: ghost has no serial");
            return;
        };
        debug!(?ghost, ?healer, "open_healer_gump: sending resurrect gump");

        let mut layout = GumpLayout::new();
        layout.page(0);
        layout.background(0, 0, 400, 350, 2600);
        layout.label(160, 20, GUMP_WHITE, "Resurrection");
        layout.label(50, 60, GUMP_WHITE, "It is possible for you to be resurrected");
        layout.label(50, 80, GUMP_WHITE, "here by this healer. Do you wish to try?");
        layout.button(65, 227, 4005, 4007, GumpButton::Reply, 0, HEALER_CONTINUE);
        layout.label(100, 235, GUMP_WHITE, "CONTINUE");
        layout.button(200, 227, 4005, 4007, GumpButton::Reply, 0, ButtonId::CLOSE_BOX);
        layout.label(235, 235, GUMP_WHITE, "CANCEL");

        let (text, lines) = layout.finish();
        // Close then draw, the `CraftGump` order: a client told to draw twice
        // draws two windows.
        self.state.send_packet(
            connection,
            &ServerPacket::CloseGump(CloseGump {
                gump_id: HEALER_GUMP,
                button:  ButtonId::CLOSE_BOX,
            }),
        );
        self.state.send_packet(
            connection,
            &ServerPacket::GumpDisplay(GumpDisplay {
                serial:  GumpKey::on(serial),
                gump_id: HEALER_GUMP,
                at:      GumpPoint::new(100, 0),
                layout:  text.to_owned(),
                lines:   lines.to_vec(),
            }),
        );
        if let Some(row) = self.state.row_of_mut(ghost) {
            row.healer_gump = Some(healer);
        }
    }

    /// Take the confirm off screen without answering it — a ghost that walked
    /// out of range before deciding, or that came back to life another way
    /// while the offer still stood.
    pub(super) fn close_healer_gump(&mut self, ghost: EntityId) {
        if let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(ghost) {
            self.state.send_packet(
                connection,
                &ServerPacket::CloseGump(CloseGump {
                    gump_id: HEALER_GUMP,
                    button:  ButtonId::CLOSE_BOX,
                }),
            );
        }
    }

    /// Answer the confirm. Returns whether the reply was one of ours.
    pub(super) fn handle_healer_gump(&mut self, connection: ConnectionId, response: &GumpResponse) -> bool {
        if response.gump_id.validate(&[HEALER_GUMP]).is_none() {
            return false;
        }
        let Some(&ghost) = self.state.players.get(&connection) else {
            debug!(%connection, "handle_healer_gump: no player for connection");
            return true;
        };
        // Taken, not borrowed: a reply to a window this side never drew finds
        // nothing and does nothing.
        let Some(healer) = self
            .state
            .row_of_mut(ghost)
            .and_then(|row| row.healer_gump.take())
        else {
            debug!(?ghost, "handle_healer_gump: no pending healer offer");
            return true;
        };
        if response.button.interpret() != GumpAnswer::Pressed(HEALER_CONTINUE) {
            debug!(?ghost, "handle_healer_gump: cancelled");
            return true; // CANCEL, or the close box
        }
        // Still a ghost, still near the healer it asked: the window can sit on
        // screen while its owner walks off, or gets raised by a spell in the
        // meantime, and a reach checked only when it opened is not checked at
        // all.
        if self.state.registry.has::<Ghost>(ghost) && self.healer_in_range(ghost, healer) {
            debug!(?ghost, ?healer, "handle_healer_gump: resurrecting");
            self.resurrect(ghost, true);
        } else {
            debug!(
                ?ghost,
                ?healer,
                "handle_healer_gump: confirmed but no longer a ghost or out of range"
            );
        }
        true
    }
}
