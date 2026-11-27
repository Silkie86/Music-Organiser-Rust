//! Writes a new Ableton Live 12 set from a song's arrangement (any DAW's, as the readers give it): its tempo, time
//! signature, locators, tracks and their names and mutes, a MIDI clip for each clip of notes and an audio clip for each
//! stretch of an audio file, all in the Arrangement view. What can't go across (instruments, effects, automation,
//! groups, reversed audio) is listed in [`Written::left_out`].
//!
//! A Live set is gzipped XML. Live expects every part of it, so the set is written from the layout Live 12.1 gives an
//! empty set, kept in `live12/` (learned from the empty set Live's trial installs, 27 Nov 2026): element names and
//! Live's own starting values, with nothing of anyone's in them. Left out of that layout: the return tracks and their
//! sends (their effects' presets name the folders of whoever saved them), and the groove pool's groove (Ableton's own
//! content). What the writer fills in is marked `$name`:
//!
//! - `set.xml`: the set, with `$tracks`, `$locators`, `$tempo` (the main track's tempo and its automation's first
//!   point), `$signature` (one number: the top minus one, plus 99 times the bottom's power of two) and `$next`.
//! - `midi-track.xml`, `audio-track.xml`: a track, with its `$id`, `$name`, `$color`, `$on` (its Track Activator) and
//!   `$clips`, its arrangement clips.
//! - `midi-clip.xml`, `audio-clip.xml`: an arrangement clip, as Live 12.1 lays one out (Live 12.4's, less its one new
//!   element). Its notes are `KeyTrack`s, one for each key, with fractional velocities (MIDI 2.0's resolution kept as
//!   far as Live goes).
//!
//! Every automation and modulation target in a track has an id of its own (`Id="@"` in the layout), counted on from
//! the empty set's, and `NextPointeeId` is the first one free after them, as Live keeps it.
//!
//! An audio clip is always warped, with two warp markers that say how many seconds of the file a beat plays: so it
//! plays at the speed the song had it. Re-Pitch (speed moves pitch) when the song's clip wasn't stretched, Complex
//! (keeps pitch) when it was.

use std::collections::BTreeMap;
use std::io::Write;

use crate::arrangement::{Arrangement, AudioPart, LaneKind, PreviewClip, PreviewMarker, Stretch, TimeSignature};
use crate::studio_one::write::{FormatOf, Written, attr};

const SET: &str = include_str!("live12/set.xml");
const MIDI_TRACK: &str = include_str!("live12/midi-track.xml");
const AUDIO_TRACK: &str = include_str!("live12/audio-track.xml");
const MIDI_CLIP: &str = include_str!("live12/midi-clip.xml");
const AUDIO_CLIP: &str = include_str!("live12/audio-clip.xml");
/// The empty set's `NextPointeeId`: the tracks' targets count on from it.
const FIRST_FREE_POINTEE: u64 = 22_182;
/// Live's slowest and fastest tempo.
const SLOWEST: f64 = 20.0;
const FASTEST: f64 = 999.0;
/// The colours Live gives a new MIDI and audio track in its empty set.
const MIDI_COLOUR: u32 = 19;
const AUDIO_COLOUR: u32 = 20;
/// Live's warp modes: Re-Pitch, and Complex.
const REPITCH: u32 = 3;
const COMPLEX: u32 = 4;

