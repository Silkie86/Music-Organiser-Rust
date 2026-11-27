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
    /// The one channel taken from what comes in (0 left, 1 right) for a take of one side of a stereo pair.
    only: Option<usize>,
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
    /// Its shape so far: the loudest sample of each [`SHAPE_EVERY`] of a second, for a waveform drawn as it records.
    shape: Vec<f32>,
    /// The loudest sample of the stretch not yet in `shape`, and how many frames it has.
    shaping: (f32, usize),
}

/// How many of each second's stretches [`Recording::shape`] has (each the loudest sample in it).
pub const SHAPE_EVERY: u32 = 20;

/// A new take of `channels` channels (1 or more) at `rate` frames a second: the [`Feed`] for the audio thread, the
/// [`Recording`] for a thread of its own.
pub fn recording(rate: u32, channels: usize) -> (Feed, Recording) {
    recording_of(rate, channels, None)
}

/// [`recording`], of `only` one channel of what comes in when it says one (0 left, 1 right): a mono take of one side
/// of a stereo pair (Solomon, 25 Nov).
pub fn recording_of(rate: u32, channels: usize, only: Option<usize>) -> (Feed, Recording) {
    let channels = if only.is_some() { 1 } else { channels.clamp(1, 8) };
    let room = (rate.max(1) as usize).saturating_mul(QUEUE_SECONDS as usize).saturating_mul(channels);
    let (queue, out) = RingBuffer::new(room);
    let kept = vec![Vec::new(); channels];
    (
        Feed { queue, channels, only, lost: 0 },
        Recording {
            queue: out,
            rate: rate.max(1),
            channels: kept,
            peak: 0.0,
            full: false,
            shape: Vec::new(),
            shaping: (0.0, 0),
        },
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
                let samples = block.get(self.only.unwrap_or(channel)).unwrap_or(first);
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
                let sample = frame
                    .get(self.only.unwrap_or(channel))
                    .or(frame.first())
                    .copied()
                    .filter(|s| s.is_finite())
                    .unwrap_or(0.0);
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
        let stretch = (self.rate / SHAPE_EVERY).max(1) as usize;
        while self.queue.slots() >= n {
            if self.full {
                // Let go: the queue still empties, so the feed never waits.
                for _ in 0..n {
                    let _ = self.queue.pop();
                }
                continue;
            }
            for channel in 0..n {
                let Ok(sample) = self.queue.pop() else { break };
                self.peak = self.peak.max(sample.abs());
                self.shaping.0 = self.shaping.0.max(sample.abs());
                if let Some(kept) = self.channels.get_mut(channel)
                    && kept.len() < most
                {
                    kept.push(sample);
                }
            }
            self.shaping.1 += 1;
            if self.shaping.1 >= stretch {
                self.shape.push(self.shaping.0);
                self.shaping = (0.0, 0);
            }
            if self.channels.first().is_some_and(|c| c.len() >= most) {
                self.full = true;
            }
        }
        self.queue.is_abandoned()
    }

    /// The take's shape so far: the loudest sample (0 to 1, more when it clips) of each [`SHAPE_EVERY`] of a second.
    pub fn shape(&self) -> &[f32] {
        &self.shape
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
///
/// A name is claimed by making its part file (`<name>.wav.part`) new: one that's there already is another save's
/// (two copies of the plugin, in one DAW or two, saving takes named for the same second), or one a cut-off save left,
/// so the next name is tried. Before, both saves took the same name and wrote into one part file, so one take was lost
/// and the other spoilt (the whole-app review of 26 Nov, XD-1). The part becomes the take only while nothing else has
/// taken its name; else it's left, and `mend_parts` saves it later.
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
    let mut n = 1;
    let (path, part, file) = loop {
        let path = folder.join(if n == 1 { format!("{stem}.wav") } else { format!("{stem} ({n}).wav") });
        n += 1;
        if std::fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        let part = path.with_extension("wav.part");
        match std::fs::File::create_new(&part) {
            Ok(file) => break (path, part, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    };
    if let Err(e) = crate::write::write_wav_into(file, &take.channels, take.sample_rate, 24) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    // Only saves claim these names, by their part files, so a take of that name now is a file of someone else's:
    // never written over. The part is left whole, for `mend_parts`.
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(AudioError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} was made meanwhile, so the take is kept in {}", path.display(), part.display()),
        )));
    }
    if let Err(e) = std::fs::rename(&part, &path) {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(path)
}

