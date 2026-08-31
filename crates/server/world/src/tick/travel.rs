//! Marking a rune, and recalling to what it remembers.
//!
//! The two halves of one fact, and the reason they sit together: what Mark
//! writes is exactly what Recall reads, and a disagreement between them is a
//! rune that says Britain and lands you in a swamp.
//!
//! Both are effects of a spell that has already been paid for, so neither
//! charges anything and neither rolls anything — by the time these run the mana
//! is spent, the reagents are gone and the skill check has passed. What is left
//! is whether the *world* allows it, which is `magic::may_travel` plus the
//! handful of things ServUO checks at the moment of arrival.

use openshard_protocol::casting::SpellId;
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
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{
    Graphic,
    SoundId,
};
use openshard_state::components::{
    Combat,
    CriminalUntil,
    Position,
    RECALL_RUNE_GRAPHIC,
    RuneMark,
    Runebook,
    RunebookEntry,
};

use super::*;

impl World {
    /// The travel family's pre-cast refusals — ServUO's `Recall.CheckCast` and
    /// the identical block in `GateTravel`.
    ///
    /// Run before a single point of mana is spent, which is the whole reason
    /// they are here and not in the effect: a criminal who cannot escape should
    /// find that out for free, not for fourteen mana and three reagents.
    ///
    /// Mark is exempt from all of it — writing a rune is not fleeing.
    pub(super) fn travel_check_cast(
        &self,
        caster: EntityId,
        effect: magic::SpellEffect,
    ) -> Option<&'static str> {
        if !matches!(
            effect,
            magic::SpellEffect::Recall | magic::SpellEffect::GateTravel
        ) {
            return None;
        }
        if self.state.is_staff(caster) {
            return None;
        }
        // A thief does not vanish out of the town they just robbed.
        if self.state.registry.has::<CriminalUntil>(caster) {
            return Some("Thou'rt a criminal and cannot escape so easily.");
        }
        // Nor does anyone mid-fight. ServUO asks whether combat was *recent*; we
        // ask whether it is happening, which is the same question this engine can
        // answer — a `Combat` with a live target is a fight in progress.
        if self
            .state
            .registry
            .get::<Combat>(caster)
            .is_some_and(|combat| combat.target().is_some())
        {
            return Some("Wouldst thou flee during the heat of battle??");
        }
        if openshard_items::is_overloaded(&self.state, caster) {
            return Some("You are too encumbered to move.");
        }
        // Something on the cursor is in neither world once you leave.
        if self.state.row_of(caster).is_some_and(|row| row.held.is_some()) {
            return Some("You cannot teleport while dragging an object.");
        }
        None
    }

    /// Write the caster's own spot onto a recall rune — the Mark spell.
    pub(super) fn mark_rune(&mut self, caster: EntityId, target_serial: Option<Serial>) {
        let Some(rune) = target_serial.and_then(|serial| self.state.registry.entity_of(serial)) else {
            return;
        };
        // Only a rune. ServUO says so with 502357 for anything else, and the
        // graphic is not the test: an unmarked rune has no `RuneMark`, so what
        // makes a thing markable is that the engine gave it the graphic.
        if self
            .state
            .registry
            .get::<Drawn>(rune)
            .is_none_or(|graphic| graphic.id != RECALL_RUNE_GRAPHIC)
        {
            self.notify_self(caster, "You cannot mark that.");
            return;
        }
        // In your own pack, not merely within arm's reach — cliloc 1062422. A
        // rune on a shop floor is somebody else's.
        if !self.in_own_backpack(caster, rune) {
            self.notify_self(
                caster,
                "You must have this rune in your backpack in order to mark it.",
            );
            return;
        }
        let facet = self.state.facet_of(caster);
        let Some(&Position(at)) = self.state.registry.get::<Position>(caster) else {
            return;
        };
        // Mark has one end — where you are standing is where the rune points.
        if !magic::may_travel(&self.state, caster, magic::TravelKind::Mark, facet, at) {
            self.notify_self(caster, magic::TravelKind::Mark.refusal());
            return;
        }

        self.state.registry.insert(
            rune,
            RuneMark {
                facet,
                destination: at,
            },
        );
        // The rune's name *is* its description — ServUO keeps a string on the
        // item and so does this, rather than a second field saying the same
        // thing. A rune that reads "1495, 1629" in a pack of sixteen is a rune
        // nobody can find twice.
        //
        // It is also the first thing in the world whose name *changes*. Nothing
        // re-sends a tooltip mid-life yet (the roadmap records that as a gap), so
        // the client picks the new one up the next time it asks, and the spell's
        // own line says what was marked meanwhile.
        let name = magic::describe(&self.state, facet, at);
        self.state
            .registry
            .insert(rune, Name(format!("a recall rune ({name})")));
        self.notify_self(caster, &format!("You mark the rune: {name}."));
    }

    /// Take the caster to where a marked rune points — the Recall spell.
    pub(super) fn recall(&mut self, caster: EntityId, target_serial: Option<Serial>) {
        let Some(rune) = target_serial.and_then(|serial| self.state.registry.entity_of(serial)) else {
            return;
        };
        // Recall is the one of the pair that does *not* want the rune in your
        // pack — ServUO's target accepts any rune you can see, and a rune held
        // out by a friend is a classic way to be fetched. Reach is still the
        // server's to judge.
        if !openshard_items::in_reach(&self.state, rune, caster) {
            self.notify_self(caster, "That is too far away.");
            return;
        }
        let Some((facet, destination)) = magic::destination_of(&self.state, rune) else {
            self.notify_self(caster, "That rune is not yet marked.");
            return;
        };
        self.travel_to(caster, facet, destination, magic::TravelKind::RecallFrom);
    }

    /// The arrival half, shared by Recall and by anything else that takes
    /// somebody to a remembered spot.
    ///
    /// `kind` names the *departure*; the destination is checked as the matching
    /// arrival, so one call site cannot check one end and forget the other.
    pub(super) fn travel_to(
        &mut self,
        traveller: EntityId,
        facet: Facet,
        destination: Point,
        kind: magic::TravelKind,
    ) {
        let arriving = match kind {
            magic::TravelKind::RecallFrom => magic::TravelKind::RecallTo,
            magic::TravelKind::GateFrom => magic::TravelKind::GateTo,
            other => other,
        };
        let here = self.state.facet_of(traveller);

        // The classic rule, pre-AoS: a rune marked on another facet is a rune
        // you walk to. `cross_facet_travel` turns it into the behaviour from AoS
        // on. The machinery underneath works either way — this is a rule, not a
        // missing feature.
        if facet != here && !self.state.gameplay.cross_facet_travel {
            self.notify_self(traveller, "You cannot recall to another facet.");
            return;
        }
        if !self.state.facets.contains_key(&facet) {
            self.notify_self(traveller, arriving.refusal());
            return;
        }
        // Both ends, each against its own kind. Leaving is checked where the
        // traveller stands and arriving where they are going — the two are
        // different questions, and ServUO's `RecallFrom` row is the permissive
        // one precisely so a place you may not enter is still one you may leave.
        if let Some((here_facet, here)) = magic::standing_at(&self.state, traveller) {
            if !magic::may_travel(&self.state, traveller, kind, here_facet, here) {
                self.notify_self(traveller, kind.refusal());
                return;
            }
        }
        if !magic::may_travel(&self.state, traveller, arriving, facet, destination) {
            self.notify_self(traveller, arriving.refusal());
            return;
        }
        // Somewhere you could not have walked to is somewhere you may not arrive
        // — ServUO's `CanSpawnMobile`, cliloc 501025. Without it a rune marked
        // in a doorway that later grew a wall is a rune that buries you in it.
        if !self.can_stand_at(facet, destination) {
            self.notify_self(traveller, "Something is blocking the location.");
            return;
        }

        // ServUO plays the same sound at both ends: one for leaving, one for
        // arriving, and they are the whole of what an onlooker at either end
        // sees of a recall.
        self.state.play_sound(traveller, RECALL_SOUND);
        self.state.move_to(traveller, facet, destination);
        self.state.play_sound(traveller, RECALL_SOUND);
    }

    /// Whether a mobile could stand on this tile — the arrival test.
    fn can_stand_at(&self, facet: Facet, at: Point) -> bool {
        // A facet with no map is development mode, where every tile is allowed;
        // the same convention the step check uses. Asked of the map itself:
        // what this wanted to know was never whether some abstraction was
        // present, only whether there is ground to have an opinion about.
        if self.state.map_terrain(facet).is_none() {
            return true;
        }
        // ServUO's `Map.CanSpawnMobile`, which is `CanFit(x, y, z, 16)`: a body
        // fits at the height the traveller is *arriving at* — a surface under
        // its feet and nothing solid in it — rather than a surface being findable
        // somewhere near. A recall lands you at the rune's own z, so that is the
        // height the question has to be about.
        //
        // **And over the live world, which this claimed to do and did not.** It
        // read `MapTerrain::stand_z`, the bare map, so a wall the world put up
        // since the rune was marked did not count, a door shut across the spot
        // did not count, and a deck moored over open water did not count either —
        // a rune marked on a ship's deck was refused as "blocking" because the
        // map underneath it is sea.
        openshard_movement::can_fit(
            &self
                .state
                .footing(facet, openshard_map::overlay::Doors::AsTheyStand),
            Tile::new(at.x, at.y),
            i32::from(at.z),
            openshard_movement::PLAYER_HEIGHT,
        )
    }

    /// Whether `item` is inside the mobile's own backpack, at any depth.
    fn in_own_backpack(&self, mobile: EntityId, item: EntityId) -> bool {
        let Some(owner) = self.state.registry.serial_of(mobile) else {
            return false;
        };
        let Some(pack) = openshard_state::equipped_items(&self.state, owner)
            .find(|(_, worn)| worn.layer == items::BACKPACK_LAYER)
            .and_then(|(pack, _)| self.state.registry.serial_of(pack))
        else {
            return false;
        };
        // Walk out through the nesting: a rune in a pouch in the pack counts,
        // which is where anyone who owns sixteen of them keeps them.
        let mut container = match openshard_state::item_location(&self.state, item) {
            Some(LiveItemLocation::Settled(openshard_state::SettledItemLocation::Contained(held))) => {
                Some(held.container)
            }
            _ => None,
        };
        for _ in 0..MAX_CONTAINER_DEPTH {
            match container {
                Some(serial) if serial == pack => return true,
                Some(serial) => {
                    container = self.state.registry.entity_of(serial).and_then(|outer| {
                        match openshard_state::item_location(&self.state, outer) {
                            Some(LiveItemLocation::Settled(
                                openshard_state::SettledItemLocation::Contained(held),
                            )) => Some(held.container),
                            _ => None,
                        }
                    });
                }
                None => return false,
            }
        }
        false
    }
}

