//! Dragging things out of the app into a DAW (or a folder): what a drag carries, and the files it hands over.
//!
//! A sound goes out as its own file, exactly where it is on disk, so the DAW gets the real sample (rule 2: never a
//! copy, never moved; the C# app's `SoundsViewModel.MakeDrag`). A clip or a track from a song's arrangement goes out as
//! what the DAW needs to play it: its notes as a new MIDI file (`.mid`) the app writes, its audio as the samples' own
//! files, found on this computer the way the arrangement found them (rule 13). A preset from the Presets page goes out
//! as a Bitwig preset wherever one can be made (mo-presets' `drag` says what it carries), else as its own file; a
//! curve as its curve file in the app's Curves folder.
//!
//! Each drag also says what the card that follows the mouse shows ([`Payload::cards`], drawn by `drag_card`): one
//! line per thing carried, with its name, a few facts and a small picture of it.
//!
//! **The files a drag makes** (rule 28, Solomon 2026-11-02: the app cleans up after itself): a clip's MIDI file, a
//! preset made for the drag. A DAW takes a MIDI file's notes into its project and names the clip after the file, so
//! each file is named after its clip ("Chords.mid"), and a preset after itself ("Serum - Lead.bwpreset"). Two drags
//! can both carry a "Chords", so each drag gets a new folder of its own right in the system's Temp folder,
//! `mo-drag-<process>-<count>`, never one that's there already. In it, before any file, goes `mo-drag.txt`: a first
//! line saying it's the app's, then each file's exact name and size. Only what that list names is ever cleared
//! ([`sweep`]): a plain file (never a link), named as listed with one of the endings the app gives ([`MADE_ENDINGS`]),
//! of exactly the size the app wrote, a day after it was made (a DAW may be slow to read it, or keep an import box
//! open), then the list, then the folder if it's empty. Never a folder by its name or age alone: the Temp folder can be
//! anywhere, even the home folder. When the app closes it clears its own drags' files a minute after they were made
//! ([`finish_up`]). Audio never goes through Temp, nor a preset file that goes as it is: a DAW plays a sample from
//! where it is, so a copy there would go missing from the song a day later.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewLane};
use mo_formats::midi::write::write_clip;
use mo_formats::textscan::{is_device_name, safe_file_name};
use mo_formats::{MidiClip, NoteAttribute, NoteEvent};
use mo_presets::curves::Curve;
use mo_presets::drag::{Carried, File};

use crate::songs::thousands;

/// What a line on the card is: a sound file, a MIDI clip or track, a preset, or a curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Audio,
    Midi,
    Preset,
    Curve,
}

/// A note in a card's small piano roll: where it starts and how long it lasts, as parts of the whole thing shown (0 to
/// 1), and its key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbNote {
    pub start: f32,
    pub length: f32,
    pub key: u8,
}

/// The small picture at a card line's left.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Thumb {
    /// Nothing to draw: the card shows a plain sign for its kind.
    #[default]
    None,
    /// A sound's waveform: the loudest it gets (0 to 1) in each slice, from its start, as the Sounds page stores them.
    Wave(Vec<f32>),
    /// A clip's notes.
    Notes(Vec<ThumbNote>),
    /// A curve's shape: points across (0 to 1) and up (0 to 1, 1 at the top), joined by straight lines.
    Line(Vec<(f32, f32)>),
}

/// One line on the card: a sound, a clip or a track, a preset or a curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// Its name: "Kick 04", "Chords".
    pub title: String,
    /// A few facts: "Kick · C · 0.4 s", "MIDI · 12 notes · 2 bars".
    pub detail: String,
    pub kind: Kind,
    /// The colour its picture is drawn in (0xRRGGBB): a sound's type colour, a track's colour. 0 for none (the card
    /// takes the accent).
    pub tint: u32,
    pub thumb: Thumb,
}

/// What a drag carries out of the app: the files the DAW gets (in the order they land) and what the card shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Payload {
    pub files: Vec<PathBuf>,
    pub cards: Vec<Card>,
    /// Under the lines: "1 sound", "3 sounds", "1 MIDI clip".
    pub caption: String,
    /// The lines are tracks, numbered in the order they land.
    pub numbered: bool,
}

/// "1 sound", "3 sounds", "1,204 notes": a count and its word, `plural` when it isn't one.
pub fn count_of(n: usize, one: &str, plural: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{} {plural}", thousands(n)) }
}

// ---- Sounds ----------------------------------------------------------------------------------------------------

/// A sound to drag out of the Sounds page: its file, and its line on the card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundItem {
    pub path: PathBuf,
    pub card: Card,
}

/// The most peaks a card's waveform keeps: the card draws it about 44 points wide.
const THUMB_PEAKS: usize = 96;

/// A sound's line on the card: its name, the facts its tile shows ("Kick · C · 0.4 s"), its waveform (`wave`: the
/// page's list may come without them, [`mo_sounds::SoundsDb::waves_apart`]), in its type's colour (`tint`, 0xRRGGBB).
pub fn sound_card(s: &mo_sounds::Sound, wave: &mo_sounds::Peaks, tint: u32) -> Card {
    Card {
        title: s.name().to_owned(),
        detail: crate::sounds::facts_line(s),
        kind: Kind::Audio,
        tint,
        thumb: wave_thumb(wave),
    }
}

/// A sound's waveform as its line on the card draws it: none when it has none.
pub fn wave_thumb(wave: &mo_sounds::Peaks) -> Thumb {
    if wave.is_empty() { Thumb::None } else { Thumb::Wave(fewer_peaks(&wave.to_vec(), THUMB_PEAKS)) }
}

/// Sounds to drag out, as they are on disk: each file once (the first time it's given), in the order given. None when
/// there's nothing to carry. Doesn't look at the disk: the caller leaves out files that aren't there.
pub fn sounds(items: Vec<SoundItem>) -> Option<Payload> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut cards = Vec::new();
    for item in items {
        if item.path.as_os_str().is_empty() || !seen.insert(same_file_key(&item.path)) {
            continue;
        }
        files.push(item.path);
        cards.push(item.card);
    }
    if files.is_empty() {
        return None;
    }
    let caption = count_of(files.len(), "sound", "sounds");
    Some(Payload { files, cards, caption, numbered: false })
}

/// The same file, however its name is written: Windows doesn't mind letter case or which slash.
fn same_file_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) { s.replace('/', "\\").to_lowercase() } else { s.into_owned() }
}

/// At most `most` peaks (0 to 1), each the loudest of the ones it stands for, so a short card picture keeps every
/// click.
fn fewer_peaks(peaks: &[f32], most: usize) -> Vec<f32> {
    let kept = if peaks.len() <= most || most == 0 {
        peaks.iter().copied().map(loudness).collect()
    } else {
        loudest(peaks, most)
    };
    kept.into_iter().map(|v| v.min(1.0)).collect()
}

/// How loud a waveform's peak is: its size, and none for a broken number.
pub(crate) fn loudness(peak: f32) -> f32 {
    if peak.is_finite() { peak.abs() } else { 0.0 }
}

/// A waveform's peaks as `slots` slices, each as loud as the loudest of the peaks it stands for ([`share`]), so a
/// click in a long sound is kept; a short one is stretched.
pub(crate) fn loudest(peaks: &[f32], slots: usize) -> Vec<f32> {
    (0..slots)
        .map(|i| {
            let (a, b) = share(i, slots, peaks.len());
            peaks.get(a..b).unwrap_or_default().iter().copied().map(loudness).fold(0.0, f32::max)
        })
        .collect()
}

/// The part of `len` things that slot `i` of `slots` stands for: at least one thing, so a short list is stretched.
pub(crate) fn share(i: usize, slots: usize, len: usize) -> (usize, usize) {
    let at = |i: usize| (i as u128 * len as u128 / slots.max(1) as u128) as usize;
    let a = at(i).min(len.saturating_sub(1));
    (a, at(i + 1).max(a + 1).min(len))
}

// ---- A MIDI page part ---------------------------------------------------------------------------------------------

