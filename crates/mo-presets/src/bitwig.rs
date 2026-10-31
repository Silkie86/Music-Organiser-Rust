//! Send to Bitwig: Bitwig's own preset files (`.bwpreset`) made from plugin settings taken out of a song, as the C#
//! `BitwigPreset` does.
//!
//! A `.bwpreset` is a Bitwig document like a project: the device and the plugin it loads are in its body, and the
//! plugin's settings sit in the zip at its end as a plain `.fxp`/`.fxb` (VST2) or `.vstpreset` (VST3) file, named by
//! the device's state field. So a preset Bitwig itself saved for that plugin is used as a template: its body is kept,
//! the settings file in its zip is swapped for the new one, and the preset's name is changed. Nothing is made up that
//! Bitwig didn't write itself.
//!
//! Unlike the C# app, which swapped names as bytes, the template is decoded, changed field by field and written again
//! with `mo_formats::bitwig::encode` (the same writer the converter uses), then the new file is decoded again and
//! checked before it's used. A template is only read: nothing here changes it.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mo_formats::bitwig::{self, Document, Text, Value};
use mo_formats::paths;
use mo_formats::presets::PresetBlob;
use rayon::prelude::*;

use crate::files::FileFormat;
use crate::library::FoundPreset;

// Bitwig's fields, as `files` reads them (the C# BitwigPreset's FDeviceName, FDevicePreset, FPluginState).
const F_DEVICE_NAME: u32 = 0x9a;
const F_DEVICE_PRESET: u32 = 0x12de;
const F_PLUGIN_STATE: u32 = 0xbe5;

/// Where Bitwig keeps plugin states in a file's zip.
const STATES: &str = "plugin-states/";

/// A preset Bitwig saved for one plugin, usable as a template for that plugin's sounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub path: PathBuf,
    /// The device's name: the plugin as Bitwig names it.
    pub plugin: String,
    /// The settings file's name in the zip (after `plugin-states/`).
    pub state_entry: String,
    /// The preset's name as the device keeps it, which is changed to the new sound's name.
    pub preset_name: String,
    /// When the file was last saved, so the newest template for a plugin wins.
    pub modified: Option<SystemTime>,
}

impl Template {
    /// "VST2" or "VST3", from its settings file.
    pub fn format(&self) -> &'static str {
        state_format(paths::extension(&self.state_entry))
    }
}

/// "VST2" for `.fxp`/`.fxb`, "VST3" for `.vstpreset`, otherwise "".
pub fn state_format(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        ".fxp" | ".fxb" => "VST2",
        ".vstpreset" => "VST3",
        _ => "",
    }
}

/// The lookup key for a plugin and format, so "Serum_x64" finds a template saved for "Serum" (the library's name key).
pub fn key(plugin: &str, format: &str) -> String {
    format!("{}|{format}", mo_library::read::name_key(plugin))
}

/// Templates by [`key`].
pub type Templates = HashMap<String, Template>;

/// A `.bwpreset` that can serve as a template: it decodes cleanly, has exactly one device that loads a plugin (a chain
/// of several isn't a template for one), and that plugin's settings file is in its zip as a VST2 or VST3 preset.
pub fn inspect(path: &Path) -> Option<Template> {
    let raw = std::fs::read(path).ok()?;
    let mut template = inspect_bytes(&raw)?;
    template.path = path.to_path_buf();
    template.modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Some(template)
}

/// [`inspect`] on a file's bytes; the path and time are left empty.
pub fn inspect_bytes(raw: &[u8]) -> Option<Template> {
    let doc = bitwig::decode(raw).ok()?;
    if doc.body_error.is_some() {
        return None;
    }
    let mut devices =
        doc.objects.iter().filter(|o| !o.str(F_PLUGIN_STATE).is_empty() && !o.str(F_DEVICE_NAME).is_empty());
    let device = devices.next()?;
    if devices.next().is_some() {
        return None;
    }
    let entry = device.str(F_PLUGIN_STATE);
    if state_format(paths::extension(entry)).is_empty() || zip_entry(&doc, entry).is_none() {
        return None;
    }
    Some(Template {
        path: PathBuf::new(),
        plugin: device.str(F_DEVICE_NAME).to_owned(),
        state_entry: entry.to_owned(),
        preset_name: device.str(F_DEVICE_PRESET).to_owned(),
        modified: None,
    })
}

