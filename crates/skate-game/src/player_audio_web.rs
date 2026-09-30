//! Web player audio. The desktop build runs the retail audio runtime on an
//! emulated guest, which needs the x86_64-exact graph and the retail banks; the
//! web build has neither. Here a small mixer plays the clean-room sound set
//! (`<assets>/clean-audio/sounds.json`, mono 16-bit WAVs regenerated from coarse
//! descriptors) from the same per-tick observations the desktop worker reads:
//! surface loops follow wheel speed, and landings, pops, bails, footsteps,
//! pushes, flips and front-end cues fire one-shots.
use crate::skate_audio::{FrontEndSoundRequest, LivePcm, PlayerAudioObservation};
use bevy::audio::Volume;
use bevy::prelude::*;
use skate_core::physics::phase::PhysicsEvent;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// Output kept this far ahead of the device, in seconds.
const LEAD: f32 = 0.10;
const MAX_ONE_SHOTS: usize = 24;

#[derive(Resource)]
pub(crate) struct PlayerAudioAssets(pub PathBuf);

pub(super) fn install(app: &mut App) {
    app.add_message::<FrontEndSoundRequest>()
        .add_systems(Startup, start)
        .add_systems(Update, forward);
}

/// Every multiplier step past 1x gets the one front-end cue this set has.
pub(crate) fn multiplier_sound_id(multiplier: f32) -> Option<u64> {
    (multiplier > 1.0).then_some(1)
}

struct Sample {
    data: Arc<[f32]>,
    rate: u32,
    gain: f32,
}

struct Voice {
    sample: Arc<Sample>,
    position: f64,
    pitch: f32,
    gain: f32,
}

/// A looping bed whose level and pitch follow the game state.
struct Bed {
    voice: Voice,
    target_gain: f32,
    target_pitch: f32,
}

#[derive(Default)]
struct Edges {
    grounded: bool,
    /// Physics tick of the last take-off (60 Hz).
    takeoff_tick: u64,
    pushing: bool,
    trick: Option<String>,
    started: bool,
}

#[derive(Resource)]
struct WebPlayerAudio {
    live: LivePcm,
    samples: BTreeMap<String, Arc<Sample>>,
    beds: BTreeMap<&'static str, Bed>,
    one_shots: Vec<Voice>,
    edges: Edges,
    seed: u32,
    scratch: Vec<f32>,
}

fn wav(bytes: &[u8]) -> Result<(Vec<f32>, u32), String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV file".into());
    }
    let (mut at, mut format) = (12, None);
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(at + 4)) {
        let body = at + 8;
        match id {
            b"fmt " => {
                format = Some((u16_at(body).ok_or("fmt")?, u16_at(body + 2).ok_or("fmt")?,
                    u32_at(body + 4).ok_or("fmt")?, u16_at(body + 14).ok_or("fmt")?));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.ok_or("WAV data before fmt")?;
                if tag != 1 || bits != 16 || channels == 0 {
                    return Err("WAV must be 16-bit PCM".into());
                }
                let data = bytes.get(body..body + len as usize).ok_or("truncated WAV")?;
                let frames = data.chunks_exact(2 * channels as usize);
                // Mono: the mixer pans nothing, and the set is authored mono.
                let mono = frames
                    .map(|f| f.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32).sum::<f32>()
                        / (32768.0 * channels as f32))
                    .collect();
                return Ok((mono, rate));
            }
            _ => {}
        }
        at = body + len as usize + (len as usize & 1);
    }
    Err("WAV has no data chunk".into())
}

