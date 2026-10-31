//! Preset files on disk: what plugin each one is for, its name, and what the file says about it. Only the first part of
//! a file is looked at for a plugin preset (its header); a Bitwig preset is decoded, as it keeps its facts in its own
//! format. Nothing here changes a file.
//!
//! - VST2 programs and banks (`.fxp`, `.fxb`): "CcnK", then big-endian numbers: the kind of file ("FxCk" a program of
//!   parameters, "FPCh" a program of the plugin's own chunk, "FxBk" and "FBCh" the same for banks), the plugin's four
//!   letters, and a program's name (28 bytes) or a bank's program count.
//! - VST3 presets (`.vstpreset`): "VST3", version, the plugin's class id (32 hex digits), where the chunk list is; the
//!   list names the chunks, and its "Info" chunk is XML with the plugin's name and category.
//! - Bitwig presets (`.bwpreset`): Bitwig's own format (see `mo_formats::bitwig::format`); the device and its
//!   category in the metadata, and a plugin's settings as an `.fxp`/`.fxb`/`.vstpreset` in the zip at its end.

use std::io::Read;
use std::path::Path;

use mo_formats::PluginKind;
use mo_formats::bitwig::{self, Value};
use mo_formats::paths;
use mo_formats::plugins;

/// The kinds of preset file the page lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileFormat {
    /// A VST2 program.
    Fxp,
    /// A VST2 bank of programs.
    Fxb,
    /// A VST3 preset.
    VstPreset,
    /// Bitwig's own preset: one of its devices, or a plugin inside one.
    BwPreset,
}

/// The extensions the page looks for, letter case aside.
pub const EXTENSIONS: [&str; 4] = ["fxp", "fxb", "vstpreset", "bwpreset"];

impl FileFormat {
    /// The format a file name says, letter case aside.
    pub fn of(file_name: &str) -> Option<FileFormat> {
        let ext = paths::extension(file_name).trim_start_matches('.');
        [FileFormat::Fxp, FileFormat::Fxb, FileFormat::VstPreset, FileFormat::BwPreset]
            .into_iter()
            .zip(EXTENSIONS)
            .find(|(_, e)| ext.eq_ignore_ascii_case(e))
            .map(|(f, _)| f)
    }

    /// The plugin format a preset of this kind loads into: "VST2", "VST3", or "Bitwig".
    pub fn plugin_format(self) -> &'static str {
        match self {
            FileFormat::Fxp | FileFormat::Fxb => "VST2",
            FileFormat::VstPreset => "VST3",
            FileFormat::BwPreset => "Bitwig",
        }
    }
}

/// What a preset file says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetFile {
    pub format: Option<FileFormat>,
    /// The preset's own name where the file keeps one (a VST2 program's, a VST3's "Name", a Bitwig preset's device
    /// preset), else the file's name without its extension.
    pub name: String,
    /// The plugin's name where the file says it (a VST3's "PlugInName", a Bitwig device's name); empty for a VST2
    /// file, which only has the plugin's four letters ([`PresetFile::plugin_id`]).
    pub plugin: String,
    /// The plugin's id as the plugin scanner writes it: a VST2's four letters, or a VST3's class id (32 hex digits).
    pub plugin_id: String,
    /// Instrument or effect, where the file says.
    pub kind: PluginKind,
    /// The file's own category ("Bass", "Pad", "Fx|Delay"...), as written.
    pub category: String,
    /// Who made it, where the file says.
    pub creator: String,
    pub tags: Vec<String>,
    /// How many programs a VST2 bank holds; 1 for anything else.
    pub programs: u32,
    /// A Bitwig preset that loads a plugin (its settings are in the file's zip) rather than one of Bitwig's own
    /// devices.
    pub wraps_plugin: bool,
}

/// Why a file couldn't be read as a preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotAPreset(pub String);

