//! Line-oriented trace for diagnosing prediction/presentation drift.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use openshard_client_render::camera::{Camera, RealPoint, WorldPoint};
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::Point;

use crate::world::{MotionSnapshot, WorldState};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Debug)]
struct Snapshot {
    view: Option<Point>,
    motion: MotionSnapshot,
    camera_eye: WorldPoint,
    body_screen: RealPoint,
}

pub(crate) struct MovementTrace {
    file: File,
    last: Option<Snapshot>,
    pid: u32,
    session: String,
    event_id: u64,
}

impl MovementTrace {
    pub(crate) fn open() -> Option<Self> {
        let path = std::path::PathBuf::from(std::env::var_os("OPENSHARD_MOVEMENT_TRACE")?);
        Self::open_at(&path)
    }

    fn open_at(path: &Path) -> Option<Self> {
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_millis());
                let pid = std::process::id();
                let session = session_id(pid, timestamp);
                let _ = writeln!(
                    file,
                    "{timestamp} pid={pid} session={session} event_id=0 source=session event=session"
                );
                let _ = file.flush();
                Some(Self {
                    file,
                    last: None,
                    pid,
                    session,
                    event_id: 0,
                })
            }
            Err(error) => {
                eprintln!("movement trace disabled: cannot open {}: {error}", path.display());
                None
            }
        }
    }

    pub(crate) fn record(&mut self, event: &str, world: &WorldState, camera: &Camera) {
        self.record_detail(event, "", world, camera);
    }

    pub(crate) fn record_detail(&mut self, event: &str, detail: &str, world: &WorldState, camera: &Camera) {
        let me = world.me();
        let motion = world.motion.snapshot();
        let snapshot = Snapshot {
            view: world.authoritative.view.as_ref().map(|view| view.player.position),
            camera_eye: camera.eye_at(),
            body_screen: camera.to_viewport_exact(camera.to_view_exact(motion.rendered.eye())),
            motion,
        };
        if event == "frame" && self.last == Some(snapshot) {
            return;
        }
        self.last = Some(snapshot);
        self.event_id += 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let _ = writeln!(
            self.file,
            "{timestamp} pid={} session={} event_id={} source={event} event={event} {detail} me={} authoritative={} confirmed={} predicted={} route_origin={} pending={} transition_from={} transition_to={} rendered_world={} camera_eye={} body_screen={}",
            self.pid,
            self.session,
            self.event_id,
            me.map_or_else(|| "-".to_owned(), |serial| format!("0x{:08X}", serial.raw())),
            optional_point(snapshot.view),
            point(snapshot.motion.confirmed.position),
            point(snapshot.motion.predicted.position),
            point(snapshot.motion.route_origin),
            snapshot.motion.pending_steps,
            optional_point(snapshot.motion.transition.map(|(from, _)| from)),
            optional_point(snapshot.motion.transition.map(|(_, to)| to)),
            gaze(snapshot.motion.rendered),
            world_point(snapshot.camera_eye),
            real_point(snapshot.body_screen),
        );
        let _ = self.file.flush();
    }
}

fn session_id(pid: u32, timestamp: u128) -> String {
    let serial = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    format!("{pid}-{timestamp}-{serial}")
}

pub(crate) fn packet_kind(packet: &ServerPacket) -> &'static str {
    match packet {
        ServerPacket::WalkAck(_) => "WalkAck",
        ServerPacket::WalkReject(_) => "WalkReject",
        ServerPacket::PlayerUpdate(_) => "PlayerUpdate",
        ServerPacket::PlayerStart(_) => "PlayerStart",
        ServerPacket::Animation(_) => "Animation",
        ServerPacket::NewAnimation(_) => "NewAnimation",
        ServerPacket::SwingTiming(_) => "SwingTiming",
        ServerPacket::OpenContainer(_) => "OpenContainer",
        ServerPacket::AddToContainer(_) => "AddToContainer",
        ServerPacket::ContainerContents(_) => "ContainerContents",
        ServerPacket::BuyList(_) => "BuyList",
        ServerPacket::SellList(_) => "SellList",
        ServerPacket::OpenPaperdoll(_) => "OpenPaperdoll",
        ServerPacket::SpokenMessage(_) => "SpokenMessage",
        ServerPacket::LocalizedMessage(_) => "LocalizedMessage",
        ServerPacket::MobileMove(_) => "MobileMove",
        ServerPacket::MobileIncoming(_) => "MobileIncoming",
        ServerPacket::Remove(_) => "Remove",
        ServerPacket::WorldItem(_) => "WorldItem",
        ServerPacket::EquipUpdate(_) => "EquipUpdate",
        _ => "Other",
    }
}

pub(crate) fn point(value: Point) -> String {
    format!("({}, {}, {})", value.x, value.y, value.z)
}

fn optional_point(value: Option<Point>) -> String {
    value.map_or_else(|| "-".to_owned(), point)
}

fn gaze(value: openshard_client_render::follow::Gaze) -> String {
    format!("({:.2}, {:.2}, {:.2})", value.x, value.y, value.lift)
}

fn world_point(value: WorldPoint) -> String {
    format!("({:.2}, {:.2})", value.x, value.y)
}

fn real_point(value: RealPoint) -> String {
    format!("({:.2}, {:.2})", value.x, value.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_identifier_distinguishes_two_clients_in_one_process() {
        let first = session_id(42, 7);
        let second = session_id(42, 7);
        assert_ne!(first, second);
        assert!(first.starts_with("42-7-"));
    }

    #[test]
    fn shared_trace_file_attributes_each_session() {
        let path = std::env::temp_dir().join(format!(
            "openshard-movement-trace-{}-{}.log",
            std::process::id(),
            session_id(std::process::id(), 0)
        ));
        let first = MovementTrace::open_at(&path).expect("first client opens the shared trace");
        let second = MovementTrace::open_at(&path).expect("second client opens the shared trace");
        let first_session = first.session.clone();
        let second_session = second.session.clone();
        drop((first, second));

        let trace = std::fs::read_to_string(&path).expect("trace headers were written");
        let lines: Vec<_> = trace.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_ne!(first_session, second_session);
        for line in lines {
            assert!(line.contains("pid="));
            assert!(line.contains("session="));
            assert!(line.contains("event_id="));
        }
        std::fs::remove_file(path).expect("the test removes only its own trace");
    }
}
