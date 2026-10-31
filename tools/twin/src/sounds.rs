//! The sounds twin: both apps analyse the same test sounds (made here, so none is anyone's sample) and every measure
//! that differs is listed. The C# analyser runs unchanged through the helper's `--sounds`; the Rust one is
//! `mo_sounds::analyse`. The sounds go in a new folder of their own inside the one given, removed afterwards.
//!
//! The type classifier is checked in the same folder (`classify.rs`).
//!
//! Finding a key and chords in the audio is left to the harmony engine (phase 6), so where the C# app found those
//! itself, they're counted as judged and not compared (see crates/mo-sounds/DIFFERENCES.md).

use std::path::{Path, PathBuf};

use mo_audio::write_wav;
use serde_json::Value;

/// One test sound: its file name (which the analyser reads for loop, tempo and key words), rate, bits and channels.
struct Made {
    name: &'static str,
    rate: u32,
    bits: u16,
    channels: Vec<Vec<f32>>,
}

fn sine(hz: f64, rate: u32, seconds: f64, level: f32) -> Vec<f32> {
    let n = (seconds * f64::from(rate)) as usize;
    (0..n).map(|i| level * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32).collect()
}

/// Noise from a fixed seed, so every run makes the same file.
fn noise(n: usize, level: f32, seed: u64) -> Vec<f32> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            level * ((s >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0)
        })
        .collect()
}

fn mix(parts: &[Vec<f32>]) -> Vec<f32> {
    let n = parts.iter().map(Vec::len).max().unwrap_or(0);
    (0..n).map(|i| parts.iter().map(|p| p.get(i).copied().unwrap_or(0.0)).sum()).collect()
}

fn clicks(rate: u32, seconds: f64, bpm: f64, seed: u64) -> Vec<f32> {
    let mut x = vec![0f32; (seconds * f64::from(rate)) as usize];
    let every = 60.0 / bpm * f64::from(rate);
    let mut beat = 0.0;
    while (beat as usize) < x.len() {
        for (i, s) in noise(600, 0.8, seed + beat as u64).into_iter().enumerate() {
            if let Some(v) = x.get_mut(beat as usize + i) {
                *v = s * (1.0 - i as f32 / 600.0);
            }
        }
        beat += every;
    }
    x
}

fn kick(rate: u32) -> Vec<f32> {
    let n = rate as usize / 2;
    let mut phase = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(rate);
            phase += std::f64::consts::TAU * (50.0 + 100.0 * (-t * 30.0).exp()) / f64::from(rate);
            (0.9 * phase.sin() * (-t * 8.0).exp()) as f32
        })
        .collect()
}

fn made() -> Vec<Made> {
    let r = 44_100;
    let chord =
        |rate, secs| mix(&[sine(261.63, rate, secs, 0.2), sine(311.13, rate, secs, 0.2), sine(392.0, rate, secs, 0.2)]);
    vec![
        Made { name: "tone A4.wav", rate: r, bits: 16, channels: vec![sine(440.0, r, 2.0, 0.5)] },
        Made {
            name: "bass stereo.wav",
            rate: 48_000,
            bits: 24,
            channels: vec![sine(110.0, 48_000, 1.5, 0.6), sine(110.5, 48_000, 1.5, 0.5)],
        },
        Made { name: "hat.wav", rate: r, bits: 16, channels: vec![noise(r as usize / 4, 0.5, 7)] },
        Made {
            name: "wide noise.wav",
            rate: r,
            bits: 16,
            channels: vec![noise(r as usize, 0.4, 3), noise(r as usize, 0.4, 5)],
        },
        Made { name: "clicks.wav", rate: 22_050, bits: 16, channels: vec![clicks(22_050, 8.0, 120.0, 11)] },
        Made { name: "Drum Loop 96bpm.wav", rate: r, bits: 16, channels: vec![clicks(r, 5.0, 96.0, 13)] },
        Made { name: "Pad Cm.wav", rate: r, bits: 16, channels: vec![chord(r, 4.0)] },
        Made { name: "stab.wav", rate: r, bits: 16, channels: vec![chord(r, 2.0)] },
        Made { name: "kick.wav", rate: r, bits: 24, channels: vec![kick(r)] },
        Made { name: "blip.wav", rate: r, bits: 16, channels: vec![sine(1000.0, r, 0.01, 0.5)] },
        Made { name: "silence.wav", rate: r, bits: 16, channels: vec![vec![0.0; r as usize / 2]] },
        Made { name: "high rate tone.wav", rate: 96_000, bits: 24, channels: vec![sine(523.25, 96_000, 1.0, 0.3)] },
        Made { name: "long tone.wav", rate: 22_050, bits: 16, channels: vec![sine(196.0, 22_050, 12.0, 0.3)] },
    ]
}

