//! Play on the selected channel in FL Studio: a MIDI part plays, round and round, through whatever is on the channel
//! selected in FL's channel rack, as if it were played on a keyboard. The app sends the notes out of the "Music
//! Organiser" MIDI port (the loopMIDI port FL's Music Organiser script listens on); the script leaves notes alone, so
//! FL plays them on the selected channel, as it does any controller's.
//!
//! Nothing goes into the FL project: the notes are only played, so stopping leaves the project as it was. They go at
//! the part's own tempo, not FL's, since FL isn't playing along. The notes keep MIDI 2.0's velocity until the last
//! step: a MIDI 1.0 port takes 7-bit ones (rule 22).
//!
//! The notes are timed on a thread of their own; [`Playing::stop`] (or dropping it) stops them within a few
//! milliseconds and lets go of every note still sounding, so none hangs on in FL. Stopping never waits for that
//! thread, so the window can stop a part without waiting on the MIDI port.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub use mo_formats::NoteEvent;

/// The MIDI port the app plays into for FL Studio: the one FL's Music Organiser script is set to. Never the Bitwig
/// extension's loopback port ([`crate::bitwig::LOOPBACK_PORT`]).
pub const FL_PORT: &str = "Music Organiser";

/// The longest the timing thread sleeps at a time, so Stop takes effect this soon.
const STEP: Duration = Duration::from_millis(4);

/// How late a note may be and still be played: later than this (the computer slept, or stalled), the player skips
/// ahead to now instead of playing everything it missed at once.
const LATEST: Duration = Duration::from_millis(150);

/// A part to play round and round.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// Its notes; muted ones aren't played.
    pub notes: Vec<NoteEvent>,
    /// Ticks to a beat.
    pub ppq: i64,
    /// How long the loop is, in ticks; it's made longer, to the next whole beat, when a note starts after it.
    pub length: i64,
    /// Beats a minute.
    pub tempo: f64,
}

/// Where the notes go: the MIDI port, or a test's stand-in (tests never reach a real port, rule 11).
pub trait NoteOut: Send + 'static {
    /// Sends one MIDI 1.0 message (three bytes for a note or a controller).
    fn send(&mut self, message: &[u8]) -> Result<(), String>;
}

/// Why a part couldn't play in FL Studio, in words to show as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// There's no MIDI output called [`FL_PORT`]: the ports there are.
    NoPort { ports: Vec<String> },
    /// The port is there, but wouldn't open: the system's words.
    Port(String),
    /// The computer's MIDI itself wouldn't start: the system's words.
    NoMidi(String),
    /// The part has no notes to play (all muted, or none).
    Empty,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoPort { .. } => write!(
                f,
                "There's no MIDI port called \u{201c}{FL_PORT}\u{201d}. Make it in loopMIDI (it's the port FL Studio's \
                 Music Organiser script listens on), then try again."
            ),
            Error::Port(why) => write!(
                f,
                "The \u{201c}{FL_PORT}\u{201d} MIDI port wouldn't open ({why}). Another program may be holding it."
            ),
            Error::NoMidi(why) => write!(f, "This computer's MIDI wouldn't start ({why})."),
            Error::Empty => f.write_str("There are no notes in it to play (they're all muted, or there are none)."),
        }
    }
}

impl std::error::Error for Error {}
/// Whether a MIDI output's name is FL's port: the name itself, or as Windows numbers a second device of the same name
/// ("Music Organiser 1"), ignoring case. Never the Bitwig loopback ("Music Organiser Bitwig").
pub fn is_fl_port(name: &str) -> bool {
    let name = name.trim();
    let n = FL_PORT.len();
    let (Some(head), Some(rest)) = (name.get(..n), name.get(n..)) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(FL_PORT) {
        return false;
    }
    let rest = rest.trim();
    rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit())
}

