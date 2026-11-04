//! The window draws with the app's patched copy of Slint's drawing library, `vendor/femtovg` (its `PATCHED.md` says
//! why): the crates.io femtovg 0.27.0 built its graphics card steps again on every frame. Cargo only warns when a patch
//! stops applying, for example once Slint asks for a newer femtovg, so these checks make it fail out loud instead.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

/// What to do once the copy isn't used any more.
const WHEN_UNUSED: &str = "If Slint now uses femtovg 0.28 or later, the copy isn't needed: delete vendor/femtovg and \
                           the lines vendor/femtovg/PATCHED.md lists. Otherwise the copy's version must match the one \
                           Slint asks for again.";

/// The `femtovg` entries in a lock file, and whether the lock file lists a patch Cargo didn't use.
fn femtovg_in(lock: &Path) -> (Vec<String>, bool) {
    let text = std::fs::read_to_string(lock).expect("the lock file is readable");
    let entries = text
        .split("[[package]]")
        .filter(|entry| entry.lines().any(|line| line.trim() == "name = \"femtovg\""))
        .map(str::to_owned)
        .collect();
    (entries, text.contains("[[patch.unused]]"))
}

fn check(lock: &Path) {
    let (entries, unused) = femtovg_in(lock);
    assert!(!unused, "{} lists a patch Cargo didn't use. {WHEN_UNUSED}", lock.display());
    assert_eq!(entries.len(), 1, "{} has {} femtovg entries. {WHEN_UNUSED}", lock.display(), entries.len());
    // A crate from crates.io has a source line; the copy in vendor/ has none.
    assert!(
        !entries[0].contains("source ="),
        "{} takes femtovg from crates.io, not vendor/femtovg. {WHEN_UNUSED}",
        lock.display()
    );
}

#[test]
fn the_app_draws_with_the_patched_drawing_library() {
    check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"));
}

#[test]
fn the_galaxy_check_draws_with_it_too() {
    check(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/galaxycheck/Cargo.lock"));
}

#[test]
fn the_copy_keeps_its_fix() {
    let renderer = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/femtovg/src/renderer/wgpu.rs");
    let source = std::fs::read_to_string(renderer).expect("the copy's renderer is readable");
    assert!(
        source.contains("if pipeline_cache.len() > PIPELINE_CACHE_MOST {"),
        "vendor/femtovg/src/renderer/wgpu.rs cleans its pipelines after every render again (PATCHED.md)"
    );
}
