//! What's in a Studio One (Fender Studio Pro) `.song`: plugins, samples, tracks, tempo, note parts and stored plugin
//! states. A `.song` is a zip of XML files and preset files:
//!
//! - `metainfo.xml`: title, tempo and the Studio One version that saved it
//! - `Song/song.xml`: tracks, parts, notes, tempo
//! - `Song/mediapool.xml`: the audio files the song uses
//! - `Devices/*.xml`: instruments (`audiosynthfolder.xml`), mixer inserts (`audiomixer.xml`), Note FX
//! - `Presets/**`: stored plugin states (.vstpreset, .fxp, .aupreset, .preset)
//!
//! The XML uses an `x:` prefix it never declares and generic `<Attributes x:id="...">` elements, so the reading is
//! tolerant, as the C# reader's is: it looks for known attribute names anywhere in the tree.

use std::borrow::Cow;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use roxmltree::Node;

use crate::model::{Confidence, MidiClip, NoteEvent, PluginKind, PluginRef, ProjectInfo, velocity};
use crate::paths;
use crate::plugins;
use crate::presets::PresetBlob;
use crate::textscan;

pub const DAW: &str = "Studio One";
/// Goes up by one whenever the reader finds more (or different) things in the same files (rule 7). 2 is the C#
/// reader's: each insert's plugin by its own name and id, not the name the insert shows; Studio One's own effects
/// and Note FX.
pub const VERSION: u32 = 2;
pub const EXTENSIONS: [&str; 1] = ["song"];

const PRESET_EXTENSIONS: [&str; 6] = [".vstpreset", ".fxp", ".fxb", ".aupreset", ".preset", ".multipreset"];
const MIDI_PPQ: u16 = 960;

/// A `.song` opened: its zip, with its files' names in the order the zip lists them.
pub struct Song<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    names: Vec<String>,
}

/// Reads what's in a Studio One song. A file that isn't a zip can't be read and says so.
pub fn read(path: &Path) -> std::io::Result<ProjectInfo> {
    let raw = std::fs::read(path)?;
    Ok(Song::open(&raw)?.info())
}

