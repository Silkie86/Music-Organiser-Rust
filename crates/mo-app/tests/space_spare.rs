//! The Space page's look for spare plugin copies, from start to end, on plugin folders made in the test build's own
//! fixtures folder under fixed names: never the computer's real plugin folders. The plugin files are made-up bytes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::space_spare::{Found, look, plugin_folders};
use mo_space::spare::{Used, Why};

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

/// What a look found, as file names, formats and sizes.
fn summary(found: &Found) -> Vec<(String, String, u64)> {
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    found.copies.iter().map(|c| (name(&c.path), c.format.clone(), c.bytes)).collect()
}

#[test]
fn copies_in_formats_your_saves_dont_use_are_found_and_measured() {
    let dir = fixtures("mo-app-space-spare");
    let (vst3, vst2, avid) = (dir.join("VST3"), dir.join("VSTPlugins"), dir.join("Avid Plug-Ins"));
    // Serum in every format: the saves use its VST3.
    write(&vst3, "Serum.vst3/Contents/x86_64-win/Serum.vst3", 300);
    write(&vst2, "Serum_x64.dll", 120);
    write(&avid, "Serum.aaxplugin/Contents/x64/Serum.aaxplugin", 200);
    write(&avid, "Serum.aaxplugin/Contents/Resources/skin.png", 50);
    // Diva: the saves use its VST2, so its VST3 is the spare one.
    write(&vst3, "Diva.vst3", 80);
    write(&vst2, "Diva(x64).dll", 70);
    // Sylenth1: an old FL Studio save says only "VST", so every copy of it counts as used; and no save uses Vital, so
    // the Plugins page's Never used shows it whole and none of it is a spare copy.
    write(&vst3, "Sylenth1.vst3", 40);
    write(&vst2, "Sylenth1.dll", 30);
    write(&vst3, "Vital.vst3", 20);
    write(&vst2, "Vital.dll", 10);
    // The saves (by their ids in the library) that use each format.
    let saves = |ids: std::ops::Range<i64>| ids.collect::<HashSet<i64>>();
    let used: Used = HashMap::from([
        ("Serum".to_owned(), HashMap::from([("VST3".to_owned(), saves(0..412))])),
        ("Diva".to_owned(), HashMap::from([("VST2".to_owned(), saves(412..419))])),
        ("Sylenth1".to_owned(), HashMap::from([("VST".to_owned(), saves(419..422))])),
    ]);
    let folders = plugin_folders(&[], vec![vst3.clone(), vst2.clone()], vec![avid.clone()]);
    let stop = AtomicBool::new(false);

    // Without Pro Tools, Serum's AAX copy is spare too, measured whole (every file in its bundle).
    let found = look(&folders, &used, false, &stop).unwrap();
    assert_eq!(
        summary(&found),
        [
            ("Serum.aaxplugin".to_owned(), "AAX".to_owned(), 250),
            ("Serum_x64.dll".to_owned(), "VST2".to_owned(), 120),
            ("Diva.vst3".to_owned(), "VST3".to_owned(), 80),
        ]
    );
    assert!(found.copies.iter().all(|c| c.can_remove()));
    assert_eq!(found.bytes(), 450);
    assert_eq!(found.copies[0].why, Why::NoProTools);
    match &found.copies[1].why {
        Why::OtherFormatsUsed { used, projects, .. } => {
            assert_eq!((used.as_slice(), *projects), (&["VST3".to_owned()][..], 412))
        }
        other => panic!("Serum's VST2 is spare because of its VST3, not {other:?}"),
    }

    // With Pro Tools, AAX copies are in use.
    let found = look(&folders, &used, true, &stop).unwrap();
    assert_eq!(summary(&found).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["Serum_x64.dll", "Diva.vst3"]);

    // Your own plugin folders, when Settings lists any, are looked in instead of the standard ones.
    let only_vst2 = plugin_folders(&[vst2.to_string_lossy().into_owned()], vec![vst3], vec![avid]);
    let found = look(&only_vst2, &used, false, &stop).unwrap();
    // With Serum's VST3 not looked at, its VST2 may be the copy the saves load: it stays. So does Diva's VST2, which
    // they use; only the AAX copy is spare.
    assert_eq!(summary(&found).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["Serum.aaxplugin"]);
}
