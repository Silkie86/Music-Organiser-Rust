//! A project laid out the way its DAW's arrangement shows it, for drawing without opening the DAW: the song panel's
//! arrangement preview (spec section 3, Songs, Overview). The same for every DAW; each reader fills it in. Ported from
//! the C# app's `Preview/Arrangement.cs`.
//!
//! Times are in beats from the song's start (a clip's notes and points from the clip's start). The readers never look
//! at the disk: an audio part keeps the path the project saved, and the app finds the file.

use std::path::Path;

/// What a lane holds, which decides how its clips are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Instrument,
    Audio,
    Automation,
    Group,
}

/// A note in a clip, in beats from the clip's start. Velocity keeps MIDI 2.0's 16 bits (rule 22).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreviewNote {
    pub start: f64,
    pub length: f64,
    pub key: u8,
    pub velocity: u16,
    pub muted: bool,
    /// Entered in FL Studio's step sequencer: it has no length of its own (it's drawn a tick long) and FL plays it
    /// until its sound ends, so a MIDI file made from it gives it a length ([`crate::midi::write::lengthen_steps`]).
    pub step: bool,
    /// Which of the clip's [`PreviewClip::parts`] plays it (0 when the clip has none).
    pub part: u16,
}

/// A point on an automation curve, in beats from the clip's start, with the value from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewPoint {
    pub beat: f64,
    pub value: f64,
}

/// How a DAW stretches a piece of audio to the song's tempo, by Bitwig's names (the other DAWs' modes map onto these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stretch {
    /// Plays at its own speed and pitch, whatever the tempo (transposed, it plays faster or slower with the pitch).
    Raw,
    /// Speeds up or slows down with the tempo, and its pitch with it, like a record.
    Repitch,
    /// Bitwig's old Stretch.
    Stretch,
    StretchHd,
    /// zplane's Elastique (Studio One's Drums and Sound).
    Elastique,
    ElastiquePro,
    /// Elastique for one voice at a time (Studio One's Solo).
    ElastiqueSolo,
    /// FL Studio's own Stretch pro.
    StretchPro,
    /// FL Studio picks the mode itself (Auto).
    Auto,
    /// Cut into slices that are moved, not stretched (FL Studio's Slice stretch and Slice map).
    Slice,
    /// A mode the reader doesn't know yet, by the number the file gives it (each DAW has its own numbers).
    Other(i32),
}

impl Stretch {
    /// Bitwig's number for it (an audio playback's 0x4b0), as Solomon's recipe saves showed.
    pub fn from_bitwig(code: i32) -> Stretch {
        match code {
            0 => Stretch::Raw,
            1 => Stretch::Stretch,
            2 => Stretch::Repitch,
            3 => Stretch::StretchHd,
            10 => Stretch::ElastiquePro,
            other => Stretch::Other(other),
        }
    }
}

/// A stretch of an audio file playing in a clip, from `start` for `length` beats (timed from the clip's start). It
/// starts `from` seconds into the file and moves through it at `seconds_per_beat`; when that's only known once the
/// file's length is, `whole_file_beats` says how many beats the whole file takes instead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioPart {
    /// The file as the project saved it.
    pub path: String,
    pub start: f64,
    pub length: f64,
    pub from: f64,
    pub seconds_per_beat: Option<f64>,
    pub whole_file_beats: Option<f64>,
    /// Plays backwards, from the end of what it plays of the file to `from` (FL Studio's reverse switch, Bitwig's
    /// Reverse). `seconds_per_beat` stays positive.
    pub reversed: bool,
    /// How it's stretched to the tempo, when the project says.
    pub stretch: Option<Stretch>,
    /// Transposed this many semitones (0 when it isn't).
    pub semitones: f64,
    /// Its own gain in dB (0 when it's left alone, minus infinity when turned right down).
    pub gain_db: f64,
}

