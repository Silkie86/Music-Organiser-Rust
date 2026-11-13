//! Playing a sound through the track the plugin is on.
//!
//! The DAW's audio thread must never wait, read a file or make or free memory: a stall there is a click in the
//! song. So a sound is read and turned to the DAW's sample rate on a background thread at low priority
//! ([`Player::play`]), then handed to the audio thread ready to play through a small queue that never waits
//! ([`rtrb`]). When the audio thread is done with a sound, it hands it back through a second queue, and the panel's
//! thread frees it ([`Player::tidy`]).
//!
//! The newest pick wins: one reading thread at a time takes the newest pick waiting, so clicking down a list reads
//! only the sound you stop on (and the one being read when you started).
//!
//! A sound can also be made instead of read ([`Player::play_made`]: a MIDI part played with a SoundFont), on a thread
//! of its own so a slow one holds up no file; it loops until it's stopped or replaced, and it too plays only if it's
//! still the newest pick once it's made.

use crate::threads::lock;
use rtrb::{Consumer, Producer, RingBuffer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The longest stretch of a sound that is read to play: a long loop plays its first minute.
pub const MOST_SECONDS: f64 = 60.0;

/// How loud the sounds play: a little under full, as a preview.
const LEVEL: f32 = 0.8;

const NOT_STARTED: &str = "The DAW hasn't started this track's audio yet";

/// A sound ready to play: one list per side (a mono sound plays the same on both), at the DAW's sample rate.
pub struct Voice {
    left: Vec<f32>,
    right: Vec<f32>,
    at: usize,
    /// Where in the whole file this stretch begins and how much of the file it covers (0 to 1 each): for the line
    /// on the waveform that shows where it's playing.
    begins: f32,
    covers: f32,
    /// Starts again at its end, until it's stopped or replaced.
    looping: bool,
    /// A loop whatever the Loop switch says (a MIDI part played with a SoundFont): the switch doesn't change it.
    always: bool,
    /// How many beats long it is and the DAW's tempo it was made at, when it's made at it (a MIDI part, a loop
    /// matched to the tempo): it then plays in step with the DAW while the DAW plays at that tempo
    /// ([`Voice::follow`]).
    beats: Option<(f64, f64)>,
    /// Put in step with the DAW since the DAW last started playing.
    in_step: bool,
    /// Not yet looked at by the audio thread, with In time on: when it comes while the DAW plays, it waits for the
    /// next beat or bar ([`Voice::lock_to`]).
    fresh: bool,
    /// Frames of quiet still to go before it plays: the wait for the next beat or bar.
    wait: usize,
    /// A part sent out as notes, not heard: how many frames long it is (it has no sound of its own).
    quiet: Option<usize>,
    /// Its notes, sent out of the plugin as it plays (Send MIDI), in its frames, by their start.
    notes: Vec<NoteOut>,
}

/// A note of a part sent out as MIDI: where it starts and ends in the part's frames, its key, and how hard it's
/// played (0 to 1, from MIDI 2.0's 16-bit velocity: rule 22).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteOut {
    pub start: usize,
    pub end: usize,
    pub key: u8,
    pub velocity: f64,
}

/// A note going out in a block of sound: how far into the block, its key, how hard (0 to 1) and whether it starts or
/// ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub offset: u32,
    pub key: u8,
    pub velocity: f64,
    pub on: bool,
}

/// The most notes sent in one block (room made when the track's audio starts): a block is a few milliseconds.
const SENT_MOST: usize = 1024;

/// What a made loop is: a sound, or notes to send out with how long the part is, in seconds.
pub enum MadeLoop {
    Sound(mo_audio::AudioData),
    /// Each note's start and end in seconds, its key and how hard (0 to 1).
    Notes {
        seconds: f64,
        notes: Vec<(f64, f64, u8, f64)>,
    },
}

impl Voice {
    /// `channels` at `from_rate`, turned to `to_rate` (straight-line steps between samples: good enough to preview).
    pub fn new(channels: &[Vec<f32>], from_rate: u32, to_rate: u32) -> Voice {
        let left = channels.first().map(|c| resample(c, from_rate, to_rate)).unwrap_or_default();
        let right = match channels.get(1) {
            Some(c) => resample(c, from_rate, to_rate),
            None => left.clone(),
        };
        Voice {
            left,
            right,
            at: 0,
            begins: 0.0,
            covers: 1.0,
            looping: false,
            always: false,
            beats: None,
            in_step: false,
            fresh: true,
            wait: 0,
            quiet: None,
            notes: Vec::new(),
        }
    }

    /// A part `frames` long sent out as `notes` (in seconds, as [`MadeLoop::Notes`]) at `rate` frames a second, with
    /// no sound of its own: it plays as quiet while its notes go out.
    pub fn notes_only(frames: usize, notes: &[(f64, f64, u8, f64)], rate: u32) -> Voice {
        let at = |seconds: f64| ((seconds.max(0.0) * f64::from(rate)).round() as usize).min(frames);
        let mut notes: Vec<NoteOut> = notes
            .iter()
            .filter(|(start, end, key, _)| start.is_finite() && end.is_finite() && *key < 128)
            .map(|&(start, end, key, velocity)| {
                let start = at(start).min(frames.saturating_sub(1));
                // At least a frame long, so its end is never before its start.
                NoteOut { start, end: at(end).max(start + 1), key, velocity: velocity.clamp(0.0, 1.0) }
            })
            .collect();
        notes.sort_by_key(|n| n.start);
        let mut voice = Voice::new(&[], rate, rate);
        voice.quiet = Some(frames);
        voice.notes = notes;
        voice
    }

    /// How many frames long it is.
    fn len(&self) -> usize {
        self.quiet.unwrap_or_else(|| self.left.len().min(self.right.len()))
    }

    /// Plays round and round while the Loop switch is on (it can be turned off as it plays).
    fn looped(mut self) -> Voice {
        self.looping = true;
        self
    }

    /// Plays round and round, whatever the Loop switch says.
    fn always_looped(mut self) -> Voice {
        self.always = true;
        self.looped()
    }

    /// It's `beats` long, made at the DAW's tempo then (none when that isn't known, or it isn't made at the DAW's
    /// tempo).
    fn in_beats(mut self, beats: Option<(f64, f64)>) -> Voice {
        self.beats = beats.filter(|(b, t)| b.is_finite() && *b > 0.0 && t.is_finite() && *t > 0.0);
        self
    }

    /// While the DAW plays, at `song` beats into the song: a sound of known beats plays where the song is in it (a
    /// 4-bar part at bar 6 plays its second bar), put there when the DAW starts and again whenever the two drift
    /// apart by more than a few milliseconds (the DAW jumped, or looped back). Not once the DAW's tempo isn't the one
    /// it was made at (it would drift and be put back over and over): it then plays on as it is.
    fn follow(&mut self, song: Song) {
        let len = self.len();
        let Some((beats, made_at)) = self.beats else { return };
        if len == 0 || !song.beats.is_finite() || song.tempo.is_some_and(|t| (t / made_at - 1.0).abs() > 0.002) {
            return;
        }
        let want = ((song.beats.rem_euclid(beats) / beats * len as f64) as usize).min(len - 1);
        let gap = self.at.abs_diff(want);
        let apart = gap.min(len.saturating_sub(gap));
        if !self.in_step || apart > DRIFT {
            self.at = want;
            self.in_step = true;
        }
    }