/// How deep the containment walk will go before giving up. A bound rather than a
/// cycle check: containment is a tree by construction, and a bound costs nothing
/// while a corrupted save that made a loop would otherwise hang the tick.
const MAX_CONTAINER_DEPTH: usize = 16;

/// ServUO's `Recall.cs` sound, played on departure and again on arrival.
const RECALL_SOUND: SoundId = SoundId(0x01FC);

/// The gump id the runebook is drawn under — its own, distinct from the quest
/// log's `0x0051_*`, the craft window's `0x0052_0001` and the moongate list's
/// `0x0053_0002`.
pub(super) const RUNEBOOK_GUMP: GumpId = openshard_protocol::gump::id::RUNEBOOK;

/// ServUO's `RunebookGump` button ids, kept verbatim.
///
/// The *encoding* has to be ServUO's exactly where a client echoes it back
/// against a layout built the same way; a scheme of one's own is a second thing
/// to get wrong for no gain. `75 + i` is Sacred Journey, which this engine does
/// not have — it is decoded and ignored rather than left to fall into another
/// range, so an unhandled press is a no-op and never a mis-read of a row.
const BOOK_USE_CHARGE: u32 = 10;
const BOOK_SACRED_JOURNEY: u32 = 75;
const BOOK_RECALL: u32 = 50;
const BOOK_GATE: u32 = 100;
const BOOK_DROP: u32 = 200;
const BOOK_DEFAULT: u32 = 300;

