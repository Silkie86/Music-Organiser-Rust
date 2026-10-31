//! Playing one sound at a time, for auditioning: the C# app's `OpenAlPlayer`, rebuilt on cpal.
//!
//! Sound goes to the output chosen in Settings while it's there, otherwise the computer's default, following it when
//! it changes. When the output goes (unplugged, or a DAW takes the audio interface for itself) the player opens it
//! again and the sound carries on from where it was. Opening and reopening happen on the player's own thread, so the
//! window never waits for them; the output's thread only mixes.

mod mix;
mod output;

pub use output::{Lost, Opened, Outputs, Render, SystemOutputs};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mix::{FromMixer, Mixer, Place, ToMixer};

use crate::AudioData;

/// Times one sound reopens the output before it gives up, so an output that keeps going can't loop (as C#).
const MAX_REOPENS: u32 = 3;

/// After an output wouldn't open, how long before trying again: a sweep through sounds mustn't try twenty times a
/// second while a DAW has the interface (as C#).
const WAIT_AFTER_FAILING: Duration = Duration::from_secs(2);

/// How often a new sound looks whether the outputs changed (the chosen one came back, or the default moved).
const LOOK_EVERY: Duration = Duration::from_secs(3);

/// Where sound is going: what Settings and the audio log show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputInfo {
    /// The output sounds play through, or `None` when none is open (yet, or none would open).
    pub name: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    /// How long sound takes from being mixed to coming out, in milliseconds, as the system last said.
    pub delay_ms: Option<f64>,
    /// Why sound isn't going where it was wanted ("Focusrite USB isn't connected"…), or `None` when it is.
    pub problem: Option<String>,
}

struct Now {
    id: u64,
    key: String,
    /// Where it was told to start (or last moved to), for before the output has played any of it.
    start: f64,
}

#[derive(Default)]
struct Shared {
    now: Mutex<Option<Now>>,
    info: Mutex<OutputInfo>,
    delay: Mutex<Option<Arc<AtomicU64>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Nothing holding these locks can leave them half changed, so a panic elsewhere doesn't make them unusable.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Order {
    Play { id: u64, sound: Arc<AudioData>, start: f64 },
    Seek { id: u64, new_id: u64, seconds: f64 },
    Stop,
    Choose(Option<String>),
    Check,
    List(Sender<Vec<String>>),
    Mixer(FromMixer),
    Lost { opening: u64, why: String },
    Quit,
}

/// Plays one sound at a time. Every call returns at once; the work is done on the player's own thread.
pub struct Player {
    orders: Sender<Order>,
    shared: Arc<Shared>,
    place: Arc<Place>,
    next_id: AtomicU64,
    keeper: Option<JoinHandle<()>>,
}

impl Player {
    /// A player on the computer's own outputs. Nothing opens until the first sound plays. `on_end` is called (on the
    /// player's thread) with a sound's key when it plays to its end, not when it's stopped or replaced.
    pub fn new(on_end: impl Fn(&str) + Send + 'static) -> Player {
        Player::with_outputs(Box::new(SystemOutputs::new()), on_end)
    }

    /// A player on the outputs given (tests give a stand-in).
    pub fn with_outputs(outputs: Box<dyn Outputs>, on_end: impl Fn(&str) + Send + 'static) -> Player {
        let (orders, inbox) = channel();
        let shared = Arc::new(Shared::default());
        let place = Arc::new(Place::default());
        let keeper = Keeper {
            outputs,
            inbox,
            orders: orders.clone(),
            shared: Arc::clone(&shared),
            place: Arc::clone(&place),
            on_end: Box::new(on_end),
            wanted: None,
            open: None,
            openings: 0,
            playing: None,
            reopens: 0,
            failed_at: None,
            looked_at: None,
        };
        let keeper = std::thread::Builder::new().name("sound output".into()).spawn(move || keeper.run()).ok();
        Player { orders, shared, place, next_id: AtomicU64::new(1), keeper }
    }

