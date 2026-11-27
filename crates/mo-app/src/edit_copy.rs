//! Copies of a sound's edits, for dragging into a DAW or keeping (the 1.1 plan's light sample edits): for now, a part
//! picked on its card's waveform ("Loop 128 (part 1.406-1.875 s, 1a2b3c4d).wav").
//!
//! The original is never changed or moved (rule 2). Copies go in `Edited sounds` in the export folder, beside
//! `Trimmed sounds`: they're yours, as DAW projects go on using them, so the app never tidies them away (only a copy
//! whose making was cut short, [`is_half_made`]). A part's name says which part of which sound it is
//! (`mo_sounds::trimmed::part_name`), so dragging the same part again reuses its copy, an original changed since
//! gets it made again, and a project that uses it still counts as using the original.
//!
//! Also where a sound's hits are, for snapping a picked part's edges to them ([`hits_of`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use mo_audio::edit;

/// The export folder's folder for edited copies.
pub const FOLDER: &str = "Edited sounds";

/// The longest part that is copied: five minutes of 48 kHz stereo is 115 MB as it's read. A longer part is dragged as
/// the whole file.
pub const MOST_SECONDS: f64 = 5.0 * 60.0;

/// The longest sound whose hits are found ([`hits_of`]): two minutes is 23 MB of 48 kHz mono as it's read. On a longer
/// sound's waveform the hits are too close together on the card to snap to.
pub const HITS_MOST_SECONDS: f64 = 2.0 * 60.0;

/// The shortest part worth a copy (seconds): a press that barely moved isn't a part.
pub const LEAST_PART_SECONDS: f64 = 0.010;

/// The copy of the part of the sound at `original` from `from` to `to` seconds in, made now or before, in [`FOLDER`]
/// in the export folder set in Settings (looked up as trimmed copies look it up, at most every half minute:
/// `crate::trim_copy::exports_now`); or why there's none. Reads the file: never on the window's thread.
pub fn part_to_drag(original: &Path, from: f64, to: f64) -> Result<PathBuf, String> {
    let folder = crate::trim_copy::exports_now()?.join(FOLDER);
    part_in(original, from, to, &folder)
}

/// Copies are made one at a time: two drags of the same part (one let go of while its copy was still being made, and
/// the next) would otherwise write the same file under the same name at once.
static MAKING: Mutex<()> = Mutex::new(());

/// [`part_to_drag`]'s work, with copies kept in `folder`. Only the part is read ([`mo_audio::read_span`]), however
/// long the sound.
pub fn part_in(original: &Path, from: f64, to: f64, folder: &Path) -> Result<PathBuf, String> {
    let (from, to) = (from.min(to).max(0.0), from.max(to));
    if !from.is_finite() || !to.is_finite() || to - from < LEAST_PART_SECONDS {
        return Err("The part picked is too short to drag.".to_owned());
    }
    if to - from > MOST_SECONDS {
        return Err(format!("The part picked is longer than {} minutes, too long to copy.", MOST_SECONDS / 60.0));
    }
    let meta = std::fs::metadata(original).map_err(|e| format!("Couldn't look at {}: {e}", original.display()))?;
    let changed = meta.modified().ok();
    let copy = folder.join(mo_sounds::trimmed::part_name(&original.to_string_lossy(), from, to));
    // Made before, and the original hasn't changed since (a copy carries its original's date).
    let fresh = |copy: &Path| {
        std::fs::metadata(copy).is_ok_and(|c| c.is_file() && c.modified().ok().zip(changed).is_none_or(|(c, o)| c >= o))
    };
    if fresh(&copy) {
        return Ok(copy);
    }
    let _making = MAKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Another drag may have made it while this one waited.
    if fresh(&copy) {
        return Ok(copy);
    }
    let part = read_part(original, from, to).map_err(|e| format!("Couldn't read {}: {e}", original.display()))?;
    write_new(&part, &copy, changed)?;
    Ok(copy)
}

/// The part of the sound at `path` from `from` to `to` seconds in, as it's dragged, saved or played: faded in where
/// it was cut from what came before, and out where it was cut from what comes after ([`edit::ease_cuts`]). Only the
/// part is read. Or why there's none (without the file's name, which the caller adds): the file can't be read, or the
/// part starts past its end.
pub fn read_part(path: &Path, from: f64, to: f64) -> Result<mo_audio::AudioData, String> {
    let mut part = mo_audio::read_span(path, from, to - from).map_err(|e| e.to_string())?;
    if part.sample_rate == 0 || part.is_empty() {
        return Err("the part picked is past the end of the file".to_owned());
    }
    let asked = mo_audio::frame_at(to - from, part.sample_rate);
    let cut_start = mo_audio::frame_at(from, part.sample_rate) > 0;
    // All of it read: the sound went on past it (or ended just there, where a short fade does no harm).
    let cut_end = part.len() as u64 >= asked;
    edit::ease_cuts(&mut part, cut_start, cut_end);
    Ok(part)
}

