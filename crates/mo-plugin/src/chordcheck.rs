//! Chord check, live (Solomon, 13 Nov: "play midi into the plugin and I can see what chord it thinks it is"): the
//! plugin reads the notes played into its note input ("Notes") and shows the chord the app's chord reader
//! (`mo_harmony`) names as they're played, the notes held, and the chords of the last few bars. Later steps keep each
//! take and let him say what the chord really is, so his answers measure the chord reader (the plan:
//! "Chord test bench in the plugin").
//!
//! Three parts, so nothing waits on the DAW's audio thread:
//!
//! - [`Catch`], on the audio thread: each note pressed or let go goes into a ring ([`rtrb`]: no waiting, no memory
//!   asked for) with the beat it came on, from the DAW's song while it plays or from the plugin's own count at the
//!   DAW's tempo while it's stopped. With Thru on, the notes go on out of the note output for an instrument after it.
//! - A low-priority thread of its own ([`Checker::start`]) takes them into the take under way ([`Take`]) and reads the
//!   last [`READ_BARS`] bars about ten times a second.
//! - The panel shows the newest [`Heard`] (`window.rs`, the Chords tab).

use crate::threads::{background, lock};
use clack_plugin::events::io::InputEvents;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::Event;
use mo_harmony::{ChordAnswer, ChordContext, ChordReader, Grid, Note, Part, PitchClass, Role, Span};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Ticks a beat in a take: a quarter note.
pub const PPQ: i64 = 960;
/// How many of the newest bars are read and shown.
pub const READ_BARS: i64 = 4;
/// The most chords the strip shows: the newest (the panel has room for about this many side by side).
pub const STRIP_MOST: usize = 8;
/// A take ends after this many bars with no key held.
pub const GAP_BARS: f64 = 2.0;
/// How often the reading thread looks at what came in.
const READ_EVERY: Duration = Duration::from_millis(100);
/// Room in the ring between the audio thread and the reading thread: far more than ten handfuls of chords a second.
const RING: usize = 4096;
/// The most notes a take keeps: a take longer than this (hours of playing) starts again.
const MOST_NOTES: usize = 50_000;
/// The most notes Thru passes on in one block (room made when the track's audio starts).
const THRU_MOST: usize = 512;
/// The tempo counted at when the DAW doesn't say one.
const USUAL_TEMPO: f64 = 120.0;

/// A key pressed or let go, as the audio thread caught it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Played {
    pub key: u8,
    /// MIDI 2.0's 16-bit velocity (rule 22); a key let go has 0.
    pub velocity: u16,
    pub on: bool,
    /// The beat it came on: the DAW song's while it plays (`song`), else the plugin's own count.
    pub beat: f64,
    pub song: bool,
    /// Where a bar of the DAW's song starts (in beats) and how many beats a bar is, when the DAW says.
    pub bar: Option<(f64, f64)>,
}

/// A note going on out of the plugin with Thru on: where in the block, its key and channel, how hard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thru {
    pub offset: u32,
    pub channel: u16,
    pub key: u8,
    pub velocity: f64,
    pub on: bool,
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

/// The audio thread's half: made when the track's audio starts ([`Checker::start`]).
pub struct Catch {
    to: rtrb::Producer<Played>,
    /// The plugin's own count of beats, at the start of the next block.
    own: f64,
    thru: Vec<Thru>,
    checker: Arc<Checker>,
}

/// MIDI 1.0's 7-bit velocity scaled up to 16 bits as MIDI 2.0 does (its min-centre-max rule): 0 stays 0, 64 is the
/// middle, 127 is 65535.
pub fn up7(v: u8) -> u16 {
    let v = u32::from(v.min(127));
    let scaled = v << 9;
    if v <= 64 {
        return scaled as u16;
    }
    let repeat = v & 0x3F;
    (scaled | (repeat << 3) | (repeat >> 3)) as u16
}

/// A CLAP velocity (0 to 1) as 16 bits.
fn up_clap(v: f64) -> u16 {
    if v.is_finite() { (v.clamp(0.0, 1.0) * 65535.0).round() as u16 } else { 0 }
}

