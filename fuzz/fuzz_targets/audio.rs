//! Sound files (WAV, AIFF, MP3, FLAC, OGG, M4A, AAC): what the Sounds page and the player read from each. The first
//! byte picks the file's ending, the rest is the file. The readers take a path, so each made-up file is written to a
//! scratch file first (in memory on Linux). A reader may say the file is damaged, but must never panic or hang.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

const ENDINGS: &[&str] = &["wav", "aif", "aiff", "mp3", "flac", "ogg", "m4a", "aac", "aifc"];

/// The scratch folder, the same for every fuzzing process: `/dev/shm` (memory) where there is one, else the temp
/// folder. Each process names its file by its own number and takes it away after each read, so nothing is left in
/// memory after a run.
fn folder() -> &'static PathBuf {
    static FOLDER: OnceLock<PathBuf> = OnceLock::new();
    FOLDER.get_or_init(|| {
        let shm = PathBuf::from("/dev/shm");
        let base = if shm.is_dir() { shm } else { std::env::temp_dir() };
        let dir = base.join("mo-fuzz-audio");
        std::fs::create_dir_all(&dir).expect("make the scratch folder");
        dir
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, raw)) = data.split_first() else { return };
    let ending = ENDINGS[usize::from(pick) % ENDINGS.len()];
    let path = folder().join(format!("fuzz-{}.{ending}", std::process::id()));
    if std::fs::write(&path, raw).is_err() {
        return;
    }
    // The first 30 seconds (the waveform and the player), from a point part way in, and in parts (the analysis).
    let _ = mo_audio::read(&path, 30.0);
    let _ = mo_audio::read_from(&path, 5.0, 1000);
    let mut parts = 0;
    let _ = mo_audio::read_parts(&path, 1.0, &mut |_, _| {
        parts += 1;
        parts < 64
    });
    let _ = mo_audio::reads_exactly(&path);
    let _ = std::fs::remove_file(&path);
});
