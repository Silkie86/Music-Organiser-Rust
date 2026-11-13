//! Record: a take of what comes in, saved as a WAV in the app's Recordings folder, that drags into the project like a
//! sound (Solomon, 2026-11-13: "sample my sound card and drag that audio into my project", tried in the plugin first).
//!
//! Two ways in, to find which works on his PC:
//! - the DAW's input to the plugin (its sidechain in Bitwig: the sound card's input picked there), pushed in from the
//!   audio thread;
//! - a sound card's input opened by the plugin itself, in Windows' shared mode beside the DAW (it can't be while
//!   the DAW holds the card for itself, as an ASIO driver can).
//!
//! The take, its trim and its file are `mo_audio::record`'s, which the app can use as it is. A take runs on a thread
//! of its own and outlives the panel: closing the window doesn't stop it.

use crate::threads::{background, lock};
use mo_audio::record::{self, Feed};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the take is gathered and the panel told how it's going.
const GATHER_EVERY: Duration = Duration::from_millis(50);

/// What the panel is told.
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// Recording: how long so far, the loudest so far (dB), and from where.
    Going { seconds: f64, peak_db: Option<f64>, from: String },
    /// Saved, at this file.
    Saved(PathBuf),
    /// It couldn't, and why.
    Failed(String),
}

impl Said {
    /// In words, for the line beside the button.
    pub fn words(&self) -> String {
        match self {
            Said::Going { seconds, peak_db, from } => {
                let whole = seconds.max(0.0) as u64;
                let peak = peak_db.map_or_else(|| "nothing heard yet".to_owned(), |db| format!("peak {db:.0} dB"));
                format!("Recording {}:{:02} from {from}  \u{b7}  {peak}", whole / 60, whole % 60)
            }
            Said::Saved(path) => format!("Saved {}", path.display()),
            Said::Failed(why) => why.clone(),
        }
    }
}

/// The plugin's recorder, shared by its threads.
#[derive(Default)]
pub struct Recorder {
    /// The DAW's input is being recorded: the audio thread takes [`Recorder::feed`] and pushes into it while on.
    daw_on: AtomicBool,
    feed: Mutex<Option<Feed>>,
    /// A take is under way.
    busy: AtomicBool,
    stop: AtomicBool,
    /// The DAW's sample rate, from when it started the track's audio.
    rate: AtomicU32,
    /// The newest for the panel.
    pub said: Mutex<Option<Said>>,
}

/// The words for the DAW's input, as the panel names it.
pub const DAW_INPUT: &str = "the DAW's input";