/// The size of the WAV header [`crate::write_wav`] writes, before the samples.
const HEAD: u64 = 44;

/// Saves the takes in `folder` whose saving was cut off (the DAW closed or crashed while a take was written): each
/// part file `is_take` says is a take's (only the names the app gives its takes as they're saved) not written to for
/// `older_than` is cut to its whole frames, its sizes are mended to what's there, and it's renamed
/// "Recording … (recovered).wav". They're the user's takes, so they're never thrown away, only
/// one with nothing in it (shorter than its header). What became of each, in plain words.
///
/// Only the takes' own part files: a part still being written (a take another copy of the plugin is saving) has
/// just been written to, so it's left for `older_than`. Writes to the disk: off the UI and audio threads.
pub fn mend_parts(folder: &Path, older_than: std::time::Duration, is_take: impl Fn(&str) -> bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut said = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".wav.part").filter(|_| is_take(&name)) else { continue };
        // A plain file only, never through a link (rule 28).
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.file_type().is_file() {
            continue;
        }
        let quiet = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|e| e >= older_than);
        if !quiet {
            continue;
        }
        let part = entry.path();
        if meta.len() <= HEAD {
            if std::fs::remove_file(&part).is_ok() {
                said.push(format!("Removed {name}: nothing had been saved in it"));
            }
            continue;
        }
        let mut to = folder.join(format!("{stem} (recovered).wav"));
        let mut n = 2;
        while to.exists() {
            to = folder.join(format!("{stem} (recovered {n}).wav"));
            n += 1;
        }
        match mend(&part, meta.len()).and_then(|()| std::fs::rename(&part, &to)) {
            Ok(()) => said.push(format!("Saved a take that was cut off as {}", to.display())),
            Err(e) => said.push(format!("Couldn't mend {name}: {e}")),
        }
    }
    said
}

/// Cuts the part file at `part` (`len` bytes) to its whole frames and sets its sizes to them.
fn mend(part: &Path, len: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(part)?;
    let mut head = [0u8; HEAD as usize];
    file.read_exact(&mut head)?;
    let not_ours = || std::io::Error::other("it isn't a WAV file the app wrote");
    if &head[..4] != b"RIFF" || &head[8..16] != b"WAVEfmt " || &head[36..40] != b"data" {
        return Err(not_ours());
    }
    let block = u64::from(u16::from_le_bytes([head[32], head[33]]));
    if block == 0 {
        return Err(not_ours());
    }
    let data = ((len - HEAD) / block * block).min(u64::from(u32::MAX - 37));
    let pad = data & 1;
    file.set_len(HEAD + data + pad)?;
    if pad == 1 {
        file.seek(SeekFrom::Start(HEAD + data))?;
        file.write_all(&[0])?;
    }
    let size = |n: u64| u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes();
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&size(36 + data + pad))?;
    file.seek(SeekFrom::Start(40))?;
    file.write_all(&size(data))?;
    file.sync_all()
}

/// A sound card's input, open and recording into a [`Feed`] (feature `record`). Dropping it closes the input, which
/// ends the take.
#[cfg(feature = "record")]
pub mod card {
    use super::{Recording, recording_of};
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

    /// Opens the input called `name` (the default for none) in shared mode, and starts a take from it: of `only` one
    /// of its channels when that says one (0 left, 1 right), as a mono take.
    pub fn open(name: Option<&str>, only: Option<usize>) -> Result<(Input, Recording), String> {
        let (device, supported) = device(name)?;
        let channels = supported.channels();
        let (mut feed, take) = recording_of(supported.sample_rate(), usize::from(channels).min(2), only);
        let input =
            start(&device, &supported, name, move |samples, channels| feed.push_interleaved(samples, channels))?;
        Ok((input, take))
    }

    /// How loud an input has been since it was last asked: the loudest sample's size (0 to 1, more when it clips),
    /// kept by the input's own thread and taken by whoever shows it.
    #[derive(Debug, Default)]
    pub struct Level(std::sync::atomic::AtomicU32);

    impl Level {
        /// The loudest since the last time it was taken, and starts again from quiet.
        pub fn take(&self) -> f32 {
            f32::from_bits(self.0.swap(0, std::sync::atomic::Ordering::AcqRel))
        }

