//! Chord check, live (Solomon, 13 Nov: "play midi into the plugin and I can see what chord it thinks it is"): the
//! plugin reads the notes played into its note input ("Notes") and shows the chord the app's chord reader
//! (`mo_harmony`) names as they're played, the keys held, and the chords of the last few bars. Later steps keep each
//! take and let him say what the chord really is, so his answers measure the chord reader (the plan:
//! "Chord test bench in the plugin"; his takes are only measured on, never learned from: Solomon, 13 Nov).
//!
//! Three parts, so nothing waits on the DAW's audio thread:
//!
//! - [`Catch`], on the audio thread: where the song is each block, and each key pressed or let go, the sustain pedal
//!   and "all notes off", go into a ring ([`rtrb`]: no waiting, no memory asked for), each with its beat: the DAW
//!   song's while it plays, or the plugin's own count at the DAW's tempo while it's stopped. It also says which of
//!   the block's events Thru passes on ([`Catch::passes`]).
//! - A low-priority thread of its own ([`Checker::start`]) takes them into the take under way ([`Take`]) and reads the
//!   last [`READ_BARS`] bars about ten times a second.
//! - The panel shows the newest [`Heard`] (`window.rs`, the Chords tab).

use crate::takes::{Ended, Label};
use crate::threads::{background, lock};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Match, UnknownEvent};
use clack_plugin::prelude::Event;
use mo_formats::velocity;
use mo_harmony::{ChordAnswer, ChordContext, ChordReader, Grid, Note, Part, Role, Span};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ticks a quarter note in a take.
pub const PPQ: i64 = 960;
/// How many of the newest bars are read and shown.
pub const READ_BARS: i64 = 4;
/// The most chords the strip shows: the newest (the panel has room for about this many beside the chord now).
pub const STRIP_MOST: usize = 5;
/// A take ends after this many bars with no key sounding.
pub const GAP_BARS: f64 = 2.0;
/// How often the reading thread looks at what came in.
const READ_EVERY: Duration = Duration::from_millis(100);
/// Room in the ring between the audio thread and the reading thread: a block's place (one a block: under a thousand a
/// second at the smallest blocks) and the keys played, for well over the [`READ_EVERY`] between two looks.
const RING: usize = 16_384;
/// The most notes a take keeps: a take longer than this (hours of playing) starts again.
const MOST_NOTES: usize = 50_000;
/// The tempo counted at when the DAW doesn't say one.
const USUAL_TEMPO: f64 = 120.0;
/// A song's beat this far behind the one before is the DAW going back (a loop, the play head moved): a new take.
const BACK: f64 = 1e-3;

/// What the audio thread tells the reading thread, in the order it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Came {
    /// Where the music is at a block's start.
    Now(Now),
    /// A key pressed (with its MIDI 2.0 16-bit velocity, rule 22) or let go (velocity 0), at a beat.
    Key { key: u8, velocity: u16, on: bool, beat: f64 },
    /// The sustain pedal pressed or let go.
    Pedal { down: bool, beat: f64 },
    /// Every key let go: all notes off, a note-off for any key, a choke.
    AllOff { beat: f64 },
}

/// Where the music is: on the DAW's song while it plays, else on the plugin's own count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    pub beat: f64,
    pub song: bool,
    /// Where a bar of the song starts (in beats) and how many quarter-note beats a bar is, when the DAW says.
    pub bar: Option<(f64, f64)>,
}

/// Where the DAW's song is at a block's start, while it plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SongAt {
    pub beat: f64,
    /// Where a bar starts and its beats, when the DAW says.
    pub bar: Option<(f64, f64)>,
}

/// What the audio thread knows of time for one block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// Where the song is, while it plays; none while it's stopped.
    pub song: Option<SongAt>,
    /// Beats a frame at the DAW's tempo (or [`USUAL_TEMPO`]).
    pub beats_a_frame: f64,
    pub frames: u32,
}

impl Clock {
    /// Beats a frame at `tempo` (none: [`USUAL_TEMPO`]) and `rate` frames a second.
    pub fn beats_a_frame(tempo: Option<f64>, rate: f64) -> f64 {
        let tempo = tempo.filter(|t| t.is_finite() && *t > 0.0).unwrap_or(USUAL_TEMPO);
        if rate > 0.0 { tempo / 60.0 / rate } else { 0.0 }
    }
}

/// What one of the DAW's events on the note input (port 0) does to the keys: CLAP's own notes, MIDI 1.0 or MIDI 2.0.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Does {
    Key {
        channel: u16,
        key: u8,
        velocity: u16,
        on: bool,
    },
    Pedal(bool),
    AllOff,
    /// Something else for an instrument (a controller, a bend, an expression): Thru passes it on, Chord check doesn't
    /// read it.
    Other,
}