/// The button id a book row takes: its action's base plus the row.
///
/// The bases are an *encoding* and not ids of their own — no single button is
/// `BOOK_RECALL` — so they stay bare numbers and this is where the two become
/// a [`ButtonId`]. The inverse is the range ladder in `handle_runebook_gump`,
/// which has to read highest-first or `BOOK_USE_CHARGE + 40` decodes as a
/// Recall.
const fn book_button(base: u32, slot: u32) -> ButtonId {
    ButtonId(base + slot)
}

/// Recall's spell id, for the mana and reagents a book's paid buttons spend.
const RECALL_SPELL: SpellId = SpellId(31);
/// Gate Travel's.
const GATE_SPELL: SpellId = SpellId(51);

/// How long a book rests between openings — ServUO's `NextUse`, in ticks.
const BOOK_COOLDOWN: u64 = 2 * TICKS_PER_SECOND;

impl World {
    /// Open a runebook onto its owner's screen, if it is theirs to open and has
    /// had its moment to recharge.
    ///
    /// Returns whether the click was a runebook's, so the tick can stop before
    /// the ordinary item dispatch sees it.
    pub(super) fn click_runebook(&mut self, player: EntityId, book: EntityId) -> bool {
        if !self.state.registry.has::<Runebook>(book) {
            return false;
        }
        if !openshard_items::in_reach(&self.state, book, player) {
            return true;
        }
        let now = self.state.ticks;
        if self
            .state
            .registry
            .get::<Runebook>(book)
            .is_some_and(|owned| now < owned.next_use)
        {
            self.notify_self(player, "This book needs time to recharge.");
            return true;
        }
        if let Some(mut owned) = self.state.registry.get::<Runebook>(book).cloned() {
            owned.next_use = now + BOOK_COOLDOWN;
            self.state.registry.insert(book, owned);
        }
        self.draw_runebook(player, book);
        true
    }

