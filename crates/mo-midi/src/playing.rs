//! A saved song's parts as they play: each instrument's notes through the song.
//!
//! [`crate::extract`] reads a project for the library: there every pattern or clip is a part of its own, moved to
//! start in bar 1. Here the parts keep their place against each other, so the chords and the bass of bars 17 to 24 of
//! the song being worked on can be taken together, and a tune from the library fitted over them ([`crate::fit`]).
//!
//! Only what plays is read: nothing from a muted track or clip, no muted notes, nothing from an FL Studio channel
//! switched off in the Channel rack. An FL Studio project is read one channel a part, from the song as the converter
//! reads it (`mo_formats::fl::song`: where each channel's patterns play, and patterns played in pattern mode one
//! after another); a Bitwig project by its arranger, or its scenes one after another when the arranger is empty,
//! and a Studio One song by its tracks (`mo_formats::arrangement`, what the Songs page's preview draws).
//!
//! Files are only read. A damaged file gives an error in plain words, never a crash. Reading a project takes about as
//! long as the Songs page's preview of it does: callers read off the window's thread.

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

/// The layout an FL Studio project is read in ([`Song::view`]): a part for each channel that's switched on, its notes
/// where its patterns play.
pub const FL_VIEW: &str = "FL Studio channels";

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
    /// An FL Studio channel with no plugin on it (a sampler, or a layer): a sampler's notes say how far from the
    /// sample's own pitch it plays, and the project doesn't say what that pitch is (most are drum hits). Left out of
    /// [`Song::harmony_parts`] and [`Song::melody_parts`]; picked by hand, its notes are taken as written, which is
    /// right when the sample is tuned to the keys.
    pub sample: bool,
    /// What kind of part it is, guessed a phrase at a time from what it plays and its names.
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
    /// Which of the project's layouts this is: [`FL_VIEW`], or the one the Songs page names (Bitwig's arranger or
    /// scenes, Studio One's tracks).
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
    /// fitted over when nothing is picked. No drums, and no samples ([`Playing::sample`]).
    pub fn harmony_parts(&self) -> Vec<usize> {
        self.parts_that(|role| matches!(role, Role::Chords | Role::Arp | Role::Bass))
    }

    /// The song's melodies, as places in [`Song::parts`]: what chords are fitted under when nothing is picked. No
    /// samples ([`Playing::sample`]).
    pub fn melody_parts(&self) -> Vec<usize> {
        self.parts_that(|role| role == Role::Melody)
    }

    fn parts_that(&self, is: impl Fn(Role) -> bool) -> Vec<usize> {
        self.parts.iter().enumerate().filter(|(_, p)| !p.drums && !p.sample && is(p.role)).map(|(at, _)| at).collect()
    }

    /// Whether the part at `part` sounds anywhere in `bars` (counted from 0): whether [`Song::stretch`] has a note.
    pub fn plays_in(&self, part: usize, bars: &Range<i64>) -> bool {
        let bar = self.bar();
        let (from, to) = (bars.start.saturating_mul(bar), bars.end.saturating_mul(bar));
        self.parts.get(part).is_some_and(|part| part.notes.iter().any(|n| sounds_in(n, from, to)))
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
            .filter(|n| sounds_in(n, from, to))
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
        let bar = self.bar();
        // Where each part starts sounding (+1) and stops (-1), in bars. A part's notes come in order, so its bars
        // with sound are gathered as runs: what this costs goes by the notes, however long the song or a held note.
        let mut edges: Vec<(i64, i32)> = Vec::new();
        let mut last = None;
        for part in places.iter().filter_map(|&p| self.parts.get(p)).filter(|p| !p.notes.is_empty()) {
            last = last.max(Some(part.bars.end));
            let mut run: Option<Range<i64>> = None;
            for n in &part.notes {
                let from = n.start.div_euclid(bar);
                let to = n.start.saturating_add(n.length.max(1) - 1).div_euclid(bar) + 1;
                match &mut run {
                    Some(run) if from <= run.end => run.end = run.end.max(to),
                    _ => edges.extend(run.replace(from..to).into_iter().flat_map(|r| [(r.start, 1), (r.end, -1)])),
                }
            }
            edges.extend(run.into_iter().flat_map(|r| [(r.start, 1), (r.end, -1)]));
        }
        // In order, a part stopping at a bar before one starting there.
        edges.sort_unstable();
        let (mut sounding, mut fullest, mut start) = (0, 0, None);
        for (at, change) in edges {
            sounding += change;
            if sounding > fullest {
                (fullest, start) = (sounding, Some(at));
            }
        }
        let start = start?;
        Some(start..last?.min(start.saturating_add(most.max(1))))
    }
}

