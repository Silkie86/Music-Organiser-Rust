//! Installed plugins and the room they take, on folders made in the test build's own fixtures folder under fixed
//! names, never the computer's real plugin folders.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::key;
use mo_space::plugin_clean::{Library, PluginUse, Stays, lines};
use mo_space::plugin_space::{Choices, SpaceKind, group, measure};
use mo_space::plugins::{pro_tools_app_in, scan_files};

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
    let stop = AtomicBool::new(false);
    let mut found = scan_files(&[vst3.clone(), vst2.clone(), vst3.clone(), dir.join("not there")], &stop).unwrap();
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
    let files = scan_files(std::slice::from_ref(&vst3), &AtomicBool::new(false)).unwrap();
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

#[test]
fn versions_folders_and_your_choices_are_each_counted_once() {
    let dir = fixtures("mo-space-plugin-choices");
    let vst3 = dir.join("VST3");
    write(&vst3, "Kontakt 7.vst3", 1);
    write(&vst3, "Kontakt 8.vst3", 1);
    write(&vst3, "Serum.vst3", 1);
    write(&vst3, "Nerve.vst3", 1);
    let docs = dir.join("Documents");
    write(&docs, "Kontakt 8 Library/Strings.nkx", 300);
    write(&docs, "Kontakt Library/Piano.nkx", 5000);
    write(&docs, "Xfer/Serum/Tables/Saw.wav", 1000);
    write(&docs, "Xfer/Settings.xml", 30);
    let plugin_roots = vec![s(&vst3)];
    let files = scan_files(std::slice::from_ref(&vst3), &AtomicBool::new(false)).unwrap();
    let vendors: HashMap<String, String> =
        [("serum".to_owned(), "Xfer Records".to_owned()), ("nerve".to_owned(), "Xfer Records".to_owned())].into();
    let installed = group(&files, &plugin_roots, &vendors);
    // Kontakt 7 adds the shared library as its own; Serum says Xfer\Serum isn't its.
    let kontakt_library = s(&docs.join("Kontakt Library"));
    let added = |k: &str| if k == "kontakt7" { vec![kontakt_library.clone()] } else { Vec::new() };
    let ruled_out = |k: &str| if k == "serum" { vec![s(&docs.join("Xfer/Serum"))] } else { Vec::new() };
    let choices = Choices { added: &added, ruled_out: &ruled_out };
    let spaces =
        measure(&installed, std::slice::from_ref(&docs), &plugin_roots, &[], &choices, None, &AtomicBool::new(false))
            .unwrap();
    let by_key: HashMap<&str, _> = spaces.iter().map(|p| (p.key.as_str(), p)).collect();
    // "Kontakt 8 Library" names its version: Kontakt 8's alone.
    assert_eq!(by_key["kontakt8"].sounds(), 300);
    // The library Kontakt 7 added is still Kontakt 8's guess too: shared, so it stays while Kontakt 8 is used.
    assert_eq!(by_key["kontakt7"].sounds(), 0);
    assert_eq!(by_key["kontakt7"].shared_bytes(), 5000);
    assert_eq!(by_key["kontakt8"].shared_bytes(), 5000);
    // Xfer\Serum, ruled out, stays part of Xfer's shared folder rather than being counted nowhere.
    assert_eq!(by_key["serum"].shared_bytes(), 1030);
    assert_eq!(by_key["nerve"].shared_bytes(), 1030);
    let total: u64 = spaces.iter().map(|p| p.bytes()).sum();
    assert_eq!(total, 4 + 300);

    // Ruled out for Kontakt 8, it's Kontakt 7's alone.
    let ruled_out = |k: &str| match k {
        "serum" => vec![s(&docs.join("Xfer/Serum"))],
        "kontakt8" => vec![kontakt_library.clone()],
        _ => Vec::new(),
    };
    let choices = Choices { added: &added, ruled_out: &ruled_out };
    let spaces =
        measure(&installed, std::slice::from_ref(&docs), &plugin_roots, &[], &choices, None, &AtomicBool::new(false))
            .unwrap();
    let by_key: HashMap<&str, _> = spaces.iter().map(|p| (p.key.as_str(), p)).collect();
    assert_eq!(by_key["kontakt7"].sounds(), 5000);
    assert_eq!(by_key["kontakt8"].shared_bytes(), 0);
}

