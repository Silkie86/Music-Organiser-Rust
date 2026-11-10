//! A Studio One song's arrangement as Studio One shows it, mapped from Solomon's songs (Studio One 6.5 and Studio Pro
//! 8). Ported from the C# app's `Parsers/StudioOneArrangement.cs`.
//!
//! `Song/song.xml` lists the tracks (MediaTrack, mediaType Music or Audio) and their events. An instrument part's notes
//! are in its clip's file (`Performances/*.musicx`, binary JSON, or `*.pattern` for a pattern part), found through
//! `Song/mediapool.xml` by clipID. Positions are in beats; an event whose timeFormat is 0 gives its length and offset
//! in seconds, and a track whose timeFormat is 0 (Tempo: Don't Follow) gives its events' starts in seconds too. The tempo
//! map holds seconds per beat, change by change, and turns those seconds into beats.
//!
//! Audio, as Solomon's recipe saves and his own songs showed (10 Nov 2026):
//!
//! - An audio track's `tempoFollow` is its Tempo setting: 0 Don't Follow, 1 Follow, 2 Timestretch. With Timestretch,
//!   `materialMode` is the Timestretch menu (0 Drums, 1 Solo, 2 Sound), and Tape is a `stretchMethod` UID instead.
//! - An event timed in beats is stretched: the file's own tempo (the media pool clip's AudioTempoMap, seconds per
//!   beat) says how much of the file a beat plays, times the event's `speed`. Only a file with a tempo is stretched; one
//!   without gets an event timed in seconds, which plays at the file's own speed whatever the track says.
//! - `transpose` is semitones and `tune` cents. The event's `volumeCurve` holds its gain (`level`, a plain multiple: 1
//!   is 0 dB) and its fades, in seconds.
//! - A track with layers (takes) has no Events list of its own: each layer's events are under `Layers`, and
//!   `activeLayer` says which one plays.
//! - An AudioPartEvent is an audio part: its events are in the media pool's AudioPartClip, timed from the part's start.
//! - A track's AutomationRegionList names its automation curves, each in a file of its own: `Envelopes/*.envelopex`
//!   (binary JSON: events with a start and a value from 0 to 1) or, before Studio One 4, `*.envelope` (the same as XML).
//!   A curve with a `stepcount` jumps from value to value. A volume's 1 is +10 dB (0 dB is 0.316). The tempo's curve is
//!   in song.xml's TempoMap, 0 to 1 for 10 to 400 BPM.
//! - `Devices/audiomixer.xml` has each channel's `disabled` and where it's sent; an instrument track reaches its
//!   instrument's channel through `Devices/musictrackdevice.xml`.

use std::collections::HashMap;

use roxmltree::Node;

use super::{Song, attr, elements, id, num, parse, unescape, url_to_path, velocity_of};
use crate::arrangement::{
    Arrangement, AudioPart, LaneKind, NoteBudget, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, PreviewPoint,
    Stretch, TOO_MANY_NOTES,
};
use crate::paths;

pub const VIEW: &str = "Studio One arrangement";

/// The most automation points a song's preview holds: far past any real song's, and a damaged file can't fill the
/// computer's memory with them.
const MOST_POINTS: usize = 500_000;
/// Said when a song reaches [`MOST_POINTS`].
const TOO_MANY_POINTS: &str =
    "This song has more automation than the preview holds (500,000 points), so only the first curves are drawn.";
/// The most events inside audio parts a song's preview looks at: each part looks at every event of its clip, so a
/// damaged song with many parts on one long clip could go on for hours.
const MOST_PART_EVENTS: usize = 2_000_000;
/// Said when a song reaches [`MOST_PART_EVENTS`].
const TOO_MANY_PART_EVENTS: &str =
    "This song's audio parts hold more events than the preview looks at (2 million), so the last parts are left out.";

/// What a part's clip id points at in the media pool: an audio file, the file in the song that holds the part's
/// notes, or an audio part's events.
#[derive(Default)]
struct Pool {
    audio: HashMap<String, PoolAudio>,
    music: HashMap<String, String>,
    parts: HashMap<String, AudioPartClip>,
}

/// An audio file in the media pool: its length in seconds (0 when not saved) and its own tempo as seconds per beat,
/// when Studio One knows it.
#[derive(Clone, Default)]
struct PoolAudio {
    path: String,
    seconds: f64,
    tempo: Option<f64>,
}

/// An audio part in the media pool: its events, their starts in seconds when `seconds`.
#[derive(Default)]
struct AudioPartClip {
    seconds: bool,
    events: Vec<AudioEvent>,
}

/// An audio event as the song writes it. `length` and `offset` are in seconds when `seconds`, else beats; `start` is
/// in whatever its track (or audio part) counts in.
#[derive(Clone, Default)]
struct AudioEvent {
    clip_id: String,
    start: f64,
    length: f64,
    offset: f64,
    seconds: bool,
    muted: bool,
    speed: f64,
    semitones: f64,
    gain_db: f64,
    /// Its fades, in seconds.
    fade_in: f64,
    fade_out: f64,
}

impl AudioEvent {
    fn read(ev: Node<'_, '_>) -> AudioEvent {
        let n = |name: &str| num(attr(ev, name)).filter(|v| v.is_finite()).unwrap_or(0.0);
        let curve = child(ev, "volumeCurve");
        let fade =
            |name: &str| curve.and_then(|c| num(attr(c, name))).filter(|v| v.is_finite()).map_or(0.0, |v| v.max(0.0));
        // The gain is a plain multiple of the sound: none saved is 1, and 0 is silence.
        let gain_db = match curve.and_then(|c| num(attr(c, "level"))) {
            Some(level) if level > 0.0 && level.is_finite() => 20.0 * level.log10(),
            Some(level) if level <= 0.0 => f64::NEG_INFINITY,
            _ => 0.0,
        };
        AudioEvent {
            clip_id: attr(ev, "clipID").to_lowercase(),
            start: n("start"),
            length: n("length"),
            offset: n("offset"),
            seconds: attr(ev, "timeFormat") == "0",
            muted: attr(ev, "mute") == "1",
            // A damaged song's speed (0, or a million) is left at 1: no real one is near either end.
            speed: num(attr(ev, "speed")).filter(|s| (0.01..=100.0).contains(s)).unwrap_or(1.0),
            // A damaged song's huge transpose is cut to a whole keyboard's worth, as a note part's is.
            semitones: (n("transpose") + n("tune") / 100.0).clamp(-128.0, 128.0),
            gain_db,
            fade_in: fade("fadeIn.length"),
            fade_out: fade("fadeOut.length"),
        }
    }