/// The song as a new Live set. `format` says what an audio file is, when it can be read; `folder` is where the set
/// will be, so each sample is also given by its path from there (as Live saves a sample outside its project).
pub fn write(song: &Arrangement, format: &FormatOf, folder: Option<&str>) -> Written {
    let mut left_out = Vec::new();
    let tempo = if song.tempo.is_finite() { song.tempo.clamp(SLOWEST, FASTEST) } else { 120.0 };
    if (tempo - song.tempo).abs() > 1e-9 {
        left_out.push(format!("The tempo ({:.0} BPM): Live's is {SLOWEST:.0} to {FASTEST:.0}.", song.tempo));
    }
    let signature = if fits_live(song.signature) {
        song.signature
    } else {
        let (top, bottom) = (song.signature.top(), song.signature.bottom());
        left_out.push(format!("The time signature ({top}/{bottom}): Live's go to 99 over 16; it's 4/4."));
        TimeSignature::FOUR_FOUR
    };
    let mut out = Tracks {
        tempo,
        signature,
        format,
        folder,
        xml: String::new(),
        count: 0,
        pointee: FIRST_FREE_POINTEE,
        clips: 0,
        reversed: false,
    };
    let (mut automation, mut groups, mut layers, mut instruments) = (false, false, false, false);
    for lane in &song.lanes {
        match lane.kind {
            LaneKind::Automation => {
                automation |= !lane.clips.is_empty();
                continue;
            }
            LaneKind::Group => {
                groups = true;
                continue;
            }
            LaneKind::Instrument | LaneKind::Audio => {}
        }
        if lane.layer {
            layers = true;
            continue;
        }
        automation |= lane.clips.iter().any(|c| !c.points.is_empty());
        groups |= lane.depth > 0;
        let name = if lane.name.trim().is_empty() { format!("Track {}", out.count + 1) } else { lane.name.clone() };
        // A track holding notes and audio both (an FL playlist track, a Bitwig hybrid track) becomes a MIDI track and
        // an audio track. One with neither keeps its kind.
        let has_notes = lane.clips.iter().any(|c| !c.notes.is_empty());
        let has_audio = lane.clips.iter().any(|c| !c.audio.is_empty());
        let empty = !has_notes && !has_audio;
        let midi = has_notes || (empty && lane.kind == LaneKind::Instrument);
        if midi {
            instruments = true;
            for (track, clips) in crate::arrangement::instrument_parts(lane, &name) {
                out.midi_track(&track, lane.muted, &clips);
            }
        }
        if has_audio || (empty && lane.kind == LaneKind::Audio) {
            let track = if midi { format!("{name} audio") } else { name };
            out.audio_track(&track, lane.muted, &lane.clips);
        }
    }
    if out.reversed {
        left_out.push("Reversed audio: it plays forwards.".into());
    }
    if automation {
        left_out.push("Automation.".into());
    }
    if groups {
        left_out.push("Groups: their tracks are side by side.".into());
    }
    if layers {
        left_out.push("Takes that don't play (a track's other layers).".into());
    }
    if instruments {
        left_out.push("Instruments: each MIDI track has none until one is chosen.".into());
    }
    if out.count > 0 {
        left_out.push("Effects and mixer settings.".into());
    }
    left_out.extend(song.notes.iter().cloned());
    let set = fill(SET, |name| match name {
        "tracks" => Some(if out.xml.is_empty() { String::new() } else { format!("\n{}\t\t", out.xml) }),
        "locators" => Some(locators(&song.markers)),
        "tempo" => Some(number(tempo)),
        "signature" => Some(crate::ableton::signature_code(signature).to_string()),
        "next" => Some(out.pointee.to_string()),
        _ => None,
    });
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let bytes = match gz.write_all(set.as_bytes()) {
        Ok(()) => gz.finish().unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    Written { bytes, left_out }
}

/// Whether Live can show a time signature: a top of 1 to 99 over 1, 2, 4, 8 or 16.
fn fits_live(signature: TimeSignature) -> bool {
    (1..=99).contains(&signature.top()) && signature.bottom().is_power_of_two() && signature.bottom() <= 16
}

/// The tracks so far, as XML, and the numbers they've taken.
struct Tracks<'f> {
    tempo: f64,
    signature: TimeSignature,
    format: &'f FormatOf,
    folder: Option<&'f str>,
    xml: String,
    count: u32,
    /// The next free automation or modulation target id.
    pointee: u64,
    /// The next clip id and take id.
    clips: u32,
    reversed: bool,
}

