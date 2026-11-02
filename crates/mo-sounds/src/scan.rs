//! Finding the sounds in your sample folders and working out what a scan has to do: which files are new or changed
//! and need listening to, which were renamed or moved (they keep everything you gave them), and which are really gone.
//! A port of the C# `SoundOrganiser.Scan` and `Moves`, without the listening itself (that comes with the analyser).
//!
//! Nothing here writes: the plan says what to do, and whatever writes the library carries it out.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use mo_formats::case;
use mo_formats::paths::{file_name, has_extension};
use mo_library::scan::{Found, FoundFile, find_files, is_inside};

use crate::KnownSound;
use crate::types::{key_from_name, loop_from_path, tempo_from_name};

/// The sound files the app reads, on every system.
///
/// The C# app reads WAV and AIFF itself and MP3, FLAC, M4A, OGG, WMA and AAC through Windows only. The Rust app decodes
/// all but WMA itself on every system, so a Mac or Linux lists them too. WMA is decoded by Windows (as in the C# app),
/// so it's listed there only; elsewhere it's left out, and like any file the app can't read, it isn't forgotten
/// either: its sound keeps what you gave it.
pub const AUDIO_EXTENSIONS: [&str; 9] = ["wav", "aif", "aiff", "aifc", "mp3", "flac", "m4a", "ogg", "aac"];

/// Whether the app reads this kind of sound file on this computer (by its extension, ignoring case).
pub fn can_read(path: &str) -> bool {
    let name = file_name(path);
    has_extension(name, &AUDIO_EXTENSIONS) || (cfg!(windows) && has_extension(name, &mo_audio::WMA_EXTENSIONS))
}

/// A Mac's "._" shadow file, which holds no sound.
pub fn is_mac_shadow(path: &str) -> bool {
    file_name(path).starts_with("._")
}

/// A hidden folder (".git"), or the "__MACOSX" folder a zip made on a Mac keeps its shadows in.
pub fn is_hidden_folder(name: &str) -> bool {
    name.starts_with('.') || case::eq_ignore_case(name, "__MACOSX")
}

/// A file a scan lists: one the app reads, not a shadow.
pub fn is_sound_file(path: &str) -> bool {
    can_read(path) && !is_mac_shadow(path)
}

/// Every sound file in `folders`, with the size and modified time the folder listing gave, walked on every core of the
/// current rayon pool (the caller runs it on the low-priority pool). Folders that couldn't be read this time are listed
/// in `unreadable`, so their sounds aren't taken for gone.
pub fn find_sounds(folders: &[String]) -> Found {
    find_files(folders, &is_sound_file, &|name, _parent| is_hidden_folder(name))
}

/// What a scan has to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanPlan {
    /// New and changed files to listen to, in path order (renamed files whose new name says the same are left out).
    pub listen: Vec<FoundFile>,
    /// Files renamed or moved: (old path, new path). Each keeps its row, so its tags, stars, type, plays and galaxy
    /// place stay with it.
    pub moves: Vec<(String, String)>,
    /// Paths of sounds really gone, to forget.
    pub forget: Vec<String>,
}

/// Works out what a scan of `folders` has to do, from what [`find_sounds`] found and what the library knows.
///
/// As the C# app: a folder on an unplugged drive, or one that couldn't be read this time, keeps its sounds; so does a
/// file still there that the app can't read. Only files really gone are forgotten.
///
/// Unlike the C# app, a sound outside every listed folder is never forgotten (the C# app forgot it): a folder taken
/// off the list for a while, or a list that failed to load, mustn't wipe the tags, stars and plays you gave its sounds.
/// The page shows only sounds under the listed folders.
pub fn plan(found: &Found, known: &[KnownSound], folders: &[String]) -> ScanPlan {
    // Each path's key is worked out once: with hundreds of thousands of sounds, every extra pass is that many strings.
    let known_keys: Vec<String> = known.iter().map(|k| case::key(&k.path)).collect();
    let known_by_key: HashMap<&str, &KnownSound> = known_keys.iter().map(String::as_str).zip(known).collect();
    let mut present: HashSet<String> = HashSet::with_capacity(found.files.len());
    let mut listen: Vec<FoundFile> = Vec::new();
    // Files the library has never seen, as (path, size, mtime): some may be gone sounds renamed or moved.
    let mut added: Vec<(&str, i64, i64)> = Vec::new();
    for f in &found.files {
        let key = case::key(&f.path);
        // The folder listing already gave each file's size and time, so unchanged sounds cost no trips to the disk.
        match known_by_key.get(key.as_str()) {
            None => {
                added.push((f.path.as_str(), f.size, f.mtime));
                listen.push(f.clone());
            }
            Some(k) if k.mtime != f.mtime || k.size != f.size => listen.push(f.clone()),
            Some(_) => {}
        }
        present.insert(key);
    }

    let away: Vec<&String> =
        folders.iter().filter(|f| !Path::new(f.as_str()).is_dir()).chain(found.unreadable.iter()).collect();
    let kept = |p: &str| {
        !folders.iter().any(|f| is_inside(p, f))
            || away.iter().any(|a| is_inside(p, a))
            || (!can_read(p) && folders.iter().any(|f| is_inside(p, f)) && Path::new(p).is_file())
    };

    let gone: Vec<&KnownSound> = known
        .iter()
        .zip(&known_keys)
        .filter(|(k, key)| !present.contains(*key) && !kept(&k.path))
        .map(|(k, _)| k)
        .collect();
    let moves = moves(gone.iter().map(|k| (k.path.as_str(), k.size, k.mtime)), added);

    // A moved file whose new name says what its old one did isn't listened to again.
    let arrived: HashSet<String> = moves.iter().filter(|(f, t)| same_hints(f, t)).map(|(_, t)| case::key(t)).collect();
    listen.retain(|f| !arrived.contains(&case::key(&f.path)));
    let moved_from: HashSet<String> = moves.iter().map(|(f, _)| case::key(f)).collect();
    let forget = gone.iter().filter(|k| !moved_from.contains(&case::key(&k.path))).map(|k| k.path.clone()).collect();
    ScanPlan { listen, moves, forget }
}