/// What an event does, and where in the block; none for an event that isn't the note input's (a knob's, the
/// transport's).
fn does(event: CoreEventSpace) -> Option<(u32, Does)> {
    let port0 = |p: Match<u16>| p.as_specific().is_none_or(|&p| p == 0);
    let key = |m: Match<u16>| m.as_specific().and_then(|&k| u8::try_from(k).ok()).filter(|k| *k < 128);
    let channel = |m: Match<u16>| m.as_specific().copied().unwrap_or(0);
    Some(match event {
        CoreEventSpace::NoteOn(e) if port0(e.port_index()) => {
            let does = match key(e.key()) {
                Some(k) => Does::Key {
                    channel: channel(e.channel()),
                    key: k,
                    velocity: velocity::from_unit(e.velocity()).max(1),
                    on: true,
                },
                None => Does::Other,
            };
            (e.header().time(), does)
        }
        CoreEventSpace::NoteOff(e) if port0(e.port_index()) => {
            let does = match key(e.key()) {
                Some(k) => Does::Key { channel: channel(e.channel()), key: k, velocity: 0, on: false },
                // A note-off for every key (or a channel's): the DAW stopping or a panic.
                None => Does::AllOff,
            };
            (e.header().time(), does)
        }
        CoreEventSpace::NoteChoke(e) if port0(e.port_index()) => {
            let does = match key(e.key()) {
                Some(k) => Does::Key { channel: channel(e.channel()), key: k, velocity: 0, on: false },
                None => Does::AllOff,
            };
            (e.header().time(), does)
        }
        CoreEventSpace::NoteExpression(e) if port0(e.port_index()) => (e.header().time(), Does::Other),
        CoreEventSpace::Midi(e) if e.port_index() == 0 => {
            let [status, data1, data2] = e.data();
            let (channel, data1) = (u16::from(status & 0x0F), data1 & 0x7F);
            let does = match status & 0xF0 {
                0x90 if data2 > 0 => {
                    Does::Key { channel, key: data1, velocity: velocity::from_7(data2).max(1), on: true }
                }
                0x90 | 0x80 => Does::Key { channel, key: data1, velocity: 0, on: false },
                0xB0 if data1 == 64 => Does::Pedal(data2 >= 64),
                // All sound off, all notes off.
                0xB0 if data1 == 120 || data1 == 123 => Does::AllOff,
                _ => Does::Other,
            };
            (e.header().time(), does)
        }
        CoreEventSpace::Midi2(e) if e.port_index() == 0 => {
            // A MIDI 2.0 channel voice message (type 4): its status and first data byte in the first word, its 16-bit
            // velocity in the second's top half, or a controller's 32-bit value in all of the second.
            let [first, second, ..] = e.data();
            let channel = ((first >> 16) & 0x0F) as u16;
            let data1 = ((first >> 8) & 0x7F) as u8;
            let does = match (first >> 28, (first >> 20) & 0x0F) {
                (4, 0x9) => Does::Key { channel, key: data1, velocity: ((second >> 16) as u16).max(1), on: true },
                (4, 0x8) => Does::Key { channel, key: data1, velocity: 0, on: false },
                (4, 0xB) if data1 == 64 => Does::Pedal(second >= 0x8000_0000),
                (4, 0xB) if data1 == 120 || data1 == 123 => Does::AllOff,
                _ => Does::Other,
            };
            (e.header().time(), does)
        }
        CoreEventSpace::MidiSysEx(e) if e.port_index() == 0 => (e.header().time(), Does::Other),
        _ => return None,
    })
}

/// The audio thread's half: made when the track's audio starts ([`Checker::start`]).
pub struct Catch {
    to: rtrb::Producer<Came>,
    /// The plugin's own count of beats, at the start of the next block.
    own: f64,
    /// Thru: for each key, the channels whose note on it was passed on and not yet let go (a bit a channel), so its
    /// note-off is passed on even after Thru is switched off, and no note is left sounding after the plugin.
    passed: [u16; 128],
    checker: Arc<Checker>,
}

impl Catch {
    /// One block of the DAW's: where the music is, and what the note input did, go into the ring. Never waits and
    /// never asks for memory.
    pub fn block(&mut self, input: &clack_plugin::events::io::InputEvents, clock: Clock) {
        let start = clock.song.map_or(self.own, |s| s.beat);
        // A full ring (the reading thread stalled for a long while) drops what doesn't fit: the take reads on.
        let _ = self.to.push(Came::Now(Now {
            beat: start,
            song: clock.song.is_some(),
            bar: clock.song.and_then(|s| s.bar),
        }));
        for event in input {
            let Some((offset, does)) = event.as_core_event().and_then(does) else { continue };
            let beat = start + f64::from(offset) * clock.beats_a_frame;
            let came = match does {
                Does::Key { key, velocity, on, .. } => Came::Key { key, velocity, on, beat },
                Does::Pedal(down) => Came::Pedal { down, beat },
                Does::AllOff => Came::AllOff { beat },
                Does::Other => continue,
            };
            let _ = self.to.push(came);
        }
        self.own += f64::from(clock.frames) * clock.beats_a_frame;
    }