#[test]
fn a_folder_two_plugins_added_is_shared_and_stays_while_either_is_used() {
    let dir = fixtures("mo-space-plugin-added-twice");
    let vst3 = dir.join("VST3");
    write(&vst3, "Kontakt 7.vst3", 1);
    write(&vst3, "Kontakt 8.vst3", 1);
    write(&dir, "D Drive/Libs/Piano.nkx", 5000);
    let plugin_roots = vec![s(&vst3)];
    let files = scan_files(std::slice::from_ref(&vst3), &AtomicBool::new(false)).unwrap();
    let installed = group(&files, &plugin_roots, &HashMap::new());
    // The guess never finds a library on another drive, so you add it to both.
    let libs = s(&dir.join("D Drive/Libs"));
    let added = |_: &str| vec![libs.clone()];
    let none = |_: &str| Vec::new();
    let choices = Choices { added: &added, ruled_out: &none };
    let stop = AtomicBool::new(false);
    let spaces = measure(&installed, &[], &plugin_roots, &[], &choices, None, &stop).unwrap();
    let by_key: HashMap<&str, _> = spaces.iter().map(|p| (p.key.as_str(), p)).collect();
    let shared = |k: &str| by_key[k].folders.iter().find(|f| key(&f.path) == key(&libs)).unwrap().shared_with.clone();
    assert_eq!(shared("kontakt7"), ["kontakt8"]);
    assert_eq!(shared("kontakt8"), ["kontakt7"]);
    // Counted once, as shared, not as each one's own.
    assert_eq!(spaces.iter().map(|p| p.bytes()).sum::<u64>(), 2);
    assert_eq!(by_key["kontakt7"].shared_bytes(), 5000);

    // Kontakt 7 isn't used, Kontakt 8 is: the library stays on Kontakt 7's clean-up card.
    let uses: HashMap<String, PluginUse> = [
        ("kontakt7".to_owned(), PluginUse { projects: 0, unused: true }),
        ("kontakt8".to_owned(), PluginUse { projects: 50, unused: false }),
    ]
    .into();
    let (no_sample, no_name) = (|_: &str| None, |_: &str| false);
    let names = |k: &str| k.to_owned();
    let library = Library {
        protected_roots: &[],
        kept_folders: &[],
        uses: &uses,
        sample_at: &no_sample,
        sample_named: &no_name,
        names: &names,
    };
    let card = lines(by_key["kontakt7"], &spaces, &library, &stop).unwrap();
    let line = card.iter().find(|l| key(&l.folder.path) == key(&libs)).unwrap();
    assert_eq!(line.stays, Some(Stays::SharedWithUsed(vec!["kontakt8".to_owned()])));
    assert!(!line.ticked);
}

#[test]
fn a_folder_you_added_inside_a_makers_folder_counts_once() {
    let dir = fixtures("mo-space-plugin-added-inside");
    let vst3 = dir.join("VST3");
    write(&vst3, "Serum.vst3", 10);
    let docs = dir.join("Documents");
    write(&docs, "Xfer/Serum/Saw.wav", 100);
    write(&docs, "Xfer/Wavetables/Big.wav", 1000);
    let plugin_roots = vec![s(&vst3)];
    let files = scan_files(std::slice::from_ref(&vst3), &AtomicBool::new(false)).unwrap();
    let vendors: HashMap<String, String> = [("serum".to_owned(), "Xfer Records".to_owned())].into();
    let installed = group(&files, &plugin_roots, &vendors);
    // The guess missed the wavetables, so you add them.
    let tables = s(&docs.join("Xfer/Wavetables"));
    let added = |_: &str| vec![tables.clone()];
    let none = |_: &str| Vec::new();
    let choices = Choices { added: &added, ruled_out: &none };
    let spaces =
        measure(&installed, std::slice::from_ref(&docs), &plugin_roots, &[], &choices, None, &AtomicBool::new(false))
            .unwrap();
    let serum = &spaces[0];
    let mut said: Vec<(String, u64)> = serum.folders.iter().map(|f| (key(&f.path), f.bytes)).collect();
    said.sort();
    // Xfer holds nothing besides: it's left off.
    let k = |rel: &str| key(&s(&dir.join(rel)));
    let mut expected =
        vec![(k("VST3/Serum.vst3"), 10), (k("Documents/Xfer/Serum"), 100), (k("Documents/Xfer/Wavetables"), 1000)];
    expected.sort();
    assert_eq!(said, expected);
    // What's on disk, once.
    assert_eq!(serum.bytes(), 10 + 100 + 1000);
}

