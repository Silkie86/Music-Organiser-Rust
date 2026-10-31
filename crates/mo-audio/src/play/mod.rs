//! Playing one sound at a time, for auditioning: the C# app's `OpenAlPlayer`, rebuilt on cpal.
//!
//! Sound goes to the output chosen in Settings while it's there, otherwise the computer's default, following it when
//! it changes. When the output goes (unplugged, or a DAW takes the audio interface for itself) the player opens it
//! again and the sound carries on from where it was. Opening and reopening happen on the player's own thread, so the
//! window never waits for them; the output's thread only mixes.

mod mix;
mod output;

pub use output::{Event, Events, Opened, Outputs, Render, SystemOutputs};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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

/// Orders waiting for the player's thread. Fixed, so sending never takes memory on the output's thread; when it's full
/// the player's thread is stuck (a driver hanging), and an order is dropped rather than the window waiting.
const INBOX: usize = 1024;

/// Orders waiting for the output's thread, which takes them every few milliseconds.
const MIXER_INBOX: usize = 64;

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
    /// Where the open output's mixer says the sound is. Each output has its own, so two never write to one.
    place: Mutex<Option<Arc<Place>>>,
    quit: AtomicBool,
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
    Output { opening: u64, event: Event },
    Quit,
}

/// Plays one sound at a time. Every call returns at once; the work is done on the player's own thread.
pub struct Player {
    orders: SyncSender<Order>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
}

impl Player {
    /// A player on the computer's own outputs. Nothing opens until the first sound plays.
    ///
    /// `on_end` is called (on the player's thread) with a sound's key when it stops by itself: it played to its end,
    /// or its output went and wouldn't come back. Not when it's stopped or replaced.
    pub fn new(on_end: impl Fn(&str) + Send + 'static) -> Player {
        Player::with_outputs(Box::new(SystemOutputs::new()), on_end)
    }

    /// A player on the outputs given (tests give a stand-in).
    pub fn with_outputs(outputs: Box<dyn Outputs>, on_end: impl Fn(&str) + Send + 'static) -> Player {
        let (orders, inbox) = sync_channel(INBOX);
        let shared = Arc::new(Shared::default());
        let keeper = Keeper {
            outputs,
            inbox,
            orders: orders.clone(),
            shared: Arc::clone(&shared),
            on_end: Box::new(on_end),
            wanted: None,
            open: None,
            openings: 0,
            playing: None,
            reopens: 0,
            failed_at: None,
            looked_at: None,
        };
        // If the thread can't start, every order is dropped and nothing plays; nothing else breaks.
        let _ = std::thread::Builder::new().name("sound output".into()).spawn(move || keeper.run());
        Player { orders, shared, next_id: AtomicU64::new(1) }
    }

    fn send(&self, order: Order) {
        // Never waits: a full inbox means the player's thread is stuck, and then the order can't help anyway.
        let _ = self.orders.try_send(order);
    }

    /// Plays `sound` from `start_seconds`, replacing whatever is playing. `key` names it (its path, or a key for sound
    /// made in memory), as [`Player::playing`] gives back.
    pub fn play(&self, sound: Arc<AudioData>, key: impl Into<String>, start_seconds: f64) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let start = start_seconds.max(0.0);
        *lock(&self.shared.now) = Some(Now { id, key: key.into(), start });
        self.send(Order::Play { id, sound, start });
    }

    /// Plays on from `seconds` in the sound playing.
    pub fn seek(&self, seconds: f64) {
        let seconds = seconds.max(0.0);
        let mut now = lock(&self.shared.now);
        if let Some(now) = now.as_mut() {
            // A new id, so the place the output last gave (from before the move) isn't shown meanwhile.
            let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            self.send(Order::Seek { id: now.id, new_id, seconds });
            now.id = new_id;
            now.start = seconds;
        }
    }

    pub fn stop(&self) {
        *lock(&self.shared.now) = None;
        self.send(Order::Stop);
    }

    /// The key of the sound playing, or `None`.
    pub fn playing(&self) -> Option<String> {
        lock(&self.shared.now).as_ref().map(|n| n.key.clone())
    }

    /// Where the sound playing is, in seconds, or `None` when nothing is playing.
    pub fn position(&self) -> Option<f64> {
        let now = lock(&self.shared.now);
        let now = now.as_ref()?;
        let place = lock(&self.shared.place).as_ref().and_then(|p| p.of(now.id));
        Some(place.unwrap_or(now.start))
    }

    /// Plays through this output from now on (`None`: the computer's default), moving a sound that's playing. Before
    /// any sound has played it's only remembered, so the app's start opens nothing.
    pub fn choose(&self, name: Option<String>) {
        self.send(Order::Choose(name));
    }

    /// Looks at the outputs again, moving back to the chosen one if it has returned; [`Player::info`] then says where
    /// sound goes. Opens nothing while no sound is playing.
    pub fn check(&self) {
        self.send(Order::Check);
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
        if self.orders.try_send(Order::List(tx)).is_err() {
            return Vec::new();
        }
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The player's thread closes the output and ends by itself; nothing waits for it (it may be in a slow driver
        // call, and `on_end` may be what dropped the player).
        self.shared.quit.store(true, Ordering::Relaxed);
        let _ = self.orders.try_send(Order::Quit);
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
    to_mixer: SyncSender<ToMixer>,
    place: Arc<Place>,
    /// Which opening this is, so word about an older one is ignored.
    opening: u64,
    /// Whether it was opened as the default (following it) rather than by name.
    as_default: bool,
}

