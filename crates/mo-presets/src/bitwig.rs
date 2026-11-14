//! Send to Bitwig: Bitwig's own preset files (`.bwpreset`) made from plugin settings taken out of a song, as the C#
//! `BitwigPreset` does.
//!
//! A `.bwpreset` is a Bitwig document like a project: the device and the plugin it loads are in its body, and the
//! plugin's settings sit in the zip at its end as a plain `.fxp`/`.fxb` (VST2) or `.vstpreset` (VST3) file, named by
//! the device's state field. So a preset Bitwig itself saved for that plugin is used as a template: its body is kept,
//! the settings file in its zip is swapped for the new one under a name of its own ([`fresh_state_name`]), and the
//! preset's name is changed. A plugin with no template of its own borrows another plugin's of the same format
//! ([`stand_in_for`]), a synth's for a synth where there is one, whose device takes the sound's plugin name and id,
//! and what the sound's plugin is (instrument or effect, so Bitwig puts a synth on an instrument track), as the
//! converter makes plugin devices from the Bitwig template's.
//!
//! Unlike the C# app, which swapped names as bytes, the template is decoded, changed field by field and written again
//! with `mo_formats::bitwig::encode` (the same writer the converter uses), then the new file is decoded again and
//! checked before it's used. A template is only read: nothing here changes it.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mo_formats::bitwig::{self, Document, ObjId, Text, Value};
use mo_formats::presets::PresetBlob;
use mo_formats::{PluginKind, paths};
use rayon::prelude::*;

use crate::files::{self, F_DEVICE_NAME, F_DEVICE_PRESET, F_PLUGIN_STATE, FileFormat};
use crate::library::FoundPreset;

/// A preset Bitwig saved for one plugin, usable as a template for that plugin's sounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub path: PathBuf,
    /// The device's name: the plugin as Bitwig names it.
    pub plugin: String,
    /// The device's state field: the settings file's name (sometimes without its ending).
    pub state_field: String,
    /// The settings file's full name in the zip ("plugin-states/....fxb").
    pub state_file: String,
    /// The preset's name as the device keeps it, which is changed to the new sound's name.
    pub preset_name: String,
    /// The plugin's id as its settings say (a VST2's four letters, a VST3's class id), or "" when they don't.
    pub plugin_id: String,
    /// What the plugin is (instrument or effect), as the preset says ([`kind_of`]), so a synth's sound borrows a
    /// synth's template first ([`stand_in_for`]).
    pub kind: PluginKind,
    /// When the file was last saved, so the newest template for a plugin wins.
    pub modified: Option<SystemTime>,
}

