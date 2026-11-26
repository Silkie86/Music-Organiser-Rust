//! The stretcher ear test (1.1 plan, the stretcher): `stretchear LOOPS OUT [COUNT]`.
//!
//! Takes up to COUNT loops (20 unless given) from the LOOPS folder and its subfolders, each with a tempo in its name
//! or folder name (or its ACID notes), 30 seconds at most. Each is stretched to 80% of its tempo (slower, longer)
//! and to 125% (faster) by each stretcher:
//!
//! - **signalsmith**: Signalsmith Stretch alone (`mo_stretch::to_length`),
//! - **timestretch**: the `timestretch` library alone,
//! - **app-today**: what the app plays today (`timestretch` with each hit put back),
//! - **ours**: our drum stretch (`mo_stretch::drums`).
//!
//! OUT gets a folder per loop with `original.wav` and the stretches named by letter only (`slower A.wav` ...), in a
//! different order for each loop, so the ear isn't led. `key.csv` says which letter is which, with each one's scores
//! (see `mo_stretch::score`): open it after listening. Mono and stereo loops only.
//!
//! The loops are only read. OUT must be a new or empty folder: nothing already in it is ever written over.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const METHODS: [&str; 4] = ["signalsmith", "timestretch", "app-today", "ours"];
const SPEEDS: [(&str, f64); 2] = [("slower", 0.8), ("faster", 1.25)];

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(loops), Some(out)) = (args.get(1), args.get(2)) else {
        return Err("stretchear LOOPS OUT [COUNT]: LOOPS a folder of loops, OUT a new or empty folder".into());
    };
    let count = match args.get(3) {
        Some(c) => c.parse::<usize>().map_err(|e| format!("COUNT {c}: {e}"))?,
        None => 20,
    };
    let out = PathBuf::from(out);
    if out.exists() && std::fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?.next().is_some() {
        return Err(format!("{} isn't empty: pick a new folder, so nothing in it is written over", out.display()));
    }

    let mut files = Vec::new();
    gather(Path::new(loops), &mut files);
    files.sort();
    let mut key = String::from("loop,speed,letter,stretcher,ghost dB,sharpness,timing ms,distance dB,hits\n");
    let mut done = 0;
    for path in files {
        if done == count {
            break;
        }
        let name = path.to_string_lossy().to_string();
        let named = mo_sounds::types::tempo_from_name(&name).or_else(|| mo_sounds::types::tempo_from_folders(&name));
        // Only a WAV or AIFF can carry its tempo inside (ACID notes): anything else without one in its name is
        // passed over unread.
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if named.is_none() && !["wav", "aif", "aiff"].contains(&ext.as_str()) {
            continue;
        }
        let Ok(audio) = mo_audio::read(&path, 30.0) else { continue };
        // Mono and stereo only: the timestretch library takes two channels at most, and all four must hear the same.
        if audio.file_seconds > 30.0 || audio.channels.is_empty() || audio.channels.len() > 2 {
            continue;
        }
        let Some(tempo) = named.or(audio.embedded_tempo) else { continue };
        let stem = path.file_stem().map_or_else(|| "loop".into(), |s| s.to_string_lossy().to_string());
        let folder_name = format!("{:02} {}", done + 1, safe(&stem));
        let folder = out.join(&folder_name);
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let rate = audio.sample_rate;
        write(&folder.join("original.wav"), &audio.channels, rate)?;
        let mono: Vec<f32> =
            (0..audio.channels[0].len()).map(|i| audio.channels.iter().map(|c| c[i]).sum::<f32>()).collect();

        for (speed, factor) in SPEEDS {
            let to = tempo * factor;
            // Each loop's own order of letters, from its place in the list.
            let order = shuffled(done * 2 + usize::from(speed == "faster"));
            for (letter, &method) in ["A", "B", "C", "D"].iter().zip(order.iter().map(|&i| &METHODS[i])) {
                let stretched = match stretch(method, &audio, tempo, to) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{stem} {speed} {method}: {e}");
                        continue;
                    }
                };
                write(&folder.join(format!("{speed} {letter}.wav")), &stretched, rate)?;
                let out_mono: Vec<f32> =
                    (0..stretched[0].len()).map(|i| stretched.iter().map(|c| c[i]).sum::<f32>()).collect();
                let s = mo_stretch::score::score(&mono, &out_mono, rate as f32);
                let _ = writeln!(
                    key,
                    "\"{}\",{speed},{letter},{method},{:.1},{:.2},{:.1},{:.2},{}",
                    folder_name.replace('"', "'"),
                    s.ghost_db,
                    s.sharpness,
                    s.timing_ms,
                    s.distance_db,
                    s.hits
                );
            }
        }
        println!("{folder_name} ({tempo:.0} BPM)");
        done += 1;
    }
    if done == 0 {
        return Err("No loop with a tempo in its name, folder or notes was found there".into());
    }
    std::fs::write(out.join("key.csv"), key).map_err(|e| format!("key.csv: {e}"))?;
    println!("{done} loops in {}. Listen first, then open key.csv.", out.display());
    Ok(())
}