    /// Plays `sound` from `start_seconds`, replacing whatever is playing. `key` names it (its path, or a key for sound
    /// made in memory), as [`Player::playing`] gives back.
    pub fn play(&self, sound: Arc<AudioData>, key: impl Into<String>, start_seconds: f64) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let start = start_seconds.max(0.0);
        *lock(&self.shared.now) = Some(Now { id, key: key.into(), start });
        let _ = self.orders.send(Order::Play { id, sound, start });
    }

    /// Plays on from `seconds` in the sound playing.
    pub fn seek(&self, seconds: f64) {
        let seconds = seconds.max(0.0);
        let mut now = lock(&self.shared.now);
        if let Some(now) = now.as_mut() {
            // A new id, so the place the output last gave (from before the move) isn't shown meanwhile.
            let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let _ = self.orders.send(Order::Seek { id: now.id, new_id, seconds });
            now.id = new_id;
            now.start = seconds;
        }
    }

    pub fn stop(&self) {
        *lock(&self.shared.now) = None;
        let _ = self.orders.send(Order::Stop);
    }

    /// The key of the sound playing, or `None`.
    pub fn playing(&self) -> Option<String> {
        lock(&self.shared.now).as_ref().map(|n| n.key.clone())
    }

    /// Where the sound playing is, in seconds, or `None` when nothing is playing.
    pub fn position(&self) -> Option<f64> {
        let now = lock(&self.shared.now);
        let now = now.as_ref()?;
        Some(self.place.of(now.id).unwrap_or(now.start))
    }

    /// Plays through this output from now on (`None`: the computer's default), moving a sound that's playing. Before
    /// any sound has played it's only remembered, so the app's start opens nothing.
    pub fn choose(&self, name: Option<String>) {
        let _ = self.orders.send(Order::Choose(name));
    }

    /// Looks at the outputs again, moving back to the chosen one if it has returned; [`Player::info`] then says where
    /// sound goes.
    pub fn check(&self) {
        let _ = self.orders.send(Order::Check);
    }

    /// Where sound is going now.
    pub fn info(&self) -> OutputInfo {
        let mut info = lock(&self.shared.info).clone();
        info.delay_ms = lock(&self.shared.delay)
            .as_ref()
            .map(|d| d.load(Ordering::Relaxed))
            .filter(|us| *us > 0)
            .map(|us| us as f64 / 1000.0);
        info
    }

    /// The outputs there are now, the computer's default first. Looking takes a moment (it waits for the player's
    /// thread), so call it off the window's thread.
    pub fn outputs(&self) -> Vec<String> {
        let (tx, rx) = channel();
        if self.orders.send(Order::List(tx)).is_err() {
            return Vec::new();
        }
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.orders.send(Order::Quit);
        if let Some(keeper) = self.keeper.take() {
            let _ = keeper.join();
        }
    }
}

/// The sound playing, as the player's thread keeps it to start again after the output is reopened.
struct Playing {
    id: u64,
    sound: Arc<AudioData>,
    start: f64,
}

struct Open {
    output: Opened,
    to_mixer: Sender<ToMixer>,
    /// Which opening this is, so word that an older one was lost is ignored.
    opening: u64,
    /// Whether it was opened as the default (following it) rather than by name.
    as_default: bool,
}

/// The player's own thread: opens, moves and reopens the output, and passes orders to the mixer.
struct Keeper {
    outputs: Box<dyn Outputs>,
    inbox: Receiver<Order>,
    orders: Sender<Order>,
    shared: Arc<Shared>,
    place: Arc<Place>,
    on_end: Box<dyn Fn(&str) + Send>,
    /// The output chosen in Settings, or `None` for the computer's default.
    wanted: Option<String>,
    open: Option<Open>,
    openings: u64,
    playing: Option<Playing>,
    /// Times the sound playing has reopened the output.
    reopens: u32,
    failed_at: Option<Instant>,
    looked_at: Option<Instant>,
}