impl Recorder {
    /// The DAW's sample rate (set as the DAW starts the audio).
    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::Relaxed);
    }

    /// Whether a take is under way.
    pub fn recording(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Starts a take from the sound card input called `card`, or the DAW's input for none, to be saved with the
    /// silence at its ends cut at `trim_db` (none: kept). Says why not when one is under way already.
    pub fn start(self: &Arc<Self>, card: Option<String>, trim_db: Option<i32>) -> Result<(), String> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("A take is under way already".into());
        }
        self.stop.store(false, Ordering::Release);
        let me = Arc::clone(self);
        background("mo-plugin-record", move || {
            let said = me.take(card, trim_db);
            me.daw_on.store(false, Ordering::Release);
            *lock(&me.said) = Some(said);
            me.busy.store(false, Ordering::Release);
        });
        Ok(())
    }

    /// Ends the take under way: it's saved.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// The take itself, on its own thread: what became of it.
    fn take(&self, card: Option<String>, trim_db: Option<i32>) -> Said {
        let (input, mut take, from) = match &card {
            Some(name) => match record::card::open(Some(name)) {
                Ok((input, take)) => {
                    let from = input.name.clone();
                    crate::log::line(
                        "Record",
                        &format!("Recording from {from} ({} Hz, {} channels)", input.rate, input.channels),
                    );
                    (Some(input), take, from)
                }
                Err(why) => {
                    crate::log::line("Record", &format!("Couldn't open {name}: {why}"));
                    return Said::Failed(format!("Couldn't open {name}: {why}"));
                }
            },
            None => {
                let rate = self.rate.load(Ordering::Relaxed);
                if rate == 0 {
                    return Said::Failed("The DAW hasn't started the track's audio yet: turn the track on".into());
                }
                let (feed, take) = record::recording(rate, 2);
                *lock(&self.feed) = Some(feed);
                self.daw_on.store(true, Ordering::Release);
                crate::log::line("Record", &format!("Recording the DAW's input ({rate} Hz)"));
                (None, take, DAW_INPUT.to_owned())
            }
        };
        loop {
            std::thread::sleep(GATHER_EVERY);
            let ended = take.gather();
            *lock(&self.said) =
                Some(Said::Going { seconds: take.seconds(), peak_db: take.peak_db(), from: from.clone() });
            if self.stop.load(Ordering::Acquire) || ended || take.full() {
                break;
            }
        }
        // The way in is closed: the audio thread lets its feed go, a sound card's input closes.
        self.daw_on.store(false, Ordering::Release);
        drop(lock(&self.feed).take());
        drop(input);
        let audio = take.finish();
        let seconds = audio.file_seconds;
        let name = format!("Recording {}", chrono::Local::now().format("%Y-%m-%d %H-%M-%S"));
        let folder = folder();
        match record::save(audio, trim_db.map(mo_audio::trim::level_of_db), &folder, &name) {
            Ok(path) => {
                crate::log::line("Record", &format!("Saved {seconds:.1} s from {from} as {}", path.display()));
                Said::Saved(path)
            }
            Err(why) => {
                crate::log::line("Record", &format!("Not saved ({seconds:.1} s from {from}): {why}"));
                let hint = if card.is_none() {
                    ". In Bitwig, pick the sound card's input in the plugin's sidechain, and keep the track on"
                } else {
                    ""
                };
                Said::Failed(format!("{why}{hint}"))
            }
        }
    }

    /// On the audio thread, each block: the feed to push the DAW's input into while it's recorded. Never waits: a
    /// feed being handed over is taken next block.
    pub fn feed_for(&self, held: &mut Option<Feed>) {
        if self.daw_on.load(Ordering::Acquire) {
            if held.is_none()
                && let Ok(mut slot) = self.feed.try_lock()
            {
                *held = slot.take();
            }
        } else if held.is_some() {
            *held = None;
        }
    }
}

/// Where takes are saved: the app's Recordings folder, in its data folder.
pub fn folder() -> PathBuf {
    mo_app::paths::data_dir().join("Recordings")
}

/// The sound card inputs to record from, by name. Asks the system: off the panel's thread.
pub fn inputs() -> Vec<String> {
    record::card::inputs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audio_thread_takes_the_feed_only_while_recording() {
        let recorder = Recorder::default();
        let mut held = None;
        recorder.feed_for(&mut held);
        assert!(held.is_none());
        let (feed, take) = record::recording(1000, 2);
        *lock(&recorder.feed) = Some(feed);
        recorder.daw_on.store(true, Ordering::Release);
        recorder.feed_for(&mut held);
        held.as_mut().expect("taken").push(&[&[0.5; 10], &[0.5; 10]]);
        recorder.daw_on.store(false, Ordering::Release);
        recorder.feed_for(&mut held);
        assert!(held.is_none(), "let go once the take ends");
        assert_eq!(take.finish().channels[0].len(), 10);
    }

    #[test]
    fn the_line_says_how_its_going() {
        let going = Said::Going { seconds: 72.4, peak_db: Some(-6.2), from: "Line 1/2".into() };
        assert_eq!(going.words(), "Recording 1:12 from Line 1/2  \u{b7}  peak -6 dB");
        let quiet = Said::Going { seconds: 0.0, peak_db: None, from: DAW_INPUT.into() };
        assert!(quiet.words().ends_with("nothing heard yet"));
    }
}
