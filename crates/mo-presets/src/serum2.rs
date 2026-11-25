//! Putting a song's Serum 1 sounds where Serum 2 finds them: Serum 2 loads a Serum 1 `.fxp` as it is (Xfer: "Serum 1
//! presets are fully compatible with Serum 2"), so each sound is written as a plain Serum 1 program into Serum 2's
//! user presets folder, `Documents\Xfer\Serum 2 Presets\Presets\User\From my songs\<song>\<sound>.fxp`. No
//! conversion: Solomon found Serum 2 loads the old presets itself (25 Nov 2026), after the C# app's convert-to-Serum-2
//! attempt.
//!
//! Serum 1 keeps the same settings in all three places a song can hold them (checked on Solomon's saves and presets):
//! a zlib stream (then 12 bytes of its own), which is an `.fxp`'s chunk (after its 60-byte header), a Bitwig `.fxb`
//! bank's chunk (after its 160-byte header), and a VST3 `.vstpreset`'s component state ("Comp"). So each becomes an
//! "FPCh" program for the plugin "XfsX", named after the sound.
//!
//! Nothing is ever written over: a file of the same name that holds the same settings is left (sent already), and one
//! that holds others (the song changed since, or a file of Solomon's) keeps its name while this one takes the next
//! free one ("Lead 2.fxp"). Each file is written as an export is (copy, check, rename: [`export::write_checked`]).

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use mo_formats::paths;
use mo_formats::plugins;
use mo_formats::presets::{PresetBlob, fxp};

use crate::export::{self, Exported};
use crate::names;
use crate::role::Thresholds;

/// Serum 1's VST2 id, as the four letters a preset names its plugin by.
pub const SERUM_1_ID: &str = "XfsX";

/// The folder inside Serum 2's user presets that the app's copies go in, one folder for each song inside it.
pub const FROM_SONGS: &str = "From my songs";

/// Serum 2's user presets folder under a Documents folder (on Windows and a Mac alike):
/// `Xfer\Serum 2 Presets\Presets\User`.
pub fn user_folder(documents: &Path) -> PathBuf {
    documents.join("Xfer").join("Serum 2 Presets").join("Presets").join("User")
}

/// Whether a plugin's name is Serum 1's ("Serum", "Serum_x64", "Serum (x64)", "Xfer Serum"), not Serum 2's
/// ("Serum 2", "Serum2", "Serum 2 FX") nor its effect's ("SerumFX"). For what the page offers; what's written is
/// checked by the settings themselves ([`serum_1_chunk`]).
pub fn is_serum_1(plugin: &str) -> bool {
    let p = plugin.trim().to_lowercase();
    let p = p.strip_prefix("xfer records ").or_else(|| p.strip_prefix("xfer ")).unwrap_or(&p);
    p.starts_with("serum") && !p.starts_with("serum 2") && !p.starts_with("serum2") && !p.starts_with("serumfx")
}

fn be_u32(raw: &[u8], at: usize) -> Option<usize> {
    raw.get(at..at.checked_add(4)?)?.first_chunk::<4>().and_then(|b| usize::try_from(u32::from_be_bytes(*b)).ok())
}

fn le_i64(raw: &[u8], at: usize) -> Option<i64> {
    raw.get(at..at.checked_add(8)?)?.first_chunk::<8>().map(|b| i64::from_le_bytes(*b))
}

/// Serum 1's own settings in a sound's saved settings (`extension` says which file they are: ".fxp", ".fxb",
/// ".vstpreset"), or why they aren't Serum 1's. Only settings that name Serum 1 ("XfsX", or a VST3 class id made from
/// it) and unpack as Serum 1's do.
pub fn serum_1_chunk<'a>(data: &'a [u8], extension: &str) -> Result<&'a [u8], String> {
    let not_serum = || "it isn't Serum 1's settings".to_owned();
    let chunk = match extension.to_lowercase().as_str() {
        ".fxp" | ".fxb" => {
            if !data.starts_with(b"CcnK")
                || be_u32(data, 16).map(|id| plugins::vst2_id(id as i32)).as_deref() != Some(SERUM_1_ID)
            {
                return Err(not_serum());
            }
            // A program: chunk size at 56, chunk at 60. A bank: chunk size at 156, chunk at 160.
            let (size_at, kind) = match data.get(8..12) {
                Some(b"FPCh") => (56, "program"),
                Some(b"FBCh") => (156, "bank"),
                _ => return Err("its Serum 1 file doesn't hold Serum's own settings".into()),
            };
            let size = be_u32(data, size_at).ok_or_else(|| format!("its {kind} ends inside its header"))?;
            let from = size_at + 4;
            data.get(from..from.saturating_add(size)).ok_or_else(|| format!("its {kind} is cut short"))?
        }
        ".vstpreset" => {
            let class = data.get(8..40).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            if !data.starts_with(b"VST3") || plugins::vst2_id_in(&class).as_deref() != Some(SERUM_1_ID) {
                return Err(not_serum());
            }
            component_state(data).ok_or_else(|| "its VST3 settings are cut short".to_owned())?
        }
        _ => return Err(not_serum()),
    };
    if chunk.first() != Some(&0x78) || !crate::octave::serum_1_unpacks(chunk) {
        return Err("its Serum 1 settings don't unpack".into());
    }
    Ok(chunk)
}

