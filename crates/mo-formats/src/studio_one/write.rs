//! Writes a new Studio One song from a song's arrangement (any DAW's, as the readers give it): its tempo, time
//! signature, markers, tracks and their names, a part for each clip of notes, and its audio clips. What can't go across
//! (plugins and their settings, automation, a stretched clip's stretch) is listed in [`Written::left_out`].
//!
//! The song is built on Studio One's own empty song (the recipe test save "MO Recipe 01 empty", Studio Pro 8.2), with
//! its artist and its folder taken out, so every file Studio One expects is there. What's added follows the recipe
//! saves:
//!
//! - `Song/song.xml`: a `MediaTrack` for each track (`mediaType` Music or Audio) after the song's own hidden tracks,
//!   pointing at its channel by `channelID`; a `MusicPart` for each clip of notes (`clipID` names its notes in the
//!   media pool, `start` and `length` in beats); an `AudioEvent` for each stretch of an audio file. Markers are
//!   `MarkerEvent`s (type 0) in the marker track, between its Start and End.
//! - The tempo is seconds per beat (`TempoMapSegment tempo`) and, for the tempo track, (BPM - 10) / 390.
//! - `Song/mediapool.xml`: a `MusicClip` for each part's notes, in `Performances/<track>/<track>(n).musicx` (binary
//!   JSON: {timeFormat 2, events [{start, pitch, noteId, length, velocity 0 to 1}], envelopes []}), and an `AudioClip`
//!   for each audio file, by its path.
//! - `Devices/musictrackdevice.xml`: a `MusicTrackChannel` for each instrument track, with no instrument (Studio One
//!   can't load another DAW's instrument: the track plays nothing until one is chosen).
//! - `Devices/audiomixer.xml`: an `AudioTrackChannel` for each audio track, to the main output.
//!
//! An audio track follows the tempo without stretching (`tempoFollow` 1, as recipe 08e), and its events count their
//! length and where they start in the file in seconds, so a file plays at its own speed, sped up or slowed down by
//! `speed` where the song had it faster or slower.
//!
//! The empty song's record of the audio interface's settings (`Devices/hardwarestoragemanager.xml`: preamp gains,
//! phantom power) is left empty, so opening a copy never changes them.

use std::collections::HashMap;
use std::io::{Cursor, Write};

use crate::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewNote, Stretch};

/// Studio One's empty song (recipe save 01).
const TEMPLATE: &[u8] = include_bytes!("../../tests/fixtures/recipe/studio-one/MO Recipe 01 empty.song");
/// The empty song's main output, where every new channel goes.
const MAIN_OUT: &str = "{BFBA70C6-942D-4589-93AE-DB56E5D4802C}";
/// The tempos Studio One's tempo track holds.
const SLOWEST: f64 = 10.0;
const FASTEST: f64 = 400.0;
/// The audio interface's settings, none (see the top of the file).
const EMPTY_HARDWARE: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<HardwareStorageManager>\n\t<List x:id=\"mixers\"/>\n</HardwareStorageManager>\n";
/// Where the empty song ends, in beats: a song that's longer ends four bars after its last clip.
const EMPTY_END: f64 = 600.0;

/// A song written, and what of the song couldn't go into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub left_out: Vec<String>,
}

/// What an audio file is, by its path, when it can be read.
pub type FormatOf = dyn Fn(&str) -> Option<SampleFormat>;

/// What Studio One keeps about an audio file: its sample rate, channels and length in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleFormat {
    pub rate: u32,
    pub channels: u16,
    pub seconds: f64,
}