    /// How the event plays its file when it starts at the beat `at`. `stretch` is its track's Timestretch mode (none
    /// when the track doesn't stretch).
    fn played(&self, file: Option<&PoolAudio>, at: f64, map: &TempoMap, stretch: Option<Stretch>) -> Played {
        let spb = map.spb_at(at);
        // Timed in beats, the file is stretched so its own beats fall on the song's; timed in seconds it plays at its
        // own speed. Either way `speed` (the event's Speedup) plays it that many times faster.
        let (rate, stretch) = if self.seconds {
            (spb * self.speed, Some(Stretch::Raw))
        } else {
            (file.and_then(|f| f.tempo).unwrap_or(spb) * self.speed, stretch)
        };
        let mut length = map.span(at, self.length, self.seconds);
        // No length saved: the whole file.
        let whole = file.map_or(0.0, |f| f.seconds);
        if length <= 0.0 && whole > 0.0 {
            length = if self.seconds { map.span(at, whole / self.speed, true) } else { whole / rate };
        }
        let from = if self.seconds { self.offset * self.speed } else { self.offset * rate };
        Played { length, rate, from, stretch }
    }

    /// The stretch of its file the event plays, from the start of its clip.
    fn part(&self, file: Option<&PoolAudio>, played: &Played) -> AudioPart {
        AudioPart {
            path: file.map_or_else(String::new, |f| f.path.clone()),
            start: 0.0,
            length: played.length,
            from: played.from,
            seconds_per_beat: Some(played.rate),
            whole_file_beats: None,
            reversed: false,
            stretch: played.stretch,
            semitones: self.semitones,
            gain_db: self.gain_db,
        }
    }
}

/// How an audio event plays its file: for how many beats, how many seconds of the file a beat plays (`rate`), where
/// in the file it starts (seconds) and how it's stretched.
struct Played {
    length: f64,
    rate: f64,
    from: f64,
    stretch: Option<Stretch>,
}

/// The song's tempo changes, first to last: the beat and the second each starts at, and its seconds per beat. What a
/// song counts in seconds (a track that doesn't follow the tempo, an unstretched event's length, a fade) becomes
/// beats through it.
struct TempoMap {
    changes: Vec<(f64, f64, f64)>,
}

impl TempoMap {
    /// The song's TempoMapSegments; `fallback` is the seconds per beat of a song that has none.
    fn new(song: Node<'_, '_>, fallback: f64) -> TempoMap {
        let mut starts: Vec<(f64, f64)> = elements(song)
            .filter(|e| e.tag_name().name() == "TempoMapSegment")
            .filter_map(|t| {
                let spb = num(attr(t, "tempo")).filter(|t| *t > 0.05 && *t < 6.0)?;
                Some((num(attr(t, "start")).filter(|s| s.is_finite()).map_or(0.0, |s| s.max(0.0)), spb))
            })
            .collect();
        starts.sort_by(|a, b| a.0.total_cmp(&b.0));
        // The first tempo holds from the song's start, wherever it's written.
        let first = starts.first().map_or(fallback, |s| s.1);
        let (mut beat, mut second, mut spb) = (0.0, 0.0, first);
        let mut changes = vec![(beat, second, spb)];
        for (start, tempo) in starts {
            second += (start - beat) * spb;
            (beat, spb) = (start, tempo);
            changes.push((beat, second, spb));
        }
        TempoMap { changes }
    }

    fn change(&self, after: usize) -> (f64, f64, f64) {
        // There's always a first change, at the song's start.
        self.changes.get(after.saturating_sub(1)).copied().unwrap_or((0.0, 0.0, 0.5))
    }

    /// The seconds per beat at a beat.
    fn spb_at(&self, beat: f64) -> f64 {
        self.change(self.changes.partition_point(|c| c.0 <= beat)).2
    }

    /// The second a beat falls on.
    fn seconds(&self, beat: f64) -> f64 {
        let (from, second, spb) = self.change(self.changes.partition_point(|c| c.0 <= beat));
        second + (beat - from) * spb
    }

    /// The beat a second falls on.
    fn beats(&self, second: f64) -> f64 {
        let (beat, from, spb) = self.change(self.changes.partition_point(|c| c.1 <= second));
        beat + (second - from) / spb
    }

    /// How many beats an amount takes from the beat `from` on: itself, or counted in seconds, what the tempo makes of
    /// it there.
    fn span(&self, from: f64, amount: f64, seconds: bool) -> f64 {
        if seconds { self.beats(self.seconds(from) + amount) - from } else { amount }
    }

    /// How many beats the seconds before the beat `to` take.
    fn span_before(&self, to: f64, seconds: f64) -> f64 {
        to - self.beats(self.seconds(to) - seconds)
    }
}

/// The child of an element with this `x:id`.
fn child<'a, 'i>(e: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    e.children().find(|c| c.is_element() && id(*c) == name)
}

/// The file in the song a `media:///` address points at.
fn media_entry(url: &str) -> Option<String> {
    url.get(..9).filter(|s| s.eq_ignore_ascii_case("media:///")).and(url.get(9..)).map(unescape)
}

/// A number in a binary JSON object.
fn number(o: &[(String, Ubjson)], key: &str) -> Option<f64> {
    o.iter().find(|(k, _)| k == key).and_then(|(_, v)| match v {
        Ubjson::Float(f) => Some(*f),
        Ubjson::Int(i) => Some(*i as f64),
        _ => None,
    })
}