impl<'a> Song<'a> {
    /// The file's bytes, opened as the zip a song is.
    pub fn open(raw: &'a [u8]) -> std::io::Result<Song<'a>> {
        let zip = zip::ZipArchive::new(Cursor::new(raw)).map_err(std::io::Error::other)?;
        let names = (0..zip.len())
            .map(|i| zip.name_for_index(i).and_then(Result::ok).unwrap_or_default().into_owned())
            .collect();
        Ok(Song { zip, names })
    }

    /// One of the song's XML files as text, with the undeclared `x:` prefixes taken out so it parses (`None` when
    /// it isn't there). The first file of that name, as .NET's zip finds it.
    fn xml(&mut self, name: &str) -> Option<String> {
        let i = self.names.iter().position(|n| n == name)?;
        let mut bytes = Vec::new();
        self.zip.by_index(i).ok()?.read_to_end(&mut bytes).ok()?;
        Some(without_prefixes(&text_of(&bytes)))
    }

    /// What the project list shows.
    pub fn info(&mut self) -> ProjectInfo {
        let mut info = ProjectInfo::new(DAW);
        info.confidence = Confidence::Partial;
        info.can_extract_midi = true;
        info.can_extract_presets = true;

        if let Some(text) = self.xml("metainfo.xml")
            && let Ok(meta) = parse(&text)
        {
            for e in elements(meta.root_element()).skip(1) {
                let value = attr(e, "value");
                match id(e) {
                    "Document:Generator" => info.version = value.to_owned(),
                    "Media:Title" => info.title = value.to_owned(),
                    "Media:Tempo" => info.tempo = num(value),
                    _ => {}
                }
            }
        }

        if let Some(text) = self.xml("Song/mediapool.xml")
            && let Ok(pool) = parse(&text)
        {
            for e in elements(pool.root_element()) {
                let url = attr(e, "url");
                if !url.is_empty() && textscan::is_audio_path(url) {
                    info.samples.push(url_to_path(url));
                }
            }
        }

        let devices: Vec<String> =
            self.names.iter().filter(|n| n.starts_with("Devices/") && n.ends_with(".xml")).cloned().collect();
        for entry in devices {
            let Some(text) = self.xml(&entry) else { continue };
            let Ok(doc) = parse(&text) else { continue };
            let kind = if entry_name(&entry).to_lowercase().contains("synth") {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            };
            for e in elements(doc.root_element()) {
                let name = attr(e, "name").trim();
                if name.is_empty() {
                    continue;
                }
                // Each insert keeps a note of its plugin, "classInfo": the plugin's own name and id, and whether it's a
                // VST2, a VST3 or one of Studio One's own. Its "deviceData" has the name the insert shows, which Studio
                // One numbers ("FabFilter Pro-Q 2 2") and you can change: read by that, plugins Solomon has showed as
                // missing ("Click Clack", 27 Oct 2026). An insert without the note goes by its own name.
                if let Some(class_id) = e.attribute("classID") {
                    info.plugins.push(plugin(e, name, class_id, kind));
                } else if id(e) == "deviceData"
                    && !e.parent_element().is_some_and(|p| elements(p).skip(1).any(|d| d.has_attribute("classID")))
                {
                    let uid = e
                        .parent_element()
                        .and_then(|p| p.children().filter(Node::is_element).find(|u| id(*u) == "deviceClassID"))
                        .map_or("", |u| attr(u, "uid"));
                    info.plugins.push(plugin(e, name, uid, kind));
                }
            }
        }

        let presets = self.names.iter().filter(|n| is_preset(n)).count();
        info.details.insert("Stored plugin states".into(), presets.to_string());

        match self.xml("Song/song.xml").as_deref().map(parse) {
            Some(Ok(song)) => {
                let mut notes = 0;
                for e in elements(song.root_element()) {
                    let tag = e.tag_name().name();
                    if matches!(tag, "MediaTrack" | "AudioTrack" | "MusicTrack" | "Track") {
                        let track = attr(e, "name");
                        if !track.is_empty() {
                            info.tracks.push(track.to_owned());
                        }
                    }
                    if info.tempo.is_none() {
                        info.tempo = num(attr(e, "tempo"));
                    }
                    if tag == "NoteEvent" && e != song.root_element() {
                        notes += 1;
                    }
                }
                info.details.insert("Notes".into(), notes.to_string());
            }
            _ => info.warnings.push("Song/song.xml is missing or unreadable.".into()),
        }
        info.dedupe()
    }

    /// The notes of each instrument part. Positions are in quarter notes unless the part (or its track) says its time
    /// format is seconds, when the song's tempo turns them into beats. Studio One has changed this between versions,
    /// so the app calls Studio One's MIDI export experimental.
    pub fn midi(&mut self) -> Vec<MidiClip> {
        let tempo = self.tempo();
        let Some(text) = self.xml("Song/song.xml") else { return Vec::new() };
        let Ok(song) = parse(&text) else { return Vec::new() };
        let mut clips = Vec::new();
        for track in elements(song.root_element()).filter(|e| e.tag_name().name() == "MusicTrack") {
            let track_name = match attr(track, "name") {
                "" => "Track",
                n => n,
            };
            let parts = elements(track).skip(1).filter(|e| matches!(e.tag_name().name(), "MusicPart" | "NotePart"));
            for (i, part) in parts.enumerate() {
                let format = format!("{}{}", attr(part, "timeFormat"), attr(track, "timeFormat"));
                let seconds = format.to_lowercase().contains("seconds");
                let beats = |v: f64| if seconds { v * tempo / 60.0 } else { v };
                let name = match attr(part, "name") {
                    "" => format!("{track_name} part {}", i + 1),
                    n => n.to_owned(),
                };
                let mut clip = MidiClip { name, ppq: MIDI_PPQ, ..MidiClip::default() };
                let notes = clip.track(track_name);
                for ev in elements(part).skip(1).filter(|e| e.tag_name().name() == "NoteEvent") {
                    let Some(start) = num(attr(ev, "start")) else { continue };
                    let length = num(attr(ev, "length")).unwrap_or(0.25);
                    let key = num(attr(ev, "pitch")).unwrap_or(60.0) as i32;
                    let vel = num(attr(ev, "velocity")).unwrap_or(0.8);
                    // 0 to 1 in newer songs, MIDI 1.0's 0 to 127 in older ones; kept in MIDI 2.0's 16 bits.
                    let velocity = if vel <= 1.0 {
                        velocity::from_unit(vel)
                    } else {
                        velocity::from_7(vel.round_ties_even().min(127.0) as u8)
                    };
                    notes.push(NoteEvent {
                        start: (beats(start) * MIDI_PPQ as f64).round_ties_even() as i64,
                        length: ((beats(length) * MIDI_PPQ as f64).round_ties_even() as i64).max(1),
                        key: key.clamp(0, 127) as u8,
                        velocity,
                        muted: false,
                        step: false,
                    });
                }
                if !notes.is_empty() {
                    clips.push(clip);
                }
            }
        }
        clips
    }

    /// The song's tempo as the project list reads it, or 120.
    fn tempo(&mut self) -> f64 {
        let meta = self.xml("metainfo.xml");
        let meta = meta.as_deref().and_then(|t| parse(t).ok());
        // The last tempo the metainfo gives, if it reads as a number; the song's own after that.
        let from_meta = meta.and_then(|m| {
            elements(m.root_element())
                .skip(1)
                .filter(|e| id(*e) == "Media:Tempo")
                .last()
                .and_then(|e| num(attr(e, "value")))
        });
        if let Some(tempo) = from_meta {
            return tempo;
        }
        let song = self.xml("Song/song.xml");
        let song = song.as_deref().and_then(|t| parse(t).ok());
        song.and_then(|s| elements(s.root_element()).find_map(|e| num(attr(e, "tempo")))).unwrap_or(120.0)
    }

    /// The stored plugin states, each named after its file and the folder it's in (the plugin's name). A state whose
    /// bytes don't come out whole is left out, not the rest with it.
    pub fn presets(&mut self) -> Vec<PresetBlob> {
        let mut presets = Vec::new();
        for i in 0..self.names.len() {
            let Some(full) = self.names.get(i) else { continue };
            let name = entry_name(full);
            let ext = paths::extension(name);
            if !PRESET_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(ext)) {
                continue;
            }
            let parts: Vec<&str> = full.split('/').collect();
            let folder = if parts.len() > 1 { parts.get(parts.len() - 2).copied().unwrap_or_default() } else { "" };
            let (stem, ext, folder) = (paths::stem(name).to_owned(), ext.to_owned(), folder.to_owned());
            let mut data = Vec::new();
            let Ok(mut entry) = self.zip.by_index(i) else { continue };
            if entry.read_to_end(&mut data).is_err() {
                continue;
            }
            presets.push(PresetBlob::new(&stem, &folder, ext, data));
        }
        presets
    }
}

