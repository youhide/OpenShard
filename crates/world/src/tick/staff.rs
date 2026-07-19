use super::*;

impl World {
    /// Act on a targeting cursor's answer. Looks up what the cursor was raised for
    /// and, if the click was not cancelled, does it. A cancel just clears the
    /// pending target.
    pub(super) fn handle_target(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::TargetResponse,
    ) {
        let Some(&actor) = self.state.players.get(&connection) else {
            return;
        };
        let Some(purpose) = self.state.pending_targets.remove(&actor) else {
            return; // no cursor was up for this mobile
        };
        if response.cancelled {
            return;
        }
        match purpose {
            openshard_state::TargetPurpose::Teleport => {
                crate::gm::teleport_to(&mut self.state, actor, response.location);
            }
        }
    }

    /// Act on an admin-gump button. The gump crate reads the response and gates it
    /// (game-master only); the acting — registering or clearing spawn regions,
    /// which only the tick can touch — is here.
    pub(super) fn handle_admin_gump(
        &mut self,
        connection: ConnectionId,
        response: openshard_protocol::GumpResponse,
    ) {
        let Some((actor, verb)) = crate::admin::button_action(&self.state, connection, &response)
        else {
            return;
        };
        // The engine holds no spawn data: it emits the verb, and the script pack —
        // where a shard's spawns are edited without a rebuild — decides what it
        // means, registering regions through `op_register_spawner` or clearing them.
        if let Some(serial) = self.state.registry.serial_of(actor) {
            self.state.bus.send(AdminMenuAction {
                serial,
                action: verb.to_owned(),
            });
        }
        gm::notify(&mut self.state, actor, &format!("Admin: {verb}."));
    }
}