    /// Come while the DAW plays at `song`: a sound of unknown beats waits for the song's next beat (a hit) or bar (a
    /// loop) so it starts in time, at `rate` frames a second. A sound of known beats is put in step instead
    /// ([`Voice::follow`]), and nothing waits while the DAW is stopped or doesn't say its tempo.
    fn lock_to(&mut self, song: Song, rate: u32) {
        self.wait = 0;
        let Some(tempo) = song.tempo.filter(|t| t.is_finite() && *t > 0.0) else { return };
        if self.beats.is_some() || !song.playing || !song.beats.is_finite() || rate == 0 {
            return;
        }
        let (start, step) = if self.looping {
            let (start, len) =
                song.bar.filter(|(s, l)| s.is_finite() && l.is_finite() && *l > 0.0).unwrap_or((0.0, 4.0));
            (start, len)
        } else {
            (0.0, 1.0)
        };
        // A hair past a beat counts as on it (the DAW's block began just after it): it starts straight away.
        let into = (song.beats - start).rem_euclid(step);
        let to_go = if into < 0.01 || step - into < 1e-9 { 0.0 } else { step - into };
        self.wait = (to_go * 60.0 / tempo * f64::from(rate)).round() as usize;
    }

    /// The Loop switch turned on or off while it plays: a sound plays on round, or to its end this time round.
    fn switch_loop(&mut self, on: bool) {
        if !self.always {
            self.looping = on;
        }
    }

    /// Starts `part` of the way in (0 the start, 1 the end) instead of at the start.
    fn from(mut self, part: f64) -> Voice {
        let len = self.len();
        self.at = ((part.clamp(0.0, 1.0) * len as f64) as usize).min(len);
        self
    }

    /// Says which stretch of the whole file this is: it begins `begins` of the way in and covers `covers` of it.
    fn of_file(mut self, begins: f64, covers: f64) -> Voice {
        self.begins = begins.clamp(0.0, 1.0) as f32;
        self.covers = covers.clamp(0.0, 1.0) as f32;
        self
    }

    /// Where it's playing now, as a part of the whole file (0 the start, 1 the end).
    fn place(&self) -> f32 {
        let len = self.len().max(1);
        self.begins + self.covers * (self.at.min(len) as f32 / len as f32)
    }

    /// Played to its end; never for a loop with something in it, which starts again instead ([`Voice::wrap`]).
    fn done(&self) -> bool {
        let len = self.len();
        !(self.looping && len > 0) && self.at >= len
    }

    /// At its end: a loop starts again.
    fn wrap(&mut self) {
        if self.looping && self.at >= self.len() {
            self.at = 0;
        }
    }
}

/// `samples` at `from` per second, as they'd be at `to` per second.
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let step = f64::from(from) / f64::from(to);
    let len = (samples.len() as f64 / step).floor() as usize;
    (0..len)
        .map(|i| {
            let at = i as f64 * step;
            let first = at.floor() as usize;
            let part = (at - first as f64) as f32;
            let a = samples.get(first).copied().unwrap_or(0.0);
            let b = samples.get(first + 1).copied().unwrap_or(a);
            a + (b - a) * part
        })
        .collect()
}

/// How far a sound in step with the DAW may drift from it before it's put back in step: 2048 frames (under 50 ms at
/// 44.1 kHz). The block the DAW hands over starts where the last ended, so in step it doesn't drift at all.
const DRIFT: usize = 2048;

/// The DAW's song, as its audio thread says it for a block of sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Song {
    /// The DAW is playing (its play head moving).
    pub playing: bool,
    /// Where its play head is at the block's start, in beats from the song's start.
    pub beats: f64,
    /// Its tempo, when it says.
    pub tempo: Option<f64>,
    /// Where the bar the play head is in starts, in beats, and how many beats a bar has, when it says (else bars of
    /// four from the song's start).
    pub bar: Option<(f64, f64)>,
}

/// What the panel asks the audio thread to do.
pub enum Ask {
    Play(Box<Voice>),
    Stop,
    /// The Loop switch, turned on or off.
    Loop(bool),
}

/// What the audio thread counts for the panel (plain numbers only: nothing there waits).
#[derive(Default)]
struct Counts {
    /// Sounds it has started.
    started: AtomicU64,
    /// Sounds that have ended: played to the end, stopped or replaced.
    ended: AtomicU64,
    /// Where the sound playing is in its whole file ([`Voice::place`]), as an `f32`'s bits.
    place: AtomicU32,
    /// How far the Volume knob is turned down (0 full, 1 silent), as an `f32`'s bits.
    down: AtomicU32,
}

/// The audio thread's side: on the DAW's audio thread, from when the DAW starts the track's audio to when it stops it.
pub struct Playing {
    asks: Consumer<Ask>,
    done: Producer<Box<Voice>>,
    now: Option<Box<Voice>>,
    /// A sound to hand back that didn't fit in the queue back: handed back first, before anything else is taken.
    /// (The queue is checked for room first, so this stays empty; it's here so nothing is ever freed on this thread.)
    waiting: Option<Box<Voice>>,
    counts: Arc<Counts>,
    /// The DAW was playing at the last block.
    was_playing: bool,
    /// The DAW's frames a second.
    rate: u32,
    /// The notes going out in this block (Send MIDI): room for [`SENT_MOST`], made when the track's audio starts.
    sent: Vec<Sent>,
    /// The keys whose notes are still on, so they can be ended when the part stops or jumps.
    sounding: [bool; 128],
    /// Where in the block a part stopping now stops: its notes end there.
    stops_at: u32,
}

impl Playing {
    /// Plays into `left` and `right` (the same length), taking any new asks first. Never waits, never makes or frees
    /// memory: a sound it's done with goes back to the panel's thread to be freed.
    pub fn fill(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.fill_in_song(left, right, None);
    }

