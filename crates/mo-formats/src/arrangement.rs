//! A project laid out the way its DAW's arrangement shows it, for drawing without opening the DAW: the song panel's
//! arrangement preview (spec section 3, Songs, Overview). The same for every DAW; each reader fills it in. Ported from
//! the C# app's `Preview/Arrangement.cs`.
//!
//! Times are in beats from the song's start (a clip's notes and points from the clip's start). The readers never look
//! at the disk: an audio part keeps the path the project saved, and the app finds the file.

/// What a lane holds, which decides how its clips are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Instrument,
    Audio,
    Automation,
    Group,
}

/// A note in a clip, in beats from the clip's start. Velocity keeps MIDI 2.0's 16 bits (rule 22).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewNote {
    pub start: f64,
    pub length: f64,
    pub key: u8,
    pub velocity: u16,
    pub muted: bool,
}

/// A point on an automation curve, in beats from the clip's start, with the value from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewPoint {
    pub beat: f64,
    pub value: f64,
}

/// A stretch of an audio file playing in a clip, from `start` for `length` beats (timed from the clip's start). It
/// starts `from` seconds into the file and moves through it at `seconds_per_beat`; when that's only known once the
/// file's length is, `whole_file_beats` says how many beats the whole file takes instead.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPart {
    /// The file as the project saved it.
    pub path: String,
    pub start: f64,
    pub length: f64,
    pub from: f64,
    pub seconds_per_beat: Option<f64>,
    pub whole_file_beats: Option<f64>,
    /// Plays backwards (FL Studio's reverse switch).
    pub reversed: bool,
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

/// A project's arrangement. `view` names the layout ("FL Studio playlist", "As it opens in Bitwig") when a project
/// can be shown more than one way.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    pub view: &'static str,
    pub tempo: f64,
    pub beats_per_bar: u32,
    pub lanes: Vec<PreviewLane>,
    pub markers: Vec<PreviewMarker>,
    /// Things the preview couldn't show, in plain words.
    pub notes: Vec<String>,
}

impl Arrangement {
    /// Where the last clip ends, in beats.
    pub fn length(&self) -> f64 {
        self.lanes.iter().flat_map(|l| &l.clips).map(PreviewClip::end).fold(0.0, f64::max)
    }

    pub fn bars(&self) -> f64 {
        if self.beats_per_bar > 0 { self.length() / self.beats_per_bar as f64 } else { 0.0 }
    }

    pub fn clip_count(&self) -> usize {
        self.lanes.iter().map(|l| l.clips.len()).sum()
    }

    /// The notes that play: none in muted clips, no muted notes.
    pub fn note_count(&self) -> usize {
        self.lanes
            .iter()
            .flat_map(|l| &l.clips)
            .filter(|c| !c.muted)
            .map(|c| c.notes.iter().filter(|n| !n.muted).count())
            .sum()
    }

    pub fn seconds(&self) -> f64 {
        if self.tempo > 0.0 { self.length() * 60.0 / self.tempo } else { 0.0 }
    }
}
