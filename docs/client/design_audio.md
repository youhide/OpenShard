# Sound: two packets, the player's own archive, and one mixer

The shard's `0x54` and `0x6D` reaching a device, read out of the player's own
installation — the UOP archive or the `.mul` pair, the music config's per-track
loop flag, and the two stacked silences that each reported success while the
client played nothing.

Status and what is left are [`README.md`](README.md).

## M6 — sound

Built. The shard's two audio packets reach a device: `0x54 PlaySound` and
`0x6D PlayMusic` both decode, `crates/client/app/src/audio.rs` is the mixer, and
what it plays comes out of the player's own installation. The rule this
milestone existed for — **every visible action plays a sound and an animation,
not just a state change** — is honoured on the sound half; the picture half is
in the backlog below, which is the opposite of the order this was planned in.

- **The reader is `crates/common/uofiles/src/sound.rs`**, where the plan put it:
  the shard picks sound ids and may one day want to know an id exists. It opens
  `soundLegacyMUL.uop` under `build/soundlegacymul/{:08}.dat` when the install
  has one and falls back to the `soundidx.mul`/`sound.mul` pair, strips the
  40-byte name, and reads what is left as 22050 Hz mono 16-bit — while still
  recognising a RIFF/WAVE wrapper when there is one, because a rate read from a
  file beats a rate assumed about it. `UOSound.Delay` was never ported and its
  arithmetic never repeated: a length here is a sample count over a rate.
- **Music is not in the archive**, and is read as the plan said: the id → name
  map out of `Music/Digital/Config.txt`, the files themselves out of `Music/` —
  any capitalisation, any depth, mp3, ogg, flac or wav. `Music/Config.txt` is
  tried as well rather than being chosen by version, so the 4.0.1.1c boundary
  costs no `Feature::since` here. An install with no config at all falls back to
  the 67 classic names, and a numbered track is found by its id, which leaves a
  pack free to ship its own music without teaching the client a new protocol.
- **A track's third field is `loop`, and it is per track.** `9 britainpos,loop`
  repeats; `10 britain1` plays once. The fallback table carries the same flag
  per entry, from the reference's own (`SoundsLoader.cs`): `britain1` loops,
  `victory` does not. Reading the name and dropping the flag is how a victory
  sting ends up playing over a player who has walked away — so the flag is read,
  and the loop is the client restarting the track rather than a repeating
  source. See the second trap below for why that distinction is not stylistic.
- **The mixer is optional, not a trait with a null sink.** The property the plan
  wanted is the one that matters and it holds — nothing in the test tree ever
  asks for a device — but the shape is an `Option<NativeAudio>` decided once at
  startup rather than an implementation chosen where the renderer is: a missing
  output or an install with no sound archive prints one line and leaves the
  world playable. Under `wasm32` the whole of it compiles away.
- **Two gains, remembered.** `desk::Audio { effects, music }` — sliders on the
  HUD's Audio tab, persisted beside the light tuning and the window frame, for
  the same reason: someone who has turned the music down should not have to find
  the slider again every launch.
- **The shard's half is the region crossing.** `regions.rs::start_music` sends
  `0x6D` when a player crosses into a region that names a track, and refuses to
  re-send the track already playing, because `0x6D` *restarts* one rather than
  continuing it. 38 of the 128 saved regions carry a track.

**A silence with nothing wrong under it.** The first version of this played
nothing at all while every layer reported success: the packet decoded, the
config parsed, the file was found, the mp3 decoded, the volume was 0.45.
`rodio::Player::clear` *pauses* the player as well as emptying it, and `append`
lifts only the stopped flag — never the paused one — so the first track of a
session queued itself behind a pause nothing ever lifted and the client stayed
mute for the rest of the run. Clear, append and play are now one function,
`start_track`, with the reason written above it; and the test that holds it needs
no sound card, because `Player::new` builds the queue without a device. That is
the same property the null sink was for, arriving where the bug actually was.

**And behind it, the same silence again, from the other side.** With the pause
lifted the player reported a queued track, playing, at 0.45, and the device
stream ran — carrying zeroes. `Source::repeat_infinite` wraps a source in
rodio's `Buffered`, and `Buffered` asks how long the current span is *before*
pulling a sample; a freshly opened Symphonia decoder answers `Some(0)`, having
read no packet yet, and `Buffered` reads that as a stream that has already
ended. What `repeat_infinite` returns for a whole mp3 is therefore an infinity
of silence — and every symptom points at the device, which is the one part that
was working. **Two independent bugs with the identical symptom, stacked**: fixing
the first changed nothing a person could hear, which is exactly the shape that
makes a fix look wrong when it was right.

The loop is now the client's own — the track is remembered and started again
when the player runs dry, once a frame in `advance` — so nothing depends on
`Buffered`'s reading of a span it was never told. Priming the decoder with one
sample would also have worked, and would have left the silence one upstream
change away from returning. Two device-free tests hold it: one hands
`start_track` a source shaped like an unread decoder (`Some(0)` until its first
sample) and asserts the queue carries signal, and an `#[ignore]`d one does the
same through the real installed mp3 — `cargo test -p openshard-client-app --lib
audio -- --ignored`, with `OPENSHARD_CLIENT` set.

**How it was actually caught**, since none of the above is visible from the
code: a headless `sway`, the playground pointed at a dedicated PulseAudio null
sink (`PULSE_SINK`), and `parec` on that sink's monitor. Peak 0 is a fact; "I
hear nothing" is a report. The same measurement afterwards reads RMS 1221,
peak 5750 — and that difference is the whole verification, because every other
signal in the system said the music was playing the entire time.