impl std::fmt::Display for NotAPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The most of a plugin preset that's read: its header and, for a VST3, the chunk list and "Info" XML. A preset's
/// own data (often megabytes for a sampler) is never needed for the list.
const HEAD: u64 = 64 * 1024;

/// Reads a preset file. Plugin presets are read only as far as their header; a VST3's chunk list sits at its end, so
/// that is read on its own. An error is a file that couldn't be opened or read; a file that isn't a preset it knows
/// is `Ok(Err(..))`, with what's wrong.
pub fn read(path: &Path) -> std::io::Result<Result<PresetFile, NotAPreset>> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match FileFormat::of(&name) {
        Some(FileFormat::BwPreset) => Ok(read_bytes(&std::fs::read(path)?, &name)),
        Some(FileFormat::VstPreset) => {
            let mut file = std::fs::File::open(path)?;
            let mut head = Vec::new();
            (&mut file).take(HEAD).read_to_end(&mut head)?;
            // The chunk list (and the "Info" chunk before it) is at the end: read that part too when it's past what's
            // been read.
            let list = vstpreset_list_offset(&head);
            let len = file.metadata()?.len();
            // The list's own entries (at most 64 of 20 bytes) can run past the first part even when it starts inside.
            if let Some(list) = list.filter(|&l| l.saturating_add(8 + 64 * 20) > HEAD && l < len && len > HEAD) {
                use std::io::{Seek, SeekFrom};
                let info_room = 64 * 1024;
                let from = list.saturating_sub(info_room).max(HEAD);
                file.seek(SeekFrom::Start(from))?;
                let mut tail = Vec::new();
                file.take(len - from).read_to_end(&mut tail)?;
                if from == head.len() as u64 {
                    // Straight on from the first part: one piece, so a chunk across the join is read whole.
                    head.extend(tail);
                    return Ok(read_bytes(&head, &name));
                }
                return Ok(read_vstpreset_parts(&head, &tail, from, &name));
            }
            Ok(read_bytes(&head, &name))
        }
        _ => {
            let mut head = Vec::new();
            std::fs::File::open(path)?.take(HEAD).read_to_end(&mut head)?;
            Ok(read_bytes(&head, &name))
        }
    }
}

/// Reads a preset from its bytes (all of it, or at least its header), by the format its file name says.
pub fn read_bytes(raw: &[u8], file_name: &str) -> Result<PresetFile, NotAPreset> {
    let stem = paths::stem(file_name);
    match FileFormat::of(file_name) {
        Some(FileFormat::Fxp | FileFormat::Fxb) => read_fxp(raw, stem),
        Some(FileFormat::VstPreset) => read_vstpreset_parts(raw, &[], 0, file_name),
        Some(FileFormat::BwPreset) => read_bwpreset(raw, stem),
        None => Err(NotAPreset(format!("{file_name} isn't a preset file the app knows"))),
    }
}

fn be_i32(raw: &[u8], at: usize) -> Option<i32> {
    raw.get(at..at.checked_add(4)?)?.first_chunk::<4>().map(|b| i32::from_be_bytes(*b))
}

fn le_i32(raw: &[u8], at: usize) -> Option<i32> {
    raw.get(at..at.checked_add(4)?)?.first_chunk::<4>().map(|b| i32::from_le_bytes(*b))
}

fn le_i64(raw: &[u8], at: usize) -> Option<i64> {
    raw.get(at..at.checked_add(8)?)?.first_chunk::<8>().map(|b| i64::from_le_bytes(*b))
}

/// Text in a fixed-size field: up to its first zero, Latin-1 as VST2 hosts write it, trimmed.
fn field_text(raw: &[u8]) -> String {
    raw.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>().trim().to_owned()
}