/// What a track plays through: the instrument an instrument track plays (empty for an audio track), whether its
/// mixer channel in `Devices/audiomixer.xml` is disabled, and the bus that's sent to (empty for the main output).
#[derive(Clone, Default)]
struct Channel {
    instrument: String,
    disabled: bool,
    to: String,
}

/// Studio One's own id for Tape, the Timestretch mode that changes the pitch with the speed.
const TAPE: &str = "{B442AEFC-7B3C-497B-A22E-630E00B1B908}";

/// An audio track's Timestretch mode by Bitwig's names (none when its Tempo setting isn't Timestretch, or the mode is
/// one the reader doesn't know).
fn stretch_of(track: Node<'_, '_>) -> Option<Stretch> {
    if attr(track, "tempoFollow") != "2" {
        return None;
    }
    let method = track.children().find(|c| c.is_element() && id(*c) == "stretchMethod").map(|m| attr(m, "uid"));
    match (method, attr(track, "materialMode")) {
        (Some(uid), _) if uid.eq_ignore_ascii_case(TAPE) => Some(Stretch::Repitch),
        (Some(_), _) => None,
        // Drums and Sound are both zplane's Elastique (Sound keeps the formants); Solo is its one-voice mode.
        (None, "0" | "2") => Some(Stretch::Elastique),
        (None, "1") => Some(Stretch::ElastiqueSolo),
        _ => None,
    }
}