impl Tracks<'_> {
    /// A track from its layout: its own id, name, colour, whether it plays, its clips, and fresh target ids.
    fn track(&mut self, layout: &str, name: &str, muted: bool, colour: u32, clips: &str) {
        // Track ids start after the empty set's own (its tracks were 8 to 14).
        let id = 20 + self.count;
        self.count += 1;
        let filled = fill(layout, |field| match field {
            "id" => Some(id.to_string()),
            "name" => Some(attr(name)),
            "color" => Some(colour.to_string()),
            "on" => Some((!muted).to_string()),
            "clips" => Some(clips.to_owned()),
            _ => None,
        });
        let mut rest = filled.as_str();
        while let Some(at) = rest.find("Id=\"@\"") {
            self.xml.push_str(rest.get(..at).unwrap_or_default());
            self.xml.push_str(&format!("Id=\"{}\"", self.pointee));
            self.pointee += 1;
            rest = rest.get(at + 6..).unwrap_or_default();
        }
        self.xml.push_str(rest);
    }

    fn next_clip(&mut self) -> u32 {
        self.clips += 1;
        self.clips
    }

    fn midi_track(&mut self, name: &str, muted: bool, clips: &[PreviewClip]) {
        let mut xml = String::new();
        for clip in clips {
            let start = if clip.start.is_finite() { clip.start.max(0.0) } else { 0.0 };
            let length = if clip.length.is_finite() && clip.length > 0.0 { clip.length } else { 1.0 / 64.0 };
            let id = self.next_clip();
            let (keytracks, next_note) = key_tracks(clip);
            let clip_name = if clip.name.trim().is_empty() { name } else { &clip.name };
            xml.push_str(&fill(MIDI_CLIP, |field| {
                self.clip_field(field, id, start, 0.0, length, clip_name, clip.muted, MIDI_COLOUR).or_else(|| {
                    match field {
                        "keytracks" => Some(keytracks.clone()),
                        "nextnote" => Some(next_note.to_string()),
                        _ => None,
                    }
                })
            }));
        }
        self.track(MIDI_TRACK, name, muted, MIDI_COLOUR, &with_line_breaks(&xml));
    }

    fn audio_track(&mut self, name: &str, muted: bool, clips: &[PreviewClip]) {
        let mut xml = String::new();
        for clip in clips {
            let clip_name = if clip.name.trim().is_empty() { name } else { &clip.name };
            for part in &clip.audio {
                xml.push_str(&self.audio_clip(clip, part, clip_name));
            }
        }
        self.track(AUDIO_TRACK, name, muted, AUDIO_COLOUR, &with_line_breaks(&xml));
    }

    /// One stretch of an audio file as a clip of its own (see the top of the file).
    fn audio_clip(&mut self, clip: &PreviewClip, part: &AudioPart, name: &str) -> String {
        let at = clip.start + part.start;
        let at = if at.is_finite() { at.max(0.0) } else { 0.0 };
        let length = if part.length.is_finite() && part.length > 0.0 { part.length } else { 1.0 / 64.0 };
        self.reversed |= part.reversed;
        let file = (self.format)(&part.path);
        // Seconds of the file a beat plays: as the song said, or the whole file over the beats it fits, or (when
        // neither can be told) the song's own beat, so it plays at its own speed.
        let rate = part.rate(file.as_ref().map_or(0.0, |f| f.seconds));
        let rate = if rate > 0.0 && rate.is_finite() { rate } else { 60.0 / self.tempo };
        let from = if part.from.is_finite() { part.from.max(0.0) / rate } else { 0.0 };
        let warp = match part.stretch {
            None | Some(Stretch::Raw | Stretch::Repitch) => REPITCH,
            Some(_) => COMPLEX,
        };
        let (fade_in, fade_out) = clip.fades_of(part);
        let semitones = if part.semitones.is_finite() { part.semitones.clamp(-48.0, 48.0) } else { 0.0 };
        let coarse = semitones.round();
        let gain = if part.gain_db.is_finite() { 10f64.powf(part.gain_db / 20.0) } else { 0.0 };
        let path = part.path.replace('\\', "/");
        let relative = self.folder.and_then(|f| relative_path(f, &part.path));
        let id = self.next_clip();
        fill(AUDIO_CLIP, |field| {
            self.clip_field(field, id, at, from, from + length, name, clip.muted, AUDIO_COLOUR).or_else(|| match field {
                "path" => Some(attr(&path)),
                // 1: a path from the set's folder, as Live saves a sample outside its project; 0: none.
                "reltype" => Some(if relative.is_some() { "1" } else { "0" }.to_owned()),
                "relpath" => Some(attr(relative.as_deref().unwrap_or_default())),
                "frames" => Some(file.as_ref().map_or(0, |f| (f.seconds * f64::from(f.rate)).round() as u64).to_string()),
                "rate" => Some(file.as_ref().map_or(44_100, |f| f.rate).to_string()),
                "warp" => Some(warp.to_string()),
                "fadein" => Some(number(fade_in)),
                "fadeout" => Some(number(fade_out)),
                "coarse" => Some(number(coarse)),
                "fine" => Some(number(((semitones - coarse) * 100.0).round())),
                "gain" => Some(number(gain)),
                "warps" => Some(format!(
                    "\n\t\t\t\t\t\t\t\t<WarpMarker Id=\"0\" SecTime=\"0\" BeatTime=\"0\" />\n\t\t\t\t\t\t\t\t<WarpMarker Id=\"1\" \
                     SecTime=\"{}\" BeatTime=\"1\" />\n\t\t\t\t\t\t\t",
                    number(rate)
                )),
                _ => None,
            })
        })
    }

    /// What every clip fills in: where it is in the song (`start`, playing `from` to `to` of its own beats).
    #[allow(clippy::too_many_arguments)]
    fn clip_field(
        &self,
        field: &str,
        id: u32,
        start: f64,
        from: f64,
        to: f64,
        name: &str,
        muted: bool,
        colour: u32,
    ) -> Option<String> {
        Some(match field {
            "id" => id.to_string(),
            "take" => id.to_string(),
            "start" => number(start),
            "end" => number(start + (to - from)),
            "from" => number(from),
            "to" => number(to),
            "name" => attr(name),
            "color" => colour.to_string(),
            "muted" => muted.to_string(),
            "top" => self.signature.top().to_string(),
            "bottom" => self.signature.bottom().to_string(),
            _ => return None,
        })
    }
}

