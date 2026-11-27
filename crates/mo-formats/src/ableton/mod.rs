//! What's in an Ableton Live set (`.als`): tracks, plugins, samples, tempo, time signature, notes and the arrangement.
//!
//! A Live set is one XML file, gzipped (very old sets and some tools' are left plain). Its root is `<Ableton>`, whose
//! `Creator` names the Live that saved it ("Ableton Live 12.1.5"), with a `<LiveSet>` inside:
//!
//! - `Tracks` holds `MidiTrack`, `AudioTrack`, `GroupTrack` and `ReturnTrack` elements, in the order Live shows them.
//!   A track's name is `Name/EffectiveName` (or `Name/UserName` when that's set); `TrackGroupId` is the `Id` of the
//!   group it's in (-1 for none).
//! - `MasterTrack` (`MainTrack` from Live 12) carries the tempo, `DeviceChain/Mixer/Tempo/Manual`, and the time
//!   signature, `DeviceChain/Mixer/TimeSignature/Manual`, one number: the top minus one, plus 99 times the bottom's
//!   power of two (201 is 4/4, 302 is 6/8).
//! - Each track's `DeviceChain/MainSequencer` holds its clips: the arrangement's under `ClipTimeable/ArrangerAutomation/
//!   Events` (a MIDI track) or `Sample/ArrangerAutomation/Events` (an audio track), the session's in `ClipSlotList`.
//!   `FreezeSequencer` holds a frozen track's audio, which Live makes itself: it's left out.
//! - A clip's `Time` (an arrangement clip) and `CurrentStart`/`CurrentEnd` are in beats. Its content has its own
//!   beats: `Loop/LoopStart` to `LoopEnd`, played from `LoopStart` (plus `StartRelative` when `LoopOn`), going back
//!   to `LoopStart` at `LoopEnd` when it loops. `Disabled` is a muted clip.
//! - A MIDI clip's notes are in `Notes/KeyTracks/KeyTrack`, each with its key (`MidiKey`) and `MidiNoteEvent`s timed in
//!   the clip's beats, with a velocity from 0 to 127 (a fraction from Live 11) and `IsEnabled` false when muted.
//! - An audio clip's file is `SampleRef/FileRef`: `Path` in full (Live 11 on), `RelativePath` from the project folder,
//!   or, before Live 11, `RelativePath` and `SearchHint/PathHint` as lists of folders with the file's `Name`.
//! - Devices are in `DeviceChain/DeviceChain/Devices`, each by its own element: `PluginDevice` (a VST2 or a VST3, by
//!   `PluginDesc/VstPluginInfo` or `Vst3PluginInfo`), `AuPluginDevice` (an Audio Unit), Max for Live devices
//!   (`MxDevice...`), and Live's own (`OriginalSimpler`, `Eq8`, `Compressor2`...). Racks hold more devices inside.
//!
//! Nothing here was taken from another program's code: these are facts about the format, to be checked on real sets.

use std::io::Read;
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

use crate::model::{Confidence, MidiClip, NoteAttribute, NoteEvent, PluginKind, PluginRef, ProjectInfo, velocity};
use crate::{paths, plugins, textscan};

pub mod arrangement;
pub mod write;

pub const DAW: &str = "Ableton Live";
/// Goes up by one whenever the reader finds more (or different) things in the same files (rule 7).
pub const VERSION: u32 = 1;
pub const EXTENSIONS: [&str; 1] = ["als"];

/// Live times notes in beats; the MIDI made from them has this many ticks a beat.
pub const MIDI_PPQ: u16 = 960;

/// The most XML a set may unpack to: far past any real set's, so a damaged gzip can't fill the memory.
const MOST_XML: u64 = crate::zipped::MOST;

/// A Live set opened: its XML as text, and the folder it's in (for samples kept by a path from the project folder).
pub struct LiveSet {
    text: String,
    folder: Option<PathBuf>,
}

/// Reads what's in a Live set. A file that's neither gzipped XML nor XML can't be read and says so.
pub fn read(path: &Path) -> std::io::Result<ProjectInfo> {
    let raw = std::fs::read(path)?;
    Ok(LiveSet::open(&raw)?.in_folder(path.parent()).info())
}

