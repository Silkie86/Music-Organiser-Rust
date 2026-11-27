//! Names of the copies the app makes of a sound to drag into a DAW: the trimmed copies Trim silence makes
//! (`mo_app::trim_copy`), the parts picked on a sound's card, the sounds edited in the Edit strip and the slices it
//! cuts for a drum machine's pads (`mo_app::edit_copy`). Each is the original's name, what was done, and a mark made
//! from the original's path, so a project that uses a copy still counts as using the original ([`crate::usage`]):
//! "Kick 01 (trimmed -60 dB, 1a2b3c4d).wav", "Loop 128 (part 1.406-1.875 s, 1a2b3c4d).wav", "Loop 128 (edited
//! 5e6f7a8b, 1a2b3c4d).wav", "Loop 128 01 (slice 5e6f7a8b, 1a2b3c4d).wav".

use mo_formats::case;

/// The longest part of the original's name a copy's name keeps, so the copy's name stays well inside Windows' limits.
const NAME_KEPT: usize = 100;

/// The mark for a sound at `path`: eight hex digits, the same for the same file however its path is written (letter
/// case, slashes), on every run and every Rust version.
pub fn mark(path: &str) -> String {
    let mut h = crate::compact::Hash::new();
    h.bytes(case::key(&path.replace('/', "\\")).as_bytes());
    format!("{:08x}", h.0 as u32)
}

/// The copy's file name for the sound at `path` trimmed at `db` below full scale.
pub fn copy_name(path: &str, db: i32) -> String {
    format!("{} (trimmed {db} dB, {}).wav", stem_of(path), mark(path))
}

/// The file name of the part of the sound at `path` from `from` to `to` seconds in: to the millisecond, so the same
/// part dragged again is the same copy.
pub fn part_name(path: &str, from: f64, to: f64) -> String {
    let ms = |s: f64| if s.is_finite() { (s.max(0.0) * 1000.0).round() as u64 } else { 0 };
    let secs = |ms: u64| format!("{}.{:03}", ms / 1000, ms % 1000);
    format!("{} (part {}-{} s, {}).wav", stem_of(path), secs(ms(from)), secs(ms(to)), mark(path))
}

/// The file name of the sound at `path` as `edits` leave it: `edits` is any text that tells one set of edits from
/// another (the edits written out in full), which goes into the name as eight hex digits, so the same edits saved again
/// are the same copy and other edits are another.
pub fn edit_name(path: &str, edits: &str) -> String {
    format!("{} (edited {}, {}).wav", stem_of(path), key_of(edits), mark(path))
}

/// The folder the slices of the sound at `path` go in ("Loop 128 slices (5e6f7a8b)"): `slicing` is any text that tells
/// one slicing from another (what was read, its edits and its cuts, written out in full), which goes into the name as
/// eight hex digits. So the same slicing again is the same folder, and another slicing never changes the slices a
/// project already uses.
pub fn slices_folder(path: &str, slicing: &str) -> String {
    format!("{} slices ({})", stem_of(path), key_of(slicing))
}

/// The file name of slice `number` (from 1) of the sound at `path`, in [`slices_folder`]: numbered so the slices sort
/// in their order ("Loop 128 01 (slice 5e6f7a8b, 1a2b3c4d).wav").
pub fn slice_name(path: &str, slicing: &str, number: usize) -> String {
    format!("{} {number:02} (slice {}, {}).wav", stem_of(path), key_of(slicing), mark(path))
}

/// Whether `name` is a folder of slices ([`slices_folder`]): for the start-up sweep, which looks in them for slices
/// whose making was cut short.
pub fn is_slices_folder(name: &str) -> bool {
    name.strip_suffix(')')
        .and_then(|n| n.rsplit_once(" slices ("))
        .is_some_and(|(stem, key)| !stem.is_empty() && is_key(key))
}

/// Eight hex digits that tell one text from another.
fn key_of(text: &str) -> String {
    let mut h = crate::compact::Hash::new();
    h.bytes(text.as_bytes());
    format!("{:08x}", h.0 as u32)
}