/// A VST2 program or bank.
fn read_fxp(raw: &[u8], stem: &str) -> Result<PresetFile, NotAPreset> {
    if !raw.starts_with(b"CcnK") {
        return Err(NotAPreset("it doesn't start as a VST2 preset does (\"CcnK\")".into()));
    }
    let magic = raw.get(8..12).ok_or_else(|| NotAPreset("it ends inside its header".into()))?;
    let bank = match magic {
        b"FxCk" | b"FPCh" => false,
        b"FxBk" | b"FBCh" => true,
        other => return Err(NotAPreset(format!("it's a VST2 file of an unknown kind ({})", field_text(other)))),
    };
    let id = be_i32(raw, 16).ok_or_else(|| NotAPreset("it ends inside its header".into()))?;
    let count = be_i32(raw, 24).unwrap_or(0);
    let mut file = PresetFile {
        format: Some(if bank { FileFormat::Fxb } else { FileFormat::Fxp }),
        plugin_id: plugins::vst2_id(id),
        programs: 1,
        ..PresetFile::default()
    };
    if bank {
        file.programs = count.max(0) as u32;
        file.name = stem.to_owned();
    } else {
        let name = raw.get(28..56).map(field_text).unwrap_or_default();
        file.name = if name.is_empty() { stem.to_owned() } else { name };
    }
    Ok(file)
}

/// Where a .vstpreset's chunk list is, when its header says.
fn vstpreset_list_offset(raw: &[u8]) -> Option<u64> {
    if !raw.starts_with(b"VST3") {
        return None;
    }
    u64::try_from(le_i64(raw, 40)?).ok()
}

/// A .vstpreset from its first part (`head`, from the file's start) and, when the chunk list is past that, a later
/// part (`tail`, from `tail_from`).
fn read_vstpreset_parts(head: &[u8], tail: &[u8], tail_from: u64, file_name: &str) -> Result<PresetFile, NotAPreset> {
    if !head.starts_with(b"VST3") {
        return Err(NotAPreset("it doesn't start as a VST3 preset does (\"VST3\")".into()));
    }
    let class_id = head.get(8..40).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    if class_id.len() != 32 || !class_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(NotAPreset("its plugin id isn't 32 hex digits".into()));
    }
    let mut file = PresetFile {
        format: Some(FileFormat::VstPreset),
        plugin_id: plugins::class_id(&class_id),
        name: paths::stem(file_name).to_owned(),
        programs: 1,
        ..PresetFile::default()
    };
    // The bytes at a place in the file, from whichever part holds them.
    let at = |offset: u64, len: u64| -> Option<&[u8]> {
        let end = offset.checked_add(len)?;
        if end <= head.len() as u64 {
            return head.get(offset as usize..end as usize);
        }
        let from = offset.checked_sub(tail_from)?;
        tail.get(usize::try_from(from).ok()?..usize::try_from(end - tail_from).ok()?)
    };
    let Some(list) = vstpreset_list_offset(head) else { return Ok(file) };
    // A missing or damaged list leaves just the plugin's id, which is what matters most.
    if at(list, 4) != Some(b"List".as_slice()) {
        return Ok(file);
    }
    let count = at(list + 4, 4).and_then(|b| le_i32(b, 0)).unwrap_or(0).clamp(0, 64) as u64;
    for i in 0..count {
        let Some(entry) = at(list + 8 + 20 * i, 20) else { break };
        let (Some(offset), Some(size)) = (le_i64(entry, 4), le_i64(entry, 12)) else { break };
        if entry.get(..4) != Some(b"Info".as_slice()) || offset < 0 || !(0..=1 << 20).contains(&size) {
            continue;
        }
        if let Some(xml) = at(offset as u64, size as u64) {
            info_xml(&String::from_utf8_lossy(xml), &mut file);
        }
    }
    Ok(file)
}