/// The player's own thread: opens, moves and reopens the output, and passes orders to the mixer.
struct Keeper {
    outputs: Box<dyn Outputs>,
    inbox: Receiver<Order>,
    orders: SyncSender<Order>,
    shared: Arc<Shared>,
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
        while !self.shared.quit.load(Ordering::Relaxed) {
            // While a sound waits for an output that wouldn't open, wake up to try again.
            let retry = self.playing.is_some() && self.open.is_none();
            let order = match self.failed_at.filter(|_| retry) {
                Some(at) => {
                    let wait = (at + WAIT_AFTER_FAILING).saturating_duration_since(Instant::now());
                    match self.inbox.recv_timeout(wait) {
                        Ok(order) => order,
                        Err(RecvTimeoutError::Timeout) => {
                            self.retry();
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
            self.obey(order);
        }
    }

    fn obey(&mut self, order: Order) {
        match order {
            Order::Play { id, sound, start } => {
                self.reopens = 0;
                self.playing = Some(Playing { id, sound: Arc::clone(&sound), start });
                if self.open.is_some() {
                    self.tell(ToMixer::Play { id, from: start * f64::from(sound.sample_rate), sound });
                    // After it has started, so a look that finds nothing to do (or can't move) doesn't hold it up.
                    self.look(false);
                } else if !self.waiting() {
                    self.open_fresh();
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
                let same = self.wanted == name && self.open.as_ref().is_some_and(|o| o.as_default == name.is_none());
                self.wanted = name;
                self.looked_at = None;
                if !same && self.open.is_some() {
                    self.move_to(self.wanted.clone());
                }
            }
            Order::Check => self.look(true),
            Order::List(reply) => {
                let _ = reply.send(self.outputs.names());
            }
            Order::Mixer(FromMixer::Ended(id)) => self.ended(id),
            // Freed here, off the output's thread.
            Order::Mixer(FromMixer::Done(sound)) => drop(sound),
            Order::Output { opening, event } => {
                if !self.open.as_ref().is_some_and(|o| o.opening == opening) {
                    return;
                }
                match event {
                    Event::Lost(why) => {
                        self.close(Some(why));
                        if self.playing.is_some() {
                            self.retry();
                        }
                    }
                    // The system moved the sound to the new default output itself: say where it goes now.
                    Event::Moved => {
                        if let Some(name) = self.outputs.default_name() {
                            lock(&self.shared.info).name = Some(name.clone());
                            if let Some(open) = self.open.as_mut() {
                                open.output.name = name;
                            }
                        }
                    }
                }
            }
            Order::Quit => {}
        }
    }

    /// The sound `id` stopped by itself: if it's still the one playing, it's no longer, and the app is told.
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

    /// Tries again to open an output for the sound playing, or gives the sound up once it has tried enough.
    fn retry(&mut self) {
        if self.reopens >= MAX_REOPENS {
            if let Some(id) = self.playing.as_ref().map(|p| p.id) {
                self.ended(id);
            }
            return;
        }
        self.reopens += 1;
        self.open_fresh();
    }

    /// Whether an output failed to open a moment ago, so a new sound shouldn't try yet.
    fn waiting(&self) -> bool {
        self.failed_at.is_some_and(|at| at.elapsed() < WAIT_AFTER_FAILING)
    }

    fn tell(&self, order: ToMixer) {
        if let Some(open) = &self.open {
            // Full only while the output has stopped asking for sound; it's then reopened, and starts the sound
            // playing afresh.
            let _ = open.to_mixer.try_send(order);
        }
    }

    /// Looks whether the output should move: the chosen one is back, or the default changed. At most every few
    /// seconds unless `now`. Opens nothing while no sound is playing.
    fn look(&mut self, now: bool) {
        if !now && self.looked_at.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked_at = Some(Instant::now());
        let Some(open) = &self.open else {
            if now && self.playing.is_some() && !self.waiting() {
                self.open_fresh();
            }
            return;
        };
        let target = match &self.wanted {
            // On the default while the chosen one was away: is it back?
            Some(wanted) if open.as_default && self.outputs.names().contains(wanted) => Some(Some(wanted.clone())),
            Some(_) => None,
            // Following the default: has it moved? (Windows says so itself; a Mac or Linux may not.)
            None => self.outputs.default_name().filter(|d| *d != open.output.name).map(|_| None),
        };
        if let Some(target) = target {
            self.move_to(target);
        }
    }

    /// Where the sound playing has got to, kept for starting it again on another output.
    fn note_place(&mut self) {
        let (Some(open), Some(p)) = (&self.open, self.playing.as_mut()) else { return };
        if let Some(at) = open.place.of(p.id) {
            p.start = at;
            if let Some(now) = lock(&self.shared.now).as_mut().filter(|n| n.id == p.id) {
                now.start = at;
            }
        }
    }

    fn close(&mut self, why: Option<String>) {
        self.note_place();
        self.open = None;
        *lock(&self.shared.delay) = None;
        *lock(&self.shared.place) = None;
        *lock(&self.shared.info) = OutputInfo { problem: why, ..OutputInfo::default() };
    }

    /// Moves to `target` (`None`: the default) while an output is open. The new one is opened before the old one
    /// closes, so if it won't open, sound carries on where it was.
    fn move_to(&mut self, target: Option<String>) {
        match self.open_one(target.as_deref()) {
            Ok(open) => {
                self.note_place();
                self.install(open, None);
            }
            Err(why) => {
                let here = self.open.as_ref().map_or_else(String::new, |o| o.output.name.clone());
                lock(&self.shared.info).problem = Some(format!("{why}, so sound stays on {here}"));
            }
        }
    }

    /// Opens the output wanted, or the default when it won't open, while none is open.
    fn open_fresh(&mut self) {
        self.close(None);
        let opened = match self.wanted.clone() {
            Some(wanted) => match self.open_one(Some(&wanted)) {
                Ok(o) => Ok((o, None)),
                Err(why) => match self.open_one(None) {
                    Ok(o) => Ok((o, Some(format!("{why}, so sound goes to the default output")))),
                    Err(also) => Err(format!("{why}, and the default output wouldn't open either: {also}")),
                },
            },
            None => self.open_one(None).map(|o| (o, None)),
        };
        match opened {
            Ok((open, problem)) => self.install(open, problem),
            Err(why) => {
                self.failed_at = Some(Instant::now());
                lock(&self.shared.info).problem = Some(why);
                if self.reopens >= MAX_REOPENS
                    && let Some(id) = self.playing.as_ref().map(|p| p.id)
                {
                    self.ended(id);
                }
            }
        }
    }

    /// Makes `open` the output (closing any other) and starts the sound playing on it from where it was.
    fn install(&mut self, open: Open, problem: Option<String>) {
        self.failed_at = None;
        *lock(&self.shared.info) = OutputInfo {
            name: Some(open.output.name.clone()),
            sample_rate: open.output.sample_rate,
            channels: open.output.channels,
            delay_ms: None,
            problem,
        };
        *lock(&self.shared.delay) = Some(Arc::clone(&open.output.delay));
        *lock(&self.shared.place) = Some(Arc::clone(&open.place));
        self.open = Some(open);
        if let Some(p) = &self.playing {
            let from = p.start * f64::from(p.sound.sample_rate);
            self.tell(ToMixer::Play { id: p.id, sound: Arc::clone(&p.sound), from });
        }
    }

    /// Opens one output (`None`: the default) with a mixer of its own.
    fn open_one(&mut self, name: Option<&str>) -> Result<Open, String> {
        self.openings += 1;
        let opening = self.openings;
        let (to_mixer, inbox) = sync_channel(MIXER_INBOX);
        let place = Arc::new(Place::default());
        let to_player = self.orders.clone();
        let tell: mix::Tell = Box::new(move |m| {
            // When the inbox is full (the player's thread stuck) a finished sound is freed here instead; rare.
            let _ = to_player.try_send(Order::Mixer(m));
        });
        let mut mixer = Mixer::new(inbox, tell, Arc::clone(&place));
        let render: Render = Box::new(move |out, rate, channels| mixer.render(out, rate, channels));
        let events_to = self.orders.clone();
        let events: Events = Box::new(move |event| {
            let _ = events_to.try_send(Order::Output { opening, event });
        });
        let output = self.outputs.open(name, render, events)?;
        Ok(Open { output, to_mixer, place, opening, as_default: name.is_none() })
    }
}

#[cfg(test)]
mod tests;