/// Whether a note sounds between two ticks (the first counted, the last not).
fn sounds_in(n: &NoteEvent, from: i64, to: i64) -> bool {
    n.start < to && n.start.saturating_add(n.length) > from
}

/// Reads a project from disk as it plays.
pub fn read(path: &Path) -> Result<Song, Unread> {
    let raw = std::fs::read(path).map_err(|e| extract::unread_file(&e))?;
    from_bytes(path, &raw)
}

/// A project's bytes, already read from `path`, as the song plays.
pub fn from_bytes(path: &Path, raw: &[u8]) -> Result<Song, Unread> {
    let text = path.to_string_lossy();
    // Reading it and working out its parts both: a damaged file's numbers reach both.
    mo_catch::catch(AssertUnwindSafe(|| by_ending(&text, raw))).unwrap_or_else(|_| {
        Err(Unread::Failed("The reader stopped on it: it looks damaged in a way the reader doesn't know yet".into()))
    })
}

/// The song, read by its file's ending.
fn by_ending(path: &str, raw: &[u8]) -> Result<Song, Unread> {
    match case::to_lower(paths::extension(paths::file_name(path))).as_str() {
        ".flp" => {
            let flp = fl::events::load(raw).map_err(|_| extract::damaged("not an FL Studio project, or damaged"))?;
            Ok(of_fl(&flp))
        }
        ".bwproject" => {
            let doc = bitwig::decode(raw).map_err(|_| extract::damaged("not a Bitwig project, or damaged"))?;
            let views = bitwig::arrangement::read(&doc);
            views.first().map(of).ok_or_else(|| Unread::Failed("It has no arrangement to read".into()))
        }
        ".song" => {
            let mut song = studio_one::Song::open(raw).map_err(|e| extract::damaged(&e.to_string()))?;
            Ok(of(&song.arrangement()))
        }
        _ => Err(Unread::Failed("It isn't a project this app reads (FL Studio, Bitwig or Studio One)".into())),
    }
}

/// A part as it's put together, with what [`finish`] needs to know of it.
struct Gathered {
    part: Playing,
    /// Its clips are its own (a track's clips), so their names say something of it. A pattern's name doesn't: it's
    /// every channel's in the pattern.
    own_clips: bool,
}

impl Gathered {
    fn new(name: &str, plugin: &str, sample: bool, own_clips: bool) -> Gathered {
        let part = Playing {
            name: name.to_owned(),
            plugin: plugin.to_owned(),
            notes: Vec::new(),
            drums: false,
            sample,
            role: Role::Other,
            role_why: String::new(),
            bars: 0..0,
            clips: Vec::new(),
        };
        Gathered { part, own_clips }
    }
}

/// Beats as [`PPQ`] ticks. None for a number no song holds (a damaged file's), so it's left out, not followed.
fn ticks_of(beats: f64) -> Option<i64> {
    (beats.is_finite() && beats.abs() <= MOST_BEATS).then(|| (beats * PPQ as f64).round_ties_even() as i64)
}

/// The bars a clip takes up, counted from 0, by its start and length in ticks.
fn clip_bars(start: i64, length: i64, bar: i64) -> Range<i64> {
    let last = (start + length.max(1) - 1).div_euclid(bar);
    start.div_euclid(bar).max(0)..last.max(0) + 1
}