    /// Whether Thru passes `event` on out of the plugin, in order with the rest of the block's. With Thru on, every
    /// event of the note input's (notes, the pedal and other controllers, bends, expressions). With it off, only
    /// what lets go of a note passed on while it was on, and every "all notes off", so nothing is left sounding.
    pub fn passes(&mut self, event: &UnknownEvent) -> bool {
        let Some((_, does)) = event.as_core_event().and_then(does) else { return false };
        let thru = self.checker.thru.load(Ordering::Relaxed);
        match does {
            Does::Key { channel, key, on, .. } => {
                let bit = 1u16 << (channel & 0x0F);
                let Some(passed) = self.passed.get_mut(usize::from(key)) else { return false };
                if on {
                    if thru {
                        *passed |= bit;
                    }
                    thru
                } else {
                    let was = *passed & bit != 0;
                    *passed &= !bit;
                    thru || was
                }
            }
            Does::AllOff => {
                self.passed = [0; 128];
                true
            }
            Does::Pedal(_) | Does::Other => thru,
        }
    }
}

/// One take: the notes from the first one pressed, on a grid of bars from where it started.
#[derive(Clone, Debug)]
pub struct Take {
    /// Counted on the DAW's song (it played) or on the plugin's own count (it was stopped).
    pub song: bool,
    /// The beat tick 0 is: a bar line of the song's, or the first note's beat.
    pub origin: f64,
    /// The bar grid the take is read on.
    grid: Grid,
    /// Each key sounding: the tick it went down, its velocity, and whether it's let go but held on by the pedal.
    sounding: Vec<Option<(i64, u16, bool)>>,
    pedal: bool,
    /// The notes ended, in the order they ended.
    pub notes: Vec<Note>,
    /// The beat the last key sounding ended on.
    last_up: f64,
    /// When it started, by the computer's clock: its name once saved.
    pub started: chrono::DateTime<chrono::Local>,
}

/// The grid for bars of `quarters` quarter notes: on quarter beats when a bar is a whole number of them, else on
/// eighths (7/8 is 7 eighths) or sixteenths, so the reader's bar lines are the song's.
fn grid_for(quarters: f64) -> Grid {
    for (ticks, beats_a_quarter) in [(PPQ, 1.0), (PPQ / 2, 2.0), (PPQ / 4, 4.0)] {
        let beats = quarters * beats_a_quarter;
        if (beats - beats.round()).abs() < 1e-6 && beats.round() >= 1.0 && beats.round() <= 64.0 {
            return Grid { ticks_per_beat: ticks as u32, beats_per_bar: beats.round() as u32, bar_line: 0 };
        }
    }
    Grid { ticks_per_beat: PPQ as u32, beats_per_bar: quarters.round().clamp(1.0, 16.0) as u32, bar_line: 0 }
}

impl Take {
    /// A take starting at `now` (the beat its first key went down, or where the song started or went back to).
    pub fn start(now: &Now) -> Take {
        let (origin, grid) = match now.bar.filter(|(start, len)| *len > 0.0 && len.is_finite() && start.is_finite()) {
            // On the song's bars: the bar the take starts in.
            Some((start, len)) if now.song => (start + ((now.beat - start) / len).floor() * len, grid_for(len)),
            // Played freely: the first note is the first downbeat, in 4/4.
            _ => (now.beat, grid_for(4.0)),
        };
        Take {
            song: now.song,
            origin,
            grid,
            sounding: vec![None; 128],
            pedal: false,
            notes: Vec::new(),
            last_up: now.beat,
            started: chrono::Local::now(),
        }
    }

    /// A take going on from `old`'s at `now` (the song started, stopped or went back): its keys still sounding sound
    /// on from there, and the pedal stays as it was.
    fn carry(old: &Take, now: &Now) -> Take {
        let mut take = Take::start(now);
        let at = take.tick(now.beat);
        take.pedal = old.pedal;
        for (new, was) in take.sounding.iter_mut().zip(&old.sounding) {
            *new = was.map(|(_, velocity, by_pedal)| (at, velocity, by_pedal));
        }
        take
    }

    /// The tick at `beat`.
    pub fn tick(&self, beat: f64) -> i64 {
        ((beat - self.origin) * PPQ as f64).round() as i64
    }

    /// The grid the chord reader reads the take on.
    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// Beats (quarter notes) a bar.
    fn bar_quarters(&self) -> f64 {
        self.grid.bar() as f64 / PPQ as f64
    }