/// The facts in a .vstpreset's "Info" chunk: `<Attr id="PlugInName" value="Serum" .../>` lines.
fn info_xml(xml: &str, file: &mut PresetFile) {
    for attr in xml.split("<Attr").skip(1) {
        let (Some(id), Some(value)) = (xml_attribute(attr, "id"), xml_attribute(attr, "value")) else { continue };
        match id.as_str() {
            "PlugInName" => file.plugin = value,
            "Name" if !value.is_empty() => file.name = value,
            "PlugInCategory" => file.kind = kind_of_vst3_category(&value),
            "MusicalCategory" | "MusicalInstrument" if file.category.is_empty() => file.category = value,
            "MusicalCharacter" | "MusicalStyle" => {
                file.tags.extend(value.split('|').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned))
            }
            "Author" | "Creator" if file.creator.is_empty() => file.creator = value,
            _ => {}
        }
    }
}

/// An XML attribute's value in one element's text, with the usual escapes undone.
fn xml_attribute(element: &str, name: &str) -> Option<String> {
    let end_of_tag = element.find('>').unwrap_or(element.len());
    let tag = element.get(..end_of_tag)?;
    let mut rest = tag;
    loop {
        let at = rest.find(name)?;
        let before_ok = at == 0 || rest.get(..at)?.ends_with(char::is_whitespace);
        let after = rest.get(at + name.len()..)?.trim_start();
        if before_ok && let Some(after) = after.strip_prefix('=') {
            let after = after.trim_start();
            let quote = after.chars().next().filter(|&q| q == '"' || q == '\'')?;
            let value = after.get(1..)?;
            let value = value.get(..value.find(quote)?)?;
            return Some(
                value
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&apos;", "'")
                    .replace("&amp;", "&"),
            );
        }
        rest = rest.get(at + name.len()..)?;
    }
}

/// A VST3 category ("Instrument|Synth", "Fx|Delay") as instrument or effect.
fn kind_of_vst3_category(category: &str) -> PluginKind {
    let first = category.split('|').next().unwrap_or("").trim();
    if first.eq_ignore_ascii_case("Instrument") {
        PluginKind::Instrument
    } else if first.eq_ignore_ascii_case("Fx") {
        PluginKind::Effect
    } else {
        PluginKind::Unknown
    }
}

// Bitwig's fields, as `mo_formats::bitwig::reader` names them (the C# BitwigPreset's FDeviceName, FDevicePreset,
// FPluginState).
const F_DEVICE_NAME: u32 = 0x9a;
const F_DEVICE_PRESET: u32 = 0x12de;
const F_PLUGIN_STATE: u32 = 0xbe5;

/// Where Bitwig keeps plugin states in a file's zip.
const STATES: &str = "plugin-states/";

/// A Bitwig preset: the device it loads (from its metadata, else its first named device), what it is, and for a
/// plugin, the plugin's id from its settings file in the zip.
fn read_bwpreset(raw: &[u8], stem: &str) -> Result<PresetFile, NotAPreset> {
    let doc =
        bitwig::decode(raw).map_err(|_| NotAPreset("it doesn't start as a Bitwig file does (\"BtWg\")".into()))?;
    let mut file = PresetFile {
        format: Some(FileFormat::BwPreset),
        name: stem.to_owned(),
        programs: 1,
        plugin: doc.meta_str("device_name").to_owned(),
        creator: doc.meta_str("creator").to_owned(),
        category: doc.meta_str("preset_category").to_owned(),
        tags: doc.meta_strings("tags").into_iter().map(str::to_owned).collect(),
        ..PresetFile::default()
    };
    if !matches!(doc.meta("tags"), Some(Value::Strings(_))) && !doc.meta_str("tags").is_empty() {
        // Some versions keep the tags as one text, spaces between.
        file.tags = doc.meta_str("tags").split_whitespace().map(str::to_owned).collect();
    }
    file.kind = match doc.meta_str("device_type") {
        "instrument" => PluginKind::Instrument,
        "note_effect" | "note-effect" => PluginKind::NoteEffect,
        "audio_to_audio" | "audio-effect" | "audio_effect" => PluginKind::Effect,
        _ => PluginKind::Unknown,
    };

    // The devices that load a plugin, and the one named device to fall back on for the name.
    let mut state_entry = None;
    let mut first_device = None;
    for o in &doc.objects {
        let name = o.str(F_DEVICE_NAME);
        if name.is_empty() {
            continue;
        }
        first_device.get_or_insert((name, o.str(F_DEVICE_PRESET)));
        let state = o.str(F_PLUGIN_STATE);
        if !state.is_empty() && state_entry.is_none() {
            state_entry = Some((name, state.to_owned()));
        }
    }
    if let Some((device, preset)) = first_device {
        if file.plugin.is_empty() {
            file.plugin = device.to_owned();
        }
        if !preset.is_empty() && !preset.eq_ignore_ascii_case(device) {
            file.name = preset.to_owned();
        }
    }
    if let Some((device, entry)) = state_entry {
        file.wraps_plugin = true;
        if file.plugin.is_empty() {
            file.plugin = device.to_owned();
        }
        if let Some((state, entry_name)) = zip_entry(doc.zip(), &entry) {
            let inner = read_bytes(&state, &entry_name).ok();
            if let Some(inner) = inner {
                file.plugin_id = inner.plugin_id;
                if file.kind == PluginKind::Unknown {
                    file.kind = inner.kind;
                }
            }
        }
    }
    Ok(file)
}