impl LiveSet {
    /// The file's bytes: gzipped XML, or plain XML.
    pub fn open(raw: &[u8]) -> std::io::Result<LiveSet> {
        let bytes = if raw.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(raw).take(MOST_XML + 1).read_to_end(&mut out)?;
            if out.len() as u64 > MOST_XML {
                return Err(std::io::Error::other("this set unpacks to more than any Live set holds"));
            }
            out
        } else {
            raw.to_vec()
        };
        let text = String::from_utf8(bytes).map_err(|_| std::io::Error::other("this isn't a Live set's text"))?;
        if !text.trim_start_matches('\u{feff}').trim_start().starts_with('<') || !text.contains("<Ableton") {
            return Err(std::io::Error::other("this isn't an Ableton Live set"));
        }
        Ok(LiveSet { text, folder: None })
    }

    /// The set, knowing the folder it's in.
    pub fn in_folder(mut self, folder: Option<&Path>) -> LiveSet {
        self.folder = folder.map(Path::to_path_buf);
        self
    }

    /// The set's XML.
    pub fn xml(&self) -> &str {
        &self.text
    }

    fn doc(&self) -> Result<Document<'_>, String> {
        let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() };
        Document::parse_with_options(&self.text, options).map_err(|e| e.to_string())
    }

    /// What the project list shows.
    pub fn info(&self) -> ProjectInfo {
        let mut info = ProjectInfo::new(DAW);
        info.confidence = Confidence::Partial;
        info.can_extract_midi = true;
        let doc = match self.doc() {
            Ok(doc) => doc,
            Err(e) => {
                info.warnings.push(format!("The set's XML couldn't be read: {e}."));
                return info;
            }
        };
        let root = doc.root_element();
        info.version = root.attribute("Creator").unwrap_or_default().trim().to_owned();
        let Some(set) = child(root, "LiveSet") else {
            info.warnings.push("The set has no LiveSet in it.".into());
            return info;
        };
        info.tempo = tempo(set);
        let tracks = tracks(set);
        let mut notes = 0usize;
        let mut clips = 0usize;
        for track in &tracks {
            info.tracks.push(track_name(*track));
            devices(*track, &mut info.plugins);
            for clip in track_clips(*track) {
                clips += 1;
                notes += clip_notes(clip).len();
            }
        }
        if let Some(master) = master(set) {
            devices(master, &mut info.plugins);
        }
        // Every audio file the set points at, wherever: clips, Simplers, Samplers, Drum Racks. Not a frozen track's
        // audio, which Live made itself and keeps in the project.
        for e in file_refs_not_frozen(set) {
            let path = self.file_ref_path(e);
            if !path.is_empty() && textscan::is_audio_path(&path) {
                info.samples.push(path);
            }
        }
        if let Some(signature) = signature(set) {
            info.details.insert("Time signature".into(), format!("{}/{}", signature.top(), signature.bottom()));
        }
        info.details.insert("Clips".into(), clips.to_string());
        info.details.insert("Notes".into(), notes.to_string());
        let scenes = scenes(set).len();
        if scenes > 0 {
            info.details.insert("Scenes".into(), scenes.to_string());
        }
        info.dedupe()
    }

    /// The notes of each MIDI clip, arrangement and session, by the track it's on, in the clip's own timeline from
    /// its start: [`MIDI_PPQ`] ticks a beat.
    /// A set whose XML won't read says so, rather than giving no clips.
    pub fn midi(&self) -> std::io::Result<Vec<MidiClip>> {
        let doc = self.doc().map_err(|e| std::io::Error::other(format!("the set's XML is damaged: {e}")))?;
        let Some(set) = child(doc.root_element(), "LiveSet") else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for track in tracks(set) {
            let name = track_name(track);
            for clip in track_clips(track).filter(|c| c.has_tag_name("MidiClip")) {
                let content = Content::of(clip);
                // A looping clip's notes before its start marker play from the second pass on: they're its own too.
                let first = content.first();
                let mut events: Vec<NoteEvent> = clip_notes(clip)
                    .into_iter()
                    .filter(|n| n.time >= first - 1e-9 && n.time < content.end)
                    .map(|n| n.event(first, f64::from(MIDI_PPQ)))
                    .collect();
                if events.is_empty() {
                    continue;
                }
                events.sort_by_key(|n| (n.start, n.key));
                let clip_name = value(clip, &["Name"]).unwrap_or_default();
                let mut midi = MidiClip {
                    name: if clip_name.trim().is_empty() { name.clone() } else { clip_name.to_owned() },
                    ppq: MIDI_PPQ,
                    tracks: Vec::new(),
                    end: Some(ticks(content.end - first, f64::from(MIDI_PPQ))),
                };
                *midi.track(&name) = events;
                out.push(midi);
            }
        }
        Ok(out)
    }

    /// An audio file's path as the `FileRef` gives it: in full when it says (Windows paths with Windows' slashes),
    /// else from the project folder, else (sets before Live 11) from its folder hint and name.
    fn file_ref_path(&self, file_ref: Node<'_, '_>) -> String {
        let full = value(file_ref, &["Path"]).unwrap_or_default().trim();
        if !full.is_empty() {
            return native(full);
        }
        let name = value(file_ref, &["Name"]).unwrap_or_default().trim();
        // Live 11 on: the path from the project folder as text. Before: a list of folders, with the name apart.
        let relative = match child(file_ref, "RelativePath") {
            Some(r) => match r.attribute("Value") {
                Some(text) => text.trim().to_owned(),
                None => joined_dirs(r, name).trim_start_matches('/').to_owned(),
            },
            None => String::new(),
        };
        let hint = child(file_ref, "SearchHint").and_then(|h| child(h, "PathHint")).map(|h| joined_dirs(h, name));
        if let Some(hint) = hint.filter(|h| paths::is_absolute(h)) {
            return native(&hint);
        }
        if let Some(data) = value_text(file_ref, "Data").and_then(windows_path_in_data) {
            return data;
        }
        if relative.is_empty() {
            return String::new();
        }
        match &self.folder {
            Some(folder) => paths::full_path(folder, &native(&relative)),
            None => native(&relative),
        }
    }
}

