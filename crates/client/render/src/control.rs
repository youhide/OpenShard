//! What the mouse, the wheel and the lock key do to a [`Camera`].
//!
//! The arithmetic between an input event and an eye: a drag in real pixels, a
//! wheel notch's anchor, the device's refusal to allocate the image a zoom asks
//! for, and whether the eye is the body's or the mouse's. All of it
//! used to live in `client/app`, where it could not be reached from a test
//! because the thing that owned it also owned a window, a GPU and a `WorldMap`. None
//! of it needs any of the three.
//!
//! What is deliberately *not* here: any decision about what happens next. This
//! answers whether something moved and reports what a device refused; asking for
//! a redraw and printing the refusal are the caller's, because a renderer with a
//! stderr is a renderer that cannot be run twice in one process.

use std::time::Duration;

use crate::camera::{Camera, RealPixel, WorldPoint, Zoom};
use crate::follow::{Follower, Gaze, Rig};

/// Whether the camera is tied to the body or the mouse.
///
/// It lives beside the camera and not inside it: the camera does not know what a
/// player is, and giving it one would put `client/net` inside `client/render`.
/// What it *is* is a rule about who may move the eye, which is exactly what
/// [`Control`] arbitrates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Follow {
    /// The eye is the body's, and the server moves it.
    Body,
    /// The eye is the mouse's, and the body may walk off screen.
    Free,
}

/// What the mouse is doing to the camera.
///
/// It used to carry a third field: the fraction of a virtual pixel a drag had
/// dragged and not yet spent, because at `2x` a one-real-pixel drag was half a
/// virtual one and the eye could only hold whole ones. Under D11 the eye is on
/// the real pixel's lattice, so a drag of one real pixel is a position the eye
/// can express exactly — there is no remainder to keep, and the drag that used
/// to move nothing three times out of four now moves the world by exactly what
/// the hand moved.
#[derive(Clone, Copy, Default, Debug)]
struct Drag {
    /// Where the cursor was last seen, in physical pixels from the viewport's
    /// top-left. Needed by the wheel, which is told a delta and not a position.
    cursor: RealPixel,
    /// Whether the middle button is down.
    panning: bool,
}

/// A zoom the device would not allocate the offscreen image for.
///
/// Returned rather than printed, and it carries the numbers because the message
/// worth writing names all of them: a silently truncated target draws a smaller
/// world into a larger rect, which looks exactly like a bug in the projection, so
/// whoever hits this has to be able to say what happened and why.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TooLarge {
    /// The zoom that was asked for and refused.
    pub wanted: Zoom,
    /// The image it would have wanted, in pixels.
    pub width: u32,
    /// Likewise.
    pub height: u32,
    /// What this device allows in either dimension.
    pub max: u32,
    /// The zoom in force now — the old one after a refusal, a tighter one after
    /// [`Control::fit_to_device`] stepped in.
    pub settled: Zoom,
}

/// A camera, who is allowed to move it, and where the mouse last was.
#[derive(Clone, Copy, Debug)]
pub struct Control {
    camera: Camera,
    follow: Follow,
    /// How the eye follows the body while [`Follow::Body`] holds — the rig and
    /// where it has got to. Arbitrating *who* may move the eye is this type's
    /// job; how it moves is [`crate::follow`]'s, and the two are separated
    /// because only the second one can be put on a bench.
    follower: Follower,
    drag: Drag,
    /// How far the ladder may be walked down before the offscreen texture is
    /// larger than the GPU allows.
    ///
    /// WebGL2 guarantees only 2048 in each dimension and a 1080p window at `1/2`
    /// wants more, so the ladder has a runtime end that depends on the device.
    max_texture: u32,
}

impl Control {
    /// A locked camera on this device, following with this rig.
    ///
    /// The rig is an argument and not a default: which camera this client ships
    /// is undecided, and a `new` that quietly picked one would be the decision
    /// (`docs/camera.md`, D9).
    pub fn new(camera: Camera, max_texture: u32, rig: Rig) -> Self {
        Self {
            camera,
            follow: Follow::Body,
            follower: Follower::new(rig),
            drag: Drag::default(),
            max_texture,
        }
    }