/// The templates among preset files the page already found ([`crate::library::find_preset_files`]), so Bitwig's
/// folder isn't walked twice: each `.bwpreset` that loads a plugin is read in full over every core, and the newest
/// one per plugin and format wins (by path when two were saved at the same time, so it's the same every run).
pub fn templates(found: &[FoundPreset]) -> Templates {
    let mut all: Vec<Template> = found
        .par_iter()
        .filter(|f| f.file.as_ref().is_ok_and(|p| p.format == Some(FileFormat::BwPreset) && p.wraps_plugin))
        // A damaged file is only not a template.
        .filter_map(|f| std::panic::catch_unwind(|| inspect(Path::new(&f.path))).ok().flatten())
        .collect();
    all.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    let mut out = Templates::new();
    for t in all {
        out.entry(key(&t.plugin, t.format())).or_insert(t);
    }
    out
}

/// The template for an exported sound, if one was found for its plugin and format.
pub fn template_for<'a>(preset: &PresetBlob, templates: &'a Templates) -> Option<&'a Template> {
    let format = state_format(&preset.extension);
    if format.is_empty() {
        return None;
    }
    templates.get(&key(&preset.plugin, format))
}

/// A VST2 program (`.fxp`) as a one-program bank (`.fxb`), the form Bitwig saves VST2 settings in and loads back
/// (it loaded a plugin's defaults from a `.fxp`). `None` when it isn't a program this can turn into a bank.
///
/// A chunk bank names the program the plugin had selected (`program`): Bitwig selects it again after loading the
/// chunk, as FL does, and KORG's M1 plays that program, so 0 brought it up on its first program, not the song's. A
/// parameter bank holds only this one program, so it's program 0 there.
pub fn to_bank(fxp: &[u8], program: u32) -> Option<Vec<u8>> {
    const PROGRAM_HEADER: usize = 60; // CcnK, size, magic, version, id, plugin version, count, name[28], chunk size
    if fxp.len() < PROGRAM_HEADER - 4 || !fxp.starts_with(b"CcnK") {
        return None;
    }
    let is_chunk = match fxp.get(8..12)? {
        b"FPCh" => true,
        b"FxCk" => false,
        _ => return None,
    };
    let body = if is_chunk {
        let size = u32::from_be_bytes(*fxp.get(PROGRAM_HEADER - 4..)?.first_chunk()?) as usize;
        let body = fxp.get(PROGRAM_HEADER..)?;
        if size != body.len() {
            return None;
        }
        body
    } else {
        // A parameter bank holds whole programs, header and all.
        fxp
    };
    let program = if is_chunk { i32::try_from(program).ok()? } else { 0 };
    let mut out = Vec::with_capacity(160 + body.len());
    out.extend(b"CcnK");
    out.extend(i32::try_from(4 + 20 + 124 + if is_chunk { 4 } else { 0 } + body.len()).ok()?.to_be_bytes());
    out.extend(if is_chunk { b"FBCh" } else { b"FxBk" });
    out.extend(2i32.to_be_bytes()); // format version
    out.extend(fxp.get(16..24)?); // plugin id and version, as the program had them
    out.extend((program + 1).to_be_bytes()); // number of programs
    out.extend(program.to_be_bytes()); // current program
    out.extend([0u8; 124]);
    if is_chunk {
        out.extend(i32::try_from(body.len()).ok()?.to_be_bytes());
    }
    out.extend(body);
    Some(out)
}

/// Builds a `.bwpreset` for `preset` from `template`, named `preset_name`, and checks it reads back with the new
/// settings, as Bitwig would read it. A VST2 program goes in as a one-program bank ([`to_bank`]).
pub fn build(template: &Template, preset: &PresetBlob, preset_name: &str) -> Result<Vec<u8>, String> {
    if state_format(&preset.extension) != template.format() {
        return Err(format!("the template is for {}, not {}", template.format(), preset.extension));
    }
    let (state, extension) = match preset.extension.eq_ignore_ascii_case(".fxp") {
        true => match to_bank(&preset.data, preset.program) {
            Some(bank) => (bank, ".fxb"),
            None => (preset.data.clone(), ".fxp"),
        },
        false => (preset.data.clone(), preset.extension.as_ref()),
    };
    let raw = std::fs::read(&template.path).map_err(|e| format!("the template couldn't be read: {e}"))?;
    let stem = paths::stem(paths::file_name(&template.path.to_string_lossy())).to_owned();
    build_from(
        &raw,
        &template.state_entry,
        &state,
        extension,
        preset_name,
        &[stem.as_str(), template.preset_name.as_str()],
        &template.plugin,
    )
}

