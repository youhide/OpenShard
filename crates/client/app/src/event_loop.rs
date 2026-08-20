//! `winit` glue: the `ApplicationHandler` impl that dispatches a platform
//! event into whichever subsystem answers to it — a step to `ui_command.rs`,
//! a packet to `net_command.rs`, a press on a window to `own_windows.rs`, a
//! redraw to `presentation.rs`. Nothing here decides gameplay on its own; it
//! is the seam between what winit reports and the method that already knows
//! what to do with it.

use std::time::Instant;

use openshard_client_render::camera::RealPixel;
use openshard_client_render::gump::GumpPixel;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

use crate::app::App;
use crate::picking::SelectedIdentity;
use crate::world::{cluttered, cluttered_with_doors_open, terrain};
use crate::{DOUBLE_CLICK, PAGE_PIXELS, desk, keyboard, keys, panes, shell, steer};

impl App {
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
            self.ask_redraw();
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
                // ClassicUO's default Tab action is held: it enters war mode
                // on the first press and returns to peace mode on release.
                // Handle both states before the pressed-only hotkeys below so
                // an operating-system repeat cannot act as another press.
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
                            let guide = terrain(&self.resources);
                            let opened = cluttered_with_doors_open(&self.world, &self.resources);
                            let cluttered = cluttered(&self.world, &self.resources);
                            let motion = self.world.motion.planning_state();
                            self.steer.press(
                                direction,
                                motion.position,
                                Instant::now(),
                                motion.facing.direction,
                                steer::Ground {
                                    real: &cluttered,
                                    through_doors: &opened,
                                    guide: &guide,
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
                let Some(hotkey) = keyboard::Hotkey::of(code) else {
                    return;
                };
                let changed = match hotkey {
                    keyboard::Hotkey::DevWindow => {
                        // The shell's `Desk` and not this one: see
                        // `Shell::toggle_dev`. Before there is a shell there is no
                        // window either, so this arm cannot be reached without one.
                        if let Some(shell) = self.shell.as_mut() {
                            shell.toggle_dev();
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
                    keyboard::Hotkey::PanUp => self.control.pan(0, PAGE_PIXELS),
                    keyboard::Hotkey::PanDown => self.control.pan(0, -PAGE_PIXELS),
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
            // another window is not what the key meant. The destination goes
            // with it, for the same reason: nobody is watching it be walked to.
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
                    self.steer.clear();
                    self.input.aiming = false;
                    self.input.shift_held = false;
                    self.set_war_mode_held(false);
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
                    // What the last frame found under the cursor — by identity,
                    // and in the same order the hover pick already answered
                    // "what is the cursor on" in: a creature first, then an item,
                    // then the map's own furniture, then bare ground. `None` all
                    // the way down is how a selection is put out: there is
                    // nothing to select where nothing is standing.
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
                        self.picking.on_mobile,
                        self.picking.on_item,
                        self.picking.on_static,
                    ) {
                        (Some(who), _, _) => Some(SelectedIdentity::Mobile(who)),
                        (None, Some(serial), _) => Some(SelectedIdentity::Item(serial)),
                        (None, None, Some(picked)) => Some(SelectedIdentity::Static(picked)),
                        (None, None, None) => self.pick_tile(camera).map(|tile| SelectedIdentity::Tile {
                            x: tile.at.x,
                            y: tile.at.y,
                        }),
                    };
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
            WindowEvent::RedrawRequested => self.draw(),
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
        // every step's length. Here and not in the input event: the operating
        // system repeats a held key at a rate that is not a walking speed, a
        // mouse held over the ground reports a move a pixel, and the fast half
        // of either is refused by the shard as a speedhack — which reads as the
        // walk stuttering. See `steer.rs`.
        //
        // Twice at most, because a turn is a step that covers no ground and
        // costs no time against the shard's pace budget: the step it precedes is
        // due the same instant, and holding that back to the next wake would put
        // a frame of standing still exactly where the player asked for movement.
        // Two and not a loop — the second ask is the step the turn was for, and
        // anything past it is a rate, which is what the clock is for.
        let mut moved = false;
        for _ in 0..2 {
            // Both halves of the ground, because this is where a destination
            // replans: see `steer::Ground`. Built here rather than held, for the
            // reason the single terrain always was — they borrow the map and the
            // view, and the walk borrows `steer` mutably beside them.
            let guide = terrain(&self.resources);
            let opened = cluttered_with_doors_open(&self.world, &self.resources);
            let cluttered = cluttered(&self.world, &self.resources);
            let ground = steer::Ground {
                real: &cluttered,
                through_doors: &opened,
                guide: &guide,
                coarse: self.resources.coarse.as_ref(),
            };
            let motion = self.world.motion.planning_state();
            let Some(facing) = self
                .steer
                .due(now, motion.position, motion.facing.direction, ground)
            else {
                break;
            };
            moved |= self.walk(facing);
        }
        if moved {
            self.ask_redraw();
        }
        // The animation clock. Watched, this is a safety net rather than the
        // pacer — `draw` asks for the next frame itself and the display answers
        // — and it is kept for the paths where that ask does not happen: `draw`
        // returns early with no window, with a swapchain it had to rebuild, and
        // on a compositor that refused to hand over a texture. Without it, one
        // of those would stop the loop dead until the next input event. The
        // redraw requests coalesce, so a net that fires while the display is
        // already pacing costs a wake and no frame.
        if now >= self.next_tick {
            self.next_tick = now + self.redraw_interval();
            self.ask_redraw();
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
            Some(after) => match now.checked_add(after) {
                Some(ui) => self.next_tick.min(ui),
                None => self.next_tick,
            },
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
        if let Some(screen) = self.window.as_ref() {
            let size = screen.window.inner_size();
            // A window whose position the platform will not report — Wayland
            // does not, by design — keeps whatever the file already said rather
            // than being moved to the origin. Half a frame restored is better
            // than a window that walks to the top-left corner every launch.
            let position = screen.window.outer_position().ok();
            let previous = self.desk.window;
            self.desk.window = Some(desk::Frame {
                x: position.map_or_else(|| previous.map_or(0, |frame| frame.x), |at| at.x),
                y: position.map_or_else(|| previous.map_or(0, |frame| frame.y), |at| at.y),
                width: size.width.max(1),
                height: size.height.max(1),
                maximized: screen.window.is_maximized(),
            });
        }
        if let Err(error) = self.desk.save(std::path::Path::new(desk::PATH)) {
            eprintln!("{error}");
        }
    }
}