/// A clip's notes as Live keeps them: a `KeyTrack` for each key, lowest first; and the clip's next free note id.
fn key_tracks(clip: &PreviewClip) -> (String, u32) {
    let mut by_key: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut note_id = 0u32;
    for n in &clip.notes {
        if !n.start.is_finite() || !n.length.is_finite() {
            continue;
        }
        note_id += 1;
        // 0 to 127 with fractions (Live 11 on), from the 16-bit velocity; Live plays nothing below 1.
        let velocity = (f64::from(n.velocity) / f64::from(u16::MAX) * 127.0).clamp(1.0, 127.0);
        by_key.entry(n.key.min(127)).or_default().push(format!(
            "<MidiNoteEvent Time=\"{}\" Duration=\"{}\" Velocity=\"{}\" VelocityDeviation=\"0\" OffVelocity=\"64\" \
             Probability=\"1\" IsEnabled=\"{}\" NoteId=\"{note_id}\" />",
            number(n.start.max(0.0)),
            number(n.length.max(1.0 / 960.0)),
            number(velocity),
            !n.muted,
        ));
    }
    let pad = "\t".repeat(10);
    let mut xml = String::new();
    for (i, (key, notes)) in by_key.iter().enumerate() {
        xml.push_str(&format!("\n{pad}<KeyTrack Id=\"{i}\">\n{pad}\t<Notes>"));
        for n in notes {
            xml.push_str(&format!("\n{pad}\t\t{n}"));
        }
        xml.push_str(&format!("\n{pad}\t</Notes>\n{pad}\t<MidiKey Value=\"{key}\" />\n{pad}</KeyTrack>"));
    }
    if !xml.is_empty() {
        xml.push_str(&format!("\n{}", "\t".repeat(9)));
    }
    (xml, note_id + 1)
}

