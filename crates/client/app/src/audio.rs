//! The client-side audio mixer.
//!
//! The shard names effects and music, but never sends audio bytes.  Effects are
//! read from the installation's classic or UOP sound archive; music is found
//! under its `Music` directory. Failure to open an output device or one
//! optional asset leaves the world playable and is reported once, rather than
//! making a headless run or an incomplete client install fail at startup.

use std::collections::{
    HashMap,
    HashSet,
};
use std::path::{
    Path,
    PathBuf,
};
use std::time::{
    Duration,
    Instant,
};

use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::wire::{
    Graphic,
    SoundId,
};
use openshard_protocol::world::{
    MusicId,
    Point,
    Weather,
    WeatherChange,
};
use openshard_uofiles::anim::{
    BodyKind,
    is_ghost,
};

/// A locally inferred human footfall.
///
/// Footsteps are deliberately not server packets.  The classic protocol tells
/// a client where a mobile is, and ClassicUO turns its local walking state into
/// the two alternating shoe sounds.  Keeping the event shaped that way means a
/// predicted player step is heard immediately, while a stranger's is heard
/// when their movement update arrives.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Footstep {
    /// `None` is the offline player before a shard assigns a serial.
    pub who:     Option<Serial>,
    pub body:    Graphic,
    /// Which animation family the body belongs to, from the install's table.
    ///
    /// Carried on the event rather than derived here, because it is not
    /// derivable from the body id — see [`openshard_uofiles::mobtypes`] — and
    /// the table lives with the crowd, which is where the packet this event is
    /// made from is read.
    pub kind:    BodyKind,
    pub at:      Point,
    pub running: bool,
    pub mounted: bool,
    pub hidden:  bool,
    pub dead:    bool,
}

/// Owns the optional platform output and the installed client audio assets.
pub(crate) struct Audio {
    #[cfg(not(target_arch = "wasm32"))]
    native: Option<NativeAudio>,
}

impl Audio {
    /// Build the mixer without making sound a prerequisite for opening a map.
    pub(crate) fn open(client_dir: &Path, effects_volume: f32, music_volume: f32) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                native: NativeAudio::open(client_dir, effects_volume, music_volume),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (client_dir, effects_volume, music_volume);
            Self {}
        }
    }

    /// Route the two server-directed audio packets to their installed assets.
    pub(crate) fn play_packet(&mut self, packet: &ServerPacket, listener: Point) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(audio) = self.native.as_mut() {
            match packet {
                ServerPacket::PlaySound(sound) => audio.play_sound(sound.sound, sound.at, listener),
                ServerPacket::PlayMusic(music) => audio.play_music(music.track),
                ServerPacket::WeatherChange(change) => audio.set_weather(*change, listener),
                _ => {}
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (packet, listener);
        }
    }

    /// Play the client-owned sound made by a human completing a step.
    pub(crate) fn play_footstep(&mut self, step: Footstep, listener: Point) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(audio) = self.native.as_mut() {
            audio.play_footstep(step, listener);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (step, listener);
        }
    }

    /// Play the client-owned sound of opening or closing a classic container
    /// gump.  `0x24` carries only the gump art, so this belongs here rather
    /// than in the shard's world-sound packet stream.
    pub(crate) fn play_container_sound(&mut self, gump: Graphic, opening: bool, listener: Point) {
        let Some((open, close)) = container_sounds(gump) else {
            return;
        };
        self.play_ui_sound(if opening { open } else { close }, listener);
    }

    /// Play an unpositioned client UI effect at the listener's ear.
    pub(crate) fn play_ui_sound(&mut self, sound: SoundId, listener: Point) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(audio) = self.native.as_mut() {
            audio.play_sound(sound, listener, listener);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (sound, listener);
        }
    }

    /// Give the music its next turn, once a frame.
    ///
    /// A looping track has to be started again when it ends, and the mixer owns
    /// no clock to notice that for itself. The check is an atomic load against a
    /// source that is minutes long, so a frame is a generous place for it — and
    /// it is the frame that already owns everything else that advances.
    pub(crate) fn advance(&mut self, listener: Point) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(audio) = self.native.as_mut() {
            audio.repeat_finished_track();
            audio.advance_weather(listener);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = listener;
        }
    }

    /// Change the two independent mixer gains without restarting their current
    /// sources. The effect gain is applied as each short source is mixed; the
    /// music player changes its gain immediately.
    pub(crate) fn set_volumes(&mut self, effects: f32, music: f32) {
        let settings = crate::desk::Audio { effects, music }.clamped();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(audio) = self.native.as_mut() {
            audio.effect_volume = settings.effects;
            audio.music.set_volume(settings.music);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = settings;
        }
    }
}