/// A .vstpreset's component state: its "Comp" chunk from the chunk list, else (no list) all from 48 to where the list
/// would be.
fn component_state(data: &[u8]) -> Option<&[u8]> {
    const STATE_AT: usize = 48;
    let list = usize::try_from(le_i64(data, 40)?).ok()?;
    if data.get(list..list.checked_add(4)?) == Some(b"List".as_slice()) {
        let count = usize::try_from(u32::from_le_bytes(*data.get(list + 4..list + 8)?.first_chunk::<4>()?)).ok()?;
        for i in 0..count.min(64) {
            let entry = data.get(list + 8 + 20 * i..list + 28 + 20 * i)?;
            if entry.get(..4) == Some(b"Comp".as_slice()) {
                let at = usize::try_from(le_i64(entry, 4)?).ok()?;
                let size = usize::try_from(le_i64(entry, 12)?).ok()?;
                return data.get(at..at.checked_add(size)?);
            }
        }
        return None;
    }
    data.get(STATE_AT..list)
}

/// A Serum 1 program ("FPCh", plugin "XfsX") of these settings, named `name`.
pub fn as_fxp(chunk: &[u8], name: &str) -> Vec<u8> {
    let id = i32::from_be_bytes(*b"XfsX");
    fxp::wrap_chunk(chunk, name, id)
}

/// The folder a song's Serum 1 sounds go to: `<user>\From my songs\<save's name>`.
pub fn folder_for(user: &Path, save: &str) -> PathBuf {
    user.join(FROM_SONGS).join(names::safe_file_name(paths::stem(paths::file_name(save))))
}

/// Each Serum 1 sound's file name among a save's sounds, worked out among all of them in the save's order, so a sound
/// keeps its name whichever are sent: its name, then where it sits ("Lead (Insert 3)"), then "Lead 2", "Lead 3" for
/// later ones of the same name. Letter case aside, as Windows sees names. `None` for a sound that isn't Serum 1's.
pub fn file_names(blobs: &[PresetBlob]) -> Vec<Option<String>> {
    let mut used: HashSet<String> = HashSet::new();
    blobs
        .iter()
        .map(|p| {
            serum_1_chunk(&p.data, &p.extension).ok()?;
            let mut label = if p.name.trim().is_empty() { p.plugin.clone() } else { p.name.clone() };
            if !p.place.is_empty() {
                label += &format!(" ({})", p.place.replace(" · ", " "));
            }
            let label = names::safe_file_name(&label);
            let mut name = format!("{label}.fxp");
            let mut n = 2;
            while !used.insert(name.to_uppercase()) {
                name = format!("{label} {n}.fxp");
                n += 1;
            }
            Some(name)
        })
        .collect()
}

/// Where a program goes in `folder` under `name`: `Ok(None)` when a file there already holds exactly it (sent
/// before); else the name itself when it's free, or the next free "name 2.fxp", "name 3.fxp"... (never written over).
fn place_for(folder: &Path, name: &str, bytes: &[u8]) -> io::Result<Option<PathBuf>> {
    let stem = name.strip_suffix(".fxp").unwrap_or(name);
    let mut n = 1;
    loop {
        let candidate = if n == 1 { folder.join(name) } else { folder.join(format!("{stem} {n}.fxp")) };
        match std::fs::symlink_metadata(&candidate) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Some(candidate)),
            Err(e) => return Err(e),
            Ok(meta) if meta.is_file() && meta.len() == bytes.len() as u64 => {
                if std::fs::read(&candidate)? == bytes {
                    return Ok(None);
                }
            }
            Ok(_) => {}
        }
        n += 1;
        if n > 999 {
            return Err(io::Error::other(format!("{name}: there are too many files of that name already")));
        }
    }
}

