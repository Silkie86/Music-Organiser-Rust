//! Dragging a preset out of the Presets page onto a track in a DAW (Solomon, 13 Nov 2026: a dragged preset only
//! worked when it was a Bitwig preset, and "both presets and curves should be draggable").
//!
//! Bitwig only loads a preset dropped on a track when it's one of its own (`.bwpreset`), so a drag carries one
//! wherever it can:
//! - a Bitwig preset file goes as it is;
//! - a VST2 or VST3 preset file (`.fxp`, `.fxb`, `.vstpreset`), or a song's VST2 or VST3 sound, goes as a Bitwig
//!   preset made from it, the way Send to Bitwig makes one ([`crate::bitwig`]): from a preset Bitwig saved for that
//!   plugin (found by the plugin's id in the settings, else by its name), else from one saved for another plugin of
//!   the same format;
//! - anything else goes as the file it is: a preset file where it is, a song's sound as the plugin preset an export
//!   writes ("Serum - Lead.fxp"), for the plugin's own window, another DAW or a folder to take.
//!
//! Nothing here writes a file: what's made is handed back, and the drag writes it into a folder of its own.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use mo_formats::presets::PresetBlob;
use mo_formats::{PluginKind, paths};

use crate::bitwig::{self, Templates};
use crate::export;
use crate::files;
use crate::names;

/// The file a dragged preset goes out as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum File {
    /// A file on the disk, as it is: nothing is made.
    There(PathBuf),
    /// A file made for the drag: its name ("Serum - Lead.bwpreset") and what goes in it.
    Made { name: String, bytes: Vec<u8> },
}

/// What a dragged preset carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    pub file: File,
    /// It's a Bitwig preset: the kind Bitwig loads when it's dropped on a track.
    pub bitwig: bool,
    /// Why it isn't a Bitwig preset when one might have been made of it (no preset Bitwig saved for a plugin of its
    /// format, say), for the log; `None` when it is one, or is a kind none can be made of.
    pub why_not: Option<String>,
}

/// The longest preset file read to make a Bitwig preset of: a VST2 bank of many programs is a few MB.
const LARGEST: u64 = 64 << 20;

/// A preset file to drag out (see the top of this file). `plugin` is its plugin's name as the page shows it (a VST2
/// file gives only the plugin's four letters, which is what the page shows then), and `kind` what the page says it is
/// (the installed plugin's own word, where the library knows it), which the file's own word stands in for when the page
/// doesn't know: a Bitwig preset made for a synth is an instrument's. Reads the file, so never on the window's thread.
/// An error when it isn't a file any more.
pub fn from_file(path: &Path, plugin: &str, kind: PluginKind, templates: Option<&Templates>) -> io::Result<Carried> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "it isn't a file any more"));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = paths::extension(&name).to_ascii_lowercase();
    let is_bitwig = ext == ".bwpreset";
    let as_it_is =
        |why_not: Option<String>| Carried { file: File::There(path.to_path_buf()), bitwig: is_bitwig, why_not };
    let format = bitwig::state_format(&ext);
    if format.is_empty() {
        return Ok(as_it_is(None));
    }
    let Some(templates) = templates.filter(|t| !t.is_empty()) else {
        return Ok(as_it_is(Some(no_template(format))));
    };
    if meta.len() > LARGEST {
        return Ok(as_it_is(Some("it's too big to make a Bitwig preset of".to_owned())));
    }
    let raw = std::fs::read(path)?;
    let file = match files::read_bytes(&raw, &name) {
        Ok(file) => file,
        Err(e) => return Ok(as_it_is(Some(format!("it doesn't read as a preset: {e}")))),
    };
    let mut blob = PresetBlob::new(&file.name, if file.plugin.is_empty() { plugin } else { &file.plugin }, ext, raw);
    blob.kind = export::kind_for(kind, file.kind);
    match made(&blob, &file.plugin_id, templates) {
        None => Ok(as_it_is(Some(no_template(format)))),
        Some(Err(why)) => Ok(as_it_is(Some(why))),
        Some(Ok(bytes)) => Ok(Carried {
            file: File::Made { name: format!("{}.bwpreset", names::safe_file_name(paths::stem(&name))), bytes },
            bitwig: true,
            why_not: None,
        }),
    }
}

/// A song's sound to drag out (see the top of this file), by its save and its key there ([`crate::list::place_keys`]):
/// read from the save again, as an export reads it, and named as an export names it. `kind` is what the page says it is
/// (what the library knows of its plugin fills in what the save doesn't say), so a synth's goes as an instrument's
/// Bitwig preset. Reads the save, so never on the window's thread. An error when the save can't be read, the sound
/// isn't in it any more, or its settings don't read back as a preset.
pub fn from_save(save: &str, key: &str, kind: PluginKind, templates: Option<&Templates>) -> io::Result<Carried> {
    let (blob, name) = export::sound_in_save(save, key, kind)?;
    Ok(from_sound(blob, name, templates))
}

/// A song's sound, named `name` as an export names its plugin preset: a Bitwig preset made from it when it can be,
/// else that plugin preset.
pub fn from_sound(blob: PresetBlob, name: String, templates: Option<&Templates>) -> Carried {
    let format = bitwig::state_format(&blob.extension);
    let why_not = if format.is_empty() {
        None
    } else {
        let made = templates.filter(|t| !t.is_empty()).and_then(|t| made(&blob, "", t));
        match made {
            Some(Ok(bytes)) => {
                let name = export::bitwig_name(&name, &export::safe_extension(&blob.extension));
                return Carried { file: File::Made { name, bytes }, bitwig: true, why_not: None };
            }
            Some(Err(why)) => Some(why),
            None => Some(no_template(format)),
        }
    };
    Carried { file: File::Made { name, bytes: blob.data }, bitwig: false, why_not }
}

/// Why no Bitwig preset was made of a preset of this format (VST2 or VST3).
fn no_template(format: &str) -> String {
    format!("no preset Bitwig saved for a {format} plugin was found to make a Bitwig preset from")
}

/// A Bitwig preset of `blob`, named as it is, made as an export makes one ([`export::bitwig_bytes`]): from a
/// template saved for its plugin (by `plugin_id`, the id its settings give, when there is one, else by its name), else
/// from one saved for another plugin of its format (whose device becomes its plugin). `None` when there's neither.
fn made(blob: &PresetBlob, plugin_id: &str, templates: &Templates) -> Option<Result<Vec<u8>, String>> {
    export::bitwig_bytes(blob, plugin_id, templates, &mut HashMap::new())
}