/// Builds a `.bwpreset` from a template's bytes with `state` as the plugin's settings, then checks it.
///
/// - `state_extension` is the settings file's type (`.fxp`, `.fxb` or `.vstpreset`); the file in the zip and the
///   device's state field are renamed when it differs from the template's.
/// - `old_names` are the template's own names for its preset (its file name, the device's preset name): wherever
///   one is the whole of exactly one text in the metadata or in the body, it becomes `preset_name`. A name found more
///   than once is left alone rather than risk changing the wrong field, and `plugin` is never renamed, even when the
///   preset shares its name.
pub fn build_from(
    template: &[u8],
    state_entry: &str,
    state: &[u8],
    state_extension: &str,
    preset_name: &str,
    old_names: &[&str],
    plugin: &str,
) -> Result<Vec<u8>, String> {
    let mut doc = bitwig::decode(template).map_err(|_| "the template isn't a Bitwig file".to_owned())?;
    if let Some(e) = &doc.body_error {
        return Err(format!("the template doesn't read cleanly: {e}"));
    }
    let new_entry = format!("{}{state_extension}", paths::stem(state_entry));

    // The device's state field names the settings file: exactly one, as the template was checked.
    let holders: Vec<usize> = (0..doc.objects.len())
        .filter(|&i| doc.objects.get(i).is_some_and(|o| o.str(F_PLUGIN_STATE) == state_entry))
        .collect();
    let &[device] = holders.as_slice() else {
        return Err(format!("the template names its settings file \"{state_entry}\" {} times", holders.len()));
    };
    if let Some(o) = doc.objects.get_mut(device) {
        o.set(F_PLUGIN_STATE, Value::Str(Text::new(&new_entry)));
    }

    // The preset's name, wherever the template keeps its own.
    if !preset_name.is_empty() {
        let mut done: Vec<&str> = Vec::new();
        for &old in old_names {
            if old.is_empty() || old.eq_ignore_ascii_case(plugin) || done.contains(&old) {
                continue;
            }
            done.push(old);
            rename_once(&mut doc, old, preset_name);
        }
    }

    let zip = swap_zip_entry(doc.zip().unwrap_or_default(), state_entry, &new_entry, state)?;
    let bytes = bitwig::encode(&doc, Some(&zip))?;
    check(&bytes, &new_entry, state, plugin)?;
    Ok(bytes)
}

/// Changes a text that's exactly `old` to `new`, in the metadata and in the body (each looked at on its own), when
/// it's there once. The device's own name is never changed.
fn rename_once(doc: &mut Document, old: &str, new: &str) {
    let in_meta: Vec<usize> = (0..doc.meta.len())
        .filter(|&i| doc.meta.get(i).is_some_and(|(_, v)| matches!(v, Value::Str(t) if t.as_str() == old)))
        .collect();
    if let &[i] = in_meta.as_slice()
        && let Some((_, v)) = doc.meta.get_mut(i)
    {
        *v = Value::Str(Text::new(new));
    }
    let in_body: Vec<(usize, usize)> = doc
        .objects
        .iter()
        .enumerate()
        .flat_map(|(o, obj)| {
            obj.fields.iter().enumerate().filter_map(move |(f, field)| match &field.value {
                Value::Str(t) if t.as_str() == old => Some((o, f)),
                _ => None,
            })
        })
        .collect();
    if let &[(o, f)] = in_body.as_slice()
        && let Some(field) = doc.objects.get_mut(o).and_then(|obj| obj.fields.get_mut(f))
        && field.id != F_DEVICE_NAME
    {
        field.value = Value::Str(Text::new(new));
    }
}

