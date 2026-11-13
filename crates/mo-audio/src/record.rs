//! Recording a sound: what comes in (a sound card's input, or a plugin's input in a DAW) is held as a take, then
//! trimmed and saved as a WAV file. Shared by the app and the CLAP plugin (Solomon, 2026-11-13: either may be the one
//! that records, so the code is written once).
//!
//! The audio comes in on a thread that must never wait (a sound card's or a DAW's audio thread), through a [`Feed`]:
//! it copies each block into a lock-free queue and does nothing else. The [`Recording`] on an ordinary thread takes
//! it out as it comes and keeps it, up to [`MOST_SECONDS`]. A take is kept in memory only; [`save`] writes it.
//!
//! [`card`] (feature `record`) opens a sound card's input itself, in Windows' shared mode beside the DAW.

use std::path::{Path, PathBuf};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::{AudioData, AudioError};

/// The longest take: 10 minutes. At 48 kHz in stereo that's about 220 MB held while it records.
pub const MOST_SECONDS: u32 = 600;
/// How much the queue between the audio thread and the [`Recording`] holds: 2 seconds of audio, so a gathering
/// thread that's late (it's at low priority) loses nothing.
const QUEUE_SECONDS: u32 = 2;

/// The audio thread's end: copies each block in, never waits, never takes memory.
pub struct Feed {
    queue: Producer<f32>,
    channels: usize,
    /// Samples that didn't fit while the [`Recording`] was behind.
    lost: u64,
}

/// The other end: takes the audio out of the queue as it comes, and keeps it.
pub struct Recording {
    queue: Consumer<f32>,
    rate: u32,
    channels: Vec<Vec<f32>>,
    /// The loudest sample so far, from 0 to 1.
    peak: f32,
    /// Whether the take is at [`MOST_SECONDS`]: what comes after is let go.
    full: bool,
}

/// A new take of `channels` channels (1 or more) at `rate` frames a second: the [`Feed`] for the audio thread, the
/// [`Recording`] for a thread of its own.
pub fn recording(rate: u32, channels: usize) -> (Feed, Recording) {
    let channels = channels.clamp(1, 8);
    let room = (rate.max(1) as usize).saturating_mul(QUEUE_SECONDS as usize).saturating_mul(channels);
    let (queue, out) = RingBuffer::new(room);
    let kept = vec![Vec::new(); channels];
    (
        Feed { queue, channels, lost: 0 },
        Recording { queue: out, rate: rate.max(1), channels: kept, peak: 0.0, full: false },
    )
}

impl Feed {
    /// Copies one block in: `block` is its channels, each a slice of samples (any length; a missing channel is taken
    /// as the first, a short one padded with silence, extra ones left out). Never waits or takes memory: what doesn't
    /// fit in the queue is lost (and counted).
    pub fn push(&mut self, block: &[&[f32]]) {
        let Some(first) = block.first() else { return };
        let frames = block.iter().map(|c| c.len()).max().unwrap_or(0);
        for frame in 0..frames {
            if self.queue.slots() < self.channels {
                self.lost += (frames - frame) as u64 * self.channels as u64;
                return;
            }
            for channel in 0..self.channels {
                let samples = block.get(channel).unwrap_or(first);
                let sample = samples.get(frame).copied().filter(|s| s.is_finite()).unwrap_or(0.0);
                let _ = self.queue.push(sample);
            }
        }
    }

    /// [`Feed::push`] for interleaved samples, as a sound card gives them: `channels` to a frame.
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        let channels = channels.max(1);
        for frame in samples.chunks(channels) {
            if self.queue.slots() < self.channels {
                self.lost += self.channels as u64;
                continue;
            }
            for channel in 0..self.channels {
                let sample = frame.get(channel).or(frame.first()).copied().filter(|s| s.is_finite()).unwrap_or(0.0);
                let _ = self.queue.push(sample);
            }
        }
    }

    /// Samples lost because the [`Recording`] fell behind.
    pub fn lost(&self) -> u64 {
        self.lost
    }
}

