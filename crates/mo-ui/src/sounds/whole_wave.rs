//! A long sound's whole waveform, for its card. The sound library keeps the waveform of a sound's first minute (all its
//! analysis listens to, [`mo_audio::MAX_SECONDS`]), so for a longer sound the card's waveform is read from its file
//! once more, a part at a time, away from the window's thread, and kept while the app runs: the waveform, its play head
//! and a click on it then all mean the same moment of the whole song. Until it's back, the card shows the first minute
//! in its share of the width ([`super::card_wave`]).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::NaiveDateTime;
use mo_app::background::Newest;
use mo_app::log;
use mo_audio::AudioError;
use mo_sounds::Sound;
use slint::ComponentHandle;

use crate::AppWindow;

/// How many lines across a whole waveform: about one for every pixel of a wide card.
pub(super) const POINTS: usize = 400;

/// How many whole waveforms are kept, the newest: under 2 KB each.
const KEEP: usize = 32;

/// How long each part read is.
const PART_SECONDS: f64 = 1.0;

/// The finest step of a waveform as it's read, in seconds: its lines are made from these once its length is known.
const STEP_SECONDS: f64 = 0.01;

/// Which sound a whole waveform is of: its id, and when its file was last changed as the list said (a file changed
/// since is read again).
type Which = (i64, NaiveDateTime);

/// A whole waveform to read: whose, from which file, and which ask it is (a newer one stops it part way).
struct Job {
    which: Which,
    path: String,
    ask: u64,
    window: slint::Weak<AppWindow>,
}

/// The newest ask: a read that isn't it stops.
static NEWEST: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The whole waveforms read, the newest last.
    static KEPT: RefCell<VecDeque<(Which, Arc<[f32]>)>> = const { RefCell::new(VecDeque::new()) };
    /// The one asked for last, so showing its card again while it's read doesn't read it again.
    static ASKED: Cell<Option<Which>> = const { Cell::new(None) };
    /// The window the cards show in; none in the tests and the screenshots, which ask for nothing.
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    /// The thread that reads them, started at the first ask.
    static READER: RefCell<Option<Newest<Job>>> = const { RefCell::new(None) };
}

/// The window whose card shows the waveforms once they're back.
pub(super) fn connect(app: &AppWindow) {
    WINDOW.with(|w| *w.borrow_mut() = Some(app.as_weak()));
}

/// `s`'s whole waveform, once it's been read.
pub(super) fn kept(s: &Sound) -> Option<Arc<[f32]>> {
    kept_of((s.id, s.added))
}

fn kept_of(which: Which) -> Option<Arc<[f32]>> {
    KEPT.with(|k| k.borrow().iter().find(|(w, _)| *w == which).map(|(_, wave)| Arc::clone(wave)))
}

/// Asks for `s`'s whole waveform: read on the reader's thread, a newer ask winning, and shown on the card once it's
/// back ([`super::whole_wave_arrived`]). Nothing is asked without a window (the tests, the screenshots).
pub(super) fn ask(s: &Sound) {
    let which = (s.id, s.added);
    let Some(window) = WINDOW.with(|w| w.borrow().clone()) else { return };
    if ASKED.get() == Some(which) {
        return;
    }
    ASKED.set(Some(which));
    let ask = NEWEST.fetch_add(1, Ordering::SeqCst) + 1;
    let job = Job { which, path: s.path.clone(), ask, window };
    READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            match Newest::start("Read whole waveform", read_job) {
                Ok(reader) => *r = Some(reader),
                Err(e) => {
                    log::warning("Sounds", &format!("Couldn't start reading a whole waveform: {e}"));
                    ASKED.set(None);
                    return;
                }
            }
        }
        if let Some(reader) = r.as_ref() {
            reader.ask(job);
        }
    });
}

/// Lets go of the whole waveform asked for last, as the card no longer needs it (a sound a minute long or less is
/// picked, or one whose whole waveform is kept): a read of it stops at its next part, and asking for it again reads it
/// again (after a read that failed, say).
pub(super) fn let_go() {
    if ASKED.take().is_some() {
        NEWEST.fetch_add(1, Ordering::SeqCst);
    }
}

/// Keeps `wave` as `which`'s whole waveform, the newest, letting the oldest go past [`KEEP`].
pub(super) fn keep(which: Which, wave: Arc<[f32]>) {
    KEPT.with(|k| {
        let mut k = k.borrow_mut();
        k.retain(|(w, _)| *w != which);
        k.push_back((which, wave));
        while k.len() > KEEP {
            k.pop_front();
        }
    });
}