/// A Windows path from Live with Windows' slashes ("C:/Samples/Kick.wav" is "C:\Samples\Kick.wav"); a Mac's as it is.
fn native(path: &str) -> String {
    let bytes = path.as_bytes();
    let windows =
        (bytes.len() >= 2 && bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':'))
            || path.starts_with("//");
    if windows { path.replace('/', "\\") } else { path.to_owned() }
}

/// The folders a `RelativePath` or `PathHint` lists, one `RelativePathElement` each, with the file's name after them.
fn joined_dirs(list: Node<'_, '_>, name: &str) -> String {
    let dirs: Vec<&str> =
        list.children().filter(|e| e.has_tag_name("RelativePathElement")).filter_map(|e| e.attribute("Dir")).collect();
    if dirs.is_empty() {
        return String::new();
    }
    let mut out = dirs.join("/");
    if !name.is_empty() {
        out.push('/');
        out.push_str(name);
    }
    // A drive is its first folder on Windows ("C:"); on a Mac the list starts from the top.
    if dirs.first().is_some_and(|d| d.len() == 2 && d.ends_with(':')) { out } else { format!("/{out}") }
}

/// Before Live 11, a Windows set keeps each file's path in `Data` as hex: UTF-16 letters ending in a zero. A Mac's
/// `Data` is an alias record, which isn't read.
fn windows_path_in_data(hex: &str) -> Option<String> {
    let digits: Vec<u8> = hex.bytes().filter(u8::is_ascii_hexdigit).collect();
    let bytes: Vec<u8> = digits
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|pair| std::str::from_utf8(pair).ok().and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect();
    let units: Vec<u16> =
        bytes.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).take_while(|&u| u != 0).collect();
    let path = String::from_utf16(&units).ok()?;
    let b = path.as_bytes();
    (b.len() > 3 && b.get(1) == Some(&b':') && matches!(b.get(2), Some(b'\\' | b'/'))).then(|| native(&path))
}

/// The element's first child with this name.
pub(crate) fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(name))
}

/// The element down this path of children.
pub(crate) fn at<'a, 'i>(node: Node<'a, 'i>, path: &[&str]) -> Option<Node<'a, 'i>> {
    path.iter().try_fold(node, |n, name| child(n, name))
}

