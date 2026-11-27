//! A long sound played as it's read: a thread of its own reads it a part at a time, a few seconds ahead of where it
//! plays, so a song plays to its end however long it is, starts as soon as its first part is read, and an hour-long
//! mix holds no more than those few seconds (rule 12). A sound made as it plays (a song's MIDI through a synth: a
//! [`Maker`]) goes the same way, made a part at a time instead of read.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{Ordering, fence};
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use rtrb::{Consumer, PushError, RingBuffer};

use super::mix::Flow;
use crate::read::frame_at;
use crate::{AudioData, AudioError};

/// How long each part read is.
const PART_SECONDS: f64 = 0.5;

/// How many parts may wait, read, ahead of the one playing: 8 seconds, so a read slowed by a busy computer or a
/// sleeping drive doesn't run dry, while a long mix holds no more than that.
const AHEAD: usize = 16;

/// How long the reader waits before looking again whether there's room for the next part. The output's thread never
/// wakes it (it mustn't wait on anything), so it looks by itself: a part plays for half a second, so the parts waiting
/// never fall far below [`AHEAD`].
const ROOM_EVERY: Duration = Duration::from_millis(100);

/// How long [`Stream::open`] waits before looking again for a part not read yet.
const PART_EVERY: Duration = Duration::from_millis(2);

/// How far into a sound Trim silence looks for its start, past silence: as far as the first minute, as for a sound
/// read whole.
const TRIM_MOST: f64 = crate::MAX_SECONDS;

/// The parts of a sound as its reader hands them on, the next first. Taking one never waits or takes memory, so the
/// output's thread can; letting them go stops the reader.
pub(crate) type Parts = Consumer<Arc<AudioData>>;

/// A sound file being read to play ([`super::Player::play_stream_if`]): its first part, read already, and the rest as
/// it's read.
pub struct Stream {
    /// The file, to read again from another place (a move, or the output opened again); none for parts made in
    /// memory.
    pub(super) source: Option<Source>,
    pub(super) first: Arc<AudioData>,
    pub(super) more: Parts,
    /// The frame of the file the first part starts at.
    pub(super) start: u64,
}

/// What a stream reads again from another place: its file, or what makes it, and its sample rate.
#[derive(Clone)]
pub(super) struct Source {
    from: Origin,
    rate: u32,
}

#[derive(Clone)]
enum Origin {
    File(PathBuf),
    Made(Arc<dyn Maker>),
}

/// Makes a sound as it plays ([`Stream::made`]): from any place in it, a part at a time, on a thread of its own, so a
/// whole song made by a synth starts at once, never sits in memory whole, and a move makes it again from there.
pub trait Maker: Send + Sync {
    /// The sample rate it makes sound at.
    fn rate(&self) -> u32;
    /// Makes the sound from `seconds` in to its end, handing each part on (its channels, each the same length, at
    /// most a second or so) to `put`, which says whether to go on: false once it's no longer wanted. It runs on a
    /// thread of its own, a few seconds ahead of where it plays (`put` waits while there's no room). Err: why it
    /// couldn't make it (said by [`Stream::made`] when nothing was made).
    fn make(&self, seconds: f64, put: &mut dyn FnMut(Vec<Vec<f32>>) -> bool) -> Result<(), String>;
}

