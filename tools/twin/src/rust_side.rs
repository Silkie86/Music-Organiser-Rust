//! What the Rust readers find in a file, as the twin test's JSON. The C# helper (`cs/Program.cs`) writes the same
//! shape from the C# readers, so the two can be compared field by field.

use std::path::Path;

use mo_formats::bitwig;
use mo_formats::fl::automation::{FlAutomation, FlPoint};
use mo_formats::fl::song::{self, FlChannel, FlInsert, FlPlugin, FlSong};
use mo_formats::fl::{self, events, presets};
use mo_formats::midi::{MidiPart, smf};
use mo_formats::presets::PresetBlob;
use mo_formats::studio_one;
use mo_formats::{MidiChannel, MidiClip, NoteEvent, ProjectInfo};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Everything the Rust readers find in one file. A file that can't be read gives `{"error": ...}`.
pub fn dump(path: &Path) -> Value {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) => return json!({ "file": path.to_string_lossy(), "error": e.to_string() }),
    };
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bwproject")) {
        return bitwig_dump(path, raw);
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("song")) {
        return studio_one_dump(path, &raw);
    }
    if smf::is_midi_path(&path.to_string_lossy()) {
        return midi_dump(path, &raw);
    }
    // Read from disk once; the project list's reading takes the same bytes.
    let mut out = json!({ "file": path.to_string_lossy(), "info": info_json(&fl::read_from(&raw, path)) });
    // The C# readers throw on a damaged file where the project list only warns; so does this.
    let Ok(flp) = events::load(&raw) else {
        out["error_more"] = json!("damaged");
        return out;
    };
    let mut channels = song::extract_channels(&flp);
    song::resolve_channel_samples(&mut channels, path);
    let mut read = song::read_song(&flp);
    read.resolve_samples(path);
    out["midi"] = song::extract_midi(&flp).iter().map(clip_json).collect();
    out["channels"] = channels.iter().map(channel_json).collect();
    out["presets"] = presets::extract_presets(&flp).iter().map(preset_json).collect();
    out["song"] = song_json(&read);
    out
}

/// A Bitwig project: what the project list shows, its note clips and its plugin states. A file that isn't Bitwig's
/// has none of either, as in the C# reader.
fn bitwig_dump(path: &Path, raw: Vec<u8>) -> Value {
    let known = &mo_formats::plugins::is_always_known;
    // Decoded once for all three.
    let (info, midi, presets) = match bitwig::Project::open(raw) {
        Ok(p) => (bitwig::read_project(Some(&p), &p.doc.raw, path, known), p.midi(), p.presets()),
        Err(raw) => (bitwig::read_project(None, &raw, path, known), Vec::new(), Vec::new()),
    };
    json!({
        "file": path.to_string_lossy(),
        "info": info_json(&info),
        "midi": midi.iter().map(clip_json).collect::<Vec<_>>(),
        "presets": presets.iter().map(preset_json).collect::<Vec<_>>(),
    })
}

/// A Studio One song: what the project list shows, its note clips and its plugin states. A file that isn't a zip
/// can't be read; the two apps word that differently, so both say just "unreadable".
fn studio_one_dump(path: &Path, raw: &[u8]) -> Value {
    let mut out = json!({ "file": path.to_string_lossy() });
    match studio_one::Song::open(raw) {
        Ok(mut song) => {
            out["info"] = info_json(&song.info());
            out["midi"] = song.midi().iter().map(clip_json).collect();
            out["presets"] = song.presets().iter().map(preset_json).collect();
        }
        Err(_) => {
            out["info"] = json!({ "error": "unreadable" });
            out["error_more"] = json!("unreadable");
        }
    }
    out
}

/// A MIDI file's parts. A file that isn't one says just "unreadable" in both apps.
fn midi_dump(path: &Path, raw: &[u8]) -> Value {
    let stem = path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    match smf::read_bytes(raw, &stem) {
        Ok(parts) => {
            json!({ "file": path.to_string_lossy(), "parts": parts.iter().map(part_json).collect::<Vec<_>>() })
        }
        Err(_) => json!({ "file": path.to_string_lossy(), "error": "unreadable" }),
    }
}

fn part_json(p: &MidiPart) -> Value {
    json!({
        "name": p.name, "ppq": p.ppq, "notes": notes_json(&p.notes), "drums": p.drums, "tempo": p.tempo,
        "program": p.program, "track_name": p.track_name,
    })
}

fn info_json(i: &ProjectInfo) -> Value {
    json!({
        "daw": i.daw,
        "confidence": format!("{:?}", i.confidence),
        "tempo": i.tempo,
        "version": i.version,
        "title": i.title,
        "plugins": i.plugins.iter().map(|p| json!({
            "name": p.name, "vendor": p.vendor, "kind": format!("{:?}", p.kind), "format": p.format,
            "path": p.path, "id": p.id,
        })).collect::<Vec<_>>(),
        "samples": i.samples,
        "tracks": i.tracks,
        "hardware": i.hardware,
        "details": i.details,
        "created": i.created.map(|c| c.format("%Y-%m-%dT%H:%M:%S%.3f").to_string()),
        "warnings": i.warnings,
        "can_extract_midi": i.can_extract_midi,
        "can_extract_presets": i.can_extract_presets,
        "unplayed": i.unplayed.iter().map(|(s, u)| (s.clone(), json!(format!("{u:?}")))).collect::<serde_json::Map<_, _>>(),
    })
}