fn is_key(key: &str) -> bool {
    key.len() == 8 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The start of a copy's name: the original's name without its extension, cut to [`NAME_KEPT`] characters.
fn stem_of(path: &str) -> String {
    let file = crate::file_name(path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let stem: String = stem.chars().take(NAME_KEPT).collect();
    if stem.trim().is_empty() { "Sound".to_owned() } else { stem }
}

/// The mark in a copy's file name ([`copy_name`], [`part_name`], [`edit_name`], [`slice_name`]), or `None` for any
/// other name.
pub fn mark_in(file_name: &str) -> Option<&str> {
    let rest = file_name.strip_suffix(").wav").or_else(|| file_name.strip_suffix(").WAV"))?;
    let (head, mark) = rest.rsplit_once(", ")?;
    let what = head.rsplit_once(" (")?.1;
    let number = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    let seconds = |s: &str| s.split_once('.').is_some_and(|(whole, ms)| number(whole) && ms.len() == 3 && number(ms));
    let made = if let Some(level) = what.strip_prefix("trimmed ").and_then(|w| w.strip_suffix(" dB")) {
        number(level.strip_prefix('-').unwrap_or(level))
    } else if let Some(span) = what.strip_prefix("part ").and_then(|w| w.strip_suffix(" s")) {
        span.split_once('-').is_some_and(|(from, to)| seconds(from) && seconds(to))
    } else if let Some(key) = what.strip_prefix("edited ").or_else(|| what.strip_prefix("slice ")) {
        is_key(key)
    } else {
        false
    };
    (made && is_key(mark)).then_some(mark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copys_name_carries_its_originals_mark() {
        let name = copy_name("D:\\Samples\\Kick 01.wav", -60);
        assert!(name.starts_with("Kick 01 (trimmed -60 dB, ") && name.ends_with(").wav"), "{name}");
        assert_eq!(mark_in(&name), Some(mark("D:\\Samples\\Kick 01.wav").as_str()));
        // However the path is written.
        assert_eq!(mark("d:/samples/kick 01.wav"), mark("D:\\Samples\\Kick 01.wav"));
        assert_ne!(mark("D:\\Samples\\Kick 02.wav"), mark("D:\\Samples\\Kick 01.wav"));
        assert_eq!(mark_in("Kick 01.wav"), None);
        assert_eq!(mark_in("Kick (trimmed loud dB, 1234abcd).wav"), None);
    }

    #[test]
    fn a_parts_name_carries_its_span_and_its_originals_mark() {
        let name = part_name("D:\\Samples\\Loop 128.wav", 1.40625, 1.875);
        assert!(name.starts_with("Loop 128 (part 1.406-1.875 s, ") && name.ends_with(").wav"), "{name}");
        assert_eq!(mark_in(&name), Some(mark("D:\\Samples\\Loop 128.wav").as_str()));
        assert_eq!(
            part_name("D:\\a.wav", 61.0006, 62.0),
            format!("a (part 61.001-62.000 s, {}).wav", mark("D:\\a.wav"))
        );
        // Not a part: the wrong shape of span, or a name of the user's own.
        assert_eq!(mark_in("Loop (part 1-2 s, 1234abcd).wav"), None);
        assert_eq!(mark_in("Loop (part one, 1234abcd).wav"), None);
        assert_eq!(mark_in("Loop (take 2, 1234abcd).wav"), None);
        assert_eq!(mark_in("Kick (trimmed -60 dB, 1234abcd).wav"), Some("1234abcd"));
    }

    #[test]
    fn an_edits_name_tells_its_edits_apart_and_carries_its_originals_mark() {
        let name = edit_name("D:\\Samples\\Loop 128.wav", "[Reverse(None)]");
        assert!(name.starts_with("Loop 128 (edited ") && name.ends_with(").wav"), "{name}");
        assert_eq!(mark_in(&name), Some(mark("D:\\Samples\\Loop 128.wav").as_str()));
        assert_eq!(name, edit_name("D:\\Samples\\Loop 128.wav", "[Reverse(None)]"), "the same edits, the same copy");
        assert_ne!(name, edit_name("D:\\Samples\\Loop 128.wav", "[Normalise { peak_db: 0.0 }]"));
        assert_eq!(mark_in("Loop (edited 12345, 1234abcd).wav"), None);
        assert_eq!(mark_in("Loop (edited nothing!, 1234abcd).wav"), None);
    }

    #[test]
    fn slices_are_numbered_in_a_folder_of_their_slicing() {
        let path = "D:\\Samples\\Loop 128.wav";
        let folder = slices_folder(path, "cuts a");
        assert!(folder.starts_with("Loop 128 slices (") && is_slices_folder(&folder), "{folder}");
        assert_ne!(folder, slices_folder(path, "cuts b"), "another slicing, another folder");
        let (first, tenth) = (slice_name(path, "cuts a", 1), slice_name(path, "cuts a", 10));
        assert!(
            first.starts_with("Loop 128 01 (slice ") && tenth.starts_with("Loop 128 10 (slice "),
            "{first} {tenth}"
        );
        assert!(first < tenth, "they sort in their order");
        assert_eq!(mark_in(&first), Some(mark(path).as_str()));
        assert_eq!(mark_in("Loop 01 (slice 1234, 1234abcd).wav"), None);
        // Not the app's folders: a name of the user's own.
        for name in ["Loop slices", "Loop slices (final)", " slices (1234abcd)", "Loop slices (1234abcd) 2"] {
            assert!(!is_slices_folder(name), "{name}");
        }
    }

    #[test]
    fn a_long_name_is_cut_short() {
        let long = format!("D:\\{}.flac", "x".repeat(300));
        let name = copy_name(&long, -40);
        assert!(name.len() < 140, "{}", name.len());
        assert!(mark_in(&name).is_some());
    }
}