/// The song as a new Studio One song named `title`. `format` says what an audio file is, when it can be read (a file
/// it can't is left for Studio One to read).
pub fn write(song: &Arrangement, title: &str, format: &FormatOf) -> Written {
    let mut left_out = Vec::new();
    let tempo = if song.tempo.is_finite() { song.tempo.clamp(SLOWEST, FASTEST) } else { 120.0 };
    if (tempo - song.tempo).abs() > 1e-9 {
        left_out.push(format!("The tempo ({:.0} BPM): Studio One's is {SLOWEST:.0} to {FASTEST:.0}.", song.tempo));
    }
    let mut out = Parts { spb: 60.0 / tempo, format: Some(format), ..Parts::default() };
    let (mut automation, mut layers, mut instruments) = (false, false, false);
    for lane in &song.lanes {
        match lane.kind {
            LaneKind::Automation => {
                automation |= !lane.clips.is_empty();
                continue;
            }
            LaneKind::Group => continue,
            LaneKind::Instrument | LaneKind::Audio => {}
        }
        if lane.layer {
            layers = true;
            continue;
        }
        automation |= lane.clips.iter().any(|c| !c.points.is_empty());
        let name = if lane.name.trim().is_empty() { format!("Track {}", out.tracks + 1) } else { lane.name.clone() };
        // A track can hold notes and audio both (an FL playlist track, a Bitwig hybrid track): Studio One's can't, so
        // it becomes an instrument track and an audio track. One with neither keeps its kind.
        let has_notes = lane.clips.iter().any(|c| !c.notes.is_empty());
        let has_audio = lane.clips.iter().any(|c| !c.audio.is_empty());
        let empty = !has_notes && !has_audio;
        let music = has_notes || (empty && lane.kind == LaneKind::Instrument);
        if music {
            instruments = true;
            for (track, clips) in crate::arrangement::instrument_parts(lane, &name) {
                let events = out.music_events(&track, &clips);
                out.track(false, &track, lane, &events);
            }
        }
        if has_audio || (empty && lane.kind == LaneKind::Audio) {
            let track = if music { format!("{name} audio") } else { name };
            let events = out.audio_events(&track, &lane.clips);
            out.track(true, &track, lane, &events);
        }
    }
    if out.stretched {
        left_out.push(
            "How audio was stretched to the tempo: it's sped up or slowed down to fit instead, which moves its pitch."
                .into(),
        );
    }
    if out.untimed {
        left_out.push("The speed of audio whose file couldn't be read: it plays at its own speed.".into());
    }
    if out.reversed {
        left_out.push("Reversed audio: it plays forwards.".into());
    }
    if automation {
        left_out.push("Automation.".into());
    }
    if layers {
        left_out.push("Takes that don't play (a track's other layers).".into());
    }
    if instruments {
        left_out.push("Instruments: each instrument track has none until one is chosen.".into());
    }
    if out.tracks > 0 {
        left_out.push("Effects and mixer settings.".into());
    }
    left_out.extend(song.notes.iter().cloned());
    let end = out.end;
    let song_end = if end > EMPTY_END - 16.0 { (end / 4.0).ceil() * 4.0 + 16.0 } else { EMPTY_END };
    let bytes = assemble(song, title, tempo, out.tracks, song_end, &out).unwrap_or_default();
    Written { bytes, left_out }
}

/// What the tracks add to each of the song's files.
#[derive(Default)]
struct Parts<'f> {
    /// The song's seconds a beat, and what an audio file is.
    spb: f64,
    format: Option<&'f FormatOf>,
    /// The tracks so far, and their channels in each group (instrument channels start at 2: the chord track's is 1).
    tracks: usize,
    music_channels: usize,
    audio_channels: usize,
    /// Where the last clip ends, in beats.
    end: f64,
    tracks_xml: String,
    /// `MusicTrackChannel`s and `AudioTrackChannel`s.
    music: String,
    mixer: String,
    /// The media pool's `MusicClip`s and `AudioClip`s.
    pool_music: String,
    pool_audio: String,
    /// The notes' files, by their names in the song.
    performances: Vec<(String, Vec<u8>)>,
    /// Each audio file's clip and what it is, by its path; each clip of notes', by the track and its notes.
    audio_ids: HashMap<String, (String, Option<SampleFormat>)>,
    music_ids: HashMap<(String, Vec<u8>), String>,
    /// What couldn't go across as it was.
    stretched: bool,
    untimed: bool,
    reversed: bool,
}