impl Keeper {
    fn run(mut self) {
        loop {
            // While a sound waits for an output that wouldn't open, wake up to try again.
            let retry = self.playing.is_some() && self.open.is_none() && self.reopens < MAX_REOPENS;
            let order = match self.failed_at.filter(|_| retry) {
                Some(at) => {
                    let wait = (at + WAIT_AFTER_FAILING).saturating_duration_since(Instant::now());
                    match self.inbox.recv_timeout(wait) {
                        Ok(order) => order,
                        Err(RecvTimeoutError::Timeout) => {
                            self.reopens += 1;
                            self.reopen();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match self.inbox.recv() {
                    Ok(order) => order,
                    Err(_) => return,
                },
            };
            match order {
                Order::Play { id, sound, start } => {
                    self.reopens = 0;
                    self.playing = Some(Playing { id, sound: Arc::clone(&sound), start });
                    let before = self.openings;
                    if self.open.is_some() {
                        self.look(false);
                    }
                    if self.openings != before {
                        // Moved to another output, which started the sound itself.
                    } else if self.open.is_some() {
                        self.tell(ToMixer::Play { id, from: start * f64::from(sound.sample_rate), sound });
                    } else if !self.waiting() {
                        self.reopen();
                    }
                }
                Order::Seek { id, new_id, seconds } => {
                    if let Some(p) = self.playing.as_mut().filter(|p| p.id == id) {
                        p.id = new_id;
                        p.start = seconds;
                        let from = seconds * f64::from(p.sound.sample_rate);
                        self.tell(ToMixer::Seek { id, new_id, from });
                    } else {
                        // It ended just before the move came.
                        self.ended(new_id);
                    }
                }
                Order::Stop => {
                    self.playing = None;
                    self.tell(ToMixer::Stop);
                }
                Order::Choose(name) => {
                    self.wanted = name;
                    self.looked_at = None;
                    if self.open.is_some() {
                        self.reopen();
                    }
                }
                Order::Check => self.look(true),
                Order::List(reply) => {
                    let _ = reply.send(self.outputs.names());
                }
                Order::Mixer(FromMixer::Ended(id)) => self.ended(id),
                // Freed here, off the output's thread.
                Order::Mixer(FromMixer::Done(sound)) => drop(sound),
                Order::Lost { opening, why } => {
                    if self.open.as_ref().is_some_and(|o| o.opening == opening) {
                        self.close(Some(why));
                        if self.playing.is_some() && self.reopens < MAX_REOPENS {
                            self.reopens += 1;
                            self.reopen();
                        }
                    }
                }
                Order::Quit => return,
            }
        }
    }

    /// The sound `id` played to its end: if it's still the one playing, it's no longer, and the app is told.
    fn ended(&mut self, id: u64) {
        if self.playing.as_ref().is_some_and(|p| p.id == id) {
            self.playing = None;
        }
        let key = {
            let mut now = lock(&self.shared.now);
            match now.as_ref() {
                Some(n) if n.id == id => now.take().map(|n| n.key),
                _ => None,
            }
        };
        if let Some(key) = key {
            (self.on_end)(&key);
        }
    }

    /// Whether an output failed to open a moment ago, so a new sound shouldn't try yet.
    fn waiting(&self) -> bool {
        self.failed_at.is_some_and(|at| at.elapsed() < WAIT_AFTER_FAILING)
    }

    fn tell(&self, order: ToMixer) {
        if let Some(open) = &self.open {
            let _ = open.to_mixer.send(order);
        }
    }

    /// Looks whether the output should move: the chosen one is back, or the default changed. At most every few
    /// seconds unless `now`.
    fn look(&mut self, now: bool) {
        if !now && self.looked_at.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked_at = Some(Instant::now());
        let Some(open) = &self.open else {
            if now {
                self.failed_at = None;
                self.reopen();
            }
            return;
        };
        let should_move = match &self.wanted {
            // On the default while the chosen one was away: is it back?
            Some(wanted) => open.as_default && self.outputs.names().contains(wanted),
            // Following the default: has it moved? (Windows says so itself, as a lost output; Mac and Linux may not.)
            None => self.outputs.default_name().is_some_and(|d| d != open.output.name),
        };
        if should_move {
            self.reopen();
        }
    }

    fn close(&mut self, why: Option<String>) {
        if let Some(open) = self.open.take() {
            // Where the sound had got to, to carry on from there.
            if let Some(p) = self.playing.as_mut()
                && let Some(at) = self.place.of(p.id)
            {
                p.start = at;
            }
            drop(open);
        }
        *lock(&self.shared.delay) = None;
        let mut info = lock(&self.shared.info);
        *info = OutputInfo { problem: why, ..OutputInfo::default() };
    }

    /// Opens the output wanted (or the default when it isn't there), closing any open one first, and starts the sound
    /// playing again from where it was.
    fn reopen(&mut self) {
        self.close(None);
        self.openings += 1;
        let opening = self.openings;
        let mut problem = None;
        let opened = match self.wanted.clone() {
            Some(wanted) => match self.open_one(Some(&wanted), opening) {
                Ok(o) => Some((o, false)),
                Err(why) => {
                    problem = Some(format!("{why}, so sound goes to the default output"));
                    self.open_one(None, opening).ok().map(|o| (o, true))
                }
            },
            None => match self.open_one(None, opening) {
                Ok(o) => Some((o, true)),
                Err(why) => {
                    problem = Some(why);
                    None
                }
            },
        };
        let Some(((output, to_mixer), as_default)) = opened else {
            self.failed_at = Some(Instant::now());
            lock(&self.shared.info).problem = Some(problem.unwrap_or_else(|| "No sound output would open".into()));
            return;
        };
        self.failed_at = None;
        *lock(&self.shared.info) = OutputInfo {
            name: Some(output.name.clone()),
            sample_rate: output.sample_rate,
            channels: output.channels,
            delay_ms: None,
            problem,
        };
        *lock(&self.shared.delay) = Some(Arc::clone(&output.delay));
        self.open = Some(Open { output, to_mixer, opening, as_default });
        if let Some(p) = &self.playing {
            let from = p.start * f64::from(p.sound.sample_rate);
            self.tell(ToMixer::Play { id: p.id, sound: Arc::clone(&p.sound), from });
        }
    }

    /// Opens one output with a mixer of its own, and gives the way to that mixer.
    fn open_one(&mut self, name: Option<&str>, opening: u64) -> Result<(Opened, Sender<ToMixer>), String> {
        let (to_mixer, inbox) = channel();
        let to_player = self.orders.clone();
        let tell: mix::Tell = Box::new(move |m| {
            let _ = to_player.send(Order::Mixer(m));
        });
        let mut mixer = Mixer::new(inbox, tell, Arc::clone(&self.place));
        let render: Render = Box::new(move |out, rate, channels| mixer.render(out, rate, channels));
        let lost_to = self.orders.clone();
        let lost: Lost = Box::new(move |why| {
            let _ = lost_to.send(Order::Lost { opening, why });
        });
        Ok((self.outputs.open(name, render, lost)?, to_mixer))
    }
}

#[cfg(test)]
mod tests;