    /// [`Playing::fill`], with where the DAW's song is (none when the DAW doesn't say). Starting the DAW puts a sound
    /// of known beats in step with it; stopping it stops what plays.
    pub fn fill_in_song(&mut self, left: &mut [f32], right: &mut [f32], song: Option<Song>) {
        self.sent.clear();
        self.stops_at = 0;
        if let Some(back) = self.waiting.take()
            && let Err(rtrb::PushError::Full(back)) = self.done.push(back)
        {
            self.waiting = Some(back);
        }
        if let Some(song) = song {
            // The DAW stopped: so does the sound (handed back as one that ended), before any new pick is taken, so a
            // pick made as the DAW stops plays. Only with room to hand it back: else at the next block.
            if self.was_playing && !song.playing && self.waiting.is_none() {
                self.retire(None);
                self.was_playing = false;
            }
        }
        // An ask is taken only while there's room to hand back the sound it replaces.
        while self.waiting.is_none() && !self.done.is_full() {
            match self.asks.pop() {
                Ok(Ask::Play(voice)) => {
                    self.counts.started.fetch_add(1, Ordering::AcqRel);
                    self.retire(Some(voice));
                }
                Ok(Ask::Stop) => self.retire(None),
                Ok(Ask::Loop(on)) => {
                    if let Some(voice) = &mut self.now {
                        voice.switch_loop(on);
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(song) = song {
            if let Some(voice) = &mut self.now {
                if std::mem::take(&mut voice.fresh) {
                    voice.lock_to(song, self.rate);
                }
                if song.playing {
                    let was = voice.at;
                    voice.follow(song);
                    // Put somewhere else: the notes on end, and those from there start as it plays.
                    if voice.at != was && !voice.notes.is_empty() {
                        all_off(&mut self.sounding, &mut self.sent, 0);
                    }
                } else {
                    // Put in step again when the DAW next starts.
                    voice.in_step = false;
                }
            }
            // A stop not yet handled (no room to hand the sound back) stays to be handled.
            self.was_playing = song.playing || self.was_playing;
        }
        let mut written = 0;
        let level = LEVEL * (1.0 - f32::from_bits(self.counts.down.load(Ordering::Acquire))).clamp(0.0, 1.0);
        let last = u32::try_from(left.len().saturating_sub(1)).unwrap_or(u32::MAX);
        let Playing { now, sent, sounding, counts, .. } = self;
        if let Some(voice) = now {
            // Without a song, or once looked at, a new sound plays straight away.
            voice.fresh = false;
            // The stretch of the part played without a jump (it starts again at its end): where it began in the part
            // and in the block.
            let mut stretch: Option<(usize, usize)> = None;
            for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                if voice.wait > 0 {
                    voice.wait -= 1;
                    *l = 0.0;
                    *r = 0.0;
                    written += 1;
                    continue;
                }
                let before = voice.at;
                voice.wrap();
                if voice.at != before
                    && let Some((from, offset)) = stretch.take()
                {
                    notes_in(&voice.notes, (from, before), offset, last, sent, sounding);
                }
                if voice.done() {
                    break;
                }
                stretch.get_or_insert((voice.at, i));
                (*l, *r) = match voice.quiet {
                    Some(_) => (0.0, 0.0),
                    None => (voice.left[voice.at] * level, voice.right[voice.at] * level),
                };
                voice.at += 1;
                written += 1;
            }
            if let Some((from, offset)) = stretch {
                notes_in(&voice.notes, (from, voice.at), offset, last, sent, sounding);
            }
            counts.place.store(voice.place().to_bits(), Ordering::Release);
        }
        left[written..].fill(0.0);
        right[written..].fill(0.0);
        self.stops_at = u32::try_from(written).unwrap_or(u32::MAX).min(last);
        if self.now.as_ref().is_some_and(|v| v.done()) && self.waiting.is_none() {
            self.retire(None);
        }
        // In the order they happen; an end before a start at the same spot (a key played again).
        self.sent.sort_unstable_by_key(|n| (n.offset, n.on));
    }

    /// The notes going out in the block just filled (Send MIDI), in order.
    pub fn sent(&self) -> &[Sent] {
        &self.sent
    }

    /// The Loop switch turned from the DAW (its knob), on the audio thread: as [`Player::set_loop`], without the
    /// queue.
    pub fn switch_loop(&mut self, on: bool) {
        if let Some(voice) = &mut self.now {
            voice.switch_loop(on);
        }
    }

    /// Hands the sound playing now back to the panel's thread and plays `next` instead.
    fn retire(&mut self, next: Option<Box<Voice>>) {
        // Its notes still on end where it stops.
        all_off(&mut self.sounding, &mut self.sent, self.stops_at);
        if let Some(old) = std::mem::replace(&mut self.now, next) {
            self.counts.ended.fetch_add(1, Ordering::AcqRel);
            if let Err(rtrb::PushError::Full(old)) = self.done.push(old) {
                self.waiting = Some(old);
            }
        }
    }
}

/// Sends `note` unless the block's room for notes is full (it never makes memory on the audio thread).
fn send(sent: &mut Vec<Sent>, note: Sent) {
    if sent.len() < sent.capacity() {
        sent.push(note);
    }
}

/// Ends every note still on, at `offset` into the block.
fn all_off(sounding: &mut [bool; 128], sent: &mut Vec<Sent>, offset: u32) {
    for (key, on) in sounding.iter_mut().enumerate() {
        if std::mem::take(on) {
            send(sent, Sent { offset, key: key as u8, velocity: 0.0, on: false });
        }
    }
}

/// The part played from frame `from` to just before `to` (`span`), starting `offset` into the block: the notes that
/// start and end in it are sent, at their places in the block (never past its `last` frame).
fn notes_in(
    notes: &[NoteOut],
    span: (usize, usize),
    offset: usize,
    last: u32,
    sent: &mut Vec<Sent>,
    sounding: &mut [bool; 128],
) {
    let (from, to) = span;
    let place = |at: usize| u32::try_from(offset + (at - from)).unwrap_or(u32::MAX).min(last);
    for note in notes {
        if note.start >= to {
            // Sorted by start: none after it starts or ends here (a long note that ends here started before it).
            break;
        }
        let key = usize::from(note.key);
        if (from..to).contains(&note.start) {
            if sounding[key] {
                send(sent, Sent { offset: place(note.start), key: note.key, velocity: 0.0, on: false });
            }
            send(sent, Sent { offset: place(note.start), key: note.key, velocity: note.velocity, on: true });
            sounding[key] = true;
        }
        if note.end > from && note.end <= to {
            send(sent, Sent { offset: place(note.end.min(to - 1)), key: note.key, velocity: 0.0, on: false });
            sounding[key] = false;
        }
    }
}

/// Where a pick's spot is, next to what was read of its file.
#[derive(Debug, PartialEq)]
enum Start {
    /// In what was read: this part of the way through it.
    Within(f64),
    /// Past it: this many frames into the file.
    Past(u64),
}

/// Where `from` of the way into the whole file (`file_seconds` long, at `rate`) is, when its first `read` frames were
/// read.
fn start(from: f32, read: usize, file_seconds: f64, rate: u32) -> Start {
    // A file that doesn't say how long it is: what was read is taken as all of it.
    let whole = (file_seconds * f64::from(rate)).max(read as f64);
    let frame = f64::from(from.clamp(0.0, 1.0)) * whole;
    // Read whole (within a hundredth: a file's own count of its length can be a little out), or the spot is in what
    // was read.
    if read == 0 {
        Start::Within(0.0)
    } else if read as f64 >= whole * 0.99 || frame < read as f64 {
        Start::Within((frame / read as f64).min(1.0))
    } else {
        Start::Past(frame as u64)
    }
}

/// What's to be read next.
struct Pick {
    path: PathBuf,
    /// Where in the whole sound to start: 0 the start, 1 the end.
    from: f32,
    /// The sound's own tempo, as the library knows it: with Loop and Match tempo on, a loop plays at the DAW's.
    tempo: Own,
    number: u64,
    told: Box<dyn FnOnce(Played) + Send>,
}

/// A picked sound's own tempo, as the library knows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Own {
    /// Not a loop (a kick, a snare): it plays at its own speed, with nothing said.
    NotALoop,
    /// A loop whose tempo isn't known: at its own speed, and the line says why.
    Unknown,
    /// A loop at this many BPM.
    Bpm(f64),
}

/// Makes a sound to play, on the reading thread: why not, in plain words.
/// It's the sound and, when it's made at the DAW's tempo, how many beats long it is and that tempo (it then plays in
/// step with the DAW).
pub type Made = Box<dyn FnOnce() -> Result<(MadeLoop, Option<(f64, f64)>), String> + Send>;

/// What became of a pick, for the panel's line under the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Played {
    /// Playing the sound named: the `n`th handed to the audio thread ([`Player::still_playing`]).
    Playing(String, u64),
    /// Couldn't play it, and why, in plain words.
    Not(String),
}

/// The panel's side, shared by everything in the plugin that isn't the audio thread.
#[derive(Default)]
pub struct Player {
    asks: Mutex<Option<Producer<Ask>>>,
    /// The waveform of the sound last read whole to be played, with its path: the panel takes it.
    pub heard: Mutex<Option<(PathBuf, mo_sounds::Peaks)>>,
    done: Mutex<Option<Consumer<Box<Voice>>>>,
    /// The DAW's sample rate while the track's audio runs; 0 when it doesn't.
    rate: AtomicU32,
    /// The newest pick still to be read: picks made while a sound is read replace each other here.
    next: Mutex<Option<Pick>>,
    /// Whether the reading thread is running.
    reading: AtomicBool,
    /// Counts picks and stops, so a sound read for a pick that's no longer the newest isn't played.
    picks: AtomicU64,
    /// How many sounds were handed to the audio thread.
    handed: AtomicU64,
    counts: Arc<Counts>,
    /// Trim silence: the level (dB below full scale, `mo_audio::trim::LEVELS_DB`) under which a sound's start and end
    /// count as silence and aren't played; 0 while it's off.
    trim_db: AtomicI32,
    /// The DAW's tempo, as an `f64`'s bits, as its audio thread last said; 0 before it has.
    tempo: AtomicU64,
    /// The Loop switch: a sound clicked plays round and round until it's stopped (or another is clicked).
    looping: AtomicBool,
    /// Match tempo: while looping, a loop whose tempo is known plays at the DAW's, its pitch kept.
    matching: AtomicBool,
    /// In time turned off: a sound clicked while the song plays starts straight away, not on the next beat or bar.
    free: AtomicBool,
    /// Send MIDI: a MIDI part plays as notes out of the plugin, not with the SoundFont.
    midi_out: AtomicBool,
}

impl Player {
    /// Turns Trim silence on at `db` below full scale (one of `mo_audio::trim::LEVELS_DB`), or off (`None`). It counts
    /// from the next sound read.
    pub fn set_trim(&self, db: Option<i32>) {
        self.trim_db.store(db.unwrap_or(0), Ordering::Release);
    }