impl Recording {
    /// Takes out what has come in since the last call. Whether the [`Feed`] is gone (the take has ended).
    pub fn gather(&mut self) -> bool {
        let most = self.rate as usize * MOST_SECONDS as usize;
        let n = self.channels.len();
        while self.queue.slots() >= n {
            for channel in 0..n {
                let Ok(sample) = self.queue.pop() else { break };
                self.peak = self.peak.max(sample.abs());
                if let Some(kept) = self.channels.get_mut(channel)
                    && kept.len() < most
                {
                    kept.push(sample);
                }
            }
            if self.channels.first().is_some_and(|c| c.len() >= most) {
                self.full = true;
            }
        }
        self.queue.is_abandoned()
    }

    /// How long the take is so far.
    pub fn seconds(&self) -> f64 {
        self.channels.first().map_or(0.0, |c| c.len() as f64 / f64::from(self.rate))
    }

    /// The loudest it has been, in dB (0 is full scale); none while it's been silent.
    pub fn peak_db(&self) -> Option<f64> {
        (self.peak > 0.0).then(|| 20.0 * f64::from(self.peak).log10())
    }

    /// Whether the take is at [`MOST_SECONDS`]: nothing more is kept.
    pub fn full(&self) -> bool {
        self.full
    }

    /// The take as a sound, with what's still in the queue.
    pub fn finish(mut self) -> AudioData {
        self.gather();
        let frames = self.channels.iter().map(Vec::len).min().unwrap_or(0);
        for c in &mut self.channels {
            c.truncate(frames);
        }
        AudioData {
            sample_rate: self.rate,
            file_seconds: frames as f64 / f64::from(self.rate),
            channels: self.channels,
            embedded_tempo: None,
            embedded_root_note: None,
            embedded_is_loop: None,
        }
    }
}

/// Saves a take in `folder` (made if it isn't there) as a 24-bit WAV named `name` (".wav" added; a number added when
/// the name is taken, so nothing is ever written over), with the silence cut off both ends when `trim` gives a level
/// (see [`crate::trim::level_of_db`]). Fails for a take that's silent all through or empty.
pub fn save(mut take: AudioData, trim: Option<f32>, folder: &Path, name: &str) -> Result<PathBuf, AudioError> {
    if let Some(level) = trim {
        if crate::trim::sound_span(&take, level).is_none() {
            return Err(AudioError::Format("Nothing was heard: the take is silent".into()));
        }
        take.trim_silence(level);
    }
    if take.channels.first().is_none_or(Vec::is_empty) {
        return Err(AudioError::Format("Nothing was recorded".into()));
    }
    std::fs::create_dir_all(folder)?;
    let stem: String =
        name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let stem = stem.trim().trim_end_matches('.');
    let stem = if stem.is_empty() { "Recording" } else { stem };
    let mut path = folder.join(format!("{stem}.wav"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem} ({n}).wav"));
        n += 1;
    }
    crate::write_wav(&path, &take.channels, take.sample_rate, 24)?;
    Ok(path)
}