/// ClassicUO's stock `ContainerManager` sounds.  Custom gumps deliberately
/// stay silent: ClassicUO's fallback record does too.
fn container_sounds(gump: Graphic) -> Option<(SoundId, SoundId)> {
    let sounds = match gump.0 {
        0x003C | 0x003D | 0x0103 | 0x775E | 0x7760 | 0x7762 => (0x0048, 0x0058),
        0x003E | 0x0048 | 0x004D | 0x0051 | 0x0104..=0x0107 | 0x010C..=0x010E => (0x002F, 0x002E),
        0x003F | 0x0041 | 0x0102 | 0x0108 => (0x004F, 0x0058),
        0x0040 | 0x0042..=0x0044 | 0x0049..=0x004C | 0x004E..=0x004F | 0x0109..=0x010B => (0x002D, 0x002C),
        0x2A63 => (0x0187, 0x01C9),
        _ => return None,
    };
    Some((SoundId(sounds.0), SoundId(sounds.1)))
}

/// ClassicUO alternates the three wind clips and the two thunder clips while
/// its weather simulation changes wind direction.
#[cfg(not(target_arch = "wasm32"))]
fn weather_sound(weather: Weather, index: usize) -> Option<SoundId> {
    let sounds: &[u16] = match weather {
        Weather::Snow => &[0x0014, 0x0015, 0x0016],
        Weather::StormBrewing | Weather::Storm => &[0x0028, 0x0206],
        Weather::Rain | Weather::Temperature | Weather::Clear => return None,
    };
    Some(SoundId(sounds[index % sounds.len()]))
}

/// ClassicUO picks a new wind direction every random 13–19 seconds. Cycling
/// that same interval range is repeatable for recordings while retaining the
/// cadence and avoiding a timer that fires in a silent clear sky.
#[cfg(not(target_arch = "wasm32"))]
fn weather_delay(index: usize) -> Duration {
    const SECONDS: [u64; 3] = [13, 19, 16];
    Duration::from_secs(SECONDS[index % SECONDS.len()])
}

