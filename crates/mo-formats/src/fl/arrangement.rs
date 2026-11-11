//! An FL Studio project's arrangement, two ways: its playlist as FL shows it (tracks holding pattern, audio and
//! automation clips), and one track per channel, the way "Open in Bitwig" lays it out. Ported from the C# app's
//! `Preview/FlArrangement.cs` and `Parsers/FlStudioPlaylist.cs`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::events::{
    EV_CHANNEL_NAME_OLD, EV_CHANNEL_TYPE, EV_PATTERN_NAME, EV_PATTERN_NEW, EV_PATTERN_NOTES, EV_PLAYLIST,
    EV_PLUGIN_NAME, Event, Flp, byte_at, u32_at,
};
use super::notes;
use super::song::{
    CHANNEL_TYPE_AUTOMATION, EV_ARRANGEMENT_NEW, EV_TRACK_DATA, EV_TRACK_NAME, FlChannel, FlSong, FlStretch,
    PATTERN_BASE, playlist_items, read_song_with, top_slot,
};
use super::{Rack, channels as known_channels};
use crate::arrangement::{
    Arrangement, AudioPart, LaneKind, NoteBudget, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, PreviewPoint,
    Stretch, TOO_MANY_NOTES,
};
use crate::fl::automation::{FlPoint, cut_curve};
use crate::paths::file_name;

pub const PLAYLIST_VIEW: &str = "FL Studio playlist";
pub const BITWIG_VIEW: &str = "As it opens in Bitwig";

/// Beats in a bar (one byte).
const EV_TIME_SIG_NUMERATOR: u8 = 17;
/// A channel's colour, 0x00BBGGRR.
const EV_CHANNEL_COLOUR: u8 = 128;
/// A marker's position in ticks (the top bits say what kind of marker).
const EV_TIME_MARKER: u8 = 148;
/// A pattern's colour, 0x00BBGGRR.
const EV_PATTERN_COLOUR: u8 = 150;
/// A marker's name, after its position.
const EV_MARKER_NAME: u8 = 205;

/// FL's default playlist track colour, which isn't worth showing.
const DEFAULT_TRACK_COLOUR: u32 = 0x485156;
/// FL's default channel colour.
const DEFAULT_CHANNEL_COLOUR: u32 = 0x5C656A;

/// Both views of the project's arrangement: its playlist, then the tracks it becomes in Bitwig.
pub fn read(flp: &Flp<'_>) -> Vec<Arrangement> {
    let song = read_song_with(flp, false);
    let playlist = read_playlist(flp);
    let tempo = if song.tempo > 0.0 { song.tempo } else { 120.0 };
    let mut views = [playlist_view(&song, &playlist, tempo), by_channel(&song, &playlist, tempo)];
    // Notes left out above MIDI's keys, or damaged note events, in both views.
    let trouble = super::notes::trouble(flp);
    for view in &mut views {
        view.notes.extend(trouble.iter().cloned());
    }
    views.into()
}

/// The song's beats in a bar, and its first arrangement's markers in beats, as [`read`]'s playlist view gives them:
/// for a reader that needs no more of the playlist (one pass over the events, no notes read).
pub fn bars_and_markers(flp: &Flp<'_>) -> (u32, Vec<PreviewMarker>) {
    let mut timeline = Timeline::new();
    for e in &flp.events {
        timeline.step(flp, e);
    }
    (timeline.beats_per_bar, markers(&timeline.markers, flp.ppq))
}

/// The song's beats in a bar and its first arrangement's markers (ticks, name), found event by event.
struct Timeline {
    beats_per_bar: u32,
    markers: Vec<(i64, String)>,
    arrangements: u32,
}

impl Timeline {
    fn new() -> Timeline {
        Timeline { beats_per_bar: 4, markers: Vec::new(), arrangements: 0 }
    }

    /// Still in the first arrangement: the next one repeats its playlist, tracks and markers.
    fn first(&self) -> bool {
        self.arrangements <= 1
    }

