//! Chord check's takes, once each is over: read whole by the chord reader, labelled by Solomon chord by chord (right,
//! one of the reader's other choices, his own name, not a chord, or skipped), and saved in the data folder's
//! `Chord check` folder as the theory bench reads them ([`mo_app::chord_check`]). A take is saved once he's
//! labelled something in it, and again at each label after; one he never labels is kept only while the plugin is
//! loaded (the newest [`KEPT`]).

use crate::chordcheck::{Sure, Take, sure_of};
use mo_app::chord_check::{self as files, Check, CheckSpan, TakeFile, TakePart};
use mo_formats::velocity;
use mo_harmony::{ChordContext, ChordReader, Grid, Note, Part, PitchClass, Role, Span, Spelled};

/// How many takes are kept to label, newest first.
pub const KEPT: usize = 30;
/// The reader's other choices offered for a chord, besides its first.
pub const CHOICES: usize = 3;
/// What a take's chords are labelled under in the bench.
const LABELLED_BY: &str = "Solomon, played and labelled in the test plugin (Chord check)";

/// What Solomon said a chord is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Label {
    /// The reader's name is right.
    Right,
    /// One of the reader's other choices ([`ToLabel::choices`]).
    Choice(usize),
    /// His own name, as read ([`typed`]).
    Typed(Spelled),
    /// Not a chord: passing notes, a run.
    NotAChord,
    /// He isn't sure either: left out of the bench.
    Skip,
}

impl Label {
    /// How it's kept on disk ([`CheckSpan::how`]).
    fn how(self) -> &'static str {
        match self {
            Label::Right => "right",
            Label::Choice(_) => "choice",
            Label::Typed(_) => "typed",
            Label::NotAChord => "not a chord",
            Label::Skip => "skipped",
        }
    }
}

/// A chord the reader read in a take, to be labelled.
#[derive(Clone, Debug, PartialEq)]
pub struct ToLabel {
    pub start: i64,
    pub end: i64,
    /// What the reader shows there (none: no chord).
    pub read: Option<Spelled>,
    /// Its other choices, each named differently from what it shows.
    pub choices: Vec<Spelled>,
    pub sure: Sure,
    /// The lowest key sounding in it.
    pub lowest: Option<u8>,
    pub label: Option<Label>,
}

impl ToLabel {
    /// The chord he said it is: `Some(None)` for not a chord, none when skipped or not labelled yet.
    pub fn said(&self) -> Option<Option<Spelled>> {
        match self.label? {
            Label::Right => Some(self.read),
            Label::Choice(n) => self.choices.get(n).copied().map(Some),
            Label::Typed(chord) => Some(Some(chord)),
            Label::NotAChord => Some(None),
            Label::Skip => None,
        }
    }

    /// What was said, in words for the panel ("" not yet).
    pub fn said_words(&self) -> String {
        match (self.label, self.said()) {
            (None, _) => String::new(),
            (Some(Label::Skip), _) => "Skipped".to_owned(),
            (Some(Label::Right), _) => "Right".to_owned(),
            (_, Some(Some(chord))) => chord.name(),
            (_, _) => "Not a chord".to_owned(),
        }
    }
}

/// A chord name as typed, for a chord whose lowest key is `lowest`: "Cmaj9", "G7#9", "D/C". With no bass typed, its
/// bass is the lowest key played when that's one of its notes (Cmaj7 typed over an E in the bass is Cmaj7 over E, as
/// played), else its root. None when it can't be read.
pub fn typed(text: &str, lowest: Option<u8>) -> Option<Spelled> {
    let spelled = Spelled::parse(text)?;
    let chord = spelled.chord();
    let slash = text
        .trim()
        .rfind('/')
        .is_some_and(|i| text.trim()[i + 1..].chars().next().is_some_and(|c| !c.is_ascii_digit()));
    match lowest.map(PitchClass::of_key) {
        Some(bass) if !slash && chord.notes().contains(bass) => {
            Some(Spelled::new(mo_harmony::Chord::new(chord.root(), chord.notes(), bass), spelled.root()))
        }
        _ => Some(spelled),
    }
}

/// A take that's over.
#[derive(Clone, Debug, PartialEq)]
pub struct Ended {
    /// When it started, to the millisecond, as its file is named: "2026-11-13 07-12-03-250".
    pub id: String,
    /// The day, for the bench's group, and the time for the panel ("07:12").
    pub day: String,
    pub time: String,
    pub song: bool,
    pub grid: Grid,
    pub length: i64,
    pub notes: Vec<Note>,
    pub chords: Vec<ToLabel>,
    /// What became of its saving ("" not saved yet).
    pub saved: String,
    /// Counted up at each label, and the count last saved: a take whose labels aren't all saved yet is never let go.
    pub revision: u64,
    pub saved_revision: u64,
}