    /// Ends the note sounding on `key` at `tick`.
    fn end(&mut self, key: u8, tick: i64, beat: f64) {
        let Some(slot) = self.sounding.get_mut(usize::from(key)) else { return };
        if let Some((start, velocity, _)) = slot.take() {
            self.notes.push(Note { start, length: (tick - start).max(1), key, velocity });
        }
        if !self.sounds() {
            self.last_up = beat;
        }
    }

    /// What came, at its beat.
    pub fn add(&mut self, came: &Came) {
        match *came {
            Came::Key { key, velocity, on, beat } => {
                let tick = self.tick(beat);
                if on {
                    // Pressed again while it sounds: the note sounding ends here.
                    self.end(key, tick, beat);
                    if let Some(slot) = self.sounding.get_mut(usize::from(key)) {
                        *slot = Some((tick, velocity, false));
                    }
                } else if self.pedal {
                    if let Some(Some((_, _, by_pedal))) = self.sounding.get_mut(usize::from(key)) {
                        *by_pedal = true;
                    }
                } else {
                    self.end(key, tick, beat);
                }
            }
            Came::Pedal { down, beat } => {
                self.pedal = down;
                if !down {
                    let tick = self.tick(beat);
                    let held_on: Vec<u8> = (0u8..128)
                        .filter(|&k| matches!(self.sounding.get(usize::from(k)), Some(Some((_, _, true)))))
                        .collect();
                    for key in held_on {
                        self.end(key, tick, beat);
                    }
                }
            }
            Came::AllOff { beat } => {
                let tick = self.tick(beat);
                self.pedal = false;
                for key in 0u8..128 {
                    self.end(key, tick, beat);
                }
            }
            Came::Now(_) => {}
        }
    }

    /// Whether a key sounds (held, or held on by the pedal).
    pub fn sounds(&self) -> bool {
        self.sounding.iter().any(Option::is_some)
    }

    /// The keys sounding, lowest first.
    pub fn sounding_keys(&self) -> Vec<u8> {
        (0u8..128).filter(|&k| self.sounding.get(usize::from(k)).is_some_and(Option::is_some)).collect()
    }

    /// Whether the take is over at `beat`: nothing sounding for [`GAP_BARS`] bars, or it has [`MOST_NOTES`].
    pub fn over(&self, beat: f64) -> bool {
        self.notes.len() >= MOST_NOTES || (!self.sounds() && beat - self.last_up >= GAP_BARS * self.bar_quarters())
    }

    /// The notes sounding after tick `from` and up to `beat`, cut to start at `from` at the earliest (those still
    /// sounding as sounding until then), in order of start. Only the notes that reach past `from` are copied.
    pub fn notes_from(&self, from: i64, beat: f64) -> Vec<Note> {
        let now = self.tick(beat).max(from + 1);
        let sounding = self.sounding.iter().enumerate().filter_map(|(key, s)| {
            let (start, velocity, _) = (*s)?;
            Some(Note { start, length: (now - start).max(1), key: u8::try_from(key).ok()?, velocity })
        });
        let mut notes: Vec<Note> = self
            .notes
            .iter()
            .copied()
            .chain(sounding)
            .filter(|n| n.end() > from && n.start < now)
            .map(|n| {
                let start = n.start.max(from);
                Note { start, length: n.end().min(now) - start, ..n }
            })
            .collect();
        notes.sort_by_key(|n| (n.start, n.key));
        notes
    }

    /// The chords of the newest [`READ_BARS`] bars up to `beat`, as the panel shows them.
    pub fn read(&self, reader: &mut ChordReader, beat: f64) -> Heard {
        let now = self.tick(beat).max(0);
        let (bar, beat_ticks) = (self.grid.bar(), self.grid.beat());
        let this_bar = now.div_euclid(bar);
        let from = (this_bar - (READ_BARS - 1)).max(0) * bar;
        let notes: Vec<Note> =
            self.notes_from(from, beat).into_iter().map(|n| Note { start: n.start - from, ..n }).collect();
        let sounding: Vec<String> = self.sounding_keys().into_iter().map(mo_app::midi::note_name).collect();
        let mut heard = Heard { sounding, take_bars: this_bar + 1, song: self.song, ..Heard::default() };
        let parts = [Part { role: Role::Harmony, notes: &notes }];
        let Some(reading) =
            (!notes.is_empty()).then(|| reader.read(&parts, self.grid, &ChordContext::new(Span::Window))).flatten()
        else {
            return heard;
        };
        let at = now - from;
        let mut sounding_now = None;
        for (span, name) in reading.spans.iter().zip(reading.names(None)) {
            let Some(name) = name else { continue };
            let start = span.start + from;
            let sounds = span.start <= at && at <= span.end;
            let chord = StripChord {
                name: name.name(),
                at: format!("{}.{}", start.div_euclid(bar) + 1, start.rem_euclid(bar) / beat_ticks + 1),
                sure: sure_of(span.answer),
                now: sounds,
            };
            if sounds {
                sounding_now = Some((chord.name.clone(), chord.sure));
            }
            heard.strip.push(chord);
        }
        let cut = heard.strip.len().saturating_sub(STRIP_MOST);
        heard.strip.drain(..cut);
        // Nothing sounds now (a gap, or the chord is let go): the chord shown is the last one read.
        let shown = sounding_now.or_else(|| heard.strip.last().map(|c| (c.name.clone(), c.sure)));
        if let Some((name, sure)) = shown {
            heard.name = Some(name);
            heard.sure = sure;
        }
        heard
    }
}

