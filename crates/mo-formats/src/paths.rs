//! File paths as the C# app's .NET `Path` handles them, so the two apps list the same files: projects come from
//! Windows, and a path written on one OS can be read on another.

use std::path::{Component, Path, PathBuf};

/// The separators a path can use on this OS.
fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Rooted on this OS (.NET's `Path.IsPathRooted`): `/x` everywhere, and `\x` or `C:x` on Windows.
pub fn is_rooted(p: &str) -> bool {
    p.starts_with('/')
        || (cfg!(windows) && (p.starts_with('\\') || matches!(p.as_bytes(), [d, b':', ..] if d.is_ascii_alphabetic())))
}

/// Rooted on this OS, or a Windows drive or network path, wherever this runs (the files come from Windows).
pub fn is_absolute(p: &str) -> bool {
    is_rooted(p)
        || p.starts_with(r"\\")
        || matches!(p.as_bytes(), [d, b':', b'\\' | b'/', ..] if d.is_ascii_alphabetic())
}

/// The last part of a path written with either separator.
pub fn file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

/// Where the name's ending starts (.NET's `Path.GetExtension`): at its last dot, unless that's the last letter.
fn dot(name: &str) -> Option<usize> {
    let start = name.rfind(is_separator).map_or(0, |i| i + 1);
    name[start..].rfind('.').map(|i| start + i)
}

/// The file's ending with its dot (".wav"), or "" when it has none or ends in a dot.
pub fn extension(name: &str) -> &str {
    match dot(name) {
        Some(i) if i + 1 < name.len() => &name[i..],
        _ => "",
    }
}

/// The file's name without its ending (.NET's `Path.GetFileNameWithoutExtension`).
pub fn stem(path: &str) -> &str {
    let start = path.rfind(is_separator).map_or(0, |i| i + 1);
    match dot(path) {
        Some(i) => &path[start..i],
        None => &path[start..],
    }
}

/// `rel` from `dir`, in full, with `.` and `..` worked out (.NET's `Path.GetFullPath(Path.Combine(dir, rel))`). Only
/// the text is changed; nothing on disk is looked at.
pub fn full_path(dir: &Path, rel: &str) -> String {
    let joined = if is_rooted(rel) { PathBuf::from(rel) } else { dir.join(rel) };
    // On Windows this is GetFullPathNameW itself, as .NET calls it.
    let absolute = std::path::absolute(&joined).unwrap_or(joined);
    if cfg!(windows) {
        return absolute.to_string_lossy().into_owned();
    }
    let mut out = PathBuf::new();
    for c in absolute.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_endings_as_dotnet_reads_them() {
        assert_eq!((extension("Kick.wav"), stem("Kick.wav")), (".wav", "Kick"));
        assert_eq!((extension("a.b.fxb"), stem("a.b.fxb")), (".fxb", "a.b"));
        assert_eq!((extension("Kick."), stem("Kick.")), ("", "Kick"));
        assert_eq!((extension(".hidden"), stem(".hidden")), (".hidden", ""));
        assert_eq!((extension("noend"), stem("noend")), ("", "noend"));
        assert_eq!(file_name(r"D:\Packs/Drums\Kick.wav"), "Kick.wav");
        assert!(is_absolute(r"\\NAS\Samples\a.wav") && is_absolute("D:/a.wav") && !is_absolute("samples/a.wav"));
    }

    #[cfg(unix)]
    #[test]
    fn works_out_dots_in_a_full_path() {
        assert_eq!(
            full_path(Path::new("/music/Song"), "samples/../recordings/./a.wav"),
            "/music/Song/recordings/a.wav"
        );
        assert_eq!(full_path(Path::new("/music/Song"), "/other/a.wav"), "/other/a.wav");
    }
}