    /// Draw the sixteen rows and what each may be asked to do.
    fn draw_runebook(&mut self, player: EntityId, book: EntityId) {
        let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(player) else {
            return;
        };
        let Some(owned) = self.state.registry.get::<Runebook>(book).cloned() else {
            return;
        };

        let mut layout = GumpLayout::default();
        layout.no_close();
        let rows = i32::try_from(owned.entries.len().max(1)).unwrap_or(1);
        layout.background(0, 0, 470, 90 + 26 * rows, 5054);
        layout.label(
            30,
            14,
            GUMP_WHITE,
            format!("Runebook — {} of {} charges", owned.charges, owned.max_charges),
        );
        if owned.entries.is_empty() {
            layout.label(30, 46, GUMP_WHITE, "This book is empty.");
        }
        for (index, entry) in owned.entries.iter().enumerate() {
            let row = i32::try_from(index).unwrap_or(0);
            let y = 46 + 26 * row;
            let slot = u32::try_from(index).unwrap_or(0);
            let marker = if owned.default_entry == Some(slot as u8) {
                "*"
            } else {
                " "
            };
            layout.label(26, y, GUMP_WHITE, format!("{marker} {}", entry.description));
            // A free charge, then the two paid casts, then the two housekeeping
            // buttons — the order ServUO draws them in.
            layout.button(
                250,
                y,
                0x00FA,
                0x00FB,
                GumpButton::Reply,
                0,
                book_button(BOOK_USE_CHARGE, slot),
            );
            let row = |base| book_button(base, slot);
            layout.button(280, y, 0x00FA, 0x00FB, GumpButton::Reply, 0, row(BOOK_RECALL));
            layout.button(310, y, 0x00FA, 0x00FB, GumpButton::Reply, 0, row(BOOK_GATE));
            layout.button(340, y, 0x00FA, 0x00FB, GumpButton::Reply, 0, row(BOOK_DEFAULT));
            layout.button(370, y, 0x00FA, 0x00FB, GumpButton::Reply, 0, row(BOOK_DROP));
        }
        layout.label(250, 30, GUMP_WHITE, "use  rec  gate  def  drop");

        let (text, lines) = layout.finish();
        // Close then draw: a client told to draw twice draws two windows.
        self.state.send_packet(
            connection,
            &ServerPacket::CloseGump(CloseGump {
                gump_id: RUNEBOOK_GUMP,
                button:  ButtonId::CLOSE_BOX,
            }),
        );
        self.state.send_packet(
            connection,
            &ServerPacket::GumpDisplay(GumpDisplay {
                serial:  self
                    .state
                    .registry
                    .serial_of(player)
                    .map_or(GumpKey::STANDALONE, GumpKey::on),
                gump_id: RUNEBOOK_GUMP,
                at:      GumpPoint::new(60, 60),
                layout:  text.to_owned(),
                lines:   lines.to_vec(),
            }),
        );
        if let Some(row) = self.state.row_of_mut(player) {
            row.runebook_gump = Some(book);
        }
    }