/// A part from the MIDI page, ready to drag out: its heard notes as one MIDI file named after it (the same file Send to
/// Bitwig writes: muted notes left out, starting at its first bar, lasting its bars), and its card line with a small
/// piano roll in `tint`. The DAW plays it at its own project's tempo. An error is what to say instead.
pub fn from_part(name: &str, notes: &[NoteEvent], bars: i32, tint: u32) -> Result<Outgoing, String> {
    if notes.is_empty() {
        return Err("Its notes aren't in the index any more, so there's nothing to drag.".to_owned());
    }
    let clip = crate::midi::part_clip(name, notes, bars);
    let heard: Vec<NoteEvent> = clip.tracks.iter().flat_map(|(_, n)| n.iter().copied()).collect();
    if heard.is_empty() {
        return Err("Every note in it is muted, so there's nothing to drag.".to_owned());
    }
    let bytes = write_clip(&clip).map_err(|e| format!("Couldn't make the MIDI file: {e}."))?.bytes;
    let ppq = f64::from(clip.ppq.max(1));
    let beats: Vec<BeatNote> = heard
        .iter()
        .map(|n| BeatNote {
            start: n.start as f64 / ppq,
            length: n.length as f64 / ppq,
            key: n.key,
            velocity: n.velocity,
            step: n.step,
        })
        .collect();
    let length = clip.end.unwrap_or(0) as f64 / ppq;
    let card = Card {
        title: name.to_owned(),
        detail: ["MIDI".to_owned(), count_of(heard.len(), "note", "notes"), bars_text(length, 4)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        kind: Kind::Midi,
        tint,
        thumb: Thumb::Notes(thumb_notes(&beats, length)),
    };
    Ok(Outgoing {
        made: vec![MadeFile { name: format!("{}.mid", safe_file_name(name)), bytes }],
        cards: vec![card],
        caption: "1 MIDI clip".to_owned(),
        ..Outgoing::default()
    })
}

// ---- Presets and curves from the Presets page ------------------------------------------------------------------

/// A preset to drag out of the Presets page, as mo-presets says it goes ([`mo_presets::drag`]: a Bitwig preset
/// wherever one can be made, else its own file), with its line on the card: its name, then what it goes as and its
/// plugin ("Bitwig preset · Serum"), in its part's colour (`tint`, 0xRRGGBB; 0 for none).
pub fn preset(carried: Carried, name: &str, plugin: &str, tint: u32) -> Outgoing {
    let what = if carried.bitwig {
        "Bitwig preset"
    } else {
        match &carried.file {
            File::There(path) => preset_words(&path.file_name().unwrap_or_default().to_string_lossy()),
            File::Made { name, .. } => preset_words(name),
        }
    };
    let card = Card {
        title: name.to_owned(),
        detail: [what, plugin].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · "),
        kind: Kind::Preset,
        tint,
        thumb: Thumb::None,
    };
    let mut out = Outgoing { cards: vec![card], caption: "1 preset".to_owned(), ..Outgoing::default() };
    match carried.file {
        File::There(path) => out.as_they_are.push(path),
        File::Made { name, bytes } => out.made.push(MadeFile { name, bytes }),
    }
    out
}

/// What a preset file is, by its ending, in a few words: "VST2 preset", "VST3 preset".
fn preset_words(file_name: &str) -> &'static str {
    const WORDS: [(&str, &str); 7] = [
        (".bwpreset", "Bitwig preset"),
        (".fxp", "VST2 preset"),
        (".fxb", "VST2 bank"),
        (".vstpreset", "VST3 preset"),
        (".vst3state", "VST3 settings"),
        (".flstate", "FL Studio settings"),
        (".aupreset", "AU preset"),
    ];
    let lower = file_name.to_ascii_lowercase();
    WORDS.iter().find(|(ending, _)| lower.ends_with(ending)).map_or("Preset", |(_, words)| words)
}

/// A curve's line on the card: its name, "Curve · 2 bars", and its shape.
pub fn curve_card(curve: &Curve) -> Card {
    Card {
        title: curve.name.clone(),
        detail: ["Curve".to_owned(), crate::presets::beats_words(curve.length_beats)]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        kind: Kind::Curve,
        tint: 0,
        thumb: Thumb::Line(crate::presets::curve_line_points(&curve.points)),
    }
}

/// A curve to drag out: its file (which mo-presets writes in the app's Curves folder, so it goes as it is) and its
/// line on the card ([`curve_card`]).
pub fn curve(path: PathBuf, card: Card) -> Payload {
    Payload { files: vec![path], cards: vec![card], caption: "1 curve".to_owned(), numbered: false }
}

// ---- Clips and tracks from a song's arrangement ----------------------------------------------------------------

/// What in an arrangement a drag picks up: one clip, or a whole lane (a track), by their places in
/// [`Arrangement::lanes`] and [`PreviewLane::clips`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Clip { lane: usize, clip: usize },
    Lane(usize),
}

/// A file a drag writes, a MIDI file or a preset: its name in the drag's folder with its ending ("Chords.mid",
/// "Serum - Lead.bwpreset"; one of [`MADE_ENDINGS`]) and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MadeFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// What a drag carries, made ready before anything is written: the files to make (a clip's MIDI file, a preset made
/// for the drag), the files that go as they are (the samples a clip plays, a preset file), and the card.
/// [`Outgoing::write`] writes the made ones and gives the [`Payload`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outgoing {
    pub made: Vec<MadeFile>,
    /// The files that go as they are on this computer: a clip's samples as found, each once, in the order they first
    /// play; a preset file.
    pub as_they_are: Vec<PathBuf>,
    pub cards: Vec<Card>,
    pub caption: String,
    pub numbered: bool,
    /// How many of the samples played aren't on this computer (each once): their clips are left out.
    pub missing: usize,
}

/// A note on its way into a MIDI file, in beats from what's dragged's start: the ticks a beat are chosen once every
/// note is in.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BeatNote {
    start: f64,
    length: f64,
    key: u8,
    velocity: u16,
    step: bool,
}

/// A clip's notes and samples, timed from what's dragged's start.
struct Gathered {
    /// Each part's name with its notes, in the order the parts first play.
    parts: Vec<(String, Vec<BeatNote>)>,
    /// Each sample found, with the path the project saved, once.
    samples: Vec<(PathBuf, String)>,
    /// How many samples played aren't on this computer.
    missing: usize,
}

/// The usual ticks a beat, smallest first: the MIDI file uses the first that places every note exactly (FL Studio's
/// own is usually 96), so nothing moves even a tick. A song timed finer than all of them gets 960, rounded.
const PPQS: [u16; 8] = [96, 192, 384, 480, 768, 960, 1920, 3840];
/// The ticks a beat when no usual one places every note exactly.
const FALLBACK_PPQ: u16 = 960;
/// The most notes a card's little piano roll draws: enough to show the shape of a whole track.
const THUMB_NOTES: usize = 400;