/// The song's markers as Live's locators, in order.
fn locators(markers: &[PreviewMarker]) -> String {
    let mut placed: Vec<&PreviewMarker> = markers.iter().filter(|m| m.beat.is_finite() && m.beat >= 0.0).collect();
    placed.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let mut xml = String::new();
    for (i, m) in placed.iter().enumerate() {
        // One with no name gets its number, as the Studio One copy names it.
        let name = if m.name.trim().is_empty() { format!("#{}", i + 1) } else { m.name.clone() };
        xml.push_str(&format!(
            "\n\t\t\t<Locator Id=\"{i}\">\n\t\t\t\t<LomId Value=\"0\" />\n\t\t\t\t<Time Value=\"{}\" />\n\t\t\t\t<Name \
             Value=\"{}\" />\n\t\t\t\t<Annotation Value=\"\" />\n\t\t\t\t<IsSongStart Value=\"false\" />\n\t\t\t</Locator>",
            number(m.beat),
            attr(&name)
        ));
    }
    if !xml.is_empty() {
        xml.push_str("\n\t\t\t");
    }
    xml
}

/// Clips placed in a list of their own, a line each.
fn with_line_breaks(clips: &str) -> String {
    if clips.is_empty() { String::new() } else { format!("\n{}{}", clips.trim_end(), "\n\t\t\t\t\t\t\t") }
}

/// A number as Live writes one: whole numbers without a point, others in full.
fn number(x: f64) -> String {
    if !x.is_finite() {
        return "0".into();
    }
    if x == x.trunc() && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

/// `layout` with each `$name` it holds filled in by `value` (one it doesn't know stays as it is).
fn fill(layout: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(layout.len() + 256);
    let mut rest = layout;
    while let Some(at) = rest.find('$') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at + 1..).unwrap_or_default();
        let name_end = after.find(|c: char| !c.is_ascii_lowercase()).unwrap_or(after.len());
        let name = after.get(..name_end).unwrap_or_default();
        match value(name) {
            Some(v) => out.push_str(&v),
            None => {
                out.push('$');
                out.push_str(name);
            }
        }
        rest = after.get(name_end..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// `file`'s path from `folder`, with forward slashes, when both are on the same drive.
fn relative_path(folder: &str, file: &str) -> Option<String> {
    let parts = |p: &str| -> Vec<String> {
        p.replace('\\', "/").split('/').filter(|s| !s.is_empty() && *s != ".").map(str::to_owned).collect()
    };
    let (from, to) = (parts(folder), parts(file));
    // Windows names aren't case-sensitive; a path that starts at a drive or share does on both.
    let same = |a: &String, b: &String| a.eq_ignore_ascii_case(b);
    if from.first().zip(to.first()).is_none_or(|(a, b)| !same(a, b)) {
        return None;
    }
    let shared = from.iter().zip(&to).take_while(|(a, b)| same(a, b)).count();
    let mut out: Vec<&str> = vec![".."; from.len() - shared];
    out.extend(to.iter().skip(shared).map(String::as_str));
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_s_fields_are_filled_and_others_left() {
        let out = fill("a $to b $top c $x $", |n| match n {
            "to" => Some("1".into()),
            "top" => Some("2".into()),
            _ => None,
        });
        assert_eq!(out, "a 1 b 2 c $x $");
    }

    #[test]
    fn a_sample_s_path_from_the_set_s_folder() {
        assert_eq!(
            relative_path("C:/Export/Song/Ableton", "C:\\Samples\\Kick.wav").as_deref(),
            Some("../../../Samples/Kick.wav")
        );
        assert_eq!(relative_path("c:/a/b", "C:/A/b/c.wav").as_deref(), Some("c.wav"));
        assert_eq!(relative_path("C:/a", "D:/b.wav"), None);
        assert_eq!(relative_path("/home/x", "/home/y/z.wav").as_deref(), Some("../y/z.wav"));
    }

    #[test]
    fn numbers_as_live_writes_them() {
        assert_eq!(number(4.0), "4");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(f64::NAN), "0");
    }

    #[test]
    fn the_layout_holds_only_what_the_writer_fills() {
        for layout in [SET, MIDI_TRACK, AUDIO_TRACK, MIDI_CLIP, AUDIO_CLIP] {
            assert!(!layout.contains("Users"), "no one's folders");
            assert!(!layout.contains("<ReturnTrack "), "no return tracks");
        }
    }
}
