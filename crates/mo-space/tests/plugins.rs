//! Installed plugins and the room they take, on folders made in the test build's own fixtures folder under fixed
//! names, never the computer's real plugin folders.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::key;
use mo_space::plugin_space::{Choices, SpaceKind, group, measure};
use mo_space::plugins::scan_files;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, bytes: usize) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![0u8; bytes]).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn plugin_files_are_found_once_with_bundles_whole() {
    let dir = fixtures("mo-space-plugin-files");
    let vst3 = dir.join("VST3");
    write(&vst3, "Serum.vst3/Contents/x86_64-win/Serum.vst3", 10);
    write(&vst3, "UVI/UVIWorkstation.vst3", 5);
    write(&vst3, "M1.vst3", 1);
    write(&vst3, "X.vst3", 1);
    write(&vst3, "readme.txt", 1);
    let vst2 = dir.join("VSTPlugins");
    write(&vst2, "Serum_x64.dll", 3);
    let mut found = scan_files(&[vst3.clone(), vst2.clone(), vst3.clone(), dir.join("not there")]);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let names: Vec<(&str, &str)> = found.iter().map(|f| (f.name.as_str(), f.format.as_str())).collect();
    // One letter isn't a name; the file inside Serum's bundle isn't a plugin of its own.
    assert_eq!(names, [("M1", "VST3"), ("Serum", "VST3"), ("UVIWorkstation", "VST3"), ("Serum_x64", "VST2")]);
}

#[test]
fn each_plugin_gets_its_own_folders_and_shares_the_makers() {
    let dir = fixtures("mo-space-plugin-space");
    let vst3 = dir.join("VST3");
    write(&vst3, "Serum.vst3/Contents/Serum.vst3", 10);
    write(&vst3, "UVI/UVIWorkstation.vst3", 3);
    write(&vst3, "Kontakt 7.vst3", 1);
    write(&vst3, "Kontakt 8.vst3", 1);
    write(&vst3, "Xfer/Nerve.vst3", 2);
    let docs = dir.join("Documents");
    write(&docs, "Serum Presets/Lead.fxp", 100);
    // Xfer's own folder: Serum's part counted as Serum's, the rest shared by Xfer's plugins.
    write(&docs, "Xfer/Serum/Tables/Saw.wav", 1000);
    write(&docs, "Xfer/Settings.xml", 30);
    // Kontakt 7 and 8 both take "Kontakt Library": shared.
    write(&docs, "Kontakt Library/Piano.nkx", 5000);
    // Never a plugin's: a DAW's folder, and your own sample folder.
    write(&docs, "Image-Line/Serum/stuff.fxp", 9);
    write(&docs, "Samples/Serum Presets/x.fxp", 9);
    // Ruled out by you, and added by you.
    write(&docs, "Serum Data/old.bin", 7);
    write(&dir, "D Drive/Serum Extra/big.wav", 50);

    let plugin_roots = vec![s(&vst3)];
    let files = scan_files(std::slice::from_ref(&vst3));
    let vendors: HashMap<String, String> =
        [("serum".to_owned(), "Xfer Records".to_owned()), ("nerve".to_owned(), "Xfer Records".to_owned())].into();
    let installed = group(&files, &plugin_roots, &vendors);
    // No maker read by the plugin scanner: the folder it sits in says.
    let uvi = installed.iter().find(|p| p.name == "UVIWorkstation").unwrap();
    assert_eq!(uvi.vendor, "UVI");

    let protected = vec![s(&docs.join("Samples"))];
    let ruled_out = |k: &str| if k == "serum" { vec![s(&docs.join("Serum Data"))] } else { Vec::new() };
    let added = |k: &str| if k == "serum" { vec![s(&dir.join("D Drive/Serum Extra"))] } else { Vec::new() };
    let choices = Choices { added: &added, ruled_out: &ruled_out };
    let stop = AtomicBool::new(false);
    let spaces =
        measure(&installed, std::slice::from_ref(&docs), &plugin_roots, &protected, &choices, None, &stop).unwrap();
    let by_key: HashMap<&str, _> = spaces.iter().map(|p| (p.key.as_str(), p)).collect();

    let serum = by_key["serum"];
    let folders: Vec<(String, SpaceKind, u64, Vec<String>, bool)> = serum
        .folders
        .iter()
        .map(|f| {
            (
                Path::new(&f.path).strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"),
                f.kind,
                f.bytes,
                f.shared_with.clone(),
                f.added,
            )
        })
        .collect();
    assert_eq!(
        folders,
        [
            ("VST3/Serum.vst3".into(), SpaceKind::Program, 10, vec![], false),
            ("Documents/Serum Presets".into(), SpaceKind::Presets, 100, vec![], false),
            ("Documents/Xfer/Serum".into(), SpaceKind::Sounds, 1000, vec![], false),
            // Xfer's own folder, less Serum's part: shared by Xfer's plugins.
            ("Documents/Xfer".into(), SpaceKind::Presets, 30, vec!["nerve".into()], false),
            ("D Drive/Serum Extra".into(), SpaceKind::Sounds, 50, vec![], true),
        ]
    );
    assert_eq!(serum.bytes(), 10 + 100 + 1000 + 50);
    assert_eq!(serum.shared_bytes(), 30);

    assert_eq!(by_key["nerve"].shared_bytes(), 30);

    let k7 = by_key["kontakt7"];
    assert_eq!(k7.sounds(), 0);
    assert_eq!(k7.shared_bytes(), 5000);
    assert_eq!(k7.folders.last().unwrap().shared_with, ["kontakt8"]);

    // Only some measured, still knowing what's shared.
    let only: HashSet<String> = ["kontakt8".to_owned()].into();
    let some =
        measure(&installed, std::slice::from_ref(&docs), &plugin_roots, &protected, &choices, Some(&only), &stop)
            .unwrap();
    assert_eq!(some.len(), 1);
    assert_eq!(some[0].shared_bytes(), 5000);
    assert!(
        spaces.iter().all(|p| p
            .folders
            .iter()
            .all(|f| !key(&f.path).contains("IMAGE-LINE") && !key(&f.path).contains("SAMPLES")))
    );
}