/// A note pressed or let go in one of the DAW's events on the note input (port 0): CLAP's own notes, MIDI 1.0 or
/// MIDI 2.0. Its place in the block, channel, key, 16-bit velocity, whether it's pressed, and its CLAP velocity for
/// Thru.
fn note_of(event: CoreEventSpace) -> Option<(u32, u16, u8, u16, bool, f64)> {
    let port0 = |p: clack_plugin::events::Match<u16>| p.as_specific().is_none_or(|&p| p == 0);
    let specific = |m: clack_plugin::events::Match<u16>| m.as_specific().copied();
    match event {
        CoreEventSpace::NoteOn(e) if port0(e.port_index()) => {
            let key = u8::try_from(specific(e.key())?).ok().filter(|k| *k < 128)?;
            let v = e.velocity();
            Some((e.header().time(), specific(e.channel()).unwrap_or(0), key, up_clap(v).max(1), true, v))
        }
        CoreEventSpace::NoteOff(e) if port0(e.port_index()) => {
            let key = u8::try_from(specific(e.key())?).ok().filter(|k| *k < 128)?;
            Some((e.header().time(), specific(e.channel()).unwrap_or(0), key, 0, false, e.velocity()))
        }
        CoreEventSpace::Midi(e) if e.port_index() == 0 => {
            let [status, key, velocity] = e.data();
            let channel = u16::from(status & 0x0F);
            let key = key & 0x7F;
            match status & 0xF0 {
                0x90 if velocity > 0 => {
                    Some((e.header().time(), channel, key, up7(velocity), true, f64::from(velocity) / 127.0))
                }
                0x90 | 0x80 => Some((e.header().time(), channel, key, 0, false, f64::from(velocity & 0x7F) / 127.0)),
                _ => None,
            }
        }
        CoreEventSpace::Midi2(e) if e.port_index() == 0 => {
            // A MIDI 2.0 channel voice message (type 4): its status in the first word, its 16-bit velocity in the
            // second's top half.
            let [first, second, ..] = e.data();
            if first >> 28 != 4 {
                return None;
            }
            let channel = ((first >> 16) & 0x0F) as u16;
            let key = ((first >> 8) & 0x7F) as u8;
            let velocity = (second >> 16) as u16;
            match (first >> 20) & 0x0F {
                0x9 => Some((e.header().time(), channel, key, velocity.max(1), true, f64::from(velocity) / 65535.0)),
                0x8 => Some((e.header().time(), channel, key, 0, false, f64::from(velocity) / 65535.0)),
                _ => None,
            }
        }
        _ => None,
    }
}

impl Catch {
    /// One block of the DAW's: the notes on its note input go into the ring, each with its beat, and (with Thru on)
    /// into [`Catch::thru`]. Never waits and never asks for memory.
    pub fn block(&mut self, input: &InputEvents, clock: Clock) {
        self.thru.clear();
        let thru = self.checker.thru.load(Ordering::Relaxed);
        for event in input {
            let Some(core) = event.as_core_event() else { continue };
            let Some((offset, channel, key, velocity, on, clap_velocity)) = note_of(core) else { continue };
            let at = f64::from(offset) * clock.beats_a_frame;
            let played = Played {
                key,
                velocity,
                on,
                beat: clock.song.map_or(self.own + at, |s| s.beat + at),
                song: clock.song.is_some(),
                bar: clock.song.and_then(|s| s.bar),
            };
            // A full ring (the reading thread stalled for a long while) drops the note: the take reads on without it.
            let _ = self.to.push(played);
            if thru && self.thru.len() < THRU_MOST {
                self.thru.push(Thru { offset, channel, key, velocity: clap_velocity, on });
            }
        }
        self.own += f64::from(clock.frames) * clock.beats_a_frame;
        let now = clock.song.map_or(self.own, |s| s.beat + f64::from(clock.frames) * clock.beats_a_frame);
        self.checker.beat.store(now.to_bits(), Ordering::Relaxed);
        self.checker.song.store(clock.song.is_some(), Ordering::Release);
    }