/// The folder made inside the one given for the test sounds; it must not be there already, and it's removed after.
const OWN_FOLDER: &str = "mo-twin-sounds";

/// Writes the test sounds into `dir` (a folder just made for them) and gives their paths.
fn write_sounds(dir: &Path) -> Result<Vec<PathBuf>, String> {
    made()
        .into_iter()
        .map(|m| {
            let path = dir.join(m.name);
            write_wav(&path, &m.channels, m.rate, m.bits).map_err(|e| format!("Couldn't write {}: {e}", m.name))?;
            Ok(path)
        })
        .collect()
}

/// What C# measures that's worked out from other fields when shown, so isn't compared.
const SHOWN_ONLY: [&str; 2] = ["KeyName", "RootNoteName"];
/// Left to the harmony engine when the C# app found them in the audio itself.
const KEY_FIELDS: [&str; 4] = ["KeyTonic", "KeyMinor", "KeyConfidence", "KeySource"];

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

pub(crate) fn same(cs: &Value, rust: &Value) -> bool {
    match (cs, rust) {
        (Value::Number(a), Value::Number(b)) => close(a.as_f64().unwrap_or(f64::NAN), b.as_f64().unwrap_or(f64::NAN)),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
        }
        _ => cs == rust,
    }
}

/// Runs the sounds twin (and the classifier twin) into `dir`. `Ok(false)` when a measure differs that isn't judged.
pub fn run(helper: &Path, parent: &Path) -> Result<bool, String> {
    let dir = parent.join(OWN_FOLDER);
    // A new folder only, so a test sound can never land on (or beside) a real sample. One left from a run that was
    // stopped is reported, not removed: the tool removes only a folder it has just made.
    std::fs::create_dir(&dir).map_err(|e| format!("Couldn't make a new folder {}: {e}", dir.display()))?;
    let outcome = write_sounds(&dir).and_then(|files| compare(helper, &files)).and_then(|sounds_same| {
        let differences = crate::classify::compare(helper, &dir)?;
        println!("Classifier twin: {} differences in training and judging.", differences.len());
        for d in &differences {
            println!("  {d}");
        }
        Ok(sounds_same && differences.is_empty())
    });
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

fn compare(helper: &Path, files: &[PathBuf]) -> Result<bool, String> {
    let cs = crate::run_helper_with(helper, &["--sounds"], files)?;
    let mut differences = Vec::new();
    let mut judged = Vec::new();
    for file in files {
        let key = file.to_string_lossy().into_owned();
        let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let Some(cs) = cs.get(&key) else {
            differences.push(format!("{name}: no C# analysis"));
            continue;
        };
        let rust = match mo_sounds::analyse::analyse_file(file) {
            Ok(a) => {
                let mut v = serde_json::to_value(&a.analysis).map_err(|e| e.to_string())?;
                if let Value::Object(o) = &mut v {
                    o.insert("Features".into(), serde_json::to_value(&a.features).map_err(|e| e.to_string())?);
                }
                v
            }
            Err(e) => {
                differences.push(format!("{name}: Rust couldn't analyse it: {e}"));
                continue;
            }
        };
        let Some(Value::Object(cs)) = cs.get("analysis") else {
            let why = cs.get("error").or_else(|| cs.get("error_more")).unwrap_or(&Value::Null);
            differences.push(format!("{name}: C# couldn't analyse it: {why}"));
            continue;
        };
        let detected_key = cs.get("KeySource").and_then(Value::as_str) == Some("detected");
        for (field, cs_value) in cs {
            if SHOWN_ONLY.contains(&field.as_str()) {
                continue;
            }
            let rust_value = rust.get(field).unwrap_or(&Value::Null);
            let harmony =
                matches!(field.as_str(), "Chord" | "Chords") || (detected_key && KEY_FIELDS.contains(&field.as_str()));
            if same(cs_value, rust_value) {
                continue;
            }
            if harmony {
                judged.push(format!("{name}: {field}: C# {cs_value}, Rust {rust_value}"));
                continue;
            }
            differences.push(format!("{name}: {field}: C# {cs_value}, Rust {rust_value}"));
        }
    }
    println!(
        "Sounds twin: {} test sounds, {} differences, {} left to the harmony engine (key and chords found in the audio).",
        files.len(),
        differences.len(),
        judged.len()
    );
    for d in &differences {
        println!("  {d}");
    }
    if !judged.is_empty() {
        println!("Left to the harmony engine:");
        for j in &judged {
            println!("  {j}");
        }
    }
    Ok(differences.is_empty())
}
