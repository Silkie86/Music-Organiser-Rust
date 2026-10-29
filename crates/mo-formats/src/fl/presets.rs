//! Plugin states in an FL project, as preset files: a VST2's becomes an .fxp (with the plugin's own id), a VST3's a
//! .vstpreset, and FL's own plugins' states are kept raw (.flstate), since they aren't whole .fst files.

use std::collections::BTreeMap;

use super::events::*;
use super::song::class_id_string;
use super::{
    CHANNEL_TYPE_INSTRUMENT, KIND_VST3, Rack, VST_CLASS_ID, VST_FOUR_CC, VST_KIND, VST_NAME, VST_PATH, VST_STATE,
    WRAPPER, is_native_generator, saved_nothing, vst2_header, wrapper_fields, wrapper_text,
};
use crate::model::PluginKind;
use crate::plugins;
use crate::presets::{PresetBlob, fxp, vstpreset};

/// A Fruity Wrapper plugin's state as a file the plugin can load: .fxp for a VST2, .vstpreset for a VST3 (or the raw
/// state, as .vst3state, when it isn't laid out as known).
pub(super) fn wrapper_preset(f: &BTreeMap<u32, &[u8]>, state: &[u8], name: &str, plugin: &str) -> PresetBlob {
    let vst3 = f.get(&VST_KIND).and_then(|k| k.first()) == Some(&KIND_VST3)
        || wrapper_text(f, VST_PATH).to_lowercase().ends_with(".vst3");
    if vst3 {
        if let Some(class_id) = f.get(&VST_CLASS_ID).and_then(|c| <&[u8; 16]>::try_from(*c).ok())
            && let Some((component, controller)) = vst3_state(state)
        {
            let data = vstpreset::build(&class_id_string(class_id), component, controller);
            return PresetBlob::new(name, plugin, ".vstpreset", data);
        }
        return PresetBlob::new(name, plugin, ".vst3state", state.to_vec());
    }

    let id = f.get(&VST_FOUR_CC).and_then(|c| <[u8; 4]>::try_from(*c).ok()).map_or(0, i32::from_le_bytes);
    let mut chunk = state;
    let mut selected = 0;
    if let Some((start, length, current)) = vst2_header(state) {
        selected = current;
        if length == 0
            && state[4] & 0x08 == 0
            && let Some((values, program)) = param_state(state, start, current)
        {
            let shown = if program.is_empty() { plugin } else { &program };
            return PresetBlob {
                program: selected,
                ..PresetBlob::new(name, plugin, ".fxp", fxp::wrap_params(&values, shown, id))
            };
        }
        if start as u64 + length as u64 <= state.len() as u64 {
            chunk = &state[start..start + length as usize];
        }
    }
    PresetBlob { program: selected, ..PresetBlob::new(name, plugin, ".fxp", fxp::wrap_chunk(chunk, plugin, id)) }
}

/// Plugins that don't save a chunk (Predator, older FabFilter) have their parameters after FL's header instead: u32
/// count, the values as floats, then u32 the number of programs and each one's name (25 bytes apart). With more than
/// one program the values are every program's, one after another: Tone2's FireBird (32-bit, in Solomon's "broken
/// homes_29") saves 128 programs of 50, so 6,400 values. This gives the `current` program's values and name, the sound
/// the channel plays; all 6,400 as one program was more than the plugin has, and it came up at its default sound.
fn param_state(state: &[u8], start: usize, current: u32) -> Option<(Vec<f32>, String)> {
    // VST's longest program name, 24 letters, and its end.
    const NAME_SPACING: usize = 25;
    if state.len() < start + 4 {
        return None;
    }
    let n = u32_at(state, start) as usize;
    if n == 0 || start + 4 + 4 * n > state.len() {
        return None;
    }
    let at = start + 4 + 4 * n;
    let programs = if at + 4 <= state.len() { u32_at(state, at) as usize } else { 0 };
    let bank = programs > 1 && n.is_multiple_of(programs) && at + 4 + NAME_SPACING * programs <= state.len();
    let (count, from) = if bank { (n / programs, (current as usize).min(programs - 1)) } else { (n, 0) };
    let values = (0..count)
        .map(|i| {
            let p = start + 4 + 4 * (from * count + i);
            f32::from_le_bytes([state[p], state[p + 1], state[p + 2], state[p + 3]])
        })
        .collect();
    let mut program = String::new();
    if programs >= 1 && at + 4 < state.len() && (bank || programs == 1) {
        let begin = at + 4 + NAME_SPACING * from;
        let len = (if bank { NAME_SPACING } else { 28 }).min(state.len() - begin);
        let text = &state[begin..begin + len];
        let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
        program = text[..end].iter().map(|&b| b as char).collect::<String>().trim().to_owned();
    }
    Some((values, program))
}