    /// Answer a runebook. Returns whether the reply was one of ours.
    pub(super) fn handle_runebook_gump(
        &mut self,
        connection: ConnectionId,
        response: &openshard_protocol::gump::GumpResponse,
    ) -> bool {
        if response.gump_id.validate(&[RUNEBOOK_GUMP]).is_none() {
            return false;
        }
        let Some(&player) = self.state.players.get(&connection) else {
            return true;
        };
        // Taken, not borrowed: a reply for a window this side never drew finds
        // nothing and does nothing.
        let Some(book) = self
            .state
            .row_of_mut(player)
            .and_then(|row| row.runebook_gump.take())
        else {
            return true;
        };

        // Reach is re-checked here and not only at the open: a window sits on
        // screen while its owner walks away.
        if !openshard_items::in_reach(&self.state, book, player) {
            self.notify_self(player, "That is too far away.");
            return true;
        }

        let GumpAnswer::Pressed(pressed) = response.button.interpret() else {
            return true; // the close box
        };
        let button = pressed.0;
        // Highest range first, or `BOOK_USE_CHARGE + 40` would read as a Recall.
        let (base, slot) = if button >= BOOK_DEFAULT {
            (BOOK_DEFAULT, button - BOOK_DEFAULT)
        } else if button >= BOOK_DROP {
            (BOOK_DROP, button - BOOK_DROP)
        } else if button >= BOOK_GATE {
            (BOOK_GATE, button - BOOK_GATE)
        } else if button >= BOOK_SACRED_JOURNEY {
            (BOOK_SACRED_JOURNEY, button - BOOK_SACRED_JOURNEY)
        } else if button >= BOOK_RECALL {
            (BOOK_RECALL, button - BOOK_RECALL)
        } else if button >= BOOK_USE_CHARGE {
            (BOOK_USE_CHARGE, button - BOOK_USE_CHARGE)
        } else {
            return true; // rename and the rest, unhandled
        };

        // An index the book does not hold is refused, not clamped — the
        // `CraftGump` rule, and the difference between doing nothing and taking
        // somebody to whatever happens to be in slot zero.
        let Some(entry) = self
            .state
            .registry
            .get::<Runebook>(book)
            .and_then(|owned| owned.entries.get(slot as usize).cloned())
        else {
            return true;
        };

        match base {
            BOOK_USE_CHARGE => self.spend_book_charge(player, book, &entry),
            BOOK_RECALL => self.cast_from_book(player, RECALL_SPELL, &entry),
            BOOK_GATE => self.cast_from_book(player, GATE_SPELL, &entry),
            BOOK_DEFAULT => {
                if let Some(mut owned) = self.state.registry.get::<Runebook>(book).cloned() {
                    owned.default_entry = u8::try_from(slot).ok();
                    self.state.registry.insert(book, owned);
                }
                self.notify_self(player, &format!("Default set to {}.", entry.description));
                self.draw_runebook(player, book);
            }
            BOOK_DROP => {
                if let Some(mut owned) = self.state.registry.get::<Runebook>(book).cloned() {
                    owned.entries.remove(slot as usize);
                    // The default is an index into a list that just shifted, so
                    // it is dropped rather than left pointing at a neighbour.
                    owned.default_entry = None;
                    self.state.registry.insert(book, owned);
                }
                self.notify_self(player, "You remove the entry.");
                self.draw_runebook(player, book);
            }
            _ => {}
        }
        true
    }