impl Parts<'_> {
    /// A track and its channel.
    fn track(&mut self, audio: bool, name: &str, lane: &crate::arrangement::PreviewLane, events: &str) {
        self.tracks += 1;
        let number = self.tracks;
        let (track_uid, channel) = (uid(), uid());
        let colour = format!("FF{:06X}", lane.colour.unwrap_or(0x2A2AFF) & 0xFF_FFFF);
        if audio {
            self.audio_channels += 1;
            self.mixer.push_str(&audio_channel(&channel, name, &colour, lane.muted, self.audio_channels));
        } else {
            self.music_channels += 1;
            self.music.push_str(&music_channel(&channel, name, &colour, lane.muted, self.music_channels + 1));
        }
        self.tracks_xml.push_str(&format!(
            "\t\t\t<MediaTrack mediaType=\"{}\" tempoFollow=\"{}\" materialMode=\"0\" trackNumber=\"{number}\" \
             layersFollow=\"1\" followLayerName=\"0\" followGlobalTransposition=\"0\" version=\"1\" \
             trackID=\"{track_uid}\" timeFormat=\"2\" name=\"{}\" color=\"{colour}\"{}>\n\
             {}\t\t\t\t<UID x:id=\"channelID\" uid=\"{channel}\"/>\n\
             \t\t\t\t<Attributes x:id=\"AutomationRegionList\" visible=\"0\">\n\t\t\t\t</Attributes>\n\
             \t\t\t\t<Attributes x:id=\"attributes\" scoreMode=\"0\"/>\n\
             \t\t\t\t<List x:id=\"Events\">\n{events}\t\t\t\t</List>\n\t\t\t</MediaTrack>\n",
            if audio { "Audio" } else { "Music" },
            if audio { 1 } else { 2 },
            attr(name),
            if lane.muted { " mute=\"1\"" } else { "" },
            if audio { "\t\t\t\t<SpeakerSetup x:id=\"trackFormat\" type=\"Stereo\"/>\n" } else { "" },
        ));
    }

    /// An instrument track's parts. A clip that starts before the song is moved to its start.
    fn music_events(&mut self, track: &str, clips: &[PreviewClip]) -> String {
        let mut events = String::new();
        for clip in clips {
            let start = if clip.start.is_finite() { clip.start.max(0.0) } else { 0.0 };
            let length = if clip.length.is_finite() { clip.length.max(0.0) } else { 0.0 };
            self.end = self.end.max(start + length);
            let clip_id = self.music_clip(track, clip);
            let name = if clip.name.trim().is_empty() { track } else { &clip.name };
            events.push_str(&format!(
                "\t\t\t\t\t<MusicPart transpose=\"0\" velocity=\"0\" clipID=\"{clip_id}\" start=\"{start}\" \
                 timeFormat=\"2\" length=\"{length}\" name=\"{}\"{}/>\n",
                attr(name),
                if clip.muted { " mute=\"1\"" } else { "" },
            ));
        }
        events
    }

    /// An audio track's events: one for each stretch of a file, in seconds (see the top of the file).
    fn audio_events(&mut self, track: &str, clips: &[PreviewClip]) -> String {
        let spb = self.spb;
        let mut events = String::new();
        for clip in clips {
            let name = if clip.name.trim().is_empty() { track } else { &clip.name };
            for part in &clip.audio {
                let at = clip.start + part.start;
                let at = if at.is_finite() { at.max(0.0) } else { 0.0 };
                let beats = if part.length.is_finite() { part.length.max(1.0 / 960.0) } else { 1.0 / 960.0 };
                self.end = self.end.max(at + beats);
                self.reversed |= part.reversed;
                let (clip_id, file) = self.audio_clip(&part.path);
                // Seconds of the file a beat plays: as the song said, or the whole file over the beats it fits.
                let rate = part.rate(file.map_or(0.0, |f| f.seconds));
                let speed = if rate > 0.0 && rate.is_finite() {
                    (rate / spb).clamp(0.01, 100.0)
                } else {
                    self.untimed |= part.whole_file_beats.is_some();
                    1.0
                };
                self.stretched |= (speed - 1.0).abs() > 1e-6 && !matches!(part.stretch, None | Some(Stretch::Raw));
                let (fade_in, fade_out) = clip.fades_of(part);
                let (fade_in, fade_out) = (fade_in * spb, fade_out * spb);
                let semitones = if part.semitones.is_finite() { part.semitones.clamp(-128.0, 128.0) } else { 0.0 };
                let (whole, cents) = (semitones.trunc(), ((semitones - semitones.trunc()) * 100.0).round());
                let from = if part.from.is_finite() { part.from.max(0.0) } else { 0.0 };
                events.push_str(&format!(
                    "\t\t\t\t\t<AudioEvent clipID=\"{clip_id}\" start=\"{at}\" timeFormat=\"0\" length=\"{}\" \
                     offset=\"{}\" name=\"{}\" speed=\"{speed}\" transpose=\"{whole}\" tune=\"{cents}\" \
                     normalize=\"0\" modification=\"0\"{}",
                    beats * spb,
                    from / speed,
                    attr(name),
                    if clip.muted { " mute=\"1\"" } else { "" },
                ));
                if fade_in > 0.0 || fade_out > 0.0 || part.gain_db != 0.0 {
                    let level = if part.gain_db.is_finite() { 10f64.powf(part.gain_db / 20.0) } else { 0.0 };
                    events.push_str(&format!(
                        ">\n\t\t\t\t\t\t<Attributes x:id=\"volumeCurve\" fadeIn.length=\"{}\" fadeOut.length=\"{}\" \
                         level=\"{level}\"/>\n\t\t\t\t\t</AudioEvent>\n",
                        fade_in.max(0.0),
                        fade_out.max(0.0)
                    ));
                } else {
                    events.push_str("/>\n");
                }
            }
        }
        events
    }

    /// The media pool's clip for an audio file (one for each file, however many events play it), and what the file
    /// is when it can be read.
    fn audio_clip(&mut self, path: &str) -> (String, Option<SampleFormat>) {
        if let Some((id, file)) = self.audio_ids.get(path) {
            return (id.clone(), *file);
        }
        let id = uid();
        let known = self.format.and_then(|f| f(path)).filter(|f| f.rate > 0 && f.channels > 0 && f.seconds > 0.0);
        let format = known.map_or_else(String::new, |f| {
            format!(
                "\t\t\t\t<Attributes x:id=\"format\" frameCount=\"{}\" sampleRate=\"{}\" numChannels=\"{}\"/>\n",
                (f.seconds * f64::from(f.rate)).round(),
                f.rate,
                f.channels
            )
        });
        self.pool_audio.push_str(&format!(
            "\t\t\t<AudioClip mediaID=\"{id}\" useCount=\"1\">\n\t\t\t\t<Url x:id=\"path\" type=\"1\" url=\"{}\"/>\n\
             {format}\t\t\t</AudioClip>\n",
            attr(&file_url(path))
        ));
        self.audio_ids.insert(path.to_owned(), (id.clone(), known));
        (id, known)
    }

    /// The media pool's clip for a clip's notes: the same notes on the same track share one.
    fn music_clip(&mut self, track: &str, clip: &PreviewClip) -> String {
        let notes = musicx(clip);
        let key = (track.to_owned(), notes);
        if let Some(id) = self.music_ids.get(&key) {
            return id.clone();
        }
        let id = uid();
        let folder = entry_name(track);
        let entry = format!("Performances/{folder}/{folder}({}).musicx", self.performances.len());
        self.pool_music.push_str(&format!(
            "\t\t\t<MusicClip mediaID=\"{id}\" useCount=\"1\">\n\t\t\t\t<Url x:id=\"dataPath\" type=\"1\" \
             url=\"media:///{}\"/>\n\t\t\t</MusicClip>\n",
            attr(&entry)
        ));
        self.performances.push((entry, key.1.clone()));
        self.music_ids.insert(key, id.clone());
        id
    }
}