/// Opens the "Music Organiser" MIDI output (on Windows, the loopMIDI port of that name).
#[cfg(feature = "midi-port")]
pub fn open_port() -> Result<Box<dyn NoteOut>, Error> {
    use midir::{MidiOutput, MidiOutputConnection};

    struct Port(MidiOutputConnection);
    impl NoteOut for Port {
        fn send(&mut self, message: &[u8]) -> Result<(), String> {
            self.0.send(message).map_err(|e| e.to_string())
        }
    }

    let out = MidiOutput::new("Music Organiser").map_err(|e| Error::NoMidi(e.to_string()))?;
    let ports = out.ports();
    let names: Vec<String> = ports.iter().map(|p| out.port_name(p).unwrap_or_default()).collect();
    // The exact name first, then a numbered one.
    let at = names
        .iter()
        .position(|n| n.trim().eq_ignore_ascii_case(FL_PORT))
        .or_else(|| names.iter().position(|n| is_fl_port(n)))
        .ok_or_else(|| Error::NoPort { ports: names.clone() })?;
    let port = ports.get(at).ok_or_else(|| Error::NoPort { ports: names.clone() })?;
    let connection = out.connect(port, "Music Organiser play").map_err(|e| Error::Port(e.to_string()))?;
    Ok(Box::new(Port(connection)))
}

/// One message in a loop: when (in ticks from the loop's start) and what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event {
    at: i64,
    /// Note offs go before note ons at the same tick, so a note played again straight after itself is heard.
    on: bool,
    key: u8,
    velocity: u8,
}

/// The loop's length: the part's, made longer to the next whole beat when a note starts at or after it, and never
/// shorter than a beat (so a part with no length can't send notes as fast as it can).
fn loop_length(part: &Part, ppq: i64) -> i64 {
    let last = part.notes.iter().filter(|n| !n.muted && n.start >= 0).map(|n| n.start).max().unwrap_or(0);
    let covering = (last / ppq + 1) * ppq;
    part.length.max(covering).max(ppq)
}

/// A part's notes as one round of the loop: each note on, and its off. A note lets go where the same key starts
/// again (two notes on one key would cut each other short in FL), and at the loop's end, before it starts again.
fn events(part: &Part, length: i64) -> Vec<Event> {
    let mut notes: Vec<&NoteEvent> =
        part.notes.iter().filter(|n| !n.muted && n.start >= 0 && n.start < length && n.key <= 127).collect();
    notes.sort_by_key(|n| (n.key, n.start));
    let mut list = Vec::with_capacity(notes.len() * 2);
    for (i, n) in notes.iter().enumerate() {
        let next = notes.get(i + 1).filter(|m| m.key == n.key).map(|m| m.start);
        if next == Some(n.start) {
            continue; // the same key struck twice at once: once is what FL can play
        }
        let end = (n.start + n.length.max(1)).min(length).min(next.unwrap_or(i64::MAX));
        list.push(Event { at: n.start, on: true, key: n.key, velocity: n.velocity7().clamp(1, 127) });
        list.push(Event { at: end, on: false, key: n.key, velocity: 0 });
    }
    list.sort_by_key(|e| (e.at, e.on, e.key));
    list
}

/// A part playing in FL Studio ([`play`]). Stopping it (or dropping it) lets go of every note.
#[derive(Debug)]
pub struct Playing {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Playing {
    /// Stops the part: the timing thread lets go of every note still sounding within a few milliseconds. Doesn't
    /// wait for it.
    pub fn stop(self) {}