impl AudioPart {
    /// Seconds of the file a beat covers, once the file's length is known; 0 when it can't be told.
    pub fn rate(&self, file_seconds: f64) -> f64 {
        self.seconds_per_beat.unwrap_or(match self.whole_file_beats {
            Some(beats) if beats > 0.0 && file_seconds > 0.0 => file_seconds / beats,
            _ => 0.0,
        })
    }
}

/// A clip on a lane: where it starts and how long it is, in beats from the song's start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewClip {
    pub name: String,
    pub start: f64,
    pub length: f64,
    pub muted: bool,
    pub notes: Vec<PreviewNote>,
    pub audio: Vec<AudioPart>,
    pub points: Vec<PreviewPoint>,
    /// The clip's own colour (0xRRGGBB), when it has one; otherwise it takes its lane's.
    pub colour: Option<u32>,
    /// How long it fades in and out, in beats (0 when it doesn't).
    pub fade_in: f64,
    pub fade_out: f64,
    /// The instruments whose notes the clip holds, by name, when a clip can hold more than one (an FL Studio pattern
    /// holds a part for each channel in it): a note's [`PreviewNote::part`] is its place here. Empty when every note is
    /// the lane's own.
    pub parts: Vec<String>,
}

impl PreviewClip {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }
}

/// A track in the arrangement. `detail` says what plays it (a plugin, a sample) or what it moves (an automation
/// lane's target); `depth` is how deep in groups it sits.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewLane {
    pub name: String,
    pub kind: LaneKind,
    pub detail: String,
    pub colour: Option<u32>,
    pub depth: u32,
    pub muted: bool,
    /// Switched off altogether, so it takes no computer power (Studio One's Disable Track): it's `muted` too.
    pub disabled: bool,
    /// A take that isn't the one playing (one of a Studio One track's other layers), shown under its track: it's
    /// `muted` too, and isn't one of the song's tracks when they're counted.
    pub layer: bool,
    pub clips: Vec<PreviewClip>,
}

impl PreviewLane {
    pub fn new(name: impl Into<String>, kind: LaneKind) -> PreviewLane {
        PreviewLane {
            name: name.into(),
            kind,
            detail: String::new(),
            colour: None,
            depth: 0,
            muted: false,
            disabled: false,
            layer: false,
            clips: Vec::new(),
        }
    }
}

/// A marker on the timeline, in beats.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewMarker {
    pub beat: f64,
    pub name: String,
}

/// A time signature: `top` beats in a bar, each a `1/bottom` note (6/8 is `top` 6, `bottom` 8). Tempo and clip
/// positions count quarter notes whatever it says, so a bar is `top × 4 ÷ bottom` of those: 3 in 6/8, 3.5 in 7/8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSignature {
    top: u32,
    bottom: u32,
}

impl Default for TimeSignature {
    fn default() -> Self {
        TimeSignature::FOUR_FOUR
    }
}

impl TimeSignature {
    pub const FOUR_FOUR: TimeSignature = TimeSignature { top: 4, bottom: 4 };

    /// None unless `top` is 1 to 255 and `bottom` is 1, 2, 4, 8, 16 or 32 (what a MIDI file can say too): what a
    /// damaged save says doesn't count.
    pub const fn new(top: u32, bottom: u32) -> Option<TimeSignature> {
        if top >= 1 && top <= 255 && bottom <= 32 && bottom.is_power_of_two() {
            Some(TimeSignature { top, bottom })
        } else {
            None
        }
    }

    /// `top` quarter notes in a bar: 3/4, 4/4.
    pub const fn quarters(top: u32) -> Option<TimeSignature> {
        TimeSignature::new(top, 4)
    }

    pub const fn top(self) -> u32 {
        self.top
    }

    pub const fn bottom(self) -> u32 {
        self.bottom
    }

    /// Quarter notes in a bar.
    pub fn beats(self) -> f64 {
        f64::from(self.top) * 4.0 / f64::from(self.bottom)
    }