/// The template's zip with its settings file swapped for `state` (renamed to `new_entry`); every other file is copied
/// as it was, in its place.
fn swap_zip_entry(zip: &[u8], old_entry: &str, new_entry: &str, state: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("the template's zip: {e}"))?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let old_name = format!("{STATES}{old_entry}");
    let mut swapped = false;
    for i in 0..input.len() {
        let entry = input.by_index_raw(i).map_err(|e| format!("the template's zip: {e}"))?;
        if entry.name().is_ok_and(|n| n == old_name) {
            let mut options =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            if let Some(time) = entry.last_modified() {
                options = options.last_modified_time(time);
            }
            drop(entry);
            out.start_file(format!("{STATES}{new_entry}"), options).map_err(|e| e.to_string())?;
            out.write_all(state).map_err(|e| e.to_string())?;
            swapped = true;
        } else {
            out.raw_copy_file(entry).map_err(|e| format!("the template's zip: {e}"))?;
        }
    }
    if !swapped {
        return Err(format!("the template's zip has no \"{old_name}\""));
    }
    Ok(out.finish().map_err(|e| e.to_string())?.into_inner())
}

/// The new preset reads back as Bitwig would read it: cleanly to its zip, one device for `plugin`, and its settings
/// file is `state`, byte for byte.
fn check(bytes: &[u8], entry: &str, state: &[u8], plugin: &str) -> Result<(), String> {
    let doc = bitwig::decode(bytes).map_err(|_| "the new preset isn't a Bitwig file".to_owned())?;
    if let Some(e) = &doc.body_error {
        return Err(format!("the new preset doesn't read back: {e}"));
    }
    let template = inspect_bytes(bytes).ok_or("the new preset doesn't load one plugin")?;
    if template.state_entry != entry || template.plugin != plugin {
        return Err("the new preset's device changed".into());
    }
    match zip_entry(&doc, entry) {
        Some(read) if read == state => Ok(()),
        _ => Err("the new preset lost its plugin settings".into()),
    }
}

/// A settings file in a document's zip, in full.
fn zip_entry(doc: &Document, entry: &str) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(doc.zip()?)).ok()?;
    let mut file = archive.by_name(&format!("{STATES}{entry}")).ok()?;
    let mut out = Vec::new();
    file.read_to_end(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::presets::fxp;

    #[test]
    fn a_chunk_program_becomes_a_bank_keeping_its_program() {
        let p = fxp::wrap_chunk(&[1, 2, 3], "Big Bass", 0x5866_7358);
        let bank = to_bank(&p, 5).unwrap();
        assert_eq!(&bank[..4], b"CcnK");
        assert_eq!(&bank[8..12], b"FBCh");
        assert_eq!(i32::from_be_bytes(bank[4..8].try_into().unwrap()) as usize, bank.len() - 8);
        assert_eq!(&bank[16..24], &p[16..24]);
        assert_eq!(i32::from_be_bytes(bank[24..28].try_into().unwrap()), 6);
        assert_eq!(i32::from_be_bytes(bank[28..32].try_into().unwrap()), 5);
        assert_eq!(&bank[156..160], &3i32.to_be_bytes());
        assert_eq!(&bank[160..], &[1, 2, 3]);
    }

    #[test]
    fn a_parameter_program_becomes_a_bank_of_one() {
        let p = fxp::wrap_params(&[0.5, 0.25], "Pad", 7);
        let bank = to_bank(&p, 9).unwrap();
        assert_eq!(&bank[8..12], b"FxBk");
        assert_eq!(i32::from_be_bytes(bank[24..28].try_into().unwrap()), 1);
        assert_eq!(i32::from_be_bytes(bank[28..32].try_into().unwrap()), 0);
        assert_eq!(&bank[156..], &p[..]);
    }

    #[test]
    fn what_isnt_a_whole_program_stays_as_it_is() {
        assert_eq!(to_bank(b"not a preset at all, not at all, not at all, not at all..", 0), None);
        let mut p = fxp::wrap_chunk(&[1, 2, 3], "x", 1);
        p.pop(); // the chunk's size no longer matches
        assert_eq!(to_bank(&p, 0), None);
    }

    #[test]
    fn formats_by_ending() {
        assert_eq!(state_format(".FXB"), "VST2");
        assert_eq!(state_format(".vstpreset"), "VST3");
        assert_eq!(state_format(".flstate"), "");
        assert_eq!(key("Serum_x64", "VST2"), key("Serum", "VST2"));
    }
}
