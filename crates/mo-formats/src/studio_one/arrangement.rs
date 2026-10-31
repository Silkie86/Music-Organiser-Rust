//! A Studio One song's arrangement as Studio One shows it, mapped from Solomon's songs (Studio One 6.5 and Studio Pro
//! 8). Ported from the C# app's `Parsers/StudioOneArrangement.cs`.
//!
//! `Song/song.xml` lists the tracks (MediaTrack, mediaType Music or Audio) and their events. An instrument part's notes
//! are in its clip's file (`Performances/*.musicx`, binary JSON, or `*.pattern` for a pattern part), found through
//! `Song/mediapool.xml` by clipID. Positions are in beats; an event whose timeFormat is 0 gives its length and offset
//! in seconds. The tempo map holds seconds per beat.

use std::collections::HashMap;

use roxmltree::Node;

use super::{Song, attr, elements, id, num, parse, unescape, url_to_path};
use crate::arrangement::{
    Arrangement, AudioPart, LaneKind, NoteBudget, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, TOO_MANY_NOTES,
};
use crate::{paths, velocity};

pub const VIEW: &str = "Studio One arrangement";

/// What a part's clip id points at in the media pool: an audio file and its length in seconds (0 when not saved), or
/// the file in the song that holds the part's notes.
#[derive(Default)]
struct Pool {
    audio: HashMap<String, (String, f64)>,
    music: HashMap<String, String>,
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
        let beats = |e: Node<'_, '_>, name: &str, seconds: bool| {
            num(attr(e, name)).map_or(0.0, |v| if seconds { v / spb } else { v })
        };
        let in_seconds = |e: Node<'_, '_>| attr(e, "timeFormat") == "0";

        let mut lanes = Vec::new();
        let mut budget = NoteBudget::new();
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
                _ => continue,
            };
            let lists = |name: &'static str| {
                track.children().filter(move |l| l.is_element() && l.tag_name().name() == "List" && id(*l) == name)
            };
            let mut clips = Vec::new();
            for ev in lists("Events").flat_map(|l| l.children().filter(Node::is_element)) {
                let name = attr(ev, "name");
                let start = beats(ev, "start", false);
                let seconds = in_seconds(ev);
                let mut length = beats(ev, "length", seconds);
                let offset = beats(ev, "offset", seconds);
                let muted = attr(ev, "mute") == "1";
                let clip_id = attr(ev, "clipID");
                match ev.tag_name().name() {
                    "AudioEvent" => {
                        let (file, file_seconds) = pool.audio.get(&clip_id.to_lowercase()).cloned().unwrap_or_default();
                        let speed = num(attr(ev, "speed")).filter(|s| *s > 0.0).unwrap_or(1.0);
                        if length <= 0.0 && file_seconds > 0.0 {
                            length = file_seconds / speed / spb;
                        }
                        let from = if seconds { num(attr(ev, "offset")).unwrap_or(0.0) } else { offset * spb * speed };
                        let name = if name.is_empty() {
                            paths::stem(paths::file_name(&file)).to_owned()
                        } else {
                            name.to_owned()
                        };
                        let part = AudioPart {
                            path: file,
                            start: 0.0,
                            length,
                            from,
                            seconds_per_beat: Some(spb * speed),
                            whole_file_beats: None,
                            reversed: false,
                        };
                        clips.push(PreviewClip {
                            name,
                            start,
                            length,
                            muted,
                            audio: vec![part],
                            ..PreviewClip::default()
                        });
                    }
                    "MusicPart" | "NotePart" => {
                        let transpose = num(attr(ev, "transpose")).unwrap_or(0.0) as i32;
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
                            'passes: while at < length - 1e-9 {
                                for n in steps.iter().filter(|n| at + n.start < length) {
                                    if !budget.take() {
                                        break 'passes;
                                    }
                                    notes.push(PreviewNote { start: at + n.start, ..*n });
                                }
                                if steps.is_empty() {
                                    break;
                                }
                                at += loop_beats;
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
                "" => "Instrument",
                n => n,
            };
            let mut lane = PreviewLane::new(name, kind);
            lane.colour = colour(attr(track, "color"));
            lane.muted = attr(track, "mute") == "1";
            lane.depth = depth;
            lane.clips = clips;
            lanes.push(lane);
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
            .map(|m| PreviewMarker { beat: beats(m, "start", in_seconds(m)), name: attr(m, "name").to_owned() })
            .collect();
        markers.sort_by(|a, b| a.beat.total_cmp(&b.beat));

        let mut notes = Vec::new();
        if lanes.iter().all(|l| l.clips.is_empty()) {
            notes.push("Nothing is on this song's timeline yet.".to_owned());
        }
        if budget.ran_out {
            notes.push(TOO_MANY_NOTES.to_owned());
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
            if clip.tag_name().name() == "AudioClip" {
                let format = children("Attributes").find(|a| id(*a) == "format");
                let frames = format.and_then(|f| num(attr(f, "frameCount")));
                let rate = format.and_then(|f| num(attr(f, "sampleRate")));
                let seconds = match (frames, rate) {
                    (Some(f), Some(r)) if f > 0.0 && r > 0.0 => f / r,
                    _ => 0.0,
                };
                pool.audio.insert(clip_id.to_lowercase(), (url_to_path(url), seconds));
            } else if let Some(rest) = url.get(..9).filter(|s| s.eq_ignore_ascii_case("media:///")).and(url.get(9..)) {
                pool.music.insert(clip_id.to_lowercase(), unescape(rest));
            }
        }
        pool
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
                let n = |key: &str, fallback: f64| match field(o, key) {
                    Some(Ubjson::Float(f)) => f,
                    Some(Ubjson::Int(i)) => i as f64,
                    _ => fallback,
                };
                PreviewNote {
                    start: n("start", 0.0),
                    length: n("length", 0.25),
                    key: (n("pitch", 60.0) as i32).clamp(0, 127) as u8,
                    velocity: velocity_of(Some(n("velocity", 0.8))),
                    muted: n("mute", 0.0) != 0.0,
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
        let notes = p
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "PatternStep")
            .map(|n| PreviewNote {
                start: num(attr(n, "start")).unwrap_or(0.0) * step,
                length: step,
                key: (num(attr(n, "pitch")).unwrap_or(36.0) as i32).clamp(0, 127) as u8,
                velocity: velocity_of(num(attr(n, "velocity"))),
                muted: attr(n, "mute") == "1",
            })
            .collect();
        (notes, count * step)
    }
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

/// A velocity as Studio One saves it, as the song reader reads it: 0 to 1 in newer songs, kept in MIDI 2.0's 16 bits
/// (rule 22), or MIDI 1.0's 0 to 127 in older ones. "NaN" is the quietest (MIDI 1.0's 1), as in the C# app.
fn velocity_of(v: Option<f64>) -> u16 {
    match v.unwrap_or(0.8) {
        v if v.is_nan() => velocity::from_7(1),
        v if v > 1.0 => velocity::from_7(v.round_ties_even().min(127.0) as u8),
        v => velocity::from_unit(v),
    }
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