/// The `Value` of the element down this path of children.
pub(crate) fn value<'a>(node: Node<'a, '_>, path: &[&str]) -> Option<&'a str> {
    at(node, path)?.attribute("Value")
}

/// A child's text (Live keeps a few things, like `Data`, as text rather than a `Value`).
fn value_text<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name)?.text()
}

pub(crate) fn number(node: Node<'_, '_>, path: &[&str]) -> Option<f64> {
    value(node, path)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

pub(crate) fn flag(node: Node<'_, '_>, path: &[&str]) -> bool {
    value(node, path).is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

/// The set's master track: `MainTrack` from Live 12, `MasterTrack` before.
pub(crate) fn master<'a, 'i>(set: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    child(set, "MainTrack").or_else(|| child(set, "MasterTrack"))
}

/// The set's tempo, in beats a minute.
pub(crate) fn tempo(set: Node<'_, '_>) -> Option<f64> {
    master(set).and_then(|m| number(m, &["DeviceChain", "Mixer", "Tempo", "Manual"])).filter(|t| *t > 0.0)
}

/// The set's time signature, from Live's one number for it.
pub(crate) fn signature(set: Node<'_, '_>) -> Option<crate::arrangement::TimeSignature> {
    let code = master(set).and_then(|m| number(m, &["DeviceChain", "Mixer", "TimeSignature", "Manual"]))?;
    signature_of(code as i64)
}

/// Live's number for a time signature: the top minus one, plus 99 times the bottom's power of two.
pub fn signature_of(code: i64) -> Option<crate::arrangement::TimeSignature> {
    if !(0..99 * 6).contains(&code) {
        return None;
    }
    crate::arrangement::TimeSignature::new((code % 99 + 1) as u32, 1 << (code / 99))
}

/// Live's number for a time signature ([`signature_of`] backwards).
pub fn signature_code(signature: crate::arrangement::TimeSignature) -> i64 {
    i64::from(signature.top()) - 1 + 99 * i64::from(signature.bottom().trailing_zeros())
}

/// The set's scenes: `Scenes` from Live 11, `SceneNames` before.
pub(crate) fn scenes<'a, 'i>(set: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    child(set, "Scenes")
        .or_else(|| child(set, "SceneNames"))
        .map(|s| s.children().filter(Node::is_element).collect())
        .unwrap_or_default()
}

/// The kinds of track Live has, as elements.
pub(crate) const TRACK_TAGS: [&str; 4] = ["MidiTrack", "AudioTrack", "GroupTrack", "ReturnTrack"];

/// The set's tracks, in the order Live shows them.
pub(crate) fn tracks<'a, 'i>(set: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    child(set, "Tracks")
        .map(|t| t.children().filter(|c| TRACK_TAGS.iter().any(|tag| c.has_tag_name(*tag))).collect())
        .unwrap_or_default()
}

/// A track's name as Live shows it: the one you gave it, else the one Live made up.
pub(crate) fn track_name(track: Node<'_, '_>) -> String {
    let user = value(track, &["Name", "UserName"]).unwrap_or_default().trim();
    let effective = value(track, &["Name", "EffectiveName"]).unwrap_or_default().trim();
    if !user.is_empty() { user.to_owned() } else { effective.to_owned() }
}

/// A track's clips: the arrangement's, then the session's, never a frozen track's audio.
pub(crate) fn track_clips<'a, 'i>(track: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    let sequencer = at(track, &["DeviceChain", "MainSequencer"]);
    let arranged = sequencer.into_iter().flat_map(arrangement_clips);
    let session = sequencer.into_iter().flat_map(session_clips).map(|(_, clip)| clip);
    arranged.chain(session)
}

/// The clips on a sequencer's arrangement timeline.
pub(crate) fn arrangement_clips<'a, 'i>(sequencer: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let events = |holder: &str| at(sequencer, &[holder, "ArrangerAutomation", "Events"]);
    events("ClipTimeable")
        .into_iter()
        .chain(events("Sample"))
        .flat_map(|e| e.children())
        .filter(|c| c.has_tag_name("MidiClip") || c.has_tag_name("AudioClip"))
        .collect()
}

/// The clips in a sequencer's session slots, with each slot's place (its scene).
pub(crate) fn session_clips<'a, 'i>(sequencer: Node<'a, 'i>) -> Vec<(usize, Node<'a, 'i>)> {
    let Some(list) = child(sequencer, "ClipSlotList") else { return Vec::new() };
    list.children()
        .filter(|s| s.has_tag_name("ClipSlot"))
        .enumerate()
        .filter_map(|(i, slot)| {
            let clip = at(slot, &["ClipSlot", "Value"])?
                .children()
                .find(|c| c.has_tag_name("MidiClip") || c.has_tag_name("AudioClip"))?;
            Some((i, clip))
        })
        .collect()
}

/// What of its own timeline a clip plays: from `start` to `end`, going back to `loop_start` at `end` when it loops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Content {
    pub loop_start: f64,
    pub start: f64,
    pub end: f64,
    pub looping: bool,
}

impl Content {
    /// Where the clip's notes begin: the start marker, or the loop's start when that's earlier (later passes play it).
    pub fn first(&self) -> f64 {
        if self.looping { self.start.min(self.loop_start) } else { self.start }
    }

    /// The same times, multiplied (an unwarped audio clip's seconds in beats).
    pub fn scaled(&self, by: f64) -> Content {
        Content { loop_start: self.loop_start * by, start: self.start * by, end: self.end * by, looping: self.looping }
    }

    pub fn of(clip: Node<'_, '_>) -> Content {
        let loop_start = number(clip, &["Loop", "LoopStart"]).unwrap_or(0.0);
        let looping = flag(clip, &["Loop", "LoopOn"]);
        let end = number(clip, &["Loop", "LoopEnd"]).unwrap_or(loop_start).max(loop_start);
        let start =
            if looping { loop_start + number(clip, &["Loop", "StartRelative"]).unwrap_or(0.0) } else { loop_start };
        // A start marker before the loop is saved as a negative StartRelative: the first pass plays from there.
        Content { loop_start, start: start.min(end), end, looping }
    }
}

/// A note as the clip keeps it, in the clip's own beats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClipNote {
    pub time: f64,
    pub length: f64,
    pub key: u8,
    /// 0 to 127, with fractions from Live 11.
    pub velocity: f64,
    pub enabled: bool,
}

