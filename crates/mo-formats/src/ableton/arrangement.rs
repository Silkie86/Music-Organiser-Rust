//! A Live set's arrangement as Live shows it: a lane for each track (a group's tracks under it), each arrangement
//! clip with its notes or its audio, the locators as markers. A set whose arrangement is empty but whose session
//! holds clips gets its scenes one after another instead, as the Bitwig reader does with its launcher.
//!
//! An audio clip that's warped plays its file as its warp markers say (seconds of the file at beats of the clip); one
//! that isn't plays it at its own speed. Live's warp modes are shown by the nearest of Bitwig's: Re-Pitch is Repitch,
//! Complex Pro Elastique Pro, Complex Elastique, Tones Elastique Solo, Beats and Texture Stretch.

use std::collections::HashMap;

use roxmltree::Node;

use super::{
    Content, LiveSet, arrangement_clips, at, child, clip_notes, flag, note_velocity, number, scenes, session_clips,
    signature, tempo, track_name, tracks, value,
};
use crate::arrangement::{
    Arrangement, AudioPart, LaneKind, NoteBudget, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, Stretch,
    TOO_MANY_NOTES, TimeSignature, fades_within,
};

pub const VIEW: &str = "Ableton arrangement";
pub const SESSION_VIEW: &str = "Ableton session";

/// The most times one clip's loop is drawn: far past a real song's, so a tiny loop on a huge clip can't go on for ever.
const MOST_PASSES: usize = 100_000;

impl LiveSet {
    /// The arrangement, and the session's scenes when the arrangement has no clips; a set too damaged to draw gives
    /// a view that says so.
    pub fn arrangements(&self) -> Vec<Arrangement> {
        self.try_arrangements().unwrap_or_else(|_| vec![crate::arrangement::unreadable(VIEW, "This set")])
    }

    /// [`LiveSet::arrangements`], or why the set's XML can't be read.
    pub fn try_arrangements(&self) -> Result<Vec<Arrangement>, String> {
        let doc = self.doc()?;
        let Some(set) = child(doc.root_element(), "LiveSet") else {
            return Err("it has no LiveSet".into());
        };
        let song_tempo = tempo(set).unwrap_or(120.0);
        let mut budget = NoteBudget::new();
        let arranged = self.view(set, song_tempo, &mut budget, None);
        if arranged.clip_count() > 0 {
            return Ok(vec![arranged]);
        }
        let session = self.view(set, song_tempo, &mut budget, Some(scene_starts(set)));
        Ok(if session.clip_count() > 0 { vec![session, arranged] } else { vec![arranged] })
    }

    /// One view: the arrangement's clips, or (with each scene's start) the session's.
    fn view(
        &self,
        set: Node<'_, '_>,
        song_tempo: f64,
        budget: &mut NoteBudget,
        scenes: Option<Vec<f64>>,
    ) -> Arrangement {
        let mut lanes = Vec::new();
        let mut depth_of: HashMap<String, u32> = HashMap::new();
        for track in tracks(set) {
            let tag = track.tag_name().name();
            let kind = match tag {
                "MidiTrack" => LaneKind::Instrument,
                "AudioTrack" => LaneKind::Audio,
                "GroupTrack" => LaneKind::Group,
                // A return track holds effects, never clips.
                _ => continue,
            };
            let group = value(track, &["TrackGroupId"]).unwrap_or("-1").trim().to_owned();
            let depth = if group == "-1" { 0 } else { depth_of.get(&group).map_or(1, |d| d + 1) };
            if let Some(id) = track.attribute("Id") {
                depth_of.insert(id.to_owned(), depth);
            }
            let mut lane = PreviewLane::new(track_name(track), kind);
            lane.depth = depth;
            // The track's on switch (its "speaker").
            lane.muted = value(track, &["DeviceChain", "Mixer", "Speaker", "Manual"]).is_some_and(|v| v == "false");
            lane.detail = instrument_of(track);
            if let Some(sequencer) = at(track, &["DeviceChain", "MainSequencer"]) {
                let placed: Vec<(f64, Node<'_, '_>, Option<f64>)> = match &scenes {
                    None => arrangement_clips(sequencer)
                        .into_iter()
                        .filter_map(|c| {
                            let start = c.attribute("Time").and_then(|t| t.trim().parse::<f64>().ok())?;
                            Some((start, c, None))
                        })
                        .collect(),
                    Some(starts) => session_clips(sequencer)
                        .into_iter()
                        .filter_map(|(scene, c)| {
                            let from = *starts.get(scene)?;
                            let to = starts.get(scene + 1).copied().unwrap_or(from);
                            Some((from, c, Some(to - from)))
                        })
                        .collect(),
                };
                for (start, c, play_for) in placed {
                    if let Some(clip) = clip(self, c, start, play_for, song_tempo, budget) {
                        lane.clips.push(clip);
                    }
                }
            }
            lane.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
            lanes.push(lane);
        }
        let markers = match scenes {
            None => locators(set),
            Some(ref starts) => scene_markers(set, starts),
        };
        let mut notes = Vec::new();
        if budget.ran_out {
            notes.push(TOO_MANY_NOTES.to_owned());
        }
        Arrangement {
            view: if scenes.is_some() { SESSION_VIEW } else { VIEW },
            tempo: song_tempo,
            signature: signature(set).unwrap_or(TimeSignature::FOUR_FOUR),
            lanes,
            markers,
            notes,
        }
    }
}