    /// The notes to pass on this block, in the order they came (Thru).
    pub fn thru(&self) -> &[Thru] {
        &self.thru
    }
}

/// One take: the notes from the first one pressed, on a grid of bars from where it started.
#[derive(Clone, Debug)]
pub struct Take {
    /// Counted on the DAW's song (it played) or on the plugin's own count (it was stopped).
    pub song: bool,
    /// The beat tick 0 is: a bar line of the song's, or the first note's beat.
    pub origin: f64,
    /// Beats a bar.
    pub bar_beats: f64,
    /// Each key held: the tick it went down and its velocity.
    held: Vec<Option<(i64, u16)>>,
    /// The notes let go, in the order they were let go.
    pub notes: Vec<Note>,
    /// The beat the last key held was let go on.
    last_up: f64,
}

impl Take {
    /// A take starting with `first`.
    pub fn start(first: &Played) -> Take {
        let (origin, bar_beats) = match first.bar.filter(|(_, len)| *len > 0.0 && len.is_finite()) {
            // On the song's bars: the bar the first note is in.
            Some((start, len)) if first.song => (start + ((first.beat - start) / len).floor() * len, len),
            // Played freely: the first note is the first downbeat, in 4/4.
            _ => (first.beat, 4.0),
        };
        Take { song: first.song, origin, bar_beats, held: vec![None; 128], notes: Vec::new(), last_up: first.beat }
    }

    /// The tick at `beat`.
    pub fn tick(&self, beat: f64) -> i64 {
        ((beat - self.origin) * PPQ as f64).round() as i64
    }

    /// Ticks a bar.
    pub fn bar(&self) -> i64 {
        (self.bar_beats * PPQ as f64).round().max(1.0) as i64
    }

    /// The grid the chord reader reads the take on: beats of a quarter note, bars from tick 0.
    pub fn grid(&self) -> Grid {
        let beats = self.bar_beats.round().clamp(1.0, 32.0) as u32;
        Grid { ticks_per_beat: PPQ as u32, beats_per_bar: beats, bar_line: 0 }
    }

    /// A key pressed or let go.
    pub fn add(&mut self, p: &Played) {
        let tick = self.tick(p.beat);
        let Some(slot) = self.held.get_mut(usize::from(p.key)) else { return };
        // Pressed again while held (or let go): the note held ends here.
        if let Some((start, velocity)) = slot.take() {
            self.notes.push(Note { start, length: (tick - start).max(1), key: p.key, velocity });
        }
        if p.on {
            *slot = Some((tick, p.velocity));
        } else if self.held.iter().all(Option::is_none) {
            self.last_up = p.beat;
        }
    }

    /// Whether a key is held.
    pub fn holding(&self) -> bool {
        self.held.iter().any(Option::is_some)
    }

    /// The keys held, lowest first.
    pub fn held_keys(&self) -> Vec<u8> {
        (0u8..128).filter(|&k| self.held.get(usize::from(k)).is_some_and(Option::is_some)).collect()
    }

    /// Whether the take is over at `beat`: nothing held for [`GAP_BARS`] bars, or it has [`MOST_NOTES`].
    pub fn over(&self, beat: f64) -> bool {
        self.notes.len() >= MOST_NOTES || (!self.holding() && beat - self.last_up >= GAP_BARS * self.bar_beats)
    }

    /// Every note up to `beat`: those let go, and those held, as held until then.
    pub fn notes_until(&self, beat: f64) -> Vec<Note> {
        let now = self.tick(beat);
        let held = self.held.iter().enumerate().filter_map(|(key, h)| {
            let (start, velocity) = (*h)?;
            Some(Note { start, length: (now - start).max(1), key: u8::try_from(key).ok()?, velocity })
        });
        let mut notes: Vec<Note> = self.notes.iter().copied().chain(held).collect();
        notes.sort_by_key(|n| (n.start, n.key));
        notes
    }