impl ClipNote {
    /// The note in ticks from `from` (a beat of the clip's own).
    fn event(&self, from: f64, ppq: f64) -> NoteEvent {
        NoteEvent {
            start: ticks(self.time - from, ppq),
            length: ticks(self.length, ppq).max(1),
            key: self.key,
            velocity: note_velocity(self.velocity),
            muted: !self.enabled,
            step: false,
            attribute: NoteAttribute::NONE,
        }
    }
}

/// Live's velocity (0 to 127, with fractions) in MIDI 2.0's 16 bits.
pub(crate) fn note_velocity(v: f64) -> u16 {
    if v.is_finite() { velocity::from_unit((v / 127.0).clamp(0.0, 1.0)) } else { velocity::from_7(100) }
}

pub(crate) fn ticks(beats: f64, ppq: f64) -> i64 {
    let t = (beats * ppq).round();
    if t.is_finite() { t.clamp(i64::MIN as f64, i64::MAX as f64) as i64 } else { 0 }
}

/// A MIDI clip's notes, by key, in its own beats.
pub(crate) fn clip_notes(clip: Node<'_, '_>) -> Vec<ClipNote> {
    let Some(key_tracks) = at(clip, &["Notes", "KeyTracks"]) else { return Vec::new() };
    let mut out = Vec::new();
    for key_track in key_tracks.children().filter(|k| k.has_tag_name("KeyTrack")) {
        let Some(key) = number(key_track, &["MidiKey"]).filter(|k| (0.0..=127.0).contains(k)) else { continue };
        let Some(notes) = child(key_track, "Notes") else { continue };
        for n in notes.children().filter(|n| n.has_tag_name("MidiNoteEvent")) {
            let attr =
                |name: &str| n.attribute(name).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
            let (Some(time), Some(length)) = (attr("Time"), attr("Duration")) else { continue };
            out.push(ClipNote {
                time,
                length: length.max(0.0),
                key: key as u8,
                velocity: attr("Velocity").unwrap_or(100.0),
                enabled: !n.attribute("IsEnabled").is_some_and(|v| v.eq_ignore_ascii_case("false")),
            });
        }
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.key.cmp(&b.key)));
    out
}