/// What plays a track: its first instrument's name, for a MIDI track.
fn instrument_of(track: Node<'_, '_>) -> String {
    if !track.has_tag_name("MidiTrack") {
        return String::new();
    }
    let mut found = Vec::new();
    super::devices(track, &mut found);
    // The instrument inside a rack says more than the rack.
    let instruments: Vec<_> = found.into_iter().filter(|p| p.kind == crate::model::PluginKind::Instrument).collect();
    let rack = |name: &str| name.ends_with("Rack");
    instruments.iter().find(|p| !rack(&p.name)).or(instruments.first()).map(|p| p.name.clone()).unwrap_or_default()
}

/// The set's locators, as markers.
fn locators(set: Node<'_, '_>) -> Vec<PreviewMarker> {
    let Some(list) = at(set, &["Locators", "Locators"]) else { return Vec::new() };
    let mut out: Vec<PreviewMarker> = list
        .children()
        .filter(|l| l.has_tag_name("Locator"))
        .filter_map(|l| {
            Some(PreviewMarker {
                beat: number(l, &["Time"])?,
                name: value(l, &["Name"]).unwrap_or_default().trim().to_owned(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    out
}

/// Where each scene starts when they're played one after another: each lasts as long as its longest clip (a bar
/// when it has none), and one more start marks where the last ends.
fn scene_starts(set: Node<'_, '_>) -> Vec<f64> {
    let mut lengths = vec![0.0f64; scenes(set).len()];
    for track in tracks(set) {
        let Some(sequencer) = at(track, &["DeviceChain", "MainSequencer"]) else { continue };
        for (scene, c) in session_clips(sequencer) {
            let content = Content::of(c);
            let length = if content.looping { content.end - content.loop_start } else { content.end - content.start };
            // A slot past the scenes listed still gets a scene of its own.
            if lengths.len() <= scene && scene < 100_000 {
                lengths.resize(scene + 1, 0.0);
            }
            if let Some(l) = lengths.get_mut(scene) {
                *l = l.max(length);
            }
        }
    }
    let count = lengths.len();
    let bar = signature(set).unwrap_or(TimeSignature::FOUR_FOUR).beats();
    let mut starts = Vec::with_capacity(count + 1);
    let mut at_beat = 0.0;
    starts.push(at_beat);
    for l in lengths {
        at_beat += if l > 0.0 && l.is_finite() { l } else { bar };
        starts.push(at_beat);
    }
    starts
}

/// The scenes' names at their starts.
fn scene_markers(set: Node<'_, '_>, starts: &[f64]) -> Vec<PreviewMarker> {
    let listed = scenes(set);
    // Every scene that has a start but its last (which only marks where the scenes end).
    let shown = starts.len().saturating_sub(1);
    (0..shown)
        .map(|i| (listed.get(i).copied(), starts.get(i).copied().unwrap_or(0.0)))
        .enumerate()
        .map(|(i, (s, beat))| {
            let name = s.and_then(|s| value(s, &["Name"]).or_else(|| s.attribute("Value"))).unwrap_or_default().trim();
            PreviewMarker { beat, name: if name.is_empty() { format!("Scene {}", i + 1) } else { name.to_owned() } }
        })
        .collect()
}

/// Where the clip's own beats play, one window at a time: from `from` to `to` of the content at `at` beats into the
/// clip. The first pass starts at the start marker; when it loops, the next ones play the loop again until the clip
/// ends.
fn windows(content: &Content, length: f64) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    let first = (content.end - content.start).min(length);
    if first > 0.0 {
        out.push((content.start, content.start + first, 0.0));
    }
    if !content.looping {
        return out;
    }
    let pass = content.end - content.loop_start;
    if pass.is_nan() || pass <= 0.0 {
        return out;
    }
    let mut at_beat = first.max(0.0);
    while at_beat < length && out.len() < MOST_PASSES {
        let played = pass.min(length - at_beat);
        out.push((content.loop_start, content.loop_start + played, at_beat));
        at_beat += pass;
    }
    out
}

fn clip(
    set: &LiveSet,
    c: Node<'_, '_>,
    start: f64,
    play_for: Option<f64>,
    song_tempo: f64,
    budget: &mut NoteBudget,
) -> Option<PreviewClip> {
    let content = Content::of(c);
    let length = match play_for {
        Some(l) => l,
        None => number(c, &["CurrentEnd"])? - number(c, &["CurrentStart"])?,
    };
    if !start.is_finite() || !length.is_finite() || length <= 0.0 {
        return None;
    }
    let mut out = PreviewClip {
        name: value(c, &["Name"]).unwrap_or_default().trim().to_owned(),
        start,
        length,
        muted: flag(c, &["Disabled"]),
        ..PreviewClip::default()
    };
    let windows = windows(&content, length);
    if c.has_tag_name("MidiClip") {
        let notes = clip_notes(c);
        'windows: for &(from, to, at_beat) in &windows {
            for n in notes.iter().filter(|n| n.time >= from - 1e-9 && n.time < to) {
                if !budget.take() {
                    break 'windows;
                }
                let note_start = at_beat + (n.time - from);
                out.notes.push(PreviewNote {
                    start: note_start,
                    length: n.length.min(length - note_start).max(0.0),
                    key: n.key,
                    velocity: note_velocity(n.velocity),
                    muted: !n.enabled,
                    step: false,
                    part: 0,
                });
            }
        }
    } else {
        let Some(file_ref) = at(c, &["SampleRef", "FileRef"]) else { return Some(out) };
        let path = set.file_ref_path(file_ref);
        if path.is_empty() {
            return Some(out);
        }
        let warped = value(c, &["IsWarped"]).is_none_or(|v| v != "false");
        let warps: Vec<(f64, f64)> = child(c, "WarpMarkers")
            .map(|w| {
                w.children()
                    .filter(|m| m.has_tag_name("WarpMarker"))
                    .filter_map(|m| {
                        let get = |n: &str| m.attribute(n)?.trim().parse::<f64>().ok().filter(|v| v.is_finite());
                        Some((get("BeatTime")?, get("SecTime")?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Seconds of the file a beat of the clip plays, and the second at the clip's beat 0.
        let (rate, zero) = match (warps.first(), warps.last()) {
            (Some(a), Some(b)) if warped && warps.len() >= 2 && b.0 > a.0 => {
                let rate = (b.1 - a.1) / (b.0 - a.0);
                (rate, a.1 - a.0 * rate)
            }
            // Not warped: the clip's own times are the file's seconds, and it plays at its own speed.
            _ => (60.0 / song_tempo, 0.0),
        };
        let unwarped_seconds = !warped;
        let semitones = number(c, &["PitchCoarse"]).unwrap_or(0.0) + number(c, &["PitchFine"]).unwrap_or(0.0) / 100.0;
        let gain = number(c, &["SampleVolume"]).unwrap_or(1.0);
        let gain_db = if gain > 0.0 { 20.0 * gain.log10() } else { f64::NEG_INFINITY };
        let stretch = if !warped {
            Some(Stretch::Raw)
        } else {
            number(c, &["WarpMode"]).map(|m| match m as i64 {
                0 | 2 => Stretch::Stretch,
                1 => Stretch::ElastiqueSolo,
                3 => Stretch::Repitch,
                4 => Stretch::Elastique,
                6 => Stretch::ElastiquePro,
                other => Stretch::Other(other as i32),
            })
        };
        // An unwarped clip's own times are seconds of the file: worked in the song's beats.
        let windows = if unwarped_seconds { self::windows(&content.scaled(1.0 / rate), length) } else { windows };
        for &(from, to, at_beat) in &windows {
            let from_second = zero + from * rate;
            let beats = to - from;
            if beats <= 0.0 {
                continue;
            }
            // Audio parts come out of the same budget as notes: a tiny loop on a long clip can't fill the memory.
            if !budget.take() {
                break;
            }
            out.audio.push(AudioPart {
                path: path.clone(),
                start: at_beat,
                length: beats.min(length - at_beat),
                from: from_second,
                seconds_per_beat: Some(rate),
                whole_file_beats: None,
                reversed: false,
                stretch,
                semitones,
                gain_db,
            });
        }
        let fade_in = number(c, &["Fades", "FadeInLength"]).unwrap_or(0.0);
        let fade_out = number(c, &["Fades", "FadeOutLength"]).unwrap_or(0.0);
        (out.fade_in, out.fade_out) = fades_within(fade_in, fade_out, length);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_looping_clip_plays_its_loop_again_until_it_ends() {
        let content = Content { loop_start: 0.0, start: 1.0, end: 4.0, looping: true };
        assert_eq!(windows(&content, 10.0), [(1.0, 4.0, 0.0), (0.0, 4.0, 3.0), (0.0, 3.0, 7.0)]);
        let once = Content { looping: false, ..content };
        assert_eq!(windows(&once, 10.0), [(1.0, 4.0, 0.0)]);
        assert_eq!(windows(&once, 2.0), [(1.0, 3.0, 0.0)]);
        let empty = Content { loop_start: 2.0, start: 2.0, end: 2.0, looping: true };
        assert!(windows(&empty, 8.0).is_empty());
    }
}