/// A clip or a track from `arrangement`, ready to drag out. Its notes become one MIDI file named after it, with a track
/// for each instrument in it (an FL Studio pattern holds several) timed from its start (a track's from the song's
/// start, so it lands in the same place); muted notes are left out, as MIDI has none, and so are a track's muted
/// clips, as they don't play. FL's step-sequencer notes get the length FL plays them for. Its audio clips' samples come
/// from `found` (the samples the arrangement found on this computer, by the path the project saved), each once, and
/// only when they're still there now: each is looked for on the disk, so this is never called on the window's thread.
/// `waves` gives a sample's waveform for its card line, when it's been read. An error is what to say instead: there's
/// nothing to drag.
pub fn from_arrangement(
    arrangement: &Arrangement,
    pick: Pick,
    found: &HashMap<String, PathBuf>,
    waves: &dyn Fn(&str) -> Option<Vec<f32>>,
) -> Result<Outgoing, String> {
    let (lane, clips, name, length) = match pick {
        Pick::Clip { lane, clip } => {
            let l = arrangement.lanes.get(lane).ok_or("That clip isn't in the song any more.")?;
            let c = l.clips.get(clip).ok_or("That clip isn't in the song any more.")?;
            // Just this clip, timed from its own start, even when it's muted: it was picked.
            (l, vec![(c, 0.0)], if c.name.trim().is_empty() { l.name.clone() } else { c.name.clone() }, c.length)
        }
        Pick::Lane(lane) => {
            let l = arrangement.lanes.get(lane).ok_or("That track isn't in the song any more.")?;
            if l.kind == LaneKind::Group {
                return Err("A group's tracks are dragged one at a time.".to_owned());
            }
            let playing: Vec<(&PreviewClip, f64)> = l.clips.iter().filter(|c| !c.muted).map(|c| (c, c.start)).collect();
            let end = playing.iter().map(|(c, _)| c.end()).fold(0.0, f64::max);
            (l, playing, l.name.clone(), end)
        }
    };
    let gathered = gather(lane, &clips, found);
    let notes: usize = gathered.parts.iter().map(|(_, n)| n.len()).sum();
    if notes == 0 && gathered.samples.is_empty() {
        return Err(nothing_to_carry(pick, lane, &clips, gathered.missing));
    }

    // The colour it's drawn in on the view: a clip's own, else its track's; a track's own, else its first clip's.
    let tint = match pick {
        Pick::Clip { .. } => clips.first().and_then(|(c, _)| c.colour).or(lane.colour),
        Pick::Lane(_) => lane.colour.or_else(|| clips.first().and_then(|(c, _)| c.colour)),
    }
    .unwrap_or(0);
    let bars = bars_text(length, arrangement.beats_per_bar);
    let mut out = Outgoing { missing: gathered.missing, ..Outgoing::default() };
    if notes > 0 {
        let midi = midi_clip(&name, &gathered.parts, length);
        let bytes = write_clip(&midi).map_err(|e| format!("Couldn't make the MIDI file: {e}."))?.bytes;
        out.made.push(MadeFile { name: format!("{}.mid", safe_file_name(&name)), bytes });
        for (part, part_notes) in &gathered.parts {
            out.cards.push(Card {
                // One part is the whole MIDI file, named as the DAW names the clip it makes; several are its tracks,
                // each by its instrument.
                title: if gathered.parts.len() == 1 { name.clone() } else { part.clone() },
                detail: ["MIDI".to_owned(), count_of(part_notes.len(), "note", "notes"), bars.clone()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                kind: Kind::Midi,
                tint,
                thumb: Thumb::Notes(thumb_notes(part_notes, length)),
            });
        }
    }
    for (path, saved) in gathered.samples {
        out.cards.push(Card {
            title: mo_formats::paths::file_name(&saved).to_owned(),
            detail: "Audio · the whole sample".to_owned(),
            kind: Kind::Audio,
            tint,
            thumb: waves(&saved)
                .filter(|w| !w.is_empty())
                .map_or(Thumb::None, |w| Thumb::Wave(fewer_peaks(&w, THUMB_PEAKS))),
        });
        out.as_they_are.push(path);
    }
    let tracks = gathered.parts.len();
    out.caption = match (notes > 0, out.as_they_are.len()) {
        (true, 0) if tracks == 1 => "1 MIDI clip".to_owned(),
        (true, 0) => format!("1 MIDI file with {tracks} tracks"),
        (true, n) => format!("1 MIDI file and {}", count_of(n, "sample", "samples")),
        (false, n) => count_of(n, "sample", "samples"),
    };
    // Said on the card, so a track that lands short of some audio isn't a surprise.
    if out.missing > 0 {
        out.caption = format!("{} · {} not on this computer", out.caption, count_of(out.missing, "sample", "samples"));
    }
    out.numbered = out.cards.len() > 1;
    Ok(out)
}

/// Why a clip or track (`lane` itself, or the clips of it that play) has nothing to drag, in the user's words.
fn nothing_to_carry(pick: Pick, lane: &PreviewLane, clips: &[(&PreviewClip, f64)], missing: usize) -> String {
    let what = if matches!(pick, Pick::Clip { .. }) { "This clip" } else { "This track" };
    let has_something = |c: &PreviewClip| !c.notes.is_empty() || !c.audio.is_empty();
    if missing == 1 {
        format!("{what}'s sample isn't on this computer, so there's nothing to drag.")
    } else if missing > 1 {
        format!("{what}'s samples aren't on this computer, so there's nothing to drag.")
    } else if lane.kind == LaneKind::Automation
        || match pick {
            Pick::Clip { .. } => !clips.iter().any(|(c, _)| has_something(c)),
            Pick::Lane(_) => !lane.clips.iter().any(has_something),
        }
    {
        format!("{what} has no notes or audio to drag.")
    } else if !clips.iter().any(|(c, _)| has_something(c)) {
        // A track whose clips with something in them are all muted.
        "This track's clips are all muted, so there's nothing to drag.".to_owned()
    } else {
        format!("{what}'s notes are all muted, so there's nothing to drag.")
    }
}

/// The notes and samples of `clips` (each with where its start goes, in beats from what's dragged's start), part by
/// part. Looks for each sample on the disk.
fn gather(lane: &PreviewLane, clips: &[(&PreviewClip, f64)], found: &HashMap<String, PathBuf>) -> Gathered {
    let mut parts: Vec<(String, Vec<BeatNote>)> = Vec::new();
    let mut samples: Vec<(PathBuf, String)> = Vec::new();
    let mut seen = HashSet::new();
    // Each sample that isn't here once, however many clips play it.
    let mut gone = HashSet::new();
    for (clip, at) in clips {
        for n in clip.notes.iter().filter(|n| !n.muted) {
            let name = clip.parts.get(usize::from(n.part)).filter(|p| !p.trim().is_empty()).unwrap_or(&lane.name);
            let at_part = match parts.iter().position(|(p, _)| p == name) {
                Some(i) => i,
                None => {
                    parts.push((name.clone(), Vec::new()));
                    parts.len() - 1
                }
            };
            if let Some((_, notes)) = parts.get_mut(at_part) {
                notes.push(BeatNote {
                    start: at + n.start,
                    length: n.length,
                    key: n.key,
                    velocity: n.velocity,
                    step: n.step,
                });
            }
        }
        for part in &clip.audio {
            match found.get(&part.path).filter(|p| usable_sample(p)) {
                Some(path) => {
                    if seen.insert(same_file_key(path)) {
                        samples.push((path.clone(), part.path.clone()));
                    }
                }
                None => {
                    gone.insert(part.path.as_str());
                }
            }
        }
    }
    Gathered { parts, samples, missing: gone.len() }
}

/// A sample a DAW can open: a whole path (not one in a DAW's own words, "%FLStudioFactoryData%…", which the
/// arrangement keeps when it knows the file is the DAW's but not where), to a file that's there now. It was found when
/// the song was read, but its drive may have been unplugged or its folder renamed since.
fn usable_sample(path: &Path) -> bool {
    path.is_absolute() && path.is_file()
}

/// The notes as a MIDI clip `length` beats long, at the first usual ticks a beat that places them all exactly.
fn midi_clip(name: &str, parts: &[(String, Vec<BeatNote>)], length: f64) -> MidiClip {
    let times = || parts.iter().flat_map(|(_, notes)| notes).flat_map(|n| [n.start, n.length]).chain([length]);
    let fits = |q: u16| {
        times().all(|t| {
            let ticks = t * f64::from(q);
            !ticks.is_finite() || (ticks - ticks.round()).abs() < 1e-6
        })
    };
    let ppq = PPQS.iter().copied().find(|&q| fits(q)).unwrap_or(FALLBACK_PPQ);
    let tick = |b: f64| if b.is_finite() { (b * f64::from(ppq)).round().max(0.0) as i64 } else { 0 };
    let tracks = parts
        .iter()
        .map(|(part, notes)| {
            let notes = notes
                .iter()
                .filter(|n| n.start.is_finite() && n.length.is_finite())
                .map(|n| NoteEvent {
                    start: tick(n.start),
                    length: tick(n.length).max(1),
                    key: n.key,
                    velocity: n.velocity,
                    muted: false,
                    step: n.step,
                    attribute: NoteAttribute::NONE,
                })
                .collect();
            (part.clone(), notes)
        })
        .collect();
    MidiClip { name: name.to_owned(), ppq, tracks, end: Some(tick(length)).filter(|&e| e > 0) }
}

/// A part's notes for a card's little piano roll, as parts of `length`; every so many when there are lots.
fn thumb_notes(notes: &[BeatNote], length: f64) -> Vec<ThumbNote> {
    let span = if length > 0.0 && length.is_finite() {
        length
    } else {
        notes.iter().map(|n| n.start + n.length).filter(|e| e.is_finite()).fold(0.0, f64::max).max(1.0)
    };
    let every = notes.len().div_ceil(THUMB_NOTES).max(1);
    notes
        .iter()
        .step_by(every)
        .filter(|n| n.start.is_finite() && n.length.is_finite())
        .map(|n| ThumbNote {
            start: (n.start / span).clamp(0.0, 1.0) as f32,
            length: (n.length / span).clamp(0.0, 1.0) as f32,
            key: n.key,
        })
        .collect()
}

/// "2 bars", "1.5 bars"; nothing when it can't be told.
fn bars_text(beats: f64, beats_per_bar: u32) -> String {
    if !(beats > 0.0 && beats.is_finite()) || beats_per_bar == 0 {
        return String::new();
    }
    let bars = beats / f64::from(beats_per_bar);
    let rounded = (bars * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 1e-9 {
        count_of(rounded.round() as usize, "bar", "bars")
    } else {
        format!("{rounded:.1} bars")
    }
}

impl Outgoing {
    /// Writes the files to make into a new drag folder in `temp` (none when there are none) and gives what the drag
    /// carries: the made files first (a clip's MIDI file), then the ones that go as they are, in the order they land.
    /// Writes to the disk, so never on the window's thread.
    pub fn write(self, temp: &Path) -> io::Result<Payload> {
        let mut files = if self.made.is_empty() { Vec::new() } else { write_drag_files(temp, &self.made)? };
        files.extend(self.as_they_are);
        Ok(Payload { files, cards: self.cards, caption: self.caption, numbered: self.numbered })
    }
}

// ---- The drag folders in Temp, and clearing them ---------------------------------------------------------------

/// The start of each drag folder's name: `mo-drag-<process>-<count>`.
const FOLDER: &str = "mo-drag-";
/// The list of a drag folder's files.
const MANIFEST: &str = "mo-drag.txt";
/// The list's first line, which says it's the app's.
const MANIFEST_HEADER: &str = "Music Organiser drag files 1";
/// A list longer than this isn't one the app wrote.
const MOST_MANIFEST: u64 = 1 << 20;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a drag's files stay at least, before the start-up sweep clears them: a DAW may be slow to read them.
pub const KEPT: Duration = DAY;
/// How old this run's drag files must be for [`finish_up`] to clear them when the app closes: the DAW has long read
/// them by then, unless it was let go just now.
pub const KEPT_AT_CLOSE: Duration = Duration::from_secs(60);
/// A time up to this far after now still counts as just made (the clock was a little fast, or was set back since);
/// further on, it was plainly wrong, and counts for nothing.
const SKEW: Duration = DAY;

/// Counts the drag folders this run makes, so each has a name of its own.
static FOLDERS_MADE: AtomicU64 = AtomicU64::new(1);

/// Writes `files` (their names are made safe and kept apart here, each keeping its ending: "Chords.mid",
/// "Chords 2.mid", "Lead.bwpreset") into a new drag folder in `temp`: the list first, with each file's exact name and
/// size from the bytes in hand, then the files, so a crash between them leaves nothing unlisted. A write that fails
/// (Temp full, say) takes back everything this one made: a file cut short doesn't have the size on its list, so the
/// sweep would leave it for good. Gives the files' paths, in order. An error before anything is made for a file
/// without one of the [`MADE_ENDINGS`].
pub fn write_drag_files(temp: &Path, files: &[MadeFile]) -> io::Result<Vec<PathBuf>> {
    write_drag_files_with(temp, files, |file, bytes| file.write_all(bytes))
}

/// [`write_drag_files`], putting each file's bytes in with `put` (a test's fails part way, as a full disk does).
fn write_drag_files_with(
    temp: &Path,
    files: &[MadeFile],
    mut put: impl FnMut(&mut fs::File, &[u8]) -> io::Result<()>,
) -> io::Result<Vec<PathBuf>> {
    let names = unique_names(files.iter().map(|f| f.name.as_str()))?;
    let folder = new_folder(temp)?;
    let mut list = format!("{MANIFEST_HEADER}\n");
    for (name, f) in names.iter().zip(files) {
        list.push_str(&format!("{}\t{name}\n", f.bytes.len()));
    }
    // Each file made, the list first, noted as soon as it's there: one whose write then fails is this call's too.
    let mut made = Vec::with_capacity(files.len() + 1);
    let mut write = |name: &str, bytes: &[u8]| {
        let path = folder.join(name);
        // Never one that's there already: the folder is new, so anything in it that this didn't make isn't the app's.
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        made.push(path);
        put(&mut file, bytes)
    };
    let written = write(MANIFEST, list.as_bytes())
        .and_then(|()| names.iter().zip(files).try_for_each(|(name, f)| write(name, &f.bytes)));
    if let Err(e) = written {
        take_back(&folder, &made);
        return Err(e);
    }
    // The files, without the list.
    Ok(made.split_off(1))
}

/// Takes back what a drag write that failed made in its new folder (`made`, the list first): the files, then the list
/// and the folder once they've all gone. A file that wouldn't go keeps the list, so the sweep clears it later when it
/// was written whole.
fn take_back(folder: &Path, made: &[PathBuf]) {
    if let Some((list, files)) = made.split_first() {
        let mut all_gone = true;
        for file in files {
            if fs::remove_file(file).is_err() {
                all_gone = false;
            }
        }
        if !all_gone || fs::remove_file(list).is_err() {
            return;
        }
    }
    // Only goes when empty, as in the sweep.
    let _ = fs::remove_dir(folder);
}

/// The names made safe, each keeping its ending (one of [`MADE_ENDINGS`], in small letters), and kept apart, letter
/// case aside (Windows doesn't tell "a" from "A"). An error for a name without one of those endings: a drag makes no
/// other kind of file.
fn unique_names<'a>(names: impl Iterator<Item = &'a str>) -> io::Result<Vec<String>> {
    let mut used = HashSet::new();
    names
        .map(|name| {
            let Some((stem, ending)) = made_ending(name) else {
                return Err(io::Error::new(ErrorKind::InvalidInput, format!("a drag doesn't make files like {name}")));
            };
            let stem = safe_file_name(stem);
            let mut out = format!("{stem}{ending}");
            let mut n = 2;
            while !used.insert(out.to_lowercase()) {
                out = format!("{stem} {n}{ending}");
                n += 1;
            }
            Ok(out)
        })
        .collect()
}

/// The endings of the files a drag makes ([`MadeFile`]): a clip's MIDI file; a Bitwig preset made for the drag; a
/// song's sound as the plugin preset an export writes (as FL Studio, Studio One and Bitwig keep them). The sweep only
/// ever clears a file with one of these.
pub const MADE_ENDINGS: [&str; 10] = [
    ".mid",
    ".bwpreset",
    ".fxp",
    ".fxb",
    ".vstpreset",
    ".vst3state",
    ".flstate",
    ".aupreset",
    ".preset",
    ".multipreset",
];

/// A drag file's name cut into what comes before its ending and that ending (one of [`MADE_ENDINGS`], letter case
/// aside): None when it has none of them, or nothing before it.
fn made_ending(name: &str) -> Option<(&str, &'static str)> {
    MADE_ENDINGS.iter().find_map(|&ending| {
        let cut = name.len().checked_sub(ending.len())?;
        let (stem, end) = (name.get(..cut)?, name.get(cut..)?);
        (!stem.is_empty() && end.eq_ignore_ascii_case(ending)).then_some((stem, ending))
    })
}

/// A new folder for one drag, right in `temp`, never one that's there already.
fn new_folder(temp: &Path) -> io::Result<PathBuf> {
    let process = std::process::id();
    for _ in 0..1000 {
        let n = FOLDERS_MADE.fetch_add(1, Ordering::Relaxed);
        let folder = temp.join(format!("{FOLDER}{process}-{n}"));
        match fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            // Left by an earlier run that had the same process number: take the next count.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(ErrorKind::AlreadyExists, "every drag folder name tried is taken"))
}

/// What clearing drag files removed, and what it couldn't (in use), which is tried again next time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub removed: usize,
    pub bytes: u64,
    pub failed: usize,
}

impl Swept {
    fn add(&mut self, other: Swept) {
        self.removed += other.removed;
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.failed += other.failed;
    }
}

/// Clears the drag files in `temp` made a day or more before `now` (see the top of this file). Reads and removes
/// files, so never on the window's thread.
pub fn sweep(temp: &Path, now: SystemTime) -> Swept {
    sweep_folders(temp, now)
}

/// Clears this run's own drag files in `temp` that are a minute old or more, as the app closes. Quick: it looks only
/// at the folders this run made, by their names, and at nothing when it made none.
pub fn finish_up(temp: &Path, now: SystemTime) -> Swept {
    let process = std::process::id();
    let mut done = Swept::default();
    for n in 1..FOLDERS_MADE.load(Ordering::Relaxed) {
        let folder = temp.join(format!("{FOLDER}{process}-{n}"));
        if real_folder(&folder) {
            done.add(sweep_folder(&folder, now, KEPT_AT_CLOSE));
        }
    }
    done
}

fn sweep_folders(temp: &Path, now: SystemTime) -> Swept {
    let mut done = Swept::default();
    let Ok(entries) = fs::read_dir(temp) else { return done };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_str().and_then(folder_numbers).is_none() {
            continue;
        }
        let folder = entry.path();
        if real_folder(&folder) {
            done.add(sweep_folder(&folder, now, KEPT));
        }
    }
    done
}