/// How sure the reader is of a chord, as the panel shades it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sure {
    /// One chord.
    Yes,
    /// One set of notes with more than one name (C6 or Am7), or a chord one line implies.
    Mostly,
    /// Leaning to this one.
    Unsure,
    #[default]
    No,
}

impl Sure {
    pub fn words(self) -> &'static str {
        match self {
            Sure::Yes => "sure",
            Sure::Mostly => "the same notes have another name too",
            Sure::Unsure => "not sure",
            Sure::No => "no chord",
        }
    }
}

pub(crate) fn sure_of(answer: ChordAnswer) -> Sure {
    match answer {
        ChordAnswer::Chord => Sure::Yes,
        ChordAnswer::SameNotes | ChordAnswer::Implied => Sure::Mostly,
        ChordAnswer::Unclear => Sure::Unsure,
        ChordAnswer::NoChord(_) => Sure::No,
    }
}

/// A chord on the strip: its name, where it starts ("3.1": bar 3, beat 1), how sure, and whether it sounds now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StripChord {
    pub name: String,
    pub at: String,
    pub sure: Sure,
    pub now: bool,
}

/// What the panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heard {
    /// The chord sounding now (or the last one read), when one is read.
    pub name: Option<String>,
    pub sure: Sure,
    /// The keys sounding (held, or held on by the pedal), lowest first ("C3 E4 G4").
    pub sounding: Vec<String>,
    /// The chords of the newest bars, in order: at most [`STRIP_MOST`].
    pub strip: Vec<StripChord>,
    /// How many bars the take has run.
    pub take_bars: i64,
    /// Counted on the DAW's song.
    pub song: bool,
}

impl Heard {
    /// The line under the chord: the keys sounding and how far the take is.
    pub fn line(&self) -> String {
        let keys = if self.sounding.is_empty() { "No keys held".to_owned() } else { self.sounding.join(" ") };
        let counted = if self.song { "on the song's bars" } else { "counted from the first note" };
        format!("{keys}  \u{b7}  bar {} of the take, {counted}", self.take_bars.max(1))
    }
}

/// Chord check's shared part: the Thru switch and the newest [`Heard`] for the panel.
pub struct Checker {
    /// Counts the DAW's starts of the track's audio: an older reading thread ends.
    run: AtomicU64,
    /// Thru: the notes played in go on out of the note output, for an instrument after the plugin.
    pub thru: AtomicBool,
    /// The newest for the panel, and its number (counted up each time): kept, so a panel opened later shows it too.
    heard: Mutex<(u64, Heard)>,
    /// The takes that are over, newest first (at most [`crate::takes::KEPT`]), and a number counted up at each change.
    takes: Mutex<(u64, Vec<Ended>)>,
}

impl Default for Checker {
    fn default() -> Checker {
        Checker {
            run: AtomicU64::new(0),
            thru: AtomicBool::new(true),
            heard: Mutex::new((0, Heard::default())),
            takes: Mutex::new((0, Vec::new())),
        }
    }
}

impl Checker {
    /// The DAW started the track's audio: the audio thread's half, and a new reading thread (the old one ends).
    /// Called on the DAW's main thread, where memory may be asked for.
    pub fn start(self: &Arc<Self>) -> Catch {
        let (to, from) = rtrb::RingBuffer::new(RING);
        let run = self.run.fetch_add(1, Ordering::AcqRel) + 1;
        let me = Arc::clone(self);
        background("mo-plugin-chords", move || me.read_on(run, from));
        Catch { to, own: 0.0, passed: [0; 128], checker: Arc::clone(self) }
    }

    /// The DAW stopped the track's audio: the reading thread ends.
    pub fn stop(&self) {
        self.run.fetch_add(1, Ordering::AcqRel);
    }

    /// A take that's over, at the top of the list; the oldest goes past [`crate::takes::KEPT`] (saved already when it
    /// was labelled).
    pub fn add_take(&self, ended: Ended) {
        let mut takes = lock(&self.takes);
        takes.1.insert(0, ended);
        takes.1.truncate(crate::takes::KEPT);
        takes.0 = takes.0.wrapping_add(1);
    }

    /// The takes, newest first, when they changed since the list numbered `seen`, with its number.
    pub fn takes(&self, seen: u64) -> Option<(u64, Vec<Ended>)> {
        let takes = lock(&self.takes);
        (takes.0 != seen).then(|| takes.clone())
    }