/// Every sound file under `folder`.
fn gather(folder: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        // Not through a link: it could lead anywhere.
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            gather(&path, files);
        } else if kind.is_file() {
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ["wav", "aif", "aiff", "flac", "mp3", "ogg"].contains(&ext.as_str()) {
                files.push(path);
            }
        }
    }
}

/// `audio` (at `from` BPM) at `to` BPM by `method`.
fn stretch(method: &str, audio: &mo_audio::AudioData, from: f64, to: f64) -> Result<Vec<Vec<f32>>, String> {
    let rate = audio.sample_rate as f32;
    let length = (audio.channels[0].len() as f64 * from / to).round() as usize;
    let inputs: Vec<&[f32]> = audio.channels.iter().map(Vec::as_slice).collect();
    match method {
        "signalsmith" => mo_stretch::to_length(&inputs, rate, length, 0.0).ok_or_else(|| "too short".into()),
        "ours" => mo_stretch::drums::to_length(&inputs, rate, length, mo_stretch::drums::DrumOptions::default())
            .ok_or_else(|| "too short".into()),
        "timestretch" => mo_audio::stretch::to_tempo_plain(audio, from, to).map(|a| a.channels),
        _ => mo_audio::stretch::to_tempo(audio, from, to).map(|a| a.channels),
    }
}

/// The order the four stretchers get the letters A to D in, for the `n`th set: each of the 24 orders in turn,
/// stepped through so neighbours differ.
fn shuffled(n: usize) -> [usize; 4] {
    let mut pick = (n * 7 + 3) % 24;
    let mut left = vec![0, 1, 2, 3];
    let mut order = [0; 4];
    for (i, slot) in order.iter_mut().enumerate() {
        let base = 4 - i;
        *slot = left.remove(pick % base);
        pick /= base;
    }
    order
}

/// A name safe for a folder: no characters Windows refuses, and no dot or space at the end (Windows drops them).
fn safe(name: &str) -> String {
    let name: String =
        name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let name = name.trim_end_matches(['.', ' ']);
    if name.is_empty() { "loop".into() } else { name.into() }
}

fn write(path: &Path, channels: &[Vec<f32>], rate: u32) -> Result<(), String> {
    if path.exists() {
        return Err(format!("{} is there already: not written over", path.display()));
    }
    mo_audio::write_wav(path, channels, rate, 24).map_err(|e| format!("{}: {e:?}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_set_gets_all_four_letters_and_neighbours_differ() {
        for n in 0..48 {
            let mut o = shuffled(n);
            assert_ne!(shuffled(n), shuffled(n + 1));
            o.sort();
            assert_eq!(o, [0, 1, 2, 3]);
        }
    }

    #[test]
    fn folder_names_windows_takes() {
        assert_eq!(safe("Break 120bpm."), "Break 120bpm");
        assert_eq!(safe("Loop 90 "), "Loop 90");
        assert_eq!(safe("a:b?c"), "a_b_c");
        assert_eq!(safe(". "), "loop");
    }
}