impl Template {
    /// "VST2" or "VST3", from its settings file.
    pub fn format(&self) -> &'static str {
        state_format(paths::extension(&self.state_file))
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

/// The lookup key for a plugin and format, so "Serum_x64" finds a template saved for "Serum" (the library's name
/// key), or `None` when the name has nothing to match by (no letters or digits), so it never matches another such.
pub fn key(plugin: &str, format: &str) -> Option<String> {
    let name = mo_formats::plugins::name_key(plugin);
    (!name.is_empty() && !format.is_empty()).then(|| format!("{name}|{format}"))
}

/// Templates by [`key`].
pub type Templates = HashMap<String, Template>;

/// A `.bwpreset` that can serve as a template ([`inspect_doc`]), with its path and time.
pub fn inspect(path: &Path) -> Option<Template> {
    // The file's bytes handed to the decoder as they are, not copied (rule 24).
    let mut template = inspect_doc(&bitwig::decode(std::fs::read(path).ok()?).ok()?)?;
    template.path = path.to_path_buf();
    template.modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Some(template)
}

/// A `.bwpreset`'s bytes as a template, if they can be one; the path and time are left empty. See [`inspect_doc`].
pub fn inspect_bytes(raw: &[u8]) -> Option<Template> {
    inspect_doc(&bitwig::decode(raw).ok()?)
}

/// A decoded `.bwpreset` as a template, if it can be one: it decodes cleanly, exactly one device in it loads a plugin
/// and no other device sits beside it (a chain with Bitwig's own reverb after the plugin isn't a template for the
/// plugin alone; what's inside the device, such as its modulators, is kept), and that plugin's settings are in its zip
/// as a VST2 or VST3 preset.
pub fn inspect_doc(doc: &Document) -> Option<Template> {
    if doc.body_error.is_some() {
        return None;
    }
    let (device, field) = one_plugin(doc)?;
    let o = doc.obj(device);
    let mut archive = zip::ZipArchive::new(Cursor::new(doc.zip()?)).ok()?;
    let (index, state_file) = files::state_file(&archive, &field)?;
    let entry_name = paths::entry_name(&state_file).to_owned();
    if state_format(paths::extension(&entry_name)).is_empty() {
        return None;
    }
    // Only the settings' head, for the plugin's id (and what it is, when the preset doesn't say).
    let mut head = Vec::new();
    archive.by_index(index).ok()?.take(64 * 1024).read_to_end(&mut head).ok()?;
    let inner = files::read_bytes(&head, &entry_name).ok();
    let kind = match kind_of(doc, device) {
        PluginKind::Unknown => inner.as_ref().map_or(PluginKind::Unknown, |f| f.kind),
        kind => kind,
    };
    Some(Template {
        path: PathBuf::new(),
        plugin: o.str(F_DEVICE_NAME).to_owned(),
        state_field: field,
        state_file,
        preset_name: o.str(F_DEVICE_PRESET).to_owned(),
        plugin_id: inner.map(|f| f.plugin_id).unwrap_or_default(),
        kind,
        modified: None,
    })
}

/// What a Bitwig preset's plugin device is: as its metadata says (`device_type`), else as the device does (Bitwig's
/// kind field, else its category), as the preset files' reader takes it ([`files::bwpreset_of`], which then asks the
/// settings in the zip, as a template's kind does).
pub fn kind_of(doc: &Document, device: ObjId) -> PluginKind {
    match files::kind_of_device_type(doc.meta_str("device_type")) {
        PluginKind::Unknown => bitwig::device_kind(doc.obj(device)),
        kind => kind,
    }
}

/// The one device that loads a plugin and its state field, when there's one and no other device beside it.
fn one_plugin(doc: &Document) -> Option<(ObjId, String)> {
    let ids = || (0..doc.objects.len()).filter_map(|i| u32::try_from(i).ok()).map(ObjId);
    let mut holders = ids().filter(|&id| !doc.obj(id).str(F_PLUGIN_STATE).is_empty());
    let device = holders.next()?;
    if holders.next().is_some() || doc.obj(device).str(F_DEVICE_NAME).is_empty() {
        return None;
    }
    let beside = ids().any(|id| {
        id != device && !doc.obj(id).str(F_DEVICE_NAME).is_empty() && doc.ancestor(id, |p, _| p == device).is_none()
    });
    (!beside).then(|| (device, doc.obj(device).str(F_PLUGIN_STATE).to_owned()))
}

/// The templates among preset files the page already found ([`crate::library::find_preset_files`]), so Bitwig's
/// folder isn't walked twice: each `.bwpreset` that loads a plugin is read over every core, and the newest one per
/// plugin and format wins (by path when two were saved at the same time, so it's the same every run).
pub fn templates(found: &[FoundPreset]) -> Templates {
    let all: Vec<Template> =
        found.par_iter().filter(|f| may_be_template(f)).filter_map(|f| inspect_caught(Path::new(&f.path))).collect();
    newest_templates(all)
}

/// Whether a preset file found may be a template: a Bitwig preset that loads a plugin.
pub fn may_be_template(f: &FoundPreset) -> bool {
    f.file.as_ref().is_ok_and(|p| p.format == Some(FileFormat::BwPreset) && p.wraps_plugin)
}

/// [`inspect`], with a reader that trips over a damaged file caught: that file is only not a template.
pub fn inspect_caught(path: &Path) -> Option<Template> {
    mo_catch::catch(|| inspect(path)).ok().flatten()
}

/// The newest template per plugin and format among `all` (by path when two were saved at the same time, so it's the
/// same every run).
pub fn newest_templates(mut all: Vec<Template>) -> Templates {
    all.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    let mut out = Templates::new();
    for t in all {
        if let Some(k) = key(&t.plugin, t.format()) {
            out.entry(k).or_insert(t);
        }
    }
    out
}

/// The template for an exported sound, if one was found for its plugin and format.
pub fn template_for<'a>(preset: &PresetBlob, templates: &'a Templates) -> Option<&'a Template> {
    templates.get(&key(&preset.plugin, state_format(&preset.extension))?)
}

