//! Walking real sample folders and planning a scan. The folders are made in the test build's own fixtures folder,
//! under a fixed name, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_sounds::KnownSound;
use mo_sounds::scan::{find_sounds, plan};

fn folder(name: &str, files: &[(&str, usize)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    for (path, bytes) in files {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; *bytes]).unwrap();
    }
    dir
}

fn rel(root: &str, path: &str) -> String {
    path[root.len() + 1..].replace('\\', "/")
}

#[test]
fn finds_sounds_leaving_out_shadows_hidden_folders_and_other_files() {
    let dir = folder(
        "mo-sounds-find",
        &[
            ("Kicks/Kick 1.wav", 3),
            ("Kicks/Kick 2.AIF", 3),
            ("Loops/Groove.flac", 3),
            ("Loops/._Groove.flac", 1),
            ("__MACOSX/Loops/Groove.flac", 1),
            (".cache/x.wav", 1),
            ("Docs/readme.txt", 1),
            ("Old/Song.wma", 1),
        ],
    );
    let root = dir.to_str().unwrap().to_owned();
    let found = find_sounds(std::slice::from_ref(&root));
    let names: Vec<String> = found.files.iter().map(|f| rel(&root, &f.path)).collect();
    assert_eq!(names, ["Kicks/Kick 1.wav", "Kicks/Kick 2.AIF", "Loops/Groove.flac"]);
    assert!(found.files.iter().all(|f| f.size == 3 && f.mtime > 0));
}

#[test]
fn plans_what_to_listen_to_follow_and_forget() {
    let dir = folder(
        "mo-sounds-plan",
        &[("Pack 2/Kick.wav", 4), ("Pack 2/Pad 90bpm.wav", 5), ("New.wav", 6), ("Same.wav", 7), ("Song.wma", 8)],
    );
    let root = dir.to_str().unwrap().to_owned();
    let found = find_sounds(std::slice::from_ref(&root));
    let stamp = |name: &str| found.files.iter().find(|f| f.path.ends_with(name)).map(|f| (f.size, f.mtime)).unwrap();
    let at = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let known = |path: String, (size, mtime): (i64, i64)| KnownSound { path, size, mtime, has_clap: false };
    let unplugged = r"Z:\Unplugged".to_owned();
    let known = [
        // Renamed folder: follows, and isn't listened to again.
        known(at("Pack/Kick.wav"), stamp("Kick.wav")),
        // Renamed with a tempo now in its name: follows, and is listened to again.
        known(at("Pack/Pad.wav"), stamp("Pad 90bpm.wav")),
        // Unchanged.
        known(at("Same.wav"), stamp("Same.wav")),
        // Gone for good.
        known(at("Gone.wav"), (1, 1)),
        // A file the app can't read, still there: kept.
        known(at("Song.wma"), (8, 1)),
        // On a drive that isn't plugged in: kept.
        known(format!(r"{unplugged}\Far.wav"), (1, 1)),
        // Outside every listed folder: kept, unlike the C# app.
        known(r"Q:\Elsewhere\Kick.wav".to_owned(), (4, 4)),
    ];
    let p = plan(&found, &known, &[root.clone(), unplugged]);
    let listen: Vec<String> = p.listen.iter().map(|f| rel(&root, &f.path)).collect();
    assert_eq!(listen, ["New.wav", "Pack 2/Pad 90bpm.wav"]);
    let moves: Vec<(String, String)> = p.moves.iter().map(|(f, t)| (rel(&root, f), rel(&root, t))).collect();
    assert_eq!(moves.len(), 2);
    assert!(moves.contains(&("Pack/Kick.wav".into(), "Pack 2/Kick.wav".into())));
    assert!(moves.contains(&("Pack/Pad.wav".into(), "Pack 2/Pad 90bpm.wav".into())));
    assert_eq!(p.forget, [at("Gone.wav")]);
}
