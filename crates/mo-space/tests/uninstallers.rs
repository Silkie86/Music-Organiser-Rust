//! Finding a plugin's uninstaller among the programs Windows lists: the C# app's tests, on made-up lists. Nothing here
//! reads the real list or runs anything.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_space::uninstallers::{Uninstaller, can_run, for_plugin, is_other_version, split};

fn u(name: &str, publisher: &str, command: &str) -> Uninstaller {
    Uninstaller { name: name.into(), publisher: publisher.into(), command: command.into(), bytes: 0 }
}

fn names(found: &[Uninstaller]) -> Vec<&str> {
    found.iter().map(|u| u.name.as_str()).collect()
}

fn installed(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn uninstallers_are_matched_by_name_and_never_a_daws() {
    let all = [
        u("Xfer Serum 2", "Xfer Records", r#""C:\Program Files\Xfer\unins2.exe""#),
        u("Serum", "Xfer Records", r#""C:\Program Files\Xfer\unins1.exe""#),
        u("FL Studio 2026", "Image-Line", r"C:\Program Files\Image-Line\uninstall.exe"),
        u("M1 Backups", "Someone", "x.exe"),
        u("Korg M1", "KORG", "MsiExec.exe /X{1234}"),
    ];
    assert_eq!(names(&for_plugin("Serum", "Xfer Records", &all, &[])), ["Serum", "Xfer Serum 2"]);
    assert_eq!(for_plugin("Serum 2", "Xfer Records", &all, &[])[0].name, "Xfer Serum 2");
    assert!(for_plugin("Fruity Reverb", "Image-Line", &all, &[]).is_empty());
    // A family shorter than two letters ("M1" is "m") finds nothing.
    assert!(for_plugin("M1", "KORG", &all, &[]).is_empty());
    // Three letters: only a program named exactly that.
    let vox = [u("Vox Continental", "Arturia", "a.exe"), u("Vox", "Someone", "b.exe")];
    assert_eq!(names(&for_plugin("Vox", "Someone", &vox, &[])), ["Vox"]);
    assert_eq!(split(&all[0].command), (r"C:\Program Files\Xfer\unins2.exe".into(), String::new()));
    assert_eq!(split("MsiExec.exe /X{1234}"), ("MsiExec.exe".into(), "/X{1234}".into()));
    assert_eq!(
        split(r"C:\Program Files\UVI\unins000.exe /SILENT"),
        (r"C:\Program Files\UVI\unins000.exe".into(), "/SILENT".into())
    );
    assert_eq!(split(r#""C:\Odd\no end quote"#), (r"C:\Odd\no end quote".into(), String::new()));
    assert_eq!(split("uninstall"), ("uninstall".into(), String::new()));
}

#[test]
fn an_uninstaller_whose_file_is_gone_is_never_offered() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-space-uninstallers");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Current")).unwrap();
    let exe = dir.join("Current").join("unins000.exe");
    let command = format!("{} /SILENT", exe.to_string_lossy());
    assert!(!can_run(&command));
    fs::write(&exe, b"").unwrap();
    assert!(can_run(&command));
    assert!(can_run(&format!("\"{}\"", exe.to_string_lossy())));
    // One Windows finds itself.
    assert!(can_run("MsiExec.exe /X{1234}"));
}

#[test]
fn another_versions_uninstaller_is_never_the_one_picked() {
    let ozone11 = u("iZotope Ozone 11 Advanced", "iZotope, Inc.", "x.exe");
    let ozone9 = u("iZotope Ozone 9", "iZotope, Inc.", "y.exe");
    let serum2 = u("Xfer Serum 2", "Xfer Records", "z.exe");
    assert!(is_other_version("Ozone 9", &ozone11, &[]));
    assert!(!is_other_version("Ozone 9", &ozone9, &[]));
    assert_eq!(for_plugin("Ozone 9", "iZotope", &[ozone11.clone(), ozone9.clone()], &[]), [ozone9, ozone11]);
    // A plugin named without a number could be any version: Kontakt 6 installs as "Kontakt", and its uninstaller is
    // picked.
    let kontakt6 = u("Native Instruments Kontakt 6", "Native Instruments", "k6.exe");
    let kontakt7 = u("Native Instruments Kontakt 7", "Native Instruments", "k7.exe");
    assert!(!is_other_version("Kontakt", &kontakt6, &[]));
    assert_eq!(for_plugin("Kontakt", "Native Instruments", std::slice::from_ref(&kontakt6), &[])[0], kontakt6);
    // ...unless that version is installed beside it: then it's that one's (Serum 2's beside Serum, Kontakt 7's beside
    // Kontakt).
    assert!(!is_other_version("Serum", &serum2, &[]));
    assert!(is_other_version("Serum", &serum2, &installed(&["Serum", "Serum2"])));
    assert!(!is_other_version("Serum 2", &serum2, &installed(&["Serum", "Serum2"])));
    assert_eq!(
        for_plugin(
            "Kontakt",
            "Native Instruments",
            &[kontakt7.clone(), kontakt6.clone()],
            &installed(&["Kontakt", "Kontakt 7"])
        ),
        [kontakt6.clone(), kontakt7]
    );
    assert!(!is_other_version("Kontakt", &kontakt6, &installed(&["Kontakt", "Kontakt 7"])));
    assert!(!is_other_version("Diva", &u("u-he Diva v1.4.5", "u-he", "d.exe"), &[]));
    // No number: can't tell.
    assert!(!is_other_version("Ozone 9", &u("Ozone", "iZotope", "o.exe"), &[]));
}

/// The whole-app review of 19 Nov, PM-6: another plugin whose name holds this one's ("Massive X" for Massive) is never
/// picked for you either, as Uninstall takes "pick it yourself" from the first one found.
#[test]
fn another_plugins_uninstaller_is_never_the_one_picked() {
    let massive = u("Native Instruments Massive", "Native Instruments", "m.exe");
    let massive_x = u("Native Instruments Massive X", "Native Instruments", "mx.exe");
    let supermassive = u("ValhallaSupermassive", "Valhalla DSP, LLC", "v.exe");
    assert!(is_other_version("Massive", &massive_x, &[]));
    assert!(is_other_version("Massive", &supermassive, &[]));
    assert!(!is_other_version("Massive", &massive, &[]));
    assert!(!is_other_version("Massive X", &massive_x, &[]));
    // Massive's own comes first; without it, every one found is another's.
    let both = [massive_x.clone(), massive.clone()];
    assert_eq!(for_plugin("Massive", "Native Instruments", &both, &[])[0], massive);
    let found = for_plugin("Massive", "Native Instruments", &[massive_x, supermassive], &[]);
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|u| is_other_version("Massive", u, &[])), "{found:?}");
    // Another plugin's: a word straight after its name.
    assert!(is_other_version("Ozone", &u("iZotope Ozone Imager", "iZotope, Inc.", "i.exe"), &[]));
    assert!(is_other_version("Serum", &u("SerumFX", "Xfer Records", "f.exe"), &[]));
    assert!(is_other_version("Kontakt", &u("Kontakt Factory Library 2", "Native Instruments", "k.exe"), &[]));
    // Not another plugin's: a format, a build, the uninstaller itself, what's in brackets, or a version after it.
    let serums = [
        "Xfer Serum x64",
        "Serum_x64",
        "Serum (64-bit)",
        "Serum VST3",
        "Serum Uninstaller",
        "XferSerum",
        "Xfer Serum 2",
        "Serum mk II",
    ];
    for name in serums {
        assert!(!is_other_version("Serum", &u(name, "Xfer Records", "s.exe"), &[]), "{name}");
    }
    assert!(!is_other_version("Analog Lab V", &u("Arturia Analog Lab V", "Arturia", "a.exe"), &[]));
    assert!(!is_other_version("Ozone 9", &u("iZotope Ozone 9 Advanced", "iZotope, Inc.", "o.exe"), &[]));
    assert!(!is_other_version("TAL-U-NO-LX", &u("TAL-U-NO-LX-V2", "TAL Software", "t.exe"), &[]));
    assert!(!is_other_version("Pro-Q 3", &u("FabFilter Pro-Q 3 x64 3.24", "FabFilter", "q.exe"), &[]));
    // After a version, only an edition or a format is this plugin's: another word is another product's.
    let library = u("Native Instruments Battery 4 Factory Library", "Native Instruments", "l.exe");
    assert!(is_other_version("Battery 4", &library, &[]));
    assert!(!is_other_version("Battery 4", &u("Native Instruments Battery 4", "Native Instruments", "b.exe"), &[]));
    assert!(!is_other_version("Ozone 9", &u("iZotope Ozone 9 Standard Edition", "iZotope, Inc.", "o.exe"), &[]));
    assert!(!is_other_version("Serum 2", &u("Serum 2 (64-bit)", "Xfer Records", "s.exe"), &[]));
    // A maker named as the plugin is: its name on its own is there too.
    assert!(!is_other_version("Vital", &u("Vital Audio Vital", "Vital Audio", "v.exe"), &[]));
    assert!(is_other_version("Vital", &u("Vital Audio VitalFX", "Vital Audio", "v.exe"), &[]));
}