/// The template saved for the plugin with this id (a VST2's four letters, a VST3's class id, as a preset file's
/// settings give it) in this format, the newest when there are several: a VST2 preset file names its plugin only by
/// its id, which [`template_for`] can't match by name. The ids must be the same to the letter: a VST2's are four
/// bytes, so "abcd" and "ABCD" are two plugins (a VST3's are always written in capitals).
pub fn template_by_id<'a>(plugin_id: &str, format: &str, templates: &'a Templates) -> Option<&'a Template> {
    if plugin_id.is_empty() || format.is_empty() {
        return None;
    }
    templates
        .values()
        .filter(|t| t.format() == format && t.plugin_id == plugin_id)
        .min_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)))
}

/// A stand-in template for a sound whose plugin has no template of its own: the newest template of the same format
/// (VST2 or VST3) for another plugin, one for a plugin of the same kind first (a synth's for a synth) when the sound's
/// kind is known. [`build_for_other`] gives it the sound's plugin, and the sound's kind when it's another. (Solomon, 5
/// Nov 2026: Send to Bitwig never sent Hive or WAVESTATION, as he had saved no Bitwig preset of either; 14 Nov: a
/// dragged Keyscape preset went onto an audio track.)
pub fn stand_in_for<'a>(preset: &PresetBlob, templates: &'a Templates) -> Option<&'a Template> {
    let format = state_format(&preset.extension);
    if format.is_empty() {
        return None;
    }
    // The newest, by path when two were saved at the same time, so it's the same every run.
    let newest = |of: &dyn Fn(&Template) -> bool| {
        templates
            .values()
            .filter(|t| t.format() == format && of(t))
            .min_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)))
    };
    let same_kind = match preset.kind {
        PluginKind::Unknown => None,
        kind => newest(&|t| t.kind == kind),
    };
    same_kind.or_else(|| newest(&|_| true))
}

/// Who a stand-in template's device becomes: the sound's plugin, by its name and its id as its settings say.
#[derive(Clone, Debug, PartialEq)]
struct Identity {
    name: String,
    id: Value,
    /// Instrument or effect, where the song or the file says; the stand-in's device and metadata are told which.
    kind: PluginKind,
}

/// The plugin the settings are for, as Bitwig's device names it: a VST2's id is its number (from the program's or
/// bank's head), a VST3's its class id (from the `.vstpreset`'s head). `None` when the settings don't say.
fn identity(plugin: &str, kind: PluginKind, state: &[u8], extension: &str) -> Option<Identity> {
    let id = match state_format(extension) {
        "VST2" if state.get(..4) == Some(b"CcnK") => {
            let id = i32::from_be_bytes(state.get(16..20)?.try_into().ok()?);
            (id != 0).then(|| smallest(i64::from(id)))?
        }
        "VST3" if state.get(..4) == Some(b"VST3") => {
            let class = mo_formats::plugins::class_id(std::str::from_utf8(state.get(8..40)?).ok()?);
            (!class.is_empty()).then(|| Value::Str(Text::new(&class)))?
        }
        _ => return None,
    };
    Some(Identity { name: plugin.to_owned(), id, kind })
}