    /// The camera, for everything that draws from it.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// The rig the eye is following with.
    pub fn rig(&self) -> Rig {
        self.follower.rig()
    }

    /// Where the eye is to a fraction of a pixel, channel by channel, for a
    /// bench or a scope — see [`Follower::exact`]. `None` before the first
    /// frame.
    pub fn eye_exact(&self) -> Option<Gaze> {
        self.follower.exact()
    }

    /// Whether the eye still owes the screen a pixel — see
    /// [`Follower::settling`]. Only while it is the body's: an unlocked eye is
    /// wherever the hand left it and is converging on nothing.
    pub fn settling(&self) -> bool {
        self.follow == Follow::Body && self.follower.settling(self.camera.quantum())
    }

    /// Follow with another one, without moving the eye — see
    /// [`Follower::set_rig`].
    pub fn set_rig(&mut self, rig: Rig) {
        self.follower.set_rig(rig);
    }

    /// Whether the eye is the body's or the mouse's.
    pub fn follow(&self) -> Follow {
        self.follow
    }

    /// Whether the middle button is held.
    pub fn panning(&self) -> bool {
        self.drag.panning
    }

    /// Where the cursor was last seen, in viewport pixels.
    pub fn cursor(&self) -> RealPixel {
        self.drag.cursor
    }

