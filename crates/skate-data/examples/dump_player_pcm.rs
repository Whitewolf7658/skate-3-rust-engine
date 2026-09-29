//! Write every player-audio sample the game would play as a 16-bit WAV, for
//! offline analysis. Reads the prepared install and the PCM decode cache; it
//! decodes through ffmpeg only where the cache has no entry.
//!
//!     cargo run -p skate-data --example dump_player_pcm -- <assets dir> <cache dir> <out dir>
//!
//! Output: <out>/<bank>/<index>.wav for bank samples, <out>/grains/<member>.wav
//! for grains and wheel spins, and <out>/index.json listing each with its rate,
//! channels, frame count and loop range.
use skate_data::audio::{catalog::PlayerAudioCatalog, grains};
use std::path::Path;

fn wav(path: &Path, samples: &[i16], channels: u16, rate: u32) -> Result<(), String> {
    let data = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend(b"RIFF");
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(channels.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * u32::from(channels) * 2).to_le_bytes());
    b.extend((channels * 2).to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for s in samples {
        b.extend(s.to_le_bytes());
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path, b).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [assets, cache, out] = args.as_slice() else {
        return Err("usage: dump_player_pcm <assets dir> <cache dir> <out dir>".into());
    };
    let (assets, cache, out) = (Path::new(assets), Path::new(cache), Path::new(out));
    let mut index = Vec::new();
    let mut catalog = PlayerAudioCatalog::from_assets(assets, Some(cache)).map_err(|e| e.to_string())?;
    // Collision, metal, body and menu SPLC banks load separately, as the game does.
    catalog
        .load_splice_banks(&assets.join("private/stock/data/audio/audiofiles.big"), Some(cache))
        .map_err(|e| e.to_string())?;
    for s in &catalog.samples {
        let src = &s.pcm.source;
        let channels = usize::from(src.channels());
        let mut pcm = Vec::with_capacity(src.frames() * channels);
        for f in 0..src.frames() {
            for c in 0..channels {
                pcm.push((src.sample(f, c).unwrap_or(0.) * 32768.).clamp(-32768., 32767.) as i16);
            }
        }
        let rel = format!("{}/{}.wav", s.bank, s.index);
        wav(&out.join(&rel), &pcm, channels as u16, s.pcm.sample_rate)?;
        index.push(serde_json::json!({"file": rel, "bank": s.bank, "index": s.index,
            "rate": s.pcm.sample_rate, "channels": channels, "frames": src.frames(),
            "loop": src.loop_range()}));
    }
    let audio = assets.join("private/stock/data/audio");
    let mut members = grains::load_grains(&audio.join("grains.big"), &[], Some(cache)).map_err(|e| e.to_string())?;
    members.extend(grains::load_members(&audio.join("wheels.big"), &[], Some(cache)).map_err(|e| e.to_string())?);
    for m in members.iter().filter(|m| m.channels > 0) {
        let rel = format!("grains/{}.wav", m.name);
        wav(&out.join(&rel), &m.samples, u16::from(m.channels), m.rate)?;
        index.push(serde_json::json!({"file": rel, "bank": "grains", "member": m.name,
            "rate": m.rate, "channels": m.channels, "frames": m.samples.len() / usize::from(m.channels)}));
    }
    std::fs::write(out.join("index.json"), serde_json::to_vec_pretty(&index).unwrap()).map_err(|e| e.to_string())?;
    eprintln!("wrote {} sounds to {}", index.len(), out.display());
    Ok(())
}