/// Reads a save again and puts its Serum 1 sounds into Serum 2's user presets folder (`user`, [`user_folder`]), in the
/// song's own folder ([`folder_for`]); only those with these keys ([`crate::list::place_keys`]) when `only` is given.
/// What was there already from before counts as done. Picked sounds that aren't Serum 1's, or aren't in the save any
/// more, are told in [`Exported::failed`]; it's an error when nothing could go. Serum 2's `Presets` folder must be
/// there (Serum 2 is installed); `User` and the folders inside it are made.
pub fn from_save(save: &str, only: Option<&[String]>, user: &Path) -> io::Result<Exported> {
    let presets = user.parent().ok_or_else(|| io::Error::other("Serum 2's presets folder has no parent"))?;
    if !presets.is_dir() {
        return Err(io::Error::other(format!(
            "Serum 2's presets folder isn't there ({}): is Serum 2 installed?",
            presets.display()
        )));
    }
    let raw = std::fs::read(save)?;
    let (sounds, warning) = crate::library::sounds_in(save, raw, &Thresholds::default()).map_err(io::Error::other)?;
    let keys = crate::list::place_keys(&sounds);
    let blobs: Vec<PresetBlob> = sounds.into_iter().map(|s| s.blob).collect();
    let names = file_names(&blobs);
    let wanted: Option<HashSet<&str>> = only.map(|keys| keys.iter().map(String::as_str).collect());
    let mut failed = Vec::new();
    if let Some(wanted) = &wanted {
        let found: HashSet<&str> = keys.iter().map(String::as_str).collect();
        let gone = wanted.iter().filter(|k| !found.contains(*k)).count();
        if gone > 0 {
            failed.push(format!(
                "{gone} of the sounds picked {} in the save any more (was it saved again since?)",
                if gone == 1 { "isn't" } else { "aren't" }
            ));
        }
    }
    let folder = folder_for(user, save);
    let mut files = Vec::new();
    for ((blob, name), key) in blobs.iter().zip(&names).zip(&keys) {
        let picked = wanted.as_ref().is_none_or(|w| w.contains(key.as_str()));
        if !picked {
            continue;
        }
        let Some(name) = name else {
            // Only a sound picked by its key is told about: a whole save's other plugins are simply left.
            if wanted.is_some() {
                let why = serum_1_chunk(&blob.data, &blob.extension).err().unwrap_or_default();
                failed.push(format!("{}: {why}", blob.name));
            }
            continue;
        };
        let Ok(chunk) = serum_1_chunk(&blob.data, &blob.extension) else { continue };
        let bytes = as_fxp(chunk, paths::stem(name));
        let written =
            std::fs::create_dir_all(&folder).and_then(|()| place_for(&folder, name, &bytes)).and_then(|place| {
                match place {
                    None => Ok(name.clone()),
                    Some(path) => export::write_checked(&path, &bytes)
                        .map(|()| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                }
            });
        match written {
            Ok(file) => files.push(file),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if let Ok(now) = std::fs::metadata(&folder).and_then(|m| m.modified()) {
        export::clear_half_written(&folder, now);
    }
    if files.is_empty() && !failed.is_empty() {
        return Err(io::Error::other(failed.join("; ")));
    }
    Ok(Exported { folder, files, failed, warning: warning.map(str::to_owned) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    /// A made-up Serum 1 chunk: a packed state of Serum's size, then Serum's 12 bytes of its own.
    fn chunk() -> Vec<u8> {
        // Serum packs with the lowest setting, so its stream starts "78 01" as this one does.
        let mut z = ZlibEncoder::new(Vec::new(), Compression::none());
        z.write_all(&vec![7u8; 172_736]).unwrap();
        let mut c = z.finish().unwrap();
        assert_eq!(c[..2], [0x78, 0x01]);
        c.extend([0u8; 12]);
        c
    }

    fn bank(chunk: &[u8]) -> Vec<u8> {
        mo_formats::presets::fxp_to_bank(&as_fxp(chunk, "x"), 0)
    }

    fn vstpreset(class: &str, chunk: &[u8]) -> Vec<u8> {
        mo_formats::presets::vstpreset::build(class, chunk, b"")
    }

    const SERUM_1_VST3: &str = "56535458667358736572756D00000000";
    const SERUM_2_VST3: &str = "56534558667350736572756D20320000";

    #[test]
    fn serum_1s_names_and_not_serum_2s() {
        for yes in ["Serum", "Serum_x64", "Serum (x64)", "serum", "Xfer Serum", "Xfer Records Serum"] {
            assert!(is_serum_1(yes), "{yes}");
        }
        for no in ["Serum 2", "Serum2", "Serum 2 FX", "SerumFX", "Vital", ""] {
            assert!(!is_serum_1(no), "{no}");
        }
    }

    #[test]
    fn the_same_settings_come_out_of_a_program_a_bank_and_a_vst3_state() {
        let c = chunk();
        let program = as_fxp(&c, "Lead");
        assert_eq!(&program[..4], b"CcnK");
        assert_eq!(&program[8..12], b"FPCh");
        assert_eq!(&program[16..20], b"XfsX");
        assert_eq!(&program[28..32], b"Lead");
        assert_eq!(serum_1_chunk(&program, ".fxp").unwrap(), c.as_slice());
        assert_eq!(serum_1_chunk(&program, ".FXP").unwrap(), c.as_slice());
        assert_eq!(serum_1_chunk(&bank(&c), ".fxb").unwrap(), c.as_slice());
        assert_eq!(serum_1_chunk(&vstpreset(SERUM_1_VST3, &c), ".vstpreset").unwrap(), c.as_slice());
        // A program made from any of them reads back as the same Serum 1 program.
        assert_eq!(as_fxp(serum_1_chunk(&bank(&c), ".fxb").unwrap(), "Lead"), program);
    }

    #[test]
    fn other_plugins_and_damaged_settings_are_refused() {
        let c = chunk();
        let mut other = as_fxp(&c, "Lead");
        other[16..20].copy_from_slice(b"XfsP");
        assert!(serum_1_chunk(&other, ".fxp").is_err());
        assert!(serum_1_chunk(&vstpreset(SERUM_2_VST3, &c), ".vstpreset").is_err());
        assert!(serum_1_chunk(&c, ".flstate").is_err());
        // Cut short, or not Serum's packing.
        let program = as_fxp(&c, "Lead");
        assert!(serum_1_chunk(&program[..program.len() - 100], ".fxp").is_err());
        assert!(serum_1_chunk(&as_fxp(&[0x78, 0x01, 1, 2, 3], "x"), ".fxp").is_err());
        assert!(serum_1_chunk(&as_fxp(&[1, 2, 3], "x"), ".fxp").is_err());
        assert!(serum_1_chunk(b"", ".fxp").is_err());
        assert!(serum_1_chunk(b"VST3", ".vstpreset").is_err());
    }

    #[test]
    fn names_are_worked_out_among_the_saves_serum_1_sounds() {
        let c = chunk();
        let mut a = PresetBlob::new("Lead", "Serum", ".fxp", as_fxp(&c, "a"));
        a.place = "Insert 3 · Vocals".into();
        let b = PresetBlob::new("Bass", "Serum_x64", ".fxp", as_fxp(&c, "b"));
        let b2 = PresetBlob::new("bass", "Serum", ".vstpreset", vstpreset(SERUM_1_VST3, &c));
        let other = PresetBlob::new("Pad", "Vital", ".vstpreset", vstpreset(SERUM_2_VST3, &c));
        let names = file_names(&[a, b, other, b2]);
        assert_eq!(
            names,
            [Some("Lead (Insert 3 Vocals).fxp".into()), Some("Bass.fxp".into()), None, Some("bass 2.fxp".into())]
        );
    }

    #[test]
    fn nothing_is_written_over_and_the_same_file_counts_as_sent() {
        let dir = std::env::temp_dir().join(format!("mo-serum2-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mine = as_fxp(&chunk(), "Lead");
        assert_eq!(place_for(&dir, "Lead.fxp", &mine).unwrap(), Some(dir.join("Lead.fxp")));
        std::fs::write(dir.join("Lead.fxp"), &mine).unwrap();
        assert_eq!(place_for(&dir, "Lead.fxp", &mine).unwrap(), None, "the same settings: sent before");
        // Another file of the name (the song changed, or a file of his) keeps it; this takes the next.
        let mut changed = mine.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(place_for(&dir, "Lead.fxp", &changed).unwrap(), Some(dir.join("Lead 2.fxp")));
        std::fs::write(dir.join("Lead 2.fxp"), &changed).unwrap();
        assert_eq!(place_for(&dir, "Lead.fxp", &changed).unwrap(), None);
        // A folder of the name is never a place for a file.
        std::fs::create_dir_all(dir.join("Pad.fxp")).unwrap();
        assert_eq!(place_for(&dir, "Pad.fxp", &mine).unwrap(), Some(dir.join("Pad 2.fxp")));
        assert_eq!(std::fs::read(dir.join("Lead.fxp")).unwrap(), mine, "left as it was");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_serum_2_is_said_and_nothing_is_made() {
        let dir = std::env::temp_dir().join(format!("mo-serum2-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let user = user_folder(&dir);
        let e = from_save("song.flp", None, &user).unwrap_err();
        assert!(e.to_string().contains("is Serum 2 installed?"), "{e}");
        assert!(!dir.exists());
        assert_eq!(
            folder_for(&user, r"K:\Bitwig Studio\Projects\a\neon: rmx.bwproject"),
            user.join(FROM_SONGS).join(names::safe_file_name("neon: rmx"))
        );
    }
}
