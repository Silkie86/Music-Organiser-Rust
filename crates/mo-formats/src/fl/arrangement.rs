//! An FL Studio project's arrangement, two ways: its playlist as FL shows it (tracks holding pattern, audio and
//! automation clips), and one track per channel, the way "Open in Bitwig" lays it out. Ported from the C# app's
//! `Preview/FlArrangement.cs` and `Parsers/FlStudioPlaylist.cs`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::events::{
    EV_CHANNEL_NAME_OLD, EV_CHANNEL_TYPE, EV_PATTERN_NAME, EV_PATTERN_NEW, EV_PATTERN_NOTES, EV_PLAYLIST,
    EV_PLUGIN_NAME, Flp, byte_at, u32_at,
};
use super::notes;
use super::song::{
    CHANNEL_TYPE_AUTOMATION, EV_ARRANGEMENT_NEW, EV_TRACK_DATA, EV_TRACK_NAME, FlChannel, FlSong, FlStretch,
    PATTERN_BASE, playlist_items, read_song_with, top_slot,
};
use super::{Rack, channels as known_channels};
use crate::arrangement::{
    Arrangement, AudioPart, LaneKind, NoteBudget, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, PreviewPoint,
    TOO_MANY_NOTES,
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
    vec![playlist_view(&song, &playlist, tempo), by_channel(&song, &playlist, tempo)]
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
    let mut beats_per_bar = 4;
    let mut channel_colours = HashMap::new();
    let mut channel_names = HashMap::new();
    let mut automation_channels = HashSet::new();
    let mut pattern_names: HashMap<u32, String> = HashMap::new();
    let mut pattern_colours: HashMap<u32, u32> = HashMap::new();
    let mut raw_notes: BTreeMap<u32, Vec<(u32, crate::NoteEvent)>> = BTreeMap::new();
    let mut tracks: BTreeMap<u32, Track> = BTreeMap::new();
    let mut markers: Vec<(i64, String)> = Vec::new();
    let mut playlist: Option<&[u8]> = None;
    let (mut pattern, mut track): (Option<u32>, Option<u32>) = (None, None);
    let mut arrangements = 0;
    let mut rack = Rack::default();

    for e in &flp.events {
        rack.step(e);
        let channel = rack.channel();
        // Only the first arrangement: the next one repeats its playlist, tracks and markers.
        let first = arrangements <= 1;
        match e.id {
            EV_TIME_SIG_NUMERATOR if (1..=32).contains(&e.int()) => beats_per_bar = e.int(),
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
                    let size = notes::note_size(&e.data, flp.major, e.offset.is_none(), Some(&known));
                    raw_notes.entry(p).or_default().extend(notes::decode(&e.data, size));
                }
            }
            EV_ARRANGEMENT_NEW => arrangements += 1,
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
            EV_TIME_MARKER if first => markers.push(((e.int() & 0x07FF_FFFF) as i64, String::new())),
            EV_MARKER_NAME if first => {
                if let Some(m) = markers.last_mut() {
                    m.1 = flp.text(&e.data);
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
                }
            }
        })
        .collect();

    Playlist {
        ppq: flp.ppq,
        beats_per_bar,
        tracks,
        patterns,
        items,
        markers,
        channel_colours,
        channel_names,
        automation_channels,
    }
}

fn channel_colour(colours: &HashMap<u32, u32>, channel: u32) -> Option<u32> {
    colours.get(&channel).copied().filter(|&c| c != DEFAULT_CHANNEL_COLOUR)
}

fn playlist_view(song: &FlSong, pl: &Playlist, tempo: f64) -> Arrangement {
    let beats = |ticks: f64| ticks / ppq(pl.ppq);
    let channels: HashMap<u32, &FlChannel> = song.channels.iter().map(|c| (c.index, c)).collect();
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
                let in_clip = pattern
                    .map(|pat| {
                        let first = pat.notes.partition_point(|(_, n)| n.start < from);
                        pat.notes
                            .get(first..)
                            .unwrap_or_default()
                            .iter()
                            .take_while(|(_, n)| n.start < end)
                            .take_while(|_| budget.take())
                            .map(|(_, n)| PreviewNote {
                                start: beats((n.start - from) as f64),
                                length: beats(n.length.max(1).min(end - n.start) as f64),
                                key: n.key,
                                velocity: n.velocity,
                                muted: n.muted,
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
                clips.push(PreviewClip {
                    name: c.name.clone(),
                    start,
                    length,
                    muted: item.muted,
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
        markers: pl
            .markers
            .iter()
            .map(|(at, name)| PreviewMarker { beat: beats(*at as f64), name: name.clone() })
            .collect(),
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
                .map(|k| PreviewClip {
                    name: c.name.clone(),
                    start: beats(k.start),
                    length: beats(k.length),
                    muted: k.muted || k.on_muted_track,
                    audio: if c.sample.is_empty() { Vec::new() } else { vec![part(c, beats(k.length), k.from, tempo)] },
                    ..PreviewClip::default()
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
    let notes = if song.channels.iter().any(|c| !c.audio_clips.is_empty() && c.sample.is_empty()) {
        vec!["Some audio clips are left out: FL Studio didn't save which sample they play.".to_owned()]
    } else {
        Vec::new()
    };
    Arrangement { view: BITWIG_VIEW, tempo, beats_per_bar: pl.beats_per_bar, lanes, markers: Vec::new(), notes }
}

/// A channel's sample playing in a clip `beats` long, from `from` seconds in, stretched as the channel says.
fn part(c: &FlChannel, beats: f64, from: f64, tempo: f64) -> AudioPart {
    let s: &FlStretch = &c.stretch;
    // Without the Time knob, how far the sample is stretched doesn't depend on its length; with it, the whole sample
    // is fitted to a length, so how fast it moves is only known once the file's length is.
    let (seconds_per_beat, whole_file_beats) = if s.time > 0 {
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