impl Ended {
    /// `take`, over at `beat`, read whole. None for a take with no notes.
    pub fn of(take: &Take, beat: f64, reader: &mut ChordReader) -> Option<Ended> {
        let notes = take.notes_from(0, beat);
        let grid = take.grid();
        let last = notes.iter().map(|n| n.end()).max()?;
        let bar = grid.bar();
        let length = last.div_euclid(bar) * bar + if last.rem_euclid(bar) == 0 { 0 } else { bar };
        let cx = ChordContext { span: Span::Song, pins: &[], readings: mo_harmony::chordread::MOST_READINGS };
        let parts = [Part { role: Role::Harmony, notes: &notes }];
        let mut chords = Vec::new();
        if let Some(reading) = reader.read(&parts, grid, &cx) {
            let chart = reading.names(None);
            for (i, (span, shown)) in reading.spans.iter().zip(&chart).enumerate() {
                let choices: Vec<Spelled> = span
                    .choices(CHOICES + 1)
                    .into_iter()
                    .map(|c| reading.spell(&chart, i, c.chord, None))
                    .filter(|c| shown.is_none_or(|s| s.name() != c.name()))
                    .take(CHOICES)
                    .collect();
                let lowest = lowest_in(&notes, span.start, span.end);
                // A stretch with nothing played and nothing to offer isn't worth a label.
                if shown.is_none() && choices.is_empty() {
                    continue;
                }
                chords.push(ToLabel {
                    start: span.start,
                    end: span.end,
                    read: *shown,
                    choices,
                    sure: sure_of(span.answer),
                    lowest,
                    label: None,
                });
            }
        }
        let started = take.started;
        Some(Ended {
            id: started.format("%Y-%m-%d %H-%M-%S-%3f").to_string(),
            day: started.format("%Y-%m-%d").to_string(),
            time: started.format("%H:%M").to_string(),
            song: take.song,
            grid,
            length,
            notes,
            chords,
            saved: String::new(),
            revision: 0,
            saved_revision: 0,
        })
    }

    /// Whether it has labels not saved yet.
    pub fn unsaved(&self) -> bool {
        self.saved_revision != self.revision
    }

    /// How many of its chords are labelled.
    pub fn labelled(&self) -> usize {
        self.chords.iter().filter(|c| c.label.is_some()).count()
    }

    /// The take's line in the panel's list: "07:12  ·  6 bars", "3 of 8 labelled".
    pub fn title(&self) -> String {
        let bars = self.length / self.grid.bar().max(1);
        format!("{}  \u{b7}  {bars} bar{}", self.time, if bars == 1 { "" } else { "s" })
    }

    pub fn details(&self) -> String {
        let n = self.chords.len();
        let said = format!("{} of {n} chord{} labelled", self.labelled(), if n == 1 { "" } else { "s" });
        if self.saved.is_empty() { said } else { format!("{said}  \u{b7}  {}", self.saved) }
    }

    /// As saved ([`mo_app::chord_check::TakeFile`]): the bench's piece, with each chord labelled (a skipped one
    /// left out), and what Chord check keeps besides.
    pub fn file(&self) -> TakeFile {
        let chords = self
            .chords
            .iter()
            .filter_map(|c| {
                let label = match c.said()? {
                    Some(chord) => chord.harte(),
                    None => "N".to_owned(),
                };
                Some((c.start, c.end, label))
            })
            .collect();
        let spans = self
            .chords
            .iter()
            .map(|c| CheckSpan {
                start: c.start,
                end: c.end,
                read: c.read.map(|r| r.name()).unwrap_or_default(),
                read_harte: c.read.map(|r| r.harte()).unwrap_or_default(),
                how: c.label.map(|l| l.how().to_owned()).unwrap_or_default(),
                said: c.said().flatten().map(|s| s.name()).unwrap_or_default(),
            })
            .collect();
        TakeFile {
            source: files::SOURCE.to_owned(),
            id: self.id.clone(),
            ppq: self.grid.ticks_per_beat,
            length: self.length,
            beats_per_bar: self.grid.beats_per_bar,
            first_downbeat: 0,
            aligned: true,
            parts: vec![TakePart {
                role: "chords".to_owned(),
                notes: self.notes.iter().map(|n| (n.start, n.length, n.key, velocity::to_7(n.velocity))).collect(),
            }],
            chords,
            chord_labels: LABELLED_BY.to_owned(),
            licence: files::OWN.to_owned(),
            group: format!("{}/{}", files::SOURCE, self.day),
            whole: "song".to_owned(),
            check: Check {
                reader_version: mo_harmony::CHORD_READER_VERSION,
                on_song: self.song,
                velocities: self.notes.iter().map(|n| n.velocity).collect(),
                spans,
            },
        }
    }
}