/// Weather is heard from a random-looking place 10–18 tiles around the player
/// in ClassicUO. These fixed rotations preserve the audible distance while
/// keeping replay audio deterministic.
#[cfg(not(target_arch = "wasm32"))]
fn weather_point(listener: Point, index: usize) -> Point {
    const OFFSETS: [(i16, i16); 4] = [(10, 14), (-18, 11), (15, -13), (-12, -17)];
    let (x, y) = OFFSETS[index % OFFSETS.len()];
    Point::new(
        listener.x.saturating_add_signed(x),
        listener.y.saturating_add_signed(y),
        listener.z,
    )
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeAudio {
    output:         rodio::MixerDeviceSink,
    effects:        openshard_uofiles::sound::SoundArchive,
    music:          rodio::Player,
    tracks:         HashMap<String, PathBuf>,
    music_names:    HashMap<MusicId, Track>,
    /// The file to start again when the music player runs dry — `None` while
    /// nothing is playing, and while what is playing is a track the install
    /// marks as playing once.
    looping:        Option<PathBuf>,
    effect_volume:  f32,
    footsteps:      HashMap<Option<Serial>, FootstepState>,
    unheard:        HashSet<SoundId>,
    missing_tracks: HashSet<MusicId>,
    weather:        Option<WeatherAudio>,
}

/// A client-owned weather ambience. ClassicUO places these effects around the
/// player rather than waiting for a server `0x54` sound packet.
#[cfg(not(target_arch = "wasm32"))]
struct WeatherAudio {
    weather:      Weather,
    next:         Instant,
    sound_index:  usize,
    offset_index: usize,
}

/// The alternating sole of one walker.  It belongs to a mobile, not to the
/// mixer: two people walking together must not make each other skip a beat.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default)]
struct FootstepState {
    next:   Option<std::time::Instant>,
    offset: u16,
}

/// A track as an installation names it: the file, without its extension, and
/// whether it plays once or until something replaces it.
///
/// The flag is not decoration. Region music loops; a victory sting does not,
/// and a client that repeats one plays it over a player who has walked away.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Track {
    name:    String,
    looping: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeAudio {
    fn open(client_dir: &Path, effect_volume: f32, music_volume: f32) -> Option<Self> {
        let effects = match openshard_uofiles::sound::SoundArchive::open(client_dir) {
            Ok(effects) => effects,
            Err(error) => {
                eprintln!("audio disabled: opening sound files: {error}");
                return None;
            }
        };
        let mut output = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(output) => output,
            Err(error) => {
                eprintln!("audio disabled: opening default output: {error}");
                return None;
            }
        };
        // Dropping the stream at shutdown is ordinary, not a diagnostic.
        output.log_on_drop(false);
        let music = rodio::Player::connect_new(output.mixer());
        music.set_volume(music_volume);
        Some(Self {
            output,
            effects,
            music,
            tracks: music_tracks(client_dir),
            music_names: music_names(client_dir),
            looping: None,
            effect_volume,
            footsteps: HashMap::new(),
            unheard: HashSet::new(),
            missing_tracks: HashSet::new(),
            weather: None,
        })
    }

    fn play_sound(&mut self, id: SoundId, at: Point, listener: Point) {
        let sound = match self.effects.sound(id) {
            Ok(Some(sound)) => sound,
            Ok(None) => {
                if self.unheard.insert(id) {
                    eprintln!("audio: sound {:04X} is absent from this install", id.0);
                }
                return;
            }
            Err(error) => {
                if self.unheard.insert(id) {
                    eprintln!("audio: cannot read sound {:04X}: {error}", id.0);
                }
                return;
            }
        };
        let [left, right] = world_volume(self.effect_volume, at, listener);
        if left == 0.0 && right == 0.0 {
            return;
        }
        let source = rodio::buffer::SamplesBuffer::new(sound.channels, sound.sample_rate, sound.samples);
        // Rodio's `Spatial` uses inverse-square attenuation.  Its coordinates
        // were tile coordinates here, so an effect ten tiles away was 1/100 as
        // loud and most sounds seemed absent.  ClassicUO instead fades linearly
        // over its 18-tile view.
        self.output
            .mixer()
            .add(rodio::source::ChannelVolume::new(source, vec![left, right]));
    }

    fn play_footstep(&mut self, step: Footstep, listener: Point) {
        if step.kind != BodyKind::Human || step.hidden || step.dead || is_ghost(step.body) {
            return;
        }
        let now = std::time::Instant::now();
        let state = self.footsteps.entry(step.who).or_default();
        if state.next.is_some_and(|next| now < next) {
            return;
        }
        let (sound, delay) = footstep_sound(state, step);
        state.next = Some(now + delay);
        self.play_sound(sound, step.at, listener);
    }

    fn set_weather(&mut self, change: WeatherChange, listener: Point) {
        let Some(sound) = weather_sound(change.weather, 0) else {
            self.weather = None;
            return;
        };
        if self
            .weather
            .as_ref()
            .is_some_and(|state| state.weather == change.weather)
        {
            return;
        }
        let now = Instant::now();
        self.weather = Some(WeatherAudio {
            weather:      change.weather,
            next:         now + weather_delay(0),
            sound_index:  1,
            offset_index: 1,
        });
        self.play_sound(sound, weather_point(listener, 0), listener);
    }

    fn advance_weather(&mut self, listener: Point) {
        let now = Instant::now();
        let Some(state) = self.weather.as_mut().filter(|state| state.next <= now) else {
            return;
        };
        let sound = weather_sound(state.weather, state.sound_index)
            .expect("only sounding weather is stored in WeatherAudio");
        let at = weather_point(listener, state.offset_index);
        state.next = now + weather_delay(state.sound_index);
        state.sound_index += 1;
        state.offset_index += 1;
        self.play_sound(sound, at, listener);
    }

    fn play_music(&mut self, track: MusicId) {
        let Some((path, looping)) = self.resolve(track) else {
            if self.missing_tracks.insert(track) {
                eprintln!("audio: music track {} is absent from this install", track.0);
            }
            return;
        };
        let Some(source) = decode(&path) else {
            return;
        };
        start_track(&self.music, source);
        // Remembered rather than wrapped in a repeating source: see
        // `repeat_finished_track`, and the trap written above it.
        self.looping = looping.then_some(path);
    }

    /// Which file this shard's track id names here, and whether it repeats.
    ///
    /// Three answers in the order they are trusted: what the installation's own
    /// config says, a file named after the id itself — which is how a pack ships
    /// music of its own without a protocol for it — and finally the classic
    /// table, so an install with no config still plays what every client has
    /// played since 1997.
    fn resolve(&self, track: MusicId) -> Option<(PathBuf, bool)> {
        let named = self
            .music_names
            .get(&track)
            .and_then(|entry| Some((self.tracks.get(&entry.name)?.clone(), entry.looping)));
        named
            .or_else(|| {
                // Nothing states whether a pack's own track repeats. Region
                // music is the overwhelming majority of what a shard sends, and
                // a region left silent after three minutes is the worse of the
                // two mistakes, so it repeats.
                numeric_names(track)
                    .iter()
                    .find_map(|name| self.tracks.get(name).cloned())
                    .map(|path| (path, true))
            })
            .or_else(|| {
                let entry = classic_track(track)?;
                Some((self.tracks.get(&entry.name)?.clone(), entry.looping))
            })
    }

    /// Start a looping track over once it has played to its end.
    ///
    /// The loop is here, and not in `Source::repeat_infinite`, because that
    /// wraps the track in rodio's `Buffered`, and `Buffered` asks a source how
    /// long its current span is *before* pulling a sample from it. A freshly
    /// opened Symphonia decoder answers `Some(0)` — it has not read a packet
    /// yet — which `Buffered` reads as a stream that has already ended. The
    /// repeat is then an infinity of silence: the player reports a queued track,
    /// playing, at full volume, and the device receives zeroes. Priming the
    /// decoder with one sample would dodge it, and would leave the silence one
    /// upstream change away from coming back.
    fn repeat_finished_track(&mut self) {
        if !self.music.empty() {
            return;
        }
        let Some(path) = self.looping.clone() else {
            return;
        };
        let Some(source) = decode(&path) else {
            // A track that has stopped decoding will not start doing so on the
            // next frame, and a message every frame is not a diagnostic.
            self.looping = None;
            return;
        };
        self.music.append(source);
        self.music.play();
    }
}