        /// `peak` was heard: kept if it's the loudest yet. (A peak is never negative, so its bits sort as it does.)
        pub fn note(&self, peak: f32) {
            if peak.is_finite() && peak > 0.0 {
                self.0.fetch_max(peak.to_bits(), std::sync::atomic::Ordering::AcqRel);
            }
        }
    }

    /// Opens the input called `name` in shared mode only to show how loud it is: nothing is recorded or kept.
    pub fn meter(name: &str) -> Result<(Input, std::sync::Arc<Level>), String> {
        let (device, supported) = device(Some(name))?;
        let level = std::sync::Arc::new(Level::default());
        let heard = std::sync::Arc::clone(&level);
        let input = start(&device, &supported, Some(name), move |samples, _| {
            heard.note(samples.iter().fold(0.0f32, |m, s| m.max(s.abs())));
        })?;
        Ok((input, level))
    }

    /// The input called `name` (the default for none), and how it gives its samples.
    fn device(name: Option<&str>) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
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
        Ok((device, supported))
    }

    /// Starts `device`'s input, handing its samples (as f32, interleaved, with how many channels) to `sink`.
    fn start(
        device: &cpal::Device,
        supported: &cpal::SupportedStreamConfig,
        name: Option<&str>,
        sink: impl FnMut(&[f32], usize) + Send + 'static,
    ) -> Result<Input, String> {
        let rate = supported.sample_rate();
        let channels = supported.channels();
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream::<f32>(device, &config, sink),
            cpal::SampleFormat::F64 => stream::<f64>(device, &config, sink),
            cpal::SampleFormat::I16 => stream::<i16>(device, &config, sink),
            cpal::SampleFormat::I24 => stream::<cpal::I24>(device, &config, sink),
            cpal::SampleFormat::I32 => stream::<i32>(device, &config, sink),
            cpal::SampleFormat::U16 => stream::<u16>(device, &config, sink),
            other => Err(format!("The input gives {other} samples, which can't be recorded")),
        }?;
        stream.play().map_err(|e| format!("Couldn't start the input: {e}"))?;
        let name = name_of(device).unwrap_or_else(|| name.unwrap_or("The default input").to_owned());
        Ok(Input { _stream: stream, name, rate, channels })
    }

    /// A stream handing the input's samples, as f32, to `sink`.
    fn stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        mut sink: impl FnMut(&[f32], usize) + Send + 'static,
    ) -> Result<cpal::Stream, String>
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
                    sink(out, channels);
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
    fn a_take_cut_off_while_it_was_saved_is_mended_not_thrown_away() {
        let dir = std::env::temp_dir().join(format!("mo-audio-mend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A whole take, written as a save writes it, then cut off part-way through a frame.
        let whole = dir.join("whole.wav");
        crate::write_wav(&whole, &[vec![0.5; 100], vec![-0.5; 100]], 1000, 24).unwrap();
        let bytes = std::fs::read(&whole).unwrap();
        std::fs::remove_file(&whole).unwrap();
        std::fs::write(dir.join("Recording 2026-11-15 21-04-10.wav.part"), &bytes[..44 + 6 * 40 + 4]).unwrap();
        std::fs::write(dir.join("Recording 2026-11-15 21-05-00.wav.part"), &bytes[..20]).unwrap();
        std::fs::write(dir.join("Kit.wav.part"), &bytes[..60]).unwrap();
        let is_take = |n: &str| n.starts_with("Recording 2026") && n.ends_with(".wav.part");
        let said = mend_parts(&dir, std::time::Duration::ZERO, is_take);
        assert_eq!(said.len(), 2, "{said:?}");
        let mended = dir.join("Recording 2026-11-15 21-04-10 (recovered).wav");
        let audio = crate::read(&mended, 60.0).unwrap();
        assert_eq!(audio.channels.len(), 2);
        assert_eq!(audio.channels[0].len(), 40);
        assert!(!dir.join("Recording 2026-11-15 21-05-00.wav.part").exists(), "an empty part goes");
        assert!(dir.join("Kit.wav.part").exists(), "only a take's own part files");
        // One being written just now is left.
        std::fs::write(dir.join("Recording 2026-11-15 22-00-00.wav.part"), &bytes).unwrap();
        assert!(mend_parts(&dir, std::time::Duration::from_secs(600), is_take).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