/// Whether a renamed or moved file's new path says what its old one did about what listening takes from names: loop
/// or one-shot, tempo and key. When it doesn't ("Pad.wav" renamed "Pad 90bpm Cm.wav", or moved from "One Shots" into
/// "Loops"), the sound is listened to again, keeping everything you gave it.
pub fn same_hints(from: &str, to: &str) -> bool {
    loop_from_path(from) == loop_from_path(to)
        && tempo_from_name(from) == tempo_from_name(to)
        && key_from_name(from) == key_from_name(to)
}

/// Most same-name pairs weighed among files of one size and time; past it they're left unpaired.
const MOST_PAIRS: usize = 200_000;

/// Pairs files gone from the library with new ones that are the same files renamed or moved: a rename or move keeps a
/// file's size and modified time. Among files that share those (a pack's loops often do), each pairs with the one whose
/// path ends the same way (file name, then its folders), when that's the only best match; a single file left on each
/// side is a rename. Anything less sure isn't paired, and is forgotten and listened to as before.
///
/// Items are (path, size, mtime). The result keeps the C# app's order: groups in the order they first appear among
/// `gone`, best matches first.
pub fn moves<'a>(
    gone: impl IntoIterator<Item = (&'a str, i64, i64)>,
    added: impl IntoIterator<Item = (&'a str, i64, i64)>,
) -> Vec<(String, String)> {
    let mut arrived: HashMap<(i64, i64), Vec<&str>> = HashMap::new();
    for (p, size, mtime) in added {
        arrived.entry((size, mtime)).or_default().push(p);
    }
    let mut order: Vec<(i64, i64)> = Vec::new();
    let mut groups: HashMap<(i64, i64), Vec<&str>> = HashMap::new();
    for (p, size, mtime) in gone {
        let olds = groups.entry((size, mtime)).or_default();
        if olds.is_empty() {
            order.push((size, mtime));
        }
        olds.push(p);
    }

    let mut out = Vec::new();
    for key in order {
        let (Some(olds), Some(news)) = (groups.get(&key), arrived.get(&key)) else { continue };
        let mut by_name: HashMap<String, Vec<&str>> = HashMap::new();
        for n in news {
            by_name.entry(case::key(file_name(n))).or_default().push(n);
        }
        let same_name = |o: &str| by_name.get(&case::key(file_name(o))).map_or(&[][..], Vec::as_slice);
        if olds.iter().map(|o| same_name(o).len()).sum::<usize>() > MOST_PAIRS {
            continue; // thousands of copies: too many to tell apart
        }
        // Every same-name pair with how many names its paths end with alike, best first (a stable sort keeps the
        // order pairs were found in within each level, as C#'s GroupBy does).
        let mut alike: Vec<(&str, &str, usize)> =
            olds.iter().flat_map(|o| same_name(o).iter().map(move |n| (*o, *n, same_ending(o, n)))).collect();
        alike.sort_by_key(|a| std::cmp::Reverse(a.2));

        let mut used: HashSet<String> = HashSet::new();
        let mut unsure: HashSet<String> = HashSet::new();
        for level in alike.chunk_by(|a, b| a.2 == b.2) {
            let free = |p: &str| {
                let k = case::key(p);
                !used.contains(&k) && !unsure.contains(&k)
            };
            let open: Vec<&(&str, &str, usize)> = level.iter().filter(|a| free(a.0) && free(a.1)).collect();
            let mut froms: HashMap<String, usize> = HashMap::new();
            let mut tos: HashMap<String, usize> = HashMap::new();
            for a in &open {
                *froms.entry(case::key(a.0)).or_default() += 1;
                *tos.entry(case::key(a.1)).or_default() += 1;
            }
            for a in open {
                let (f, t) = (case::key(a.0), case::key(a.1));
                if froms.get(&f) == Some(&1) && tos.get(&t) == Some(&1) {
                    out.push((a.0.to_owned(), a.1.to_owned()));
                    used.insert(f);
                    used.insert(t);
                } else {
                    // Two equally good matches: neither is sure, now or with a worse match further down.
                    unsure.insert(f);
                    unsure.insert(t);
                }
            }
        }
        let left = |list: &[&'a str]| -> Vec<&'a str> {
            list.iter().copied().filter(|p| !used.contains(&case::key(p))).collect()
        };
        if let ([o], [n]) = (left(olds).as_slice(), left(news).as_slice())
            && !unsure.contains(&case::key(o))
            && !unsure.contains(&case::key(n))
        {
            out.push(((*o).to_owned(), (*n).to_owned()));
        }
    }
    out
}

/// How many names at the end of two paths are the same, ignoring case: 1 for the file name, 2 for it and its folder…
fn same_ending(a: &str, b: &str) -> usize {
    a.split(['\\', '/']).rev().zip(b.split(['\\', '/']).rev()).take_while(|(x, y)| case::eq_ignore_case(x, y)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_files() {
        assert!(can_read(r"D:\s\Kick.WAV"));
        assert!(can_read("D:/s/loop.flac"));
        // Windows decodes WMA; a Mac or Linux can't.
        assert_eq!(can_read(r"D:\s\Song.WMA"), cfg!(windows));
        assert!(!can_read(r"D:\s.wav\readme"));
        assert!(!is_sound_file(r"D:\s\._Kick.wav"));
        assert!(is_hidden_folder("__macosx"));
        assert!(is_hidden_folder(".git"));
        assert!(!is_hidden_folder("Kicks"));
    }

    #[test]
    fn hints() {
        assert!(same_hints(r"D:\a\Pad.wav", r"D:\b\Pad 2.wav"));
        assert!(!same_hints(r"D:\a\Pad.wav", r"D:\a\Pad 90bpm.wav"));
        assert!(!same_hints(r"D:\One Shots\Pad.wav", r"D:\Loops\Pad.wav"));
        assert!(!same_hints(r"D:\a\Pad.wav", r"D:\a\Pad Cm.wav"));
    }

    #[test]
    fn renames_and_moves_pair_up() {
        // A pack's folder renamed: each file pairs with the one of the same name.
        let gone = [(r"D:\Pack\Kick.wav", 10, 1), (r"D:\Pack\Snare.wav", 10, 1)];
        let added = [(r"D:\Pack 2\Snare.wav", 10, 1), (r"D:\Pack 2\Kick.wav", 10, 1)];
        let mut m = moves(gone, added);
        m.sort();
        assert_eq!(
            m,
            [
                (r"D:\Pack\Kick.wav".to_owned(), r"D:\Pack 2\Kick.wav".to_owned()),
                (r"D:\Pack\Snare.wav".to_owned(), r"D:\Pack 2\Snare.wav".to_owned())
            ]
        );
        // One file renamed: a single one left on each side.
        assert_eq!(moves([(r"D:\a\x.wav", 5, 2)], [(r"D:\a\y.wav", 5, 2)]).len(), 1);
        // Different size or time: not the same file.
        assert!(moves([(r"D:\a\x.wav", 5, 2)], [(r"D:\b\x.wav", 6, 2)]).is_empty());
        // Two equally good matches: neither pairs.
        assert!(moves([(r"D:\a\x.wav", 5, 2)], [(r"D:\b\x.wav", 5, 2), (r"D:\c\x.wav", 5, 2)]).is_empty());
        // The longer matching ending wins.
        let m = moves(
            [(r"D:\Old\Kicks\x.wav", 5, 2), (r"D:\Old\Snares\x.wav", 5, 2)],
            [(r"D:\New\Snares\x.wav", 5, 2), (r"D:\New\Kicks\x.wav", 5, 2)],
        );
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|(f, t)| file_name(&f.replace(r"\x.wav", "")) == file_name(&t.replace(r"\x.wav", ""))));
    }
}