impl Song<'_> {
    /// The song as Studio One's arrangement shows it. One whose song.xml can't be read says so instead.
    pub fn arrangement(&mut self) -> Arrangement {
        let Some(text) = self.xml("Song/song.xml") else { return unreadable() };
        let Ok(doc) = parse(&text) else { return unreadable() };
        let song = doc.root_element();

        let seconds_per_beat = elements(song)
            .filter(|e| e.tag_name().name() == "TempoMapSegment")
            .filter_map(|t| num(attr(t, "tempo")))
            .find(|t| *t > 0.05 && *t < 6.0);
        let meta_tempo = self.xml("metainfo.xml").and_then(|text| {
            let meta = parse(&text).ok()?;
            elements(meta.root_element()).skip(1).find(|e| id(*e) == "Media:Tempo").map(|e| num(attr(e, "value")))
        });
        let tempo = match (seconds_per_beat, meta_tempo.flatten()) {
            (Some(spb), _) => 60.0 / spb,
            (None, Some(t)) if (10.0..=999.0).contains(&t) => t,
            _ => 120.0,
        };
        let spb = seconds_per_beat.unwrap_or(60.0 / tempo);
        let beats_per_bar = elements(song).find(|e| e.tag_name().name() == "TimeSignatureMapSegment").map_or(4, |s| {
            let numerator = num(attr(s, "numerator")).unwrap_or(4.0);
            let denominator = num(attr(s, "denominator")).unwrap_or(4.0);
            (numerator * 4.0 / denominator).round_ties_even().max(1.0).min(u32::MAX as f64) as u32
        });

        let pool = self.pool();
        let channels = self.channels();
        let map = TempoMap::new(song, spb);
        // The tempo shown is the one the song starts in, wherever its segment is written.
        let tempo = if seconds_per_beat.is_some() { 60.0 / map.spb_at(0.0) } else { tempo };
        let n = |e: Node<'_, '_>, name: &str| num(attr(e, name)).filter(|v| v.is_finite()).unwrap_or(0.0);
        // Where something starts, in beats.
        let place = |e: Node<'_, '_>, seconds: bool| if seconds { map.beats(n(e, "start")) } else { n(e, "start") };
        let in_seconds = |e: Node<'_, '_>| attr(e, "timeFormat") == "0";

        let mut lanes = Vec::new();
        let mut budget = NoteBudget::new();
        let (mut points_held, mut too_many_points) = (0usize, false);
        let mut part_events_left = MOST_PART_EVENTS;
        // Each curve file's points, read once however many regions name it.
        let mut curves: HashMap<String, Vec<PreviewPoint>> = HashMap::new();
        let mut too_many_part_events = false;
        // The tempo's own curve, when it changes: first, as Studio One's tempo track is.
        let tempo_points: Vec<PreviewPoint> = elements(song)
            .find(|e| e.tag_name().name() == "TempoMap")
            .and_then(|map| child(map, "envelope"))
            .map(|envelope| xml_points(envelope).take(MOST_POINTS).collect())
            .unwrap_or_default();
        points_held += tempo_points.len();
        lanes.extend(curve_lane("Tempo", "Song tempo", 0, tempo_points));
        let top = elements(song)
            .find(|l| l.tag_name().name() == "List" && id(*l) == "Tracks")
            .or_else(|| song.children().find(|c| c.is_element() && c.tag_name().name() == "Tracks"));
        // The tracks left to walk in each folder, last first, with how deep the folder is.
        let mut stack: Vec<(Vec<Node<'_, '_>>, u32)> = Vec::new();
        if let Some(list) = top {
            stack.push((list.children().filter(Node::is_element).collect::<Vec<_>>().into_iter().rev().collect(), 0));
        }
        while let Some((left, depth)) = stack.last_mut() {
            let depth = *depth;
            let Some(track) = left.pop() else {
                stack.pop();
                continue;
            };
            let kind = match track.tag_name().name() {
                "MediaTrack" if attr(track, "mediaType") == "Audio" => LaneKind::Audio,
                "MediaTrack" | "MusicTrack" | "InstrumentTrack" => LaneKind::Instrument,
                "AudioTrack" => LaneKind::Audio,
                "FolderTrack" => LaneKind::Group,
                // A track that only holds a bus or effect channel's automation.
                "AutomationTrack" => LaneKind::Automation,
                _ => continue,
            };
            let lists = |name: &'static str| {
                track.children().filter(move |l| l.is_element() && l.tag_name().name() == "List" && id(*l) == name)
            };
            let mut clips = Vec::new();
            // A track that doesn't follow the tempo (timeFormat 0) counts its events' starts in seconds.
            let track_seconds = attr(track, "timeFormat") == "0";
            let stretch = stretch_of(track);
            // A track with layers (takes) keeps every layer's events under Layers, and plays the active one. Its own
            // Events list is missing, or there and empty.
            let mut event_lists: Vec<Node<'_, '_>> = lists("Events").collect();
            if !event_lists.iter().any(|l| l.children().any(|c| c.is_element())) {
                let active = match attr(track, "activeLayer") {
                    "" => "0",
                    layer => layer,
                };
                event_lists = lists("Layers")
                    .flat_map(|l| l.children().filter(|a| a.is_element() && id(*a) == active))
                    .flat_map(|a| a.children().filter(|l| l.is_element() && id(*l) == "Events"))
                    .collect();
            }
            for ev in event_lists.into_iter().flat_map(|l| l.children().filter(Node::is_element)) {
                let name = attr(ev, "name");
                let start = place(ev, track_seconds);
                let seconds = in_seconds(ev);
                let mut length = map.span(start, n(ev, "length"), seconds);
                let offset = map.span(start, n(ev, "offset"), seconds);
                let muted = attr(ev, "mute") == "1";
                let clip_id = attr(ev, "clipID");
                match ev.tag_name().name() {
                    "AudioEvent" => {
                        let event = AudioEvent::read(ev);
                        let file = pool.audio.get(&event.clip_id);
                        let played = event.played(file, start, &map, stretch);
                        let part = event.part(file, &played);
                        let length = played.length;
                        let name = if name.is_empty() {
                            paths::stem(paths::file_name(&part.path)).to_owned()
                        } else {
                            name.to_owned()
                        };
                        // The fades are in seconds; together they can't be longer than the clip.
                        let fade_in = map.span(start, event.fade_in, true).min(length.max(0.0));
                        let fade_out = map.span_before(start + length, event.fade_out).min((length - fade_in).max(0.0));
                        clips.push(PreviewClip {
                            name,
                            start,
                            length,
                            muted,
                            fade_in,
                            fade_out,
                            audio: vec![part],
                            ..PreviewClip::default()
                        });
                    }
                    "AudioPartEvent" => {
                        // An audio part: the events are the pool's, timed from the part's own start, and the part
                        // shows them from `offset` on for its length.
                        let Some(inside) = pool.parts.get(&clip_id.to_lowercase()) else { continue };
                        let Some(left) = part_events_left.checked_sub(inside.events.len()) else {
                            too_many_part_events = true;
                            continue;
                        };
                        part_events_left = left;
                        // Where the part's own time starts: `offset` before the part does.
                        let origin = start - offset;
                        let inner: Vec<_> = inside
                            .events
                            .iter()
                            .map(|event| {
                                let begins = origin + map.span(origin, event.start, inside.seconds);
                                let file = pool.audio.get(&event.clip_id);
                                (begins - start, event, file, event.played(file, begins, &map, stretch))
                            })
                            .collect();
                        // No length saved: the part runs to the end of its last event.
                        if length <= 0.0 {
                            let ends = inner.iter().map(|(at, _, _, played)| at + played.length);
                            length = ends.filter(|e| e.is_finite()).reduce(f64::max).unwrap_or(0.0).max(0.0);
                        }
                        let mut audio = Vec::new();
                        for (at, event, file, played) in inner {
                            // Cut to what the part shows of it.
                            let cut = (-at).max(0.0);
                            let shown = (at + played.length).min(length) - at.max(0.0);
                            if event.muted || shown.is_nan() || shown <= 0.0 {
                                continue;
                            }
                            let mut part = event.part(file, &played);
                            part.start = at.max(0.0);
                            part.length = shown;
                            part.from += cut * played.rate;
                            audio.push(part);
                        }
                        audio.sort_by(|a, b| a.start.total_cmp(&b.start));
                        let name = if name.is_empty() { attr(track, "name") } else { name };
                        clips.push(PreviewClip {
                            name: name.to_owned(),
                            start,
                            length,
                            muted,
                            audio,
                            ..PreviewClip::default()
                        });
                    }
                    "MusicPart" | "NotePart" => {
                        // A damaged song's huge transpose ("1e10") is cut to a whole keyboard's worth either way.
                        let transpose = num(attr(ev, "transpose")).unwrap_or(0.0).clamp(-128.0, 128.0) as i32;
                        let all = match pool.music.get(&clip_id.to_lowercase()) {
                            Some(file) => self.musicx_notes(file),
                            None => inline_notes(ev),
                        };
                        let from_offset = |n: &&PreviewNote| n.start >= offset - 1e-9;
                        // No length saved: the part runs to the end of its last note.
                        if length <= 0.0 {
                            length = all
                                .iter()
                                .filter(from_offset)
                                .map(|n| n.start + n.length - offset)
                                .reduce(f64::max)
                                .unwrap_or(4.0);
                        }
                        let notes = all
                            .iter()
                            .filter(from_offset)
                            .filter(|n| n.start < offset + length)
                            .take_while(|_| budget.take())
                            .map(|n| PreviewNote {
                                start: n.start - offset,
                                length: n.length.min(offset + length - n.start),
                                key: (n.key as i32 + transpose).clamp(0, 127) as u8,
                                ..*n
                            })
                            .collect();
                        let name = if name.is_empty() { attr(track, "name") } else { name };
                        clips.push(PreviewClip {
                            name: name.to_owned(),
                            start,
                            length,
                            muted,
                            notes,
                            ..PreviewClip::default()
                        });
                    }
                    "MusicPatternPart" => {
                        let (steps, loop_beats) = match pool.music.get(&clip_id.to_lowercase()) {
                            Some(file) => self.pattern_notes(file),
                            None => (Vec::new(), 0.0),
                        };
                        let mut notes = Vec::new();
                        if loop_beats > 0.0 && length.is_finite() {
                            let mut at = 0.0;
                            // Stops when the arrangement holds all the notes it can, however short the loop.
                            // A pass that places nothing ends it too: `at` only grows, so no later pass can (a
                            // damaged pattern's steps all past the part, or a loop too short to move `at` on).
                            'passes: while at < length - 1e-9 {
                                let mut placed = false;
                                for n in steps.iter().filter(|n| at + n.start < length) {
                                    if !budget.take() {
                                        break 'passes;
                                    }
                                    notes.push(PreviewNote { start: at + n.start, ..*n });
                                    placed = true;
                                }
                                let next = at + loop_beats;
                                if !placed || next <= at {
                                    break;
                                }
                                at = next;
                            }
                        }
                        let name = if name.is_empty() { "Pattern" } else { name };
                        clips.push(PreviewClip {
                            name: name.to_owned(),
                            start,
                            length,
                            muted,
                            notes,
                            ..PreviewClip::default()
                        });
                    }
                    _ => {}
                }
            }
            clips.sort_by(|a, b| a.start.total_cmp(&b.start));
            let name = match attr(track, "name") {
                "" if kind == LaneKind::Audio => "Audio",
                "" if kind == LaneKind::Automation => "Automation",
                "" => "Instrument",
                n => n,
            };
            let mut lane = PreviewLane::new(name, kind);
            lane.colour = colour(attr(track, "color"));
            lane.muted = attr(track, "mute") == "1";
            let channel_id = child(track, "channelID").map(|c| attr(c, "uid"));
            if let Some(channel) = channel_id.and_then(|uid| channels.get(&uid.to_lowercase())) {
                // A disabled track doesn't play, so it's muted too.
                lane.disabled = channel.disabled;
                lane.muted |= channel.disabled;
                // The instrument, unless the track is simply named after it.
                let instrument = Some(channel.instrument.as_str()).filter(|i| !i.eq_ignore_ascii_case(name));
                lane.detail = match (instrument.unwrap_or_default(), channel.to.as_str()) {
                    (i, "") => i.to_owned(),
                    ("", to) => format!("To {to}"),
                    (i, to) => format!("{i}, to {to}"),
                };
            }
            lane.depth = depth;
            lane.clips = clips;
            // Its automation curves, each a lane under it. An automation track is nothing but its curves.
            let moves = format!("Moves {}", lane.name);
            let silent = lane.muted;
            if kind != LaneKind::Automation {
                lanes.push(lane);
            }
            let regions = track
                .children()
                .filter(|l| l.is_element() && id(*l) == "AutomationRegionList")
                .flat_map(|l| l.children().filter(|r| r.is_element() && r.tag_name().name() == "AutomationRegion"));
            for region in regions {
                if too_many_points {
                    break;
                }
                let Some(entry) = child(region, "points").and_then(|u| media_entry(attr(u, "url"))) else { continue };
                let room = MOST_POINTS - points_held;
                let mut points = match curves.get(&entry) {
                    Some(read) => read.clone(),
                    None => {
                        let read = self.envelope(&entry);
                        curves.insert(entry, read.clone());
                        read
                    }
                };
                if points.len() > room {
                    too_many_points = true;
                    continue;
                }
                points_held += points.len();
                // Counted in seconds like the track's events (no save has shown such a curve yet).
                if track_seconds {
                    points.iter_mut().for_each(|p| p.beat = map.beats(p.beat));
                }
                let title = match attr(region, "title") {
                    "" => "Automation",
                    t => t,
                };
                if let Some(mut curve) = curve_lane(title, &moves, depth + 1, points) {
                    curve.colour = colour(attr(region, "color"));
                    // A muted or disabled track's curves move nothing.
                    curve.muted = silent || attr(region, "mute") == "1";
                    lanes.push(curve);
                }
            }
            if kind == LaneKind::Group
                && let Some(inner) = lists("Tracks").next()
            {
                stack.push((
                    inner.children().filter(Node::is_element).collect::<Vec<_>>().into_iter().rev().collect(),
                    depth + 1,
                ));
            }
        }

        // Markers the person placed: not the song's own start and end (types 2 and 3).
        let mut markers: Vec<PreviewMarker> = elements(song)
            .filter(|m| m.tag_name().name() == "MarkerEvent")
            .filter(|m| !matches!(attr(*m, "markerType"), "2" | "3") && !attr(*m, "name").is_empty())
            .map(|m| PreviewMarker { beat: place(m, in_seconds(m)), name: attr(m, "name").to_owned() })
            .collect();
        markers.sort_by(|a, b| a.beat.total_cmp(&b.beat));

        let mut notes = Vec::new();
        if lanes.iter().filter(|l| l.kind != LaneKind::Automation).all(|l| l.clips.is_empty()) {
            notes.push("Nothing is on this song's timeline yet.".to_owned());
        }
        if budget.ran_out {
            notes.push(TOO_MANY_NOTES.to_owned());
        }
        if too_many_points {
            notes.push(TOO_MANY_POINTS.to_owned());
        }
        if too_many_part_events {
            notes.push(TOO_MANY_PART_EVENTS.to_owned());
        }
        Arrangement { view: VIEW, tempo, beats_per_bar, lanes, markers, notes }
    }

    /// What each clip id in the media pool points at (ids by lower case: Studio One's don't care about letter case).
    fn pool(&mut self) -> Pool {
        let mut pool = Pool::default();
        let Some(text) = self.xml("Song/mediapool.xml") else { return pool };
        let Ok(doc) = parse(&text) else { return pool };
        for clip in elements(doc.root_element()).skip(1) {
            let clip_id = attr(clip, "mediaID");
            if clip_id.is_empty() {
                continue;
            }
            let children =
                |name: &'static str| clip.children().filter(move |c| c.is_element() && c.tag_name().name() == name);
            let url = children("Url").map(|u| attr(u, "url")).next().unwrap_or_default();
            match clip.tag_name().name() {
                "AudioClip" => {
                    let format = children("Attributes").find(|a| id(*a) == "format");
                    let frames = format.and_then(|f| num(attr(f, "frameCount")));
                    let rate = format.and_then(|f| num(attr(f, "sampleRate")));
                    let seconds = match (frames, rate) {
                        (Some(f), Some(r)) if f > 0.0 && r > 0.0 => f / r,
                        _ => 0.0,
                    };
                    // The file's own tempo, as seconds per beat like the song's (0.05 to 6 is 1,200 to 10 BPM).
                    let tempo = children("AudioTempoMap")
                        .flat_map(|m| m.children().filter(|s| s.is_element()))
                        .filter(|s| s.tag_name().name() == "AudioTempoMapSegment")
                        .filter_map(|s| num(attr(s, "tempo")))
                        .find(|t| *t > 0.05 && *t < 6.0);
                    pool.audio.insert(clip_id.to_lowercase(), PoolAudio { path: url_to_path(url), seconds, tempo });
                }
                "AudioPartClip" => {
                    let Some(events) = children("Attributes").find(|a| id(*a) == "Events") else { continue };
                    let part = AudioPartClip {
                        seconds: attr(events, "timeFormat") == "0",
                        events: events
                            .children()
                            .filter(|e| e.is_element() && e.tag_name().name() == "AudioEvent")
                            .map(AudioEvent::read)
                            .collect(),
                    };
                    pool.parts.insert(clip_id.to_lowercase(), part);
                }
                _ => {
                    if let Some(entry) = media_entry(url) {
                        pool.music.insert(clip_id.to_lowercase(), entry);
                    }
                }
            }
        }
        pool
    }

    /// Each track's mixer channel by the track's channel id (lower case): an audio track's own, and for an instrument
    /// track its instrument's, reached through the track's channel in `Devices/musictrackdevice.xml`.
    fn channels(&mut self) -> HashMap<String, Channel> {
        let mut channels = HashMap::new();
        let Some(text) = self.xml("Devices/audiomixer.xml") else { return channels };
        let Ok(doc) = parse(&text) else { return channels };
        let uid = |e: Node<'_, '_>| child(e, "uniqueID").map(|u| attr(u, "uid").to_lowercase());
        let is_channel = |e: &Node<'_, '_>| e.tag_name().name().ends_with("Channel");
        // The main output takes what isn't sent to a bus.
        let outputs: Vec<String> = elements(doc.root_element())
            .filter(|e| e.tag_name().name() == "AudioOutputChannel")
            .filter_map(uid)
            .collect();
        for e in elements(doc.root_element()).filter(is_channel) {
            let Some(uid) = uid(e) else { continue };
            let to = child(e, "destination").filter(|d| {
                let target = attr(*d, "objectID").split('/').next().unwrap_or_default().to_lowercase();
                !outputs.contains(&target)
            });
            let channel = Channel {
                instrument: String::new(),
                disabled: attr(e, "disabled") == "1",
                to: to.map_or_else(String::new, |d| attr(d, "friendlyName").to_owned()),
            };
            channels.insert(uid, channel);
        }
        // Each instrument's name, by the id its note input goes by.
        let mut instruments: HashMap<String, String> = HashMap::new();
        if let Some(text) = self.xml("Devices/audiosynthfolder.xml")
            && let Ok(doc) = parse(&text)
        {
            for data in elements(doc.root_element()).filter(|e| id(*e) == "deviceData") {
                if let Some(uid) = uid(data) {
                    instruments.insert(uid, attr(data, "name").trim().to_owned());
                }
            }
        }
        // An instrument track's channel is in the track device, and points at its instrument's mixer channel.
        if let Some(text) = self.xml("Devices/musictrackdevice.xml")
            && let Ok(doc) = parse(&text)
        {
            for e in elements(doc.root_element()).filter(is_channel) {
                let out = child(e, "instrumentOut").map(|c| attr(c, "objectID").split('/').next().unwrap_or_default());
                let Some(uid) = uid(e) else { continue };
                let mut channel = out.and_then(|o| channels.get(&o.to_lowercase()).cloned()).unwrap_or_default();
                // Its notes go to the instrument's input. The instrument's own name is with its id; an outside
                // instrument (a hardware synth) has none there, so it goes by its input's name, "Mai Tai/Synth Input",
                // without the number Studio One puts before it in older songs ("7 - Analog Rytm MKII/CH 14").
                let to = child(e, "destination");
                let target = to.map_or("", |d| attr(d, "objectID")).split('/').next().unwrap_or_default();
                channel.instrument = match instruments.get(&target.to_lowercase()).filter(|name| !name.is_empty()) {
                    Some(name) => name.clone(),
                    None => {
                        let input = to.map_or("", |d| attr(d, "friendlyName"));
                        let device = input.rsplit_once('/').map_or(input, |(device, _)| device);
                        let numbered = device.split_once(" - ").filter(|(n, _)| n.bytes().all(|b| b.is_ascii_digit()));
                        numbered.map_or(device, |(_, name)| name).trim().to_owned()
                    }
                };
                channel.disabled |= attr(e, "disabled") == "1";
                channels.entry(uid).or_insert(channel);
            }
        }
        channels
    }

    /// An automation curve's points from its file in the song: `.envelopex` is binary JSON
    /// ({stepcount, events: [{start, value}]}; a point with no start is at 0), the older `.envelope` the same as XML. A
    /// curve with a stepcount jumps, so each point is held until the next. None when the file is missing or damaged.
    fn envelope(&mut self, entry: &str) -> Vec<PreviewPoint> {
        let (mut points, steps): (Vec<PreviewPoint>, bool) =
            if crate::plugins::ends_with_ignore_case(entry, ".envelopex") {
                let Some(bytes) = self.entry(entry) else { return Vec::new() };
                let Some(Ubjson::Object(root)) = Ubjson::read(&bytes) else { return Vec::new() };
                let steps = number(&root, "stepcount").is_some();
                let Some((_, Ubjson::List(events))) = root.iter().find(|(k, _)| k == "events") else {
                    return Vec::new();
                };
                let points = events.iter().take(MOST_POINTS).filter_map(|e| match e {
                    Ubjson::Object(o) => point(number(o, "start").unwrap_or(0.0), number(o, "value")),
                    _ => None,
                });
                (points.collect(), steps)
            } else {
                let Some(text) = self.xml(entry) else { return Vec::new() };
                let Ok(doc) = parse(&text) else { return Vec::new() };
                let root = doc.root_element();
                (xml_points(root).take(MOST_POINTS).collect(), root.has_attribute("stepcount"))
            };
        if !steps {
            return points;
        }
        // Held in time order, whatever order the file has them in.
        points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let mut held = Vec::with_capacity(points.len() * 2);
        for p in points {
            if let Some(&PreviewPoint { value, .. }) = held.last() {
                held.push(PreviewPoint { beat: p.beat, value });
            }
            held.push(p);
        }
        held
    }

    /// A file in the song, whole, or none.
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        let i = *self.index.get(name)?;
        self.zip.by_index(i).ok().and_then(crate::zipped::read_whole)
    }

    /// A part's notes from its .musicx file: binary JSON holding {timeFormat, events: [{start, pitch, length,
    /// velocity, mute}]}, in beats; a note with no start is at 0.
    fn musicx_notes(&mut self, entry: &str) -> Vec<PreviewNote> {
        let Some(bytes) = self.entry(entry) else { return Vec::new() };
        let Some(Ubjson::Object(root)) = Ubjson::read(&bytes) else { return Vec::new() };
        let field = |o: &[(String, Ubjson)], key: &str| o.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let Some(Ubjson::List(events)) = field(&root, "events") else { return Vec::new() };
        // In seconds: not seen in Solomon's songs, so left out rather than drawn in the wrong place.
        if matches!(field(&root, "timeFormat"), Some(Ubjson::Int(0))) {
            return Vec::new();
        }
        events
            .iter()
            .filter_map(|e| match e {
                Ubjson::Object(o) => Some(o),
                _ => None,
            })
            .map(|o| {
                let n = |key: &str, fallback: f64| number(o, key).unwrap_or(fallback);
                PreviewNote {
                    start: n("start", 0.0),
                    length: n("length", 0.25),
                    key: (n("pitch", 60.0) as i32).clamp(0, 127) as u8,
                    velocity: velocity_of(Some(n("velocity", 0.8))),
                    muted: n("mute", 0.0) != 0.0,
                    step: false,
                    part: 0,
                }
            })
            .collect()
    }

    /// A pattern part's first variation: its steps as notes, and how many beats it takes before it repeats.
    fn pattern_notes(&mut self, entry: &str) -> (Vec<PreviewNote>, f64) {
        let Some(text) = self.xml(entry) else { return (Vec::new(), 0.0) };
        let Ok(doc) = parse(&text) else { return (Vec::new(), 0.0) };
        let Some(p) = elements(doc.root_element()).find(|e| e.tag_name().name() == "PatternPerformance") else {
            return (Vec::new(), 0.0);
        };
        let step = num(attr(p, "stepLength")).filter(|s| *s > 0.0).unwrap_or(0.25);
        let count = num(attr(p, "stepCount")).unwrap_or(16.0);
        // Steps past the pattern's length (left from when it was longer) aren't played.
        let notes = p
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "PatternStep")
            .filter(|n| num(attr(*n, "start")).unwrap_or(0.0) < count)
            .map(|n| PreviewNote {
                start: num(attr(n, "start")).unwrap_or(0.0) * step,
                length: step,
                key: (num(attr(n, "pitch")).unwrap_or(36.0) as i32).clamp(0, 127) as u8,
                velocity: velocity_of(num(attr(n, "velocity"))),
                muted: attr(n, "mute") == "1",
                // A pattern step has its length (the step's), unlike FL Studio's.
                step: false,
                part: 0,
            })
            .collect();
        (notes, count * step)
    }
}

