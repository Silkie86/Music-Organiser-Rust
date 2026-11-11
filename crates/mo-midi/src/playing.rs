//! A saved song's parts as they play: each instrument's notes through the song, read from the project's arrangement
//! (`mo_formats::arrangement`, what the Songs page's preview draws).
//!
//! [`crate::extract`] reads a project for the library: there every pattern or clip is a part of its own, moved to
//! start in bar 1. Here the parts keep their place against each other, so the chords and the bass of bars 17 to 24 of
//! the song being worked on can be taken together, and a tune from the library fitted over them ([`crate::fit`]).
//!
//! Only what plays is read: nothing from a muted track or clip, no muted notes. FL Studio projects are read one
//! channel a part (the "As it opens in Bitwig" layout), where patterns played in pattern mode come one after another;
//! Bitwig projects by their arranger, or their scenes one after another when the arranger is empty.
//!
//! Files are only read. A damaged file gives an error in plain words, never a crash.

use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::path::Path;

use mo_formats::arrangement::{Arrangement, MOST_BEATS};
use mo_formats::dates::RealDates;
use mo_formats::{NoteAttribute, NoteEvent, bitwig, case, fl, paths, studio_one};
use mo_harmony::Grid;

use crate::PPQ;
use crate::analyser::{self, Shape};
use crate::extract::{self, Unread};
use crate::roles::Role;

/// One clip (in FL Studio, one pattern) of a part where it plays in the song.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipPlace {
    pub name: String,
    /// The bars it takes up, counted from 0: from the bar it starts in to the bar after the one it ends in.
    pub bars: Range<i64>,
}

/// One instrument of a song (an FL Studio channel, a Bitwig or Studio One track) and everything it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub name: String,
    /// What plays it, when the project says (a plugin's name), else "".
    pub plugin: String,
    /// Its notes through the song, at [`PPQ`] ticks a beat from the song's start, in order.
    pub notes: Vec<NoteEvent>,
    /// Played by a drum plugin, whose notes pick pads, not pitches.
    pub drums: bool,
    /// What kind of part it is, guessed from everything it plays and its names.
    pub role: Role,
    /// Why, in a few words.
    pub role_why: String,
    /// The bars it plays in, counted from 0: from its first note's bar to the bar after its last note ends.
    pub bars: Range<i64>,
    /// Its clips with notes that play, in the order they play.
    pub clips: Vec<ClipPlace>,
}

/// A place the song's maker named ("Verse", "Drop").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    /// The bar it's in, counted from 0.
    pub bar: i64,
    pub name: String,
}

/// A saved song as it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    /// Which of the project's layouts this is, as the Songs page names it.
    pub view: &'static str,
    pub tempo: f64,
    /// Beats in a bar. A song that changes its time signature on the way is read by the one it starts with.
    pub beats_per_bar: u32,
    /// The instruments that play notes, in the project's order.
    pub parts: Vec<Playing>,
    pub markers: Vec<Marker>,
    /// What couldn't be read, in plain words.
    pub warnings: Vec<String>,
}

impl Song {
    /// Ticks in one of the song's bars.
    pub fn bar(&self) -> i64 {
        self.grid().bar()
    }

    /// The song's beats and bars, for `mo-harmony`.
    pub fn grid(&self) -> Grid {
        Grid { ticks_per_beat: PPQ as u32, beats_per_bar: self.beats_per_bar.max(1), bar_line: 0 }
    }

    /// How many bars the song's notes run.
    pub fn bars(&self) -> i64 {
        self.parts.iter().map(|p| p.bars.end).max().unwrap_or(0)
    }

    /// The parts that carry the song's chords (chords, arps, basslines), as places in [`Song::parts`]: what a tune is
    /// fitted over when nothing is picked.
    pub fn harmony_parts(&self) -> Vec<usize> {
        self.parts_that(|role| matches!(role, Role::Chords | Role::Arp | Role::Bass))
    }

    /// The song's melodies, as places in [`Song::parts`]: what chords are fitted under when nothing is picked.
    pub fn melody_parts(&self) -> Vec<usize> {
        self.parts_that(|role| role == Role::Melody)
    }

    fn parts_that(&self, is: impl Fn(Role) -> bool) -> Vec<usize> {
        self.parts.iter().enumerate().filter(|(_, p)| !p.drums && is(p.role)).map(|(at, _)| at).collect()
    }