/// An instrument track's channel, with no instrument.
fn music_channel(uid: &str, name: &str, colour: &str, muted: bool, number: usize) -> String {
    format!(
        "\t\t\t<MusicTrackChannel transpose=\"0\" velocity=\"0\" iconid=\"\" name=\"Channel{:02}\" label=\"{}\" \
         mute=\"{}\" solo=\"0\" soloSafe=\"0\" color=\"{colour}\">\n\
         \t\t\t\t<UID x:id=\"uniqueID\" uid=\"{uid}\"/>\n\
         \t\t\t\t<Attributes x:id=\"Inserts\">\n\t\t\t\t\t<Attributes x:id=\"Presets\" pname=\"default\" dirty=\"0\"/>\n\
         \t\t\t\t</Attributes>\n\t\t\t</MusicTrackChannel>\n",
        number,
        attr(name),
        u8::from(muted)
    )
}

/// An audio track's channel, to the main output, as recipe save 04's (without its recording input).
fn audio_channel(id: &str, name: &str, colour: &str, muted: bool, number: usize) -> String {
    let mut macros = String::new();
    for kind in ["float", "bool"] {
        for n in 1..=8 {
            macros.push_str(&format!("\t\t\t\t\t<Attributes x:id=\"{kind}-{n}\" color=\"0\"/>\n"));
        }
    }
    for pad in ["pad-x-1", "pad-y-1", "pad-x-2", "pad-y-2"] {
        macros.push_str(&format!("\t\t\t\t\t<Attributes x:id=\"{pad}\" color=\"0\"/>\n"));
    }
    format!(
        "\t\t\t<AudioTrackChannel gain=\"1\" pan=\"0.5\" iconid=\"\" name=\"Channel{number:02}\" label=\"{}\" \
         mute=\"{}\" \
         solo=\"0\" soloSafe=\"0\" color=\"{colour}\">\n\
         \t\t\t\t<SpeakerSetup x:id=\"speakerType\" type=\"Stereo\"/>\n\
         \t\t\t\t<UID x:id=\"uniqueID\" uid=\"{id}\"/>\n\
         \t\t\t\t<Connection x:id=\"destination\" objectID=\"{MAIN_OUT}/Input\" friendlyName=\"Main\"/>\n\
         \t\t\t\t<Attributes x:id=\"Inserts\">\n\t\t\t\t\t<Attributes x:id=\"Presets\" pname=\"default\" dirty=\"0\"/>\n\
         \t\t\t\t\t<Attributes x:id=\"Combinator\" name=\"Combinator\">\n\t\t\t\t\t\t<UID x:id=\"uniqueID\" uid=\"{}\"/>\n\
         \t\t\t\t\t</Attributes>\n\t\t\t\t</Attributes>\n\
         \t\t\t\t<Attributes x:id=\"Panner\" panX1=\"0.5\" panX2=\"0\" panWidth=\"1\" stereoMode=\"0\" \
         spatialObjectMode=\"0\" panMode=\"1\" name=\"Panner\">\n\t\t\t\t\t<UID x:id=\"uniqueID\" uid=\"{}\"/>\n\
         \t\t\t\t</Attributes>\n\
         \t\t\t\t<Attributes x:id=\"Sends\">\n\t\t\t\t\t<Attributes x:id=\"Presets\" pname=\"default\" dirty=\"0\"/>\n\
         \t\t\t\t</Attributes>\n\
         \t\t\t\t<Attributes x:id=\"InputFX\" gain=\"0\" invertPhaseL=\"0\" invertPhaseR=\"0\"/>\n\
         \t\t\t\t<Attributes x:id=\"MacroControls\" showMacroPads=\"0\">\n{macros}\t\t\t\t</Attributes>\n\
         \t\t\t</AudioTrackChannel>\n",
        attr(name),
        u8::from(muted),
        uid(),
        uid(),
    )
}