/// Writes `audio` to `copy` (its folder made if need be), dated `changed` (the original's date): 24-bit, or 32-bit
/// float when it goes over full scale (so nothing is clipped). Written under a name of this program's own and then
/// renamed, so two programs making the same copy never write into one file, and a half-made copy never takes the
/// copy's name.
fn write_new(audio: &mo_audio::AudioData, copy: &Path, changed: Option<SystemTime>) -> Result<(), String> {
    if let Some(folder) = copy.parent() {
        std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
    }
    let bits = if edit::peak(audio) > 1.0 { 32 } else { 24 };
    let mine = copy.with_extension(format!("{}.wav", std::process::id()));
    let written = mo_audio::write_wav(&mine, &audio.channels, audio.sample_rate, bits)
        .map_err(|e| e.to_string())
        .and_then(|()| check(&mine, audio))
        .and_then(|()| stamp(&mine, changed).map_err(|e| e.to_string()))
        .and_then(|()| std::fs::rename(&mine, copy).map_err(|e| e.to_string()));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&mine);
        format!("Couldn't write {}: {e}", copy.display())
    })
}

/// Reads `written` back and checks it's `audio`: as many channels and frames, at the same rate.
fn check(written: &Path, audio: &mo_audio::AudioData) -> Result<(), String> {
    let back = mo_audio::read(written, MOST_SECONDS).map_err(|e| format!("it didn't read back ({e})"))?;
    let same = back.sample_rate == audio.sample_rate
        && back.channels.len() == audio.channels.len()
        && back.len() == audio.len();
    if same { Ok(()) } else { Err("it didn't read back as written".to_owned()) }
}

/// Dates the file at `path` `when`, when there's a date to give.
fn stamp(path: &Path, when: Option<SystemTime>) -> std::io::Result<()> {
    match when {
        Some(when) => std::fs::File::options().write(true).open(path)?.set_modified(when),
        None => Ok(()),
    }
}

/// Whether `name` is a copy whose making was cut short, in [`FOLDER`]: written under this program's own name first
/// (`<copy>.<pid>.wav`, and `write_wav`'s `<copy>.<pid>.wav.part` beside it) and renamed after (for the start-up
/// sweep). Never a copy itself.
pub fn is_half_made(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".wav.part").or_else(|| name.strip_suffix(".wav")) else { return false };
    let Some((copy, pid)) = rest.rsplit_once('.') else { return false };
    !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && mo_sounds::trimmed::mark_in(&format!("{copy}.wav")).is_some()
}

/// Where a sound's hits are: each hit's place in seconds and how big its rise was (dB), and how long the sound is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hits {
    pub at: Vec<(f64, f32)>,
    pub seconds: f64,
}

