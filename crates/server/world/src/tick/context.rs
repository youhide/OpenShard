use super::*;
use openshard_protocol::context::{ContextMenuFlags, RawContextMenuIndex};
use openshard_protocol::serial::RawSerial;
use openshard_protocol::wire::ClilocId;

/// The cliloc numbers for the default context-menu entries. These are the
/// **`3006xxx`** context-menu range that lives in a modern `cliloc.enu` (verified
/// against a live client) — `3006123` "Open Paperdoll", `3006103` "Buy",
/// `3006104` "Sell", `3000362` "Open". ServUO's `ContextMenuEntry` stores the
/// short `6xxx` form and the value is resolved `+3000000`; a modern client has
/// only the full form, so sending `6123` shows "megacliloc: missing 6123".
const CLILOC_PAPERDOLL: ClilocId = ClilocId(3_006_123);
const CLILOC_OPEN: ClilocId = ClilocId(3_000_362);
const CLILOC_BUY: ClilocId = ClilocId(3_006_103);
const CLILOC_SELL: ClilocId = ClilocId(3_006_104);
/// `3006168` "Quest Log" — the same entry ServUO offers on a player's own menu,
/// and a way into the log for a client whose paperdoll draws no Quest button.
const CLILOC_QUEST_LOG: ClilocId = ClilocId(3_006_168);

/// What a chosen context-menu entry does. Every one routes to a handler a
/// double-click already reaches — the menu decides *what*, the existing rule does
/// *how*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContextAction {
    /// Open the object's paperdoll (a mobile).
    Paperdoll,
    /// Open your own quest log.
    QuestLog,
    /// Use the object — for a container, open it.
    Open,
    /// Open a vendor's buy window.
    Buy,
    /// Offer the vendor's sell window.
    Sell,
}

impl World {
    /// Answer a context-menu request (`0xBF` `0x13`): send the clicked object's
    /// default entries. Off when the shard serves no context menus, or the client
    /// is too old for the new popup format. An object with no entries gets none.
    pub(super) fn context_menu_request(&mut self, connection: ConnectionId, serial: RawSerial) {
        if !self.state.gameplay.context_menus {
            return;
        }
        let Some(version) = self.state.version_of(connection) else {
            return;
        };
        // The new (0x02) popup format only; older clients want the 0x01 layout,
        // which is a later slice.
        if !version.supports(Feature::NewContextMenu) {
            return;
        }
        let Some(object) = serial.validate() else {
            return;
        };
        let Some(entity) = self.state.registry.entity_of(object) else {
            return;
        };
        let entries = self.context_entries(entity);
        if entries.is_empty() {
            return;
        }
        let wire = entries
            .iter()
            .map(|(cliloc, _)| ContextMenuEntry {
                cliloc: *cliloc,
                flags: ContextMenuFlags::NONE,
            })
            .collect();
        let packet = ServerPacket::ContextMenu(ContextMenu {
            serial: object,
            entries: wire,
        });
        self.state.send_packet(connection, &packet);
    }

    /// Act on a context-menu choice (`0xBF` `0x15`): rebuild the object's entries
    /// and run the one at `index`. Rebuilding rather than remembering keeps this
    /// stateless — the entries are a pure function of the object, so a replay picks
    /// the same one.
    pub(super) fn context_menu_select(
        &mut self,
        connection: ConnectionId,
        serial: RawSerial,
        index: RawContextMenuIndex,
    ) {
        if !self.state.gameplay.context_menus {
            return;
        }
        let Some(actor) = self.state.players.get(&connection).copied() else {
            return;
        };
        let Some(object) = serial.validate() else {
            return;
        };
        let Some(entity) = self.state.registry.entity_of(object) else {
            return;
        };
        let entries = self.context_entries(entity);
        // "Is this one I offered": the tags are the rows numbered from zero, so
        // the list the object has *now* is the whole domain. Rebuilt rather than
        // remembered, which is what makes the answer a pure function of the
        // object — and what makes a menu that shrank between the draw and the
        // click refuse rather than run the wrong row.
        let index = match index.validate(entries.len()) {
            Ok(index) => index,
            Err(err) => {
                debug!(%err, %object, "context menu select refused");
                return;
            }
        };
        let (_, action) = entries[usize::from(index.0)];
        match action {
            ContextAction::Paperdoll => {
                items::paperdoll_request(&mut self.state, connection, object);
            }
            ContextAction::Open => {
                let _ = items::double_click(&mut self.state, connection, object);
            }
            ContextAction::Buy => {
                npc::open_shop(&mut self.state, connection, object);
            }
            ContextAction::Sell => {
                npc::offer_sell_list(&mut self.state, connection, actor);
            }
            ContextAction::QuestLog => {
                quests::open_log(&mut self.state, connection);
            }
        }
    }

    /// The default menu for an object — a container opens, a vendor buys and
    /// sells, any other mobile shows a paperdoll, everything else has no menu.
    /// Order is the tag order the client reports back on select.
    fn context_entries(&self, entity: EntityId) -> Vec<(ClilocId, ContextAction)> {
        if self.state.registry.has::<Container>(entity) {
            vec![(CLILOC_OPEN, ContextAction::Open)]
        } else if self.state.registry.has::<Vendor>(entity) {
            vec![
                (CLILOC_BUY, ContextAction::Buy),
                (CLILOC_SELL, ContextAction::Sell),
                (CLILOC_PAPERDOLL, ContextAction::Paperdoll),
            ]
        } else if self.state.registry.has::<Body>(entity) {
            let mut entries = vec![(CLILOC_PAPERDOLL, ContextAction::Paperdoll)];
            // Your own menu also offers the quest log. The paperdoll's Quest
            // button is the usual way in, but it is only drawn by a client whose
            // expansion set includes ML — this reaches the same window without
            // depending on that.
            if self.state.registry.has::<Client>(entity) {
                entries.push((CLILOC_QUEST_LOG, ContextAction::QuestLog));
            }
            entries
        } else {
            Vec::new()
        }
    }
}