    /// What the part at `part` plays in `bars` (counted from 0), moved so the first of those bars starts at tick 0. A
    /// note held into the stretch sounds from its start, and one held past its end stops there.
    pub fn stretch(&self, part: usize, bars: &Range<i64>) -> Vec<NoteEvent> {
        let bar = self.bar();
        let (from, to) = (bars.start.saturating_mul(bar), bars.end.saturating_mul(bar));
        let Some(part) = self.parts.get(part).filter(|_| from < to) else { return Vec::new() };
        let mut notes: Vec<NoteEvent> = part
            .notes
            .iter()
            .filter(|n| n.start < to && n.start.saturating_add(n.length) > from)
            .map(|n| {
                let start = n.start.max(from);
                let end = n.start.saturating_add(n.length).min(to);
                NoteEvent { start: start - from, length: end - start, ..*n }
            })
            .collect();
        // A note held in from before now starts with the stretch's first: keep them in order.
        notes.sort_by_key(|n| (n.start, n.key));
        notes
    }

    /// The bars to fit to when none are picked: up to `most` bars from the first bar where the most of `parts` play
    /// at once (the chords and the bass together, not the chords' bars alone before the bass comes in), ending where
    /// the last of them stops. None when none of them plays a note.
    pub fn busiest_bars(&self, parts: &[usize], most: i64) -> Option<Range<i64>> {
        let mut places = parts.to_vec();
        places.sort_unstable();
        places.dedup();
        let picked: Vec<&Playing> =
            places.iter().filter_map(|&p| self.parts.get(p)).filter(|p| !p.notes.is_empty()).collect();
        let first = picked.iter().map(|p| p.bars.start).min()?;
        let last = picked.iter().map(|p| p.bars.end).max()?;
        let bar = self.bar();
        let count = usize::try_from(last - first).ok()?;
        // How many of the parts sound in each bar. Each part's bars are marked where its notes start and stop and
        // then added up along the song, so a note held for a thousand bars costs no more than a short one.
        let mut playing = vec![0u32; count];
        let mut edges = vec![0i32; count + 1];
        for p in &picked {
            edges.fill(0);
            for n in &p.notes {
                let from = usize::try_from(n.start.div_euclid(bar) - first).unwrap_or(0).min(count);
                let to = usize::try_from((n.start + n.length.max(1) - 1).div_euclid(bar) - first + 1).unwrap_or(0);
                if let Some(edge) = edges.get_mut(from) {
                    *edge += 1;
                }
                if let Some(edge) = edges.get_mut(to.min(count)) {
                    *edge -= 1;
                }
            }
            let mut sounding = 0;
            for (bar_count, edge) in playing.iter_mut().zip(&edges) {
                sounding += edge;
                *bar_count += u32::from(sounding > 0);
            }
        }
        let fullest = playing.iter().copied().max()?;
        let start = first + playing.iter().position(|&c| c == fullest)? as i64;
        Some(start..last.min(start.saturating_add(most.max(1))))
    }
}

/// Reads a project from disk as it plays.
pub fn read(path: &Path) -> Result<Song, Unread> {
    let raw = std::fs::read(path).map_err(|e| extract::unread_file(&e))?;
    from_bytes(path, &raw)
}

/// A project's bytes, already read from `path`, as the song plays.
pub fn from_bytes(path: &Path, raw: &[u8]) -> Result<Song, Unread> {
    let text = path.to_string_lossy();
    match mo_catch::catch(AssertUnwindSafe(|| views(&text, raw))) {
        Ok(views) => {
            let mut views = views?;
            // FL Studio's second layout has a lane for each channel; the playlist's lanes are its own tracks.
            let at = views.iter().position(|v| v.view == fl::arrangement::BITWIG_VIEW).unwrap_or(0);
            if at >= views.len() {
                return Err(Unread::Failed("It has no arrangement to read".into()));
            }
            Ok(of(&views.swap_remove(at)))
        }
        Err(_) => Err(Unread::Failed(
            "The reader stopped on it: it looks damaged in a way the reader doesn't know yet".into(),
        )),
    }
}