/// A clip's notes as Studio One's `.musicx`: binary JSON, in beats from the part's start, velocity 0 to 1.
fn musicx(clip: &PreviewClip) -> Vec<u8> {
    let mut notes: Vec<_> = clip.notes.iter().filter(|n| n.start.is_finite() && n.length.is_finite()).collect();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    let mut b = Vec::new();
    b.push(b'{');
    key(&mut b, "timeFormat");
    int(&mut b, 2);
    key(&mut b, "events");
    b.push(b'[');
    for (i, n) in notes.iter().enumerate() {
        b.push(b'{');
        if n.start > 0.0 {
            key(&mut b, "start");
            float(&mut b, n.start);
        }
        key(&mut b, "pitch");
        int(&mut b, i64::from(n.key.min(127)));
        key(&mut b, "noteId");
        int(&mut b, i as i64 + 1);
        key(&mut b, "length");
        float(&mut b, step_length(clip, n).max(1.0 / 960.0));
        key(&mut b, "velocity");
        float(&mut b, f64::from(n.velocity) / f64::from(u16::MAX));
        if n.muted {
            key(&mut b, "mute");
            int(&mut b, 1);
        }
        b.push(b'}');
    }
    b.push(b']');
    key(&mut b, "envelopes");
    b.extend_from_slice(b"[]");
    b.push(b'}');
    b
}