impl Stream {
    /// Starts reading `path` from `from_seconds` in, on a thread of its own, and waits for its first part, so a file
    /// that can't be read says so here. With `quiet` (Trim silence's level) it starts past the silence at the file's
    /// start, when it plays from there. Past the file's end, it has nothing to play. Nor once `wanted` says it's no
    /// longer wanted (another sound was asked for): it stops waiting then, as a place far into a long compressed file
    /// is only reached by decoding all before it.
    pub fn open(
        path: &Path,
        from_seconds: f64,
        quiet: Option<f32>,
        wanted: &dyn Fn() -> bool,
    ) -> Result<Stream, AudioError> {
        let (mut more, done) = read_on(path.to_owned(), from_seconds)?;
        let mut first = match next_part(&mut more, wanted) {
            Next::Part(first) => first,
            // Its parts are let go here, which stops its reader.
            Next::Unwanted => return Ok(Stream::nothing()),
            // Nothing came: the read failed, or there's nothing from there on.
            Next::Ended => {
                return match done.recv() {
                    Ok(Err(e)) => Err(e),
                    _ => Ok(Stream::nothing()),
                };
            }
        };
        let rate = first.sample_rate;
        let mut start = frame_at(from_seconds, rate);
        if let Some(level) = quiet.filter(|_| start == 0) {
            let most = frame_at(TRIM_MOST, rate);
            loop {
                if let Some((loud, _)) = crate::trim::sound_span(&first, level) {
                    if loud > 0 {
                        first = Arc::new(tail(&first, loud));
                        start += loud as u64;
                    }
                    break;
                }
                // Silent all through: the next part, unless that goes past where Trim silence looks.
                let next_start = start + first.len() as u64;
                let next = if next_start < most { next_part(&mut more, wanted) } else { Next::Ended };
                match next {
                    Next::Part(next) => {
                        first = next;
                        start = next_start;
                    }
                    Next::Unwanted => return Ok(Stream::nothing()),
                    // Silent as far as it looks (or to its end): it plays from its start, as a sound read whole does.
                    Next::Ended => return Stream::open(path, from_seconds, None, wanted),
                }
            }
        }
        Ok(Stream { source: Some(Source { from: Origin::File(path.to_owned()), rate }), first, more, start })
    }

    /// Starts making `maker`'s sound from `from_seconds` in, on a thread of its own, and waits for its first part. Past
    /// its end, or once `wanted` says it's no longer wanted, it has nothing to play. A move ([`super::Player::seek`]),
    /// or the output opened again, makes it again from there.
    pub fn made(maker: Arc<dyn Maker>, from_seconds: f64, wanted: &dyn Fn() -> bool) -> Result<Stream, AudioError> {
        let rate = maker.rate();
        let (mut more, done) = make_on(Arc::clone(&maker), from_seconds)?;
        match next_part(&mut more, wanted) {
            Next::Part(first) => Ok(Stream {
                source: Some(Source { from: Origin::Made(maker), rate }),
                first,
                more,
                start: frame_at(from_seconds, rate),
            }),
            Next::Unwanted => Ok(Stream::nothing()),
            // Nothing came: it couldn't be made, or there's nothing from there on.
            Next::Ended => match done.recv() {
                Ok(Err(e)) => Err(e),
                _ => Ok(Stream::nothing()),
            },
        }
    }

    /// A stream with nothing to play: it ends as soon as it starts.
    fn nothing() -> Stream {
        let (_, more) = RingBuffer::new(1);
        Stream { source: None, first: Arc::default(), more, start: 0 }
    }

    /// The file's sample rate (0 when it had nothing to play).
    pub fn sample_rate(&self) -> u32 {
        self.first.sample_rate
    }

    /// Where it starts in its file, in seconds: where it was asked to, or past the silence at the file's start.
    pub fn start_seconds(&self) -> f64 {
        self.start as f64 / f64::from(self.sample_rate().max(1))
    }

    /// A stream of parts made in memory, all there: for the tests.
    #[cfg(test)]
    pub(super) fn of_parts(parts: Vec<Arc<AudioData>>, start: u64) -> Stream {
        let (mut tx, more) = RingBuffer::new(parts.len().max(1));
        let mut parts = parts.into_iter();
        let first = parts.next().unwrap_or_default();
        for p in parts {
            let _ = tx.push(p);
        }
        Stream { source: None, first, more, start }
    }
}

impl Source {
    /// Starts reading the file again from `seconds` in, without waiting for it: the part to start with (nothing yet,
    /// so it fades in as the read comes) and the flow the rest comes through.
    pub(super) fn read_from(&self, seconds: f64) -> Result<(Arc<AudioData>, Box<Flow>), AudioError> {
        let (more, _) = match &self.from {
            Origin::File(path) => read_on(path.clone(), seconds)?,
            Origin::Made(maker) => make_on(Arc::clone(maker), seconds)?,
        };
        let nothing = Arc::new(AudioData { sample_rate: self.rate, ..AudioData::default() });
        Ok((nothing, Box::new(Flow::new(frame_at(seconds, self.rate), more))))
    }
}