/// Live's own instruments, by element: what isn't one of these or a MIDI effect is an audio effect.
const INSTRUMENTS: [(&str, &str); 16] = [
    ("OriginalSimpler", "Simpler"),
    ("MultiSampler", "Sampler"),
    ("Operator", "Operator"),
    ("UltraAnalog", "Analog"),
    ("InstrumentVector", "Wavetable"),
    ("Collision", "Collision"),
    ("LoungeLizard", "Electric"),
    ("StringStudio", "Tension"),
    ("InstrumentImpulse", "Impulse"),
    ("Drift", "Drift"),
    ("InstrumentMeld", "Meld"),
    ("DrumGroupDevice", "Drum Rack"),
    ("InstrumentGroupDevice", "Instrument Rack"),
    ("DrumCell", "Drum Sampler"),
    ("Tension", "Tension"),
    ("InstrumentSpectralResonator", "Spectral Resonator"),
];

/// Live's MIDI effects, by element.
const MIDI_EFFECTS: [(&str, &str); 9] = [
    ("MidiArpeggiator", "Arpeggiator"),
    ("MidiChord", "Chord"),
    ("MidiScale", "Scale"),
    ("MidiPitcher", "Pitch"),
    ("MidiRandom", "Random"),
    ("MidiVelocity", "Velocity"),
    ("MidiNoteLength", "Note Length"),
    ("MidiEffectGroupDevice", "MIDI Effect Rack"),
    ("MidiNoteEcho", "Note Echo"),
];

/// Live's audio effects whose element isn't the name Live shows.
const EFFECT_NAMES: [(&str, &str); 14] = [
    ("Eq8", "EQ Eight"),
    ("FilterEQ3", "EQ Three"),
    ("Compressor2", "Compressor"),
    ("GlueCompressor", "Glue Compressor"),
    ("AutoFilter", "Auto Filter"),
    ("AutoPan", "Auto Pan"),
    ("Reverb", "Reverb"),
    ("Delay", "Delay"),
    ("StereoGain", "Utility"),
    ("AudioEffectGroupDevice", "Audio Effect Rack"),
    ("FrequencyShifter", "Frequency Shifter"),
    ("CrossDelay", "Simple Delay"),
    ("PingPongDelay", "Ping Pong Delay"),
    ("Hybrid", "Hybrid Reverb"),
];

/// Every `FileRef` in the set, in the file's order, but none inside a `FreezeSequencer` (a frozen track's audio). The
/// frozen parts are left out as the walk meets them, rather than each `FileRef` looking up through everything above it:
/// a damaged or made-up set nested thousands deep would take minutes that way.
fn file_refs_not_frozen<'a, 'i>(set: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let mut found = Vec::new();
    let mut next = vec![set];
    while let Some(e) = next.pop() {
        if e.has_tag_name("FreezeSequencer") {
            continue;
        }
        if e.has_tag_name("FileRef") {
            found.push(e);
        }
        // In reverse, so the first is taken first.
        next.extend(e.children().rev().filter(Node::is_element));
    }
    found
}

/// Every device on a track, racks' insides too, in order. A plugin on a MIDI track before any instrument is its
/// instrument (a VST2 says which it is); everything after an instrument is an effect.
fn devices(track: Node<'_, '_>, out: &mut Vec<PluginRef>) {
    let Some(chain) = at(track, &["DeviceChain", "DeviceChain", "Devices"]) else { return };
    walk_devices(chain, !track.has_tag_name("MidiTrack"), out);
}

