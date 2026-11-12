//! Trimmed copies for dragging into a DAW: with Trim silence on, a sound dragged out of the app or the plugin goes as a
//! copy without its quiet start and end (`mo_audio::trim`), so the DAW gets what you heard (Solomon, 12 Nov 2026).
//!
//! The original is never changed or moved (rule 2). Copies go in the data folder's `Trimmed sounds` folder and stay
//! there: a DAW project that was given one goes on using it, so the app's tidying never removes them (rule 28 removes
//! only what's done with). One copy for each sound and level: dragging the same sound again at the same level drags
//! the copy already made, and an original that has changed since (its size or date) gets a new one.

use std::path::{Path, PathBuf};

/// The data folder's folder for trimmed copies.
pub const FOLDER: &str = "Trimmed sounds";

/// Where trimmed copies go: [`FOLDER`] in the app's data folder.
pub fn folder() -> PathBuf {
    crate::paths::data_dir().join(FOLDER)
}

/// The longest sound a copy is made of: a longer one is dragged as it is (read whole, it would be held in memory).
const MOST_SECONDS: f64 = 30.0 * 60.0;

/// What to drag for `original` with Trim silence on at `db` below full scale, copies kept in `folder`: `Ok(Some(copy))`
/// for a trimmed copy (made now, or the one made before), `Ok(None)` when there's nothing to trim (drag the original),
/// or why no copy could be made (drag the original, and say why in the log). Reads the whole file: call it off the
/// window's thread.
pub fn for_drag(original: &Path, db: i32, folder: &Path) -> Result<Option<PathBuf>, String> {
    let meta = std::fs::metadata(original).map_err(|e| format!("Couldn't look at {}: {e}", original.display()))?;
    let stamp =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let copy = folder.join(copy_name(original, db, meta.len(), stamp));
    if copy.is_file() {
        return Ok(Some(copy));
    }
    let mut audio =
        mo_audio::read(original, MOST_SECONDS).map_err(|e| format!("Couldn't read {}: {e}", original.display()))?;
    // Read only so far (a very long file, or one that says it's longer than it is): the whole sound isn't here to trim.
    if audio.sample_rate == 0 || audio.seconds() < audio.file_seconds * 0.99 {
        return Ok(None);
    }
    let level = mo_audio::trim::level_of_db(db);
    match mo_audio::trim::sound_span(&audio, level) {
        Some((start, end)) if start > 0 || end < audio.len() => audio.keep(start, end),
        // Loud from start to end, or silent all through: the original is what you'd hear.
        _ => return Ok(None),
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
    // Written beside its name and then renamed (write_wav), so a half-made copy never takes the copy's name.
    mo_audio::write_wav(&copy, &audio.channels, audio.sample_rate, 24)
        .map_err(|e| format!("Couldn't write {}: {e}", copy.display()))?;
    Ok(Some(copy))
}

/// A copy's file name: the original's name, the level, and a short mark of which file it was (its path, size and date),
/// so two sounds with the same name, or a changed original, never share a copy: "Kick 01 (trimmed -60 dB, 1a2b3c4d).wav".
fn copy_name(original: &Path, db: i32, size: u64, stamp: u128) -> String {
    let stem = original.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Sound".into());
    // FNV-1a: the same mark on every run and every Rust version (std's hasher may change between versions).
    let mut mark: u64 = 0xcbf2_9ce4_8422_2325;
    let path = original.to_string_lossy();
    for b in path.as_bytes().iter().chain(&size.to_le_bytes()).chain(&stamp.to_le_bytes()) {
        mark = (mark ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{stem} (trimmed {db} dB, {:08x}).wav", mark as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-trim-copy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 1,000 frames a second, one channel: silent 0.2 s, loud 0.3 s, silent 0.5 s.
    fn quiet_loud_quiet(path: &Path) {
        let mut s = vec![0.0; 1_000];
        s[200..500].fill(0.5);
        mo_audio::write_wav(path, &[s], 1_000, 24).unwrap();
    }

    #[test]
    fn a_copy_is_trimmed_made_once_and_the_original_stays() {
        let dir = folder("once");
        let original = dir.join("Kick.wav");
        quiet_loud_quiet(&original);
        let before = std::fs::read(&original).unwrap();
        let copies = dir.join(FOLDER);
        let copy = for_drag(&original, -60, &copies).unwrap().expect("a copy");
        assert_eq!(std::fs::read(&original).unwrap(), before, "the original is never changed");
        let name = copy.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("Kick (trimmed -60 dB, ") && name.ends_with(").wav"), "{name}");
        let audio = mo_audio::read(&copy, 60.0).unwrap();
        // From the first loud frame to the last, with its 10 ms tail.
        assert_eq!(audio.len(), 310);
        assert!(audio.channels[0][0] > 0.4);
        // The second drag drags the same copy; another level makes another.
        assert_eq!(for_drag(&original, -60, &copies).unwrap(), Some(copy.clone()));
        let other = for_drag(&original, -40, &copies).unwrap().unwrap();
        assert_ne!(other, copy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_to_trim_drags_the_original_and_a_missing_file_says_why() {
        let dir = folder("nothing");
        let loud = dir.join("Loud.wav");
        mo_audio::write_wav(&loud, &[vec![0.5; 500]], 1_000, 24).unwrap();
        let copies = dir.join(FOLDER);
        assert_eq!(for_drag(&loud, -60, &copies).unwrap(), None);
        assert!(!copies.exists(), "no folder made for nothing");
        assert!(for_drag(&dir.join("Gone.wav"), -60, &copies).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_name_tells_files_apart_and_stays_the_same() {
        let a = copy_name(Path::new("D:/A/Kick.wav"), -60, 10, 1);
        assert_eq!(a, copy_name(Path::new("D:/A/Kick.wav"), -60, 10, 1));
        assert_ne!(a, copy_name(Path::new("D:/B/Kick.wav"), -60, 10, 1));
        assert_ne!(a, copy_name(Path::new("D:/A/Kick.wav"), -60, 11, 1), "a changed original");
    }
}