/// A point of a curve, when it has a value and both numbers are real ones: the value kept between 0 and 1.
fn point(start: f64, value: Option<f64>) -> Option<PreviewPoint> {
    let value = value?;
    (start.is_finite() && value.is_finite()).then(|| PreviewPoint { beat: start, value: value.clamp(0.0, 1.0) })
}

/// The EnvelopePoints written straight under an element (not its EnvelopeAnchor, which marks the song's end).
fn xml_points<'a>(envelope: Node<'a, '_>) -> impl Iterator<Item = PreviewPoint> + 'a {
    envelope
        .children()
        .filter(|p| p.is_element() && p.tag_name().name() == "EnvelopePoint")
        .filter_map(|p| point(num(attr(p, "start")).unwrap_or(0.0), num(attr(p, "value"))))
}

/// An automation curve as a lane with one clip from its first point to its last, or none when it has fewer than two
/// points (a setting that never moves).
fn curve_lane(name: &str, detail: &str, depth: u32, mut points: Vec<PreviewPoint>) -> Option<PreviewLane> {
    if points.len() < 2 {
        return None;
    }
    points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let start = points.first()?.beat;
    let length = (points.last()?.beat - start).max(1.0);
    points.iter_mut().for_each(|p| p.beat -= start);
    let mut lane = PreviewLane::new(name, LaneKind::Automation);
    lane.detail = detail.to_owned();
    lane.depth = depth;
    lane.clips = vec![PreviewClip { name: name.to_owned(), start, length, points, ..PreviewClip::default() }];
    Some(lane)
}