/// An FL Studio project's song: a part for each channel that's switched on and plays notes.
fn of_fl(flp: &fl::events::Flp<'_>) -> Song {
    let song = fl::song::read_song_with(flp, false);
    let (beats_per_bar, markers) = fl::arrangement::bars_and_markers(flp);
    let beats_per_bar = beats_per_bar.max(1);
    let bar = PPQ * i64::from(beats_per_bar);
    // A damaged header can say 0 ticks a beat: FL's default stands in, as in the arrangement's layouts.
    let per_beat = if song.ppq > 0 { f64::from(song.ppq) } else { 96.0 };
    let ticks = |at: i64| ticks_of(at as f64 / per_beat);
    let mut gathered = Vec::new();
    // A channel switched off in the Channel rack (its green light off) plays nothing. One whose light is automated
    // counts as on: what it would play while off is muted, clip by clip and note by note.
    for c in song.channels.iter().filter(|c| c.enabled && !c.notes.is_empty()) {
        let plugin = c.instrument.as_ref().map_or("", |i| i.name.as_str());
        let mut found = Gathered::new(&c.name, plugin, c.instrument.is_none(), false);
        // What plays of it across the song: no muted notes, none from a muted clip or track, none past its clip.
        found.part.notes = c
            .notes
            .iter()
            .filter_map(|n| {
                let start = ticks(n.start).filter(|start| *start >= 0)?;
                // A step-sequencer note has no length of its own: a sixteenth, as the library's parts give it.
                let length = if n.step { PPQ / 4 } else { ticks(n.length)?.clamp(1, analyser::MAX_HOLD) };
                Some(NoteEvent { start, length, muted: false, step: false, ..*n })
            })
            .collect();
        found.part.clips = c
            .clips
            .iter()
            .filter(|k| !k.muted && !k.on_muted_track && k.notes.iter().any(|n| !n.muted))
            .filter_map(|k| {
                Some(ClipPlace { name: k.name.clone(), bars: clip_bars(ticks(k.start)?, ticks(k.length)?, bar) })
            })
            .collect();
        if !found.part.notes.is_empty() {
            gathered.push(found);
        }
    }
    // What the song couldn't hold, then notes left out above MIDI's keys or damaged note events.
    let mut warnings: Vec<String> = song.warning.map(str::to_owned).into_iter().collect();
    warnings.extend(fl::notes::trouble(flp));
    Song {
        view: FL_VIEW,
        tempo: if song.tempo > 0.0 { song.tempo } else { 120.0 },
        beats_per_bar,
        parts: finish(gathered, bar),
        markers: markers
            .iter()
            .filter_map(|m| Some(Marker { bar: ticks_of(m.beat)?.div_euclid(bar).max(0), name: m.name.clone() }))
            .collect(),
        warnings,
    }
}

