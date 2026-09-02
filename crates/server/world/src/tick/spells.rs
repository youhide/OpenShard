//! The cast sequence: the wire and timing around a spell, over the core
//! spell table in `magic`.
//!
//! Two shapes, chosen by `gameplay.cast_style`:
//!
//! - **`Walk` (Sphere)** — a cast resolves the instant it is asked: mana and
//!   reagents are spent, the skill rolled, and the effect (or its target cursor)
//!   comes up at once, with no rooting. The caster keeps walking.
//! - **`Stop` (ServUO/UO)** — the caster is committed to a [`Casting`] over a
//!   cast delay; moving breaks it, and taking damage disturbs it when the shard
//!   runs `spell_disturb`. Only when the delay runs out does it resolve, and only
//!   then does a targeted spell raise its cursor.
//!
//! The *effect* is the engine's own: damage, heal and teleport for the spells it
//! runs today, and `Unimplemented` for the rest — those still cast, and then
//! nothing happens. This module never decides what a spell *does* beyond
//! dispatching on the table's archetype.

use openshard_magic::{
    MAGERY_SKILL,
    SpellEffect,
    SpellTarget,
};
use openshard_protocol::casting::SpellId;
use openshard_protocol::feedback::{
    EffectKind,
    GraphicalEffect,
    PlaySound,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::target::{
    TargetCursor,
    TargetKind,
};
use openshard_protocol::wire::{
    CursorId,
    Hue,
};
use openshard_state::components::{
    Casting,
    Skills,
};
use openshard_state::{
    CastStyle,
    TargetPurpose,
};

use super::*;

impl World {
    /// A client asked to cast a spell (`0xBF`). Begin it: right away in the
    /// Sphere style, or as a rooted [`Casting`] with a cast delay in the ServUO
    /// style. An unknown spell id or a dead caster is ignored.
    pub(super) fn begin_cast(&mut self, connection: ConnectionId, spell: SpellId) {
        let Some(&caster) = self.state.players.get(&connection) else {
            return;
        };
        let Some(info) = magic::info(spell) else {
            return; // past the eighth circle; not a spell
        };
        if self
            .state
            .registry
            .get::<Hitpoints>(caster)
            .is_some_and(|h| h.current == 0)
        {
            return; // the dead do not cast
        }
        // The classic gate: a spell is castable only if it is written in a
        // spellbook the caster carries. A scroll learned into the book, or the
        // full book the mage sells, is what puts it there.
        if !self.caster_has_spell(caster, spell) {
            self.notify_self(caster, "That spell is not in your spellbook.");
            return;
        }
        // The travel family's own refusals — ServUO's `CheckCast`, which runs
        // *before* the cast and so costs nothing. Escaping is the whole point of
        // Recall, and these are the four times you are not allowed to.
        if let Some(refusal) = self.travel_check_cast(caster, info.effect) {
            self.notify_self(caster, refusal);
            return;
        }
        // One cast at a time: a second request while a rooted one is held is not a
        // cast at all, so it is refused *before* the reveal below — ServUO's own
        // order, where `Spell.Cast` tests `m_Caster.Spell == null` first.
        if self.state.registry.has::<Casting>(caster) {
            return;
        }
        // Everything above this line refuses the cast for free; from here it is
        // begun, and beginning one is both a revealing and a disruptive act —
        // ServUO's `Spell.Cast` calls `RevealingAction` the moment the state turns
        // to `Casting`, and `RevealingAction` ends with a `DisruptiveAction`
        // ("anything that unhides you will also disrupt meditation"). So it is one
        // call here as it is one there: a hidden mage who casts is seen, and a
        // meditating one leaves the trance rather than casting out of it at twice
        // the regen rate.
        //
        // It happens in `begin_cast` and not at resolution because that is where
        // the two references put it: the reveal is the price of *trying*, and a
        // rooted ServUO-style cast must give the caster away for the seconds it is
        // held, not at the end of them.
        self.state.break_cover(caster);
        // And the words, in the same breath — ServUO's `Spell.Cast` calls
        // `SayMantra` on the line after `RevealingAction`, before any delay is
        // measured. They are said whether or not the cast then succeeds: the
        // point of power words is that everyone nearby hears what is coming, and
        // a mantra that waited for the spell to land would be a warning that
        // arrives with the fireball.
        self.say_mantra(caster, info.mantra);
        let gesture = match info.gesture {
            magic::CastGesture::Directed => openshard_state::Action::CastDirected,
            magic::CastGesture::Area => openshard_state::Action::CastArea,
        };
        match self.state.gameplay.cast_style {
            CastStyle::Walk => {
                // Nothing to wait out: the gesture and the effect are the same
                // instant, so the animation is given no duration to hold for.
                self.state.animate(caster, gesture);
                self.resolve_cast(caster, spell);
            }
            CastStyle::Stop => {
                let delay = magic::cast_delay_ticks(info, TICKS_PER_SECOND);
                // The gesture is held for as long as the cast is. ServUO restarts
                // the animation on a timer every 1.5s for the length of the delay;
                // this says the length once and lets the client keep the cycles
                // alive, which is the same picture without a second clock.
                self.state.animate_timed(caster, gesture, delay);
                self.state.registry.insert(
                    caster,
                    Casting {
                        spell,
                        complete_at: self.state.ticks + delay,
                    },
                );
            }
        }
    }

    /// Say a spell's power words over the caster's head, for everyone in earshot.
    ///
    /// ServUO's `Spell.SayMantra`, which is a `PublicOverheadMessage` in
    /// `MessageType.Spell` — ordinary speech as far as range and drawing go, told
    /// apart from a sentence only by the mode. It goes through `chat::speak` and
    /// not through a packet built here, so a mantra is heard by exactly whoever
    /// would hear the caster talk: a ghost's words reach the living no more than
    /// its speech does.
    ///
    /// Only a player ever gets here — [`Self::begin_cast`] is reached from a
    /// client's `0xBF` and nothing else — which is also ServUO's rule, where a
    /// creature keeps its mantra to itself unless it is built to show it.
    fn say_mantra(&mut self, caster: EntityId, mantra: &str) {
        chat::speak(
            &mut self.state,
            caster,
            openshard_protocol::speech::TalkMode::Spell,
            Hue::SPELL_WORDS,
            openshard_protocol::speech::Font::DEFAULT,
            mantra,
        );
    }

    /// Advance the ServUO-style casts once per tick: break any the caster took a
    /// disturbing blow to, then resolve those whose delay has run out.
    pub(super) fn advance_casts(&mut self) {
        // Disturb first, so a cast hit *and* due this tick is broken, not cast.
        if self.state.gameplay.spell_disturb {
            let hurt: Vec<EntityId> = self
                .state
                .bus
                .read(&mut self.disturbed)
                .map(|event| event.entity)
                .collect();
            for entity in hurt {
                if !self.state.registry.has::<Casting>(entity) {
                    continue;
                }
                // Protection holds concentration: roll the caster's chance (the
                // seeded tick generator, so it replays) and, on a pass, the blow
                // does not break the cast.
                if let Some(chance) = magic::behaviour_buff(
                    &self.state,
                    entity,
                    openshard_state::BehaviourBuffKind::PROTECTION,
                ) {
                    if self.state.rng.below(100) < chance.max(0) as u32 {
                        self.notify_self(entity, "Your protection holds your concentration.");
                        continue;
                    }
                }
                self.state.registry.remove::<Casting>(entity);
                self.notify_self(entity, "Your concentration is broken.");
            }
        }
        // Then the casts whose delay is up.
        let now = self.state.ticks;
        let ready: Vec<(EntityId, SpellId)> = self
            .state
            .registry
            .query::<Casting>()
            .filter(|(_, casting)| now >= casting.complete_at)
            .map(|(entity, casting)| (entity, casting.spell))
            .collect();
        for (caster, spell) in ready {
            self.state.registry.remove::<Casting>(caster);
            self.resolve_cast(caster, spell);
        }
    }

    /// Pay for a cast and roll it, then either land a self-cast now or raise the
    /// target cursor a targeted spell waits on. A fizzle (short mana or a
    /// reagent) says so and stops.
    fn resolve_cast(&mut self, caster: EntityId, spell: SpellId) {
        let Some(info) = magic::info(spell) else {
            return;
        };
        let Some(serial) = self.state.registry.serial_of(caster) else {
            return;
        };
        // Read the cost knobs first — a copy each, so the `&mut self.state` the
        // call takes does not clash with reading `self.state.gameplay`.
        let reagents_required = self.state.gameplay.reagents;
        let mana_loss_on_fail = self.state.gameplay.mana_loss_on_fail;
        let reagent_loss_on_fail = self.state.gameplay.reagent_loss_on_fail;
        // Reagents off means an empty list: nothing to check, nothing to consume.
        let reagents: Vec<(Graphic, u16)> = if reagents_required {
            info.reagents.iter().map(|&graphic| (graphic, 1)).collect()
        } else {
            Vec::new()
        };
        let pack = self.caster_pack(serial);
        let (min_skill, max_skill) = magic::cast_skills(info);
        let Some(success) = magic::pay_and_roll(
            &mut self.state,
            caster,
            magic::mana(info),
            openshard_skills::SkillBand::new(min_skill, max_skill),
            MAGERY_SKILL,
            pack,
            &reagents,
            mana_loss_on_fail,
            reagent_loss_on_fail,
        ) else {
            self.state.bus.send(magic::SpellCast {
                caster,
                serial,
                spell,
                target: None,
                success: false,
            });
            self.notify_self(caster, "You lack the mana or reagents to cast that.");
            return;
        };

        match info.target {
            SpellTarget::SelfCast => {
                // No cursor: it lands on the caster or the ground around them.
                self.state.bus.send(magic::SpellCast {
                    caster,
                    serial,
                    spell,
                    target: None,
                    success,
                });
                if success {
                    let at = self.caster_position(caster);
                    self.apply_spell_effect(caster, spell, None, at);
                }
            }
            SpellTarget::Mobile | SpellTarget::Location | SpellTarget::Item => {
                // Raise the cursor; the effect and the `SpellCast` wait for the
                // aim (see `handle_target`). A creature with no client cannot aim,
                // so its targeted cast simply lapses.
                //
                // An item-targeted spell raises the *object* cursor, so the client
                // itself refuses bare ground — "Select Marked item." wants a thing,
                // not a place. What comes back is still re-checked server-side.
                if let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(caster) {
                    let kind = if info.target == SpellTarget::Item {
                        TargetKind::Object
                    } else {
                        TargetKind::Location
                    };
                    self.state
                        .raise_target(caster, TargetPurpose::Spell { spell, success });
                    self.state.send_packet(
                        connection,
                        &ServerPacket::TargetCursor(TargetCursor {
                            cursor_id: CursorId(serial.raw()),
                            kind,
                        }),
                    );
                }
            }
        }
    }

    /// Run a spell's core effect on its aim. Called immediately for a self-cast,
    /// and from the target cursor's answer for a targeted one. `Unimplemented`
    /// archetypes do nothing here: they cast, and the effect is not built yet.
    pub(super) fn apply_spell_effect(
        &mut self,
        caster: EntityId,
        spell: SpellId,
        mut target_serial: Option<Serial>,
        mut target_location: Point,
    ) {
        let Some(info) = magic::info(spell) else {
            return;
        };
        let by = self.state.registry.serial_of(caster);
        // Magic Reflection: an offensive spell aimed at a mobile that carries the
        // reflect bounces back at its own caster, and the buff is spent. Redirect
        // before the feedback, so the bolt flies at whoever actually takes it.
        if matches!(
            info.effect,
            SpellEffect::Damage(..) | SpellEffect::Poison | SpellEffect::Paralyze
        ) {
            if let Some(target) = target_serial.and_then(|serial| self.state.registry.entity_of(serial)) {
                if magic::consume_behaviour_buff(
                    &mut self.state,
                    target,
                    openshard_state::BehaviourBuffKind::MAGIC_REFLECT,
                ) {
                    if let Some(caster_serial) = by {
                        target_serial = Some(caster_serial);
                        target_location = self.caster_position(caster);
                    }
                }
            }
        }
        // The sound and bolt/sparkle that make the cast land — before the effect,
        // so a target killed by the blow is still there for the bolt to fly at.
        self.spell_feedback(caster, target_serial, target_location, info.art);
        match info.effect {
            SpellEffect::Damage(kind, base) => {
                if let Some(target) = target_serial {
                    let base = self.resist_damage(caster, target, info.circle, base);
                    combat::damage(&mut self.state, target, base, kind, by);
                }
            }
            SpellEffect::AreaDamage(kind, base) => {
                // Centre on the caster for a self-cast (Earthquake), on the aimed
                // spot otherwise (Chain Lightning, Meteor Swarm).
                let centre = if matches!(info.target, SpellTarget::SelfCast) {
                    self.caster_position(caster)
                } else {
                    target_location
                };
                let facet = self.state.facet_of(caster);
                let victims: Vec<Serial> = self
                    .state
                    .facet_state(facet)
                    .sectors()
                    .mobiles_near(centre, magic::AREA_RADIUS)
                    .filter(|(entity, _)| *entity != caster)
                    .filter_map(|(entity, _)| self.state.registry.serial_of(entity))
                    .collect();
                for victim in victims {
                    // Every victim rolls its own resist: a blast that catches four
                    // people is four spells as far as the skill is concerned, which
                    // is ServUO's reading and the only one under which standing in
                    // an area spell trains the skill at all.
                    let base = self.resist_damage(caster, victim, info.circle, base);
                    combat::damage(&mut self.state, victim, base, kind, by);
                }
            }
            SpellEffect::Heal(amount) => {
                // A self-cast answered its own cursor and carries no mark, so it
                // mends the caster.
                if let Some(who) = target_serial.or(by) {
                    magic::heal(&mut self.state, who, amount);
                }
            }
            SpellEffect::Poison => {
                if let Some(target) = target_serial {
                    // The dose scales with the caster's Magery — a novice lands a
                    // lesser poison, a master a greater one (Poisoning, the
                    // deadlier levels, is a later skill).
                    let magery = self
                        .state
                        .registry
                        .get::<Skills>(caster)
                        .map_or(0, |s| s.get(MAGERY_SKILL));
                    let level = ((magery / 300) as u8).min(2);
                    let now = self.state.ticks;
                    combat::apply_poison(
                        &mut self.state,
                        target,
                        openshard_protocol::world::PoisonLevel::new(level),
                        now,
                    );
                }
            }
            SpellEffect::Cure => {
                if let Some(who) = target_serial.or(by) {
                    combat::cure_poison(&mut self.state, who);
                }
            }
            SpellEffect::AreaCure => {
                let facet = self.state.facet_of(caster);
                let healed: Vec<Serial> = self
                    .state
                    .facet_state(facet)
                    .sectors()
                    .mobiles_near(target_location, magic::AREA_RADIUS)
                    .filter_map(|(entity, _)| self.state.registry.serial_of(entity))
                    .collect();
                for mobile in healed {
                    combat::cure_poison(&mut self.state, mobile);
                }
            }
            SpellEffect::Teleport => {
                // The spell obeys the same region rule the staff `.tele` does —
                // one predicate, so a shard cannot bar one route and leave the
                // other open.
                if self.state.may_teleport(caster, target_location) {
                    self.state.teleport(caster, target_location);
                    self.state.broadcast_move(caster);
                } else {
                    self.notify_self(caster, "You cannot teleport from or to that place.");
                }
            }
            SpellEffect::StatMod(kind) => {
                // A Mobile-target spell, so it lands on the aimed mobile — or on
                // the caster for a self-cast that answered its own cursor.
                if let Some(who) = target_serial.or(by) {
                    let (offset, mut span) = self.stat_buff_terms(caster, kind);
                    // A curse can be resisted; a blessing is not something the
                    // blessed fights off. Only the debuff half of the family rolls,
                    // and what a resist buys is a shorter hold, not none.
                    if openshard_state::is_debuff(kind) && self.spell_resisted(caster, who, info.circle) {
                        span = magic::resisted(span);
                    }
                    let expires_at = self.state.ticks + span;
                    magic::apply_stat_buff(&mut self.state, who, kind, offset, expires_at);
                    self.refresh_status_of(who);
                }
            }
            SpellEffect::Resurrect => {
                // Aimed at either the ghost or its body. The core identifies the
                // body's owner and restores that original outfit before removing
                // the corpse.
                if let Some(target) = target_serial {
                    self.resurrect_target(target, false);
                }
            }
            SpellEffect::Mark => self.mark_rune(caster, target_serial),
            SpellEffect::Recall => self.recall(caster, target_serial),
            SpellEffect::GateTravel => self.open_gate_pair(caster, target_serial),
            SpellEffect::BehaviourBuff(kind) => {
                // Night Sight can land on another mobile; the self-cast trio
                // (Protection, Reactive Armor, Magic Reflection) answers its own
                // cursor and lands on the caster.
                if let Some(who) = target_serial.or(by) {
                    let (amount, expires_at) = self.behaviour_buff_terms(caster, kind);
                    magic::apply_behaviour_buff(&mut self.state, who, kind, amount, expires_at);
                    // Night Sight lights the buffed mobile's own screen. (Ambient
                    // is always daylight until a day/night cycle exists, so this is
                    // presently a no-op the moment one lands — it is sent correctly
                    // regardless.)
                    if kind == openshard_state::BehaviourBuffKind::NIGHT_SIGHT {
                        self.send_light(who, LIGHT_NIGHTSIGHT);
                    }
                }
            }
            SpellEffect::Field(kind) => {
                // Lay the row of tiles at the aimed spot; the tiles themselves are
                // the visual, so `spell_feedback` only voiced the cast.
                self.lay_field(caster, kind, target_location);
            }
            SpellEffect::Paralyze => {
                // A Mobile-target spell: it freezes the aimed mobile in place.
                if let Some(target) = target_serial {
                    // The Resisting-Spells cut the paralyze slice deferred: a
                    // resisted hold is three quarters as long, not absent. The
                    // *field* does not roll one — ServUO's `ParalyzeFieldSpell`
                    // freezes whoever crosses it outright — so the cut lives here
                    // and not in `paralyze_ticks`.
                    let mut span = self.paralyze_ticks(Some(caster));
                    if self.spell_resisted(caster, target, info.circle) {
                        span = magic::resisted(span);
                    }
                    let until = self.state.ticks + span;
                    magic::apply_paralyze(&mut self.state, target, until);
                }
            }
            SpellEffect::Unimplemented => {} // casts, and then nothing happens
        }
    }

    /// Play the art the spell table gives a spell, where it lands. Broadcast to
    /// everyone who can see the caster.
    ///
    /// This used to guess from the coarse [`SpellEffect`] — one fire bolt for
    /// every fire spell, one sparkle for every stat buff — and the guess is gone:
    /// [`SpellArt`](magic::SpellArt) is a row of the table, read straight off
    /// ServUO's own `PlaySound`/`FixedParticles` call for that spell. A
    /// [`Silent`](magic::SpellArt::Silent) row is a spell that has none, or one
    /// whose art belongs where its effect happens rather than here: Recall's two
    /// ends, a gate's pair, the rune Mark writes.
    ///
    /// The *placement* is still this function's: a bolt flies caster→mark, a
    /// sparkle sits on the mark, a fixed effect plants itself at the aimed spot.
    fn spell_feedback(
        &mut self,
        caster: EntityId,
        target_serial: Option<Serial>,
        target_location: Point,
        art: magic::SpellArt,
    ) {
        let magic::SpellArt::Landing { sound, visual } = art else {
            return;
        };
        let caster_serial = self.state.registry.serial_of(caster);
        let caster_pos = self.caster_position(caster);
        let target_pos = target_serial
            .and_then(|s| self.state.registry.entity_of(s))
            .and_then(|e| self.state.registry.get::<Position>(e).map(|p| p.0))
            // An area spell has no mark: it aims at a spot, not a mobile.
            .unwrap_or(target_location);
        // `Unseen` draws nothing at all: the field the spell lays is its own
        // picture, and a second one over the same tiles is two answers to one
        // question. It still sounds, which is why this is not an early return.
        let packet = match visual {
            magic::SpellVisual::Unseen => None,
            magic::SpellVisual::Bolt(art) => {
                Some(GraphicalEffect {
                    kind: EffectKind::Moving,
                    from: caster_serial,
                    to: target_serial,
                    art,
                    from_point: caster_pos,
                    to_point: target_pos,
                    speed: 7,
                    duration: 0,
                    fixed_direction: false,
                    explode: true,
                })
            }
            magic::SpellVisual::OnTarget(art) => {
                Some(GraphicalEffect {
                    kind: EffectKind::FixedFrom,
                    from: target_serial,
                    to: None,
                    art,
                    from_point: target_pos,
                    to_point: target_pos,
                    speed: 9,
                    duration: 20,
                    fixed_direction: true,
                    explode: false,
                })
            }
            magic::SpellVisual::AtSpot(art) => {
                Some(GraphicalEffect {
                    kind: EffectKind::FixedXyz,
                    from: None,
                    to: None,
                    art,
                    from_point: target_location,
                    to_point: target_location,
                    speed: 9,
                    duration: 20,
                    fixed_direction: true,
                    explode: false,
                })
            }
            // The strike carries no art id — the graphic is the client's own, and
            // the serial is what it strikes. With no mark (Chain Lightning aims at
            // ground) the coordinates are what it falls on.
            magic::SpellVisual::Lightning => {
                Some(GraphicalEffect {
                    kind:            EffectKind::Lightning,
                    from:            target_serial,
                    to:              None,
                    art:             Graphic(0),
                    from_point:      target_pos,
                    to_point:        target_pos,
                    speed:           0,
                    duration:        0,
                    fixed_direction: false,
                    explode:         false,
                })
            }
        };
        if let Some(packet) = packet {
            self.state.broadcast_packet(caster, &ServerPacket::Effect(packet));
        }
        // The sound at the point of the effect — target_pos is the aimed spot for
        // an area spell, which has no mark.
        self.state.broadcast_packet(
            caster,
            &ServerPacket::PlaySound(PlaySound {
                sound,
                at: target_pos,
            }),
        );
    }

    /// Whether `target` shrugs part of a `circle` spell off, and the line it reads
    /// when it does.
    ///
    /// The one door the Resisting-Spells roll goes through, so the skill trains from
    /// exactly the spells it softens and a caster cannot find a spell that skips it.
    /// The roll itself, and the gain it offers, are [`magic::check_resisted`]; what
    /// a resist is *worth* is the caller's, because a bolt loses damage and a curse
    /// loses seconds.
    fn spell_resisted(&mut self, caster: EntityId, target: Serial, circle: magic::SpellCircle) -> bool {
        let Some(target) = self.state.registry.entity_of(target) else {
            return false;
        };
        // A spell cast on yourself is not resisted: there is nobody to resist it,
        // and rolling would train the skill off a self-cast Curse.
        if target == caster {
            return false;
        }
        if !magic::check_resisted(&mut self.state, caster, target, circle) {
            return false;
        }
        self.state.localized_message(target, magic::RESISTED_MESSAGE, "");
        true
    }

    /// A spell's damage after the target's Resisting Spells has had its say — three
    /// quarters of it on a resist, all of it otherwise.
    fn resist_damage(
        &mut self,
        caster: EntityId,
        target: Serial,
        circle: magic::SpellCircle,
        base: u16,
    ) -> u16 {
        if self.spell_resisted(caster, target, circle) {
            magic::resisted(u64::from(base)) as u16
        } else {
            base
        }
    }

    /// How strong a stat buff the caster lands, and how long it holds, in ticks.
    ///
    /// Both scale from the caster's Magery, ServUO's shape: the magnitude rises to
    /// `+10` at grandmaster, the duration to a couple of minutes. A debuff kind
    /// takes the same magnitude with the sign flipped — the negation the `magic`
    /// crate then folds in and, later, backs out.
    ///
    /// A *span* and not the tick it lifts, because a resist shortens the hold: an
    /// absolute deadline cannot be cut by a quarter without first subtracting the
    /// present out of it again.
    fn stat_buff_terms(&self, caster: EntityId, kind: openshard_state::StatEffectKind) -> (i16, u64) {
        let magery = self
            .state
            .registry
            .get::<Skills>(caster)
            .map_or(0, |s| s.get(MAGERY_SKILL));
        let magnitude = (magery / 100).clamp(1, 10) as i16;
        let offset = if openshard_state::is_debuff(kind) {
            -magnitude
        } else {
            magnitude
        };
        let seconds = u64::from(magery / 10).clamp(10, 120);
        (offset, seconds * TICKS_PER_SECOND)
    }

    /// How strong a behaviour buff the caster lands, and the tick it lifts. The
    /// `amount` carries what the buff's decision point reads — a Protection chance,
    /// a Reactive Armor reflect percent — and is unused for the bare markers. All
    /// scale from the caster's Magery (in tenths, grandmaster `1000`), ServUO's
    /// classic pre-AoS shape approximated.
    fn behaviour_buff_terms(
        &self,
        caster: EntityId,
        kind: openshard_state::BehaviourBuffKind,
    ) -> (i16, openshard_state::WorldTick) {
        use openshard_state::BehaviourBuffKind;
        let magery = i32::from(
            self.state
                .registry
                .get::<Skills>(caster)
                .map_or(0, |s| s.get(MAGERY_SKILL)),
        );
        let (amount, seconds): (i16, u64) = match kind {
            // Night Sight: a marker; 15–25 minutes, Magery-scaled.
            BehaviourBuffKind::NIGHT_SIGHT => (0, (900 + (magery * 6 / 10) as u64).clamp(900, 1500)),
            // Protection: the chance a blow does not break a cast, capped 75%.
            BehaviourBuffKind::PROTECTION => {
                (
                    (magery * 75 / 1000).clamp(0, 75) as i16,
                    (magery / 5).clamp(15, 240) as u64,
                )
            }
            // Reactive Armor: the percent of a melee blow bounced back, capped 50%.
            BehaviourBuffKind::REACTIVE_ARMOR => {
                (
                    (magery * 50 / 1000).clamp(5, 50) as i16,
                    (magery / 5).clamp(15, 240) as u64,
                )
            }
            // Magic Reflection: a marker, spent on the first bounce; the timer is a
            // fallback if no spell arrives.
            _ => (0, (magery / 5).clamp(15, 240) as u64),
        };
        (amount, self.state.ticks + seconds * TICKS_PER_SECOND)
    }

    /// How long a paralysis this caster lands holds, in ticks — ServUO's pre-AoS
    /// `7 + Magery*0.2` seconds (grandmaster `1000` tenths → 27s). A missing caster
    /// (a field whose caster has gone) falls to the 7-second floor.
    ///
    /// A span rather than a deadline for the same reason [`Self::stat_buff_terms`]
    /// gives one: the Paralyze spell cuts it on a resist, and a quarter cannot be
    /// taken off a tick that already has the present folded into it.
    fn paralyze_ticks(&self, caster: Option<EntityId>) -> u64 {
        let magery = caster
            .and_then(|c| self.state.registry.get::<Skills>(c))
            .map_or(0, |s| s.get(MAGERY_SKILL));
        let seconds = 7 + u64::from(magery) / 50;
        seconds * TICKS_PER_SECOND
    }

    /// The tick a paralysis this caster lands would lift. What a Paralyze Field's
    /// pulse wants: it freezes whoever it catches outright, with no resist roll of
    /// its own (ServUO's `ParalyzeFieldSpell` has none), so it asks for the deadline
    /// and not the span.
    pub(super) fn paralyze_until(&self, caster: Option<EntityId>) -> openshard_state::WorldTick {
        self.state.ticks + self.paralyze_ticks(caster)
    }

    /// Send a mobile its personal light level, if it has a client — the seam Night
    /// Sight lights and its expiry restores. A creature (no `Client`) is a no-op.
    pub(super) fn send_light(&mut self, serial: Serial, level: Light) {
        let Some(entity) = self.state.registry.entity_of(serial) else {
            return;
        };
        if let Some(&Client { connection, .. }) = self.state.registry.get::<Client>(entity) {
            self.state
                .send_packet(connection, &ServerPacket::LightLevel(LightLevel { level }));
        }
    }

    /// Whether the caster carries a spellbook that holds `spell` — a book in its
    /// backpack with the spell's bit set. The gate `begin_cast` reads.
    pub(super) fn caster_has_spell(&self, caster: EntityId, spell: SpellId) -> bool {
        let Some(serial) = self.state.registry.serial_of(caster) else {
            return false;
        };
        let Some(pack) = self.caster_pack(serial) else {
            return false;
        };
        self.state.registry.query::<Spellbook>().any(|(book, mask)| {
            mask.has(spell)
                && matches!(
                    openshard_state::item_location(&self.state, book),
                    Some(LiveItemLocation::Settled(
                        openshard_state::SettledItemLocation::Contained(c)
                    )) if c.container == pack
                )
        })
    }

    /// The backpack reagents come out of, or `None` if the caster wears no pack.
    pub(super) fn caster_pack(&self, caster: Serial) -> Option<Serial> {
        openshard_state::equipped_items(&self.state, caster)
            .find(|(_, worn)| worn.layer == items::BACKPACK_LAYER)
            .and_then(|(item, _)| self.state.registry.serial_of(item))
    }

    /// Where a caster stands, or the origin if it somehow has no position.
    fn caster_position(&self, caster: EntityId) -> Point {
        self.state
            .registry
            .get::<Position>(caster)
            .map_or(Point::new(0, 0, 0), |p| p.0)
    }

    /// A private system line to a player, if it is one. A creature hears nothing.
    pub(super) fn notify_self(&mut self, entity: EntityId, text: &str) {
        self.state.system_message(entity, text);
    }
}