/// Reads one whole waveform on the reader's thread, and hands it to the window's.
fn read_job(job: Job) {
    let Job { which, path, ask, window } = job;
    let wanted = || NEWEST.load(Ordering::SeqCst) == ask;
    if !wanted() {
        return;
    }
    match read(Path::new(&path), wanted) {
        Ok(Some(wave)) => {
            let wave: Arc<[f32]> = wave.into();
            let _ = window.upgrade_in_event_loop(move |app| {
                keep(which, wave);
                super::whole_wave_arrived(&app, which.0);
            });
        }
        // A newer one was asked for: it's read next.
        Ok(None) => {}
        Err(e) => log::warning("Sounds", &format!("Couldn't read the whole waveform of {path}: {e}")),
    }
}

/// The whole of the sound file at `path` as [`POINTS`] lines, each the loudest of its share of the sound's mono mix
/// (as the analysis makes a waveform); none when `wanted` says to stop part way.
fn read(path: &Path, wanted: impl Fn() -> bool) -> Result<Option<Vec<f32>>, AudioError> {
    let mut steps: Vec<f32> = Vec::new();
    let (mut loudest, mut filled, mut stopped) = (0f32, 0usize, false);
    mo_audio::read_parts(path, PART_SECONDS, &mut |rate, part| {
        if !wanted() {
            stopped = true;
            return false;
        }
        let step = ((f64::from(rate) * STEP_SECONDS).round() as usize).max(1);
        let frames = part.iter().map(Vec::len).min().unwrap_or(0);
        let share = 1.0 / part.len().max(1) as f32;
        for i in 0..frames {
            let mono: f32 = part.iter().map(|c| c[i]).sum::<f32>() * share;
            loudest = loudest.max(mono.abs());
            filled += 1;
            if filled == step {
                steps.push(loudest);
                (loudest, filled) = (0.0, 0);
            }
        }
        true
    })?;
    if stopped {
        return Ok(None);
    }
    if filled > 0 {
        steps.push(loudest);
    }
    Ok(Some(lines(&steps, POINTS)))
}

/// `steps` as `points` lines, each the loudest of its share.
pub(super) fn lines(steps: &[f32], points: usize) -> Vec<f32> {
    (0..points)
        .map(|p| {
            let from = steps.len() * p / points;
            let to = (from + 1).max(steps.len() * (p + 1) / points).min(steps.len());
            steps.get(from..to).unwrap_or_default().iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_waveform_spans_the_whole_sound() {
        // Two minutes, quiet for the first and loud for the second: past the minute the library's waveform covers.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/mo-ui-whole-wave");
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let path = dir.join("two-minutes.wav");
        let samples: Vec<f32> = (0..120_000).map(|i| if i < 60_000 { 0.1 } else { -0.8 }).collect();
        assert!(mo_audio::write_wav(&path, &[samples.clone(), samples], 1000, 16).is_ok());
        let Ok(Some(wave)) = read(&path, || true) else { panic!("not read") };
        assert_eq!(wave.len(), POINTS);
        assert!(wave[..POINTS / 2].iter().all(|w| (w - 0.1).abs() < 1e-3), "{wave:?}");
        assert!(wave[POINTS / 2..].iter().all(|w| (w - 0.8).abs() < 1e-3), "{wave:?}");
        // A newer ask stops it.
        assert!(matches!(read(&path, || false), Ok(None)));
        assert!(read(&dir.join("not-there.wav"), || true).is_err());
    }

    #[test]
    fn lines_take_the_loudest_of_their_share() {
        assert_eq!(lines(&[0.1, 0.5, 0.2, 0.9], 2), vec![0.5, 0.9]);
        // Fewer steps than lines: each line shows the step under it.
        assert_eq!(lines(&[0.1, 0.5], 4), vec![0.1, 0.1, 0.5, 0.5]);
        assert_eq!(lines(&[], 3), vec![0.0; 3]);
    }

    #[test]
    fn letting_go_stops_the_read_asked_for_last() {
        let which = (5, NaiveDateTime::default());
        ASKED.set(Some(which));
        let newest = NEWEST.load(Ordering::SeqCst);
        let_go();
        // A read of it stops at its next part (it's no longer the newest ask), and it can be asked for again.
        assert_eq!((NEWEST.load(Ordering::SeqCst), ASKED.get()), (newest + 1, None));
        // Nothing asked for: nothing to stop.
        let_go();
        assert_eq!(NEWEST.load(Ordering::SeqCst), newest + 1);
    }

    #[test]
    fn the_newest_waveforms_are_kept() {
        let at = NaiveDateTime::default();
        for id in 0..(KEEP as i64 + 2) {
            keep((id, at), Arc::from(vec![id as f32]));
        }
        assert!(kept_of((1, at)).is_none());
        let newest = KEEP as i64 + 1;
        assert_eq!(kept_of((newest, at)).as_deref(), Some(&[newest as f32][..]));
        // Its file changed since: read again.
        assert!(kept_of((newest, at + chrono::Duration::seconds(1))).is_none());
    }
}