/// The parts of an arrangement already read (the Songs page's preview holds one). A clip that holds several
/// instruments' notes (an FL Studio pattern in the playlist's own layout) gives each instrument its part, the same
/// instrument on two lanes one part.
///
/// For an FL Studio project use [`read`]: it reads the song itself, and knows which channels play a sample, not a
/// plugin.
pub fn of(arrangement: &Arrangement) -> Song {
    let beats_per_bar = arrangement.beats_per_bar.max(1);
    let bar = PPQ * i64::from(beats_per_bar);
    // FL Studio's lane-a-channel layout: the clips on a lane are patterns, which other channels play in too.
    let patterns = arrangement.view == fl::arrangement::BITWIG_VIEW;
    // (the lane it's on, or none for an instrument named by its clips; its name) -> its place in `parts`.
    let mut places: Vec<(Option<usize>, String)> = Vec::new();
    let mut parts: Vec<Gathered> = Vec::new();
    for (lane_at, lane) in arrangement.lanes.iter().enumerate().filter(|(_, l)| !l.muted && !l.disabled) {
        // The lane's own part, and each clip's named instruments' parts: looked up once, not for every note.
        let mut own: Option<usize> = None;
        for clip in lane.clips.iter().filter(|c| !c.muted) {
            let (Some(clip_start), Some(clip_length)) = (ticks_of(clip.start), ticks_of(clip.length)) else {
                continue;
            };
            let mut named: Vec<Option<usize>> = vec![None; clip.parts.len()];
            // Which parts this clip gave notes to, to note where the clip plays in each.
            let mut gave: Vec<usize> = Vec::new();
            for n in clip.notes.iter().filter(|n| !n.muted) {
                let (Some(at), Some(length)) = (ticks_of(n.start), ticks_of(n.length)) else { continue };
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
                        parts.push(match name {
                            Some(name) => Gathered::new(name, "", false, false),
                            None => Gathered::new(&lane.name, &lane.detail, false, !patterns),
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
                if let Some(found) = parts.get_mut(part_at) {
                    found.part.notes.push(NoteEvent {
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
            for at in gave {
                if let Some(found) = parts.get_mut(at) {
                    found
                        .part
                        .clips
                        .push(ClipPlace { name: clip.name.clone(), bars: clip_bars(clip_start, clip_length, bar) });
                }
            }
        }
    }
    let markers = arrangement
        .markers
        .iter()
        .filter_map(|m| Some(Marker { bar: ticks_of(m.beat)?.div_euclid(bar).max(0), name: m.name.clone() }))
        .collect();
    Song {
        view: arrangement.view,
        tempo: arrangement.tempo,
        beats_per_bar,
        parts: finish(parts, bar),
        markers,
        warnings: arrangement.notes.clone(),
    }
}

/// The parts once every note is in: in order, with the bars each plays in and what kind of part it is.
fn finish(gathered: Vec<Gathered>, bar: i64) -> Vec<Playing> {
    gathered
        .into_iter()
        .map(|Gathered { mut part, own_clips }| {
            part.notes.sort_by_key(|n| (n.start, n.key));
            part.clips.sort_by_key(|c| (c.bars.start, c.bars.end));
            let first = part.notes.first().map_or(0, |n| n.start);
            let end = part.notes.iter().map(|n| n.start + n.length).max().unwrap_or(0);
            part.bars = first.div_euclid(bar)..(end - 1).div_euclid(bar) + 1;
            part.drums = extract::is_unpitched_plugin(&part.name) || extract::is_unpitched_plugin(&part.plugin);
            (part.role, part.role_why) = role_of(&part, own_clips);
            part
        })
        .collect()
}

/// What kind of part it is, and why. It's judged a phrase at a time, as the library's parts are
/// ([`analyser::split`]), and the kind most of its notes are in wins: a part's notes through a whole song aren't one
/// phrase (an arp that comes in at bar 33 plays few notes for the song's length, and a lead's verse and chorus
/// together wander further than either).
fn role_of(part: &Playing, own_clips: bool) -> (Role, String) {
    // Its names: its own, and each of its own clips' once.
    let mut names: Vec<&str> = Vec::new();
    for c in part.clips.iter().filter(|_| own_clips) {
        if !names.contains(&c.name.as_str()) {
            names.push(&c.name);
        }
    }
    let hints = extract::project_hints(&part.name, &names.join(" "));
    // By its first notes: as many as the library's longest part holds.
    let judged = part.notes.get(..analyser::MAX_NOTES).unwrap_or(&part.notes);
    if part.drums {
        return analyser::role(&Shape::of(judged), judged, &hints, true);
    }
    // Each kind found, why (the first time), and how many notes are in phrases of that kind.
    let mut kinds: Vec<(Role, String, usize)> = Vec::new();
    for piece in analyser::split(judged, None) {
        let (role, why) = analyser::role(&Shape::of(&piece.notes), &piece.notes, &hints, false);
        match kinds.iter_mut().find(|(kind, ..)| *kind == role) {
            Some((.., notes)) => *notes += piece.notes.len(),
            None => kinds.push((role, why, piece.notes.len())),
        }
    }
    // The kind with the most notes: the one found first when two have as many.
    let mut most: Option<(Role, String, usize)> = None;
    for kind in kinds {
        if most.as_ref().is_none_or(|(.., notes)| kind.2 > *notes) {
            most = Some(kind);
        }
    }
    most.map_or((Role::Other, String::new()), |(role, why, _)| (role, why))
}

/// The saves written last, newest first, each with its song: the first is the song being worked on. Only saves whose
/// date is real (not one still to come, or before 1981), that were there and readable at the last scan, of a kind
/// [`read`] takes; `most` of them at the most, one for each song.
pub fn worked_on<'a>(
    songs: impl IntoIterator<Item = &'a mo_library::Song>,
    dates: &RealDates,
    most: usize,
) -> Vec<(&'a mo_library::Song, &'a mo_library::Save)> {
    // The projects the MIDI library reads are the ones read here.
    let reads = |s: &mo_library::Save| {
        !s.file_missing && s.error.is_none() && dates.seconds(s.mtime) && extract::project_version(&s.path).is_some()
    };
    let mut newest: Vec<(&mo_library::Song, &mo_library::Save)> = songs
        .into_iter()
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

        // A part that stops as another starts: they never play together, so the first bar either plays in. And two
        // that meet again after a gap: the first bar both play in, which is after the gap.
        let run = |from: i64, bars: i64| -> PreviewClip {
            clip("r", from as f64 * 4.0, bars as f64 * 4.0, (0..bars).map(|b| note(b as f64 * 4.0, 4.0, 60)).collect())
        };
        let song = of(&arrangement(vec![lane("A", vec![run(0, 4)]), lane("B", vec![run(4, 4)])]));
        assert_eq!(song.busiest_bars(&[0, 1], 8), Some(0..8));
        let song = of(&arrangement(vec![lane("A", vec![run(0, 2), run(6, 2)]), lane("B", vec![run(6, 4)])]));
        assert_eq!(song.busiest_bars(&[0, 1], 2), Some(6..8));
        assert_eq!(song.busiest_bars(&[0, 1], 8), Some(6..10));
        // A note held for a thousand bars is one run of bars, not a thousand steps for each part.
        let drone = lane("Drone", vec![clip("d", 0.0, 4096.0, vec![note(0.0, 4096.0, 36)])]);
        let song = of(&arrangement(vec![drone, lane("B", vec![run(1000, 4)])]));
        assert_eq!(song.busiest_bars(&[0, 1], 8), Some(1000..1008));
    }

    #[test]
    fn a_part_is_judged_a_phrase_at_a_time() {
        // Eight bars of sixteenths going up and down a chord, coming in at bar 33: an arp. Counted over the song's
        // length it plays few notes a beat, which a tune does.
        let arp: Vec<PreviewNote> =
            (0..128).map(|i| note(i as f64 * 0.25, 0.25, [57, 60, 64, 69, 64, 60][i % 6])).collect();
        let late = lane("Track 5", vec![clip("c", 128.0, 32.0, arp)]);
        // Chords for eight bars, then, after a long rest, a bar of single notes: most of it is chords.
        let mut fill = clip("d", 80.0, 4.0, (0..4).map(|b| note(b as f64, 1.0, 72)).collect());
        fill.name = "c".into();
        let keys = lane(
            "Track 6",
            vec![
                clip(
                    "c",
                    0.0,
                    32.0,
                    (0..8).flat_map(|b| chord([57, 60, 64]).into_iter().map(shifted(b as f64 * 4.0))).collect(),
                ),
                fill,
            ],
        );
        let song = of(&arrangement(vec![late, keys]));
        assert_eq!(song.parts.iter().map(|p| p.role).collect::<Vec<_>>(), [Role::Arp, Role::Chords], "{song:?}");
    }

    #[test]
    fn a_clips_name_says_what_its_own_track_plays_and_a_patterns_does_not() {
        // Single notes around middle C, in a clip called "Bass".
        let line = || vec![note(0.0, 1.0, 60), note(1.0, 1.0, 62), note(2.0, 1.0, 64), note(3.0, 1.0, 62)];
        let track = of(&arrangement(vec![lane("Synth", vec![clip("Bass", 0.0, 4.0, line())])]));
        assert_eq!(track.parts[0].role, Role::Bass, "{track:?}");
        // An FL Studio pattern of that name holds whichever channels play in it: the name isn't this channel's.
        let mut patterns = arrangement(vec![lane("Synth", vec![clip("Bass", 0.0, 4.0, line())])]);
        patterns.view = fl::arrangement::BITWIG_VIEW;
        assert_eq!(of(&patterns).parts[0].role, Role::Melody);
        let mut pattern = clip("Bass", 0.0, 4.0, line());
        pattern.parts = vec!["Synth".into()];
        assert_eq!(of(&arrangement(vec![lane("Track 1", vec![pattern])])).parts[0].role, Role::Melody);
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
        let dates = RealDates::at(now);
        let songs = mo_library::songs::group_songs(&saves, &mo_library::SongChoices::default(), &dates);
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
        assert_eq!(fl.view, FL_VIEW);
        assert_eq!(fl.tempo, 128.0);
        let keys: Vec<u8> = fl.parts.iter().flat_map(|p| &p.notes).map(|n| n.key % 12).collect();
        assert_eq!(keys, [0, 2, 4, 7], "{fl:?}");
        // FL Keys is a plugin, so its notes are pitches: a part the song's own tune or chords can be.
        assert_eq!((fl.parts[0].plugin.as_str(), fl.parts[0].sample), ("FL Keys", false));
        let bitwig = read(&recipe("bitwig", "MO Recipe 02 notes.bwproject")).unwrap();
        let keys: Vec<u8> = bitwig.parts.iter().flat_map(|p| &p.notes).map(|n| n.key % 12).collect();
        assert_eq!(keys, [0, 2, 4, 7], "{bitwig:?}");
        // A save with no notes has no parts.
        assert!(read(&recipe("fl", "MO Recipe 01 empty.flp")).unwrap().parts.is_empty());
        // 3/4 is read as 3 beats a bar, and the marker is in the bar it was put in: FL's on beat 4 (bar 2), Bitwig's
        // on beat 5 (bar 2 too).
        let verse = [Marker { bar: 1, name: "Verse".into() }];
        let fl = read(&recipe("fl", "MO Recipe 16 time signature and marker.flp")).unwrap();
        assert_eq!((fl.beats_per_bar, fl.markers.as_slice()), (3, verse.as_slice()));
        let bitwig = read(&recipe("bitwig", "MO Recipe 16 time signature and marker.bwproject")).unwrap();
        assert_eq!((bitwig.beats_per_bar, bitwig.markers.as_slice()), (3, verse.as_slice()));
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