/// How long a note plays. A step from FL's step sequencer has no length of its own: it plays a sixteenth, or up to the
/// next note on its key or the clip's end if that's sooner, as the MIDI writer gives it
/// ([`crate::midi::write::lengthen_steps`]); written as it is, an instrument would barely sound it.
fn step_length(clip: &PreviewClip, n: &PreviewNote) -> f64 {
    if !n.step {
        return n.length;
    }
    let next = clip
        .notes
        .iter()
        .filter(|o| o.key == n.key && !o.muted && o.start > n.start)
        .map(|o| o.start)
        .fold(clip.length, f64::min);
    n.length.max((next - n.start).min(0.25))
}

/// A binary JSON key: its length, then its letters.
fn key(b: &mut Vec<u8>, k: &str) {
    int(b, k.len() as i64);
    b.extend_from_slice(k.as_bytes());
}

/// A binary JSON whole number, in as few bytes as it fits, as Studio One writes them.
fn int(b: &mut Vec<u8>, n: i64) {
    if let Ok(v) = i8::try_from(n) {
        b.push(b'i');
        b.extend_from_slice(&v.to_be_bytes());
    } else if let Ok(v) = u8::try_from(n) {
        b.push(b'U');
        b.push(v);
    } else if let Ok(v) = i16::try_from(n) {
        b.push(b'I');
        b.extend_from_slice(&v.to_be_bytes());
    } else if let Ok(v) = i32::try_from(n) {
        b.push(b'l');
        b.extend_from_slice(&v.to_be_bytes());
    } else {
        b.push(b'L');
        b.extend_from_slice(&n.to_be_bytes());
    }
}

fn float(b: &mut Vec<u8>, f: f64) {
    b.push(b'D');
    b.extend_from_slice(&f.to_be_bytes());
}