/// Whether a zip entry is a stored plugin state, by its ending.
fn is_preset(name: &str) -> bool {
    PRESET_EXTENSIONS.iter().any(|x| plugins::ends_with_ignore_case(name, x))
}

/// A zip entry's own name, without its folders (as .NET's `ZipArchiveEntry.Name`).
fn entry_name(full: &str) -> &str {
    full.rsplit(['/', if cfg!(windows) { '\\' } else { '/' }]).next().unwrap_or(full)
}

/// A plugin from Studio One's note of it, with its id as the plugin scanner writes it.
fn plugin(e: Node<'_, '_>, name: &str, class_id: &str, kind: PluginKind) -> PluginRef {
    let vendor = attr(e, "vendor");
    let format = format_of(e, vendor);
    let mut id = plugins::class_id(class_id);
    // A VST2 has no class id of its own: Studio One makes one with the plugin's four letters inside.
    if format == "VST2" {
        id = plugins::vst2_id_in(&id).unwrap_or_default();
    }
    PluginRef {
        name: name.to_owned(),
        vendor: vendor.to_owned(),
        kind: kind_of(e).unwrap_or(kind),
        format: format.to_owned(),
        path: String::new(),
        id,
    }
}

fn kind_of(e: Node<'_, '_>) -> Option<PluginKind> {
    match attr(e, "category") {
        "AudioSynth" => Some(PluginKind::Instrument),
        "AudioEffect" | "AudioMixEffect" => Some(PluginKind::Effect),
        "MusicEffect" => Some(PluginKind::NoteEffect),
        _ => None,
    }
}

fn format_of(e: Node<'_, '_>, vendor: &str) -> &'static str {
    // subCategory starts with it: "VST2", "VST3/Delay", "(Native)/EQ".
    let sub = attr(e, "subCategory");
    if sub.starts_with("(Native)") {
        return "Native";
    }
    for key in ["subCategory", "deviceClass", "category", "type"] {
        let v = attr(e, key).to_lowercase();
        if v.contains("vst3") {
            return "VST3";
        }
        if v.contains("vst") {
            return "VST2";
        }
        if v.contains("audiounit") {
            return "AU";
        }
        if v.contains("clap") {
            return "CLAP";
        }
    }
    // Note FX (the Arpeggiator, Chorder, Repeater) are all Studio One's own, and say nothing more.
    if attr(e, "category") == "MusicEffect" && sub.is_empty() {
        return "Native";
    }
    if vendor.to_lowercase().contains("presonus") { "Native" } else { "" }
}

fn parse(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse(text)
}

/// An element and every element inside it, in the order they're written.
fn elements<'a, 'input>(e: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    e.descendants().filter(Node::is_element)
}

fn attr<'a>(e: Node<'a, '_>, name: &str) -> &'a str {
    e.attribute(name).unwrap_or("")
}