/// FL's VST3 state: u32 version, then records of (u32 id, u64 size, data); `None` when one runs past the end.
fn vst3_records(state: &[u8]) -> Option<BTreeMap<u32, &[u8]>> {
    let mut records = BTreeMap::new();
    let mut pos = 4;
    while pos + 12 <= state.len() {
        let id = u32_at(state, pos);
        let size = u64::from_le_bytes(state[pos + 4..pos + 12].try_into().expect("8 bytes"));
        pos += 12;
        if size > (state.len() - pos) as u64 {
            return None;
        }
        records.insert(id, &state[pos..pos + size as usize]);
        pos += size as usize;
    }
    Some(records)
}

/// Record 3 is the component (processor) state, 2 the controller state.
fn vst3_state(state: &[u8]) -> Option<(&[u8], &[u8])> {
    let records = vst3_records(state)?;
    let component = *records.get(&3)?;
    Some((component, records.get(&2).copied().unwrap_or(&[])))
}

/// The plugin's parameter ids in the order FL lists its settings (record 4: u32 count, then the ids), which is how FL's
/// automation names a setting. Seen in FL 26 with Serum 2: setting 205 is id 2000003.
pub(super) fn vst3_parameter_ids(state: &[u8]) -> Option<Vec<u32>> {
    let records = vst3_records(state)?;
    let r = records.get(&4).filter(|r| r.len() >= 4)?;
    let count = u32_at(r, 0) as usize;
    if count == 0 || 4 + 4 * count > r.len() {
        return None;
    }
    Some((0..count).map(|i| u32_at(r, 4 + 4 * i)).collect())
}

/// Every plugin state in the project, in file order. Each knows whether it's an instrument (on a channel) or an effect
/// (in the mixer), and an effect where it sits.
pub fn extract_presets(flp: &Flp<'_>) -> Vec<PresetBlob> {
    let mut presets: Vec<PresetBlob> = Vec::new();
    let mut channel_types: BTreeMap<u32, u32> = BTreeMap::new();
    let mut rack = Rack::default();
    let mut pending_internal = String::new();
    let mut current_name = String::new();
    let mut insert: i64 = -1;
    let mut insert_names: BTreeMap<i64, String> = BTreeMap::new();
    let mut next_insert_name = String::new();
    // Preset index → mixer insert.
    let mut preset_insert: BTreeMap<usize, i64> = BTreeMap::new();
    for e in &flp.events {
        rack.step(e);
        match e.id {
            EV_CHANNEL_TYPE => {
                if let Some(c) = rack.channel() {
                    channel_types.insert(c, e.int());
                }
            }
            EV_INSERT_FLAGS => {
                insert += 1;
                if !next_insert_name.is_empty() {
                    insert_names.insert(insert, std::mem::take(&mut next_insert_name));
                }
            }
            EV_INSERT_NAME => next_insert_name = flp.text(&e.data),
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                current_name.clear();
            }
            EV_PLUGIN_NAME => current_name = flp.text(&e.data),
            EV_PLUGIN_STATE if !e.data.is_empty() => {
                let on_channel = rack.channel().filter(|c| channel_types.get(c) == Some(&CHANNEL_TYPE_INSTRUMENT));
                if rack.in_mixer() && insert >= 0 {
                    preset_insert.insert(presets.len(), insert);
                }
                let kind = |instrument: bool| if instrument { PluginKind::Instrument } else { PluginKind::Effect };
                if pending_internal.eq_ignore_ascii_case(WRAPPER) {
                    let f = wrapper_fields(&e.data);
                    let plugin = match wrapper_text(&f, VST_NAME) {
                        n if n.is_empty() => plugins::name_from_path(&wrapper_text(&f, VST_PATH)),
                        n => n,
                    };
                    let name = if current_name.is_empty() { plugin.clone() } else { current_name.clone() };
                    if let Some(state) = f.get(&VST_STATE).filter(|s| !s.is_empty() && !saved_nothing(s)) {
                        presets.push(PresetBlob {
                            kind: kind(on_channel.is_some()),
                            channel: on_channel,
                            ..wrapper_preset(&f, state, &name, &plugin)
                        });
                    }
                } else if !pending_internal.is_empty() {
                    let name = if current_name.is_empty() { &pending_internal } else { &current_name };
                    presets.push(PresetBlob {
                        kind: kind(on_channel.is_some() || is_native_generator(&pending_internal)),
                        native: true,
                        channel: on_channel,
                        ..PresetBlob::new(name, &pending_internal, ".flstate", e.data.to_vec())
                    });
                }
                pending_internal.clear();
            }
            _ => {}
        }
    }
    for (i, ins) in preset_insert {
        if let Some(p) = presets.get_mut(i) {
            p.place = if ins == 0 { "Master".into() } else { format!("Insert {ins}") };
            if let Some(n) = insert_names.get(&ins).filter(|n| !n.is_empty()) {
                p.place += &format!(" · {n}");
            }
        }
    }
    presets
}