/// The template's files, with the song's put in, as a new zip. None when the template won't open (it always does: it's
/// part of the app).
fn assemble(song: &Arrangement, title: &str, tempo: f64, tracks: usize, end: f64, parts: &Parts) -> Option<Vec<u8>> {
    let mut input = zip::ZipArchive::new(Cursor::new(TEMPLATE)).ok()?;
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for i in 0..input.len() {
        let mut entry = input.by_index(i).ok()?;
        let name = entry.name().ok()?.into_owned();
        let changed = match name.as_str() {
            "Devices/hardwarestoragemanager.xml" => Some(EMPTY_HARDWARE.to_owned()),
            "Song/song.xml"
            | "Song/mediapool.xml"
            | "Devices/musictrackdevice.xml"
            | "Devices/audiomixer.xml"
            | "metainfo.xml" => {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut entry, &mut text).ok()?;
                // Studio One ends its lines with CR LF: worked on with LF alone, then written back as it was.
                let text = text.replace("\r\n", "\n");
                Some(match name.as_str() {
                    "Song/song.xml" => song_xml(&text, song, tempo, end, &parts.tracks_xml)?,
                    "Song/mediapool.xml" => pool_xml(&text, parts)?,
                    "Devices/musictrackdevice.xml" => put_before_last(&text, "\t\t</ChannelGroup>", &parts.music)?,
                    "Devices/audiomixer.xml" if parts.mixer.is_empty() => text,
                    "Devices/audiomixer.xml" => put_before_last(
                        &text,
                        "\t</Attributes>\n\t<Attributes x:id=\"ListenBusManager\"",
                        &format!(
                            "\t\t<ChannelGroup name=\"AudioTrack\" flags=\"1\">\n{}\t\t</ChannelGroup>\n",
                            parts.mixer
                        ),
                    )?,
                    _ => meta_xml(&text, song, title, tempo, tracks, end),
                })
            }
            _ => None,
        };
        match changed {
            Some(text) => {
                drop(entry);
                out.start_file(name, options).ok()?;
                out.write_all(text.replace('\n', "\r\n").as_bytes()).ok()?;
            }
            None => {
                drop(entry);
                out.raw_copy_file(input.by_index_raw(i).ok()?).ok()?;
            }
        }
    }
    for (name, bytes) in &parts.performances {
        out.start_file(name.as_str(), options).ok()?;
        out.write_all(bytes).ok()?;
    }
    Some(out.finish().ok()?.into_inner())
}

/// `text` with `add` put in before the last `before`.
fn put_before_last(text: &str, before: &str, add: &str) -> Option<String> {
    let at = text.rfind(before)?;
    Some(format!("{}{add}{}", text.get(..at)?, text.get(at..)?))
}

/// The song's tempo, time signature, markers, end and tracks put into the empty song's `song.xml`.
fn song_xml(text: &str, song: &Arrangement, tempo: f64, end: f64, tracks: &str) -> Option<String> {
    let mut t = replace_value(text, "<EnvelopePoint value=\"", &((tempo - SLOWEST) / (FASTEST - SLOWEST)).to_string())?;
    t = replace_value(&t, " tempo=\"", &(60.0 / tempo).to_string())?;
    t = replace_value(&t, " numerator=\"", &song.signature.top().to_string())?;
    t = replace_value(&t, " denominator=\"", &song.signature.bottom().to_string())?;
    t = t.replace(
        "markerType=\"3\" start=\"600\" timeFormat=\"2\" name=\"End\"",
        &format!("markerType=\"3\" start=\"{end}\" timeFormat=\"2\" name=\"End\""),
    );
    let markers: String = song
        .markers
        .iter()
        .filter(|m| m.beat.is_finite() && m.beat >= 0.0)
        .enumerate()
        .map(|(n, m)| {
            // A marker with no name gets its number: one with none would be taken for one of Studio One's own.
            let name = if m.name.trim().is_empty() { format!("#{}", n + 1) } else { attr(&m.name) };
            format!(
                "\t\t\t\t<MarkerEvent markerStop=\"0\" markerType=\"0\" start=\"{}\" timeFormat=\"2\" name=\"{name}\"/>\n",
                m.beat,
            )
        })
        .collect();
    t = put_before_last(&t, "\t\t\t</MarkerTrack>", &markers)?;
    put_before_last(&t, "\t\t</List>", tracks)
}

/// The media pool with the song's clips, and without the folder the empty song was saved in.
fn pool_xml(text: &str, parts: &Parts) -> Option<String> {
    let mut folders = String::new();
    if !parts.pool_music.is_empty() {
        folders.push_str(&format!("\t\t<MediaFolder name=\"Music\">\n{}\t\t</MediaFolder>\n", parts.pool_music));
    }
    if !parts.pool_audio.is_empty() {
        folders.push_str(&format!("\t\t<MediaFolder name=\"Audio\">\n{}\t\t</MediaFolder>\n", parts.pool_audio));
    }
    let root = if folders.is_empty() {
        "<Attributes x:id=\"rootFolder\"/>".to_owned()
    } else {
        format!("<Attributes x:id=\"rootFolder\">\n{folders}\t</Attributes>")
    };
    let t = text.replacen("<Attributes x:id=\"rootFolder\"/>", &root, 1);
    let start = t.find("\t<Attributes x:id=\"documentPath\"")?;
    let stop = start + t.get(start..)?.find('\n')? + 1;
    Some(format!("{}{}", t.get(..start)?, t.get(stop..)?))
}