    /// Labels chord `index` of the take `id`: what to save then, when it's still kept.
    pub fn label(&self, id: &str, index: usize, label: Label) -> Option<mo_app::chord_check::TakeFile> {
        let mut takes = lock(&self.takes);
        let ended = takes.1.iter_mut().find(|t| t.id == id)?;
        ended.chords.get_mut(index)?.label = Some(label);
        let file = ended.file();
        takes.0 = takes.0.wrapping_add(1);
        Some(file)
    }

    /// What became of saving the take `id`, for its line.
    pub fn saved(&self, id: &str, words: String) {
        let mut takes = lock(&self.takes);
        if let Some(ended) = takes.1.iter_mut().find(|t| t.id == id) {
            ended.saved = words;
            takes.0 = takes.0.wrapping_add(1);
        }
    }

    /// The newest [`Heard`] when it's newer than the one numbered `seen`, with its number.
    pub fn newest(&self, seen: u64) -> Option<(u64, Heard)> {
        let heard = lock(&self.heard);
        (heard.0 != seen).then(|| heard.clone())
    }

    /// The reading thread: takes in what came every [`READ_EVERY`] and reads the take while one is under way.
    fn read_on(&self, run: u64, mut from: rtrb::Consumer<Came>) {
        let mut reader = ChordReader::new();
        let mut steps = Steps::default();
        let mut came = Vec::new();
        while self.run.load(Ordering::Acquire) == run {
            std::thread::sleep(READ_EVERY);
            came.clear();
            while let Ok(c) = from.pop() {
                came.push(c);
            }
            if let Some(heard) = steps.step(&came, &mut reader) {
                let mut newest = lock(&self.heard);
                *newest = (newest.0.wrapping_add(1), heard);
            }
            for (take, beat) in std::mem::take(&mut steps.finished) {
                if let Some(ended) = Ended::of(&take, beat, &mut reader) {
                    self.add_take(ended);
                }
            }
        }
    }
}

/// The reading thread's state from step to step: where the music is, the take under way, and whether it changed
/// since it was read.
#[derive(Default)]
pub struct Steps {
    now: Option<Now>,
    pub take: Option<Take>,
    changed: bool,
    /// The bar the take was last read in: a chord sounding is read again each bar, so the strip moves on.
    read_in: i64,
    /// The takes that ended, with the beat each ended on, for [`crate::takes::Ended`].
    pub finished: Vec<(Take, f64)>,
}