/// Open and decode a music file, reporting the failure once at the seam that
/// knows the path.
#[cfg(not(target_arch = "wasm32"))]
fn decode(path: &Path) -> Option<rodio::Decoder<std::io::BufReader<std::fs::File>>> {
    match std::fs::File::open(path)
        .and_then(|file| rodio::Decoder::try_from(file).map_err(std::io::Error::other))
    {
        Ok(source) => Some(source),
        Err(error) => {
            eprintln!("audio: cannot play {}: {error}", path.display());
            None
        }
    }
}

/// Replace whatever the music player is playing with `source`.
///
/// The three calls are one operation, and the order is not a style choice.
/// `Player::clear` *pauses* the player as well as emptying it, and `append`
/// lifts only the stopped flag — never the paused one. A track handed over
/// without the closing `play` therefore queues itself behind a pause nothing
/// ever lifts: the very first `0x6D` of a session silences music for the rest
/// of it, with no error anywhere to say so, because every layer below did
/// exactly what it was asked.
#[cfg(not(target_arch = "wasm32"))]
fn start_track(player: &rodio::Player, source: impl rodio::Source + Send + 'static) {
    player.clear();
    player.append(source);
    player.play();
}

#[cfg(not(target_arch = "wasm32"))]
fn world_volume(effect_volume: f32, at: Point, listener: Point) -> [f32; 2] {
    // ClassicUO's `AudioManager.PlaySoundWithDistance`: the view is 18 tiles,
    // and the falloff is linear, including along a diagonal (Chebyshev range).
    const VIEW_RANGE: u16 = 18;
    let dx = i32::from(at.x) - i32::from(listener.x);
    let dy = i32::from(at.y) - i32::from(listener.y);
    let distance = dx.unsigned_abs().max(dy.unsigned_abs()) as u16;
    let gain = effect_volume * (1.0 - f32::from(distance) / f32::from(VIEW_RANGE + 1));
    if distance > VIEW_RANGE || gain <= 0.0 {
        return [0.0, 0.0];
    }
    // ClassicUO leaves effects mono; distance changes their volume but not
    // their balance.
    [gain, gain]
}