    /// Trim silence's level in dB below full scale, while it's on.
    pub fn trim(&self) -> Option<i32> {
        match self.trim_db.load(Ordering::Acquire) {
            0 => None,
            db => Some(db),
        }
    }

    /// Trim silence's level as a sample's size, while it's on.
    fn trim_level(&self) -> Option<f32> {
        self.trim().map(mo_audio::trim::level_of_db)
    }

    /// Turns the Loop switch on or off: from the next sound played, and for the one playing now.
    pub fn set_loop(&self, on: bool) {
        self.looping.store(on, Ordering::Release);
        // Nothing playing (the track's audio not started) is nothing to change.
        let _ = self.ask(Ask::Loop(on));
    }

    /// The Loop switch's setting only, for the one playing changed on the audio thread ([`Playing::switch_loop`]).
    pub fn note_loop(&self, on: bool) {
        self.looping.store(on, Ordering::Release);
    }

    /// Turns the sounds up or down: 1 as loud as they play, 0 silent (the Volume knob). From the next block.
    pub fn set_volume(&self, volume: f64) {
        let down = (1.0 - volume.clamp(0.0, 1.0)) as f32;
        self.counts.down.store(down.to_bits(), Ordering::Release);
    }

    /// Whether the Loop switch is on.
    pub fn looping(&self) -> bool {
        self.looping.load(Ordering::Acquire)
    }

    /// Turns Match tempo on or off: from the next sound played.
    pub fn set_matching(&self, on: bool) {
        self.matching.store(on, Ordering::Release);
    }

    /// Turns In time on or off: from the next sound played. On, a sound clicked while the song plays starts on its
    /// next beat (a hit) or bar (a loop).
    pub fn set_in_time(&self, on: bool) {
        self.free.store(!on, Ordering::Release);
    }

    /// Turns Send MIDI on or off: from the next part played.
    pub fn set_midi_out(&self, on: bool) {
        self.midi_out.store(on, Ordering::Release);
    }

    /// Whether Send MIDI is on.
    pub fn midi_out(&self) -> bool {
        self.midi_out.load(Ordering::Acquire)
    }

    /// Whether In time is on (it starts on).
    pub fn in_time(&self) -> bool {
        !self.free.load(Ordering::Acquire)
    }

    /// Whether Match tempo is on.
    pub fn matching(&self) -> bool {
        self.matching.load(Ordering::Acquire)
    }

    /// With Loop and Match tempo on, `audio` (a sound of tempo `own`) at the DAW's tempo, whether it was stretched,
    /// and the words the playing line adds ("at 128 BPM"); as it is, with why not, when a loop can't be (and nothing
    /// said for a sound that isn't a loop, or when it isn't asked for).
    fn matched(&self, audio: mo_audio::AudioData, own: Own) -> (mo_audio::AudioData, bool, String) {
        if !(self.looping() && self.matching()) {
            return (audio, false, String::new());
        }
        let bpm = match own {
            Own::NotALoop => return (audio, false, String::new()),
            Own::Unknown => return (audio, false, " (its tempo isn't known: at its own speed)".into()),
            Own::Bpm(bpm) => bpm,
        };
        let Some(tempo) = self.tempo() else { return (audio, false, " (the DAW hasn't said its tempo yet)".into()) };
        match mo_audio::stretch::to_tempo(&audio, bpm, tempo) {
            Ok(stretched) => (stretched, true, format!(" at {tempo:.0} BPM")),
            Err(why) => {
                crate::log::line("Play", &format!("Not stretched: {why}"));
                (audio, false, " (couldn't be stretched: at its own speed)".into())
            }
        }
    }

    /// The DAW's tempo now (the audio thread's side: a plain store, nothing waits).
    pub fn set_tempo(&self, bpm: f64) {
        // One a part can be played at (as the app takes a file's): a DAW's odd moment while its tempo is dragged
        // is left out.
        if bpm.is_finite() && (20.0..=400.0).contains(&bpm) {
            self.tempo.store(bpm.to_bits(), Ordering::Relaxed);
        }
    }

    /// The DAW's tempo, once its audio thread has said it.
    pub fn tempo(&self) -> Option<f64> {
        match self.tempo.load(Ordering::Relaxed) {
            0 => None,
            bits => Some(f64::from_bits(bits)),
        }
    }

    /// The DAW starts the track's audio at `rate` samples a second: the audio thread's side, to give the DAW.
    pub fn start(&self, rate: u32) -> Playing {
        let (asks_in, asks) = RingBuffer::new(4);
        let (done, done_out) = RingBuffer::new(8);
        // Anything still in the old queues is freed here, on the DAW's main thread.
        *lock(&self.asks) = Some(asks_in);
        *lock(&self.done) = Some(done_out);
        self.rate.store(rate, Ordering::Release);
        self.settle();
        Playing {
            asks,
            done,
            now: None,
            waiting: None,
            counts: Arc::clone(&self.counts),
            was_playing: false,
            rate,
            sent: Vec::with_capacity(SENT_MOST),
            sounding: [false; 128],
            stops_at: 0,
        }
    }

    /// The DAW stopped the track's audio (its side of the queues has gone with it, and what was playing).
    pub fn stop_all(&self) {
        self.rate.store(0, Ordering::Release);
        *lock(&self.asks) = None;
        self.settle();
    }

    /// Nothing is playing: every sound handed over counts as started and ended.
    fn settle(&self) {
        let handed = self.handed.load(Ordering::Acquire);
        self.counts.started.store(handed, Ordering::Release);
        self.counts.ended.store(handed, Ordering::Release);
    }

    /// Plays the sound at `path` from `from` of the way into the whole sound (0 the start, 1 the end), read on a
    /// background thread at low priority; `told` hears how it went, unless a newer pick came first.
    /// `tempo` is the sound's own, as the library knows it.
    pub fn play(self: &Arc<Self>, path: PathBuf, from: f32, tempo: Own, told: impl FnOnce(Played) + Send + 'static) {
        let number = self.picks.fetch_add(1, Ordering::AcqRel) + 1;
        let from = if from.is_finite() { from.clamp(0.0, 1.0) } else { 0.0 };
        *lock(&self.next) = Some(Pick { path, from, tempo, number, told: Box::new(told) });
        if self.reading.swap(true, Ordering::AcqRel) {
            // The reading thread is running: it takes this pick next.
            return;
        }
        let me = Arc::clone(self);
        crate::threads::background("mo-plugin-audition", move || {
            // Marked as stopped however the thread ends (a read that panics on a damaged file too), so the next pick
            // starts a new one.
            struct Stopped<'a>(&'a AtomicBool);
            impl Drop for Stopped<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _stopped = Stopped(&me.reading);
            loop {
                while let Some(pick) = take_next(&me) {
                    me.read(pick);
                }
                me.reading.store(false, Ordering::Release);
                // A pick made just as it stopped would find it running and leave the pick to it: so look once more,
                // and go on if there's one and no new thread has taken over.
                if lock(&me.next).is_none() || me.reading.swap(true, Ordering::AcqRel) {
                    break;
                }
            }
        });
    }

