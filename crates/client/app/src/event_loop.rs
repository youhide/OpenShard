//! `winit` glue: the `ApplicationHandler` impl that dispatches a platform
//! event into whichever subsystem answers to it — a step to `ui_command.rs`,
//! a packet to `net_command.rs`, a press on a window to `own_windows.rs`, a
//! redraw to `presentation.rs`. Nothing here decides gameplay on its own; it
//! is the seam between what winit reports and the method that already knows
//! what to do with it.

use std::time::{
    Duration,
    Instant,
};

use openshard_client_render::camera::RealPixel;
use openshard_client_render::gump::GumpPixel;
use openshard_movement::Bodies;
use winit::application::ApplicationHandler;
use winit::event::{
    ElementState,
    WindowEvent,
};
use winit::event_loop::{
    ActiveEventLoop,
    ControlFlow,
};
use winit::keyboard::{
    KeyCode,
    PhysicalKey,
};
use winit::window::WindowId;

use crate::app::App;
use crate::picking::SelectedIdentity;
use crate::world::{
    footing,
    guide,
};
use crate::{
    DOUBLE_CLICK,
    PAGE_PIXELS,
    desk,
    keyboard,
    keys,
    panes,
    shell,
    steer,
};

/// How far `App::last_advance` may lag behind the redraw cadence before
/// `about_to_wait` stops trusting `App::watched` and ticks the state clock
/// directly. Well above a healthy watched frame's gap (a redraw interval of
/// at most [`FRAME_DELAY`](openshard_client_render::animation::FRAME_DELAY),
/// 80ms) so ordinary vsync jitter never trips it, and well below "the player
/// would notice" so a stalled draw loop costs a fraction of a second rather
/// than lasting until the next `Focused`/`Occluded`/input event happens to
/// arrive.
const STALLED_DRAW_TOLERANCE: Duration = Duration::from_millis(250);

impl App {
    /// Send every movement step which is due now.
    ///
    /// The caller is the state clock rather than a redraw callback: a route is
    /// an order already given to the client, so it must keep advancing while a
    /// desktop compositor has stopped painting this window.
    pub(crate) fn advance_walk(&mut self, now: Instant) {
        for _ in 0..2 {
            // Both halves of the ground, because this is where a destination
            // replans: see `steer::Readings`. Built here rather than held, for
            // the reason the single terrain always was — they borrow the map
            // and the crowd, and the walk borrows `steer` mutably beside them.
            let ground = steer::Readings {
                live:   footing(&self.resources, self.walking_doors())
                    .among(Bodies::standing(&self.world.bodies)),
                guide:  guide(&self.resources),
                coarse: self.resources.coarse.as_ref(),
            };
            let motion = self.world.motion.planning_state();
            let Some(facing) = self
                .steer
                .due(now, motion.position, motion.facing.direction, ground)
            else {
                break;
            };
            self.walk(facing);
        }
    }

    /// A key the speech line owns. Answers whether the picture changed.
    ///
    /// Every arm is a call into [`crate::chat::Chat`] and nothing here decides
    /// what a key means — that is [`keyboard::Edit`], which is a table with
    /// tests rather than a `match` inside an event handler. What is left is the
    /// one thing the table cannot answer: whether a key that is not a binding
    /// carried any *text*, which is what a keyboard layout and an input method
    /// speak through.
    fn speech_key(&mut self, code: KeyCode, text: Option<&str>) -> bool {
        // What the shard says this character may command, read fresh at every
        // keystroke rather than kept on the chat — see `chat::Chat::refresh`. A
        // client with no view yet has nobody's authority, which is the same
        // answer as an ordinary player's and the safe direction to guess in.
        let authority = self.authority();
        let Some(edit) = keyboard::Edit::of(code, self.input.shift_held, self.input.ctrl_held) else {
            let Some(text) = text else {
                return false;
            };
            self.chat.insert(text, authority);
            return true;
        };
        match edit {
            keyboard::Edit::Submit => {
                if let Some(line) = self.chat.take() {
                    self.say(line);
                }
            }
            keyboard::Edit::Cancel => {
                self.chat.cancel(authority);
            }
            keyboard::Edit::Complete => return self.chat.complete(authority),
            keyboard::Edit::NextChannel => self.chat.channel = self.chat.channel.next(),
            keyboard::Edit::NextCandidate => return self.chat.highlight_next(),
            keyboard::Edit::PreviousCandidate => return self.chat.highlight_previous(),
            keyboard::Edit::Backspace => self.chat.backspace(authority),
            keyboard::Edit::BackspaceWord => self.chat.backspace_word(authority),
            keyboard::Edit::Delete => self.chat.delete(authority),
            keyboard::Edit::Left => self.chat.left(),
            keyboard::Edit::Right => self.chat.right(),
            keyboard::Edit::Start => self.chat.cursor = 0,
            keyboard::Edit::End => self.chat.cursor = self.chat.typed.len(),
        }
        true
    }
}