/// The smallest whole-number kind that holds `v`, as Bitwig writes them.
fn smallest(v: i64) -> Value {
    if let Ok(b) = u8::try_from(v) {
        Value::U8(b)
    } else if let Ok(s) = i16::try_from(v) {
        Value::I16(s)
    } else if let Ok(i) = i32::try_from(v) {
        Value::I32(i)
    } else {
        Value::I64(v)
    }
}

/// A VST2 program (`.fxp`) as a one-program bank (`.fxb`), the form Bitwig saves VST2 settings in and loads back
/// (it loaded a plugin's defaults from a `.fxp`), keeping the program the plugin had selected. `None` when it isn't a
/// program that can be one. (mo-formats' `fxp_to_bank`, shared with the converter.)
pub fn to_bank(fxp: &[u8], program: u32) -> Option<Vec<u8>> {
    // The bank's counts are signed: a damaged save's program past them is taken as the first.
    let program = if program >= i32::MAX as u32 { 0 } else { program };
    let bank = mo_formats::presets::fxp_to_bank(fxp, program);
    (bank != fxp).then_some(bank)
}

/// Builds a `.bwpreset` for `preset` from a template's bytes (`template` is its file, read once per export), named
/// `preset_name`, and checks it reads back with the new settings, as Bitwig would read it. A VST2 program goes in as
/// a one-program bank ([`to_bank`]). A sound for another plugin than the template's (by the id in its settings) is
/// refused.
pub fn build(template: &[u8], template_path: &Path, preset: &PresetBlob, preset_name: &str) -> Result<Vec<u8>, String> {
    build_any(template, template_path, preset, preset_name, false)
}

/// [`build`] from a stand-in template ([`stand_in_for`]) saved for another plugin of the same format: the device
/// takes the sound's plugin, by its name and the id in its settings, as the converter makes a plugin's device from
/// the Bitwig template's (what only the template's plugin had, its version, outputs, program and file, is left for
/// Bitwig to read again when it loads the plugin). Refused when the settings don't say which plugin they're for.
pub fn build_for_other(
    template: &[u8],
    template_path: &Path,
    preset: &PresetBlob,
    preset_name: &str,
) -> Result<Vec<u8>, String> {
    build_any(template, template_path, preset, preset_name, true)
}

fn build_any(
    template: &[u8],
    template_path: &Path,
    preset: &PresetBlob,
    preset_name: &str,
    other: bool,
) -> Result<Vec<u8>, String> {
    let (state, extension) = match preset.extension.eq_ignore_ascii_case(".fxp") {
        true => match to_bank(&preset.data, preset.program) {
            Some(bank) => (bank, ".fxb"),
            None => (preset.data.clone(), ".fxp"),
        },
        false => (preset.data.clone(), preset.extension.as_ref()),
    };
    let stem = paths::stem(paths::file_name(&template_path.to_string_lossy())).to_owned();
    let who = match other {
        true => Some(
            identity(&preset.plugin, preset.kind, &state, extension)
                .ok_or_else(|| format!("{}'s settings don't say which plugin they're for", preset.plugin))?,
        ),
        false => None,
    };
    build_with(template, &stem, &state, extension, preset_name, who.as_ref())
}

/// Builds a `.bwpreset` from a template's bytes with `state` as the plugin's settings, then checks it.
///
/// - `state_extension` is the settings file's type (`.fxp`, `.fxb` or `.vstpreset`); the file in the zip and the
///   device's state field are renamed when it differs from the template's.
/// - The template's own names for its preset (`file_stem`, its file's name, and the device's preset name) become
///   `preset_name` wherever one is the whole of exactly one text in the metadata or in the body. A name found more
///   than once is left alone rather than risk changing the wrong field, and the plugin's name is never changed, even
///   when the preset shares it.
pub fn build_from(
    template: &[u8],
    file_stem: &str,
    state: &[u8],
    state_extension: &str,
    preset_name: &str,
) -> Result<Vec<u8>, String> {
    build_with(template, file_stem, state, state_extension, preset_name, None)
}

