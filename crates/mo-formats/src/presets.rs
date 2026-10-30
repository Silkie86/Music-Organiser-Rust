//! Plugin presets as files a plugin (or another DAW) can load: VST2 `.fxp` programs and `.fxb` banks, and VST3
//! `.vstpreset` files.

use std::borrow::Cow;

use crate::model::PluginKind;

/// A plugin's saved settings, taken out of a project.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetBlob {
    /// What the project called it: the channel's or slot's name, else the plugin's.
    pub name: String,
    pub plugin: String,
    /// ".fxp", ".vstpreset", ".flstate" (FL's own plugins), or ".vst3state" when a VST3's state isn't laid out as known;
    /// Bitwig's are whatever its file in the project is called (".vstpreset", ".fxb"...).
    pub extension: Cow<'static, str>,
    pub data: Vec<u8>,
    /// Instrument or effect, where the project says.
    pub kind: PluginKind,
    /// One of the DAW's own plugins rather than a third-party one.
    pub native: bool,
    /// Where it sits when that tells copies apart: "Insert 3 · Vocals" or "Master".
    pub place: String,
    /// FL Studio: the channel an instrument is on (its place in the channel rack, from 0).
    pub channel: Option<u32>,
    /// A VST2's program that was selected when it was saved, which the host selects again after loading it.
    pub program: u32,
}

impl PresetBlob {
    pub fn new(name: &str, plugin: &str, extension: impl Into<Cow<'static, str>>, data: Vec<u8>) -> PresetBlob {
        PresetBlob {
            name: name.to_owned(),
            plugin: plugin.to_owned(),
            extension: extension.into(),
            data,
            kind: PluginKind::Unknown,
            native: false,
            place: String::new(),
            channel: None,
            program: 0,
        }
    }
}

/// VST2 program and bank files: a "CcnK" header, then the program's chunk ("FPCh") or its parameters ("FxCk").
pub mod fxp {
    /// A program's name field: 28 bytes, at most 27 letters, then zeros.
    fn program_name(name: &str) -> [u8; 28] {
        let mut out = [0u8; 28];
        // Only plain ASCII fits the field; anything else becomes '?', as .NET's ASCII encoding writes it.
        for (o, c) in out.iter_mut().zip(name.chars().take(27)) {
            *o = if c.is_ascii() { c as u8 } else { b'?' };
        }
        out
    }

    /// The plugin's own chunk in an "FPCh" .fxp program. `plugin_id` is its four letters as a number, so hosts can
    /// match the preset to the plugin.
    pub fn wrap_chunk(chunk: &[u8], program: &str, plugin_id: i32) -> Vec<u8> {
        let mut out = Vec::with_capacity(60 + chunk.len());
        out.extend(b"CcnK");
        out.extend(((4 + 16 + 28 + 4 + chunk.len()) as i32).to_be_bytes());
        out.extend(b"FPCh");
        out.extend(1i32.to_be_bytes()); // format version
        out.extend(plugin_id.to_be_bytes());
        out.extend(1i32.to_be_bytes()); // plugin version
        out.extend(1i32.to_be_bytes()); // number of programs
        out.extend(program_name(program));
        out.extend((chunk.len() as i32).to_be_bytes());
        out.extend(chunk);
        out
    }

    /// An "FxCk" (parameter list) .fxp, for plugins that save their parameters rather than a chunk. The values are
    /// written big-endian, as the format wants.
    pub fn wrap_params(values: &[f32], program: &str, plugin_id: i32) -> Vec<u8> {
        let mut out = Vec::with_capacity(56 + 4 * values.len());
        out.extend(b"CcnK");
        out.extend(((4 + 16 + 28 + 4 * values.len()) as i32).to_be_bytes());
        out.extend(b"FxCk");
        out.extend(1i32.to_be_bytes());
        out.extend(plugin_id.to_be_bytes());
        out.extend(1i32.to_be_bytes());
        out.extend((values.len() as i32).to_be_bytes());
        out.extend(program_name(program));
        out.extend(values.iter().flat_map(|v| v.to_be_bytes()));
        out
    }
}

/// VST3 `.vstpreset` files: "VST3", version 1, the class id, where the chunk list is; then the component state
/// ("Comp") and the controller state ("Cont"); then the chunk list.
pub mod vstpreset {
    const HEADER: usize = 4 + 4 + 32 + 8;

    /// A .vstpreset around a plugin's states. `class_id` is written as a .vstpreset writes it (32 hex digits).
    pub fn build(class_id: &str, component: &[u8], controller: &[u8]) -> Vec<u8> {
        let list = HEADER + component.len() + controller.len();
        let mut out = Vec::with_capacity(list + 8 + 2 * 20);
        out.extend(b"VST3");
        out.extend(1i32.to_le_bytes());
        out.extend(class_id.as_bytes());
        out.extend((list as i64).to_le_bytes());
        out.extend(component);
        out.extend(controller);
        out.extend(b"List");
        // Bitwig lists the controller even when it's empty.
        out.extend(2i32.to_le_bytes());
        for (id, offset, size) in
            [(b"Comp", HEADER, component.len()), (b"Cont", HEADER + component.len(), controller.len())]
        {
            out.extend(id);
            out.extend((offset as i64).to_le_bytes());
            out.extend((size as i64).to_le_bytes());
        }
        out
    }
}