    /// The chords of the newest [`READ_BARS`] bars up to `beat`, as the panel shows them. None with nothing to read.
    pub fn read(&self, reader: &mut ChordReader, beat: f64) -> Option<Heard> {
        let now = self.tick(beat).max(0);
        let bar = self.bar();
        let this_bar = now.div_euclid(bar);
        let from = (this_bar - (READ_BARS - 1)).max(0) * bar;
        let notes: Vec<Note> = self
            .notes_until(beat)
            .into_iter()
            .filter(|n| n.end() > from)
            .map(|n| Note {
                start: n.start.max(from) - from,
                length: n.end().min(now.max(n.start + 1)) - n.start.max(from),
                ..n
            })
            .filter(|n| n.length > 0)
            .collect();
        let held: Vec<String> = self.held_keys().into_iter().map(note_name).collect();
        if notes.is_empty() {
            return Some(Heard { held, take_bars: this_bar + 1, ..Heard::default() });
        }
        let parts = [Part { role: Role::Harmony, notes: &notes }];
        let reading = reader.read(&parts, self.grid(), &ChordContext::new(Span::Window))?;
        let names = reading.names(None);
        let beat_ticks = PPQ;
        let at = now - from;
        let mut strip = Vec::new();
        let mut now_chord = None;
        for (span, name) in reading.spans.iter().zip(names) {
            let Some(name) = name else { continue };
            let start = span.start + from;
            let sure = sure_of(span.answer);
            let place = format!("{}.{}", start.div_euclid(bar) + 1, (start.rem_euclid(bar)) / beat_ticks + 1);
            let sounding = span.start <= at && at <= span.end;
            if sounding {
                now_chord = Some((name.name(), sure));
            }
            strip.push(StripChord { name: name.name(), at: place, sure, now: sounding });
        }
        let cut = strip.len().saturating_sub(STRIP_MOST);
        strip.drain(..cut);
        // Nothing sounds now (a gap, or the chord held is let go): the chord shown is the last one read.
        let (name, sure) = now_chord.or_else(|| strip.last().map(|c| (c.name.clone(), c.sure))).unzip();
        Some(Heard { name, sure: sure.unwrap_or(Sure::No), held, strip, take_bars: this_bar + 1, song: self.song })
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

fn sure_of(answer: ChordAnswer) -> Sure {
    match answer {
        ChordAnswer::Chord => Sure::Yes,
        ChordAnswer::SameNotes | ChordAnswer::Implied => Sure::Mostly,
        ChordAnswer::Unclear => Sure::Unsure,
        ChordAnswer::NoChord(_) => Sure::No,
    }
}

/// A note's name with its octave (C4 is middle C), as charts name keys.
fn note_name(key: u8) -> String {
    format!("{}{}", PitchClass::of_key(key), i32::from(key) / 12 - 1)
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
    /// The keys held, lowest first ("C3 E4 G4").
    pub held: Vec<String>,
    /// The chords of the newest bars, in order.
    pub strip: Vec<StripChord>,
    /// How many bars the take has run.
    pub take_bars: i64,
    /// Counted on the DAW's song.
    pub song: bool,
}

impl Heard {
    /// The line under the chord: the keys held and how far the take is.
    pub fn line(&self) -> String {
        let held = if self.held.is_empty() { "No keys held".to_owned() } else { self.held.join(" ") };
        let counted = if self.song { "on the song's bars" } else { "counted from the first note" };
        format!("{held}  \u{b7}  bar {} of the take, {counted}", self.take_bars.max(1))
    }
}

/// Chord check's shared part: what the audio thread tells the reading thread, the Thru switch and the newest
/// [`Heard`] for the panel.
pub struct Checker {
    /// The beat at the newest block's end (f64 bits), and whether it's the song's.
    beat: AtomicU64,
    song: AtomicBool,
    /// Counts the DAW's starts of the track's audio: an older reading thread ends.
    run: AtomicU64,
    /// Thru: the notes played in go on out of the note output, for an instrument after the plugin.
    pub thru: AtomicBool,
    /// The newest for the panel; the panel takes it.
    pub heard: Mutex<Option<Heard>>,
}

impl Default for Checker {
    fn default() -> Checker {
        Checker {
            beat: AtomicU64::new(0f64.to_bits()),
            song: AtomicBool::new(false),
            run: AtomicU64::new(0),
            thru: AtomicBool::new(true),
            heard: Mutex::new(None),
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
        Catch { to, own: 0.0, thru: Vec::with_capacity(THRU_MOST), checker: Arc::clone(self) }
    }

    /// The DAW stopped the track's audio: the reading thread ends.
    pub fn stop(&self) {
        self.run.fetch_add(1, Ordering::AcqRel);
    }

    /// The beat now, and whether it's the song's.
    fn now(&self) -> (f64, bool) {
        let song = self.song.load(Ordering::Acquire);
        (f64::from_bits(self.beat.load(Ordering::Relaxed)), song)
    }

    /// The reading thread: takes in what was played every [`READ_EVERY`] and reads the take while one is under way.
    fn read_on(&self, run: u64, mut from: rtrb::Consumer<Played>) {
        let mut reader = ChordReader::new();
        let mut steps = Steps::default();
        while self.run.load(Ordering::Acquire) == run {
            std::thread::sleep(READ_EVERY);
            let mut came = Vec::new();
            while let Ok(p) = from.pop() {
                came.push(p);
            }
            let (beat, song) = self.now();
            if let Some(heard) = steps.step(&came, beat, song, &mut reader) {
                *lock(&self.heard) = Some(heard);
            }
        }
    }
}

/// The reading thread's state from step to step: the take under way, and whether it changed since it was read.
#[derive(Default)]
pub struct Steps {
    pub take: Option<Take>,
    changed: bool,
    /// The bar the take was last read in: a held chord is read again each bar, so the strip moves on.
    read_in: i64,
}

impl Steps {
    /// One step: what `came` is added to the take (a new take starts at the first key pressed after one ended, or
    /// when the song starts or stops), and the take is read again when it changed. What the panel should show, when
    /// that changed.
    pub fn step(&mut self, came: &[Played], beat: f64, song: bool, reader: &mut ChordReader) -> Option<Heard> {
        for p in came {
            if self.take.as_ref().is_some_and(|t| t.song != p.song) {
                self.take = None;
            }
            match &mut self.take {
                Some(take) => take.add(p),
                None if p.on => {
                    let mut take = Take::start(p);
                    take.add(p);
                    self.take = Some(take);
                }
                None => {}
            }
            self.changed = true;
        }
        let take = self.take.as_ref()?;
        if take.song != song || take.over(beat) {
            // Ended: the last reading stays on the panel until the next take starts.
            self.take = None;
            return None;
        }
        let bar = take.tick(beat).max(0).div_euclid(take.bar());
        if !self.changed && !(take.holding() && bar != self.read_in) {
            return None;
        }
        self.changed = false;
        self.read_in = bar;
        take.read(reader, beat)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn press(key: u8, beat: f64) -> Played {
        Played { key, velocity: up7(100), on: true, beat, song: false, bar: None }
    }

    fn lift(key: u8, beat: f64) -> Played {
        Played { key, velocity: 0, on: false, beat, song: false, bar: None }
    }

    /// A chord held for `beats` from `at`.
    fn chord(keys: &[u8], at: f64, beats: f64) -> Vec<Played> {
        let mut out: Vec<Played> = keys.iter().map(|&k| press(k, at)).collect();
        out.extend(keys.iter().map(|&k| lift(k, at + beats - 0.05)));
        out
    }

    #[test]
    fn velocities_scale_up_as_midi_2_does() {
        assert_eq!(up7(0), 0);
        assert_eq!(up7(64), 0x8000);
        assert_eq!(up7(127), 0xFFFF);
        assert!(up7(100) > up7(99));
        assert_eq!(up_clap(1.0), 0xFFFF);
        assert_eq!(up_clap(f64::NAN), 0);
    }

    #[test]
    fn a_take_counts_from_its_first_note_when_the_song_is_stopped() {
        let take = Take::start(&press(60, 7.25));
        assert_eq!(take.tick(7.25), 0);
        assert_eq!(take.tick(8.25), PPQ);
        assert_eq!(take.grid().beats_per_bar, 4);
    }

    #[test]
    fn a_take_counts_on_the_songs_bars_while_it_plays() {
        // 3/4 bars from beat 1; the first note on beat 8.5 is in the bar from beat 7.
        let first = Played { song: true, bar: Some((1.0, 3.0)), ..press(60, 8.5) };
        let take = Take::start(&first);
        assert_eq!(take.origin, 7.0);
        assert_eq!(take.tick(8.5), (1.5 * PPQ as f64) as i64);
        assert_eq!(take.grid().beats_per_bar, 3);
    }

    #[test]
    fn notes_held_count_until_now_and_a_key_pressed_twice_ends_the_first() {
        let mut take = Take::start(&press(60, 0.0));
        take.add(&press(60, 0.0));
        take.add(&press(64, 0.0));
        take.add(&lift(64, 1.0));
        take.add(&press(60, 2.0)); // pressed again while held
        let notes = take.notes_until(3.0);
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].key, notes[0].length), (60, 2 * PPQ));
        assert_eq!((notes[1].key, notes[1].length), (64, PPQ));
        assert_eq!((notes[2].start, notes[2].length), (2 * PPQ, PPQ), "held: until now");
        assert_eq!(take.held_keys(), [60]);
    }

    #[test]
    fn a_take_ends_after_two_bars_with_nothing_held() {
        let mut take = Take::start(&press(60, 0.0));
        take.add(&press(60, 0.0));
        assert!(!take.over(100.0), "still held");
        take.add(&lift(60, 4.0));
        assert!(!take.over(11.9));
        assert!(take.over(12.0));
    }

    #[test]
    fn it_names_the_chords_played() {
        // Am7, Dm7, G7, Cmaj7, a bar each, played freely.
        let mut played = Vec::new();
        for (bar, keys) in [[45, 60, 64, 67], [50, 60, 65, 69], [43, 59, 62, 65], [48, 59, 64, 67]].iter().enumerate() {
            played.extend(chord(keys, bar as f64 * 4.0, 4.0));
        }
        let mut steps = Steps::default();
        let mut reader = ChordReader::new();
        let heard = steps.step(&played, 15.99, false, &mut reader).unwrap();
        let names: Vec<&str> = heard.strip.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Am7", "Dm7", "G7", "Cmaj7"], "{heard:?}");
        assert_eq!(heard.strip[1].at, "2.1");
        assert_eq!(heard.name.as_deref(), Some("Cmaj7"));
        assert!(heard.strip.last().unwrap().now);
        assert!(!heard.song);
        // Nothing changed: nothing new to show.
        assert!(steps.step(&[], 15.99, false, &mut reader).is_none());
        // Two bars later with nothing held, the take is over.
        assert!(steps.step(&[], 24.0, false, &mut reader).is_none());
        assert!(steps.take.is_none());
    }

    #[test]
    fn a_chord_held_is_shown_while_it_sounds() {
        let mut steps = Steps::default();
        let mut reader = ChordReader::new();
        let held: Vec<Played> = [48, 52, 55, 59].iter().map(|&k| press(k, 0.0)).collect();
        let heard = steps.step(&held, 3.0, false, &mut reader).unwrap();
        assert_eq!(heard.name.as_deref(), Some("Cmaj7"), "{heard:?}");
        assert_eq!(heard.held, ["C3", "E3", "G3", "B3"]);
        assert!(heard.line().starts_with("C3 E3 G3 B3"));
        // Held into the next bar: read again there.
        assert!(steps.step(&[], 5.0, false, &mut reader).is_some());
    }

    #[test]
    fn the_song_starting_starts_a_new_take() {
        let mut steps = Steps::default();
        let mut reader = ChordReader::new();
        steps.step(&[press(60, 0.0)], 1.0, false, &mut reader);
        assert!(steps.take.as_ref().is_some_and(|t| !t.song));
        let on_song = Played { song: true, bar: Some((0.0, 4.0)), ..press(64, 33.0) };
        steps.step(&[on_song], 33.5, true, &mut reader);
        let take = steps.take.as_ref().unwrap();
        assert!(take.song);
        assert_eq!(take.origin, 32.0);
    }
}