fn build_with(
    template: &[u8],
    file_stem: &str,
    state: &[u8],
    state_extension: &str,
    preset_name: &str,
    other: Option<&Identity>,
) -> Result<Vec<u8>, String> {
    let mut doc = bitwig::decode(template).map_err(|_| "the template isn't a Bitwig file".to_owned())?;
    let t = inspect_doc(&doc).ok_or("the template no longer loads one plugin with its settings")?;
    if state_format(state_extension) != t.format() {
        return Err(format!("the template is for {}, not {state_extension}", t.format()));
    }
    let new_id = files::read_bytes(state, &format!("x{state_extension}")).map(|f| f.plugin_id).unwrap_or_default();
    // To the letter, as [`template_by_id`] matches them: a VST2's "abcd" and "ABCD" are two plugins.
    if other.is_none() && !t.plugin_id.is_empty() && !new_id.is_empty() && t.plugin_id != new_id {
        return Err(format!(
            "the sound is for plugin \"{new_id}\", the template's {} is \"{}\"",
            t.plugin, t.plugin_id
        ));
    }

    // The settings file takes a name of its own ([`fresh_state_name`]) with the new ending, in the template's folder
    // in the zip; the state field names it as the template's named its own (with its ending or without).
    let old_name = paths::entry_name(&t.state_file);
    let new_stem = fresh_state_name();
    let new_name = format!("{new_stem}{state_extension}");
    let new_file = format!("{}{new_name}", t.state_file.strip_suffix(old_name).unwrap_or_default());
    let new_field = if paths::extension(&t.state_field).is_empty() { new_stem } else { new_name };
    let (device, _) = one_plugin(&doc).ok_or("the template no longer loads one plugin")?;
    doc.obj_mut(device).set(F_PLUGIN_STATE, Value::Str(Text::new(&new_field)));
    if let Some(who) = other {
        become_plugin(&mut doc, device, who, t.kind);
    }
    let plugin = other.map_or(t.plugin.as_str(), |w| w.name.as_str());

    // The preset's name, wherever the template keeps its own.
    if !preset_name.is_empty() {
        let mut done: Vec<&str> = Vec::new();
        for old in [file_stem, t.preset_name.as_str()] {
            if old.is_empty()
                || old.eq_ignore_ascii_case(&t.plugin)
                || old.eq_ignore_ascii_case(plugin)
                || done.contains(&old)
            {
                continue;
            }
            done.push(old);
            rename_once(&mut doc, old, preset_name);
        }
    }

    let zip = swap_zip_file(doc.zip().unwrap_or_default(), &t.state_file, &new_file, state)?;
    let bytes = bitwig::encode(&doc, Some(&zip))?;
    check(&bytes, plugin, &new_field, &new_file, state)?;
    Ok(bytes)
}

/// A name for the plugin's settings file in a preset made here, new each time, as Bitwig names each one it saves (a
/// version 4 UUID, "8b7c6b5b-349f-4805-ae01-e3bcafb33be8"). Bitwig opens a preset's settings from its own
/// plugin-states folder by that name (its error names the file there), so a preset made under its template's name
/// could open with what Bitwig already kept under it, the template's settings or an earlier drag's, rather than its own
/// (Solomon, 14 Nov 2026: a dragged VST3 preset failed with "Error loading VST3 preset" on that file, and another came
/// up at its plugin's default sound).
pub fn fresh_state_name() -> String {
    let hex: String = bitwig::encode::fresh_id().iter().map(|b| format!("{b:02x}")).collect();
    let part = |from: usize, to: usize| hex.get(from..to).unwrap_or_default();
    format!("{}-{}-{}-{}-{}", part(0, 8), part(8, 12), part(12, 16), part(16, 20), part(20, 32))
}