    /// The viewport changed size — a window resize, or a panel that grew.
    ///
    /// Zero is a minimised window rather than an error, and a texture of zero
    /// width is, so both are floored at one.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.camera.width = width.max(1);
        self.camera.height = height.max(1);
    }

    /// What this device will allocate, once there is a device to ask.
    pub fn set_max_texture(&mut self, max: u32) {
        self.max_texture = max;
    }

    /// Put the eye back on the body and lock it there.
    ///
    /// Snaps rather than eases, and deliberately: this is the answer to a camera
    /// that has been dragged somewhere else entirely, so there is no step to
    /// interpolate across — easing it would be a second kind of motion, over a
    /// distance nothing bounds. A body's own step is [`Control::follow_body`],
    /// which glides.
    ///
    /// The cut is what makes that true for a rig that eases. Moving the camera
    /// without it would leave the follower's own idea of where the eye is on the
    /// far side of the map, and the next frame would ease all the way back.
    ///
    /// A [`Gaze`] and not a tile, for the same reason [`Control::follow_body`]
    /// takes one: a body relocked to mid-glide is between two tiles, and the
    /// tile it is nominally on is up to half a tile from where it is drawn. The
    /// pixel the eye is put on has to be the pixel the sprite is on, or the
    /// first frame after a relock is off by that much and the second corrects
    /// it — which is the jump the cut was there to avoid, one frame late.
    pub fn relock(&mut self, gaze: Gaze) {
        self.follow = Follow::Body;
        self.follower.cut();
        self.camera.look_at(gaze.eye());
    }

    /// Let the body walk off screen: the eye stops following it.
    pub fn unlock(&mut self) {
        self.follow = Follow::Free;
    }

    /// The body moved. The eye follows it only while locked.
    ///
    /// The one rule the lock exists for, and the reason it is a method rather
    /// than an `if` at each call site: `App::step` and `App::entered` are two
    /// writers of the same eye, and a third would forget.
    ///
    /// A [`Gaze`] and not a tile, because a body between two tiles is where the
    /// eye has to be: a camera that only moved when a `0x77` arrived would jump
    /// the whole world a tile at a time under a character gliding smoothly
    /// across it — which is worse than the teleport the glide removed, since it
    /// is the *world* that jerks. `mobiles::gaze` is what to hand it.
    ///
    /// Called every frame and not only when a step lands, whatever the rig: a
    /// glide moves the body between packets, and a filtered rig is still
    /// converging on frames where nothing arrived at all.
    pub fn follow_body(&mut self, gaze: Gaze, dt: Duration) {
        if self.follow == Follow::Body {
            let eye = self.follower.advance(gaze, dt);
            self.camera.look_at(eye);
        }
    }

    /// The middle button went down or came up.
    pub fn set_panning(&mut self, down: bool) {
        self.drag.panning = down;
    }

    /// The cursor moved to a viewport pixel, panning if the button is down.
    ///
    /// Answers whether the eye actually moved: at zoom 4 most one-pixel drags
    /// move nothing, and asking for a redraw for each of them would be a frame
    /// per mouse report showing the same picture.
    pub fn cursor_moved(&mut self, at: RealPixel) -> bool {
        let (dx, dy) = (at.x - self.drag.cursor.x, at.y - self.drag.cursor.y);
        self.drag.cursor = at;
        self.drag.panning && self.pan(dx, dy)
    }

    /// Move the eye by a drag, in real pixels.
    ///
    /// One real pixel of hand is one real pixel of world, at every rung of the
    /// ladder: the eye's lattice *is* the real pixel, so the conversion below is
    /// exact and there is nothing left over. Before D11 this was the arithmetic
    /// with the remainder in it, and what it bought was a camera that ignored
    /// three drags in four at `4x` and then jumped four pixels.
    ///
    /// Unlocks: a hand on the camera outranks the server, until `Home` says
    /// otherwise. Answers whether the eye moved.
    pub fn pan(&mut self, dx: i32, dy: i32) -> bool {
        self.follow = Follow::Free;
        if dx == 0 && dy == 0 {
            return false;
        }
        let quantum = self.camera.quantum();
        let eye = self.camera.eye_at();
        // The world follows the cursor, so the eye goes the other way.
        self.camera.look_at(WorldPoint {
            x: eye.x - f64::from(dx) * quantum,
            y: eye.y - f64::from(dy) * quantum,
        });
        true
    }

    /// One notch of the wheel.
    ///
    /// `Ok(true)` when something changed, `Ok(false)` at either end of the
    /// ladder, and `Err` when the device would not hold the image the new zoom
    /// wants — refused rather than truncated, because a smaller world drawn into
    /// a larger rect looks like a projection bug and reads as one.
    pub fn zoom(&mut self, inwards: bool) -> Result<bool, TooLarge> {
        let wanted = if inwards {
            self.camera.zoom().scale_up()
        } else {
            self.camera.zoom().scale_down()
        };
        if wanted == self.camera.zoom() {
            return Ok(false);
        }
        // Locked to the body, the zoom is about the middle: an eye held to the
        // cursor would be moved here and moved back by the next `WorldView`,
        // which is a fight rather than a camera. Unlocked, it is about the
        // cursor, which is the difference between a camera that feels placed and
        // one that feels shoved.
        let anchor = match self.follow {
            Follow::Body => RealPixel::new(self.camera.width as i32 / 2, self.camera.height as i32 / 2),
            Follow::Free => self.drag.cursor,
        };
        let mut probe = self.camera;
        probe.zoom_about(anchor, wanted);
        if let Some(refusal) = self.refuses(&probe) {
            return Err(refusal);
        }
        self.camera = probe;
        Ok(true)
    }

    /// Step the zoom back in until the world texture fits this device.
    ///
    /// [`Control::zoom`] refuses a step that would not fit, and that is not the
    /// whole of it: the offscreen image is `viewport / zoom`, so *growing the
    /// window* at a zoom that fitted asks for a texture that does not. Nobody
    /// zooms in that path, so the check has to live where the size is used and
    /// not only where the zoom changes — without this, dragging a window wider at
    /// `1/2` is a validation error from the texture rather than a camera that
    /// stops zooming out.
    ///
    /// Answers with what did not fit, once, whatever the number of rungs it had
    /// to climb: the caller wants to say it, not to say it repeatedly.
    pub fn fit_to_device(&mut self) -> Option<TooLarge> {
        let mut refusal: Option<TooLarge> = None;
        while let Some(too_large) = self.refuses(&self.camera) {
            let tighter = self.camera.zoom().scale_up();
            if tighter == self.camera.zoom() {
                // The top of the ladder and still too large, which means a
                // viewport bigger than the device's limit — nothing a zoom can
                // answer. Reported rather than looped over.
                break;
            }
            refusal.get_or_insert(too_large);
            // About the middle: this is not somebody's wheel, it is the device
            // saying no, and there is no cursor that asked for it.
            let centre = RealPixel::new(self.camera.width as i32 / 2, self.camera.height as i32 / 2);
            self.camera.zoom_about(centre, tighter);
        }
        // The zoom in force is the one this settled on, not the one the first
        // rung refused.
        refusal.map(|first| TooLarge {
            settled: self.camera.zoom(),
            ..first
        })
    }

    /// Whether the offscreen image this camera wants is larger than the device
    /// allows, and by how much.
    fn refuses(&self, camera: &Camera) -> Option<TooLarge> {
        // The image as it will actually be allocated, which above 1:1 is the
        // viewport's own size rather than the world's extent — a magnified
        // camera asks for *less* texture than an unmagnified one, and asking
        // `render_width` here would refuse a zoom that fits.
        let (width, height) = camera.image_size();
        if width <= self.max_texture && height <= self.max_texture {
            return None;
        }
        Some(TooLarge {
            wanted: camera.zoom(),
            width,
            height,
            max: self.max_texture,
            // Overwritten by `fit_to_device`, which knows where it stopped. On a
            // refusal from `zoom` the camera did not move, so this is already it.
            settled: self.camera.zoom(),
        })
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::world::Point;

    use super::*;

    /// A device that will hold anything, so a test about panning is not also a
    /// test about texture limits.
    const HUGE: u32 = 1 << 20;

    fn control() -> Control {
        Control::new(Camera::new(Point::new(100, 100, 0), 800, 600), HUGE, Rig::HARD)
    }

    /// A frame's worth of time, for the calls that take one and do not care.
    const FRAME: Duration = Duration::from_millis(16);

    /// Zooming out four rungs from `1:1` and back in four lands on the same
    /// zoom, which is the ladder's whole promise.
    #[test]
    fn the_ladder_ends_rather_than_wrapping() {
        let mut control = control();
        let mut out = 0;
        while control.zoom(false).unwrap() {
            out += 1;
            assert!(out < 100, "the ladder has no bottom");
        }
        assert_eq!(control.camera().zoom().to_string(), "1/2x");
        let mut back = 0;
        while control.zoom(true).unwrap() {
            back += 1;
            assert!(back < 100, "the ladder has no top");
        }
        assert_eq!(control.camera().zoom().to_string(), "4x");
        assert_eq!((out, back), (3, 6), "seven rungs, with 1:1 the fourth");
    }

    /// The gate D11 names, on the input side: one real pixel of hand is one real
    /// pixel of world, at every rung of the ladder.
    ///
    /// This is the test the old one is the negative of. Before D11 the eye held
    /// whole *virtual* pixels, so at `4x` three drags in four moved nothing and
    /// the fourth moved four real pixels at once — which read as a camera that
    /// ignored small movements and then lurched, and which the remainder in
    /// `Drag` was there to make orderly rather than to remove.
    ///
    /// Measured through `to_viewport`, and deliberately: `eye()` is rounded to a
    /// virtual pixel and at `4x` it does not change at all for three drags out
    /// of four, so an assertion on it would pass on the old arithmetic too. What
    /// is asserted is where a fixed point of the *world* lands on the display.
    #[test]
    fn a_drag_of_one_real_pixel_moves_the_world_one_real_pixel() {
        let mut control = control();
        // From the bottom of the ladder, so the minifying rungs are walked too:
        // there a real pixel is *coarser* than a virtual one and the eye moves
        // by two at a time, which is the other side of the same promise and the
        // side that goes through the blit rather than through the transform.
        while control.zoom(false).unwrap() {}
        let mut rung = 0;
        loop {
            // A tile well away from the eye, so nothing about this is a
            // property of the origin.
            let fixed = Point::new(340, 360, 0);
            let before = control.camera().to_viewport(control.camera().to_screen(fixed));
            assert!(control.pan(1, 1), "a real pixel is always a position at {rung}");
            let after = control.camera().to_viewport(control.camera().to_screen(fixed));
            assert_eq!(
                (after.x - before.x, after.y - before.y),
                (1.0, 1.0),
                "the world did not follow the hand at rung {rung}",
            );
            if !control.zoom(true).unwrap() {
                break;
            }
            rung += 1;
        }
        assert_eq!(rung, 6, "every rung of the ladder was walked");
    }

    /// And the direction of the rounding: a drag out and back ends where it
    /// started rather than a pixel to one side.
    ///
    /// It survives D11 unchanged and is worth keeping for it — under the old
    /// arithmetic it was a statement about a signed remainder truncating towards
    /// zero, and under this one it holds because there is no remainder at all.
    #[test]
    fn a_drag_out_and_back_ends_where_it_started() {
        let mut control = control();
        assert!(control.zoom(true).unwrap());
        let before = control.camera().eye_at();
        for _ in 0..7 {
            control.pan(1, 1);
        }
        for _ in 0..7 {
            control.pan(-1, -1);
        }
        assert_eq!(control.camera().eye_at(), before);
    }

    /// A drag finer than the display cannot exist, so a zoom has nothing to
    /// forget — but the eye it leaves behind has to land on the *new* rung's
    /// lattice, or it sits between two real pixels until somebody moves it.
    ///
    /// Half a virtual pixel is a position at `2x` and is not one at `3x`, which
    /// is the case this walks. The old test in this slot asserted that a zoom
    /// dropped the fraction a drag was saving up; there is no such fraction now,
    /// and the question it was really about — what happens to a sub-pixel offset
    /// when the rung changes — is this.
    #[test]
    fn a_zoom_leaves_the_eye_on_the_new_rungs_lattice() {
        let mut control = control();
        assert!(control.zoom(true).unwrap());
        assert_eq!(control.camera().zoom().to_string(), "2x");
        assert!(control.pan(1, 0), "half a virtual pixel, which 2x can express");
        let half = control.camera().eye_at();
        assert_eq!(half.x.fract().abs(), 0.5);

        assert!(control.zoom(true).unwrap());
        assert_eq!(control.camera().zoom().to_string(), "3x");
        let thirds = control.camera().eye_at();
        assert_eq!(
            thirds,
            control.camera().snap(thirds),
            "an eye off the lattice is a frame resampled by a fraction of a texel",
        );
    }

    /// `cursor_moved` pans only while the button is down, and it pans by the
    /// delta rather than by the position — the first cursor report after the
    /// window opens is a jump from wherever the origin happens to be.
    #[test]
    fn the_cursor_moves_the_eye_only_while_the_button_is_down() {
        let mut control = control();
        assert!(
            !control.cursor_moved(RealPixel::new(400, 300)),
            "no button, no pan"
        );
        assert_eq!(control.cursor(), RealPixel::new(400, 300));
        let before = control.camera().eye();
        control.set_panning(true);
        assert!(control.cursor_moved(RealPixel::new(410, 300)));
        assert_eq!(control.camera().eye().x, before.x - 10);
    }

    /// The lock is the rule two writers share: while it holds, the body moves
    /// the eye; once a drag has unlocked it, the body is free to walk off screen.
    #[test]
    fn the_body_moves_the_eye_only_while_locked() {
        let mut control = control();
        assert_eq!(control.follow(), Follow::Body);
        control.follow_body(Gaze::on(Point::new(200, 200, 0)), FRAME);
        assert_eq!(
            control.camera().eye(),
            crate::camera::project(Point::new(200, 200, 0))
        );

        control.pan(30, 30);
        assert_eq!(control.follow(), Follow::Free, "a hand on the camera unlocks it");
        let free = control.camera().eye();
        control.follow_body(Gaze::on(Point::new(300, 300, 0)), FRAME);
        assert_eq!(control.camera().eye(), free, "the body no longer drags the eye");

        control.relock(Gaze::on(Point::new(300, 300, 0)));
        assert_eq!(control.follow(), Follow::Body);
        assert_eq!(
            control.camera().eye(),
            crate::camera::project(Point::new(300, 300, 0))
        );
    }

    /// Locked, the wheel zooms about the middle rather than the cursor: an eye
    /// pinned to the cursor would be moved by the zoom and moved straight back by
    /// the next `WorldView`, which is a fight rather than a camera.
    #[test]
    fn a_locked_zoom_is_about_the_middle() {
        let mut locked = control();
        locked.set_panning(false);
        locked.cursor_moved(RealPixel::new(0, 0));
        let eye = locked.camera().eye();
        assert!(locked.zoom(true).unwrap());
        assert_eq!(locked.camera().eye(), eye, "the middle does not move");

        // The same notch with the same cursor, unlocked, holds the *cursor*
        // still instead — which moves the eye.
        let mut free = control();
        free.unlock();
        free.cursor_moved(RealPixel::new(0, 0));
        let under_cursor = free.camera().pick(RealPixel::new(0, 0));
        assert!(free.zoom(true).unwrap());
        assert_eq!(free.camera().pick(RealPixel::new(0, 0)), under_cursor);
        assert_ne!(free.camera().eye(), eye);
    }

    /// The device's refusal is a value and not a truncation: the zoom does not
    /// change, and the numbers needed to explain it come back.
    #[test]
    fn a_device_that_cannot_hold_the_image_refuses_the_zoom() {
        // 800 wide at 3/4 wants 1068, which this device will not hold.
        let mut control = Control::new(Camera::new(Point::new(100, 100, 0), 800, 600), 1024, Rig::HARD);
        let before = *control.camera();
        let refusal = control.zoom(false).unwrap_err();
        assert_eq!(*control.camera(), before, "a refusal moves nothing");
        assert_eq!(refusal.max, 1024);
        assert_eq!(refusal.width, 1068);
        assert_eq!(refusal.settled, before.zoom());
        assert_eq!(refusal.wanted.to_string(), "3/4x");
    }

    /// And the path no wheel takes: a window dragged wider at a zoom that fitted
    /// asks for a texture that does not, so the fit is checked where the size is
    /// used. Without this the next frame is a validation error.
    #[test]
    fn growing_the_viewport_steps_the_zoom_back_in() {
        let mut control = Control::new(Camera::new(Point::new(100, 100, 0), 400, 300), 1024, Rig::HARD);
        assert!(control.zoom(false).unwrap(), "536 fits in 1024");
        assert_eq!(control.camera().zoom().to_string(), "3/4x");
        assert_eq!(control.fit_to_device(), None);

        control.resize(1000, 300);
        let refusal = control.fit_to_device().expect("1336 is past 1024");
        assert_eq!(refusal.width, 1336);
        assert_eq!(refusal.settled, control.camera().zoom());
        assert_eq!(control.camera().zoom(), Zoom::ONE);
        assert!(control.camera().render_width() <= 1024);
        assert_eq!(control.fit_to_device(), None, "once it fits it stays fitted");
    }

    /// Several rungs at once, and the answer is still one refusal — the caller
    /// prints it, and a caller that printed one per rung would be shouting.
    #[test]
    fn a_fit_that_climbs_several_rungs_reports_once() {
        let mut control = Control::new(Camera::new(Point::new(100, 100, 0), 400, 300), 4096, Rig::HARD);
        assert!(control.zoom(false).unwrap());
        control.set_max_texture(512);
        control.resize(1200, 900);
        let refusal = control.fit_to_device().expect("1200 at 3/4 is 1600");
        assert_eq!(refusal.wanted.to_string(), "3/4x", "the rung that first refused");
        assert_eq!(refusal.settled, control.camera().zoom());
        assert!(control.camera().render_width() <= 512);
    }

    /// A viewport larger than the device's limit is not a zoom problem, and the
    /// loop that steps in has to stop rather than spin at the top of the ladder.
    #[test]
    fn a_viewport_past_the_limit_stops_at_the_top_of_the_ladder() {
        let mut control = Control::new(Camera::new(Point::new(100, 100, 0), 8192, 8192), 1024, Rig::HARD);
        let refusal = control.fit_to_device().expect("nothing here fits");
        assert_eq!(refusal.settled.to_string(), "4x", "climbed as far as it could");
        assert_eq!(control.camera().zoom().to_string(), "4x");
    }

    /// The viewport is floored rather than allowed to be zero: a minimised
    /// window is not an error and a texture of zero width is.
    #[test]
    fn a_minimised_window_is_one_pixel_wide() {
        let mut control = control();
        control.resize(0, 0);
        assert_eq!(control.camera().width, 1);
        assert_eq!(control.camera().render_width(), 1);
    }
}