/// What [`next_part`] came to.
enum Next {
    Part(Arc<AudioData>),
    /// The reader finished without another.
    Ended,
    /// It was no longer wanted.
    Unwanted,
}

/// The next part from `parts` as soon as it's read, unless the reader finishes without another, or `wanted` says to
/// stop waiting first.
fn next_part(parts: &mut Parts, wanted: &dyn Fn() -> bool) -> Next {
    loop {
        if !wanted() {
            return Next::Unwanted;
        }
        if let Ok(part) = parts.pop() {
            return Next::Part(part);
        }
        if parts.is_abandoned() {
            // What the reader handed on before it finished is all there now (rtrb's own advice, for Rust 1.74 on).
            fence(Ordering::Acquire);
            return parts.pop().map_or(Next::Ended, Next::Part);
        }
        std::thread::sleep(PART_EVERY);
    }
}

/// Frames `from..` of `part`.
fn tail(part: &AudioData, from: usize) -> AudioData {
    AudioData {
        sample_rate: part.sample_rate,
        channels: part.channels.iter().map(|c| c.get(from..).unwrap_or_default().to_vec()).collect(),
        ..AudioData::default()
    }
}

/// What a stream's reader hands on: its parts (at most [`AHEAD`] waiting), and once it ends, what the read came to.
type Reading = (Parts, Receiver<Result<f64, AudioError>>);

/// Reads `path` from `from_seconds` on, on a thread of its own. It waits while [`AHEAD`] parts wait to be played, and
/// stops once the parts are let go (the play has gone), also while it's still on its way to where it starts. Its
/// priority is the system's to give: on Windows a new thread always starts at normal priority; on Linux and macOS it
/// takes the priority of the thread that starts it, so a stream a page's low-priority reader opens is read at low
/// priority there, which its 8 seconds read ahead cover unless the computer stays busy for longer.
fn read_on(path: PathBuf, from_seconds: f64) -> Result<Reading, AudioError> {
    hand_on("sound stream", move |each| {
        crate::read::read_parts_from(&path, from_seconds, PART_SECONDS, &mut |rate, part| each(rate, part.to_vec()))
    })
}

/// Makes `maker`'s sound from `from_seconds` on, on a thread of its own, as [`read_on`] reads a file: it waits while
/// [`AHEAD`] parts wait to be played, and stops once they're let go.
fn make_on(maker: Arc<dyn Maker>, from_seconds: f64) -> Result<Reading, AudioError> {
    hand_on("made sound", move |each| {
        let rate = maker.rate();
        maker.make(from_seconds, &mut |part| each(rate, part)).map_err(AudioError::NotSupported)?;
        Ok(from_seconds)
    })
}

/// What [`hand_on`]'s work hands each part to: the part's rate and channels (taken, not copied), and whether to go on.
type Each<'a> = &'a mut dyn FnMut(u32, Vec<Vec<f32>>) -> bool;

/// Runs `work` on a thread named `name`, handing on the parts it gives to the [`Reading`]: it waits while [`AHEAD`]
/// parts wait to be played, and is told to stop once they're let go.
fn hand_on(
    name: &str,
    work: impl FnOnce(Each<'_>) -> Result<f64, AudioError> + Send + 'static,
) -> Result<Reading, AudioError> {
    let (mut parts, more) = RingBuffer::new(AHEAD);
    let (said, done) = sync_channel(1);
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let read = work(&mut |rate, part| {
                // A part with nothing in it comes while it's on its way to where it starts: it only asks whether to go on.
                if parts.is_abandoned() || part.first().is_none_or(Vec::is_empty) {
                    return !parts.is_abandoned();
                }
                let mut part = Arc::new(AudioData { sample_rate: rate, channels: part, ..AudioData::default() });
                loop {
                    match parts.push(part) {
                        Ok(()) => return true,
                        Err(PushError::Full(back)) if !parts.is_abandoned() => {
                            part = back;
                            std::thread::sleep(ROOM_EVERY);
                        }
                        Err(PushError::Full(_)) => return false,
                    }
                }
            });
            drop(parts);
            let _ = said.send(read);
        })
        .map_err(AudioError::Io)?;
    Ok((more, done))
}