/// ClassicUO's footstep selection and its 1.3x cadence, separated from device
/// time so the table remains testable without an audio output.
#[cfg(not(target_arch = "wasm32"))]
fn footstep_sound(state: &mut FootstepState, step: Footstep) -> (SoundId, std::time::Duration) {
    if step.mounted && step.running {
        (SoundId(0x0129), std::time::Duration::from_millis(195))
    } else if step.mounted {
        (SoundId(0x012B), std::time::Duration::from_millis(455))
    } else {
        let sound = SoundId(0x012B + state.offset);
        state.offset = (state.offset + 1) % 2;
        (sound, std::time::Duration::from_millis(520))
    }
}

/// Collect native music files once, accepting the capitalisation and file type
/// variants that UO installations have shipped over the years.
#[cfg(not(target_arch = "wasm32"))]
fn music_tracks(client_dir: &Path) -> HashMap<String, PathBuf> {
    let mut tracks = HashMap::new();
    for dir in [
        client_dir.join("Music"),
        client_dir.join("music"),
        client_dir.join("MUSIC"),
    ] {
        collect_music_tracks(&dir, &mut tracks);
    }
    tracks
}

/// The install owns the mapping from a wire music id to a filename.  Read its
/// configuration first so a shard with patched music does not get silently
/// redirected to one of the stock track names.
#[cfg(not(target_arch = "wasm32"))]
fn music_names(client_dir: &Path) -> HashMap<MusicId, Track> {
    let config = [
        client_dir.join("Music/Digital/Config.txt"),
        client_dir.join("Music/Config.txt"),
        client_dir.join("music/digital/config.txt"),
        client_dir.join("music/config.txt"),
    ]
    .into_iter()
    .find(|path| path.is_file());
    let Some(config) = config else {
        return HashMap::new();
    };
    let Ok(contents) = std::fs::read_to_string(config) else {
        return HashMap::new();
    };
    contents.lines().filter_map(music_line).collect()
}