fn unreadable() -> Arrangement {
    crate::arrangement::unreadable(VIEW, "This song")
}

/// Studio One's colours are AARRGGBB in hex; the preview wants 0xRRGGBB.
fn colour(argb: &str) -> Option<u32> {
    if argb.len() != 8 {
        return None;
    }
    let rgb = argb.get(2..)?;
    rgb.bytes().all(|b| b.is_ascii_hexdigit()).then(|| u32::from_str_radix(rgb, 16).ok()).flatten()
}

/// Notes written straight into a part (as some older songs, and the tests, do).
fn inline_notes(part: Node<'_, '_>) -> Vec<PreviewNote> {
    elements(part)
        .skip(1)
        .filter(|n| n.tag_name().name() == "NoteEvent")
        .map(|n| PreviewNote {
            start: num(attr(n, "start")).unwrap_or(0.0),
            length: num(attr(n, "length")).unwrap_or(0.25),
            key: (num(attr(n, "pitch")).unwrap_or(60.0) as i32).clamp(0, 127) as u8,
            velocity: velocity_of(num(attr(n, "velocity"))),
            muted: attr(n, "mute") == "1",
            step: false,
            part: 0,
        })
        .collect()
}

/// Universal Binary JSON, as Studio One writes it: objects, lists, numbers and text, big-endian.
#[derive(Clone, Debug, PartialEq)]
enum Ubjson {
    Object(Vec<(String, Ubjson)>),
    List(Vec<Ubjson>),
    Text(String),
    Float(f64),
    Int(i64),
    Bool(bool),
    Null,
}