/// The project's layouts, by its ending.
fn views(path: &str, raw: &[u8]) -> Result<Vec<Arrangement>, Unread> {
    match case::to_lower(paths::extension(paths::file_name(path))).as_str() {
        ".flp" => {
            let flp = fl::events::load(raw).map_err(|_| extract::damaged("not an FL Studio project, or damaged"))?;
            Ok(fl::arrangement::read(&flp))
        }
        ".bwproject" => {
            let doc = bitwig::decode(raw).map_err(|_| extract::damaged("not a Bitwig project, or damaged"))?;
            Ok(bitwig::arrangement::read(&doc))
        }
        ".song" => {
            let mut song = studio_one::Song::open(raw).map_err(|e| extract::damaged(&e.to_string()))?;
            Ok(vec![song.arrangement()])
        }
        _ => Err(Unread::Failed("It isn't a project this app reads (FL Studio, Bitwig or Studio One)".into())),
    }
}

/// The parts of an arrangement already read. A clip that holds several instruments' notes (an FL Studio pattern in
/// the playlist's own layout) gives each instrument its part, the same instrument on two lanes one part.
pub fn of(arrangement: &Arrangement) -> Song {
    let beats_per_bar = arrangement.beats_per_bar.max(1);
    let bar = PPQ * i64::from(beats_per_bar);
    let ticks = |beats: f64| -> Option<i64> {
        (beats.is_finite() && beats.abs() <= MOST_BEATS).then(|| (beats * PPQ as f64).round_ties_even() as i64)
    };
    // (the lane it's on, or none for an instrument named by its clips; its name) -> its place in `parts`.
    let mut places: Vec<(Option<usize>, String)> = Vec::new();
    let mut parts: Vec<Playing> = Vec::new();
    for (lane_at, lane) in arrangement.lanes.iter().enumerate().filter(|(_, l)| !l.muted && !l.disabled) {
        // The lane's own part, and each clip's named instruments' parts: looked up once, not for every note.
        let mut own: Option<usize> = None;
        for clip in lane.clips.iter().filter(|c| !c.muted) {
            let (Some(clip_start), Some(clip_length)) = (ticks(clip.start), ticks(clip.length)) else { continue };
            let mut named: Vec<Option<usize>> = vec![None; clip.parts.len()];
            // Which parts this clip gave notes to, to note where the clip plays in each.
            let mut gave: Vec<usize> = Vec::new();
            for n in clip.notes.iter().filter(|n| !n.muted) {
                let (Some(at), Some(length)) = (ticks(n.start), ticks(n.length)) else { continue };
                // A step-sequencer note has no length of its own: a sixteenth, as the library's parts give it.
                let length = if n.step { PPQ / 4 } else { length.clamp(1, analyser::MAX_HOLD) };
                // Nothing of a note sounds before its clip starts or past its end, or before the song starts.
                let from = at.max(0);
                let to = if clip_length > 0 { (at + length).min(clip_length) } else { at + length };
                let (start, end) = ((clip_start + from).max(0), clip_start + to);
                if to <= from || end <= start {
                    continue;
                }
                let slot = usize::from(n.part);
                let name = clip.parts.get(slot).filter(|p| !p.trim().is_empty());
                let known = if name.is_some() { named.get(slot).copied().flatten() } else { own };
                let part_at = known.unwrap_or_else(|| {
                    let key = match name {
                        Some(name) => (None, name.clone()),
                        None => (Some(lane_at), lane.name.clone()),
                    };
                    let at = places.iter().position(|p| *p == key).unwrap_or_else(|| {
                        parts.push(Playing {
                            name: key.1.clone(),
                            plugin: if name.is_some() { String::new() } else { lane.detail.clone() },
                            notes: Vec::new(),
                            drums: false,
                            role: Role::Other,
                            role_why: String::new(),
                            bars: 0..0,
                            clips: Vec::new(),
                        });
                        places.push(key);
                        parts.len() - 1
                    });
                    match (name, named.get_mut(slot)) {
                        (Some(_), Some(kept)) => *kept = Some(at),
                        _ => own = Some(at),
                    }
                    at
                });
                if let Some(part) = parts.get_mut(part_at) {
                    part.notes.push(NoteEvent {
                        start,
                        length: end - start,
                        key: n.key,
                        velocity: n.velocity,
                        muted: false,
                        step: false,
                        attribute: NoteAttribute::default(),
                    });
                    if !gave.contains(&part_at) {
                        gave.push(part_at);
                    }
                }
            }
            let last_bar = (clip_start + clip_length.max(1) - 1).div_euclid(bar);
            let bars = clip_start.div_euclid(bar).max(0)..last_bar.max(0) + 1;
            for at in gave {
                if let Some(part) = parts.get_mut(at) {
                    part.clips.push(ClipPlace { name: clip.name.clone(), bars: bars.clone() });
                }
            }
        }
    }
    for part in &mut parts {
        part.notes.sort_by_key(|n| (n.start, n.key));
        part.clips.sort_by_key(|c| (c.bars.start, c.bars.end));
        let first = part.notes.first().map_or(0, |n| n.start);
        let end = part.notes.iter().map(|n| n.start + n.length).max().unwrap_or(0);
        part.bars = first.div_euclid(bar)..(end - 1).div_euclid(bar) + 1;
        part.drums = extract::is_unpitched_plugin(&part.name) || extract::is_unpitched_plugin(&part.plugin);
        // Its names: its own, and each clip's once.
        let mut names: Vec<&str> = Vec::new();
        for c in &part.clips {
            if !names.contains(&c.name.as_str()) {
                names.push(&c.name);
            }
        }
        let hints = extract::project_hints(&part.name, &names.join(" "));
        // A song-long part is judged by its first notes: as many as the library's longest part holds.
        let judged = part.notes.get(..analyser::MAX_NOTES).unwrap_or(&part.notes);
        (part.role, part.role_why) = analyser::role(&Shape::of(judged), judged, &hints, part.drums);
    }
    let markers = arrangement
        .markers
        .iter()
        .filter_map(|m| Some(Marker { bar: ticks(m.beat)?.div_euclid(bar).max(0), name: m.name.clone() }))
        .collect();
    Song {
        view: arrangement.view,
        tempo: arrangement.tempo,
        beats_per_bar,
        parts,
        markers,
        warnings: arrangement.notes.clone(),
    }
}