impl Steps {
    /// One step: what `came` does, in order (a take starts at the first key pressed after one ended; the song
    /// starting, stopping or going back starts a new one, the keys sounding carried on), and the take read again
    /// when it changed. What the panel should show, when that changed.
    pub fn step(&mut self, came: &[Came], reader: &mut ChordReader) -> Option<Heard> {
        for c in came {
            match c {
                Came::Now(now) => {
                    let went_back = self.now.is_some_and(|was| was.song && now.song && now.beat < was.beat - BACK);
                    let other_clock = self.now.is_some_and(|was| was.song != now.song);
                    if (went_back || other_clock)
                        && let Some(old) = self.take.take()
                    {
                        // Carried on only while a key sounds: else the next key pressed starts the next take.
                        if old.sounds() {
                            self.take = Some(Take::carry(&old, now));
                            self.changed = true;
                        }
                        let ended_at = self.now.map_or(now.beat, |was| was.beat);
                        self.finished.push((old, ended_at));
                    }
                    self.now = Some(*now);
                }
                Came::Key { on: true, beat, .. } if self.take.is_none() => {
                    let now =
                        self.now.map_or(Now { beat: *beat, song: false, bar: None }, |n| Now { beat: *beat, ..n });
                    let mut take = Take::start(&now);
                    take.add(c);
                    self.take = Some(take);
                    self.changed = true;
                }
                _ => {
                    if let Some(take) = &mut self.take {
                        take.add(c);
                        self.changed = true;
                    }
                }
            }
        }
        let beat = self.now?.beat;
        let take = self.take.as_ref()?;
        if take.over(beat) {
            // Ended: the last reading stays on the panel until the next take starts.
            if let Some(old) = self.take.take() {
                self.finished.push((old, beat));
            }
            return None;
        }
        let bar = take.tick(beat).max(0).div_euclid(take.grid().bar());
        if !self.changed && !(take.sounds() && bar != self.read_in) {
            return None;
        }
        self.changed = false;
        self.read_in = bar;
        Some(take.read(reader, beat))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn free(beat: f64) -> Came {
        Came::Now(Now { beat, song: false, bar: None })
    }

    fn press(key: u8, beat: f64) -> Came {
        Came::Key { key, velocity: velocity::from_7(100), on: true, beat }
    }

    fn lift(key: u8, beat: f64) -> Came {
        Came::Key { key, velocity: 0, on: false, beat }
    }

    /// A chord held for `beats` from `at`.
    fn chord(keys: &[u8], at: f64, beats: f64) -> Vec<Came> {
        let mut out: Vec<Came> = keys.iter().map(|&k| press(k, at)).collect();
        out.extend(keys.iter().map(|&k| lift(k, at + beats - 0.05)));
        out
    }

    #[test]
    fn a_take_counts_from_its_first_note_when_the_song_is_stopped() {
        let take = Take::start(&Now { beat: 7.25, song: false, bar: None });
        assert_eq!(take.tick(7.25), 0);
        assert_eq!(take.tick(8.25), PPQ);
        assert_eq!(take.grid().beats_per_bar, 4);
    }

    #[test]
    fn a_take_counts_on_the_songs_bars_while_it_plays() {
        // 3/4 bars from beat 1; the take starting on beat 8.5 is in the bar from beat 7.
        let take = Take::start(&Now { beat: 8.5, song: true, bar: Some((1.0, 3.0)) });
        assert_eq!(take.origin, 7.0);
        assert_eq!(take.tick(8.5), (1.5 * PPQ as f64) as i64);
        assert_eq!(take.grid().beats_per_bar, 3);
    }

    #[test]
    fn odd_bars_are_read_on_eighths() {
        // 7/8: three and a half quarters a bar, seven eighths.
        let grid = grid_for(3.5);
        assert_eq!((grid.ticks_per_beat, grid.beats_per_bar), (PPQ as u32 / 2, 7));
        assert_eq!(grid.bar(), (3.5 * PPQ as f64) as i64);
        assert_eq!(grid_for(2.5).beats_per_bar, 5);
        assert_eq!(grid_for(4.0).beats_per_bar, 4);
    }

    #[test]
    fn notes_sounding_count_until_now_and_a_key_pressed_twice_ends_the_first() {
        let mut take = Take::start(&Now { beat: 0.0, song: false, bar: None });
        for c in [press(60, 0.0), press(64, 0.0), lift(64, 1.0), press(60, 2.0)] {
            take.add(&c);
        }
        let notes = take.notes_from(0, 3.0);
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].key, notes[0].length), (60, 2 * PPQ));
        assert_eq!((notes[1].key, notes[1].length), (64, PPQ));
        assert_eq!((notes[2].start, notes[2].length), (2 * PPQ, PPQ), "sounding: until now");
        assert_eq!(take.sounding_keys(), [60]);
        // From beat 2.5: the first two are left out, the third cut.
        let late = take.notes_from(PPQ * 5 / 2, 3.0);
        assert_eq!(late.len(), 1);
        assert_eq!((late[0].start, late[0].length), (PPQ * 5 / 2, PPQ / 2));
    }

    #[test]
    fn the_pedal_holds_keys_on_and_all_notes_off_lets_everything_go() {
        let mut take = Take::start(&Now { beat: 0.0, song: false, bar: None });
        for c in [Came::Pedal { down: true, beat: 0.0 }, press(60, 0.0), lift(60, 0.5)] {
            take.add(&c);
        }
        assert_eq!(take.sounding_keys(), [60], "held on by the pedal");
        take.add(&Came::Pedal { down: false, beat: 2.0 });
        assert!(!take.sounds());
        assert_eq!(take.notes[0].length, 2 * PPQ);
        take.add(&press(62, 3.0));
        take.add(&press(65, 3.0));
        take.add(&Came::AllOff { beat: 4.0 });
        assert!(!take.sounds());
        assert_eq!(take.notes.len(), 3);
    }

    #[test]
    fn a_take_ends_after_two_bars_with_nothing_sounding() {
        let mut take = Take::start(&Now { beat: 0.0, song: false, bar: None });
        take.add(&press(60, 0.0));
        assert!(!take.over(100.0), "still held");
        take.add(&lift(60, 4.0));
        assert!(!take.over(11.9));
        assert!(take.over(12.0));
    }

    #[test]
    fn it_names_the_chords_played() {
        // Am7, Dm7, G7, Cmaj7, a bar each, played freely.
        let mut came = vec![free(0.0)];
        for (bar, keys) in [[45, 60, 64, 67], [50, 60, 65, 69], [43, 59, 62, 65], [48, 59, 64, 67]].iter().enumerate() {
            came.extend(chord(keys, bar as f64 * 4.0, 4.0));
        }
        came.push(free(15.99));
        let mut steps = Steps::default();
        let mut reader = ChordReader::new();
        let heard = steps.step(&came, &mut reader).unwrap();
        let names: Vec<&str> = heard.strip.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Am7", "Dm7", "G7", "Cmaj7"], "{heard:?}");
        assert_eq!(heard.strip[1].at, "2.1");
        assert_eq!(heard.name.as_deref(), Some("Cmaj7"));
        assert!(heard.strip.last().unwrap().now);
        assert!(!heard.song);
        // Nothing changed: nothing new to show.
        assert!(steps.step(&[free(15.99)], &mut reader).is_none());
        // Two bars later with nothing held, the take is over.
        assert!(steps.step(&[free(24.0)], &mut reader).is_none());
        assert!(steps.take.is_none());
        // It's handed on to be labelled, read whole.
        let (take, ended_at) = steps.finished.pop().unwrap();
        assert_eq!(ended_at, 24.0);
        let ended = crate::takes::Ended::of(&take, ended_at, &mut reader).unwrap();
        assert_eq!(ended.chords.len(), 4);
    }

    #[test]
    fn a_chord_held_is_shown_while_it_sounds() {
        let mut steps = Steps::default();
        let mut reader = ChordReader::new();
        let mut came = vec![free(0.0)];
        came.extend([48, 52, 55, 59].iter().map(|&k| press(k, 0.0)));
        came.push(free(3.0));
        let heard = steps.step(&came, &mut reader).unwrap();
        assert_eq!(heard.name.as_deref(), Some("Cmaj7"), "{heard:?}");
        assert_eq!(heard.sounding, ["C3", "E3", "G3", "B3"]);
        assert!(heard.line().starts_with("C3 E3 G3 B3"));
        // Held into the next bar: read again there.
        assert!(steps.step(&[free(5.0)], &mut reader).is_some());
    }

    #[test]
    fn the_song_starting_or_going_back_carries_the_keys_held_into_a_new_take() {
        let mut steps = Steps::default();
        let mut reader = ChordReader::new();
        steps.step(&[free(0.0), press(60, 0.0)], &mut reader);
        assert!(steps.take.as_ref().is_some_and(|t| !t.song));
        // The song starts at beat 33 in bars of 4: the key still held is in the new take, from there.
        let song = |beat| Came::Now(Now { beat, song: true, bar: Some((0.0, 4.0)) });
        steps.step(&[song(33.0)], &mut reader);
        let take = steps.take.as_ref().unwrap();
        assert!(take.song);
        assert_eq!(take.origin, 32.0);
        assert_eq!(take.sounding_keys(), [60]);
        // Its note-off comes on the song: the note ends in the new take.
        steps.step(&[song(34.0), lift(60, 34.0)], &mut reader);
        assert_eq!(steps.take.as_ref().unwrap().notes.len(), 1);
        // The loop goes back with nothing held: the take ends; the next key starts the next one.
        steps.step(&[song(32.0)], &mut reader);
        assert!(steps.take.is_none());
        // The take before the song started, and the one on the song, are both over now.
        assert_eq!(steps.finished.len(), 2);
        steps.step(&[press(64, 32.5), song(33.0)], &mut reader);
        assert_eq!(steps.take.as_ref().unwrap().origin, 32.0);
    }

    #[test]
    fn thru_never_leaves_a_note_sounding() {
        use clack_plugin::events::Pckn;
        use clack_plugin::events::event_types::{MidiEvent, NoteOffEvent, NoteOnEvent};
        let checker = Arc::new(Checker::default());
        let mut catch = checker.start();
        let at = |key: u16| Pckn::new(0u16, 0u16, key, Match::<u32>::All);
        assert!(catch.passes(NoteOnEvent::new(0, at(60), 0.8).as_ref()));
        assert!(catch.passes(MidiEvent::new(0, 0, [0xB0, 64, 127]).as_ref()), "the pedal goes on too");
        checker.thru.store(false, Ordering::Release);
        assert!(!catch.passes(NoteOnEvent::new(0, at(62), 0.8).as_ref()));
        assert!(catch.passes(NoteOffEvent::new(0, at(60), 0.0).as_ref()), "passed on: its note-off goes too");
        assert!(!catch.passes(NoteOffEvent::new(0, at(60), 0.0).as_ref()), "once");
        assert!(!catch.passes(NoteOffEvent::new(0, at(62), 0.0).as_ref()), "never passed on");
        assert!(catch.passes(MidiEvent::new(0, 0, [0xB0, 123, 0]).as_ref()), "all notes off always goes");
        assert!(!catch.passes(MidiEvent::new(0, 0, [0xE0, 0, 64]).as_ref()), "a bend with Thru off doesn't");
        checker.stop();
    }

    #[test]
    fn the_newest_is_kept_for_a_panel_opened_later() {
        let checker = Checker::default();
        assert!(checker.newest(0).is_none());
        *lock(&checker.heard) = (1, Heard { name: Some("C".into()), ..Heard::default() });
        assert_eq!(checker.newest(0).unwrap().1.name.as_deref(), Some("C"));
        assert!(checker.newest(1).is_none());
        assert!(checker.newest(0).is_some(), "still there for a new panel");
    }
}