/// A real folder right there: not a link or a junction to one somewhere else.
fn real_folder(folder: &Path) -> bool {
    fs::symlink_metadata(folder).is_ok_and(|m| m.file_type().is_dir())
}

/// The process and count in a drag folder's name, `mo-drag-<process>-<count>`, both all digits; None for any other
/// name.
fn folder_numbers(name: &str) -> Option<(u32, u64)> {
    let (process, count) = name.strip_prefix(FOLDER)?.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(process) || !digits(count) {
        return None;
    }
    Some((process.parse().ok()?, count.parse().ok()?))
}

/// Clears one drag folder: the files its list names that are still as the app wrote them and `kept` old, then the
/// list once none of those is left, then the folder if nothing else is in it.
fn sweep_folder(folder: &Path, now: SystemTime, kept: Duration) -> Swept {
    let mut done = Swept::default();
    let manifest = folder.join(MANIFEST);
    let Ok(meta) = fs::symlink_metadata(&manifest) else { return done };
    if !meta.file_type().is_file() || meta.len() > MOST_MANIFEST || age(&meta, now) < kept {
        return done;
    }
    let Some(listed) = fs::read_to_string(&manifest).ok().and_then(|t| read_manifest(&t)) else { return done };
    // A listed file that's still the app's but couldn't go (in use, or not old enough): the list stays for it.
    let mut left = false;
    for (size, name) in listed {
        let path = folder.join(&name);
        let Ok(m) = fs::symlink_metadata(&path) else { continue };
        // Changed since (another size, a link, a folder): it isn't the app's any more, so it stays, and doesn't keep
        // the list.
        if !m.file_type().is_file() || m.len() != size {
            continue;
        }
        if age(&m, now) < kept {
            left = true;
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                done.removed += 1;
                done.bytes = done.bytes.saturating_add(size);
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            // Another account's, in a Temp folder shared by everyone (Linux's /tmp): not ours to clear, and not a fault.
            Err(e) if e.kind() == ErrorKind::PermissionDenied && !cfg!(windows) => left = true,
            Err(_) => {
                done.failed += 1;
                left = true;
            }
        }
    }
    if !left {
        if fs::remove_file(&manifest).is_ok() {
            done.removed += 1;
            done.bytes = done.bytes.saturating_add(meta.len());
        }
        // Only goes when empty: anything else put in it stays, and so does the folder.
        let _ = fs::remove_dir(folder);
    }
    done
}