/// A note as [start, length, key, velocity, muted, step]: short, as a song can have thousands.
fn note_json(n: &NoteEvent) -> Value {
    // MIDI 1.0's velocity, as the C# app has only that.
    json!([n.start, n.length, n.key, n.velocity7(), n.muted, n.step])
}

fn notes_json(notes: &[NoteEvent]) -> Value {
    notes.iter().map(note_json).collect()
}

fn clip_json(c: &MidiClip) -> Value {
    json!({
        "name": c.name,
        "ppq": c.ppq,
        "end": c.end,
        "tracks": c.tracks.iter().map(|(name, notes)| json!({ "name": name, "notes": notes_json(notes) })).collect::<Vec<_>>(),
    })
}

fn channel_json(c: &MidiChannel) -> Value {
    json!({
        "name": c.name, "source": c.source, "ppq": c.ppq, "plugin": c.plugin, "sample_path": c.sample_path,
        "root_key": c.root_key, "pitch_cents": c.pitch_cents, "channel": c.channel, "notes": notes_json(&c.notes),
    })
}

/// A preset's bytes go in as their size and SHA-256, not the bytes themselves.
fn preset_json(p: &PresetBlob) -> Value {
    json!({
        "name": p.name, "plugin": p.plugin, "extension": p.extension, "kind": format!("{:?}", p.kind),
        "native": p.native, "place": p.place, "channel": p.channel, "program": p.program,
        "size": p.data.len(), "sha256": sha256(&p.data),
    })
}

fn sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn plugin_json(p: &FlPlugin) -> Value {
    json!({
        "name": p.name, "vendor": p.vendor, "format": p.format, "device_id": p.device_id, "path": p.path,
        "nothing_saved": p.nothing_saved, "slot": p.slot, "enabled": p.enabled,
        "state": p.state.as_ref().map(|s| json!({ "extension": s.extension, "size": s.data.len(), "sha256": sha256(&s.data) })),
        "parameter_ids": p.parameter_ids,
    })
}

fn points_json(points: &[FlPoint]) -> Value {
    points.iter().map(|p| json!([p.beat, p.value])).collect()
}

fn song_channel_json(c: &FlChannel) -> Value {
    json!({
        "index": c.index, "name": c.name, "instrument": c.instrument.as_ref().map(plugin_json), "sample": c.sample,
        "insert": c.insert, "notes": notes_json(&c.notes), "root_key": c.root_key, "pitch_cents": c.pitch_cents,
        "volume": c.volume, "pan": c.pan,
        "clips": c.clips.iter().map(|k| json!({
            "name": k.name, "start": k.start, "length": k.length, "notes": notes_json(&k.notes), "muted": k.muted,
            "track": k.track, "on_muted_track": k.on_muted_track,
        })).collect::<Vec<_>>(),
        "volume_envelope": c.volume_envelope.map(|e| json!({
            "delay": e.delay, "attack": e.attack, "hold": e.hold, "decay": e.decay, "sustain": e.sustain,
            "release": e.release,
        })),
        "audio_clips": c.audio_clips.iter().map(|a| json!({
            "start": a.start, "length": a.length, "from": a.from, "muted": a.muted, "track": a.track,
            "on_muted_track": a.on_muted_track, "partly_off": a.partly_off,
        })).collect::<Vec<_>>(),
        "stretch": {
            "multiplier": c.stretch.multiplier, "pitch_cents": c.stretch.pitch_cents, "mode": c.stretch.mode,
            "time": c.stretch.time, "reversed": c.stretch.reversed,
        },
        "enabled": c.enabled,
        "switched_by": c.switched_by,
    })
}

fn insert_json(i: &FlInsert) -> Value {
    json!({
        "index": i.index, "name": i.name, "effects": i.effects.iter().map(plugin_json).collect::<Vec<_>>(),
        "fader": i.fader, "pan": i.pan,
    })
}

fn automation_json(a: &FlAutomation) -> Value {
    json!({
        "name": a.name, "channel": a.channel, "control": format!("{:?}", a.control), "parameter": a.parameter,
        "points": points_json(&a.points), "insert": a.insert, "slot": a.slot, "starts": a.starts,
    })
}

fn song_json(s: &FlSong) -> Value {
    json!({
        "title": s.title, "ppq": s.ppq, "tempo": s.tempo,
        "channels": s.channels.iter().map(song_channel_json).collect::<Vec<_>>(),
        "inserts": s.inserts.iter().map(insert_json).collect::<Vec<_>>(),
        "automation": s.automation.iter().map(automation_json).collect::<Vec<_>>(),
        "muted_tracks": s.muted_tracks.iter().map(|t| json!({ "number": t.number, "name": t.name })).collect::<Vec<_>>(),
        "curves": s.curves.iter().map(|(c, p)| (c.to_string(), points_json(p))).collect::<serde_json::Map<_, _>>(),
    })
}