// The plugin device's fields a stand-in template's device changes, as the converter's `plugin` does.
const F_VENDOR: u32 = 0x9b;
const F_CATEGORY: u32 = 0x9c;
/// Instrument (2), note effect (1) or audio effect (0), as Bitwig saves it.
const F_DEVICE_KIND: u32 = 0x9d;
/// 1 on an instrument plugin, 0 on an effect, as Bitwig saves it (what it stands for isn't known).
const F_INSTRUMENT_FLAG: u32 = 0x1809;
const F_PLUGIN_ID: u32 = 0xce4;
const F_PLUGIN_VERSION: u32 = 0xc5e;
const F_AUTOMATED: u32 = 0x1318;
const F_OUTPUTS: u32 = 0x137c;
const F_OUTPUT_LIST: u32 = 0x8e0;
const F_PROGRAM: u32 = 0x1b75;
const F_PROGRAM_NAME: u32 = 0x1b72;
const F_LOCATION: u32 = 0xc5a;
const F_LOCATION_FILES: u32 = 0xc5b;

/// A field's value replaced, whatever kind it was (an id can be a number in one preset and text in another), or
/// added when the object has none.
fn put(doc: &mut Document, obj: ObjId, id: u32, value: Value) {
    let o = doc.obj_mut(obj);
    match o.fields.iter_mut().find(|f| f.id == id) {
        Some(f) => f.value = value,
        None => o.fields.push(bitwig::Field { id, value }),
    }
}

/// What Bitwig says a plugin of this kind is: its device's kind field, the metadata's device type, and a category for
/// one that was another kind (a stand-in effect's own, "EQ" say, isn't a synth's). `None` for a kind not known.
fn kind_words(kind: PluginKind) -> Option<(u8, &'static str, &'static str)> {
    match kind {
        PluginKind::Instrument => Some((2, "note_to_audio", "Synth")),
        PluginKind::NoteEffect => Some((1, "note_to_note", "Note FX")),
        PluginKind::Effect => Some((0, "audio_to_audio", "Audio FX")),
        PluginKind::Unknown => None,
    }
}

/// A metadata text changed to `value`, when the preset has it (none is added).
fn set_meta(doc: &mut Document, key: &str, value: &str) {
    if let Some((_, v)) = doc.meta.iter_mut().rev().find(|(k, _)| k.as_str() == key) {
        *v = Value::Str(Text::new(value));
    }
}

