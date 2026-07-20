use super::*;

/// The cliloc numbers for the default context-menu entries. From ServUO's
/// `ContextMenuEntry` uses: 6123 "Open Paperdoll", 3000362 "Open", 6103 "Buy",
/// 6104 "Sell".
const CLILOC_PAPERDOLL: u32 = 6123;
const CLILOC_OPEN: u32 = 3_000_362;
const CLILOC_BUY: u32 = 6103;
const CLILOC_SELL: u32 = 6104;

/// What a chosen context-menu entry does. Every one routes to a handler a
/// double-click already reaches — the menu decides *what*, the existing rule does
/// *how*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContextAction {
    /// Open the object's paperdoll (a mobile).
    Paperdoll,
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
    pub(super) fn context_menu_request(&mut self, connection: ConnectionId, serial: u32) {
        if !self.state.gameplay.context_menus {
            return;
        }
        let Some(version) = self.client_version(connection) else {
            return;
        };
        // The new (0x02) popup format only; older clients want the 0x01 layout,
        // which is a later slice.
        if !version.supports(Feature::NewContextMenu) {
            return;
        }
        let Some(entity) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };
        let entries = self.context_entries(entity);
        if entries.is_empty() {
            return;
        }
        let wire: Vec<(u32, u16)> = entries.iter().map(|(cliloc, _)| (*cliloc, 0)).collect();
        let packet = encode_context_menu(serial, &wire);
        self.state.send(connection, packet);
    }

    /// Act on a context-menu choice (`0xBF` `0x15`): rebuild the object's entries
    /// and run the one at `index`. Rebuilding rather than remembering keeps this
    /// stateless — the entries are a pure function of the object, so a replay picks
    /// the same one.
    pub(super) fn context_menu_select(
        &mut self,
        connection: ConnectionId,
        serial: u32,
        index: u16,
    ) {
        if !self.state.gameplay.context_menus {
            return;
        }
        let Some(actor) = self.state.players.get(&connection).copied() else {
            return;
        };
        let Some(entity) = Serial::new(serial).and_then(|s| self.state.registry.entity_of(s))
        else {
            return;
        };
        let entries = self.context_entries(entity);
        let Some(&(_, action)) = entries.get(usize::from(index)) else {
            return;
        };
        match action {
            ContextAction::Paperdoll => {
                items::paperdoll_request(&mut self.state, connection, serial);
            }
            ContextAction::Open => {
                items::double_click(&mut self.state, connection, serial);
            }
            ContextAction::Buy => {
                npc::open_shop(&mut self.state, connection, serial);
            }
            ContextAction::Sell => {
                npc::offer_sell_list(&mut self.state, connection, actor);
            }
        }
    }

    /// The default menu for an object — a container opens, a vendor buys and
    /// sells, any other mobile shows a paperdoll, everything else has no menu.
    /// Order is the tag order the client reports back on select.
    fn context_entries(&self, entity: EntityId) -> Vec<(u32, ContextAction)> {
        if self.state.registry.has::<Container>(entity) {
            vec![(CLILOC_OPEN, ContextAction::Open)]
        } else if self.state.registry.has::<Vendor>(entity) {
            vec![
                (CLILOC_BUY, ContextAction::Buy),
                (CLILOC_SELL, ContextAction::Sell),
                (CLILOC_PAPERDOLL, ContextAction::Paperdoll),
            ]
        } else if self.state.registry.has::<Body>(entity) {
            vec![(CLILOC_PAPERDOLL, ContextAction::Paperdoll)]
        } else {
            Vec::new()
        }
    }

    /// The client version on a connection, if it is a player in the world.
    fn client_version(&self, connection: ConnectionId) -> Option<ClientVersion> {
        let entity = self.state.players.get(&connection).copied()?;
        self.state.registry.get::<Client>(entity).map(|c| c.version)
    }
}