/// The hits of the sound at `path` ([`mo_audio::hits`]), reading the whole file, a part at a time into one channel:
/// never on the window's thread. None for a sound longer than [`HITS_MOST_SECONDS`].
pub fn hits_of(path: &Path) -> Result<Hits, String> {
    let mut mono: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    let mut too_long = false;
    let seconds = mo_audio::read_parts(path, 1.0, &mut |r, part| {
        rate = r;
        let frames = part.iter().map(Vec::len).min().unwrap_or(0);
        let share = 1.0 / part.len().max(1) as f32;
        mono.extend((0..frames).map(|i| part.iter().map(|c| c[i]).sum::<f32>() * share));
        too_long = mono.len() as f64 > HITS_MOST_SECONDS * f64::from(r);
        !too_long
    })
    .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    if too_long || rate == 0 {
        return Ok(Hits { at: Vec::new(), seconds });
    }
    let each = f64::from(rate);
    let at = mo_audio::hits::find_in(&mono, rate).into_iter().map(|h| (h.at as f64 / each, h.rise_db)).collect();
    Ok(Hits { at, seconds: mono.len() as f64 / each })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-edit-copy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 1,000 frames a second, two channels: a ramp up over one second.
    fn ramp(path: &Path) {
        let ch: Vec<f32> = (0..1_000).map(|i| i as f32 / 1_000.0).collect();
        mo_audio::write_wav(path, &[ch.clone(), ch], 1_000, 24).unwrap();
    }

    #[test]
    fn a_part_is_cut_out_once_and_reused() {
        let dir = folder("part");
        let original = dir.join("Loop 128.wav");
        ramp(&original);
        let copies = dir.join("Edited sounds");
        let copy = part_in(&original, 0.5, 0.25, &copies).unwrap();
        assert_eq!(copy.parent(), Some(copies.as_path()), "its folder made");
        let name = copy.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, mo_sounds::trimmed::part_name(&original.to_string_lossy(), 0.25, 0.5));
        let part = mo_audio::read(&copy, 60.0).unwrap();
        assert_eq!((part.channels.len(), part.len()), (2, 250));
        // In the middle, as it was; faded where it was cut.
        assert!((part.channels[0][125] - 0.375).abs() < 1e-4, "{}", part.channels[0][125]);
        assert_eq!(part.channels[0][0], 0.0);
        // Dated as its original, so it's fresh: dragged again, it's the same copy, not made again.
        std::fs::write(&copy, b"kept").unwrap();
        stamp(&copy, std::fs::metadata(&original).unwrap().modified().ok()).unwrap();
        assert_eq!(part_in(&original, 0.25, 0.5, &copies).unwrap(), copy);
        assert_eq!(std::fs::read(&copy).unwrap(), b"kept");
        // Nothing half-made is left beside it.
        let names: Vec<String> = std::fs::read_dir(&copies)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![name]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_too_short_or_past_the_end_or_of_no_file_is_refused() {
        let dir = folder("refused");
        let original = dir.join("Kick.wav");
        ramp(&original);
        let copies = dir.join("Edited sounds");
        assert!(part_in(&original, 0.5, 0.505, &copies).is_err());
        assert!(part_in(&original, 2.0, 3.0, &copies).unwrap_err().contains("past the end"));
        assert!(part_in(&original, f64::NAN, 0.5, &copies).is_err());
        assert!(part_in(&dir.join("gone.wav"), 0.1, 0.5, &copies).is_err());
        assert!(!copies.exists(), "nothing made");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_is_faded_only_where_it_was_cut() {
        let dir = folder("edges");
        let original = dir.join("Pad.wav");
        mo_audio::write_wav(&original, &[vec![0.5; 1_000]], 1_000, 24).unwrap();
        // From the start: not faded in, but faded out where it was cut from the rest.
        let head = read_part(&original, 0.0, 0.5).unwrap();
        assert_eq!(head.len(), 500);
        assert!((head.channels[0][0] - 0.5).abs() < 1e-6);
        assert_eq!(head.channels[0][499], 0.0);
        // To the end (asked past it): faded in, not out; only as long as there is.
        let tail = read_part(&original, 0.5, 2.0).unwrap();
        assert_eq!(tail.len(), 500);
        assert_eq!(tail.channels[0][0], 0.0);
        assert!((tail.channels[0][499] - 0.5).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_that_goes_over_full_scale_is_kept_as_floats() {
        let dir = folder("loud");
        let original = dir.join("Loud.wav");
        mo_audio::write_wav(&original, &[vec![1.5; 1_000]], 1_000, 32).unwrap();
        let copy = part_in(&original, 0.0, 0.5, &dir.join("Edited sounds")).unwrap();
        let part = mo_audio::read(&copy, 60.0).unwrap();
        assert!((part.channels[0][100] - 1.5).abs() < 1e-6, "not clipped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn half_made_copies_are_told_from_copies() {
        let copy = mo_sounds::trimmed::part_name("D:\\Samples\\Loop.wav", 1.0, 2.0);
        let stem = copy.strip_suffix(".wav").unwrap();
        assert!(is_half_made(&format!("{stem}.4242.wav")));
        assert!(is_half_made(&format!("{stem}.4242.wav.part")));
        assert!(!is_half_made(&copy), "never a copy");
        assert!(!is_half_made("Loop.4242.wav"), "never a file of the user's");
    }

    #[test]
    fn hits_are_found_in_seconds() {
        let dir = folder("hits");
        let original = dir.join("Two hits.wav");
        let mut ch = vec![0.0f32; 48_000];
        for at in [12_000, 36_000] {
            for (i, s) in ch[at..at + 4_800].iter_mut().enumerate() {
                *s = (-(i as f32) / 800.0).exp() * if i % 2 == 0 { 0.8 } else { -0.8 };
            }
        }
        mo_audio::write_wav(&original, &[ch], 48_000, 24).unwrap();
        let hits = hits_of(&original).unwrap();
        assert!((hits.seconds - 1.0).abs() < 1e-9);
        let at: Vec<f64> = hits.at.iter().map(|h| h.0).collect();
        assert_eq!(at.len(), 2, "{at:?}");
        assert!((at[0] - 0.25).abs() < 0.003 && (at[1] - 0.75).abs() < 0.003, "{at:?}");
        // Longer than two minutes: none, as they'd be too close together on the card to snap to.
        let long = dir.join("Long.wav");
        mo_audio::write_wav(&long, &[vec![0.0; 121_000]], 1_000, 16).unwrap();
        let hits = hits_of(&long).unwrap();
        assert!(hits.at.is_empty());
        assert!(hits.seconds > HITS_MOST_SECONDS, "{}", hits.seconds);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
