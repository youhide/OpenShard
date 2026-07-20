//! Audiovisual feedback: sound, animation and graphical effects.
//!
//! The packets that make a world *felt* rather than merely correct — a swing
//! whooshes, a fireball flies, a body crumples. UO drives all of it from the
//! server: there is no client-side "you swung, so animate" rule, so a state
//! change with no feedback packet is silent and still to the client, which reads
//! as broken even when the numbers are right. Every one of these is broadcast to
//! the watchers who can see the actor, through the same interest machinery as a
//! `0x78`.
//!
//! Layouts are ported from ServUO's `Server/Network/Packets.cs`
//! (`PlaySound`, `MobileAnimation`, `NewMobileAnimation`, `GraphicalEffect`,
//! `HuedEffect`) and agree with Sphere's `sphereproto.h`. The wire is
//! big-endian, like the rest of the protocol.

use crate::codec::PacketWriter;

/// `0x54` — play a sound at a world location. 12 bytes.
///
/// The point places the sound in 3D so the client attenuates it by distance; a
/// sound with no place (a UI blip) is not this packet. `volume` is left at
/// ServUO's `0` — the client scales by distance, and the flag byte is its fixed
/// `1`.
pub fn encode_play_sound(sound: u16, x: u16, y: u16, z: i8) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(12);
    writer.u8(0x54);
    writer.u8(0x01); // flags — ServUO's constant
    writer.u16(sound);
    writer.u16(0x0000); // volume; the client scales by distance
    writer.u16(x);
    writer.u16(y);
    // ServUO writes Z as a full `short`, so a negative height sign-extends to 16
    // bits — not the 8-bit z the map tiles carry.
    writer.u16(i16::from(z) as u16);
    writer.into_bytes()
}

/// `0x6E` — animate a mobile with the classic action packet. 14 bytes.
///
/// The pre-7.0 form, and what a client without [`Feature::NewMobileAnimation`]
/// understands. `forward` is written inverted on the wire (the protocol field is
/// really "reverse"); this takes the intuitive sense and flips it, as ServUO
/// does. A swing, a bow, a cast gesture, a death throe are all one of these.
///
/// [`Feature::NewMobileAnimation`]: crate::Feature::NewMobileAnimation
pub fn encode_action(
    serial: u32,
    action: u16,
    frame_count: u16,
    repeat_count: u16,
    forward: bool,
    repeat: bool,
    delay: u8,
) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(14);
    writer.u8(0x6E);
    writer.u32(serial);
    writer.u16(action);
    writer.u16(frame_count);
    writer.u16(repeat_count);
    writer.bool(!forward); // the wire field is "reverse"
    writer.bool(repeat);
    writer.u8(delay);
    writer.into_bytes()
}

/// `0xE2` — animate a mobile with the 7.0.0.0+ action packet. 10 bytes.
///
/// Gate the choice between this and [`encode_action`] on
/// [`Feature::NewMobileAnimation`], never on era. The action numbering differs
/// from the classic packet — `animation_type` selects a category the client maps
/// to its newer animation tables — so a caller picks the number to match the
/// packet it is sending, not the reverse.
///
/// [`Feature::NewMobileAnimation`]: crate::Feature::NewMobileAnimation
pub fn encode_new_action(serial: u32, animation_type: u16, action: u16, delay: u8) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(10);
    writer.u8(0xE2);
    writer.u32(serial);
    writer.u16(animation_type);
    writer.u16(action);
    writer.u8(delay);
    writer.into_bytes()
}

/// How a graphical effect moves, ServUO's `EffectType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum EffectKind {
    /// A projectile from one point/mobile to another — a bolt, an arrow.
    Moving = 0x00,
    /// A lightning strike on the source.
    Lightning = 0x01,
    /// A fixed animation at a world point.
    FixedXyz = 0x02,
    /// A fixed animation on the source mobile.
    FixedFrom = 0x03,
}

/// A point a graphical effect starts or ends at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EffectPoint {
    /// East–west.
    pub x: u16,
    /// North–south.
    pub y: u16,
    /// Height.
    pub z: i8,
}

/// Write the 0x70/0xC0 shared body (everything up to the hue fields).
#[allow(clippy::too_many_arguments)]
fn write_effect_body(
    writer: &mut PacketWriter,
    kind: EffectKind,
    from: u32,
    to: u32,
    item: u16,
    from_pt: EffectPoint,
    to_pt: EffectPoint,
    speed: u8,
    duration: u8,
    fixed_direction: bool,
    explode: bool,
) {
    writer.u8(kind as u8);
    writer.u32(from);
    writer.u32(to);
    writer.u16(item);
    writer.u16(from_pt.x);
    writer.u16(from_pt.y);
    writer.u8(from_pt.z as u8);
    writer.u16(to_pt.x);
    writer.u16(to_pt.y);
    writer.u8(to_pt.z as u8);
    writer.u8(speed);
    writer.u8(duration);
    writer.u16(0x0000); // two reserved bytes ServUO zeroes
    writer.bool(fixed_direction);
    writer.bool(explode);
}

/// `0x70` — a graphical effect: a projectile, a strike, a fixed animation. 28 bytes.
///
/// `item` is the effect's art (a fireball graphic, a bolt). `from`/`to` are the
/// mobiles it links, `0` when a point is used instead. The uncoloured form; for a
/// tinted or particle effect use [`encode_hued_effect`].
#[allow(clippy::too_many_arguments)]
pub fn encode_graphical_effect(
    kind: EffectKind,
    from: u32,
    to: u32,
    item: u16,
    from_pt: EffectPoint,
    to_pt: EffectPoint,
    speed: u8,
    duration: u8,
    fixed_direction: bool,
    explode: bool,
) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(28);
    writer.u8(0x70);
    write_effect_body(
        &mut writer,
        kind,
        from,
        to,
        item,
        from_pt,
        to_pt,
        speed,
        duration,
        fixed_direction,
        explode,
    );
    writer.into_bytes()
}