/// The most of a plugin state in a Bitwig preset that's read for its header.
const STATE_HEAD: u64 = 64 * 1024;

/// The start of a plugin state in a Bitwig zip and its file's name, matched as the Bitwig reader matches a device's
/// state field to its file: by the file's name, else by its name without the extension (letter case aside).
fn zip_entry(zip: Option<&[u8]>, state: &str) -> Option<(Vec<u8>, String)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip?)).ok()?;
    let names: Vec<(usize, String)> = (0..archive.len())
        .filter_map(|i| Some((i, archive.name_for_index(i).and_then(Result::ok)?.into_owned())))
        .filter(|(_, n)| n.starts_with(STATES))
        .collect();
    let (index, full) = names
        .iter()
        .find(|(_, n)| paths::entry_name(n).eq_ignore_ascii_case(state))
        .or_else(|| names.iter().find(|(_, n)| paths::stem(paths::entry_name(n)).eq_ignore_ascii_case(state)))?;
    let entry = archive.by_index(*index).ok()?;
    let mut out = Vec::new();
    entry.take(STATE_HEAD).read_to_end(&mut out).ok()?;
    Some((out, paths::entry_name(full).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_attributes_with_either_quote_and_escapes() {
        let el = r#" id="Name" value='Tom &amp; Jerry' type="string"/>"#;
        assert_eq!(xml_attribute(el, "value").as_deref(), Some("Tom & Jerry"));
        assert_eq!(xml_attribute(el, "id").as_deref(), Some("Name"));
        // "id" inside "valid" isn't the attribute.
        assert_eq!(xml_attribute(r#" valid="x" id="y""#, "id").as_deref(), Some("y"));
        assert_eq!(xml_attribute(r#" value="x""#, "id"), None);
    }

    #[test]
    fn knows_formats_by_extension() {
        assert_eq!(FileFormat::of("A.FXP"), Some(FileFormat::Fxp));
        assert_eq!(FileFormat::of(r"C:\x\Bank.fxb"), Some(FileFormat::Fxb));
        assert_eq!(FileFormat::of("Pad.vstpreset"), Some(FileFormat::VstPreset));
        assert_eq!(FileFormat::of("Bass.bwpreset"), Some(FileFormat::BwPreset));
        assert_eq!(FileFormat::of("Song.flp"), None);
        assert_eq!(kind_of_vst3_category("Instrument|Synth"), PluginKind::Instrument);
        assert_eq!(kind_of_vst3_category("Fx|Delay"), PluginKind::Effect);
        assert_eq!(kind_of_vst3_category(""), PluginKind::Unknown);
    }
}