impl ApplicationHandler<()> for App {
    /// The shard thread staged one or more updates for us.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, (): ()) {
        // The crowd's clock first, and before the packet is folded in. A step is
        // timestamped with `Crowd`'s own `now` — that is what the *next* step's
        // crossing is measured against (`crowd::glide_time`) — and this handler
        // used to fold packets in between two `advance` calls, so every step was
        // recorded at the previous frame's instant: up to 16ms in the past
        // mid-walk and up to a whole `FRAME_DELAY` for a body that had stopped.
        // The measurement is a difference of two of those, so the error lands on
        // the crossing *length*: the walk oracle in `dst.rs` caught a tile after
        // a turn taking 416ms instead of 400, which is a body a frame behind
        // itself and then yanked forward.
        let mut changed = false;
        for update in self.updates.take() {
            if !self.on_update(update) {
                return;
            }
            changed = true;
            // The initial world snapshot is the earliest point at which the
            // shard can send the live view. Stalling any earlier only freezes
            // the login conversation, leaving no world stream to exercise the
            // ordered mailbox against.
            if self.world.authoritative.view.is_some() {
                if let Some(stall) = self.stall_on_update.take() {
                    tracing::warn!(?stall, "stalling the App event loop after entering the world");
                    std::thread::sleep(stall);
                }
            }
        }
        if changed {
            if self.watched() {
                self.ask_redraw();
            } else {
                // Receiving a packet is itself a state-clock wake.  In
                // particular, `fold_incoming` can start or replace weather
                // ambience and music, whose next turns belong to
                // `Audio::advance` in `tick`.  A minimised compositor is free
                // to defer redraw callbacks (and, on some platforms, timer
                // wakes), so waiting for one made those sounds appear only
                // when the window was restored.
                self.tick(Instant::now());
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        match self.create_window(event_loop) {
            Ok(window) => {
                self.window = Some(window);
                self.begin_opening_scenario();
            }
            Err(error) => {
                eprintln!("{error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Escape puts away an armed map-editor tool before egui can consume the
        // key for one of the editor's controls. Chat and modal panes retain
        // their established Escape behaviour: they own the keyboard first.
        if matches!(
            &event,
            WindowEvent::KeyboardInput {
                event: winit::event::KeyEvent {
                    physical_key: PhysicalKey::Code(KeyCode::Escape),
                    state: ElementState::Pressed,
                    ..
                },
                ..
            }
        ) && !self.chat.focused
            && self.keyboard_window().is_none()
            && self.map_editor.active()
            && self.map_editor.cancel_tool()
        {
            self.ask_redraw();
            return;
        }
        // The UI sees everything first, and what it takes reaches neither the
        // camera nor the walk keys — otherwise a drag inside a panel pans the
        // world underneath it. egui never claims a close or a resize, so
        // returning here cannot swallow one.
        let consumed = match (self.shell.as_mut(), self.window.as_ref()) {
            (Some(shell), Some(screen)) => shell.on_window_event(&screen.window, &event),
            _ => false,
        };
        if consumed {
            // A key the UI took is a key this will never hear come up, and a
            // held direction that is never released walks for ever. Typing into
            // a panel should stop the character anyway, so letting go of
            // everything is both the fix and the behaviour.
            if matches!(event, WindowEvent::KeyboardInput { .. }) {
                self.steer.clear();
                self.input.aiming = false;
                self.set_war_mode_held(false);
            }
            // And the same argument one device over: a window taken hold of by
            // a press this end *did* hear, let go over a panel egui claims, is
            // a hold nothing ever ends — the button is up and the window still
            // follows the pointer around. The UI is welcome to the click; what
            // it must not do is leave this end believing the button is down.
            // See `windows::WindowGrip`, which is where the rest of that
            // gesture lives.
            if matches!(
                event,
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: winit::event::MouseButton::Left,
                    ..
                }
            ) {
                self.windows.grip.release();
                self.input.left_press = None;
            }
            self.ask_redraw();
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(window) = self.window.as_mut() {
                    window.config.width = size.width.max(1);
                    window.config.height = size.height.max(1);
                    window.surface.configure(&window.device, &window.config);
                    self.control.resize(window.config.width, window.config.height);
                }
                // The world texture and the depth buffer follow the
                // *camera's* size and not the window's, which are the same
                // thing only at zoom 1. `draw` resizes them together.
                self.ask_redraw();
            }
            WindowEvent::RedrawRequested => self.draw(),
            // Everything past this line asks the world a question — which tile
            // is under the cursor, which way to walk, what is at the end of a
            // route — and until the facet arrives there is no world to ask. See
            // `Resources::grounded`: this is one of the two doors it is checked
            // at, and the three arms above it are the *window's* own events
            // rather than the world's, so a client can still be resized,
            // redrawn and closed while its ground is on its way.
            //
            // Only [`WorldSource::Shard`](crate::WorldSource::Shard) ever
            // reaches this: under the other two arms the facet is in hand before
            // the window exists.
            _ if !self.grounded() => {}
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // Whose key this is, asked once and in one place — see
                // `keyboard::Owner`, which is also where the fourth owner (egui,
                // answered above by `Shell::on_window_event`) is written down.
                let owner = keyboard::Owner::of(self.chat.focused, self.keyboard_window().is_some());
                // The speech line, ahead of every hotkey and every walk key:
                // once it has the keyboard, a letter is a letter typed and not
                // a body walked. Arrows move the caret here rather than the
                // player, which is why this comes before `direction_of` below
                // and not after it, and Escape closes the line rather than
                // reaching the `KeyCode::Escape` arm further down that quits
                // the client.
                if owner == keyboard::Owner::Speech {
                    if event.state == ElementState::Pressed && self.speech_key(code, event.text.as_deref()) {
                        self.ask_redraw();
                    }
                    return;
                }
                // A `{ textentry }` the player has clicked into, on the same
                // terms and ahead of the same keys: while a field has the
                // keyboard, a letter is a letter typed. Below the speech line
                // rather than above it because the two cannot both be focused —
                // Enter opens one and a click opens the other — and the order
                // only decides which is asked first.
                //
                // Escape gives the keyboard back rather than quitting the
                // client, which is what the arm further down would do.
                if owner == keyboard::Owner::Pane {
                    if event.state == ElementState::Pressed {
                        // Which physical key means which of the three things a
                        // box can be told is decided here and nowhere else — see
                        // `panes::Key`, whose three arms are what a window
                        // answers. A character at a time, in the order the
                        // keyboard produced them, so that an input method that
                        // hands over two at once appends both.
                        let mut answer = panes::Response::ignored();
                        match code {
                            KeyCode::Enter | KeyCode::NumpadEnter => {
                                answer.absorb(self.deliver(panes::Input::Key(panes::Key::Done)));
                            }
                            // **Not the same key as Enter**, though a
                            // `{ textentry }` answers both by putting itself
                            // down: a modal is where "done" and "never mind"
                            // are opposite answers, and the amount picker sends
                            // its number on one and dismisses the press on the
                            // other. See `panes::Key::Cancel`.
                            KeyCode::Escape => {
                                answer.absorb(self.deliver(panes::Input::Key(panes::Key::Cancel)));
                            }
                            KeyCode::Backspace => {
                                answer.absorb(self.deliver(panes::Input::Key(panes::Key::Backspace)));
                            }
                            _ => {
                                for character in event.text.as_deref().unwrap_or_default().chars() {
                                    answer.absorb(
                                        self.deliver(panes::Input::Key(panes::Key::Typed(character))),
                                    );
                                }
                            }
                        }
                        // A key that changed nothing asks for no frame — the
                        // wheel's lesson on the other input: a control character
                        // the field refused is not a picture that moved.
                        if answer.redraw {
                            self.ask_redraw();
                        }
                    }
                    return;
                }
                // Tab toggles war mode on its first press. Handle both states
                // before the pressed-only hotkeys below so an operating-system
                // repeat cannot act as another press; release merely resets
                // the remembered key state.
                //
                // This arm is reached at all only because egui is never handed a
                // `Tab` — see `keyboard::egui_may_see`. It used to be reached
                // exactly once per launch: the first press entered war mode and
                // gave an egui widget the focus, and from the next frame egui
                // claimed every key.
                if code == KeyCode::Tab {
                    self.set_war_mode_held(event.state == ElementState::Pressed);
                    return;
                }
                // An arrow is *held*, not pressed: while it is down a step is
                // due every step's length, and that clock is ours rather than
                // the operating system's repeat rate. See `keys.rs`.
                if let Some(direction) = keys::Held::direction_of(code) {
                    let step = match event.state {
                        ElementState::Pressed => {
                            let motion = self.world.motion.planning_state();
                            self.steer.press(
                                direction,
                                motion.position,
                                Instant::now(),
                                motion.facing.direction,
                                steer::Readings {
                                    // An enabled auto-door mode turns a shut
                                    // leaf into a usable next step; `walk`
                                    // sends its use before this step.
                                    //
                                    // The bodies in the way of this press. A
                                    // held arrow never plans, but it does ask
                                    // `Detour` for a way past whatever is ahead
                                    // — and somebody standing there is one of
                                    // the things it has to get past.
                                    live:   footing(&self.resources, self.walking_doors())
                                        .among(Bodies::standing(&self.world.bodies)),
                                    guide:  guide(&self.resources),
                                    coarse: self.resources.coarse.as_ref(),
                                },
                            )
                        }
                        ElementState::Released => {
                            self.steer.release(direction);
                            None
                        }
                    };
                    if let Some(facing) = step {
                        if self.walk(facing) {
                            self.ask_redraw();
                        }
                    }
                    return;
                }
                if event.state != ElementState::Pressed {
                    return;
                }
                // Escape takes the topmost window down and does *not* quit the
                // client. Quitting on it was this client's own invention — no
                // reference client does it — and it cost more than a keystroke:
                // a window whose right button is eaten by a panel over it (see
                // [`App::close_top_window`]) had no way out at all, because the
                // one key a player reaches for closed the whole session
                // instead. Leaving is `CloseRequested`, which is the window
                // manager's close box and every other application's answer.
                if code == KeyCode::Escape {
                    if self.close_top_window() {
                        self.ask_redraw();
                    }
                    return;
                }
                // Every remaining key is a *binding*, looked up in one table
                // rather than matched arm by arm here — see `keyboard::Hotkey`,
                // which is also where each of them is written down. What is left
                // below is only the doing: an arm that names its hotkey and
                // answers whether the picture changed.
                //
                // A key with no binding asks for nothing, which is what the
                // `match` this replaced said with a `_ => false` arm.
                let Some(hotkey) = keyboard::Hotkey::of(keyboard::Gesture::new(code, self.input.ctrl_held))
                else {
                    return;
                };
                let changed = match hotkey {
                    keyboard::Hotkey::DevWindow => {
                        // The shell's `Desk` and not this one: see
                        // `Shell::toggle_dev`. Before there is a shell there is no
                        // window either, so this arm cannot be reached without one.
                        let authority = self.authority();
                        if let Some(shell) = self.shell.as_mut() {
                            shell.toggle_dev(authority);
                        }
                        true
                    }
                    // `steer.clear()` for the reason the UI-consumed path above
                    // does it: an arrow held down when this fires would otherwise
                    // never see its `Released` and walk for ever, since arrows
                    // move the caret and not the body once `chat.focused` is true.
                    keyboard::Hotkey::Speak => {
                        self.chat.focused = true;
                        self.steer.clear();
                        self.set_war_mode_held(false);
                        true
                    }
                    keyboard::Hotkey::Relock => {
                        self.relock();
                        true
                    }
                    keyboard::Hotkey::Paperdoll => {
                        self.open_own_paperdoll();
                        true
                    }
                    keyboard::Hotkey::Inventory => {
                        self.open_own_inventory();
                        true
                    }
                    keyboard::Hotkey::UseLastItem => {
                        self.use_last_item();
                        false
                    }
                    keyboard::Hotkey::CraftCatalogue => {
                        if let Some(link) = self.world.shard.link() {
                            link.act(openshard_client_net::action::Outgoing::OpenCraftCatalogue);
                        }
                        false
                    }
                    keyboard::Hotkey::HouseInventory => {
                        if let Some(shell) = self.shell.as_mut() {
                            shell.toggle_house_inventory();
                        }
                        true
                    }
                    keyboard::Hotkey::Minimap => {
                        if let Some(open) = self
                            .windows
                            .own_windows
                            .iter_mut()
                            .find(|open| open.subject == crate::windows::WindowSubject::Minimap)
                        {
                            if let crate::panes::AnyPane::Minimap(pane) = &mut open.pane {
                                pane.toggle_size();
                            }
                        } else {
                            crate::windows::open_local_window(
                                &mut self.windows.own_windows,
                                crate::windows::WindowSubject::Minimap,
                            );
                        }
                        true
                    }
                    keyboard::Hotkey::WorldMap => {
                        crate::windows::open_local_window(
                            &mut self.windows.own_windows,
                            crate::panes::LocalWindow::WorldMap.subject(),
                        );
                        true
                    }
                    keyboard::Hotkey::PanUp => self.control.pan(0, PAGE_PIXELS),
                    keyboard::Hotkey::PanDown => self.control.pan(0, -PAGE_PIXELS),
                    keyboard::Hotkey::FloorUp => {
                        if self.graphics.z_slice {
                            let (lower, upper) = match self.graphics.z_slice_view {
                                openshard_client_render::interiors::ZSliceView::Auto => {
                                    let lower = self.world.presentation.cutaway_at.z;
                                    (lower, lower.saturating_add(20))
                                }
                                openshard_client_render::interiors::ZSliceView::Manual { lower, upper } => {
                                    (lower, upper)
                                }
                            };
                            self.graphics.z_slice_view =
                                openshard_client_render::interiors::ZSliceView::Manual {
                                    lower: lower.saturating_add(20),
                                    upper: upper.saturating_add(20),
                                };
                            true
                        } else {
                            let relative = match self.graphics.floor_view {
                                openshard_client_render::interiors::FloorView::Auto => 1,
                                openshard_client_render::interiors::FloorView::Manual { relative } => {
                                    relative.saturating_add(1)
                                }
                            };
                            self.graphics.floor_view =
                                openshard_client_render::interiors::FloorView::Manual { relative };
                            true
                        }
                    }
                    keyboard::Hotkey::FloorDown => {
                        if self.graphics.z_slice {
                            let (lower, upper) = match self.graphics.z_slice_view {
                                openshard_client_render::interiors::ZSliceView::Auto => {
                                    let lower = self.world.presentation.cutaway_at.z;
                                    (lower, lower.saturating_add(20))
                                }
                                openshard_client_render::interiors::ZSliceView::Manual { lower, upper } => {
                                    (lower, upper)
                                }
                            };
                            self.graphics.z_slice_view =
                                openshard_client_render::interiors::ZSliceView::Manual {
                                    lower: lower.saturating_sub(20),
                                    upper: upper.saturating_sub(20),
                                };
                            true
                        } else {
                            let relative = match self.graphics.floor_view {
                                openshard_client_render::interiors::FloorView::Auto => -1,
                                openshard_client_render::interiors::FloorView::Manual { relative } => {
                                    relative.saturating_sub(1)
                                }
                            };
                            self.graphics.floor_view =
                                openshard_client_render::interiors::FloorView::Manual { relative };
                            true
                        }
                    }
                    keyboard::Hotkey::SpeechProbe => {
                        self.say("AbCdEfGh The Quick Brown Fox 123".to_owned());
                        false
                    }
                    keyboard::Hotkey::Night => {
                        self.graphics.night = !self.graphics.night;
                        true
                    }
                    keyboard::Hotkey::Sunlight => {
                        self.graphics.sunlit = !self.graphics.sunlit;
                        true
                    }
                    keyboard::Hotkey::SkyField => {
                        self.graphics.sky_field = !self.graphics.sky_field;
                        true
                    }
                    keyboard::Hotkey::Lantern => {
                        self.graphics.lantern = !self.graphics.lantern;
                        true
                    }
                    keyboard::Hotkey::Solids => {
                        self.graphics.show_solids = !self.graphics.show_solids;
                        true
                    }
                    keyboard::Hotkey::SolidsOnly => {
                        self.graphics.solids_only = !self.graphics.solids_only;
                        true
                    }
                    keyboard::Hotkey::SolidsEverything => {
                        self.graphics.solids_everything = !self.graphics.solids_everything;
                        true
                    }
                    keyboard::Hotkey::LightView => {
                        self.graphics.light_view = self.graphics.light_view.next();
                        tracing::info!(view = self.graphics.light_view.name(), "lighting view");
                        true
                    }
                    keyboard::Hotkey::Fringe => {
                        self.graphics.fringe = self.graphics.fringe.next();
                        // **The state on a line of its own**, which is the whole
                        // difference between a knob and a knob that looks
                        // broken: `discard` cuts a stripe out of every course of
                        // every roof, so a person who sees no change at all is
                        // looking at a frame this key never reached, and the log
                        // is what tells those two apart.
                        //
                        // It does *not* need the lights on. Daylight builds an
                        // empty grid, not an absent one, and
                        // `statics::push_volumes` calls `boxes_of` regardless —
                        // so a sprite is met against its own per-tile box either
                        // way, and only the box's *name* (`SolidId::NOBODY`) and
                        // its merging are what a grid adds. Measured rather than
                        // reasoned: `isolated_scene` with `_IMPOSTOR=0` still
                        // moves 9.7% of the frame's pixels between two of these
                        // states.
                        tracing::info!(fringe = self.graphics.fringe.name(), "fringe");
                        true
                    }
                    keyboard::Hotkey::FrameDump => {
                        self.request_frame_dump();
                        true
                    }
                    keyboard::Hotkey::MarkCombat => {
                        // The snapshot is taken *here*, at the keystroke, and
                        // that is the whole value of the key: a mark stamped a
                        // second later, after a hand has found a panel, records
                        // a screen that has moved on from the one it is about.
                        let me = self.world.me();
                        let seen = self.world.presentation.crowd.preparing(me);
                        self.world.presentation.combat_log.mark(me, String::new(), seen);
                        // Say what was marked, not that a mark happened. The
                        // person pressing this key is about to hand somebody the
                        // terminal, and "marked the combat log" is a line that
                        // answers none of the question it was pressed about.
                        tracing::info!(
                            seen = crate::combat_log::describe(&crate::combat_log::Event::Mark {
                                note: String::new(),
                                seen: seen.map(crate::combat_log::Seen::of),
                            }),
                            "marked the combat log"
                        );
                        // And write the file, here, rather than leaving it in a
                        // ring that only the panel can read. The key exists so
                        // that a hand does not have to find F1, then a tab, then
                        // a button — a mark that still needs those three to be
                        // *read* has moved the cost rather than removed it, and
                        // the ring is gone with the process. The panel's own
                        // button stays for when a note is being typed anyway.
                        self.save_combat_log();
                        false
                    }
                };
                if changed {
                    self.ask_redraw();
                }
            }
            // Shift is the whole of "run", and it arrives here rather than as a
            // key: `ModifiersChanged` is what winit reports a held modifier
            // with, and a `KeyboardInput` for the shift itself would miss the
            // case of it going down between two steps.
            WindowEvent::ModifiersChanged(modifiers) => {
                let shift = modifiers.state().shift_key();
                self.steer.set_running(shift);
                self.input.shift_held = shift;
                // Toggling Ctrl mid-drag switches the right-hold from a
                // heading to a move order (or back) on the next cursor move —
                // no special-casing needed, `walk_toward_cursor` reads this
                // fresh every call.
                self.input.ctrl_held = modifiers.state().control_key();
            }
            // A window that loses focus never hears the key come up, and a
            // character that keeps walking into a wall while its player is in
            // another window is not what the key meant. A destination is not a
            // held input, though: it is an already-issued move order and must
            // survive changing virtual desktops.
            //
            // It is also half of what paces the loop — see [`App::watched`] —
            // and regaining focus has to ask for a frame, because the redraw
            // that would have asked for the next one is the one that stopped
            // being drawn.
            WindowEvent::Focused(focused) => {
                self.input.focused = focused;
                if focused {
                    self.ask_redraw();
                } else {
                    self.steer.release_transient_inputs();
                    self.input.aiming = false;
                    self.input.shift_held = false;
                    self.set_war_mode_held(false);
                    // The mouse button is the keys' case exactly: a window
                    // being carried when the focus goes elsewhere is let go of
                    // in some other application, and the release never comes
                    // back here. Held past that, it would resume dragging on
                    // the first pointer move after the window is clicked back
                    // into — with no button down and nothing on screen to say
                    // why.
                    self.windows.grip.release();
                    // And the item press the same button may have started:
                    // its slop is measured from where the button went down,
                    // so a press whose release happened elsewhere would turn
                    // into a lift on the first move after the window comes
                    // back. See `Input::left_press`.
                    self.input.left_press = None;
                }
            }
            // Entirely covered by another window: the compositor will not show
            // anything drawn, so the loop stops drawing at the display's rate
            // and falls back to the animation clock. Uncovered, it restarts the
            // same way focus does.
            WindowEvent::Occluded(occluded) => {
                self.input.occluded = occluded;
                if !occluded {
                    self.ask_redraw();
                }
            }
            // A cursor that has left says so once and then goes quiet, so the
            // flag is what stands in for the positions that stop arriving. It
            // reaches here even when egui consumed the move that preceded it:
            // `on_window_event` does not claim these.
            WindowEvent::CursorEntered { .. } => {
                self.input.pointer_inside = true;
            }
            WindowEvent::CursorLeft { .. } => {
                self.input.pointer_inside = false;
                self.ask_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input.pointer_inside = true;
                // Relative to the *viewport* and not the window: the camera's
                // own centre is the viewport's, so a cursor measured from the
                // window would zoom about a point half a panel away.
                let origin = self.shell.as_ref().map_or((0, 0), |shell| {
                    (shell.viewport().x as i32, shell.viewport().y as i32)
                });
                let cursor = RealPixel::new(position.x as i32 - origin.0, position.y as i32 - origin.1);
                // The interface's cursor is measured from the surface's own
                // corner and in gump pixels, which is what everything drawn by
                // the gump pass is placed in.
                let scale = self.gump_scale();
                self.input.pointer_gump = GumpPixel::new(
                    (position.x as f32 / scale) as i32,
                    (position.y as f32 / scale) as i32,
                );
                let mut changed = self.control.cursor_moved(cursor);
                // The window layer, in one call — see `panes::route`. A move is
                // not exclusive: the camera above has already had it, and
                // nothing in the client asks whether a window took one.
                changed |= self.deliver(panes::Input::Move).redraw;
                // Held, the button steers: a heading toward wherever the cursor
                // is, by default, or a Ctrl-held move order — see
                // `walk_toward_cursor` and `steer.rs`'s module docs for why
                // those are two different things and not one idiom stated
                // twice.
                if self.input.aiming {
                    changed |= self.walk_toward_cursor();
                }
                if changed {
                    self.ask_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == winit::event::MouseButton::Middle {
                    self.control.set_panning(state == ElementState::Pressed);
                }
                // Where the left button went down, remembered before anything
                // is told that it did: it is the anchor every press that may
                // become a drag is measured from, whichever of the three
                // holders ends up with that press. Written here and nowhere
                // else, the way `WindowGrip`'s own frozen pointer is — one
                // gesture, one place that says where it started. See
                // `Input::left_press` and `hand::past_slop`.
                if button == winit::event::MouseButton::Left {
                    self.input.left_press = match state {
                        ElementState::Pressed => Some(self.input.pointer_gump),
                        ElementState::Released => None,
                    };
                }
                // A left click selects the tile under the cursor for the Tile
                // panel — reached here and not through egui, because `consumed`
                // above already sent every click the UI wanted to it.
                //
                // The release goes to the window layer whatever it lands on: it
                // ends a window drag, commits a held item to the bag under the
                // pointer, and lets a pressed button back up. See
                // `panes::route`.
                if button == winit::event::MouseButton::Left && state == ElementState::Released {
                    let response = self.deliver(panes::Input::Release(panes::Button::Left));
                    if response.redraw {
                        self.ask_redraw();
                    }
                }
                // The chat's own control, ahead of the windows and the world for
                // the reason it is drawn ahead of both — see
                // `App::press_channel_button`. It is the whole of the channel's
                // interface now: `Shift+Tab` still turns it for a hand that is
                // already typing, and this is what a hand on the mouse reaches
                // for.
                if button == winit::event::MouseButton::Left
                    && state == ElementState::Pressed
                    && self.press_channel_button()
                {
                    self.ask_redraw();
                    return;
                }
                // A container window takes the press before the world sees it,
                // the same way a panel does: the click that raises a bag must
                // not also select the tile behind it, and it must not start a
                // double-click pair that would use whatever is under there.
                if button == winit::event::MouseButton::Left
                    && state == ElementState::Pressed
                    && self.deliver(panes::Input::Press(panes::Button::Left)).taken
                {
                    self.ask_redraw();
                } else if button == winit::event::MouseButton::Left
                    && state == ElementState::Pressed
                    && !self.target_under_cursor(*self.control.camera())
                {
                    // The camera as it stands, which between two frames is the
                    // one the last frame was drawn with — the picture the player
                    // is clicking on.
                    let camera = *self.control.camera();
                    // What the last frame found under the cursor — by identity.
                    // At most one of the three is `Some` (see [`Hover`]), so
                    // this is a match over three exclusive answers rather than a
                    // shortlist being re-ranked here: the frame already settled
                    // which of them the cursor is on. `None` all the way down is
                    // how a selection is put out — there is nothing to select
                    // where nothing is standing, and bare ground is the tile.
                    //
                    // **The tile a bare click names is the ground under the
                    // cursor; everything else's is its own.** Two different
                    // arithmetics, on purpose: a wall's picture stands up the
                    // screen from the cell it is built on, so the ground *under
                    // the cursor* is the cell behind the wall — two tiles behind
                    // it, for a wall of ordinary height. Selecting a wall and
                    // marking that other tile is the client saying "this one"
                    // about two places at once, which is what this arm used to
                    // do; a mobile or an item stands exactly where it says it
                    // does, so no such correction applies to either.
                    self.picking.selected = match (
                        self.picking.hover.mobile,
                        self.picking.hover.item,
                        self.picking.hover.static_,
                    ) {
                        (Some(who), _, _) => Some(SelectedIdentity::Mobile(who)),
                        (None, Some(item), _) => Some(SelectedIdentity::Item(item)),
                        (None, None, Some(picked)) => Some(SelectedIdentity::Static(picked)),
                        (None, None, None) => {
                            self.pick_tile(camera).map(|tile| {
                                SelectedIdentity::Tile {
                                    x: tile.at.x,
                                    y: tile.at.y,
                                }
                            })
                        }
                    };
                    if self.map_editor.active() {
                        self.apply_map_editor_click(camera);
                        self.input.last_click = None;
                        self.ask_redraw();
                        return;
                    }
                    // **In war mode, a single click on a body is an attack.**
                    // The reference client's own gesture, and the reason the
                    // stance exists at all: at peace the same click selects and
                    // nothing more. Beside the selection rather than instead of
                    // it — the Tile panel still shows what was clicked, and a
                    // click that both selects and aims is one click with two
                    // readers, not two gestures fighting over one button.
                    //
                    // An aim and nothing else goes out; the shard's `swings`
                    // strikes on its own timer and answers with a `0xAA`. See
                    // `openshard_client_net::combat`.
                    self.attack_under_cursor();
                    // And the second click of a pair is a *use*: a door opens, a
                    // container opens, food is eaten. Which of those it is, is
                    // the shard's answer and not this end's — see
                    // `openshard_client_net::interact`.
                    let now = Instant::now();
                    let paired = self
                        .input
                        .last_click
                        .is_some_and(|last| now.duration_since(last) <= DOUBLE_CLICK);
                    // Cleared on a pair rather than restarted, so a third click
                    // starts a fresh one — ClassicUO's own reset.
                    self.input.last_click = (!paired).then_some(now);
                    if paired {
                        self.use_under_cursor(camera);
                    } else {
                        // A world item is lifted only once the press becomes a
                        // drag. The first click still selects it; the second is
                        // the ordinary double-click use.
                        self.press_world_item();
                    }
                    self.ask_redraw();
                }
                // A right hold is a heading toward the cursor by default, or a
                // Ctrl-held move order — either way it stays under way while
                // the button is, driven from `CursorMoved`. Left is spoken for
                // by the Tile panel above, and the middle button pans.
                // Right over a window closes it — the reference client's own
                // gesture — and does not steer: a press that never reached the
                // world cannot be a heading into it.
                // The two gestures are `||`-ed rather than chained because they
                // want the same redraw; the short-circuit keeps the order, so
                // the window layer still only sees the press when the target
                // cursor did not already take it.
                if button == winit::event::MouseButton::Right
                    && state == ElementState::Pressed
                    && (self.cancel_target_cursor()
                        || self.deliver(panes::Input::Press(panes::Button::Right)).taken)
                {
                    self.ask_redraw();
                } else if button == winit::event::MouseButton::Right {
                    self.input.aiming = state == ElementState::Pressed;
                    if self.input.aiming {
                        if self.walk_toward_cursor() {
                            self.ask_redraw();
                        }
                    } else {
                        // A heading stops the instant the button does — unlike
                        // a move order, which keeps walking itself there after
                        // the button that gave it is gone. `mouse_up` only
                        // touches the heading; a Ctrl-held destination in
                        // flight is untouched.
                        self.steer.mouse_up();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // A notch is a line on a wheel and a fraction of one on a
                // touchpad, and only the sign is asked for here: the ladder is
                // what decides how far a notch goes.
                let notches = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                // A list under the pointer takes the notch before the camera
                // does: rolling a wheel over a window is scrolling that window,
                // in this client as in every other.
                //
                // **`taken` and not `redraw` decides whether the camera hears
                // it**, and that distinction is the whole reason `panes` exists.
                // This used to be `scroll_skills() || scroll_vendor() ||
                // zoom()`, where one `bool` meant both — so a catalogue already
                // at its last row answered "nothing moved", the chain fell
                // through, and the wheel became a map zoom under a pointer that
                // had never left the shop window.
                if notches != 0.0 {
                    let response = self.deliver(panes::Input::Wheel(notches));
                    let zoomed = !response.taken && self.zoom(notches > 0.0);
                    if response.redraw || zoomed {
                        self.ask_redraw();
                    }
                }
            }
            _ => {}
        }
    }

    /// Re-arm the animation clock and ask for a redraw when it has advanced.
    ///
    /// `winit`'s idiomatic timer: `ControlFlow::WaitUntil` sleeps the event
    /// loop rather than spinning it, and returning here every
    /// [`App::redraw_interval`] is what stands in for a real client's own
    /// `Mobile.ProcessAnimation` poll.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        // A held arrow — or a tile the mouse sent the body to — asks for a step
        // every step's length. The step itself is dispatched by `tick`, rather
        // than here: an inactive window may never receive a redraw callback,
        // but it still receives the state-clock wake below.
        let walk_due = self.steer.deadline().is_some_and(|deadline| now >= deadline);
        // The animation clock. Watched, this is a safety net rather than the
        // pacer — `draw` asks for the next frame itself and the display answers
        // — and it is kept for the paths where that ask does not happen: `draw`
        // returns early with no window, with a swapchain it had to rebuild, and
        // on a compositor that refused to hand over a texture. Without it, one
        // of those would stop the loop dead until the next input event. An
        // unwatched window runs the same state tick directly: a compositor need
        // not issue a redraw for a virtual desktop it is not showing, but the
        // client's route and network state cannot depend on one.
        if walk_due {
            self.tick(now);
            if self.watched() {
                self.ask_redraw();
            }
        }
        if now >= self.next_tick {
            self.next_tick = now + self.redraw_interval();
            // `watched()` trusts `Focused`/`Occluded`, and a compositor is free
            // to delay or never send the one that would clear it: a surface
            // fully covered or swept to another workspace stops receiving frame
            // callbacks before winit reports either event, and on the loop's own
            // account (`Focused` and `Occluded` are `WindowEvent`s, dispatched
            // the same way `RedrawRequested` is) it may not report it in time to
            // matter. `draw` — the only caller of `tick` while watched — never
            // runs without one, so trusting the flag alone here can wait on a
            // frame callback that never arrives, and the state clock stops with
            // it. Measuring staleness against `last_advance` catches that
            // regardless of what the flag currently says: a window actually
            // being shown re-runs `draw` well inside this bound, and one that
            // is not gets ticked here instead of waiting on a callback that
            // will not come until it is shown again.
            let stalled = now.saturating_duration_since(self.last_advance) > STALLED_DRAW_TOLERANCE;
            if self.watched() && !stalled {
                self.ask_redraw();
            } else if !walk_due {
                self.tick(now);
            }
        }
        // Three reasons to come back, so three terms: the animation clock,
        // whatever the UI is animating, and the next step a held key is owed.
        // The deadline is the earliest — a loop that slept past the step would
        // walk at whatever rate it happened to wake at.
        // `checked_add`, because a still UI asks for eternity
        // (`Duration::MAX`, see `Shell::repaint_after`) and `now + MAX`
        // overflows the instant rather than meaning "never". An overflow is
        // exactly the case where the UI wants no frame of its own, so it falls
        // back to the animation clock.
        let deadline = match self.shell.as_ref().map(shell::Shell::repaint_after) {
            Some(after) => {
                match now.checked_add(after) {
                    Some(ui) => self.next_tick.min(ui),
                    None => self.next_tick,
                }
            }
            None => self.next_tick,
        };
        let deadline = match self.steer.deadline() {
            Some(step) => deadline.min(step),
            None => deadline,
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
    }

    /// The loop is over: write down what the HUD looked like.
    ///
    /// Here and not on `CloseRequested`, because that is one of several ways out
    /// — `event_loop.exit()` is also called from a startup failure and from the
    /// link — and this is the one place all of them pass through. A client that
    /// is killed writes nothing, which is the honest behaviour: the file says
    /// where things were when the client was last *closed*.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // The HUD's half — tab, panel, scale — and then the platform's, which
        // only the window itself can answer for.
        if let Some(shell) = self.shell.as_ref() {
            self.desk = shell.desk();
        }
        // F1 has two homes: the shell owns its layout and direct controls,
        // while the app owns the World, Tile and Rig settings it applies from
        // requests. Capture the latter at the same shutdown boundary so the
        // next session is a faithful continuation of this one.
        let mut f1 = desk::F1Settings::from_runtime(
            &self.graphics,
            self.control.rig(),
            self.world.presentation.crowd.ease(),
            self.scope.span(),
        );
        f1.apply_pending_request(&self.pending);
        self.desk.f1 = Some(f1);
        if let Some(screen) = self.window.as_ref() {
            let size = screen.window.inner_size();
            // A window whose position the platform will not report — Wayland
            // does not, by design — keeps whatever the file already said rather
            // than being moved to the origin. Half a frame restored is better
            // than a window that walks to the top-left corner every launch.
            let position = screen.window.outer_position().ok();
            let previous = self.desk.window;
            self.desk.window = Some(desk::Frame {
                x:         position.map_or_else(|| previous.map_or(0, |frame| frame.x), |at| at.x),
                y:         position.map_or_else(|| previous.map_or(0, |frame| frame.y), |at| at.y),
                width:     size.width.max(1),
                height:    size.height.max(1),
                maximized: screen.window.is_maximized(),
            });
        }
        if let Err(error) = self.desk.save(std::path::Path::new(desk::PATH)) {
            eprintln!("{error}");
        }
    }
}