/// A sound card's input, open and recording into a [`Feed`] (feature `record`). Dropping it closes the input, which
/// ends the take.
#[cfg(feature = "record")]
pub mod card {
    use super::{Feed, Recording, recording};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    /// The sound card inputs this computer has, by name, the system's default first.
    pub fn inputs() -> Vec<String> {
        let host = cpal::default_host();
        let mut names: Vec<String> = host.default_input_device().and_then(|d| name_of(&d)).into_iter().collect();
        if let Ok(devices) = host.input_devices() {
            for name in devices.filter_map(|d| name_of(&d)) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    fn name_of(device: &cpal::Device) -> Option<String> {
        device.description().ok().map(|d| d.name().to_owned())
    }

    /// An open input. Not Send on every system: keep it on the thread that opened it.
    pub struct Input {
        _stream: cpal::Stream,
        /// The input's name, as the system gives it.
        pub name: String,
        pub rate: u32,
        pub channels: u16,
    }

    /// Opens the input called `name` (the default for none) in shared mode, and starts a take from it.
    pub fn open(name: Option<&str>) -> Result<(Input, Recording), String> {
        let host = cpal::default_host();
        let device = match name {
            Some(wanted) => {
                host.input_devices().map_err(|e| e.to_string())?.find(|d| name_of(d).as_deref() == Some(wanted))
            }
            None => host.default_input_device(),
        }
        .ok_or_else(|| match name {
            Some(wanted) => format!("{wanted} isn't connected"),
            None => "There's no sound card input".to_owned(),
        })?;
        let supported = device.default_input_config().map_err(|e| format!("Couldn't open the input: {e}"))?;
        let rate = supported.sample_rate();
        let channels = supported.channels();
        let config = supported.config();
        let (feed, take) = recording(rate, usize::from(channels).min(2));
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream::<f32>(&device, &config, feed),
            cpal::SampleFormat::F64 => stream::<f64>(&device, &config, feed),
            cpal::SampleFormat::I16 => stream::<i16>(&device, &config, feed),
            cpal::SampleFormat::I24 => stream::<cpal::I24>(&device, &config, feed),
            cpal::SampleFormat::I32 => stream::<i32>(&device, &config, feed),
            cpal::SampleFormat::U16 => stream::<u16>(&device, &config, feed),
            other => Err(format!("The input gives {other} samples, which can't be recorded")),
        }?;
        stream.play().map_err(|e| format!("Couldn't start the input: {e}"))?;
        let name = name_of(&device).unwrap_or_else(|| name.unwrap_or("The default input").to_owned());
        Ok((Input { _stream: stream, name, rate, channels }, take))
    }

    /// A stream handing the input's samples, as f32, to `feed`.
    fn stream<T>(device: &cpal::Device, config: &cpal::StreamConfig, mut feed: Feed) -> Result<cpal::Stream, String>
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let channels = usize::from(config.channels);
        // Room for 16,384 frames a call, far more than an input gives at once, so its thread doesn't take memory.
        let mut samples: Vec<f32> = vec![0.0; 16_384 * channels.max(1)];
        device
            .build_input_stream::<T, _, _>(
                *config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    if samples.len() < data.len() {
                        samples.resize(data.len(), 0.0);
                    }
                    let Some(out) = samples.get_mut(..data.len()) else { return };
                    for (o, s) in out.iter_mut().zip(data) {
                        *o = s.to_sample::<f32>();
                    }
                    feed.push_interleaved(out, channels);
                },
                |_| {},
                None,
            )
            .map_err(|e| format!("Couldn't open the input: {e}"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_take_is_what_came_in() {
        let (mut feed, mut take) = recording(1000, 2);
        let left = [0.5f32; 100];
        let right = [-0.25f32; 100];
        feed.push(&[&left, &right]);
        // A mono block fills both channels; a short channel is padded with silence.
        feed.push(&[&[0.1f32; 10]]);
        feed.push(&[&[0.2f32; 10], &[0.3f32; 4]]);
        assert!(!take.gather());
        assert!((take.seconds() - 0.12).abs() < 1e-9);
        assert!((take.peak_db().unwrap() - 20.0 * 0.5f64.log10()).abs() < 1e-6);
        drop(feed);
        assert!(take.gather(), "the feed gone ends the take");
        let audio = take.finish();
        assert_eq!(audio.channels[0].len(), 120);
        assert_eq!(audio.channels[1][..100], right);
        assert_eq!(audio.channels[1][100..110], [0.1; 10]);
        assert_eq!(audio.channels[1][110..114], [0.3; 4]);
        assert_eq!(audio.channels[1][114..], [0.0; 6]);
    }

    #[test]
    fn a_feed_never_waits_for_a_late_recording() {
        let (mut feed, take) = recording(100, 1);
        // The queue holds 2 seconds: the rest is lost, not waited for.
        feed.push(&[&[0.5f32; 500]]);
        assert_eq!(feed.lost(), 300);
        assert_eq!(take.finish().channels[0].len(), 200);
        // Interleaved, as a sound card gives it: stereo into a mono take keeps the first channel.
        let (mut feed, take) = recording(100, 1);
        feed.push_interleaved(&[0.1, 0.9, 0.2, 0.8], 2);
        assert_eq!(take.finish().channels[0], [0.1, 0.2]);
    }
}