/// Walks the device lists in order, a rack's chains straight after the rack. It keeps its own list of where it is
/// rather than calling itself for each rack inside a rack, so a damaged or made-up set with racks nested thousands
/// deep can't run the thread out of stack (which ends the app at once); and each element is looked at once.
fn walk_devices(devices: Node<'_, '_>, instrument_seen: bool, out: &mut Vec<PluginRef>) {
    // Each list being walked, with whether an instrument was seen in it so far. The top one is the one being walked.
    let mut lists = vec![(devices.children(), instrument_seen)];
    while let Some((list, seen)) = lists.last_mut() {
        let Some(d) = list.next() else {
            lists.pop();
            continue;
        };
        if !d.is_element() {
            continue;
        }
        let tag = d.tag_name().name();
        // What was seen before this device: a plugin inside an Instrument Rack is the rack's instrument.
        let before = *seen;
        let plugin = match tag {
            "PluginDevice" => plugin_device(d, *seen),
            "AuPluginDevice" => au_device(d, *seen),
            t if t.starts_with("MxDevice") => Some(max_device(d)),
            t => Some(live_device(t)),
        };
        if let Some(p) = plugin {
            if p.kind == PluginKind::Instrument {
                *seen = true;
            }
            out.push(p);
        }
        // A rack's chains: `Branches/*/DeviceChain/MidiToAudioDeviceChain|AudioToAudioDeviceChain/Devices`.
        // A Drum Rack's return chains (`ReturnBranches`) hold effects only. Walked next, in order, before the
        // device after the rack.
        let mut chains = Vec::new();
        for (list, seen_at_start) in [("Branches", before), ("ReturnBranches", true)] {
            if let Some(branches) = child(d, list) {
                chains.extend(own_device_lists(branches).into_iter().map(|c| (c.children(), seen_at_start)));
            }
        }
        lists.extend(chains.into_iter().rev());
    }
}

/// The device lists inside a rack's `Branches`, in order: only the rack's own chains, not a list inside a device
/// further down (that one is walked from its own device). Each element is looked at once.
fn own_device_lists<'a, 'i>(branches: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let mut found = Vec::new();
    let mut next = vec![branches];
    while let Some(e) = next.pop() {
        if e.has_tag_name("Devices") {
            // Everything in a device list is a device: its own lists are its own.
            found.push(e);
            continue;
        }
        // In reverse, so the first is taken first (the lists come out in the file's order).
        next.extend(e.children().rev().filter(Node::is_element));
    }
    found
}

fn plugin_device(d: Node<'_, '_>, instrument_seen: bool) -> Option<PluginRef> {
    let desc = child(d, "PluginDesc")?;
    if let Some(vst) = child(desc, "VstPluginInfo") {
        let path = native(value(vst, &["Path"]).unwrap_or_default().trim());
        let name = value(vst, &["PlugName"]).map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
        let name = name.unwrap_or_else(|| plugins::name_from_path(&path));
        // VST2's own categories: 1 an effect, 2 a synth.
        let kind = match number(vst, &["Category"]).map(|c| c as i64) {
            Some(2) => PluginKind::Instrument,
            Some(1) => PluginKind::Effect,
            _ if instrument_seen => PluginKind::Effect,
            _ => PluginKind::Instrument,
        };
        let id = number(vst, &["UniqueId"]).map(|n| plugins::vst2_id(n as i64 as i32)).unwrap_or_default();
        return Some(PluginRef { name, vendor: String::new(), kind, format: "VST2".into(), path, id });
    }
    if let Some(vst3) = child(desc, "Vst3PluginInfo") {
        let name = value(vst3, &["Name"]).unwrap_or_default().trim().to_owned();
        let kind = if instrument_seen { PluginKind::Effect } else { PluginKind::Instrument };
        return Some(PluginRef {
            name,
            vendor: String::new(),
            kind,
            format: "VST3".into(),
            path: String::new(),
            id: vst3_id(vst3),
        });
    }
    None
}

/// A VST3's class id from Live's four signed numbers (`Uid/Fields.0` to `Fields.3`), as the plugin scanner writes it.
fn vst3_id(info: Node<'_, '_>) -> String {
    let Some(uid) = child(info, "Uid") else { return String::new() };
    let mut hex = String::new();
    for i in 0..4 {
        let Some(n) = number(uid, &[&format!("Fields.{i}")]) else { return String::new() };
        hex.push_str(&format!("{:08X}", n as i64 as u32));
    }
    plugins::class_id(&hex)
}