    /// Reads one pick and plays it, if it's still the newest.
    fn read(&self, pick: Pick) {
        if self.picks.load(Ordering::Acquire) != pick.number {
            return;
        }
        let name = pick.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let started = std::time::Instant::now();
        let read = mo_audio::read(&pick.path, MOST_SECONDS);
        if self.picks.load(Ordering::Acquire) != pick.number {
            return;
        }
        let rate = self.rate.load(Ordering::Acquire);
        let played = match read {
            Err(e) => Played::Not(format!("Couldn't read {name}: {e}")),
            Ok(_) if rate == 0 => Played::Not(NOT_STARTED.into()),
            Ok(audio) => {
                // The whole sound was read: its waveform, for a line the library has none for.
                let frames = audio.channels.first().map_or(0, Vec::len);
                if frames as f64 >= audio.file_seconds * f64::from(audio.sample_rate) * 0.99 {
                    let wave = crate::waves::peaks(&audio.channels, crate::waves::PEAKS);
                    *lock(&self.heard) = Some((pick.path.clone(), wave));
                }
                let (audio, stretched, how) = self.matched(audio, pick.tempo);
                // The stretch can take a moment: a newer pick made meanwhile wins.
                if self.picks.load(Ordering::Acquire) != pick.number {
                    return;
                }
                let name = format!("{name}{how}");
                // A loop matched to the DAW's tempo plays in step with it, when it plays whole (not trimmed).
                // Its length in beats, a whole number of them (else it would drift against the bars: then it doesn't
                // follow).
                let beats = match (stretched, self.tempo(), self.trim_level()) {
                    (true, Some(tempo), None) if audio.sample_rate > 0 => {
                        let frames = audio.channels.first().map_or(0, Vec::len);
                        let beats = frames as f64 / f64::from(audio.sample_rate) * tempo / 60.0;
                        ((beats - beats.round()).abs() < 0.02 && beats >= 1.0).then_some((beats, tempo))
                    }
                    _ => None,
                };
                match self.voice_from(&pick, audio, rate, stretched) {
                    Ok(voice) if self.looping() => self.hand_over(Box::new(voice.looped().in_beats(beats)), name),
                    Ok(voice) => self.hand_over(Box::new(voice), name),
                    Err(why) => Played::Not(why),
                }
            }
        };
        match &played {
            Played::Playing(..) => crate::log::took_if_slow("Play", &format!("Read {}", pick.path.display()), started),
            Played::Not(why) => crate::log::took("Play", &format!("{}: {why}", pick.path.display()), started),
        }
        (pick.told)(played);
    }

    /// Plays the sound `make` makes (named `name`) as a loop. It's made on a thread of its own at low priority, so a
    /// slow one holds up no other pick, and plays only if it's still the newest pick once it's made; `told` hears how
    /// it went, unless a newer pick came first.
    pub fn play_made(self: &Arc<Self>, name: String, make: Made, told: impl FnOnce(Played) + Send + 'static) {
        let number = self.picks.fetch_add(1, Ordering::AcqRel) + 1;
        // A file still to be read is no longer the newest pick.
        lock(&self.next).take();
        let me = Arc::clone(self);
        crate::threads::background("mo-plugin-make", move || {
            if let Some(played) = me.made(&name, make, number) {
                told(played);
            }
        });
    }

    /// Makes a sound to loop and plays it, if its pick is still the newest when it's made (none when it isn't).
    fn made(&self, name: &str, make: Made, number: u64) -> Option<Played> {
        let started = std::time::Instant::now();
        let made = make();
        if self.picks.load(Ordering::Acquire) != number {
            return None;
        }
        let rate = self.rate.load(Ordering::Acquire);
        let played = match made {
            Err(why) => Played::Not(why),
            Ok(_) if rate == 0 => Played::Not(NOT_STARTED.into()),
            Ok((made, beats)) => {
                let voice = match made {
                    MadeLoop::Sound(audio) => Voice::new(&audio.channels, audio.sample_rate, rate),
                    MadeLoop::Notes { seconds, notes } => {
                        let frames = (seconds.max(0.0) * f64::from(rate)).round() as usize;
                        Voice::notes_only(frames.max(1), &notes, rate)
                    }
                };
                self.hand_over(Box::new(voice.always_looped().in_beats(beats)), name.into())
            }
        };
        match &played {
            Played::Playing(..) => crate::log::took_if_slow("Play", &format!("Made {name}"), started),
            Played::Not(why) => crate::log::took("Play", &format!("{name}: {why}"), started),
        }
        Some(played)
    }

    /// The sound ready to play from the pick's spot. `audio` is the file read from its start; a spot past what was
    /// read (over a minute into a long file) is read from there instead, where the file's kind can be; not when it
    /// was `stretched` (the file's own frames aren't the stretched sound's).
    fn voice_from(
        &self,
        pick: &Pick,
        mut audio: mo_audio::AudioData,
        rate: u32,
        stretched: bool,
    ) -> Result<Voice, String> {
        let read = audio.channels.first().map_or(0, Vec::len);
        // The whole file's length in frames, as `start` takes it.
        let whole = (audio.file_seconds * f64::from(audio.sample_rate)).max(read as f64).max(1.0);
        match start(pick.from, read, audio.file_seconds, audio.sample_rate) {
            // Trim silence: the spot still counts into the whole sound; the play starts past the silence at the
            // start (or at the spot, when that's later) and ends with the sound.
            Start::Within(part) => match self
                .trim_level()
                .and_then(|level| mo_audio::trim::play_span(&audio, (part * read as f64).round() as usize, level))
            {
                Some((first, end)) => {
                    audio.keep(first, end);
                    let kept = end.saturating_sub(first) as f64;
                    Ok(Voice::new(&audio.channels, audio.sample_rate, rate).of_file(first as f64 / whole, kept / whole))
                }
                None => Ok(Voice::new(&audio.channels, audio.sample_rate, rate)
                    .of_file(0.0, read as f64 / whole)
                    .from(part)),
            },
            Start::Past(_) if stretched => Err("Only the first minute of a loop plays at the DAW's tempo".into()),
            Start::Past(_) if !mo_audio::can_read_from(&pick.path) => {
                Err("Only the first minute of this kind of file plays".into())
            }
            Start::Past(frame) => {
                drop(audio);
                let mut later = mo_audio::read_from(&pick.path, MOST_SECONDS, frame)
                    .map_err(|e| format!("Couldn't read from there: {e}"))?;
                // A spot picked past the first minute plays from there: only the silence at the end is cut.
                if let Some(level) = self.trim_level() {
                    later.cut_end_silence(level);
                }
                let frames = later.channels.first().map_or(0, Vec::len) as f64;
                Ok(Voice::new(&later.channels, later.sample_rate, rate).of_file(frame as f64 / whole, frames / whole))
            }
        }
    }

    fn hand_over(&self, mut voice: Box<Voice>, name: String) -> Played {
        // With In time off, it isn't held for the next beat.
        voice.fresh = self.in_time();
        match self.ask(Ask::Play(voice)) {
            Ok(()) => Played::Playing(name, self.handed.fetch_add(1, Ordering::AcqRel) + 1),
            Err(why) => Played::Not(why),
        }
    }

    /// Stops what's playing, and any sound still to be read.
    pub fn stop(&self) {
        self.picks.fetch_add(1, Ordering::AcqRel);
        lock(&self.next).take();
        let _ = self.ask(Ask::Stop);
    }

    /// Whether the `n`th sound handed to the audio thread ([`Played::Playing`]) may still be playing: it hasn't
    /// started yet, or it's the newest started and hasn't ended.
    pub fn still_playing(&self, n: u64) -> bool {
        let started = self.counts.started.load(Ordering::Acquire);
        started < n || (started == n && self.counts.ended.load(Ordering::Acquire) < started)
    }

    /// Where the `n`th sound handed to the audio thread is playing now, as a part of its whole file (0 the start, 1
    /// the end): `None` before it has started and once it has ended.
    pub fn place(&self, n: u64) -> Option<f32> {
        let started = self.counts.started.load(Ordering::Acquire);
        (started == n && self.counts.ended.load(Ordering::Acquire) < started)
            .then(|| f32::from_bits(self.counts.place.load(Ordering::Acquire)).clamp(0.0, 1.0))
    }