/// One `Config.txt` line: an id, a filename, and the word `loop` when the track
/// is meant to play until something else replaces it — `9 britainpos,loop`.
///
/// The separators are all three the file has been seen to use, and a line that
/// does not begin with a number is not an entry.
#[cfg(not(target_arch = "wasm32"))]
fn music_line(line: &str) -> Option<(MusicId, Track)> {
    let mut fields = line.split([' ', ',', '\t']).filter(|field| !field.is_empty());
    let id = fields.next()?.parse().ok()?;
    let name = Path::new(fields.next()?)
        .file_stem()?
        .to_str()?
        .to_ascii_lowercase();
    let looping = fields.any(|field| field.eq_ignore_ascii_case("loop"));
    Some((MusicId(id), Track { name, looping }))
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_music_tracks(dir: &Path, tracks: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_music_tracks(&path, tracks);
            continue;
        }
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if !matches!(
            extension.to_ascii_lowercase().as_str(),
            "mp3" | "ogg" | "flac" | "wav"
        ) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            tracks.entry(stem.to_ascii_lowercase()).or_insert(path);
        }
    }
}

/// A file named after the id itself, in the two spellings a pack has been seen
/// to use. Tried before the classic table so a shard can ship its own music
/// without the client needing a protocol for it.
#[cfg(not(target_arch = "wasm32"))]
fn numeric_names(track: MusicId) -> [String; 2] {
    [track.0.to_string(), format!("{:02}", track.0)]
}