fn load(root: &std::path::Path) -> Result<BTreeMap<String, Arc<Sample>>, String> {
    let dir = root.join("clean-audio");
    let manifest: serde_json::Value = serde_json::from_slice(
        &skate_vfs::read(dir.join("sounds.json")).map_err(|e| format!("clean-audio/sounds.json: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for (name, entry) in manifest["sounds"].as_object().ok_or("sounds.json has no sounds")? {
        let file = entry["file"].as_str().ok_or("sound without file")?;
        let (data, rate) = wav(&skate_vfs::read(dir.join(file)).map_err(|e| format!("{file}: {e}"))?)
            .map_err(|e| format!("{file}: {e}"))?;
        let gain = entry["gain"].as_f64().unwrap_or(1.0) as f32;
        out.insert(name.clone(), Arc::new(Sample { data: data.into(), rate, gain }));
    }
    Ok(out)
}

const BEDS: [&str; 7] = ["roll_smooth", "roll_rough", "roll_wood", "roll_metal", "grind_metal", "grind_concrete", "powerslide"];

fn start(mut commands: Commands, mut assets: ResMut<Assets<LivePcm>>, configured: Option<Res<PlayerAudioAssets>>) {
    let Some(configured) = configured else { return };
    let samples = match load(&configured.0) {
        Ok(samples) => samples,
        Err(error) => {
            warn!("web player audio disabled: {error}");
            return;
        }
    };
    let Ok(live) = LivePcm::new(CHANNELS, SAMPLE_RATE, (SAMPLE_RATE as f32 * LEAD * 3.0) as usize) else { return };
    commands.spawn((AudioPlayer(assets.add(live.clone())), PlaybackSettings::ONCE.with_volume(Volume::Linear(1.0))));
    let beds = BEDS
        .iter()
        .filter_map(|name| {
            let sample = samples.get(*name)?.clone();
            Some((*name, Bed { voice: Voice { sample, position: 0.0, pitch: 1.0, gain: 0.0 }, target_gain: 0.0, target_pitch: 1.0 }))
        })
        .collect();
    info!("web player audio: {} clean sounds", samples.len());
    commands.insert_resource(WebPlayerAudio {
        live,
        samples,
        beds,
        one_shots: Vec::new(),
        edges: Edges::default(),
        seed: 0x9E37_79B9,
        scratch: Vec::new(),
    });
}

/// The retail audio-material vote, folded to the four beds this set has.
fn roll_bed(surface: u32) -> &'static str {
    match surface {
        1 | 2 | 6 => "roll_rough",
        5 => "roll_wood",
        9 => "roll_metal",
        _ => "roll_smooth",
    }
}

impl WebPlayerAudio {
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1 << 24) as f32
    }
    /// One-shot `name`, or a random `name_N` variant when the set has several.
    fn play(&mut self, name: &str, gain: f32, pitch: f32) {
        let variants: Vec<_> = self
            .samples
            .iter()
            .filter(|(k, _)| *k == name || k.strip_prefix(name).is_some_and(|r| r.starts_with('_') && r[1..].parse::<u32>().is_ok()))
            .map(|(_, s)| s.clone())
            .collect();
        if variants.is_empty() || gain <= 0.0 {
            return;
        }
        let pick = ((self.random() * variants.len() as f32) as usize).min(variants.len() - 1);
        let jitter = 1.0 + (self.random() - 0.5) * 0.08;
        if self.one_shots.len() >= MAX_ONE_SHOTS {
            self.one_shots.remove(0);
        }
        self.one_shots.push(Voice { sample: variants[pick].clone(), position: 0.0, pitch: pitch * jitter, gain });
    }

    fn observe(&mut self, o: &PlayerAudioObservation) {
        let speed = o.board_speed.abs();
        let rolling = o.grounded && !o.grinding && !o.wiping_out && o.contact_count > 0;
        let roll = roll_bed(o.wheel_surface);
        let level = (speed / 12.0).clamp(0.0, 1.0);
        for (name, bed) in self.beds.iter_mut() {
            let (gain, pitch) = match *name {
                n if n == roll && rolling => (0.15 + 0.6 * level, 0.9 + 0.25 * level),
                "grind_metal" if o.grinding && o.grind_audio_surface == 9 => (0.35 + 0.4 * level, 0.9 + 0.2 * level),
                "grind_concrete" if o.grinding && o.grind_audio_surface != 9 => (0.35 + 0.4 * level, 0.9 + 0.2 * level),
                "powerslide" if o.powersliding && o.grounded => (0.5 * level + 0.1, 1.0),
                _ => (0.0, bed.target_pitch),
            };
            bed.target_gain = if speed < 0.2 && name.starts_with("roll") { 0.0 } else { gain };
            bed.target_pitch = pitch;
        }
        if !self.edges.started {
            // The first observation only primes the edges.
            self.edges = Edges { grounded: o.grounded, takeoff_tick: o.tick, pushing: o.foot_push_speed > 0.0, trick: o.trick_identifier.clone(), started: true };
            return;
        }
        if self.edges.grounded && !o.grounded && !o.wiping_out {
            self.play("pop", 0.8, 1.0);
        }
        let air_seconds = o.tick.saturating_sub(self.edges.takeoff_tick) as f32 / 60.0;
        let mut landed = false;
        for event in &o.events {
            match event {
                PhysicsEvent::Landing => {
                    landed = true;
                    // Retail: the impact at a fixed level, plus one ladder layer
                    // chosen by time in air (0.75 s) -- harder sounds different,
                    // not louder (LandingTuning in skate-data audio/splice.rs).
                    self.play("land_impact", 1.0, 1.0);
                    self.play(if air_seconds >= 0.75 { "land_hard" } else { "land_soft" }, 1.0, 1.0);
                }
                PhysicsEvent::Wipeout => self.play("bail", 1.0, 1.0),
                _ => {}
            }
        }
        // Touching down after a real air always lands audibly, even when the
        // physics step reports the contact without a Landing event.
        if !landed && !self.edges.grounded && o.grounded && !o.wiping_out && air_seconds >= 0.2 {
            self.play("land_impact", 1.0, 1.0);
        }
        if o.footstep_strength > 0.0 {
            self.play("footstep", o.footstep_strength.clamp(0.2, 1.0), 1.0);
        }
        let pushing = o.foot_push_speed > 0.0;
        if pushing && !self.edges.pushing {
            self.play("push", 0.7, 1.0);
        }
        let spins = |id: Option<&str>| {
            let id = id.unwrap_or("").to_ascii_lowercase();
            ["flip", "shuv", "varial", "heel", "kick"].iter().any(|k| id.contains(k))
        };
        if !o.grounded && o.trick_identifier.is_some() && o.trick_identifier != self.edges.trick
            && spins(o.trick_identifier.as_deref())
        {
            self.play("flip", 0.6, 1.0);
        }
        // Feet catching the board: a board-spinning trick ends while still airborne.
        if !o.grounded && spins(self.edges.trick.as_deref()) && o.trick_identifier != self.edges.trick {
            self.play("catch", 0.9, 1.0);
        }
        if !o.frontend_sounds.is_empty() {
            self.play("chime", 0.7, 1.0);
        }
        if self.edges.grounded && !o.grounded {
            self.edges.takeoff_tick = o.tick;
        }
        self.edges.grounded = o.grounded;
        self.edges.pushing = pushing;
        self.edges.trick = o.trick_identifier.clone();
    }

    fn render(&mut self, frames: usize) {
        let mut out = std::mem::take(&mut self.scratch);
        out.clear();
        out.resize(frames * CHANNELS as usize, 0.0);
        // Gains and pitches glide over the block so a bed never clicks.
        let smooth = 1.0 - (-(frames as f32) / (SAMPLE_RATE as f32 * 0.06)).exp();
        for bed in self.beds.values_mut() {
            let v = &mut bed.voice;
            let (g0, p0) = (v.gain, v.pitch);
            let g1 = g0 + (bed.target_gain - g0) * smooth;
            let p1 = p0 + (bed.target_pitch - p0) * smooth;
            if g0 < 1e-4 && g1 < 1e-4 {
                v.gain = g1;
                v.pitch = p1;
                continue;
            }
            let len = v.sample.data.len() as f64;
            let ratio = v.sample.rate as f64 / SAMPLE_RATE as f64;
            for (i, frame) in out.chunks_exact_mut(CHANNELS as usize).enumerate() {
                let t = i as f32 / frames as f32;
                let s = sample_at(&v.sample.data, v.position) * (g0 + (g1 - g0) * t) * v.sample.gain;
                frame.iter_mut().for_each(|x| *x += s);
                v.position = (v.position + ratio * f64::from(p0 + (p1 - p0) * t)) % len;
            }
            v.gain = g1;
            v.pitch = p1;
        }
        self.one_shots.retain_mut(|v| {
            let len = v.sample.data.len() as f64;
            let step = v.sample.rate as f64 / SAMPLE_RATE as f64 * f64::from(v.pitch);
            for frame in out.chunks_exact_mut(CHANNELS as usize) {
                if v.position >= len - 1.0 {
                    return false;
                }
                let s = sample_at(&v.sample.data, v.position) * v.gain * v.sample.gain;
                frame.iter_mut().for_each(|x| *x += s);
                v.position += step;
            }
            true
        });
        for x in out.iter_mut() {
            // Soft limit instead of hard clipping when several cues stack.
            *x = x.tanh();
        }
        let _ = self.live.push(&out);
        self.scratch = out;
    }
}

fn sample_at(data: &[f32], position: f64) -> f32 {
    let i = position as usize;
    let f = (position - i as f64) as f32;
    let a = data[i % data.len()];
    let b = data[(i + 1) % data.len()];
    a + (b - a) * f
}

fn forward(
    audio: Option<ResMut<WebPlayerAudio>>,
    mut observations: MessageReader<PlayerAudioObservation>,
    mut frontend: MessageReader<FrontEndSoundRequest>,
    virtual_time: Res<Time<bevy::time::Virtual>>,
) {
    let Some(mut audio) = audio else {
        observations.clear();
        frontend.clear();
        return;
    };
    for o in observations.read() {
        audio.observe(o);
    }
    if frontend.read().count() > 0 {
        audio.play("chime", 0.7, 1.0);
    }
    if virtual_time.is_paused() {
        for bed in audio.beds.values_mut() {
            bed.target_gain = 0.0;
        }
    }
    let want = (SAMPLE_RATE as f32 * LEAD) as usize;
    let queued = audio.live.queued_frames();
    if queued < want {
        audio.render(want - queued);
    }
}