/// A drag folder's list, as (size, name) for each file; None when it isn't one the app wrote (another first line, a
/// line that doesn't read, a name that isn't one the app gives a drag's file).
fn read_manifest(text: &str) -> Option<Vec<(u64, String)>> {
    let mut lines = text.lines();
    if lines.next()? != MANIFEST_HEADER {
        return None;
    }
    let mut out = Vec::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let (size, name) = line.split_once('\t')?;
        let size = size.parse::<u64>().ok()?;
        if !plain_drag_name(name) {
            return None;
        }
        out.push((size, name.to_owned()));
    }
    Some(out)
}

/// A name the app gives a drag's file: one name with no folder in it (no slashes, no ':' for an NTFS stream, not "."
/// or ".."), no control letters, not a Windows device's, ending in one of [`MADE_ENDINGS`] with something before it.
fn plain_drag_name(name: &str) -> bool {
    made_ending(name).is_some()
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(|c| c < ' ')
        && !name.starts_with(' ')
        && !is_device_name(name)
        && !name.eq_ignore_ascii_case(MANIFEST)
}

/// How old a file is, from the latest of when it was made and last changed (and, off Windows, when its details last
/// changed): a copy keeps the changed time of what it was copied from. A time up to [`SKEW`] after `now` counts as
/// just made; further on, as old as can be.
pub(crate) fn age(meta: &fs::Metadata, now: SystemTime) -> Duration {
    let mut latest = meta.modified().ok();
    if let Ok(made) = meta.created() {
        latest = latest.max(Some(made));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let changed = u64::try_from(meta.ctime()).ok().map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s));
        latest = latest.max(changed);
    }
    match latest {
        None => Duration::MAX,
        Some(t) => match t.duration_since(now) {
            Ok(ahead) if ahead > SKEW => Duration::MAX,
            Ok(_) => Duration::ZERO,
            Err(behind) => behind.duration(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::arrangement::{AudioPart, PreviewNote};
    use mo_formats::midi::smf;

    fn card(title: &str) -> Card {
        Card { title: title.into(), ..Card::default() }
    }

    #[test]
    fn a_midi_page_part_drags_as_one_midi_clip() {
        let n = |start, muted| NoteEvent {
            start,
            length: 96,
            key: 60,
            velocity: 50_000,
            muted,
            step: false,
            attribute: NoteAttribute::NONE,
        };
        let out = from_part("Keys · Pattern 3", &[n(0, false), n(96, false), n(192, true)], 2, 0x33aa77).unwrap();
        assert_eq!(out.made.len(), 1);
        assert!(out.made[0].name.ends_with(".mid"));
        assert!(out.made[0].bytes.starts_with(b"MThd"));
        assert_eq!(out.caption, "1 MIDI clip");
        assert_eq!(out.cards[0].detail, "MIDI · 2 notes · 2 bars");
        assert!(matches!(&out.cards[0].thumb, Thumb::Notes(t) if t.len() == 2));
        assert!(from_part("x", &[n(0, true)], 1, 0).is_err());
    }

    #[test]
    fn sounds_go_out_once_each_in_order() {
        let item = |p: &str, t: &str| SoundItem { path: PathBuf::from(p), card: card(t) };
        let p = sounds(vec![item("/s/Kick.wav", "Kick"), item("/s/Snare.wav", "Snare"), item("/s/Kick.wav", "Again")])
            .unwrap();
        assert_eq!(p.files, [PathBuf::from("/s/Kick.wav"), PathBuf::from("/s/Snare.wav")]);
        assert_eq!(p.cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Kick", "Snare"]);
        assert_eq!(p.caption, "2 sounds");
        assert!(!p.numbered);
        assert_eq!(sounds(vec![item("/s/Kick.wav", "Kick")]).unwrap().caption, "1 sound");
        assert_eq!(sounds(vec![item("", "Nothing")]), None);
        assert_eq!(sounds(Vec::new()), None);
    }

    #[test]
    fn counts_have_their_word_and_thousands() {
        assert_eq!(count_of(1, "note", "notes"), "1 note");
        assert_eq!(count_of(0, "note", "notes"), "0 notes");
        assert_eq!(count_of(1204, "note", "notes"), "1,204 notes");
    }

    #[test]
    fn a_long_waveform_keeps_its_loudest_peaks() {
        let mut peaks = vec![0.1f32; 1000];
        peaks[503] = 0.9;
        peaks[10] = f32::NAN;
        let fewer = fewer_peaks(&peaks, 10);
        assert_eq!(fewer.len(), 10);
        assert_eq!(fewer[5], 0.9);
        assert!(fewer.iter().all(|v| (0.0..=1.0).contains(v)));
        // A peak's size, at most 1.
        assert_eq!(fewer_peaks(&[0.5, 2.0, -0.25, f32::NAN], 10), [0.5, 1.0, 0.25, 0.0]);
        // Stretched when there are fewer peaks than slices.
        assert_eq!(loudest(&[0.5, 0.25], 4), [0.5, 0.5, 0.25, 0.25]);
    }

    #[test]
    fn slices_share_out_what_they_stand_for() {
        assert_eq!(share(0, 4, 2), (0, 1));
        assert_eq!(share(3, 4, 2), (1, 2));
        assert_eq!(share(1, 2, 10), (5, 10));
        assert_eq!(share(5, 0, 0), (0, 0));
    }

    fn note(start: f64, length: f64, key: u8) -> PreviewNote {
        PreviewNote { start, length, key, velocity: 40000, ..PreviewNote::default() }
    }

    fn song(lanes: Vec<PreviewLane>) -> Arrangement {
        Arrangement { view: "Test", tempo: 120.0, beats_per_bar: 4, lanes, markers: Vec::new(), notes: Vec::new() }
    }

    fn no_waves(_: &str) -> Option<Vec<f32>> {
        None
    }

    /// The MIDI file's notes as (track name, start, length, key), at the file's own ticks a beat.
    fn read_back(bytes: &[u8]) -> (u16, Vec<(String, i64, i64, u8)>) {
        let parts = smf::read_bytes(bytes, "x.mid").unwrap();
        let ppq = parts.first().map_or(0, |p| p.ppq);
        let notes = parts
            .iter()
            .flat_map(|p| p.notes.iter().map(move |n| (p.track_name.clone(), n.start, n.length, n.key)))
            .collect();
        (ppq, notes)
    }

    #[test]
    fn an_fl_pattern_clip_goes_out_as_a_track_for_each_channel() {
        let mut clip = PreviewClip { name: "Drums".into(), start: 8.0, length: 4.0, ..PreviewClip::default() };
        clip.parts = vec!["Kick".into(), "Hat".into()];
        // FL's steps: a tick long (1/96 beat), all on C5, on two channels.
        let step = |start: f64, part: u16| PreviewNote { step: true, part, ..note(start, 1.0 / 96.0, 60) };
        clip.notes = vec![step(0.0, 0), step(0.5, 1), step(1.0, 1), step(2.0, 0), note(3.0, 0.5, 64)];
        clip.notes[4].muted = true;
        let mut lane = PreviewLane::new("Track 1", LaneKind::Instrument);
        lane.clips = vec![clip];
        let out =
            from_arrangement(&song(vec![lane]), Pick::Clip { lane: 0, clip: 0 }, &HashMap::new(), &no_waves).unwrap();
        assert_eq!(out.made.len(), 1);
        assert_eq!(out.made[0].name, "Drums.mid");
        assert_eq!(out.caption, "1 MIDI file with 2 tracks");
        assert!(out.numbered);
        assert_eq!(out.cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Kick", "Hat"]);
        assert_eq!(out.cards[0].detail, "MIDI · 2 notes · 1 bar");
        let (ppq, notes) = read_back(&out.made[0].bytes);
        // FL's own 96 ticks a beat places every note exactly.
        assert_eq!(ppq, 96);
        // Timed from the clip's start; each step plays a sixteenth (24 ticks), as FL plays a short sample; the muted
        // note is left out.
        let mut kick: Vec<_> = notes.iter().filter(|n| n.0 == "Kick").map(|n| (n.1, n.2)).collect();
        kick.sort_unstable();
        assert_eq!(kick, [(0, 24), (192, 24)]);
        let mut hat: Vec<_> = notes.iter().filter(|n| n.0 == "Hat").map(|n| (n.1, n.2)).collect();
        hat.sort_unstable();
        assert_eq!(hat, [(48, 24), (96, 24)]);
        assert!(notes.iter().all(|n| n.3 == 60));
    }

    #[test]
    fn a_track_keeps_the_songs_timing_and_leaves_out_muted_clips() {
        let mut lane = PreviewLane::new("Bass", LaneKind::Instrument);
        lane.colour = Some(0x4FB3FF);
        let a = PreviewClip { start: 4.0, length: 4.0, notes: vec![note(0.0, 1.0, 36)], ..PreviewClip::default() };
        let muted = PreviewClip { start: 8.0, length: 4.0, muted: true, notes: vec![note(0.0, 1.0, 40)], ..a.clone() };
        let b = PreviewClip { start: 12.0, length: 4.0, notes: vec![note(1.5, 0.25, 38)], ..PreviewClip::default() };
        lane.clips = vec![a, muted, b];
        let out = from_arrangement(&song(vec![lane]), Pick::Lane(0), &HashMap::new(), &no_waves).unwrap();
        assert_eq!(out.made[0].name, "Bass.mid");
        assert_eq!(out.caption, "1 MIDI clip");
        assert!(!out.numbered);
        assert_eq!(out.cards[0].tint, 0x4FB3FF);
        assert_eq!(out.cards[0].detail, "MIDI · 2 notes · 4 bars");
        let (ppq, notes) = read_back(&out.made[0].bytes);
        let at = |beats: f64| (beats * f64::from(ppq)).round() as i64;
        let mut got: Vec<_> = notes.iter().map(|n| (n.1, n.2, n.3)).collect();
        got.sort_unstable();
        assert_eq!(got, [(at(4.0), at(1.0), 36), (at(13.5), at(0.25), 38)]);
        // A clip picked on its own goes even when it's muted, named as it is: the file and its line on the card.
        let mut lane = PreviewLane::new("Bass", LaneKind::Instrument);
        lane.clips = vec![PreviewClip {
            name: "Bass line".into(),
            muted: true,
            length: 4.0,
            notes: vec![note(0.0, 1.0, 40)],
            ..PreviewClip::default()
        }];
        let out =
            from_arrangement(&song(vec![lane]), Pick::Clip { lane: 0, clip: 0 }, &HashMap::new(), &no_waves).unwrap();
        assert_eq!(out.made[0].name, "Bass line.mid");
        assert_eq!(out.cards[0].title, "Bass line");
    }

    #[test]
    fn notes_off_the_usual_grid_get_960_ticks_a_beat() {
        let mut lane = PreviewLane::new("Keys", LaneKind::Instrument);
        lane.clips = vec![PreviewClip {
            name: "Odd".into(),
            length: 1.0,
            notes: vec![note(0.123_456_7, 0.5, 60)],
            ..PreviewClip::default()
        }];
        let out =
            from_arrangement(&song(vec![lane]), Pick::Clip { lane: 0, clip: 0 }, &HashMap::new(), &no_waves).unwrap();
        let (ppq, notes) = read_back(&out.made[0].bytes);
        assert_eq!(ppq, 960);
        assert_eq!(notes[0].1, 119);
        // Triplets fit 96.
        let mut lane = PreviewLane::new("Keys", LaneKind::Instrument);
        lane.clips =
            vec![PreviewClip { length: 1.0, notes: vec![note(1.0 / 3.0, 1.0 / 3.0, 60)], ..PreviewClip::default() }];
        let out = from_arrangement(&song(vec![lane]), Pick::Lane(0), &HashMap::new(), &no_waves).unwrap();
        assert_eq!(read_back(&out.made[0].bytes).0, 96);
    }

    #[test]
    fn audio_goes_out_as_the_samples_found_each_once() {
        // Real files, as each sample is looked for on the disk; a "%" in a real name is fine.
        let temp = test_temp("samples");
        let (kick, odd) = (temp.join("Kick.wav"), temp.join("100% Kick.wav"));
        fs::write(&kick, b"RIFF").unwrap();
        fs::write(&odd, b"RIFF").unwrap();
        let mut found = HashMap::new();
        found.insert("Kick.wav".to_owned(), kick.clone());
        found.insert("100% Kick.wav".to_owned(), odd.clone());
        // In FL's own words: not a whole path, so not one a DAW can open.
        found.insert("%FLStudioFactoryData%\\Snare.wav".to_owned(), PathBuf::from("%FLStudioFactoryData%\\Snare.wav"));
        // Found when the song was read, but gone since (an unplugged drive).
        found.insert("Unplugged.wav".to_owned(), temp.join("Unplugged.wav"));
        let part = |path: &str| AudioPart {
            path: path.into(),
            start: 0.0,
            length: 4.0,
            from: 0.0,
            seconds_per_beat: None,
            whole_file_beats: None,
            reversed: false,
            ..Default::default()
        };
        let mut lane = PreviewLane::new("Drums", LaneKind::Audio);
        let clip =
            |p: &str, start: f64| PreviewClip { start, length: 4.0, audio: vec![part(p)], ..PreviewClip::default() };
        lane.clips = vec![
            clip("Kick.wav", 0.0),
            clip("Kick.wav", 4.0),
            clip("100% Kick.wav", 6.0),
            clip("%FLStudioFactoryData%\\Snare.wav", 8.0),
            clip("Gone.wav", 12.0),
            clip("Gone.wav", 16.0),
            clip("Unplugged.wav", 20.0),
        ];
        let waves = |p: &str| (p == "Kick.wav").then(|| vec![0.2, 0.8]);
        let out = from_arrangement(&song(vec![lane]), Pick::Lane(0), &found, &waves).unwrap();
        assert!(out.made.is_empty());
        assert_eq!(out.as_they_are, [kick, odd]);
        assert_eq!(out.missing, 3);
        assert_eq!(out.caption, "2 samples · 3 samples not on this computer");
        assert_eq!(out.cards[0].title, "Kick.wav");
        assert_eq!(out.cards[0].thumb, Thumb::Wave(vec![0.2, 0.8]));
        assert_eq!(out.cards[1].title, "100% Kick.wav");
        // A clip whose sample isn't here has nothing to carry.
        let gone = |name: &str| {
            let lane = PreviewLane { clips: vec![clip(name, 0.0)], ..PreviewLane::new("A", LaneKind::Audio) };
            from_arrangement(&song(vec![lane]), Pick::Clip { lane: 0, clip: 0 }, &found, &waves).unwrap_err()
        };
        assert_eq!(gone("Gone.wav"), "This clip's sample isn't on this computer, so there's nothing to drag.");
        assert_eq!(gone("Unplugged.wav"), "This clip's sample isn't on this computer, so there's nothing to drag.");
        let mut two = PreviewLane::new("A", LaneKind::Audio);
        two.clips = vec![clip("Gone.wav", 0.0), clip("Unplugged.wav", 4.0)];
        assert_eq!(
            from_arrangement(&song(vec![two]), Pick::Lane(0), &found, &waves).unwrap_err(),
            "This track's samples aren't on this computer, so there's nothing to drag."
        );
    }

    #[test]
    fn nothing_to_carry_says_why() {
        let mut lane = PreviewLane::new("Volume", LaneKind::Automation);
        lane.clips = vec![PreviewClip { length: 4.0, ..PreviewClip::default() }];
        let mut muted = PreviewLane::new("Keys", LaneKind::Instrument);
        let mut quiet = note(0.0, 1.0, 60);
        quiet.muted = true;
        muted.clips = vec![PreviewClip { length: 4.0, notes: vec![quiet], ..PreviewClip::default() }];
        let group = PreviewLane::new("Drums", LaneKind::Group);
        // Every clip on it muted, though they have notes.
        let mut off = PreviewLane::new("Pads", LaneKind::Instrument);
        off.clips =
            vec![PreviewClip { length: 4.0, muted: true, notes: vec![note(0.0, 1.0, 60)], ..PreviewClip::default() }];
        let empty = PreviewLane::new("Empty", LaneKind::Instrument);
        let a = song(vec![lane, muted, group, off, empty]);
        let pick = |p| from_arrangement(&a, p, &HashMap::new(), &no_waves).unwrap_err();
        assert_eq!(pick(Pick::Clip { lane: 0, clip: 0 }), "This clip has no notes or audio to drag.");
        assert_eq!(pick(Pick::Lane(1)), "This track's notes are all muted, so there's nothing to drag.");
        assert_eq!(
            pick(Pick::Clip { lane: 1, clip: 0 }),
            "This clip's notes are all muted, so there's nothing to drag."
        );
        assert_eq!(pick(Pick::Lane(2)), "A group's tracks are dragged one at a time.");
        assert_eq!(pick(Pick::Lane(3)), "This track's clips are all muted, so there's nothing to drag.");
        assert_eq!(pick(Pick::Lane(4)), "This track has no notes or audio to drag.");
        assert_eq!(pick(Pick::Lane(9)), "That track isn't in the song any more.");
        assert_eq!(pick(Pick::Clip { lane: 1, clip: 4 }), "That clip isn't in the song any more.");
    }

    #[test]
    fn the_card_takes_the_colour_the_view_draws_it_in() {
        let mut lane = PreviewLane::new("Track 1", LaneKind::Instrument);
        lane.colour = Some(0xCC3333);
        let green = PreviewClip {
            length: 4.0,
            notes: vec![note(0.0, 1.0, 60)],
            colour: Some(0x33CC33),
            ..PreviewClip::default()
        };
        let plain = PreviewClip { start: 4.0, colour: None, ..green.clone() };
        lane.clips = vec![green, plain];
        let a = song(vec![lane]);
        let tint = |p| from_arrangement(&a, p, &HashMap::new(), &no_waves).unwrap().cards[0].tint;
        // A clip in its own colour, else its track's; a track in its own.
        assert_eq!(tint(Pick::Clip { lane: 0, clip: 0 }), 0x33CC33);
        assert_eq!(tint(Pick::Clip { lane: 0, clip: 1 }), 0xCC3333);
        assert_eq!(tint(Pick::Lane(0)), 0xCC3333);
    }

    #[test]
    fn bars_read_plainly() {
        assert_eq!(bars_text(8.0, 4), "2 bars");
        assert_eq!(bars_text(4.0, 4), "1 bar");
        assert_eq!(bars_text(6.0, 4), "1.5 bars");
        assert_eq!(bars_text(0.0, 4), "");
        assert_eq!(bars_text(f64::NAN, 4), "");
        assert_eq!(bars_text(8.0, 0), "");
    }

    /// An empty folder of the test's own to stand for the Temp folder (rule 11: never the real one), named for the test.
    fn test_temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-drag-out-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn midi(name: &str, bytes: &[u8]) -> MadeFile {
        MadeFile { name: name.into(), bytes: bytes.to_vec() }
    }

    #[test]
    fn a_drag_folder_lists_its_files_before_they_are_written() {
        let temp = test_temp("writes");
        let files = [midi("Chords.mid", b"abc"), midi("chords.mid", b"defg"), midi("A/B:C.mid", b"h")];
        let paths = write_drag_files(&temp, &files).unwrap();
        let folder = paths[0].parent().unwrap().to_path_buf();
        assert!(folder_numbers(folder.file_name().unwrap().to_str().unwrap()).is_some());
        assert_eq!(folder.parent().unwrap(), temp);
        let names: Vec<_> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Chords.mid", "chords 2.mid", "A_B_C.mid"]);
        assert_eq!(fs::read(&paths[1]).unwrap(), b"defg");
        let list = fs::read_to_string(folder.join(MANIFEST)).unwrap();
        assert_eq!(list, format!("{MANIFEST_HEADER}\n3\tChords.mid\n4\tchords 2.mid\n1\tA_B_C.mid\n"));
        // The next drag gets a folder of its own.
        let again = write_drag_files(&temp, &files[..1]).unwrap();
        assert_ne!(again[0].parent().unwrap(), folder);
    }

    #[test]
    fn a_write_that_fails_takes_back_all_it_made() {
        let temp = test_temp("fails");
        let files = [midi("One.mid", b"1111"), midi("Two.mid", b"2222")];
        // The disk fills up part way through the list, the first file or the second.
        for failing in 1..=3 {
            let mut writes = 0;
            let full = |file: &mut fs::File, bytes: &[u8]| {
                writes += 1;
                if writes < failing {
                    return file.write_all(bytes);
                }
                file.write_all(&bytes[..2])?;
                Err(io::Error::new(ErrorKind::StorageFull, "the disk is full"))
            };
            let e = write_drag_files_with(&temp, &files, full).unwrap_err();
            assert_eq!(e.kind(), ErrorKind::StorageFull);
            // Nothing is left: a file cut short wouldn't match its size on the list, so the sweep would never clear it.
            assert_eq!(fs::read_dir(&temp).unwrap().count(), 0, "failing at write {failing}");
        }
        // With room again, it all goes as before.
        let paths = write_drag_files(&temp, &files).unwrap();
        assert_eq!(fs::read(&paths[1]).unwrap(), b"2222");
    }

    #[test]
    fn the_sweep_clears_only_what_its_list_names_once_a_day_old() {
        let temp = test_temp("sweep");
        let paths = write_drag_files(&temp, &[midi("One.mid", b"1111"), midi("Two.mid", b"22")]).unwrap();
        let folder = paths[0].parent().unwrap().to_path_buf();
        // Someone else's file in it, and another folder of a lookalike name with no list.
        fs::write(folder.join("Mine.txt"), b"keep").unwrap();
        let lookalike = temp.join("mo-drag-1-1x");
        fs::create_dir_all(&lookalike).unwrap();
        fs::write(lookalike.join("x.mid"), b"x").unwrap();
        // Young: nothing goes.
        assert_eq!(sweep(&temp, SystemTime::now()), Swept::default());
        assert!(paths[0].exists());
        // A day on: the listed files and the list go; the other file keeps the folder.
        let later = SystemTime::now() + DAY + Duration::from_secs(60);
        let done = sweep(&temp, later);
        assert_eq!(done.removed, 3);
        assert_eq!(done.failed, 0);
        assert!(!paths[0].exists() && !paths[1].exists() && !folder.join(MANIFEST).exists());
        assert!(folder.join("Mine.txt").exists());
        assert!(lookalike.join("x.mid").exists());
        // With its list gone, the folder isn't the app's to clear any more, even once it's empty.
        fs::remove_file(folder.join("Mine.txt")).unwrap();
        assert_eq!(sweep(&temp, later), Swept::default());
        assert!(folder.exists());
    }

    #[test]
    fn presets_keep_their_endings_and_go_once_a_day_old() {
        let temp = test_temp("presets");
        let files = [
            midi("Serum - Lead.bwpreset", b"BtWg"),
            midi("serum - lead.BWPRESET", b"12"),
            midi("Serum - Lead.fxp", b"CcnK"),
            midi("Pad.vstpreset", b"VST3"),
            midi("Fruity Limiter.flstate", b"1"),
        ];
        let paths = write_drag_files(&temp, &files).unwrap();
        let names: Vec<_> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(
            names,
            [
                "Serum - Lead.bwpreset",
                "serum - lead 2.bwpreset",
                "Serum - Lead.fxp",
                "Pad.vstpreset",
                "Fruity Limiter.flstate"
            ]
        );
        let done = sweep(&temp, SystemTime::now() + DAY * 2);
        assert_eq!((done.removed, done.failed), (6, 0));
        assert!(!paths.iter().any(|p| p.exists()));
        assert!(!paths[0].parent().unwrap().exists());
    }

    #[test]
    fn a_drag_makes_no_other_kind_of_file() {
        let temp = test_temp("endings");
        for name in ["notes.txt", "Lead.exe", ".bwpreset", "Lead", ""] {
            let e = write_drag_files(&temp, &[midi(name, b"1")]).unwrap_err();
            assert_eq!(e.kind(), ErrorKind::InvalidInput, "{name}");
        }
        // Nothing was made for them, not even a folder.
        assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
    }

    #[test]
    fn a_preset_goes_as_mo_presets_says_with_its_card() {
        let made = Carried {
            file: File::Made { name: "Serum - Lead.bwpreset".into(), bytes: b"BtWg".to_vec() },
            bitwig: true,
            why_not: None,
        };
        let out = preset(made, "Lead", "Serum", 0x00FF00);
        assert_eq!(out.made, [midi("Serum - Lead.bwpreset", b"BtWg")]);
        assert!(out.as_they_are.is_empty());
        let c = &out.cards[0];
        assert_eq!((c.title.as_str(), c.detail.as_str()), ("Lead", "Bitwig preset · Serum"));
        assert_eq!((c.kind, c.tint, &c.thumb), (Kind::Preset, 0x00FF00, &Thumb::None));
        assert_eq!(out.caption, "1 preset");
        // A file that goes as it is: nothing is written for it.
        let there = Carried {
            file: File::There(PathBuf::from("/p/Pad.vstpreset")),
            bitwig: false,
            why_not: Some("no template".into()),
        };
        let out = preset(there, "Pad", "Diva", 0);
        assert!(out.made.is_empty());
        assert_eq!(out.cards[0].detail, "VST3 preset · Diva");
        let temp = test_temp("preset-payload");
        assert_eq!(out.write(&temp.join("not there")).unwrap().files, [PathBuf::from("/p/Pad.vstpreset")]);
        // An FL plugin's own settings, by their ending; no plugin, no plugin words.
        let fl = Carried {
            file: File::Made { name: "Fruity Limiter.flstate".into(), bytes: vec![1] },
            bitwig: false,
            why_not: None,
        };
        assert_eq!(preset(fl, "Fruity Limiter", "", 0).cards[0].detail, "FL Studio settings");
        assert_eq!(preset_words("Odd.multipreset"), "Preset");
    }

    #[test]
    fn a_curve_goes_as_its_file_with_its_shape_on_the_card() {
        use mo_formats::bitwig::curve::CurvePoint;
        let c = Curve {
            id: "x".into(),
            name: "Rise".into(),
            points: vec![CurvePoint { x: 0.0, y: 0.0, bend: 0.0 }, CurvePoint { x: 1.0, y: 1.0, bend: 0.0 }],
            length_beats: 8.0,
            hidden: false,
            sources: Vec::new(),
        };
        let card = curve_card(&c);
        assert_eq!((card.title.as_str(), card.detail.as_str(), card.kind), ("Rise", "Curve · 2 bars", Kind::Curve));
        assert_eq!(card.thumb, Thumb::Line(vec![(0.0, 0.0), (0.0, 0.0), (1.0, 1.0), (1.0, 1.0)]));
        let file = PathBuf::from("/d/Curves/Rise 0123.bwcurve");
        let p = curve(file.clone(), card.clone());
        assert_eq!(p, Payload { files: vec![file], cards: vec![card], caption: "1 curve".into(), numbered: false });
    }

    #[test]
    fn a_changed_file_is_not_the_apps_any_more() {
        let temp = test_temp("changed");
        let paths = write_drag_files(&temp, &[midi("One.mid", b"1111"), midi("Two.mid", b"22")]).unwrap();
        let folder = paths[0].parent().unwrap().to_path_buf();
        // Saved over by someone with something else: another size.
        fs::write(&paths[1], b"a different file").unwrap();
        let done = sweep(&temp, SystemTime::now() + DAY * 2);
        assert_eq!(done.removed, 2);
        assert!(!paths[0].exists());
        assert!(paths[1].exists());
        assert!(!folder.join(MANIFEST).exists());
        assert!(folder.exists());
    }

    #[test]
    fn a_list_that_names_other_places_is_not_the_apps() {
        let temp = test_temp("traversal");
        let outside = temp.join("Outside.mid");
        fs::write(&outside, b"12").unwrap();
        let names = [
            "../Outside.mid",
            "..\\Outside.mid",
            "a:b.mid",
            "notes.txt",
            "CON.mid",
            ".mid",
            ".bwpreset",
            "aux.fxp",
            "Lead.bwpreset.exe",
        ];
        for (i, name) in names.iter().enumerate() {
            let folder = temp.join(format!("mo-drag-7-{i}"));
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join(MANIFEST), format!("{MANIFEST_HEADER}\n2\t{name}\n")).unwrap();
        }
        // Another first line.
        let folder = temp.join("mo-drag-7-99");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("Song.mid"), b"12").unwrap();
        fs::write(folder.join(MANIFEST), "My own list\n2\tSong.mid\n").unwrap();
        assert_eq!(sweep(&temp, SystemTime::now() + DAY * 2), Swept::default());
        assert!(outside.exists());
        assert!(folder.join("Song.mid").exists());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_never_followed() {
        use std::os::unix::fs::symlink;
        let temp = test_temp("links");
        let elsewhere = temp.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("Real.mid"), b"12").unwrap();
        fs::write(elsewhere.join(MANIFEST), format!("{MANIFEST_HEADER}\n2\tReal.mid\n")).unwrap();
        // A drag folder's name, but a link to another folder.
        symlink(&elsewhere, temp.join("mo-drag-5-1")).unwrap();
        // A real drag folder whose listed file is a link to a file elsewhere.
        let folder = temp.join("mo-drag-5-2");
        fs::create_dir_all(&folder).unwrap();
        symlink(elsewhere.join("Real.mid"), folder.join("Linked.mid")).unwrap();
        fs::write(folder.join(MANIFEST), format!("{MANIFEST_HEADER}\n2\tLinked.mid\n")).unwrap();
        let done = sweep(&temp, SystemTime::now() + DAY * 2);
        assert!(elsewhere.join("Real.mid").exists());
        assert!(folder.join("Linked.mid").symlink_metadata().is_ok());
        // The link isn't the app's file, so only the list goes.
        assert_eq!(done.removed, 1);
    }

    #[test]
    fn times_far_ahead_count_as_old_and_a_little_ahead_as_new() {
        let temp = test_temp("age");
        let f = temp.join("x");
        fs::write(&f, b"1").unwrap();
        let meta = fs::metadata(&f).unwrap();
        let now = SystemTime::now();
        assert!(age(&meta, now) < Duration::from_secs(60));
        assert!(age(&meta, now + DAY * 3) >= DAY * 3 - Duration::from_secs(60));
        // The file's times are an hour "after" a clock set back an hour: just made.
        assert_eq!(age(&meta, now - Duration::from_secs(3600)), Duration::ZERO);
        // Two days "after" a clock set back two days: the clock was wrong once, so it's old.
        assert_eq!(age(&meta, now - DAY * 2), Duration::MAX);
    }

    #[test]
    fn closing_clears_only_this_runs_folders_a_minute_old() {
        let temp = test_temp("close");
        let paths = write_drag_files(&temp, &[midi("Mine.mid", b"1")]).unwrap();
        let other = temp.join("mo-drag-4294967295-1");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("Theirs.mid"), b"1").unwrap();
        fs::write(other.join(MANIFEST), format!("{MANIFEST_HEADER}\n1\tTheirs.mid\n")).unwrap();
        assert_eq!(finish_up(&temp, SystemTime::now()), Swept::default());
        let done = finish_up(&temp, SystemTime::now() + Duration::from_secs(120));
        assert_eq!(done.removed, 2);
        assert!(!paths[0].exists());
        assert!(!paths[0].parent().unwrap().exists());
        assert!(other.join("Theirs.mid").exists());
    }

    #[test]
    fn folder_names_are_exact() {
        assert_eq!(folder_numbers("mo-drag-123-4"), Some((123, 4)));
        for name in ["mo-drag-123", "mo-drag--4", "mo-drag-1-2-3", "mo-drag-a-1", "Mo-drag-1-2", "mo-drag-1-2 ", "x"] {
            assert_eq!(folder_numbers(name), None, "{name}");
        }
    }

    #[test]
    fn written_files_go_out_midi_first_then_samples() {
        let temp = test_temp("payload");
        let out = Outgoing {
            made: vec![midi("Lead.mid", b"MThd")],
            as_they_are: vec![PathBuf::from("/s/Vox.wav")],
            cards: vec![card("Lead"), card("Vox.wav")],
            caption: "1 MIDI file and 1 sample".into(),
            numbered: true,
            missing: 0,
        };
        let p = out.write(&temp).unwrap();
        assert_eq!(p.files.len(), 2);
        assert!(p.files[0].ends_with("Lead.mid"));
        assert_eq!(p.files[1], PathBuf::from("/s/Vox.wav"));
        // Audio alone writes nothing.
        let audio = Outgoing { as_they_are: vec![PathBuf::from("/s/Vox.wav")], ..Outgoing::default() };
        assert_eq!(audio.write(&temp.join("not there")).unwrap().files, [PathBuf::from("/s/Vox.wav")]);
    }
}