    /// Stops the part and waits (a few milliseconds) until every note is let go: for when the app closes, which
    /// would otherwise end the timing thread before it let go of them.
    pub fn finish(self) {
        self.stop.store(true, Ordering::Relaxed);
        let waited = Instant::now();
        while !self.thread.is_finished() && waited.elapsed() < Duration::from_millis(500) {
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Whether it's still playing: false once it was stopped, or the port went.
    pub fn is_playing(&self) -> bool {
        !self.stop.load(Ordering::Relaxed) && !self.thread.is_finished()
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Plays `part` round and round into `out` until it's stopped: open the port with [`open_port`].
pub fn play(out: Box<dyn NoteOut>, part: &Part) -> Result<Playing, Error> {
    let ppq = part.ppq.max(1);
    let tempo = if part.tempo.is_finite() && part.tempo > 0.0 { part.tempo.clamp(10.0, 999.0) } else { 120.0 };
    let length = loop_length(part, ppq);
    let list = events(part, length);
    if list.is_empty() {
        return Err(Error::Empty);
    }
    let seconds_a_tick = 60.0 / tempo / ppq as f64;
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let thread = thread::Builder::new()
        .name("FL notes".into())
        .spawn(move || {
            let mut keys = Keys { out, sounding: [0; 128] };
            keys.run(&list, length, seconds_a_tick, &stopping);
        })
        .map_err(|e| Error::Port(e.to_string()))?;
    Ok(Playing { stop, thread })
}

/// The port and the notes sounding on it. Going (at the end, or if the thread panics), it lets go of every one of
/// them and sends All Notes Off, so no note hangs on in FL.
struct Keys {
    out: Box<dyn NoteOut>,
    sounding: [u16; 128],
}

impl Keys {
    /// Each event at its time, worked out from one start time, so the loop never drifts.
    fn run(&mut self, list: &[Event], length: i64, seconds_a_tick: f64, stop: &AtomicBool) {
        let mut start = Instant::now();
        let mut round: u64 = 0;
        let round_time = Duration::from_secs_f64(length as f64 * seconds_a_tick);
        loop {
            for e in list {
                let due =
                    start + Duration::from_secs_f64((round as f64 * length as f64 + e.at as f64) * seconds_a_tick);
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let now = Instant::now();
                    if now >= due {
                        break;
                    }
                    thread::sleep((due - now).min(STEP));
                }
                let key = usize::from(e.key);
                let late = Instant::now().saturating_duration_since(due) > LATEST;
                if e.on && late {
                    continue; // missed: not played now, all at once
                }
                let Some(count) = self.sounding.get_mut(key) else { continue };
                if !e.on && *count == 0 {
                    continue; // its note was skipped
                }
                let message = if e.on { [0x90, e.key, e.velocity] } else { [0x80, e.key, 0] };
                if self.out.send(&message).is_err() {
                    return;
                }
                *count = if e.on { count.saturating_add(1) } else { count.saturating_sub(1) };
            }
            round += 1;
            // Far behind (the computer slept): start again from now, at the start of a round.
            let next = start + round_time.mul_f64(round as f64);
            if Instant::now().saturating_duration_since(next) > LATEST {
                start = Instant::now();
                round = 0;
            }
        }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        for (key, count) in self.sounding.iter().enumerate() {
            for _ in 0..*count {
                let _ = self.out.send(&[0x80, key as u8, 0]);
            }
        }
        let _ = self.out.send(&[0xB0, 123, 0]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A stand-in port that keeps what it's sent, with when.
    #[derive(Clone, Default)]
    struct Heard(Arc<Mutex<Log>>, Option<Instant>);

    /// Each message sent, with how long after the first.
    type Log = Vec<(Duration, Vec<u8>)>;

    impl NoteOut for Heard {
        fn send(&mut self, message: &[u8]) -> Result<(), String> {
            let at = self.1.get_or_insert_with(Instant::now).elapsed();
            self.0.lock().unwrap().push((at, message.to_vec()));
            Ok(())
        }
    }

    fn part(notes: Vec<NoteEvent>, length: i64, tempo: f64) -> Part {
        Part { notes, ppq: 96, length, tempo }
    }

    fn note(start: i64, length: i64, key: u8) -> NoteEvent {
        NoteEvent {
            start,
            length,
            key,
            velocity: 50_000,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        }
    }

    #[test]
    fn knows_fls_port_by_name() {
        assert!(is_fl_port("Music Organiser"));
        assert!(is_fl_port("music organiser "));
        assert!(is_fl_port("Music Organiser 1"));
        assert!(!is_fl_port("Music Organiser Bitwig"));
        assert!(!is_fl_port("loopMIDI Port"));
        assert!(!is_fl_port("Music"));
    }

    #[test]
    fn a_long_note_lets_go_at_the_loops_end_and_offs_go_first() {
        let p = part(vec![note(0, 500, 60), note(96, 96, 64), note(192, 96, 64)], 384, 120.0);
        let list = events(&p, 384);
        let at = |t: i64| list.iter().filter(|e| e.at == t).map(|e| (e.on, e.key)).collect::<Vec<_>>();
        assert_eq!(at(0), [(true, 60)]);
        assert_eq!(at(192), [(false, 64), (true, 64)]);
        assert_eq!(at(288), [(false, 64)]);
        assert_eq!(at(384), [(false, 60)]);
    }

    #[test]
    fn plays_round_and_lets_every_note_go_when_stopped() {
        let heard = Heard::default();
        let log = heard.0.clone();
        // 4 beats at 960 bpm: a round every quarter of a second.
        let p = part(vec![note(0, 96, 60), note(192, 400, 67)], 384, 960.0);
        let playing = play(Box::new(heard), &p).unwrap();
        thread::sleep(Duration::from_millis(400));
        assert!(playing.is_playing());
        playing.stop();
        // Stop doesn't wait: the timing thread lets the notes go within a few milliseconds.
        let waited = Instant::now();
        while log.lock().unwrap().last().is_none_or(|(_, m)| m[0] != 0xB0) {
            assert!(waited.elapsed() < Duration::from_secs(2), "the notes weren't let go");
            thread::sleep(Duration::from_millis(5));
        }
        let sent: Vec<Vec<u8>> = log.lock().unwrap().iter().map(|(_, m)| m.clone()).collect();
        let ons = sent.iter().filter(|m| m[0] == 0x90 && m[1] == 60).count();
        assert!(ons >= 2, "went round: {sent:?}");
        let held = |key: u8| {
            sent.iter().filter(|m| m[0] == 0x90 && m[1] == key).count()
                - sent.iter().filter(|m| m[0] == 0x80 && m[1] == key).count()
        };
        assert_eq!((held(60), held(67)), (0, 0), "every note let go: {sent:?}");
        assert_eq!(sent.last().unwrap(), &vec![0xB0, 123, 0]);
    }

    #[test]
    fn keeps_time() {
        let heard = Heard::default();
        let log = heard.0.clone();
        // A note every beat at 600 bpm: every 100 ms.
        let p = part((0..4).map(|b| note(b * 96, 48, 60)).collect(), 384, 600.0);
        let playing = play(Box::new(heard), &p).unwrap();
        thread::sleep(Duration::from_millis(650));
        playing.stop();
        let ons: Vec<Duration> = log.lock().unwrap().iter().filter(|(_, m)| m[0] == 0x90).map(|(t, _)| *t).collect();
        assert!(ons.len() >= 6, "{ons:?}");
        for (i, at) in ons.iter().enumerate() {
            let want = Duration::from_millis(100 * i as u64);
            let off = at.abs_diff(want);
            assert!(off < Duration::from_millis(60), "note {i} at {at:?}, wanted {want:?}");
        }
    }

    #[test]
    fn a_part_with_no_notes_isnt_played() {
        let mut muted = note(0, 10, 60);
        muted.muted = true;
        assert_eq!(play(Box::new(Heard::default()), &part(vec![muted], 384, 120.0)).unwrap_err(), Error::Empty);
        assert_eq!(play(Box::new(Heard::default()), &part(Vec::new(), 384, 120.0)).unwrap_err(), Error::Empty);
    }

    #[test]
    fn a_loop_too_short_for_its_notes_grows_to_hold_them() {
        let p = part(vec![note(0, 10, 60), note(300, 10, 62)], 0, 120.0);
        assert_eq!(loop_length(&p, 96), 384);
        assert_eq!(events(&p, 384).iter().filter(|e| e.on).count(), 2);
    }

    #[test]
    fn notes_on_one_key_that_overlap_take_turns() {
        // A from 0 to 200 and B from 50 to 300 on the same key; and two struck at once at 320.
        let p = part(vec![note(0, 200, 60), note(50, 250, 60), note(320, 10, 64), note(320, 30, 64)], 384, 120.0);
        let list: Vec<(i64, bool, u8)> = events(&p, 384).iter().map(|e| (e.at, e.on, e.key)).collect();
        assert_eq!(
            list,
            [(0, true, 60), (50, false, 60), (50, true, 60), (300, false, 60), (320, true, 64), (350, false, 64)]
        );
    }

    #[test]
    fn velocity_goes_to_7_bits_only_at_the_port() {
        let p = part(vec![note(0, 10, 60)], 96, 120.0);
        assert_eq!(events(&p, 96)[0].velocity, note(0, 10, 60).velocity7());
        assert!(events(&p, 96)[0].velocity > 90);
    }

    #[test]
    fn a_port_that_goes_stops_the_part() {
        struct Gone;
        impl NoteOut for Gone {
            fn send(&mut self, _: &[u8]) -> Result<(), String> {
                Err("unplugged".into())
            }
        }
        let playing = play(Box::new(Gone), &part(vec![note(0, 10, 60)], 96, 120.0)).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(!playing.is_playing());
    }
}