    fn step(&mut self, flp: &Flp<'_>, e: &Event<'_>) {
        let first = self.first();
        match e.id {
            EV_TIME_SIG_NUMERATOR if (1..=32).contains(&e.int()) => self.beats_per_bar = e.int(),
            EV_ARRANGEMENT_NEW => self.arrangements += 1,
            EV_TIME_MARKER if first => self.markers.push(((e.int() & 0x07FF_FFFF) as i64, String::new())),
            EV_MARKER_NAME if first => {
                if let Some(m) = self.markers.last_mut() {
                    m.1 = flp.text(&e.data);
                }
            }
            _ => {}
        }
    }
}

/// Markers in beats, from ticks.
fn markers(markers: &[(i64, String)], ticks_per_beat: u16) -> Vec<PreviewMarker> {
    markers
        .iter()
        .map(|(at, name)| PreviewMarker { beat: *at as f64 / ppq(ticks_per_beat), name: name.clone() })
        .collect()
}

/// FL's 0x00BBGGRR colour as 0xRRGGBB.
fn rgb(bgr: u32) -> u32 {
    (bgr & 0xFF) << 16 | (bgr & 0xFF00) | (bgr >> 16 & 0xFF)
}

/// Ticks per beat: a damaged header can say 0, so FL's default stands in.
fn ppq(ppq: u16) -> f64 {
    if ppq > 0 { ppq as f64 } else { 96.0 }
}

/// A playlist track: its place from 0, its name when it was given one, its colour, and whether it's muted.
#[derive(Default)]
struct Track {
    name: String,
    colour: Option<u32>,
    muted: bool,
}

/// A pattern: its name, colour (when it has one) and its notes, by channel, in time order.
struct Pattern {
    name: String,
    colour: Option<u32>,
    notes: Vec<(u32, crate::NoteEvent)>,
}

/// A clip in the playlist, in song ticks: a pattern clip starts `offset` ticks into its pattern; an audio or automation
/// clip has its channel, and `offset` as saved.
struct Placement {
    track: u32,
    start: i64,
    length: i64,
    offset: f64,
    muted: bool,
    pattern: Option<u32>,
    channel: Option<u32>,
    /// An audio clip's own fades, in seconds.
    fades: (f64, f64),
}

/// A clip's fades in beats, from seconds: together they can't be longer than the clip.
fn fades((fade_in, fade_out): (f64, f64), length: f64, tempo: f64) -> (f64, f64) {
    let fade_in = (fade_in * tempo / 60.0).clamp(0.0, length.max(0.0));
    (fade_in, (fade_out * tempo / 60.0).clamp(0.0, (length - fade_in).max(0.0)))
}

/// The playlist as FL Studio shows it (its first arrangement).
struct Playlist {
    ppq: u16,
    beats_per_bar: u32,
    tracks: BTreeMap<u32, Track>,
    patterns: BTreeMap<u32, Pattern>,
    items: Vec<Placement>,
    markers: Vec<(i64, String)>,
    channel_colours: HashMap<u32, u32>,
    channel_names: HashMap<u32, String>,
    automation_channels: HashSet<u32>,
}

