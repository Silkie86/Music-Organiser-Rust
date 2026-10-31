//! Plugin sounds a save lost, taken from the song's older numbered saves in the same folder (the C# `LostSounds`).
//! Some plugins give FL nothing when it saves (KORG LegacyCell in FL 25), so Solomon's "funk step_9" lost the sound that
//! "funk step_8", saved in FL 9, still has; converting "funk step_9" takes it from there.

use std::path::{Path, PathBuf};

use mo_formats::case::eq_ignore_case;
use mo_formats::fl::events;
use mo_formats::fl::song::{FlPlugin, FlSong, read_song, read_song_with};
use mo_library::songs;

/// Fills in each lost sound from the newest older save that has the same plugin on the same channel (or mixer slot),
/// and says each sound taken and the save it came from. Each older save is read once, newest first, and only until
/// nothing is lost any more. A save that can't be read is passed over: the song converts without it.
pub fn from_older_saves(song: &mut FlSong, path: &Path) -> Vec<String> {
    if !lost(song) {
        return Vec::new();
    }
    let mut taken = Vec::new();
    for file in older_saves(path) {
        let Ok(raw) = std::fs::read(&file) else { continue };
        let Ok(flp) = events::load(&raw) else { continue };
        // A quick look first, without copying out every plugin's sound, at whether it has one of the lost ones.
        if !has_any(&read_song_with(&flp, false), song) {
            continue;
        }
        let old = read_song(&flp);
        let from = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();

        for c in &mut song.channels {
            let Some(p) = c.instrument.as_mut().filter(|p| p.nothing_saved) else { continue };
            let same: Vec<_> =
                old.channels.iter().filter(|o| o.instrument.as_ref().is_some_and(|i| same(i, p))).collect();
            let found = same
                .iter()
                .find(|o| o.index == c.index && o.name == c.name)
                .or_else(|| same.iter().find(|o| o.name == c.name))
                .or_else(|| same.iter().find(|o| o.index == c.index));
            if let Some(state) = found.and_then(|o| o.instrument.as_ref()).and_then(|i| i.state.clone()) {
                taken.push(format!("{}: {}'s sound, from {from}", c.name, p.name));
                p.state = Some(state);
                p.nothing_saved = false;
            }
        }
        for i in &mut song.inserts {
            let label = if i.name.is_empty() { format!("Insert {}", i.index) } else { i.name.clone() };
            let old_effects = old.inserts.iter().find(|o| o.index == i.index).map_or(&[][..], |o| &o.effects[..]);
            for e in i.effects.iter_mut().filter(|e| e.nothing_saved) {
                let same: Vec<_> = old_effects.iter().filter(|o| saved(o) && same(o, e)).collect();
                let found = same.iter().find(|o| o.slot == e.slot).or_else(|| same.first());
                if let Some(state) = found.and_then(|o| o.state.clone()) {
                    taken.push(format!("{label}: {}'s sound, from {from}", e.name));
                    e.state = Some(state);
                    e.nothing_saved = false;
                }
            }
        }
        if !lost(song) {
            break;
        }
    }
    taken
}

/// The song's older saves beside it, newest first: the same song name with a lower number.
fn older_saves(path: &Path) -> Vec<PathBuf> {
    let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return Vec::new() };
    let (name, number) = songs::split(&stem);
    let Some(dir) = std::path::absolute(path).ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut older: Vec<(i32, bool, PathBuf)> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("flp")))
        .filter_map(|f| {
            let stem = f.file_stem()?.to_string_lossy().into_owned();
            let (song, n) = songs::split(&stem);
            (eq_ignore_case(&song, &name) && n < number).then_some((n, songs::is_fixed_copy(&stem), f))
        })
        .collect();
    // Newest first; of a save and Fix's copy of it, the save itself first.
    older.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    older.into_iter().map(|(_, _, f)| f).collect()
}

fn lost(song: &FlSong) -> bool {
    song.channels.iter().any(|c| c.instrument.as_ref().is_some_and(|p| p.nothing_saved))
        || song.inserts.iter().any(|i| i.effects.iter().any(|e| e.nothing_saved))
}

/// The plugins on a song's channels and in its mixer.
fn plugins(song: &FlSong) -> impl Iterator<Item = &FlPlugin> {
    song.channels.iter().filter_map(|c| c.instrument.as_ref()).chain(song.inserts.iter().flat_map(|i| &i.effects))
}

/// Whether an older save, read without its sounds, may have one of the song's lost sounds: the same plugin, and it
/// didn't give FL nothing there.
fn has_any(old: &FlSong, song: &FlSong) -> bool {
    let lost: Vec<_> = plugins(song).filter(|p| p.nothing_saved).collect();
    plugins(old).any(|o| !o.nothing_saved && lost.iter().any(|l| same(o, l)))
}

/// A plugin with a sound saved for it.
fn saved(p: &FlPlugin) -> bool {
    p.state.is_some()
}

/// The same plugin: its id when both saves have one (an FL 9 save may not), else its name.
fn same(old: &FlPlugin, lost: &FlPlugin) -> bool {
    old.format == lost.format
        && if lost.device_id.is_empty() || old.device_id.is_empty() {
            eq_ignore_case(&old.name, &lost.name)
        } else {
            old.device_id == lost.device_id
        }
}
