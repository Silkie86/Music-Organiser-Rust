//! "Sounds like this" keeps CLAP's numbers, and the analyser's fingerprints, each in a file beside the app's data,
//! read as searches need them, instead of in the app's own memory (Solomon, 2026-11-25): see `mo_sounds::index`'s
//! `kept` for the files themselves. The app alone: the plugin writes none of the app's files, and lets go of its
//! index as soon as it has used it.

use std::path::Path;
use std::time::Instant;

use mo_sounds::index::SoundIndex;

/// Puts the CLAP numbers of `index` (just built) in their file in `folder`, then the analyser's fingerprints in
/// theirs ([`keep_prints`]), and says in the log how it went. With no CLAP index, files of earlier ones are removed.
/// When it can't be done the index works as before, from memory. Reads or writes a file as big as the numbers, so
/// never on the window's thread, nor on the pool the pages' quick jobs share.
pub fn keep(index: &mut SoundIndex, folder: &Path) {
    hear_trouble();
    keep_clap(index, folder);
    keep_prints(index, folder);
}

/// A file that stops being readable later (its drive gone) is said once in the log; searches then find nothing.
fn hear_trouble() {
    mo_sounds::index::on_kept_trouble(|what| {
        crate::log::warning("Sounds", &format!("Sounds like this can't read its numbers: {what}"));
    });
}

/// Puts the analyser's fingerprints of `index` (just built) in their file in `folder`, and says in the log how it
/// went. With none, files of earlier ones are removed. Not after a scan changes a few (they're held again until the
/// index is next built): the whole file isn't written again for those.
fn keep_prints(index: &mut SoundIndex, folder: &Path) {
    let began = Instant::now();
    if index.features.is_empty() {
        let removed = mo_sounds::index::remove_kept_prints(folder);
        if removed > 0 {
            crate::log::info("Sounds", &format!("Fingerprints: {removed} file(s) of numbers no longer used removed"));
        }
        return;
    }
    match index.keep_features_in(folder) {
        None => {}
        Some(Ok(kept)) => {
            let size = mo_space::size_text(kept.bytes);
            let how = if kept.written { "written" } else { "found from last time and checked" };
            let older = if kept.removed > 0 { format!("; {} older removed", kept.removed) } else { String::new() };
            crate::log::info(
                "Sounds",
                &format!(
                    "Fingerprints: the analyser's numbers are read from a file as they're needed, {size} the app no \
                     longer holds ({how} in {} ms{older})",
                    began.elapsed().as_millis()
                ),
            );
        }
        Some(Err(why)) => crate::log::warning(
            "Sounds",
            &format!("Fingerprints: the analyser's numbers couldn't be kept in a file, so they stay in memory: {why}"),
        ),
    }
}

fn keep_clap(index: &mut SoundIndex, folder: &Path) {
    let began = Instant::now();
    if index.clap.is_none() {
        let removed = mo_sounds::index::remove_kept(folder);
        if removed > 0 {
            crate::log::info(
                "Sounds",
                &format!("Sounds like this: {removed} file(s) of numbers no longer used removed"),
            );
        }
        return;
    }
    match index.keep_clap_in(folder) {
        None => {}
        Some(Ok(kept)) => {
            let size = mo_space::size_text(kept.bytes);
            let how = if kept.written { "written" } else { "found from last time and checked" };
            let older = if kept.removed > 0 { format!("; {} older removed", kept.removed) } else { String::new() };
            crate::log::info(
                "Sounds",
                &format!(
                    "Sounds like this: its numbers are read from a file as they're needed, {size} the app no longer \
                     holds ({how} in {} ms{older})",
                    began.elapsed().as_millis()
                ),
            );
        }
        Some(Err(why)) => crate::log::warning(
            "Sounds",
            &format!("Sounds like this: its numbers couldn't be kept in a file, so they stay in memory: {why}"),
        ),
    }
}