    /// The free travel a charge buys: no mana, no reagents, no skill roll — the
    /// charge *is* the cost, which is what makes a runebook worth carrying.
    fn spend_book_charge(&mut self, player: EntityId, book: EntityId, entry: &RunebookEntry) {
        let Some(mut owned) = self.state.registry.get::<Runebook>(book).cloned() else {
            return;
        };
        if owned.charges == 0 {
            self.notify_self(player, "This book has no charges left.");
            return;
        }
        if let Some(refusal) = self.travel_check_cast(player, magic::SpellEffect::Recall) {
            self.notify_self(player, refusal);
            return;
        }
        owned.charges -= 1;
        self.state.registry.insert(book, owned);
        self.travel_to(
            player,
            entry.facet,
            entry.destination,
            magic::TravelKind::RecallFrom,
        );
    }

    /// The paid buttons: mana, reagents and a Magery roll, exactly as the spell
    /// costs when cast at a rune — the book saves you the cursor, not the price.
    ///
    /// This does not go through `begin_cast`, and deliberately: that path exists
    /// to raise a cursor and wait for an answer, and the answer is already known
    /// here. What it *does* reuse is the one place cost is decided
    /// (`magic::pay_and_roll`), so a shard that turns reagents off or makes a
    /// fizzle free gets the same behaviour through a book as through a rune.
    fn cast_from_book(&mut self, player: EntityId, spell: SpellId, entry: &RunebookEntry) {
        let Some(info) = magic::info(spell) else {
            return;
        };
        if let Some(refusal) = self.travel_check_cast(player, info.effect) {
            self.notify_self(player, refusal);
            return;
        }
        if !self.caster_has_spell(player, spell) {
            self.notify_self(player, "That spell is not in your spellbook.");
            return;
        }
        let Some(serial) = self.state.registry.serial_of(player) else {
            return;
        };
        let reagents: Vec<(Graphic, u16)> = if self.state.gameplay.reagents {
            info.reagents.iter().map(|&graphic| (graphic, 1)).collect()
        } else {
            Vec::new()
        };
        let pack = self.caster_pack(serial);
        let (min_skill, max_skill) = magic::cast_skills(info);
        let mana_loss = self.state.gameplay.mana_loss_on_fail;
        let reagent_loss = self.state.gameplay.reagent_loss_on_fail;
        let Some(success) = magic::pay_and_roll(
            &mut self.state,
            player,
            magic::mana(info),
            openshard_skills::SkillBand::new(min_skill, max_skill),
            openshard_magic::MAGERY_SKILL,
            pack,
            &reagents,
            mana_loss,
            reagent_loss,
        ) else {
            self.notify_self(player, "You lack the mana or reagents to cast that.");
            return;
        };
        self.state.bus.send(magic::SpellCast {
            caster: player,
            serial,
            spell,
            target: None,
            success,
        });
        if !success {
            return;
        }
        match info.effect {
            magic::SpellEffect::GateTravel => {
                self.open_gate_to(player, entry.facet, entry.destination);
            }
            _ => {
                self.travel_to(
                    player,
                    entry.facet,
                    entry.destination,
                    magic::TravelKind::RecallFrom,
                )
            }
        }
    }
}