/// An element's `x:id` (as rewritten, `x_id`), or its plain `id`.
fn id<'a>(e: Node<'a, '_>) -> &'a str {
    e.attribute("x_id").unwrap_or_else(|| attr(e, "id"))
}

/// A file's text: UTF-8 (or UTF-16 with its byte-order mark), as .NET's `StreamReader` reads it.
fn text_of(bytes: &[u8]) -> Cow<'_, str> {
    let utf16 = |b: &[u8], big: bool| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if big { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) })
            .collect();
        Cow::Owned(String::from_utf16_lossy(&units))
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        _ => String::from_utf8_lossy(bytes),
    }
}

/// Studio One writes an `x:` prefix it never declares: dropped from element names, and prefixed attributes become
/// plain ones (`x:id` → `x_id`), as the C# reader does, so the XML parses.
fn without_prefixes(text: &str) -> String {
    static ELEMENT: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"<(/?)\w+:").ok());
    static ATTRIBUTE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(\s)(\w+):(\w+)=").ok());
    let (Some(element), Some(attribute)) = (ELEMENT.as_ref(), ATTRIBUTE.as_ref()) else { return text.to_owned() };
    let text = element.replace_all(text, "<$1");
    attribute.replace_all(&text, "${1}${2}_${3}=").into_owned()
}

/// A number as .NET's invariant `double.TryParse` reads it (spaces around it allowed; "Infinity" and "NaN" too,
/// but not Rust's own "inf").
fn num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.trim_start_matches(['+', '-']).eq_ignore_ascii_case("inf") {
        return None;
    }
    t.parse().ok()
}

/// A media pool url as a path: `file:///C:/Samples/Kick%20A.wav` is `C:\Samples\Kick A.wav`, `file:///Users/...` is
/// `/Users/...`.
fn url_to_path(url: &str) -> String {
    let Some(rest) = url.get(..5).filter(|s| s.eq_ignore_ascii_case("file:")).and(url.get(5..)) else {
        return unescape(url);
    };
    let p = unescape(rest.trim_start_matches('/'));
    if matches!(p.as_bytes(), [d, b':', b'/', ..] if d.is_ascii_alphabetic()) {
        p.replace('/', "\\")
    } else {
        format!("/{p}")
    }
}

/// `%xx` escapes turned back into the letters they stand for (.NET's `Uri.UnescapeDataString`): escapes that don't
/// make whole UTF-8 letters are left as written.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16);
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        // A run of escapes, decoded together so a letter written as several bytes comes back whole.
        let mut bytes = Vec::new();
        let mut written = Vec::new();
        while let Some([b'%', h, l]) = b.get(i..i + 3)
            && let (Some(h), Some(l)) = (hex(*h), hex(*l))
        {
            bytes.push((h * 16 + l) as u8);
            written.push(s.get(i..i + 3).unwrap_or_default());
            i += 3;
        }
        if bytes.is_empty() {
            let next = s.get(i..).and_then(|r| r.chars().next());
            let Some(c) = next else { break };
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        let mut at = 0;
        for chunk in bytes.utf8_chunks() {
            out.push_str(chunk.valid());
            at += chunk.valid().len();
            for _ in chunk.invalid() {
                out.push_str(written.get(at).copied().unwrap_or_default());
                at += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_become_paths_as_dotnet_reads_them() {
        assert_eq!(url_to_path("file:///C:/Users/Sol/Samples/Vox%20Chop.wav"), r"C:\Users\Sol\Samples\Vox Chop.wav");
        assert_eq!(url_to_path("FILE:///Users/sol/Kick.wav"), "/Users/sol/Kick.wav");
        assert_eq!(url_to_path("Samples/Caf%C3%A9.wav"), "Samples/Café.wav");
        // An escape that isn't a whole letter stays as written.
        assert_eq!(unescape("a%FFb%2"), "a%FFb%2");
        assert_eq!(unescape("%e2%82%ac and %41"), "€ and A");
    }

    #[test]
    fn prefixes_come_out_so_the_xml_parses() {
        assert_eq!(
            without_prefixes(r#"<x:Song><Attributes x:id="a" name="b"/></x:Song>"#),
            r#"<Song><Attributes x_id="a" name="b"/></Song>"#
        );
    }

    #[test]
    fn numbers_read_as_dotnet_reads_them() {
        assert_eq!((num(" 92 "), num("1e2"), num(".5")), (Some(92.0), Some(100.0), Some(0.5)));
        assert_eq!((num("inf"), num(""), num("1,5")), (None, None, None));
        assert!(num("NaN").is_some_and(f64::is_nan) && num("-Infinity") == Some(f64::NEG_INFINITY));
    }
}