    /// Ticks in a bar at `ppq` ticks a quarter note, at least 1 (whole when `ppq` × 4 divides by `bottom`, as 96
    /// does).
    pub const fn bar_ticks(self, ppq: i64) -> i64 {
        let bar = ppq.saturating_mul(4).saturating_mul(self.top as i64) / self.bottom as i64;
        if bar > 0 { bar } else { 1 }
    }
}

impl From<TimeSignature> for crate::midi::write::TimeSignature {
    /// The same signature, for a MIDI file: every one a project can have fits.
    fn from(t: TimeSignature) -> Self {
        crate::midi::write::TimeSignature { numerator: t.top as u8, denominator: t.bottom as u8 }
    }
}

/// A clip's two fades, in beats, no longer together than the clip: the fade in gives way last. A number that isn't
/// one (or is below 0) is no fade.
pub(crate) fn fades_within(fade_in: f64, fade_out: f64, length: f64) -> (f64, f64) {
    let room = if length > 0.0 { length } else { 0.0 };
    let whole = |fade: f64| if fade > 0.0 { fade } else { 0.0 };
    let fade_in = whole(fade_in).min(room);
    (fade_in, whole(fade_out).min(room - fade_in))
}

/// A project's arrangement. `view` names the layout ("FL Studio playlist", "As it opens in Bitwig") when a project
/// can be shown more than one way.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    pub view: &'static str,
    pub tempo: f64,
    /// The time signature the song starts in.
    pub signature: TimeSignature,
    pub lanes: Vec<PreviewLane>,
    pub markers: Vec<PreviewMarker>,
    /// Things the preview couldn't show, in plain words.
    pub notes: Vec<String>,
}

impl Arrangement {
    /// Where the last clip ends, in beats.
    /// Where the last clip ends, in beats, but never past [`MOST_BEATS`]: a damaged project's clip can end at 1e20.
    pub fn length(&self) -> f64 {
        self.lanes.iter().flat_map(|l| &l.clips).map(PreviewClip::end).fold(0.0, f64::max).min(MOST_BEATS)
    }

    pub fn bars(&self) -> f64 {
        self.length() / self.signature.beats()
    }

    pub fn clip_count(&self) -> usize {
        self.lanes.iter().map(|l| l.clips.len()).sum()
    }

    /// The notes that play: none on muted lanes or in muted clips, no muted notes.
    pub fn note_count(&self) -> usize {
        self.lanes
            .iter()
            .filter(|l| !l.muted)
            .flat_map(|l| &l.clips)
            .filter(|c| !c.muted)
            .map(|c| c.notes.iter().filter(|n| !n.muted).count())
            .sum()
    }

    pub fn seconds(&self) -> f64 {
        if self.tempo > 0.0 { self.length() * 60.0 / self.tempo } else { 0.0 }
    }
}

/// The longest song an arrangement is drawn to, in beats: about 100 days at 120 BPM, past any real song, and far below
/// where a bar number or a song's seconds stop fitting in a u64.
pub const MOST_BEATS: f64 = 1.0e7;

/// The most notes an arrangement holds, as the song readers' limit: a damaged file can loop a clip millions of times.
pub const MOST_NOTES: usize = 4_000_000;
/// Said when an arrangement reaches [`MOST_NOTES`].
pub const TOO_MANY_NOTES: &str =
    "This project has more notes than the preview holds (4 million), so only the first are drawn.";

/// What's left of an arrangement's [`MOST_NOTES`], spent as notes are placed.
#[derive(Debug)]
pub(crate) struct NoteBudget {
    left: usize,
    pub ran_out: bool,
}

impl NoteBudget {
    pub fn new() -> NoteBudget {
        NoteBudget::of(MOST_NOTES)
    }

    /// A budget of its own size, for what is shown beside the song and mustn't use up the song's own.
    pub fn of(left: usize) -> NoteBudget {
        NoteBudget { left, ran_out: false }
    }

    /// Takes one note from the budget: false when there are none left.
    pub fn take(&mut self) -> bool {
        if self.left == 0 {
            self.ran_out = true;
            return false;
        }
        self.left -= 1;
        true
    }
}