/// `0xC0` — a hued graphical effect: [`encode_graphical_effect`] plus a colour
/// and a render mode. 36 bytes.
///
/// `hue` tints the effect art (a green poison bolt, a blue frost); `render_mode`
/// selects the client's blend (0 normal, higher values additive/translucent).
#[allow(clippy::too_many_arguments)]
pub fn encode_hued_effect(
    kind: EffectKind,
    from: u32,
    to: u32,
    item: u16,
    from_pt: EffectPoint,
    to_pt: EffectPoint,
    speed: u8,
    duration: u8,
    fixed_direction: bool,
    explode: bool,
    hue: u32,
    render_mode: u32,
) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(36);
    writer.u8(0xC0);
    write_effect_body(
        &mut writer,
        kind,
        from,
        to,
        item,
        from_pt,
        to_pt,
        speed,
        duration,
        fixed_direction,
        explode,
    );
    writer.u32(hue);
    writer.u32(render_mode);
    writer.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_sound_is_twelve_bytes_ported_from_servuo() {
        // 0x54, flag 1, sound, volume 0, x, y, z-as-short. A negative z must
        // sign-extend, not be truncated — a sound underground is otherwise placed
        // at z 65408 and silent.
        let packet = encode_play_sound(0x0028, 0x0568, 0x0640, -5);
        assert_eq!(packet.len(), 12);
        assert_eq!(packet[0], 0x54);
        assert_eq!(packet[1], 0x01);
        assert_eq!(&packet[2..4], &[0x00, 0x28], "sound id, big-endian");
        assert_eq!(&packet[4..6], &[0x00, 0x00], "volume");
        assert_eq!(&packet[6..8], &[0x05, 0x68], "x");
        assert_eq!(&packet[8..10], &[0x06, 0x40], "y");
        assert_eq!(&packet[10..12], &[0xFF, 0xFB], "z = -5 sign-extended");
    }

    #[test]
    fn classic_animation_inverts_forward_like_servuo() {
        // 0x6E, serial, action, frameCount, repeatCount, !forward, repeat, delay.
        let packet = encode_action(0x0000_1234, 0x000A, 0x0007, 0x0001, true, false, 0);
        assert_eq!(packet.len(), 14);
        assert_eq!(packet[0], 0x6E);
        assert_eq!(&packet[1..5], &[0x00, 0x00, 0x12, 0x34]);
        assert_eq!(&packet[5..7], &[0x00, 0x0A], "action");
        assert_eq!(&packet[7..9], &[0x00, 0x07], "frame count");
        assert_eq!(&packet[9..11], &[0x00, 0x01], "repeat count");
        assert_eq!(packet[11], 0x00, "forward=true writes reverse=0");
        assert_eq!(packet[12], 0x00, "repeat=false");
        assert_eq!(packet[13], 0x00, "delay");
    }

    #[test]
    fn new_animation_is_ten_bytes() {
        let packet = encode_new_action(0x0000_1234, 0x0005, 0x0009, 1);
        assert_eq!(packet.len(), 10);
        assert_eq!(packet[0], 0xE2);
        assert_eq!(&packet[1..5], &[0x00, 0x00, 0x12, 0x34]);
        assert_eq!(&packet[5..7], &[0x00, 0x05], "type");
        assert_eq!(&packet[7..9], &[0x00, 0x09], "action");
        assert_eq!(packet[9], 0x01, "delay");
    }

    #[test]
    fn graphical_effect_is_twenty_eight_bytes() {
        let from = EffectPoint {
            x: 0x0568,
            y: 0x0640,
            z: 0,
        };
        let to = EffectPoint {
            x: 0x0570,
            y: 0x0640,
            z: 0,
        };
        let packet = encode_graphical_effect(
            EffectKind::Moving,
            0x0000_0001,
            0x0000_0002,
            0x36D4,
            from,
            to,
            7,
            0,
            false,
            true,
        );
        assert_eq!(packet.len(), 28);
        assert_eq!(packet[0], 0x70);
        assert_eq!(packet[1], 0x00, "EffectKind::Moving");
        assert_eq!(&packet[2..6], &[0x00, 0x00, 0x00, 0x01], "from serial");
        assert_eq!(&packet[6..10], &[0x00, 0x00, 0x00, 0x02], "to serial");
        assert_eq!(&packet[10..12], &[0x36, 0xD4], "effect graphic");
        assert_eq!(packet[27], 0x01, "explode=true");
    }

    #[test]
    fn hued_effect_is_thirty_six_bytes_with_the_colour_last() {
        let pt = EffectPoint {
            x: 0x0568,
            y: 0x0640,
            z: 0,
        };
        let packet = encode_hued_effect(
            EffectKind::FixedFrom,
            0x0000_0001,
            0x0000_0000,
            0x373A,
            pt,
            pt,
            9,
            20,
            true,
            false,
            0x0026,
            0x0000_0001,
        );
        assert_eq!(packet.len(), 36);
        assert_eq!(packet[0], 0xC0);
        assert_eq!(packet[1], 0x03, "EffectKind::FixedFrom");
        assert_eq!(&packet[28..32], &[0x00, 0x00, 0x00, 0x26], "hue");
        assert_eq!(&packet[32..36], &[0x00, 0x00, 0x00, 0x01], "render mode");
    }
}
