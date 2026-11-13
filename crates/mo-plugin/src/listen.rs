//! Listen: the plugin listens to what comes into its input ("In", Bitwig's sidechain for an instrument) and says what
//! it hears: the notes the track plays most, and a rough tempo. Item 6 of Solomon's plugin list (13 Nov), built on
//! the shared listener, `mo_listen`. The key and chords from what it heard come once the harmony engine can name them
//! from heard notes.
//!
//! The audio thread only copies each block into the listener's ring ([`mo_listen::Tap`]) and notes where the song
//! was; a low-priority thread of its own sums it up about every 100 ms and adds it to the song, bar by bar
//! ([`mo_listen::Track`]). It runs while the DAW runs the track's audio, so what it heard is there when the panel
//! opens.

use crate::threads::{background, lock};
use mo_listen::{Ear, Track};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the ear sums up the newest audio.
const HEAR_EVERY: Duration = Duration::from_millis(100);
/// How often the panel is told (and the tempo worked out: it weighs every pair of hits).
const TELL_EVERY: Duration = Duration::from_secs(1);
/// How many notes the panel names, at most.
const MOST_NOTES: usize = 4;
/// A note is named when it was heard at least this much of the loudest one.
const NOTE_SHARE: f32 = 0.5;
/// How many of the newest bars the "lately" notes come from.
const LATELY_BARS: i64 = 4;

/// What the panel is told.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hears {
    /// The notes heard most over the whole time listened, loudest first (0 is C).
    pub notes: Vec<u8>,
    /// The notes heard most in the newest bars, when the song played.
    pub lately: Vec<u8>,
    /// The tempo the hits keep, in beats a minute.
    pub tempo: Option<f64>,
    /// How many bars it heard notes in.
    pub bars: usize,
    /// Anything above silence came in lately.
    pub sound: bool,
}

impl Hears {
    /// In words, for the panel's line.
    pub fn words(&self) -> String {
        if !self.sound && self.notes.is_empty() {
            return "Hears nothing yet: in Bitwig, pick what to listen to in the plugin's sidechain".to_owned();
        }
        let mut said = if self.notes.is_empty() {
            "Hears no notes yet".to_owned()
        } else {
            format!("Hears {}", names(&self.notes))
        };
        if !self.lately.is_empty() && self.lately != self.notes {
            said.push_str(&format!("  \u{b7}  lately {}", names(&self.lately)));
        }
        if self.bars > 0 {
            said.push_str(&format!("  \u{b7}  {} bar{}", self.bars, if self.bars == 1 { "" } else { "s" }));
        }
        if let Some(tempo) = self.tempo {
            said.push_str(&format!("  \u{b7}  hits keep about {tempo:.0} BPM"));
        }
        said
    }
}

/// Note names as chord charts write them (C, Db … F#, G, Ab), as the app names keys.
fn names(notes: &[u8]) -> String {
    notes.iter().map(|n| mo_harmony::PitchClass::new(i32::from(*n)).to_string()).collect::<Vec<_>>().join(" ")
}

/// The notes heard most in `chroma`, loudest first: at most [`MOST_NOTES`], each at least [`NOTE_SHARE`] of the
/// loudest.
pub fn strongest(chroma: &[f32; 12]) -> Vec<u8> {
    let loudest = chroma.iter().copied().fold(0f32, f32::max);
    if loudest <= 0.0 || !loudest.is_finite() {
        return Vec::new();
    }
    let mut notes: Vec<u8> = (0u8..12).filter(|&n| chroma[usize::from(n)] >= loudest * NOTE_SHARE).collect();
    notes.sort_by(|a, b| chroma[usize::from(*b)].total_cmp(&chroma[usize::from(*a)]));
    notes.truncate(MOST_NOTES);
    notes
}

/// The plugin's listener, shared by its threads.
#[derive(Default)]
pub struct Listener {
    /// Where the song was at the end of the newest block: how many frames the tap had then, the song's beat, and
    /// how many beats a second it went (0 when stopped). Read together loosely: a torn read only moves one summary's
    /// notes by a block.
    frames: AtomicU64,
    beat: AtomicU64,
    beats_a_second: AtomicU64,
    /// The song's beats a bar (f64 bits; 0 for not told).
    bar: AtomicU64,
    /// Counts the DAW's starts of the track's audio: the listening thread of an older one ends.
    run: AtomicU64,
    /// The newest for the panel.
    pub hears: Mutex<Option<Hears>>,
}

/// Where the song was at the end of a block, from the DAW's transport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// The beat at the end of the block.
    pub beat: f64,
    /// Beats a second while it plays.
    pub beats_a_second: f64,
    /// Beats a bar, when the DAW says.
    pub bar: Option<f64>,
}

impl Listener {
    /// The DAW started the track's audio: a new listening thread with this `ear`, and the old one ends.
    pub fn start(self: &Arc<Self>, ear: Ear) {
        let run = self.run.fetch_add(1, Ordering::AcqRel) + 1;
        let me = Arc::clone(self);
        background("mo-plugin-listen", move || me.listen(run, ear));
    }

    /// The DAW stopped the track's audio: the listening thread ends.
    pub fn stop(&self) {
        self.run.fetch_add(1, Ordering::AcqRel);
    }