/// How deep objects and lists may nest: a damaged file mustn't go round until the app runs out of stack.
const MOST_DEPTH: u32 = 64;

impl Ubjson {
    /// The value the bytes hold, or none when they're damaged.
    fn read(d: &[u8]) -> Option<Ubjson> {
        let mut at = 0;
        Ubjson::value(d, &mut at, 0)
    }

    fn take<'a>(d: &'a [u8], at: &mut usize, n: usize) -> Option<&'a [u8]> {
        let bytes = d.get(*at..at.checked_add(n)?)?;
        *at += n;
        Some(bytes)
    }

    fn value(d: &[u8], at: &mut usize, depth: u32) -> Option<Ubjson> {
        if depth > MOST_DEPTH {
            return None;
        }
        let kind = *d.get(*at)?;
        *at += 1;
        Some(match kind {
            b'{' => {
                let mut o = Vec::new();
                while *d.get(*at)? != b'}' {
                    let key = Ubjson::text(d, at)?;
                    o.push((key, Ubjson::value(d, at, depth + 1)?));
                }
                *at += 1;
                Ubjson::Object(o)
            }
            b'[' => {
                let mut l = Vec::new();
                while *d.get(*at)? != b']' {
                    l.push(Ubjson::value(d, at, depth + 1)?);
                }
                *at += 1;
                Ubjson::List(l)
            }
            b'S' => Ubjson::Text(Ubjson::text(d, at)?),
            b'D' => Ubjson::Float(f64::from_be_bytes(Ubjson::take(d, at, 8)?.try_into().ok()?)),
            b'd' => Ubjson::Float(f32::from_be_bytes(Ubjson::take(d, at, 4)?.try_into().ok()?) as f64),
            b'Z' => Ubjson::Null,
            b'T' => Ubjson::Bool(true),
            b'F' => Ubjson::Bool(false),
            _ => {
                *at -= 1;
                Ubjson::Int(Ubjson::int(d, at)?)
            }
        })
    }

    fn int(d: &[u8], at: &mut usize) -> Option<i64> {
        let kind = *d.get(*at)?;
        *at += 1;
        Some(match kind {
            b'i' => i8::from_be_bytes(Ubjson::take(d, at, 1)?.try_into().ok()?) as i64,
            b'U' => *Ubjson::take(d, at, 1)?.first()? as i64,
            b'I' => i16::from_be_bytes(Ubjson::take(d, at, 2)?.try_into().ok()?) as i64,
            b'l' => i32::from_be_bytes(Ubjson::take(d, at, 4)?.try_into().ok()?) as i64,
            b'L' => i64::from_be_bytes(Ubjson::take(d, at, 8)?.try_into().ok()?),
            _ => return None,
        })
    }

    fn text(d: &[u8], at: &mut usize) -> Option<String> {
        let length = usize::try_from(Ubjson::int(d, at)?).ok()?;
        Some(String::from_utf8_lossy(Ubjson::take(d, at, length)?).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::velocity;

    #[test]
    fn reads_ubjson_as_studio_one_writes_it() {
        // {"timeFormat": 1, "events": [{"start": 0.5, "pitch": 60}]}
        let mut b = vec![b'{', b'i', 10];
        b.extend(b"timeFormat");
        b.extend([b'U', 1, b'i', 6]);
        b.extend(b"events");
        b.extend([b'[', b'{', b'i', 5]);
        b.extend(b"start");
        b.push(b'D');
        b.extend(0.5f64.to_be_bytes());
        b.extend([b'i', 5]);
        b.extend(b"pitch");
        b.extend([b'I', 0, 60, b'}', b']', b'}']);
        let note = Ubjson::Object(vec![("start".into(), Ubjson::Float(0.5)), ("pitch".into(), Ubjson::Int(60))]);
        assert_eq!(
            Ubjson::read(&b),
            Some(Ubjson::Object(vec![
                ("timeFormat".into(), Ubjson::Int(1)),
                ("events".into(), Ubjson::List(vec![note])),
            ]))
        );
        // Cut short anywhere, or nested too deep, it's damaged, not a crash.
        for end in 0..b.len() {
            assert_eq!(Ubjson::read(&b[..end]), None, "{end}");
        }
        assert_eq!(Ubjson::read(&[b'['; 200]), None);
        assert_eq!(Ubjson::read(b"x"), None);
    }

    #[test]
    fn colours_and_velocities() {
        assert_eq!(colour("FF3366CC"), Some(0x33_66CC));
        assert_eq!(colour("3366CC"), None);
        assert_eq!(colour("FF3366ZZ"), None);
        assert_eq!(velocity_of(Some(1.0)), velocity::from_unit(1.0));
        assert_eq!(velocity_of(Some(0.503)), velocity::from_unit(0.503), "16 bits kept");
        assert_eq!(velocity_of(Some(100.0)), velocity::from_7(100));
        assert_eq!(velocity_of(Some(f64::NAN)), velocity::from_7(1));
        assert_eq!(velocity_of(None), velocity::from_unit(0.8));
    }
}