fn read_playlist(flp: &Flp<'_>) -> Playlist {
    let known = known_channels(flp);
    let mut timeline = Timeline::new();
    let mut channel_colours = HashMap::new();
    let mut channel_names = HashMap::new();
    let mut automation_channels = HashSet::new();
    let mut pattern_names: HashMap<u32, String> = HashMap::new();
    let mut pattern_colours: HashMap<u32, u32> = HashMap::new();
    let mut raw_notes: BTreeMap<u32, Vec<(u32, crate::NoteEvent)>> = BTreeMap::new();
    let mut tracks: BTreeMap<u32, Track> = BTreeMap::new();
    let mut playlist: Option<&[u8]> = None;
    let (mut pattern, mut track): (Option<u32>, Option<u32>) = (None, None);
    let mut rack = Rack::default();

    for e in &flp.events {
        rack.step(e);
        let channel = rack.channel();
        // Only the first arrangement: the next one repeats its playlist, tracks and markers.
        let first = timeline.first();
        timeline.step(flp, e);
        match e.id {
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD => {
                if let Some(c) = channel {
                    channel_names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_CHANNEL_TYPE if e.int() == CHANNEL_TYPE_AUTOMATION => {
                if let Some(c) = channel {
                    automation_channels.insert(c);
                }
            }
            EV_CHANNEL_COLOUR => {
                if let Some(c) = channel {
                    channel_colours.entry(c).or_insert(rgb(e.int()));
                }
            }
            EV_PATTERN_NEW => pattern = Some(e.int()),
            EV_PATTERN_NAME => {
                if let Some(p) = pattern {
                    pattern_names.insert(p, flp.text(&e.data));
                }
            }
            EV_PATTERN_COLOUR => {
                if let Some(p) = pattern {
                    pattern_colours.insert(p, rgb(e.int()));
                }
            }
            EV_PATTERN_NOTES => {
                if let Some(p) = pattern {
                    let size = notes::note_size(&e.data, (flp.major, flp.minor), e.offset.is_none(), Some(&known));
                    raw_notes.entry(p).or_default().extend(notes::decode(&e.data, size));
                }
            }
            EV_PLAYLIST if playlist.is_none() => playlist = Some(&e.data),
            EV_TRACK_DATA if first => {
                // Settings too short to read: the name that follows isn't put on the track before.
                track = if e.data.len() >= 8 { u32_at(&e.data, 0).checked_sub(1) } else { None };
                if let Some(t) = track {
                    // The byte at 12 is 1 when the track plays and 0 when it's muted.
                    let muted = e.data.len() > 12 && byte_at(&e.data, 12) == 0;
                    tracks.insert(t, Track { name: String::new(), colour: Some(rgb(u32_at(&e.data, 4))), muted });
                }
            }
            EV_TRACK_NAME if first => {
                if let Some(t) = track.and_then(|t| tracks.get_mut(&t)) {
                    t.name = flp.text(&e.data);
                }
            }
            _ => {}
        }
    }

    let mut patterns: BTreeMap<u32, Pattern> = raw_notes
        .into_iter()
        .map(|(p, mut notes)| {
            // In time order, so each placement finds its notes by a search, not by looking at every one.
            notes.sort_by_key(|(_, n)| n.start);
            let name = pattern_names.get(&p).cloned().unwrap_or_else(|| format!("Pattern {p}"));
            (p, Pattern { name, colour: pattern_colours.get(&p).copied(), notes })
        })
        .collect();
    for (p, name) in pattern_names {
        patterns.entry(p).or_insert_with(|| Pattern {
            name,
            colour: pattern_colours.get(&p).copied(),
            notes: Vec::new(),
        });
    }

    let placed = playlist.map(playlist_items).unwrap_or_default();
    let top = top_slot(&flp.version, placed.iter().map(|i| i.slot).max().unwrap_or(0));
    let mut placed: Vec<_> = placed.into_iter().filter(|i| i.length > 0).collect();
    placed.sort_by_key(|i| i.at);
    let items = placed
        .into_iter()
        .map(|i| {
            let track = u32::try_from((top - i.slot).max(0)).unwrap_or(0);
            if i.index > PATTERN_BASE {
                Placement {
                    track,
                    start: i.at,
                    length: i.length,
                    offset: i.offset,
                    muted: i.muted,
                    pattern: Some(i.index - PATTERN_BASE),
                    channel: None,
                    fades: (0.0, 0.0),
                }
            } else {
                Placement {
                    track,
                    start: i.at,
                    length: i.length,
                    offset: i.offset,
                    muted: i.muted,
                    pattern: None,
                    channel: Some(i.index),
                    fades: (i.fade_in / 1000.0, i.fade_out / 1000.0),
                }
            }
        })
        .collect();

    Playlist {
        ppq: flp.ppq,
        beats_per_bar: timeline.beats_per_bar,
        tracks,
        patterns,
        items,
        markers: timeline.markers,
        channel_colours,
        channel_names,
        automation_channels,
    }
}

/// Each channel's name as a pattern clip's part ([`PreviewClip::parts`]), kept apart from every other channel's: two
/// channels named alike ("Perc" and "Perc") would otherwise go into one MIDI track when a track is dragged out of the
/// app, so the one further down the Channel rack is "Perc 2". A channel with no name is "Channel 3", by its place.
fn part_names(channels: &HashMap<u32, &FlChannel>, pl: &Playlist) -> HashMap<u32, String> {
    let mut indices: Vec<u32> = channels.keys().chain(pl.channel_names.keys()).copied().collect();
    indices.sort_unstable();
    indices.dedup();
    let mut used = HashSet::new();
    let mut names = HashMap::new();
    for ch in indices {
        let base = channels
            .get(&ch)
            .map(|c| c.name.trim())
            .filter(|n| !n.is_empty())
            .or_else(|| pl.channel_names.get(&ch).map(|n| n.trim()).filter(|n| !n.is_empty()))
            .map_or_else(|| format!("Channel {}", ch + 1), str::to_owned);
        let mut name = base.clone();
        let mut n = 2;
        // Letter case aside, as a DAW's track names and Windows' file names go.
        while !used.insert(name.to_lowercase()) {
            name = format!("{base} {n}");
            n += 1;
        }
        names.insert(ch, name);
    }
    names
}

/// A channel's place among a pattern clip's parts, adding it if it's new. A clip with more channels than a part number
/// holds (65,535) puts the rest on the last.
fn part_of(parts: &mut Vec<u32>, channel: u32) -> u16 {
    let at = parts.iter().position(|&c| c == channel).unwrap_or_else(|| {
        parts.push(channel);
        parts.len() - 1
    });
    u16::try_from(at).unwrap_or(u16::MAX)
}

fn channel_colour(colours: &HashMap<u32, u32>, channel: u32) -> Option<u32> {
    colours.get(&channel).copied().filter(|&c| c != DEFAULT_CHANNEL_COLOUR)
}

fn playlist_view(song: &FlSong, pl: &Playlist, tempo: f64) -> Arrangement {
    let beats = |ticks: f64| ticks / ppq(pl.ppq);
    let channels: HashMap<u32, &FlChannel> = song.channels.iter().map(|c| (c.index, c)).collect();
    let part_names = part_names(&channels, pl);
    let mut by_track: BTreeMap<u32, Vec<&Placement>> = BTreeMap::new();
    for item in &pl.items {
        by_track.entry(item.track).or_default().push(item);
    }
    let last = by_track.keys().next_back().copied();
    let mut lanes = Vec::new();
    let mut notes = Vec::new();
    let mut budget = NoteBudget::new();

    // Every track up to the last with a clip on it; named tracks below that too.
    let numbers: BTreeSet<u32> = by_track
        .keys()
        .copied()
        .chain(pl.tracks.iter().filter(|(t, info)| !info.name.is_empty() && Some(**t) <= last).map(|(t, _)| *t))
        .collect();
    for t in numbers {
        let info = pl.tracks.get(&t);
        let mut clips = Vec::new();
        let mut kinds = BTreeSet::new();
        for item in by_track.get(&t).map(Vec::as_slice).unwrap_or_default() {
            let start = beats(item.start as f64);
            let length = beats(item.length as f64);
            if let Some(p) = item.pattern {
                kinds.insert(LaneKind::Instrument as u8);
                let pattern = pl.patterns.get(&p);
                let from = item.offset as i64;
                let end = from.saturating_add(item.length);
                // The channels the clip's notes are on, in the order they first play: the clip's parts.
                let mut parts: Vec<u32> = Vec::new();
                let in_clip = pattern
                    .map(|pat| {
                        let first = pat.notes.partition_point(|(_, n)| n.start < from);
                        pat.notes
                            .get(first..)
                            .unwrap_or_default()
                            .iter()
                            .take_while(|(_, n)| n.start < end)
                            .take_while(|_| budget.take())
                            .map(|(ch, n)| PreviewNote {
                                start: beats((n.start - from) as f64),
                                length: beats(n.length.max(1).min(end - n.start) as f64),
                                key: n.key,
                                velocity: n.velocity,
                                muted: n.muted,
                                step: n.step,
                                part: part_of(&mut parts, *ch),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                clips.push(PreviewClip {
                    name: pattern.map_or_else(|| format!("Pattern {p}"), |pat| pat.name.clone()),
                    start,
                    length,
                    muted: item.muted,
                    notes: in_clip,
                    colour: pattern.and_then(|pat| pat.colour),
                    parts: parts
                        .iter()
                        .map(|ch| part_names.get(ch).cloned().unwrap_or_else(|| format!("Channel {}", ch + 1)))
                        .collect(),
                    ..PreviewClip::default()
                });
            } else if let Some(ch) = item.channel.filter(|c| pl.automation_channels.contains(c)) {
                kinds.insert(LaneKind::Automation as u8);
                let name = pl.channel_names.get(&ch).cloned().unwrap_or_else(|| format!("Automation {}", ch + 1));
                // The clip's own curve, from as far into it as this placement starts.
                let points = song.curves.get(&ch).map(|c| cut(c, beats(item.offset), length)).unwrap_or_default();
                clips.push(PreviewClip {
                    name,
                    start,
                    length,
                    muted: item.muted,
                    points,
                    colour: channel_colour(&pl.channel_colours, ch),
                    ..PreviewClip::default()
                });
            } else if let Some(c) = item.channel.and_then(|ch| channels.get(&ch)) {
                kinds.insert(LaneKind::Audio as u8);
                let audio =
                    if c.sample.is_empty() { Vec::new() } else { vec![part(c, length, item.offset / 1000.0, tempo)] };
                let (fade_in, fade_out) = fades(item.fades, length, tempo);
                clips.push(PreviewClip {
                    name: c.name.clone(),
                    start,
                    length,
                    muted: item.muted,
                    fade_in,
                    fade_out,
                    audio,
                    colour: channel_colour(&pl.channel_colours, c.index),
                    ..PreviewClip::default()
                });
            }
        }
        // An empty track only shows if it was named: an old project's clips can sit a hundred tracks down.
        let name = info.map(|i| i.name.as_str()).unwrap_or_default();
        if clips.is_empty() && name.is_empty() {
            continue;
        }
        let kind = match kinds.iter().next() {
            Some(&k) if kinds.len() == 1 && k == LaneKind::Audio as u8 => LaneKind::Audio,
            Some(&k) if kinds.len() == 1 && k == LaneKind::Automation as u8 => LaneKind::Automation,
            _ => LaneKind::Instrument,
        };
        let mut lane =
            PreviewLane::new(if name.is_empty() { format!("Track {}", t + 1) } else { name.to_owned() }, kind);
        lane.colour = info.and_then(|i| i.colour).filter(|&c| c != DEFAULT_TRACK_COLOUR);
        lane.muted = info.is_some_and(|i| i.muted);
        lane.detail = describe(&clips);
        lane.clips = clips;
        lanes.push(lane);
    }
    if budget.ran_out {
        notes.push(TOO_MANY_NOTES.to_owned());
    }
    if pl.items.iter().all(|i| i.pattern.is_none()) && pl.patterns.values().any(|p| !p.notes.is_empty()) {
        notes.push(
            "No patterns are in the playlist: FL Studio plays them in pattern mode. \"As it opens in Bitwig\" lays \
             them out one after another."
                .to_owned(),
        );
    }
    Arrangement {
        view: PLAYLIST_VIEW,
        tempo,
        beats_per_bar: pl.beats_per_bar,
        lanes,
        markers: markers(&pl.markers, pl.ppq),
        notes,
    }
}

/// What a playlist track holds, as a line under its name: its patterns' or clips' names.
fn describe(clips: &[PreviewClip]) -> String {
    let mut names: Vec<&str> = Vec::new();
    for c in clips {
        if !names.contains(&c.name.as_str()) {
            names.push(&c.name);
        }
    }
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, rest @ ..] => format!("{first} + {} more", rest.len()),
    }
}

fn by_channel(song: &FlSong, pl: &Playlist, tempo: f64) -> Arrangement {
    let beats = |ticks: i64| ticks as f64 / ppq(song.ppq);
    let mut lanes = Vec::new();
    // As the converter makes them: every channel with a plugin or sample gets a track, even one the playlist never
    // plays, except an audio clip channel with no notes, whose clips go on audio tracks.
    let audio: HashSet<u32> =
        song.channels.iter().filter(|c| !c.audio_clips.is_empty() && !c.sample.is_empty()).map(|c| c.index).collect();
    for c in &song.channels {
        let playing = (!c.notes.is_empty() || !c.clips.is_empty() || !audio.contains(&c.index))
            && (c.instrument.is_some() || !c.sample.is_empty());
        let colour = channel_colour(&pl.channel_colours, c.index);
        if !c.clips.is_empty() || playing {
            let mut lane = PreviewLane::new(c.name.clone(), LaneKind::Instrument);
            lane.detail = match (&c.instrument, c.sample.is_empty()) {
                (Some(i), _) => i.name.clone(),
                (None, false) => format!("Sampler · {}", file_name(&c.sample)),
                (None, true) => String::new(),
            };
            lane.colour = colour;
            lane.clips = c
                .clips
                .iter()
                .map(|k| PreviewClip {
                    name: k.name.clone(),
                    start: beats(k.start),
                    length: beats(k.length),
                    // On a muted playlist track it doesn't play either.
                    muted: k.muted || k.on_muted_track,
                    notes: k
                        .notes
                        .iter()
                        .map(|n| PreviewNote {
                            start: beats(n.start),
                            length: beats(n.length.max(1)),
                            key: n.key,
                            velocity: n.velocity,
                            muted: n.muted,
                            step: n.step,
                            part: 0,
                        })
                        .collect(),
                    ..PreviewClip::default()
                })
                .collect();
            lanes.push(lane);
        }
        if !c.audio_clips.is_empty() {
            let mut lane = PreviewLane::new(c.name.clone(), LaneKind::Audio);
            lane.detail = if c.sample.is_empty() {
                "Sample not saved in the project".to_owned()
            } else {
                file_name(&c.sample).to_owned()
            };
            lane.colour = colour;
            lane.clips = c
                .audio_clips
                .iter()
                .map(|k| {
                    let (fade_in, fade_out) = fades((k.fade_in, k.fade_out), beats(k.length), tempo);
                    PreviewClip {
                        name: c.name.clone(),
                        start: beats(k.start),
                        length: beats(k.length),
                        muted: k.muted || k.on_muted_track,
                        fade_in,
                        fade_out,
                        audio: if c.sample.is_empty() {
                            Vec::new()
                        } else {
                            vec![part(c, beats(k.length), k.from, tempo)]
                        },
                        ..PreviewClip::default()
                    }
                })
                .collect();
            lanes.push(lane);
        }
    }
    for a in &song.automation {
        let (Some(first), Some(last)) = (a.points.first(), a.points.last()) else { continue };
        let start = first.beat;
        let length = (last.beat - start).max(1.0);
        let target = a.channel.and_then(|ch| song.channels.iter().find(|c| c.index == ch)).map(|c| c.name.as_str());
        let mut lane = PreviewLane::new(a.name.clone(), LaneKind::Automation);
        lane.detail = target.map_or_else(|| format!("{:?}", a.control), |t| format!("Moves {t}"));
        lane.clips = vec![PreviewClip {
            name: a.name.clone(),
            start,
            length,
            points: cut(&a.points, start, length),
            ..PreviewClip::default()
        }];
        lanes.push(lane);
    }
    let mut notes = Vec::new();
    if song.channels.iter().any(|c| !c.audio_clips.is_empty() && c.sample.is_empty()) {
        notes.push("Some audio clips are left out: FL Studio didn't save which sample they play.".to_owned());
    }
    // The lanes come from the song, which stops placing notes or points at its limit.
    notes.extend(song.warning.map(str::to_owned));
    Arrangement { view: BITWIG_VIEW, tempo, beats_per_bar: pl.beats_per_bar, lanes, markers: Vec::new(), notes }
}

/// A channel's sample playing in a clip `beats` long, from `from` seconds in, stretched as the channel says.
fn part(c: &FlChannel, beats: f64, from: f64, tempo: f64) -> AudioPart {
    let s: &FlStretch = &c.stretch;
    // Without the Time knob, how far the sample is stretched doesn't depend on its length; with it, the whole sample
    // is fitted to a length, so how fast it moves is only known once the file's length is.
    let (seconds_per_beat, whole_file_beats) = if s.time > 0.0 {
        (None, Some(s.played_seconds(0.0, tempo) * tempo / 60.0))
    } else {
        (Some(60.0 / tempo / s.played_seconds(1.0, tempo).max(1e-6)), None)
    };
    AudioPart {
        path: c.sample.clone(),
        start: 0.0,
        length: beats,
        from,
        seconds_per_beat,
        whole_file_beats,
        reversed: s.reversed,
        stretch: Some(stretch_of(s)),
        // The converter's own rule. Resampled to a set length, the pitch isn't known until the file's length is.
        semitones: if s.mode == 0 && s.time > 0.0 { 0.0 } else { s.semitones(1.0, s.played_seconds(1.0, tempo)) },
        ..Default::default()
    }
}

/// FL's stretch mode by the shared names. The numbers are FL's own (the sampler settings' mode): Solomon's FL 25
/// recipe saves showed Resample 0, e3 generic 1, Stretch pro -2, Auto 5, e2 generic 6, and 8 in the save made as "e3
/// mono" (PyFLP's list has 8 as e2 mono and 2 as e3 mono; both are one-voice modes, so they read the same here).
/// The rest (-1, 2, 3, 4, 7, 9) are PyFLP's list, not checked against a save.
fn stretch_of(s: &FlStretch) -> Stretch {
    match s.mode {
        // Resample plays like a record. Fitted to a length in beats (the Time knob) it follows the tempo; without,
        // it plays at its own speed whatever the tempo, faster or slower as Pitch and Mul say.
        0 if s.time > 0.0 => Stretch::Repitch,
        0 => Stretch::Raw,
        -1 => Stretch::Stretch,
        -2 => Stretch::StretchPro,
        // FL's e3 is zplane's Elastique Pro, e2 the Elastique before it; the mono modes are for one voice.
        1 => Stretch::ElastiquePro,
        2 | 8 => Stretch::ElastiqueSolo,
        3 | 4 => Stretch::Slice,
        5 => Stretch::Auto,
        6 | 7 | 9 => Stretch::Elastique,
        other => Stretch::Other(other),
    }
}

/// The points of a song-long curve between `start` and `start + length`, timed from `start`, with the value at each
/// end: the automation reader's own cut, sliding between points, so jumps come out as the song plays them. A jump right
/// at the end is what's held after the clip, so the clip doesn't draw it.
fn cut(curve: &[FlPoint], start: f64, length: f64) -> Vec<PreviewPoint> {
    let mut points = cut_curve(curve, 0.0, start, length, false);
    if let [.., a, b] = points.as_slice()
        && a.beat == b.beat
    {
        points.pop();
    }
    points.into_iter().map(|p| PreviewPoint { beat: p.beat, value: p.value }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(beat: f64, value: f64) -> FlPoint {
        FlPoint { beat, value }
    }

    #[test]
    fn colours_come_out_as_rgb() {
        assert_eq!(rgb(0x0033_2211), 0x11_22_33);
        assert_eq!(rgb(0xFF00_00FF), 0xFF_00_00, "the top byte isn't a colour");
    }

    #[test]
    fn cuts_a_curve_to_a_clip_with_the_value_at_each_end() {
        let curve = [point(0.0, 0.0), point(4.0, 1.0), point(8.0, 0.0)];
        let cut = cut(&curve, 2.0, 4.0);
        assert_eq!(
            cut,
            [
                PreviewPoint { beat: 0.0, value: 0.5 },
                PreviewPoint { beat: 2.0, value: 1.0 },
                PreviewPoint { beat: 4.0, value: 0.5 },
            ]
        );
        assert!(super::cut(&[], 0.0, 4.0).is_empty());
        // Before the first point and after the last, the curve holds.
        assert_eq!(super::cut(&curve, 10.0, 2.0).iter().map(|p| p.value).collect::<Vec<_>>(), [0.0, 0.0]);
    }

    #[test]
    fn describes_a_track_by_its_clips_names() {
        let clip = |name: &str| PreviewClip { name: name.into(), ..PreviewClip::default() };
        assert_eq!(describe(&[]), "");
        assert_eq!(describe(&[clip("Drums"), clip("Drums")]), "Drums");
        assert_eq!(describe(&[clip("Drums"), clip("Bass"), clip("Drums"), clip("Lead")]), "Drums + 2 more");
    }
}