fn au_device(d: Node<'_, '_>, instrument_seen: bool) -> Option<PluginRef> {
    let info = at(d, &["PluginDesc", "AuPluginInfo"])?;
    let kind = if instrument_seen { PluginKind::Effect } else { PluginKind::Instrument };
    Some(PluginRef {
        name: value(info, &["Name"]).unwrap_or_default().trim().to_owned(),
        vendor: value(info, &["Manufacturer"]).unwrap_or_default().trim().to_owned(),
        kind,
        format: "AU".into(),
        path: String::new(),
        id: String::new(),
    })
}

/// A Max for Live device: named by its `.amxd` file.
fn max_device(d: Node<'_, '_>) -> PluginRef {
    let file = d
        .descendants()
        .filter(|e| e.has_tag_name("FileRef"))
        .find_map(|f| {
            let given = |name: &str| value(f, &[name]).filter(|v| !v.trim().is_empty());
            let path = given("Path").or_else(|| given("RelativePath")).or_else(|| given("Name"))?;
            Some(paths::stem(paths::file_name(path)).to_owned()).filter(|n| !n.is_empty())
        })
        .unwrap_or_else(|| "Max for Live device".to_owned());
    let kind = match d.tag_name().name() {
        "MxDeviceInstrument" => PluginKind::Instrument,
        "MxDeviceMidiEffect" => PluginKind::NoteEffect,
        _ => PluginKind::Effect,
    };
    PluginRef {
        name: file,
        vendor: "Ableton".into(),
        kind,
        format: "Native".into(),
        path: String::new(),
        id: String::new(),
    }
}

/// One of Live's own devices, by its element.
fn live_device(tag: &str) -> PluginRef {
    let find = |list: &[(&str, &'static str)]| list.iter().find(|(t, _)| *t == tag).map(|(_, n)| *n);
    let (name, kind) = if let Some(n) = find(&INSTRUMENTS) {
        (n.to_owned(), PluginKind::Instrument)
    } else if let Some(n) = find(&MIDI_EFFECTS) {
        (n.to_owned(), PluginKind::NoteEffect)
    } else {
        (find(&EFFECT_NAMES).map_or_else(|| spaced(tag), str::to_owned), PluginKind::Effect)
    };
    PluginRef { name, vendor: "Ableton".into(), kind, format: "Native".into(), path: String::new(), id: String::new() }
}

/// An element's name with spaces between its words ("BeatRepeat" is "Beat Repeat").
fn spaced(tag: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in tag.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrangement::TimeSignature;

    #[test]
    fn time_signatures_go_to_and_from_lives_number() {
        assert_eq!(signature_of(201), TimeSignature::new(4, 4));
        assert_eq!(signature_of(302), TimeSignature::new(6, 8));
        assert_eq!(signature_of(200), TimeSignature::new(3, 4));
        assert_eq!(signature_of(-1), None);
        assert_eq!(signature_of(99 * 6), None);
        for (top, bottom) in [(4, 4), (3, 4), (6, 8), (7, 8), (5, 4), (2, 2), (12, 16)] {
            let s = TimeSignature::new(top, bottom).unwrap();
            assert_eq!(signature_of(signature_code(s)), Some(s), "{top}/{bottom}");
        }
    }

    #[test]
    fn old_windows_sets_keep_paths_as_utf16_hex() {
        let path = r"C:\Samples\Kick.wav";
        let mut hex: String = path.encode_utf16().flat_map(u16::to_le_bytes).map(|b| format!("{b:02X}")).collect();
        hex.push_str("0000");
        assert_eq!(windows_path_in_data(&hex).as_deref(), Some(path));
        assert_eq!(windows_path_in_data("00000000"), None);
        assert_eq!(windows_path_in_data("zz"), None);
    }

    #[test]
    fn element_names_read_as_words() {
        assert_eq!(spaced("BeatRepeat"), "Beat Repeat");
        assert_eq!(spaced("Redux2"), "Redux2");
        assert_eq!(live_device("Eq8").name, "EQ Eight");
        assert_eq!(live_device("Operator").kind, PluginKind::Instrument);
        assert_eq!(live_device("MidiArpeggiator").kind, PluginKind::NoteEffect);
    }

    #[test]
    fn windows_paths_get_windows_slashes() {
        assert_eq!(native("C:/Samples/Kick.wav"), r"C:\Samples\Kick.wav");
        assert_eq!(native("/Users/me/Kick.wav"), "/Users/me/Kick.wav");
    }
}