    /// On the audio thread, after each block went into the tap: how many frames the tap has had, and where the song
    /// is (none while stopped). Plain stores: never waits.
    pub fn tell(&self, frames: u64, place: Option<Place>) {
        let (beat, speed, bar) =
            place.map_or((f64::NAN, 0.0, 0.0), |p| (p.beat, p.beats_a_second, p.bar.unwrap_or(0.0)));
        self.beat.store(beat.to_bits(), Ordering::Relaxed);
        self.beats_a_second.store(speed.to_bits(), Ordering::Relaxed);
        self.bar.store(bar.to_bits(), Ordering::Relaxed);
        self.frames.store(frames, Ordering::Release);
    }

    /// The song's beat at the tap's frame `at`, worked back from the newest block's end; none while stopped.
    fn beat_at(&self, at: u64, rate: f64) -> Option<f64> {
        let frames = self.frames.load(Ordering::Acquire);
        let beat = f64::from_bits(self.beat.load(Ordering::Relaxed));
        let speed = f64::from_bits(self.beats_a_second.load(Ordering::Relaxed));
        if !beat.is_finite() || speed <= 0.0 || rate <= 0.0 {
            return None;
        }
        Some(beat - frames.saturating_sub(at) as f64 / rate * speed)
    }

    /// The listening thread: hears every [`HEAR_EVERY`], tells the panel every [`TELL_EVERY`], until a newer run
    /// starts or the audio stops.
    fn listen(&self, run: u64, mut ear: Ear) {
        let rate = ear.sample_rate();
        let mut track = Track::for_ear(&ear);
        let mut told = Instant::now();
        let mut sound = false;
        while self.run.load(Ordering::Acquire) == run {
            std::thread::sleep(HEAR_EVERY);
            let bar = f64::from_bits(self.bar.load(Ordering::Relaxed));
            track.set_bar(bar);
            if let Some(heard) = ear.hear() {
                sound |= heard.peak > mo_listen::QUIET;
                track.add(&heard, self.beat_at(heard.samples_to, rate));
            }
            if told.elapsed() >= TELL_EVERY {
                told = Instant::now();
                *lock(&self.hears) = Some(hears_of(&track, sound));
                sound = false;
            }
        }
    }
}

/// What `track` heard, for the panel.
fn hears_of(track: &Track, sound: bool) -> Hears {
    let lately = track.newest_bar().map_or_else(Vec::new, |newest| {
        let mut chroma = [0f32; 12];
        for (_, bar) in track.bars().filter(|(n, _)| *n > newest - LATELY_BARS) {
            for (sum, c) in chroma.iter_mut().zip(bar.chroma) {
                *sum += c * bar.seconds;
            }
        }
        strongest(&chroma)
    });
    Hears { notes: strongest(&track.all().chroma), lately, tempo: track.tempo(), bars: track.bars().count(), sound }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strongest_notes_loudest_first() {
        let mut chroma = [0f32; 12];
        chroma[9] = 1.0; // A
        chroma[0] = 0.8; // C
        chroma[4] = 0.6; // E
        chroma[7] = 0.2; // G: too quiet to name
        assert_eq!(strongest(&chroma), [9, 0, 4]);
        assert!(strongest(&[0.0; 12]).is_empty());
        assert_eq!(strongest(&[1.0; 12]).len(), MOST_NOTES);
    }

    #[test]
    fn the_line_says_what_it_hears() {
        assert!(Hears::default().words().contains("sidechain"));
        let hears = Hears { notes: vec![9, 0, 4], lately: vec![2, 5], tempo: Some(121.6), bars: 8, sound: true };
        assert_eq!(hears.words(), "Hears A C E  \u{b7}  lately D F  \u{b7}  8 bars  \u{b7}  hits keep about 122 BPM");
        let quiet = Hears { sound: true, ..Hears::default() };
        assert_eq!(quiet.words(), "Hears no notes yet");
    }

    #[test]
    fn the_beat_is_worked_back_from_the_newest_block() {
        let listener = Listener::default();
        assert_eq!(listener.beat_at(0, 1000.0), None);
        // 2 beats a second, at beat 10 when the tap had 3000 frames (at 1000 Hz).
        listener.tell(3000, Some(Place { beat: 10.0, beats_a_second: 2.0, bar: Some(4.0) }));
        assert_eq!(listener.beat_at(3000, 1000.0), Some(10.0));
        assert_eq!(listener.beat_at(2500, 1000.0), Some(9.0));
        listener.tell(4000, None);
        assert_eq!(listener.beat_at(4000, 1000.0), None, "stopped: no beat");
    }

    #[test]
    fn a_track_heard_for_a_while_is_said() {
        // A made-up A minor chord, heard while the song plays at 120 BPM.
        let rate = 8000.0;
        let (mut tap, mut ear) = mo_listen::tap(rate, 1);
        let mut track = Track::for_ear(&ear);
        let tone = |n: usize, f: f64| (2.0 * std::f64::consts::PI * f * n as f64 / rate).sin() as f32 * 0.2;
        let mut at = 0usize;
        for _ in 0..40 {
            let block: Vec<f32> = (at..at + 800).map(|n| tone(n, 220.0) + tone(n, 261.63) + tone(n, 329.63)).collect();
            at += 800;
            tap.push(&[&block]);
            if let Some(heard) = ear.hear() {
                track.add(&heard, Some(at as f64 / rate * 2.0));
            }
        }
        let hears = hears_of(&track, true);
        let mut notes = hears.notes.clone();
        notes.sort_unstable();
        assert_eq!(notes, [0, 4, 9], "{hears:?}");
        assert!(hears.bars >= 1, "{hears:?}");
    }
}