/// The lowest key of the notes in `start..end` that start in it or sound for at least half of it: a note held over
/// from the chord before, or one just touched on as it ends, isn't its bass.
fn lowest_in(notes: &[Note], start: i64, end: i64) -> Option<u8> {
    let half = (end - start).max(1) / 2;
    notes
        .iter()
        .filter(|n| {
            let starts_in = n.start >= start && n.start < end;
            starts_in || n.end().min(end) - n.start.max(start) >= half
        })
        .map(|n| n.key)
        .min()
}

/// Where in a bar of `grid` a tick is: "3.1" is bar 3, beat 1.
pub fn place(grid: Grid, tick: i64) -> String {
    let bar = grid.bar().max(1);
    format!("{}.{}", tick.div_euclid(bar) + 1, tick.rem_euclid(bar) / grid.beat() + 1)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::chordcheck::{Came, Now, PPQ};

    fn played(chords: &[&[u8]]) -> Take {
        let mut take = Take::start(&Now { beat: 0.0, song: false, bar: None });
        for (bar, keys) in chords.iter().enumerate() {
            let at = bar as f64 * 4.0;
            for &k in *keys {
                take.add(&Came::Key { key: k, velocity: 40_000, on: true, beat: at });
            }
            for &k in *keys {
                take.add(&Came::Key { key: k, velocity: 0, on: false, beat: at + 3.95 });
            }
        }
        take
    }

    #[test]
    fn a_take_is_read_whole_and_labelled_into_the_benchs_piece() {
        let take = played(&[&[45, 60, 64, 67], &[50, 60, 65, 69], &[43, 59, 62, 65], &[48, 59, 64, 67]]);
        let mut ended = Ended::of(&take, 20.0, &mut ChordReader::new()).unwrap();
        let read: Vec<String> = ended.chords.iter().map(|c| c.read.map(|r| r.name()).unwrap_or_default()).collect();
        assert_eq!(read, ["Am7", "Dm7", "G7", "Cmaj7"], "{ended:?}");
        assert_eq!(ended.length, 4 * 4 * PPQ);
        assert_eq!(ended.title().split("  ").nth(2), Some("4 bars"));
        ended.chords[0].label = Some(Label::Right);
        ended.chords[1].label = Some(Label::Typed(typed("Dm9", ended.chords[1].lowest).unwrap()));
        ended.chords[2].label = Some(Label::Skip);
        ended.chords[3].label = Some(Label::NotAChord);
        assert_eq!(ended.labelled(), 4);
        let file = ended.file();
        assert_eq!(file.chords.len(), 3, "skipped is left out");
        assert_eq!(file.chords[0].2, Spelled::parse("Am7").unwrap().harte());
        assert_eq!(file.chords[2].2, "N");
        assert_eq!(file.licence, files::OWN);
        assert_eq!(file.parts[0].notes.len(), 16);
        assert_eq!(file.check.velocities.len(), 16);
        assert_eq!(file.check.spans[1].how, "typed");
        assert_eq!(file.check.spans[1].said, "Dm9");
        assert_eq!(file.check.spans[2].read, "G7");
        assert_eq!(file.check.reader_version, mo_harmony::CHORD_READER_VERSION);
        assert_eq!(ended.chords[1].said_words(), "Dm9");
        assert_eq!(ended.chords[2].said_words(), "Skipped");
    }

    #[test]
    fn a_chord_typed_takes_the_bass_played() {
        // Cmaj7 typed over an E in the bass is Cmaj7 over E; over a D (not one of its notes), its root.
        let over_e = typed("Cmaj7", Some(40)).unwrap();
        assert_eq!(over_e.chord().bass(), PitchClass::of_key(40));
        assert_eq!(typed("Cmaj7", Some(38)).unwrap().chord().bass(), PitchClass::of_key(36));
        // A bass typed is kept.
        assert_eq!(typed("C/G", Some(40)).unwrap().chord().bass(), PitchClass::of_key(43));
        assert_eq!(typed("C6/9", Some(36)).unwrap().chord().bass(), PitchClass::of_key(36));
        assert!(typed("not a chord", None).is_none());
    }

    #[test]
    fn the_lowest_key_is_one_that_belongs_to_the_chord() {
        let note = |start, length, key| Note { start, length, key, velocity: 40_000 };
        // A bass held 10 ticks into the bar from the bar before, and the chord's own from its start.
        let notes = [note(0, 970, 36), note(960, 960, 43), note(960, 960, 60)];
        assert_eq!(lowest_in(&notes, 960, 1920), Some(43));
        // Held over for most of it: it's its bass too.
        assert_eq!(lowest_in(&[note(0, 1800, 36), note(1200, 720, 60)], 960, 1920), Some(36));
    }

    #[test]
    fn places_count_bars_and_beats_from_one() {
        let grid = Grid::four_four(PPQ as u32);
        assert_eq!(place(grid, 0), "1.1");
        assert_eq!(place(grid, 5 * PPQ), "2.2");
    }
}
