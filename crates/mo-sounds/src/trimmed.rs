//! Names of the trimmed copies Trim silence drags into a DAW (`mo_app::trim_copy` makes them): the original's name,
//! the level, and a mark made from the original's path, so a project that uses a copy still counts as using the
//! original ([`crate::usage`]): "Kick 01 (trimmed -60 dB, 1a2b3c4d).wav".

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
    let file = crate::file_name(path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let stem: String = stem.chars().take(NAME_KEPT).collect();
    let stem = if stem.trim().is_empty() { "Sound".to_owned() } else { stem };
    format!("{stem} (trimmed {db} dB, {}).wav", mark(path))
}

/// The mark in a copy's file name ([`copy_name`]), or `None` for any other name.
pub fn mark_in(file_name: &str) -> Option<&str> {
    let rest = file_name.strip_suffix(").wav").or_else(|| file_name.strip_suffix(").WAV"))?;
    let (head, mark) = rest.rsplit_once(" dB, ")?;
    let level = head.rsplit_once(" (trimmed ")?.1;
    let ok = mark.len() == 8
        && mark.bytes().all(|b| b.is_ascii_hexdigit())
        && level.strip_prefix('-').unwrap_or(level).bytes().all(|b| b.is_ascii_digit())
        && !level.is_empty();
    ok.then_some(mark)
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
    fn a_long_name_is_cut_short() {
        let long = format!("D:\\{}.flac", "x".repeat(300));
        let name = copy_name(&long, -40);
        assert!(name.len() < 140, "{}", name.len());
        assert!(mark_in(&name).is_some());
    }
}