/// The song's details as Studio One lists them, without the empty song's artist.
fn meta_xml(text: &str, song: &Arrangement, title: &str, tempo: f64, tracks: usize, end: f64) -> String {
    let values = [
        ("Media:Artist", String::new()),
        ("Document:Creator", String::new()),
        ("Document:Title", title.to_owned()),
        ("Media:Tempo", tempo.to_string()),
        ("Media:TimeSignatureNumerator", song.signature.top().to_string()),
        ("Media:TimeSignatureDenominator", song.signature.bottom().to_string()),
        ("Media:TrackCount", tracks.to_string()),
        ("Media:Length", (end * 60.0 / tempo).to_string()),
    ];
    text.lines()
        .map(|line| {
            let set = values.iter().find(|(id, _)| line.contains(&format!("id=\"{id}\"")));
            match set {
                Some((id, value)) => format!("\t<Attribute id=\"{id}\" value=\"{}\"/>", attr(value)),
                None => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// `text` with the value after the first `before` (up to its closing quote) set to `value`.
fn replace_value(text: &str, before: &str, value: &str) -> Option<String> {
    let start = text.find(before)? + before.len();
    let stop = start + text.get(start..)?.find('"')?;
    Some(format!("{}{value}{}", text.get(..start)?, text.get(stop..)?))
}

/// Text as an XML attribute's value.
pub(crate) fn attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' | '\r' | '\t' => out.push(' '),
            c if c < ' ' => {}
            c => out.push(c),
        }
    }
    out
}

/// A name as a folder or file name inside the song.
fn entry_name(s: &str) -> String {
    let cleaned: String =
        s.chars()
            .map(|c| {
                if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '%') || c < ' ' { '_' } else { c }
            })
            .take(64)
            .collect();
    match cleaned.trim_matches([' ', '.']) {
        "" => "Track".to_owned(),
        name => name.to_owned(),
    }
}

/// A path as the media pool's address, as Studio One writes them (`file:///C:/Samples/Kick A.wav`, unescaped).
fn file_url(path: &str) -> String {
    // A `%` is written `%25`, so it isn't read as the start of an escape.
    let path = path.replace('%', "%25");
    if let Some(share) = path.strip_prefix(r"\\") {
        return format!("file://{}", share.replace('\\', "/"));
    }
    let p = path.replace('\\', "/");
    format!("file:///{}", p.trim_start_matches('/'))
}

/// A new id, as Studio One writes them ({8-4-4-4-12}, capitals).
fn uid() -> String {
    let b = crate::bitwig::encode::fresh_id();
    let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
    let part = |r: std::ops::Range<usize>| hex.get(r).unwrap_or_default().to_owned();
    format!("{{{}-{}-{}-{}-{}}}", part(0..8), part(8..12), part(12..16), part(16..20), part(20..32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_the_addresses_studio_one_writes() {
        assert_eq!(file_url(r"C:\Samples\Kick A.wav"), "file:///C:/Samples/Kick A.wav");
        assert_eq!(file_url("/Users/me/Kick.wav"), "file:///Users/me/Kick.wav");
        assert_eq!(file_url(r"\\NAS\Samples\Kick.wav"), "file://NAS/Samples/Kick.wav");
    }

    #[test]
    fn ids_look_like_studio_ones() {
        let id = uid();
        assert_eq!(id.len(), 38);
        assert!(id.starts_with('{') && id.ends_with('}'));
        assert_ne!(id, uid());
    }
}