/// The project's arrangement views, read from its bytes by its ending: FL Studio has two (its playlist, and the tracks
/// it becomes in Bitwig); Bitwig its arranger and, when the clip launcher holds clips, its scenes one after another
/// (the scenes first when the arranger timeline is empty); Studio One one. None for a file of another kind; a project
/// too damaged to draw gives a view that says so.
pub fn read_bytes(path: &Path, raw: &[u8]) -> Vec<Arrangement> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "flp" => match crate::fl::events::load(raw) {
            Ok(flp) => crate::fl::arrangement::read(&flp),
            Err(_) => vec![unreadable(crate::fl::arrangement::PLAYLIST_VIEW, "This project")],
        },
        "bwproject" => match crate::bitwig::decode(raw) {
            Ok(doc) => crate::bitwig::arrangement::read(&doc),
            Err(_) => vec![unreadable(crate::bitwig::arrangement::VIEW, "This project")],
        },
        "als" => match crate::ableton::LiveSet::open(raw) {
            Ok(set) => set.in_folder(path.parent()).arrangements(),
            Err(_) => vec![unreadable(crate::ableton::arrangement::VIEW, "This set")],
        },
        "song" => match crate::studio_one::Song::open(raw) {
            Ok(mut song) => vec![song.arrangement()],
            Err(_) => vec![unreadable(crate::studio_one::arrangement::VIEW, "This song")],
        },
        _ => Vec::new(),
    }
}

/// [`read_bytes`] for a file on disk, read once.
pub fn read(path: &Path) -> std::io::Result<Vec<Arrangement>> {
    Ok(read_bytes(path, &std::fs::read(path)?))
}

/// An empty view saying the project couldn't be read well enough to draw.
pub(crate) fn unreadable(view: &'static str, what: &str) -> Arrangement {
    Arrangement {
        view,
        tempo: 120.0,
        signature: TimeSignature::FOUR_FOUR,
        lanes: Vec::new(),
        markers: Vec::new(),
        notes: vec![format!("{what} couldn't be read in enough detail to draw.")],
    }
}

#[cfg(test)]
mod signature_tests {
    use super::TimeSignature;

    #[test]
    fn a_time_signature_counts_its_bar_in_quarter_notes() {
        let of = |top, bottom| TimeSignature::new(top, bottom).unwrap();
        assert_eq!(TimeSignature::default(), of(4, 4));
        assert_eq!((of(6, 8).beats(), of(7, 8).beats(), of(2, 2).beats(), of(12, 8).beats()), (3.0, 3.5, 4.0, 6.0));
        // Ticks in a bar are whole at 96 a quarter note, down to thirty-seconds.
        assert_eq!((of(6, 8).bar_ticks(96), of(7, 8).bar_ticks(96), of(5, 32).bar_ticks(96)), (288, 336, 60));
        assert_eq!(of(1, 32).bar_ticks(0), 1);
        for (top, bottom) in [(0, 4), (256, 4), (4, 0), (4, 3), (4, 64), (u32::MAX, u32::MAX)] {
            assert_eq!(TimeSignature::new(top, bottom), None, "{top}/{bottom}");
        }
        let midi: crate::midi::write::TimeSignature = of(12, 8).into();
        assert_eq!((midi.numerator, midi.denominator), (12, 8));
    }

    #[test]
    fn two_fades_are_never_longer_together_than_their_clip() {
        use super::fades_within;
        assert_eq!(fades_within(1.0, 2.0, 4.0), (1.0, 2.0));
        assert_eq!(fades_within(3.0, 2.0, 4.0), (3.0, 1.0));
        assert_eq!(fades_within(9.0, 2.0, 4.0), (4.0, 0.0));
        assert_eq!(fades_within(-1.0, f64::NAN, 4.0), (0.0, 0.0));
        assert_eq!(fades_within(1.0, 1.0, -4.0), (0.0, 0.0));
        assert_eq!(fades_within(1.0, f64::INFINITY, f64::NAN), (0.0, 0.0));
    }
}