#[cfg(unix)]
#[test]
fn a_plugin_file_and_a_link_to_it_count_once() {
    let dir = fixtures("mo-space-plugin-file-link");
    write(&dir, "VSTPlugins/Serum_x64.dll", 70);
    fs::create_dir_all(dir.join("Steinberg")).unwrap();
    std::os::unix::fs::symlink(dir.join("VSTPlugins/Serum_x64.dll"), dir.join("Steinberg/Serum_x64.dll")).unwrap();
    let roots = vec![dir.join("VSTPlugins"), dir.join("Steinberg")];
    let plugin_roots: Vec<String> = roots.iter().map(|r| s(r)).collect();
    let files = scan_files(&roots, &AtomicBool::new(false)).unwrap();
    let installed = group(&files, &plugin_roots, &HashMap::new());
    let none = |_: &str| Vec::new();
    let choices = Choices { added: &none, ruled_out: &none };
    let spaces = measure(&installed, &[], &plugin_roots, &[], &choices, None, &AtomicBool::new(false)).unwrap();
    assert_eq!(spaces[0].program(), 70);
}

#[cfg(unix)]
#[test]
fn plugins_moved_behind_links_are_found_and_measured_where_they_are() {
    let dir = fixtures("mo-space-plugin-links");
    let vst3 = dir.join("VST3");
    write(&dir, "D Drive/Omnisphere.vst3/Contents/x.so", 70);
    write(&dir, "D Drive/Moved/Diva.vst3", 20);
    fs::create_dir_all(&vst3).unwrap();
    std::os::unix::fs::symlink(dir.join("D Drive/Omnisphere.vst3"), vst3.join("Omnisphere.vst3")).unwrap();
    std::os::unix::fs::symlink(dir.join("D Drive/Moved"), vst3.join("Moved")).unwrap();
    // A link back up doesn't go round for ever.
    std::os::unix::fs::symlink(&vst3, vst3.join("Moved").join("Loop")).unwrap();
    let files = scan_files(std::slice::from_ref(&vst3), &AtomicBool::new(false)).unwrap();
    let mut names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Diva", "Omnisphere"]);
    let installed = group(&files, &[s(&vst3)], &HashMap::new());
    let none = |_: &str| Vec::new();
    let choices = Choices { added: &none, ruled_out: &none };
    let spaces = measure(&installed, &[], &[s(&vst3)], &[], &choices, None, &AtomicBool::new(false)).unwrap();
    let omni = spaces.iter().find(|p| p.name == "Omnisphere").unwrap();
    assert_eq!(omni.program(), 70);
}

#[test]
fn pro_tools_is_found_by_any_of_its_app_names_and_in_a_folder() {
    let dir = fixtures("mo-space-pro-tools");
    let none = dir.join("None");
    fs::create_dir_all(none.join("Avid/Media Composer.app")).unwrap();
    fs::create_dir_all(none.join("Logic Pro.app")).unwrap();
    assert!(!pro_tools_app_in(&none));
    for (name, app) in
        [("Plain", "Pro Tools.app"), ("First", "Pro Tools First.app"), ("In a folder", "Avid/Pro Tools.app")]
    {
        fs::create_dir_all(dir.join(name).join(app)).unwrap();
        assert!(pro_tools_app_in(&dir.join(name)), "{app}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_vst2_plugins_are_so_files() {
    let dir = fixtures("mo-space-plugin-linux");
    write(&dir, ".vst/Dexed.so", 5);
    let files = scan_files(&[dir.join(".vst")], &AtomicBool::new(false)).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!((files[0].name.as_str(), files[0].format.as_str()), ("Dexed", "VST2"));
}

#[test]
fn a_stopped_scan_says_so() {
    let dir = fixtures("mo-space-plugin-stop");
    write(&dir, "VST3/A.vst3", 1);
    assert_eq!(scan_files(&[dir.join("VST3")], &AtomicBool::new(true)), Err(mo_space::Stopped));
}
