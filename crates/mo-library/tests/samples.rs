//! Finding samples in real folders, made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_library::samples::SampleLocator;

#[test]
fn finds_samples_beside_in_the_tree_and_in_the_daws_folders() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples");
    let _ = fs::remove_dir_all(&dir);
    for f in [
        "Songs/Night Drive/samples/kick.wav",
        "Songs/Night Drive/Bounce/deep/vox.wav",
        "Songs/Night Drive/Backup/old.wav",
        "Content/Drums/snare.wav",
        "Content/Other/hat.wav",
        "Content/Projects/Other Song/snare2.wav",
    ] {
        let p = dir.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }
    let project = dir.join("Songs/Night Drive/Night Drive.flp").to_str().unwrap().to_owned();
    let content = dir.join("Content").to_str().unwrap().to_owned();
    let locator = SampleLocator::new(Box::new(move |_| vec![content.clone()]));
    // Where the project says, relative to it.
    assert!(locator.found("samples/kick.wav", &project));
    // Gone from where it was, found beside the project in a samples folder.
    assert!(locator.found("/nowhere/at/all/kick.wav", &project));
    // Somewhere in the project's tree, but not in its Backup folder.
    assert!(locator.found("/old/vox.wav", &project));
    assert!(!locator.found("/old/old.wav", &project));
    // In the DAW's folders only when the folder it's in has the same name too.
    assert!(locator.found("/old/Drums/snare.wav", &project));
    assert!(!locator.found("/old/Drums/hat.wav", &project));
    assert!(!locator.found("/old/Other Song/snare2.wav", &project));
    // FL Studio's own content always counts.
    assert!(locator.found("%FLStudioFactoryData%/Data/Patches/Packs/x.wav", &project));
    assert!(!locator.found("/old/missing.wav", &project));
    assert!(!locator.found("", &project));
}

#[cfg(unix)]
#[test]
fn folder_links_back_up_the_tree_are_walked_once() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples-links");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Song/deep")).unwrap();
    fs::write(dir.join("Song/deep/vox.wav"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("Song"), dir.join("Song/deep/a")).unwrap();
    std::os::unix::fs::symlink(dir.join("Song"), dir.join("Song/deep/b")).unwrap();
    let project = dir.join("Song/Song.flp").to_str().unwrap().to_owned();
    let locator = SampleLocator::new(Box::new(|_| Vec::new()));
    assert!(locator.found("/old/vox.wav", &project));
    assert!(!locator.found("/old/gone.wav", &project));
}