    fn ask(&self, ask: Ask) -> Result<(), String> {
        match lock(&self.asks).as_mut().map(|asks| asks.push(ask)) {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("Busy: try again".into()),
            None => Err(NOT_STARTED.into()),
        }
    }

    /// Frees the sounds the audio thread is done with. Called often from the panel's thread.
    pub fn tidy(&self) {
        if let Some(done) = lock(&self.done).as_mut() {
            while done.pop().is_ok() {}
        }
    }
}

fn take_next(player: &Player) -> Option<Pick> {
    lock(&player.next).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(len: usize) -> Box<Voice> {
        Box::new(Voice::new(&[vec![1.0; len]], 48_000, 48_000))
    }

    #[test]
    fn resample_halves_and_keeps_the_same_rate() {
        let s = [0.0, 1.0, 2.0, 3.0];
        assert_eq!(resample(&s, 48_000, 48_000), s.to_vec());
        assert_eq!(resample(&s, 48_000, 24_000), vec![0.0, 2.0]);
        assert_eq!(resample(&s, 24_000, 48_000), vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.0]);
    }

    #[test]
    fn a_sound_plays_from_the_spot_asked_for() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let from_half = Voice::new(&[vec![0.1, 0.2, 0.3, 0.4]], 48_000, 48_000).from(0.5);
        player.hand_over(Box::new(from_half), "a".into());
        let (mut l, mut r) = (vec![9.0; 3], vec![9.0; 3]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.3 * LEVEL, 0.4 * LEVEL, 0.0]);
        // Played to its end and handed back: no place to show any more.
        assert_eq!(player.place(1), None);
        // The very end plays nothing, and doesn't run past the sound.
        assert!(Voice::new(&[vec![0.1; 4]], 48_000, 48_000).from(1.0).done());
    }

    fn pick(from: f32) -> Pick {
        Pick { path: PathBuf::new(), from, tempo: Own::NotALoop, number: 1, told: Box::new(|_| {}) }
    }

    #[test]
    fn trim_silence_skips_the_quiet_start_and_end_and_a_spot_still_counts_into_the_whole_sound() {
        let mut s = vec![0.0; 100];
        s[20..50].fill(0.5);
        let audio =
            mo_audio::AudioData { sample_rate: 1_000, channels: vec![s], file_seconds: 0.1, ..Default::default() };
        let player = Player::default();
        // Off: the whole sound.
        assert_eq!(player.voice_from(&pick(0.0), audio.clone(), 1_000, false).unwrap().left.len(), 100);
        player.set_trim(Some(-60));
        // From the start: from the first loud frame to the end of the sound and its 10 ms tail.
        let v = player.voice_from(&pick(0.0), audio.clone(), 1_000, false).unwrap();
        assert_eq!((v.left.len(), v.left[0]), (40, 0.5));
        // Its place on the waveform counts into the whole file: it starts a fifth of the way in.
        assert!((v.place() - 0.2).abs() < 1e-6 && (v.covers - 0.4).abs() < 1e-6);
        // A spot in the sound plays from there; one in the quiet end plays as asked.
        assert_eq!(player.voice_from(&pick(0.3), audio.clone(), 1_000, false).unwrap().left.len(), 30);
        let v = player.voice_from(&pick(0.8), audio.clone(), 1_000, false).unwrap();
        assert_eq!((v.left.len(), v.at), (100, 80));
        player.set_trim(None);
        assert_eq!(player.voice_from(&pick(0.0), audio, 1_000, false).unwrap().left.len(), 100);
    }

    #[test]
    fn a_spot_is_counted_into_the_whole_file() {
        // All 100 frames were read: half way is half way.
        assert_eq!(start(0.5, 100, 1.0, 100), Start::Within(0.5));
        assert_eq!(start(0.0, 100, 1.0, 100), Start::Within(0.0));
        // The first 100 of 400 were read: an eighth of the way in is half way through them, and three quarters of
        // the way in is past them, at frame 300.
        assert_eq!(start(0.125, 100, 4.0, 100), Start::Within(0.5));
        assert_eq!(start(0.75, 100, 4.0, 100), Start::Past(300));
        // The end of a file read whole is its end, not a spot past it.
        assert_eq!(start(1.0, 100, 1.0, 100), Start::Within(1.0));
        // A file that doesn't say how long it is, and an empty one.
        assert_eq!(start(0.5, 100, 0.0, 100), Start::Within(0.5));
        assert_eq!(start(0.5, 0, 0.0, 100), Start::Within(0.0));
    }

    #[test]
    fn the_place_playing_is_told_as_a_part_of_the_whole_file() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        // The second half of a file, played from its own start.
        let voice = Voice::new(&[vec![0.1; 8]], 48_000, 48_000).of_file(0.5, 0.5);
        let Played::Playing(_, n) = player.hand_over(Box::new(voice), "a".into()) else { panic!("not handed over") };
        assert_eq!(player.place(n), None, "not started yet");
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        playing.fill(&mut l, &mut r);
        assert_eq!(player.place(n), Some(0.75));
        assert_eq!(player.place(n + 1), None);
        // A spot clicked in a file read whole starts the line there.
        let audio = mo_audio::AudioData {
            sample_rate: 1_000,
            channels: vec![vec![0.5; 100]],
            file_seconds: 0.1,
            ..Default::default()
        };
        assert!((player.voice_from(&pick(0.3), audio, 1_000, false).unwrap().place() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_mono_sound_plays_on_both_sides() {
        let v = Voice::new(&[vec![0.5, 0.25]], 44_100, 44_100);
        assert_eq!(v.left, v.right);
    }

    #[test]
    fn plays_then_hands_the_sound_back_and_goes_quiet() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let Played::Playing(_, n) = player.hand_over(voice(3), "a".into()) else { panic!("not handed over") };
        assert!(player.still_playing(n), "handed over, not started yet");
        let (mut l, mut r) = (vec![9.0; 4], vec![9.0; 4]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![LEVEL, LEVEL, LEVEL, 0.0]);
        assert_eq!(r, l);
        // Done: it went back to be freed off the audio thread, and the panel's line can go.
        assert!(playing.now.is_none());
        assert!(!player.still_playing(n));
        assert_eq!(lock(&player.done).as_ref().unwrap().slots(), 1);
        player.tidy();
        assert_eq!(lock(&player.done).as_ref().unwrap().slots(), 0);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 4]);
    }

    #[test]
    fn the_loop_switch_turned_off_lets_a_sound_end_but_never_a_midi_part() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        player.set_loop(true);
        assert!(player.looping());
        let sound = Voice::new(&[vec![0.1, 0.2]], 48_000, 48_000).looped();
        player.hand_over(Box::new(sound), "a".into());
        let (mut l, mut r) = (vec![9.0; 3], vec![9.0; 3]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, [0.1, 0.2, 0.1].map(|x| x * LEVEL).to_vec());
        // Turned off as it plays: it plays to its end this time round, and stops.
        player.set_loop(false);
        let (mut l, mut r) = (vec![9.0; 3], vec![9.0; 3]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.2 * LEVEL, 0.0, 0.0]);
        assert!(playing.now.is_none());
        // A MIDI part loops whatever the switch says.
        let part = Voice::new(&[vec![0.3]], 48_000, 48_000).always_looped();
        player.hand_over(Box::new(part), "b".into());
        player.set_loop(false);
        let (mut l, mut r) = (vec![9.0; 3], vec![9.0; 3]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.3 * LEVEL; 3]);
    }

    #[test]
    fn match_tempo_stretches_a_loop_only_when_asked_and_the_tempos_are_known() {
        let player = Player::default();
        let second = mo_audio::AudioData {
            sample_rate: 48_000,
            channels: vec![(0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.2).collect()],
            file_seconds: 1.0,
            embedded_tempo: None,
            embedded_root_note: None,
            embedded_is_loop: Some(true),
        };
        let len = |a: &mo_audio::AudioData| a.channels[0].len();
        // Off, or the loop switch off: played as it is, with nothing said.
        let (same, _, said) = player.matched(second.clone(), Own::Bpm(120.0));
        assert_eq!((len(&same), said.as_str()), (48_000, ""));
        player.set_loop(true);
        player.set_matching(true);
        // On, but a tempo missing: at its own speed, and why.
        let (same, _, said) = player.matched(second.clone(), Own::Unknown);
        assert_eq!(len(&same), 48_000);
        assert!(said.contains("isn't known"), "{said}");
        let (same, _, said) = player.matched(second.clone(), Own::Bpm(120.0));
        assert_eq!(len(&same), 48_000);
        assert!(said.contains("hasn't said"), "{said}");
        // Both known: a 120 BPM loop at the DAW's 100 is a fifth longer.
        player.set_tempo(100.0);
        let (same, stretched, said) = player.matched(second.clone(), Own::NotALoop);
        assert_eq!((len(&same), stretched, said.as_str()), (48_000, false, ""), "a kick plays as it is, nothing said");
        let (slower, stretched, said) = player.matched(second, Own::Bpm(120.0));
        assert_eq!(said, " at 100 BPM");
        assert!(stretched);
        let want = 48_000.0 * 1.2;
        assert!((len(&slower) as f64 - want).abs() / want < 0.02, "{} frames", len(&slower));
    }

    #[test]
    fn a_part_plays_in_step_with_the_daw_and_stops_when_it_stops() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        // Four beats over eight frames: two frames a beat.
        let samples: Vec<f32> = (0..8).map(|i| i as f32 / 10.0).collect();
        let part = Voice::new(&[samples], 48_000, 48_000).always_looped().in_beats(Some((4.0, 120.0)));
        player.hand_over(Box::new(part), "a".into());
        let (mut l, mut r) = (vec![9.0; 2], vec![9.0; 2]);
        let stopped = Some(Song { playing: false, beats: 0.0, tempo: Some(120.0), bar: None });
        // The DAW stopped: it plays from where it was clicked, on its own.
        playing.fill_in_song(&mut l, &mut r, stopped);
        assert_eq!(l, [0.0, 0.1].map(|x| x * LEVEL).to_vec());
        // The DAW starts at beat 5 (bar 2's second beat): the part jumps to its second beat, frame 2.
        playing.fill_in_song(&mut l, &mut r, Some(Song { playing: true, beats: 5.0, tempo: Some(120.0), bar: None }));
        assert_eq!(l, [0.2, 0.3].map(|x| x * LEVEL).to_vec());
        // In step, it plays on as it is.
        playing.fill_in_song(&mut l, &mut r, Some(Song { playing: true, beats: 6.0, tempo: Some(120.0), bar: None }));
        assert_eq!(l, [0.4, 0.5].map(|x| x * LEVEL).to_vec());
        // The DAW stops: so does the part.
        playing.fill_in_song(&mut l, &mut r, stopped);
        assert_eq!(l, vec![0.0; 2]);
        assert!(playing.now.is_none());
        // A sound of unknown beats isn't moved (clicked on a bar's start, it doesn't wait), but stops with the DAW too.
        let sound = Voice::new(&[vec![0.1, 0.2, 0.3]], 48_000, 48_000).looped();
        player.hand_over(Box::new(sound), "b".into());
        let (mut l, mut r) = (vec![9.0; 1], vec![9.0; 1]);
        playing.fill_in_song(&mut l, &mut r, Some(Song { playing: true, beats: 4.0, tempo: Some(120.0), bar: None }));
        assert_eq!(l, vec![0.1 * LEVEL]);
        playing.fill_in_song(&mut l, &mut r, stopped);
        assert!(playing.now.is_none());
        // Made at 120 BPM, the DAW now at 124: it isn't jumped about, it plays on as it is (and still stops).
        let part = Voice::new(&[vec![0.1, 0.2, 0.3, 0.4]], 48_000, 48_000).always_looped().in_beats(Some((4.0, 120.0)));
        player.hand_over(Box::new(part), "c".into());
        playing.fill_in_song(&mut l, &mut r, Some(Song { playing: true, beats: 3.0, tempo: Some(124.0), bar: None }));
        assert_eq!(l, vec![0.1 * LEVEL]);
        playing.fill_in_song(&mut l, &mut r, Some(Song { playing: false, beats: 3.0, tempo: Some(124.0), bar: None }));
        assert!(playing.now.is_none());
    }

    #[test]
    fn a_sound_clicked_while_the_daw_plays_waits_for_the_next_beat_or_bar() {
        let player = Player::default();
        // Four frames a second at 120 BPM: two frames a beat, so the waits are short to check.
        let mut playing = player.start(4);
        let song = |beats: f64, bar: Option<(f64, f64)>| Some(Song { playing: true, beats, tempo: Some(120.0), bar });
        let (mut l, mut r) = (vec![9.0; 4], vec![9.0; 4]);
        // A hit clicked half a beat after beat 5: one frame of quiet, then it plays.
        player.hand_over(Box::new(Voice::new(&[vec![0.1, 0.2, 0.3]], 4, 4)), "hit".into());
        playing.fill_in_song(&mut l, &mut r, song(5.5, None));
        assert_eq!(l, [0.0, 0.1, 0.2, 0.3].map(|x| x * LEVEL).to_vec());
        // A loop clicked at beat 5 of bars of four: three beats (six frames) of quiet to bar 3, over two blocks.
        player.hand_over(Box::new(Voice::new(&[vec![0.5, 0.6]], 4, 4).looped()), "loop".into());
        playing.fill_in_song(&mut l, &mut r, song(5.0, None));
        assert_eq!(l, vec![0.0; 4]);
        playing.fill_in_song(&mut l, &mut r, song(7.0, None));
        assert_eq!(l, [0.0, 0.0, 0.5, 0.6].map(|x| x * LEVEL).to_vec());
        // In 3/4 with the bar begun at beat 3: clicked at beat 5, it waits one beat (two frames) for beat 6.
        player.hand_over(Box::new(Voice::new(&[vec![0.5, 0.6]], 4, 4).looped()), "waltz".into());
        playing.fill_in_song(&mut l, &mut r, song(5.0, Some((3.0, 3.0))));
        assert_eq!(l, [0.0, 0.0, 0.5, 0.6].map(|x| x * LEVEL).to_vec());
        // Right on a beat, a hit plays straight away; so does one clicked while the DAW is stopped.
        player.hand_over(Box::new(Voice::new(&[vec![0.1, 0.2, 0.3, 0.4]], 4, 4)), "on".into());
        playing.fill_in_song(&mut l, &mut r, song(8.0, None));
        assert_eq!(l, [0.1, 0.2, 0.3, 0.4].map(|x| x * LEVEL).to_vec());
        let stopped = Some(Song { playing: false, beats: 8.5, tempo: Some(120.0), bar: None });
        playing.fill_in_song(&mut l, &mut r, stopped);
        player.hand_over(Box::new(Voice::new(&[vec![0.1, 0.2, 0.3, 0.4]], 4, 4)), "stopped".into());
        playing.fill_in_song(&mut l, &mut r, stopped);
        assert_eq!(l, [0.1, 0.2, 0.3, 0.4].map(|x| x * LEVEL).to_vec());
        // In time off: a hit clicked between beats plays straight away.
        player.set_in_time(false);
        player.hand_over(Box::new(Voice::new(&[vec![0.1, 0.2, 0.3, 0.4]], 4, 4)), "free".into());
        playing.fill_in_song(&mut l, &mut r, song(8.5, None));
        assert_eq!(l, [0.1, 0.2, 0.3, 0.4].map(|x| x * LEVEL).to_vec());
        player.set_in_time(true);
        // The DAW stops during the wait: the sound stops too, never heard.
        player.hand_over(Box::new(Voice::new(&[vec![0.5, 0.6]], 4, 4).looped()), "late".into());
        playing.fill_in_song(&mut l, &mut r, Some(Song { playing: true, beats: 9.0, tempo: Some(120.0), bar: None }));
        assert_eq!(l, vec![0.0; 4]);
        playing.fill_in_song(&mut l, &mut r, stopped);
        assert!(playing.now.is_none());
        assert_eq!(l, vec![0.0; 4]);
    }

    #[test]
    fn a_part_sent_as_midi_sends_its_notes_in_time_and_ends_them() {
        let player = Player::default();
        // Ten frames a second: a part of one second, a note at 0.2 s to 0.5 s and one at 0.8 s to the end.
        let mut playing = player.start(10);
        let notes = [(0.2, 0.5, 60, 1.0), (0.8, 1.0, 64, 0.5)];
        let part = Voice::notes_only(10, &notes, 10).always_looped();
        player.hand_over(Box::new(part), "part".into());
        let on = |offset, key, velocity| Sent { offset, key, velocity, on: true };
        let off = |offset, key| Sent { offset, key, velocity: 0.0, on: false };
        let (mut l, mut r) = (vec![9.0; 6], vec![9.0; 6]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 6], "nothing heard: the notes go out instead");
        assert_eq!(playing.sent(), [on(2, 60, 1.0), off(5, 60)]);
        // On round: the second note ends at the part's end, and the first starts again.
        let (mut l, mut r) = (vec![9.0; 8], vec![9.0; 8]);
        playing.fill(&mut l, &mut r);
        assert_eq!(playing.sent(), [on(2, 64, 0.5), off(3, 64), on(6, 60, 1.0)]);
        // Stopped with a note on: it ends.
        player.stop();
        let (mut l, mut r) = (vec![9.0; 2], vec![9.0; 2]);
        playing.fill(&mut l, &mut r);
        assert_eq!(playing.sent(), [off(0, 60)]);
        playing.fill(&mut l, &mut r);
        assert!(playing.sent().is_empty());
    }

    #[test]
    fn a_part_sent_as_midi_ends_its_notes_when_the_daw_jumps_or_stops() {
        let player = Player::default();
        // 120 BPM at four frames a second: two frames a beat; a four-beat part with a note over its first beat.
        let mut playing = player.start(4);
        let part = Voice::notes_only(8, &[(0.0, 0.5, 48, 0.8)], 4).always_looped().in_beats(Some((4.0, 120.0)));
        player.hand_over(Box::new(part), "part".into());
        let song = |playing: bool, beats: f64| Some(Song { playing, beats, tempo: Some(120.0), bar: None });
        let on = Sent { offset: 0, key: 48, velocity: 0.8, on: true };
        let (mut l, mut r) = (vec![9.0; 1], vec![9.0; 1]);
        playing.fill_in_song(&mut l, &mut r, song(true, 0.0));
        assert_eq!(playing.sent(), [on]);
        // The DAW jumps to beat 2: the note on ends there; none starts until the part comes round.
        playing.fill_in_song(&mut l, &mut r, song(true, 2.0));
        assert_eq!(playing.sent(), [Sent { offset: 0, key: 48, velocity: 0.0, on: false }]);
        // Picked again at the start, then the DAW stops: the note ends with it. (A jump back here would be too small to
        // put it back in step: [`DRIFT`] is far more than these few frames.)
        let part = Voice::notes_only(8, &[(0.0, 0.5, 48, 0.8)], 4).always_looped().in_beats(Some((4.0, 120.0)));
        player.hand_over(Box::new(part), "again".into());
        playing.fill_in_song(&mut l, &mut r, song(true, 0.0));
        assert_eq!(playing.sent(), [on]);
        playing.fill_in_song(&mut l, &mut r, song(false, 0.5));
        assert_eq!(playing.sent(), [Sent { offset: 0, key: 48, velocity: 0.0, on: false }]);
        assert!(playing.now.is_none());
    }

    #[test]
    fn a_loop_starts_again_at_its_end_until_stopped() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let looped = Voice::new(&[vec![0.1, 0.2]], 48_000, 48_000).looped();
        let Played::Playing(_, n) = player.hand_over(Box::new(looped), "a".into()) else { panic!("not handed over") };
        let (mut l, mut r) = (vec![9.0; 5], vec![9.0; 5]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, [0.1, 0.2, 0.1, 0.2, 0.1].map(|x| x * LEVEL).to_vec());
        assert!(player.still_playing(n));
        // A block that ends right on the loop's end doesn't end the loop.
        let (mut l, mut r) = (vec![9.0; 1], vec![9.0; 1]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.2 * LEVEL]);
        assert!(playing.now.is_some(), "still looping");
        let (mut l, mut r) = (vec![9.0; 2], vec![9.0; 2]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.1 * LEVEL, 0.2 * LEVEL]);
        assert!(player.still_playing(n));
        let (mut l, mut r) = (vec![9.0; 5], vec![9.0; 5]);
        player.stop();
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 5]);
        assert!(!player.still_playing(n));
    }

    #[test]
    fn a_made_sound_plays_and_the_daw_tempo_is_kept() {
        let player = Arc::new(Player::default());
        assert_eq!(player.tempo(), None);
        player.set_tempo(f64::NAN);
        player.set_tempo(0.5);
        assert_eq!(player.tempo(), None);
        player.set_tempo(128.0);
        assert_eq!(player.tempo(), Some(128.0));
        let mut playing = player.start(48_000);
        let made =
            |channels: Vec<Vec<f32>>| mo_audio::AudioData { sample_rate: 48_000, channels, ..Default::default() };
        let (tx, rx) = std::sync::mpsc::channel();
        let audio = made(vec![vec![0.5; 4], vec![0.25; 4]]);
        player.play_made("Chords 01".into(), Box::new(move || Ok((MadeLoop::Sound(audio), None))), move |p| {
            tx.send(p).unwrap()
        });
        let told = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(matches!(&told, Played::Playing(name, _) if name == "Chords 01"), "{told:?}");
        let (mut l, mut r) = (vec![0.0; 6], vec![0.0; 6]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.5 * LEVEL; 6]);
        assert_eq!(r, vec![0.25 * LEVEL; 6]);
        // What couldn't be made is told.
        let (tx, rx) = std::sync::mpsc::channel();
        player.play_made("x".into(), Box::new(|| Err("No SoundFont".into())), move |p| tx.send(p).unwrap());
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), Played::Not("No SoundFont".into()));
    }

    #[test]
    fn a_newer_sound_replaces_the_one_playing() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let Played::Playing(_, first) = player.hand_over(voice(100), "a".into()) else { panic!() };
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        playing.fill(&mut l, &mut r);
        let Played::Playing(_, second) = player.hand_over(voice(100), "b".into()) else { panic!() };
        playing.fill(&mut l, &mut r);
        assert!(!player.still_playing(first));
        assert!(player.still_playing(second));
    }

    #[test]
    fn stop_hands_the_sound_back() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let Played::Playing(_, n) = player.hand_over(voice(100), "a".into()) else { panic!() };
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        playing.fill(&mut l, &mut r);
        player.stop();
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 4]);
        assert!(playing.now.is_none());
        assert!(!player.still_playing(n));
    }

    #[test]
    fn nothing_plays_before_the_daw_starts_the_audio() {
        let player = Player::default();
        assert_eq!(player.hand_over(voice(1), "a".into()), Played::Not(NOT_STARTED.into()));
    }

    #[test]
    fn a_stopped_track_plays_nothing_it_was_handed() {
        let player = Player::default();
        let _playing = player.start(48_000);
        let Played::Playing(_, n) = player.hand_over(voice(100), "a".into()) else { panic!() };
        player.stop_all();
        assert!(!player.still_playing(n));
    }
}