/// A plugin id as text, as its device holds it (a VST3's class id, a VST2's number); `None` for none.
fn id_text(v: &Value) -> Option<String> {
    let text = match v {
        Value::Str(t) => t.as_str().to_owned(),
        Value::U8(n) => n.to_string(),
        Value::I16(n) => n.to_string(),
        Value::I32(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        _ => return None,
    };
    (!text.is_empty() && text != "0").then_some(text)
}

/// `text` with the plugin id `old` in it (the whole of it, or a whole word in it, letter case aside) swapped for `new`;
/// `None` when it isn't there.
fn swapped_id(text: &str, old: &str, new: &str) -> Option<String> {
    if old.is_empty() || !old.is_ascii() {
        return None;
    }
    let (lower, old_lower) = (text.to_ascii_lowercase(), old.to_ascii_lowercase());
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let (mut out, mut from, mut found) = (String::new(), 0, false);
    while let Some(at) = lower.get(from..).and_then(|l| l.find(&old_lower)).map(|i| i + from) {
        let end = at + old.len();
        let alone = !word(text.get(..at)?.chars().next_back()) && !word(text.get(end..)?.chars().next());
        out.push_str(text.get(from..at)?);
        out.push_str(if alone { new } else { text.get(at..end)? });
        found |= alone;
        from = end;
    }
    out.push_str(text.get(from..)?);
    found.then_some(out)
}

/// A stand-in template's device becomes the sound's plugin: its name and id, what it is (instrument or effect) where
/// that's known, in the device and in the preset's metadata (Bitwig's browser and a drop on the track list go by it),
/// and nothing kept that only the template's plugin had (its maker, version, automated settings, outputs, program name
/// and file), which Bitwig reads again from the plugin when it loads it. `was` is what the template's plugin is.
fn become_plugin(doc: &mut Document, device: ObjId, who: &Identity, was: PluginKind) {
    let old_id = doc.obj(device).get(F_PLUGIN_ID).and_then(id_text);
    put(doc, device, F_DEVICE_NAME, Value::Str(Text::new(&who.name)));
    put(doc, device, F_PLUGIN_ID, who.id.clone());
    if let Some((kind_field, device_type, category)) = kind_words(who.kind) {
        // As the kinds of value the template's device holds them in (`set`).
        if doc.obj(device).has(F_DEVICE_KIND) {
            doc.obj_mut(device).set(F_DEVICE_KIND, Value::U8(kind_field));
        }
        if who.kind != PluginKind::NoteEffect && doc.obj(device).has(F_INSTRUMENT_FLAG) {
            doc.obj_mut(device).set(F_INSTRUMENT_FLAG, Value::U8(u8::from(who.kind == PluginKind::Instrument)));
        }
        if who.kind != was {
            if doc.obj(device).has(F_CATEGORY) {
                put(doc, device, F_CATEGORY, Value::Str(Text::new(category)));
            }
            set_meta(doc, "device_category", category);
        }
        if files::kind_of_device_type(doc.meta_str("device_type")) != who.kind {
            set_meta(doc, "device_type", device_type);
        }
    }
    set_meta(doc, "device_name", &who.name);
    set_meta(doc, "device_creator", "");
    // The template's plugin's id where the metadata names the device by it.
    if let (Some(old), Some(new)) = (old_id, id_text(&who.id)) {
        for (key, value) in doc.meta.iter_mut() {
            if !matches!(key.as_str(), "device_id" | "referenced_device_ids") {
                continue;
            }
            match value {
                Value::Str(t) => {
                    if let Some(swapped) = swapped_id(t.as_str(), &old, &new) {
                        *t = Text::new(&swapped);
                    }
                }
                Value::Strings(list) => {
                    for t in list.iter_mut() {
                        if let Some(swapped) = swapped_id(t.as_str(), &old, &new) {
                            *t = Text::new(&swapped);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if doc.obj(device).has(F_VENDOR) {
        put(doc, device, F_VENDOR, Value::Str(Text::new("")));
    }
    if doc.obj(device).has(F_PLUGIN_VERSION) {
        let blank = match who.id {
            Value::Str(_) => Value::Str(Text::new("")),
            _ => Value::U8(0),
        };
        put(doc, device, F_PLUGIN_VERSION, blank);
    }
    if doc.obj(device).has(F_AUTOMATED) {
        put(doc, device, F_AUTOMATED, Value::List(Vec::new()));
    }
    for (holder, field, blank) in [
        (F_OUTPUTS, F_OUTPUT_LIST, Value::List(Vec::new())),
        (F_PROGRAM, F_PROGRAM_NAME, Value::Str(Text::new(""))),
        (F_LOCATION, F_LOCATION_FILES, Value::List(Vec::new())),
    ] {
        if let Some(o) = doc.field_obj(device, holder)
            && doc.obj(o).has(field)
        {
            put(doc, o, field, blank);
        }
    }
}

/// Changes a text that's exactly `old` to `new`, in the metadata and in the body (each looked at on its own), when
/// it's there once. The device's own name is never changed.
fn rename_once(doc: &mut Document, old: &str, new: &str) {
    let is_old = |v: &Value| matches!(v, Value::Str(t) if t.as_str() == old);
    let mut in_meta = doc.meta.iter_mut().filter(|(_, v)| is_old(v));
    if let (Some((_, v)), None) = (in_meta.next(), in_meta.next()) {
        *v = Value::Str(Text::new(new));
    }
    let mut in_body = doc.objects.iter_mut().flat_map(|o| o.fields.iter_mut()).filter(|f| is_old(&f.value));
    if let (Some(field), None) = (in_body.next(), in_body.next())
        && field.id != F_DEVICE_NAME
    {
        field.value = Value::Str(Text::new(new));
    }
}

/// The template's zip with its settings file swapped for `state` (renamed to `new_file`); every other file is copied
/// as it was, in its place.
fn swap_zip_file(zip: &[u8], old_file: &str, new_file: &str, state: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("the template's zip: {e}"))?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut swapped = false;
    for i in 0..input.len() {
        let entry = input.by_index_raw(i).map_err(|e| format!("the template's zip: {e}"))?;
        if entry.name().is_ok_and(|n| n == old_file) {
            let mut options =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            if let Some(time) = entry.last_modified() {
                options = options.last_modified_time(time);
            }
            drop(entry);
            out.start_file(new_file, options).map_err(|e| e.to_string())?;
            out.write_all(state).map_err(|e| e.to_string())?;
            swapped = true;
        } else {
            out.raw_copy_file(entry).map_err(|e| format!("the template's zip: {e}"))?;
        }
    }
    if !swapped {
        return Err(format!("the template's zip has no \"{old_file}\""));
    }
    Ok(out.finish().map_err(|e| e.to_string())?.into_inner())
}

/// The new preset reads back as Bitwig would read it: cleanly to its zip, one device for `plugin` naming `field`,
/// whose settings file is `file`, holding `state` byte for byte.
fn check(bytes: &[u8], plugin: &str, field: &str, file: &str, state: &[u8]) -> Result<(), String> {
    let doc = bitwig::decode(bytes).map_err(|_| "the new preset isn't a Bitwig file".to_owned())?;
    if let Some(e) = &doc.body_error {
        return Err(format!("the new preset doesn't read back: {e}"));
    }
    let t = inspect_doc(&doc).ok_or("the new preset doesn't load one plugin with its settings")?;
    if t.plugin != plugin || t.state_field != field || t.state_file != file {
        return Err("the new preset's device changed".into());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(doc.zip().unwrap_or_default())).map_err(|e| e.to_string())?;
    let mut read = Vec::new();
    archive.by_name(file).map_err(|e| e.to_string())?.read_to_end(&mut read).map_err(|e| e.to_string())?;
    if read != state {
        return Err("the new preset lost its plugin settings".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::presets::fxp;

    #[test]
    fn a_chunk_program_becomes_a_bank_keeping_its_program() {
        let p = fxp::wrap_chunk(&[1, 2, 3], "Big Bass", 0x5866_7358);
        let bank = to_bank(&p, 5).unwrap();
        assert_eq!(&bank[8..12], b"FBCh");
        assert_eq!(i32::from_be_bytes(bank[4..8].try_into().unwrap()) as usize, bank.len() - 8);
        assert_eq!(&bank[16..24], &p[16..24]);
        assert_eq!(i32::from_be_bytes(bank[24..28].try_into().unwrap()), 6);
        assert_eq!(i32::from_be_bytes(bank[28..32].try_into().unwrap()), 5);
        assert_eq!(&bank[160..], &[1, 2, 3]);
        // A damaged save's program number never overflows.
        assert!(to_bank(&p, u32::MAX).is_some());
        assert!(to_bank(&p, i32::MAX as u32).is_some());
    }

    #[test]
    fn what_isnt_a_whole_program_stays_as_it_is() {
        assert_eq!(to_bank(b"not a preset at all, not at all, not at all, not at all..", 0), None);
        let mut p = fxp::wrap_chunk(&[1, 2, 3], "x", 1);
        p.pop(); // the chunk's size no longer matches
        assert_eq!(to_bank(&p, 0), None);
    }

    #[test]
    fn formats_and_keys() {
        assert_eq!(state_format(".FXB"), "VST2");
        assert_eq!(state_format(".vstpreset"), "VST3");
        assert_eq!(state_format(".flstate"), "");
        assert_eq!(key("Serum_x64", "VST2"), key("Serum", "VST2"));
        // A name with nothing to match by matches nothing.
        assert_eq!(key("", "VST2"), None);
        assert_eq!(key("Сирена", "VST2"), None);
        assert_eq!(key("Serum", ""), None);
    }
}