/// What every install has played since 1997, for one that ships no config.
///
/// Names and loop flags both come from the reference's own fallback table
/// (`ClassicUO.Assets/SoundsLoader.cs`), because the flag is per track and not
/// per kind: `britain1` repeats, `victory` plays once, and guessing either way
/// gets one of them wrong.
#[cfg(not(target_arch = "wasm32"))]
fn classic_track(track: MusicId) -> Option<Track> {
    const CLASSIC: &[(&str, bool)] = &[
        ("oldult01", true),
        ("create1", false),
        ("dragflit", false),
        ("oldult02", true),
        ("oldult03", true),
        ("oldult04", true),
        ("oldult05", true),
        ("oldult06", true),
        ("stones2", true),
        ("britain1", true),
        ("britain2", true),
        ("bucsden", true),
        ("jhelom", false),
        ("lbcastle", false),
        ("linelle", false),
        ("magincia", true),
        ("minoc", true),
        ("ocllo", true),
        ("samlethe", false),
        ("serpents", true),
        ("skarabra", true),
        ("trinsic", true),
        ("vesper", true),
        ("wind", true),
        ("yew", true),
        ("cave01", false),
        ("dungeon9", false),
        ("forest_a", false),
        ("intown01", false),
        ("jungle_a", false),
        ("mountn_a", false),
        ("plains_a", false),
        ("sailing", false),
        ("swamp_a", false),
        ("tavern01", false),
        ("tavern02", false),
        ("tavern03", false),
        ("tavern04", false),
        ("combat1", false),
        ("combat2", false),
        ("combat3", false),
        ("approach", false),
        ("death", false),
        ("victory", false),
        ("btcastle", false),
        ("nujelm", true),
        ("dungeon2", false),
        ("cove", true),
        ("moonglow", true),
        ("zento", true),
        ("tokunodungeon", true),
        ("taiko", true),
        ("dreadhornarea", true),
        ("elfcity", true),
        ("grizzledungeon", true),
        ("melisandeslair", true),
        ("paroxysmuslair", true),
        ("gwennoconversation", true),
        ("goodendgame", true),
        ("goodvsevil", true),
        ("greatearthserpents", true),
        ("humanoids_u9", true),
        ("minocnegative", true),
        ("paws", true),
        ("selimsbar", true),
        ("serpentislecombat_u7", true),
        ("valoriaships", true),
    ];
    let (name, looping) = CLASSIC.get(usize::from(track.0))?;
    Some(Track {
        name:    (*name).to_owned(),
        looping: *looping,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::num::{
        NonZeroU16,
        NonZeroU32,
    };

    use openshard_protocol::wire::{
        Graphic,
        SoundId,
    };
    use openshard_protocol::world::{
        MusicId,
        Point,
    };
    use openshard_uofiles::anim::BodyKind;

    /// A short buffer of silence — enough to be a source, and nothing is ever
    /// asked to play it.
    fn silence() -> rodio::buffer::SamplesBuffer {
        rodio::buffer::SamplesBuffer::new(
            NonZeroU16::new(1).expect("one channel"),
            NonZeroU32::new(22050).expect("the classic rate"),
            vec![0.0; 32],
        )
    }

    /// A source shaped like a freshly opened decoder: it cannot say how long
    /// its current span is until it has decoded something, so it answers
    /// `Some(0)` until the first sample has been pulled.
    ///
    /// That shape is the whole trap. `Source::repeat_infinite` buffers, and
    /// `Buffered` reads `Some(0)` as a stream that has already ended, so the
    /// samples below never reach the queue at all.
    #[derive(Default)]
    struct UnreadDecoder {
        produced: usize,
    }

    /// Long enough to outlast the queue's own silence, short enough to be free.
    const DECODED_SAMPLES: usize = 512;

    impl Iterator for UnreadDecoder {
        type Item = rodio::Sample;

        fn next(&mut self) -> Option<Self::Item> {
            (self.produced < DECODED_SAMPLES).then(|| {
                self.produced += 1;
                0.5
            })
        }
    }

    impl rodio::Source for UnreadDecoder {
        fn current_span_len(&self) -> Option<usize> {
            // Nothing decoded yet, so nothing is known about the span — which
            // is the same `Some(0)` a real decoder answers, and is not the same
            // statement as "this stream has ended", though `Buffered` reads it
            // as one.
            match self.produced {
                0 => Some(0),
                produced => Some(DECODED_SAMPLES - produced),
            }
        }

        fn channels(&self) -> rodio::ChannelCount {
            NonZeroU16::new(1).expect("one channel")
        }

        fn sample_rate(&self) -> rodio::SampleRate {
            NonZeroU32::new(22050).expect("the classic rate")
        }

        fn total_duration(&self) -> Option<std::time::Duration> {
            None
        }
    }

    /// What the player is handed has to arrive at the mixer as sound.
    ///
    /// The silence this catches had every symptom of working: a queued track, a
    /// player that is not paused, a volume of 0.45 and a device stream running.
    /// Only the samples were missing, so only the samples are asserted.
    #[test]
    fn a_track_reaches_the_queue_as_a_signal() {
        let (player, queue) = rodio::Player::new();
        super::start_track(&player, UnreadDecoder::default());
        let peak = queue
            .take(DECODED_SAMPLES * 4)
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak > 0.0, "the queue produced silence: peak {peak}");
    }

    /// The same claim against the installed music, which is the only place the
    /// real decoder can be exercised. Ignored by default: it needs a client.
    #[test]
    #[ignore = "needs an installed client — set OPENSHARD_CLIENT"]
    fn the_installed_track_reaches_the_queue_as_a_signal() {
        let dir = std::env::var("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT names an install");
        let path = std::path::Path::new(&dir).join("Music/Digital/Britainpos.mp3");
        let source = super::decode(&path).expect("the installed track decodes");
        let (player, queue) = rodio::Player::new();
        super::start_track(&player, source);
        let peak = queue
            .take(200_000)
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak > 0.001, "the queue produced silence: peak {peak}");
    }

    /// The loop flag is per track and decides whether the client starts it
    /// again, so it has to survive the line it is written on.
    #[test]
    fn a_config_line_carries_its_loop_flag() {
        assert_eq!(
            super::music_line("9 britainpos,loop"),
            Some((
                MusicId(9),
                super::Track {
                    name:    "britainpos".to_owned(),
                    looping: true,
                }
            ))
        );
        assert_eq!(
            super::music_line("10 britain1"),
            Some((
                MusicId(10),
                super::Track {
                    name:    "britain1".to_owned(),
                    looping: false,
                }
            ))
        );
        assert_eq!(super::music_line(""), None);
        assert_eq!(super::music_line("; a comment"), None);
    }

    /// The regression that made every session silent: `Player::clear` pauses,
    /// so a track appended after it never plays unless the player is resumed.
    ///
    /// `Player::new` builds the queue without touching a device, which is what
    /// lets the trap be caught on a machine with no sound card at all — the
    /// condition the whole mixer was written to keep.
    #[test]
    fn a_started_track_is_not_left_paused() {
        let (player, _queue) = rodio::Player::new();
        super::start_track(&player, silence());
        assert!(
            !player.is_paused(),
            "the music player is paused by `clear` and must be resumed after the track is queued"
        );
    }

    #[test]
    fn world_effects_fade_linearly_through_the_view() {
        let listener = Point::new(100, 100, 0);
        let here = super::world_volume(0.8, listener, listener);
        let far = super::world_volume(0.8, Point::new(118, 100, 0), listener);
        let absent = super::world_volume(0.8, Point::new(119, 100, 0), listener);
        assert_eq!(here, [0.8, 0.8]);
        assert!(
            far[0] > 0.02 && far[1] > 0.02,
            "the edge of sight is still audible"
        );
        assert_eq!(absent, [0.0, 0.0]);
    }

    #[test]
    fn classic_container_gumps_keep_their_open_and_close_sounds() {
        assert_eq!(
            super::container_sounds(Graphic(0x003C)),
            Some((SoundId(0x0048), SoundId(0x0058))),
            "the classic backpack opens and closes audibly"
        );
        assert_eq!(
            super::container_sounds(Graphic(0x003E)),
            Some((SoundId(0x002F), SoundId(0x002E))),
            "a chest keeps its own latch sounds"
        );
        assert_eq!(super::container_sounds(Graphic(0x9999)), None);
    }

    #[test]
    fn classic_weather_uses_wind_for_snow_and_thunder_for_storms() {
        use openshard_protocol::world::Weather;

        assert_eq!(super::weather_sound(Weather::Snow, 0), Some(SoundId(0x0014)));
        assert_eq!(super::weather_sound(Weather::Snow, 2), Some(SoundId(0x0016)));
        assert_eq!(
            super::weather_sound(Weather::StormBrewing, 1),
            Some(SoundId(0x0206))
        );
        assert_eq!(super::weather_sound(Weather::Storm, 0), Some(SoundId(0x0028)));
        assert_eq!(super::weather_sound(Weather::Rain, 0), None);
        assert_eq!(super::weather_delay(0), std::time::Duration::from_secs(13));
        assert_eq!(super::weather_delay(1), std::time::Duration::from_secs(19));
        let here = Point::new(100, 100, 0);
        let at = super::weather_point(here, 1);
        assert_eq!((at.x.abs_diff(here.x), at.y.abs_diff(here.y)), (18, 11));
    }

    #[test]
    fn classic_footsteps_alternate_and_keep_their_mounted_cadence() {
        let step = |mounted, running| {
            super::Footstep {
                who: None,
                body: Graphic(400),
                kind: BodyKind::Human,
                at: Point::new(0, 0, 0),
                running,
                mounted,
                hidden: false,
                dead: false,
            }
        };
        let mut state = super::FootstepState::default();
        assert_eq!(
            super::footstep_sound(&mut state, step(false, false)),
            (SoundId(0x012B), std::time::Duration::from_millis(520))
        );
        assert_eq!(
            super::footstep_sound(&mut state, step(false, true)),
            (SoundId(0x012C), std::time::Duration::from_millis(520))
        );
        assert_eq!(
            super::footstep_sound(&mut state, step(true, false)),
            (SoundId(0x012B), std::time::Duration::from_millis(455))
        );
        assert_eq!(
            super::footstep_sound(&mut state, step(true, true)),
            (SoundId(0x0129), std::time::Duration::from_millis(195))
        );
    }
}