/// The saves written last, newest first, each with its song: the first is the song being worked on. Only saves whose
/// date is real (not one still to come, or before 1981), that were there and readable at the last scan, of a kind
/// [`read`] takes; `most` of them at the most, one for each song.
pub fn worked_on<'a>(
    songs: &'a [mo_library::Song],
    dates: &RealDates,
    most: usize,
) -> Vec<(&'a mo_library::Song, &'a mo_library::Save)> {
    let reads = |s: &mo_library::Save| {
        !s.file_missing
            && s.error.is_none()
            && dates.seconds(s.mtime)
            && matches!(
                case::to_lower(paths::extension(paths::file_name(&s.path))).as_str(),
                ".flp" | ".bwproject" | ".song"
            )
    };
    let mut newest: Vec<(&mo_library::Song, &mo_library::Save)> = songs
        .iter()
        .filter_map(|song| Some((song, song.saves().filter(|s| reads(s)).max_by_key(|s| s.mtime)?)))
        .collect();
    // The newest first; songs saved in the same second stay in the order they came.
    newest.sort_by_key(|(_, save)| std::cmp::Reverse(save.mtime));
    newest.truncate(most);
    newest
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use mo_formats::arrangement::{LaneKind, PreviewClip, PreviewLane, PreviewMarker, PreviewNote};

    fn note(start: f64, length: f64, key: u8) -> PreviewNote {
        PreviewNote { start, length, key, velocity: 40_000, ..PreviewNote::default() }
    }

    fn clip(name: &str, start: f64, length: f64, notes: Vec<PreviewNote>) -> PreviewClip {
        PreviewClip { name: name.into(), start, length, notes, ..PreviewClip::default() }
    }

    fn lane(name: &str, clips: Vec<PreviewClip>) -> PreviewLane {
        PreviewLane { clips, ..PreviewLane::new(name, LaneKind::Instrument) }
    }

    fn arrangement(lanes: Vec<PreviewLane>) -> Arrangement {
        Arrangement { view: "Test", tempo: 128.0, beats_per_bar: 4, lanes, markers: Vec::new(), notes: Vec::new() }
    }

    /// A bar of a chord: three notes held for the bar.
    fn chord(keys: [u8; 3]) -> Vec<PreviewNote> {
        keys.iter().map(|&k| note(0.0, 4.0, k)).collect()
    }

    #[test]
    fn each_instrument_keeps_its_place_in_the_song() {
        // Chords from bar 5 (a clip of two bars, played twice), a bass that comes in at bar 7.
        let two_bars: Vec<PreviewNote> =
            [chord([57, 60, 64]), chord([53, 57, 60]).into_iter().map(shifted(4.0)).collect()].concat();
        let keys = lane(
            "Keys",
            vec![clip("Verse chords", 16.0, 8.0, two_bars.clone()), clip("Verse chords", 24.0, 8.0, two_bars)],
        );
        let bass = lane("Sub", vec![clip("Bass 1", 24.0, 8.0, vec![note(0.0, 2.0, 33), note(4.0, 2.0, 29)])]);
        let song = of(&arrangement(vec![keys, bass]));
        assert_eq!(song.parts.len(), 2);
        let (keys, bass) = (&song.parts[0], &song.parts[1]);
        assert_eq!((keys.name.as_str(), keys.role, keys.bars.clone()), ("Keys", Role::Chords, 4..8));
        assert_eq!((bass.name.as_str(), bass.role, bass.bars.clone()), ("Sub", Role::Bass, 6..8));
        assert_eq!(keys.notes.len(), 12);
        assert_eq!(keys.notes[0].start, 16 * PPQ);
        assert_eq!(bass.notes[1].start, 28 * PPQ);
        assert_eq!(
            keys.clips,
            [
                ClipPlace { name: "Verse chords".into(), bars: 4..6 },
                ClipPlace { name: "Verse chords".into(), bars: 6..8 }
            ]
        );
        assert_eq!((song.bars(), song.bar(), song.tempo), (8, 4 * PPQ, 128.0));
    }

    fn shifted(by: f64) -> impl Fn(PreviewNote) -> PreviewNote {
        move |n| PreviewNote { start: n.start + by, ..n }
    }

    #[test]
    fn only_what_plays_is_read() {
        let mut muted_lane = lane("Muted track", vec![clip("A", 0.0, 4.0, chord([60, 64, 67]))]);
        muted_lane.muted = true;
        let mut muted_clip = clip("B", 0.0, 4.0, chord([60, 64, 67]));
        muted_clip.muted = true;
        let mut quiet = note(1.0, 1.0, 72);
        quiet.muted = true;
        let lead = lane(
            "Lead",
            vec![
                muted_clip,
                // A note that runs past its clip's end stops there; one that starts after it never sounds.
                clip("C", 4.0, 4.0, vec![note(0.0, 1.0, 72), quiet, note(3.0, 4.0, 74), note(4.0, 1.0, 76)]),
            ],
        );
        let song = of(&arrangement(vec![muted_lane, lead, lane("Empty", Vec::new())]));
        assert_eq!(song.parts.len(), 1);
        let lead = &song.parts[0];
        assert_eq!(
            lead.notes.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>(),
            [(4 * PPQ, PPQ, 72), (7 * PPQ, PPQ, 74)]
        );
        assert_eq!(lead.clips, [ClipPlace { name: "C".into(), bars: 1..2 }]);
    }

    #[test]
    fn a_pattern_gives_each_of_its_channels_a_part() {
        // FL Studio's playlist layout: one pattern on two playlist tracks, holding two channels' notes.
        let mut pattern = clip("Pattern 1", 0.0, 4.0, Vec::new());
        pattern.parts = vec!["FLEX".into(), "FPC".into()];
        let step = PreviewNote { step: true, part: 1, ..note(0.0, 0.01, 36) };
        pattern.notes = vec![note(0.0, 4.0, 60), note(0.0, 4.0, 64), note(0.0, 4.0, 67), step];
        let mut again = pattern.clone();
        again.start = 4.0;
        let song = of(&arrangement(vec![lane("Track 1", vec![pattern]), lane("Track 2", vec![again])]));
        assert_eq!(song.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["FLEX", "FPC"]);
        assert_eq!(song.parts[0].notes.len(), 6);
        assert_eq!(song.parts[0].bars, 0..2);
        // A drum plugin's notes are drums, and a step-sequencer note is a sixteenth long.
        assert!(song.parts[1].drums);
        assert_eq!(song.parts[1].role, Role::Drums);
        assert_eq!(song.parts[1].notes[0].length, PPQ / 4);
        // Two lanes with the same name and no parts named are two instruments.
        let two = of(&arrangement(vec![
            lane("Polysynth", vec![clip("a", 0.0, 4.0, chord([60, 64, 67]))]),
            lane("Polysynth", vec![clip("b", 0.0, 4.0, vec![note(0.0, 1.0, 72)])]),
        ]));
        assert_eq!(two.parts.len(), 2);
    }

    #[test]
    fn a_stretch_of_bars_starts_at_tick_0_and_holds_what_sounds_in_it() {
        // A chord held from the last beat of bar 1 into bar 2, and a note in bar 3.
        let keys =
            lane("Keys", vec![clip("x", 0.0, 12.0, vec![note(3.0, 3.0, 60), note(3.0, 3.0, 64), note(8.0, 8.0, 67)])]);
        let song = of(&arrangement(vec![keys]));
        let bar_2 = song.stretch(0, &(1..2));
        assert_eq!(
            bar_2.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>(),
            [(0, 2 * PPQ, 60), (0, 2 * PPQ, 64)]
        );
        // Held past the stretch's end (and its clip's): it stops with the stretch.
        assert_eq!(song.stretch(0, &(2..3)).iter().map(|n| (n.start, n.length)).collect::<Vec<_>>(), [(0, 4 * PPQ)]);
        assert!(song.stretch(0, &(5..9)).is_empty());
        // No bars, or bars the wrong way round: nothing, not notes of a length below 0.
        assert!(song.stretch(0, &(2..2)).is_empty());
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = 3..0;
        assert!(song.stretch(0, &backwards).is_empty());
        assert!(song.stretch(7, &(0..4)).is_empty());
    }

    #[test]
    fn the_bars_to_fit_to_start_where_the_picked_parts_play_together() {
        let keys = lane(
            "Keys",
            vec![clip(
                "c",
                0.0,
                80.0,
                (0..20).flat_map(|b| chord([57, 60, 64]).into_iter().map(shifted(b as f64 * 4.0))).collect(),
            )],
        );
        let bass = lane("Sub", vec![clip("b", 16.0, 64.0, (0..16).map(|b| note(b as f64 * 4.0, 2.0, 33)).collect())]);
        let song = of(&arrangement(vec![keys, bass]));
        // Both from bar 5: eight bars from there. The chords alone: from the top.
        assert_eq!(song.busiest_bars(&[0, 1], 8), Some(4..12));
        assert_eq!(song.busiest_bars(&[0], 8), Some(0..8));
        // Never past where they stop, and nothing for parts that aren't there.
        assert_eq!(song.busiest_bars(&[1], 64), Some(4..20));
        assert_eq!(song.busiest_bars(&[9], 8), None);
        // Picked twice, a part counts once.
        assert_eq!(song.busiest_bars(&[0, 0, 1], 8), Some(4..12));
        assert_eq!(song.busiest_bars(&[], 8), None);
    }

    #[test]
    fn three_beats_a_bar_and_markers() {
        let mut waltz = arrangement(vec![lane("Keys", vec![clip("w", 3.0, 6.0, chord([60, 64, 67]))])]);
        waltz.beats_per_bar = 3;
        waltz.markers = vec![PreviewMarker { beat: 6.0, name: "Verse".into() }];
        waltz.notes = vec!["Something was left out.".into()];
        let song = of(&waltz);
        assert_eq!(song.bar(), 3 * PPQ);
        // The chord is cut to nothing more than its clip; it starts in bar 2 and runs four beats, into bar 3.
        assert_eq!(song.parts[0].bars, 1..3);
        assert_eq!(song.markers, [Marker { bar: 2, name: "Verse".into() }]);
        assert_eq!(song.warnings, ["Something was left out."]);
        assert_eq!(song.grid().bar(), 3 * PPQ);
    }

    #[test]
    fn damaged_numbers_are_left_out_not_followed() {
        let wild = lane(
            "Wild",
            vec![
                clip("far", 1.0e20, 4.0, chord([60, 64, 67])),
                clip("nan", f64::NAN, 4.0, chord([60, 64, 67])),
                clip(
                    "ok",
                    0.0,
                    4.0,
                    vec![
                        note(f64::INFINITY, 1.0, 60),
                        note(0.0, f64::NAN, 62),
                        note(-8.0, 2.0, 64),
                        note(1.0, 1.0, 65),
                    ],
                ),
            ],
        );
        let song = of(&arrangement(vec![wild]));
        assert_eq!(song.parts.len(), 1);
        assert_eq!(song.parts[0].notes.iter().map(|n| n.key).collect::<Vec<_>>(), [65]);
    }

    #[test]
    fn the_song_worked_on_is_the_one_saved_last() {
        let save = |path: &str, mtime: i64| mo_library::Save {
            path: path.to_owned(),
            name: paths::stem(paths::file_name(path)).to_owned(),
            daw: "FL Studio".to_owned(),
            folder: "C:\\Songs".to_owned(),
            mtime,
            ..mo_library::Save::default()
        };
        let now = 1_800_000_000;
        let saves = vec![
            save("C:\\Songs\\Night drive_12.flp", now - 500),
            // An older number saved again since: it's the one being worked on.
            save("C:\\Songs\\Night drive_11.flp", now - 100),
            save("C:\\Songs\\Dawn.bwproject", now - 300),
            // A date still to come says nothing; nor does a file that's gone, or one no reader takes.
            save("C:\\Songs\\Clock wrong.flp", now + 90_000_000),
            mo_library::Save { file_missing: true, ..save("C:\\Songs\\Gone.flp", now - 10) },
            save("C:\\Songs\\Sketch.als", now - 20),
        ];
        let songs = mo_library::songs::group_songs(&saves, &mo_library::SongChoices::default());
        let dates = RealDates::at(now);
        let names = |most| -> Vec<String> {
            worked_on(&songs, &dates, most).into_iter().map(|(_, save)| save.name.clone()).collect()
        };
        assert_eq!(names(5), ["Night drive_11", "Dawn"]);
        assert_eq!(names(1), ["Night drive_11"]);
        assert_eq!(worked_on(&songs, &dates, 5)[0].0.save_count(), 2);
        assert!(worked_on(&[], &dates, 5).is_empty());
    }

    fn recipe(daw: &str, name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../mo-formats/tests/fixtures/recipe").join(daw).join(name)
    }

    #[test]
    fn real_saves_read_as_they_play() {
        // The recipe saves (made by hand for tests, rule 4's one exception): C D E G on FL Keys, and on Polysynth.
        let fl = read(&recipe("fl", "MO Recipe 02 notes.flp")).unwrap();
        assert_eq!(fl.view, fl::arrangement::BITWIG_VIEW);
        assert_eq!(fl.tempo, 128.0);
        let keys: Vec<u8> = fl.parts.iter().flat_map(|p| &p.notes).map(|n| n.key % 12).collect();
        assert_eq!(keys, [0, 2, 4, 7], "{fl:?}");
        let bitwig = read(&recipe("bitwig", "MO Recipe 02 notes.bwproject")).unwrap();
        let keys: Vec<u8> = bitwig.parts.iter().flat_map(|p| &p.notes).map(|n| n.key % 12).collect();
        assert_eq!(keys, [0, 2, 4, 7], "{bitwig:?}");
        // A save with no notes has no parts; 3/4 is read as 3 beats a bar.
        assert!(read(&recipe("fl", "MO Recipe 01 empty.flp")).unwrap().parts.is_empty());
        assert_eq!(read(&recipe("fl", "MO Recipe 16 time signature and marker.flp")).unwrap().beats_per_bar, 3);
    }

    #[test]
    fn damaged_files_are_failures_not_crashes() {
        assert!(matches!(from_bytes(Path::new("x.flp"), b"not a project"), Err(Unread::Failed(_))));
        assert!(matches!(from_bytes(Path::new("x.bwproject"), b"not a project"), Err(Unread::Failed(_))));
        assert!(matches!(from_bytes(Path::new("x.song"), b"not a project"), Err(Unread::Failed(_))));
        assert!(matches!(from_bytes(Path::new("x.mid"), b"MThd"), Err(Unread::Failed(_))));
        assert!(matches!(read(Path::new("/no/such/song.flp")), Err(Unread::InUse(_))));
    }
}
