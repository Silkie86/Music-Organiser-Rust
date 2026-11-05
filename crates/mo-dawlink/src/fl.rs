//! Play on the selected channel in FL Studio: a MIDI part plays, round and round, through whatever is on the channel
//! selected in FL's channel rack, as if it were played on a keyboard. The app sends the notes out of the "Music
//! Organiser" MIDI port (the loopMIDI port FL's Music Organiser script listens on); the script leaves notes alone, so
//! FL plays them on the selected channel, as it does any controller's.
//!
//! Nothing goes into the FL project: the notes are only played, so stopping leaves the project as it was. They go at
//! the part's own tempo, not FL's, since FL isn't playing along. A MIDI 1.0 port takes 7-bit velocities (rule 22), so
//! the caller gives them that way.
//!
//! The notes are timed on a thread of their own; [`Playing::stop`] (or dropping it) stops them within a few
//! milliseconds and lets go of every note still sounding, so none hangs on in FL.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The MIDI port the app plays into for FL Studio: the one FL's Music Organiser script is set to. Never the Bitwig
/// extension's loopback port ([`crate::bitwig::LOOPBACK_PORT`]).
pub const FL_PORT: &str = "Music Organiser";

/// The longest the timing thread sleeps at a time, so Stop takes effect this soon.
const STEP: Duration = Duration::from_millis(4);

/// The fewest beats a loop goes round in, so a part with no length can't send notes as fast as it can.
const SHORTEST_LOOP_BEATS: f64 = 0.25;

/// One note of a part, in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub start: i64,
    pub length: i64,
    /// MIDI note number, 0 to 127.
    pub key: u8,
    /// MIDI 1.0's velocity, 1 to 127.
    pub velocity: u8,
}

/// A part to play round and round.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub notes: Vec<Note>,
    /// Ticks to a beat.
    pub ppq: i64,
    /// How long the loop is, in ticks.
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
    /// The part has no notes to play.
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
            Error::Empty => f.write_str("Every note in it is muted, so there's nothing to play."),
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

    let out = MidiOutput::new("Music Organiser").map_err(|e| Error::Port(e.to_string()))?;
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

/// A part's notes as one round of the loop: each note on, and its off, cut at the loop's end so a long note lets go
/// before the loop starts again.
fn events(part: &Part, length: i64) -> Vec<Event> {
    let mut list = Vec::with_capacity(part.notes.len() * 2);
    for n in &part.notes {
        if n.start < 0 || n.start >= length || n.key > 127 {
            continue;
        }
        let velocity = n.velocity.clamp(1, 127);
        let end = (n.start + n.length.max(1)).min(length);
        list.push(Event { at: n.start, on: true, key: n.key, velocity });
        list.push(Event { at: end, on: false, key: n.key, velocity: 0 });
    }
    list.sort_by_key(|e| (e.at, e.on, e.key));
    list
}

/// A part playing in FL Studio ([`play`]). Stopping it (or dropping it) lets go of every note.
#[derive(Debug)]
pub struct Playing {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Playing {
    /// Stops the part and lets go of every note still sounding. Takes a few milliseconds.
    pub fn stop(mut self) {
        self.halt();
    }

    /// Whether it's still playing: false once it was stopped, or the port went (FL's notes were let go then too, as
    /// far as the port still takes them).
    pub fn is_playing(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Plays `part` round and round into `out` until it's stopped: open the port with [`open_port`].
pub fn play(out: Box<dyn NoteOut>, part: &Part) -> Result<Playing, Error> {
    let ppq = part.ppq.max(1);
    let tempo = if part.tempo.is_finite() && part.tempo > 0.0 { part.tempo.clamp(10.0, 999.0) } else { 120.0 };
    let shortest = (SHORTEST_LOOP_BEATS * ppq as f64).ceil() as i64;
    let length = part.length.max(shortest).max(1);
    let list = events(part, length);
    if list.is_empty() {
        return Err(Error::Empty);
    }
    let tick = Duration::from_secs_f64(60.0 / tempo / ppq as f64);
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let thread = thread::Builder::new()
        .name("FL notes".into())
        .spawn(move || run(out, &list, length, tick, &stopping))
        .map_err(|e| Error::Port(e.to_string()))?;
    Ok(Playing { stop, thread: Some(thread) })
}

/// The timing thread: each event at its time, from a start time worked out once, so the loop never drifts.
fn run(mut out: Box<dyn NoteOut>, list: &[Event], length: i64, tick: Duration, stop: &AtomicBool) {
    let start = Instant::now();
    let mut sounding = [0u16; 128];
    let mut round: u64 = 0;
    'playing: loop {
        for e in list {
            let ticks = (round as f64) * length as f64 + e.at as f64;
            let due = start + tick.mul_f64(ticks);
            loop {
                if stop.load(Ordering::Relaxed) {
                    break 'playing;
                }
                let now = Instant::now();
                if now >= due {
                    break;
                }
                thread::sleep((due - now).min(STEP));
            }
            let key = usize::from(e.key);
            let message = if e.on { [0x90, e.key, e.velocity] } else { [0x80, e.key, 0] };
            if out.send(&message).is_err() {
                break 'playing;
            }
            if let Some(count) = sounding.get_mut(key) {
                *count = if e.on { count.saturating_add(1) } else { count.saturating_sub(1) };
            }
        }
        round += 1;
    }
    // Let go of every note still sounding, then All Notes Off for good measure.
    for (key, count) in sounding.iter().enumerate() {
        for _ in 0..*count {
            let _ = out.send(&[0x80, key as u8, 0]);
        }
    }
    let _ = out.send(&[0xB0, 123, 0]);
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

    fn part(notes: Vec<Note>, length: i64, tempo: f64) -> Part {
        Part { notes, ppq: 96, length, tempo }
    }

    fn note(start: i64, length: i64, key: u8) -> Note {
        Note { start, length, key, velocity: 100 }
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
        let p = part(vec![note(500, 10, 60)], 384, 120.0);
        assert_eq!(play(Box::new(Heard::default()), &p).unwrap_err(), Error::Empty);
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
