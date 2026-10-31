//! The computer's sound outputs, through cpal (WASAPI on Windows, Core Audio on a Mac, ALSA on Linux). Kept behind
//! [`Outputs`] so the player's tests use a stand-in and never open a real output (rule 11).

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Fills a buffer of frames (`channels` samples each, one after another) at the output's `rate`.
pub type Render = Box<dyn FnMut(&mut [f32], u32, usize) + Send>;

/// Called, from any thread, when the output has stopped working (unplugged, or a DAW took the interface), with why.
pub type Lost = Box<dyn Fn(String) + Send + Sync>;

/// An output that's open and playing what its [`Render`] gives. Dropping it closes the output.
pub struct Opened {
    /// The output's name, as people know it.
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// How long sound takes from being mixed to coming out, in microseconds, as the system last said (0 until then).
    pub delay: Arc<AtomicU64>,
    /// The open stream, kept only so that dropping this closes it.
    pub stream: Box<dyn Any + Send>,
}

/// The computer's sound outputs.
pub trait Outputs: Send {
    /// The outputs there are now, by name, the computer's default first. It looks afresh, so it can take a moment.
    fn names(&mut self) -> Vec<String>;
    /// The computer's default output's name now.
    fn default_name(&mut self) -> Option<String>;
    /// Opens the output named (`None`: the default) and starts it, rendering with `render`.
    fn open(&mut self, name: Option<&str>, render: Render, lost: Lost) -> Result<Opened, String>;
}

/// The real outputs, through cpal.
pub struct SystemOutputs {
    host: cpal::Host,
}

impl Default for SystemOutputs {
    fn default() -> SystemOutputs {
        SystemOutputs { host: cpal::default_host() }
    }
}

impl SystemOutputs {
    pub fn new() -> SystemOutputs {
        SystemOutputs::default()
    }
}

fn name_of(device: &cpal::Device) -> Option<String> {
    use cpal::traits::DeviceTrait;
    device.description().ok().map(|d| d.name().to_owned())
}

impl Outputs for SystemOutputs {
    fn names(&mut self) -> Vec<String> {
        use cpal::traits::HostTrait;
        let mut names: Vec<String> = self.default_name().into_iter().collect();
        if let Ok(devices) = self.host.output_devices() {
            for name in devices.filter_map(|d| name_of(&d)) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    fn default_name(&mut self) -> Option<String> {
        use cpal::traits::HostTrait;
        self.host.default_output_device().as_ref().and_then(name_of)
    }

    fn open(&mut self, name: Option<&str>, render: Render, lost: Lost) -> Result<Opened, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = match name {
            None => self.host.default_output_device(),
            Some(wanted) => {
                self.host.output_devices().map_err(|e| e.to_string())?.find(|d| name_of(d).as_deref() == Some(wanted))
            }
        }
        .ok_or_else(|| match name {
            Some(wanted) => format!("{wanted} isn't connected"),
            None => "There's no sound output".to_owned(),
        })?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = supported.sample_rate();
        let channels = supported.channels();
        let delay = Arc::new(AtomicU64::new(0));
        let lost = Arc::new(lost);
        let mut config = supported.config();
        // About 10 ms a buffer, inside what the output allows: sweeping through sounds wants them heard at once.
        if let cpal::SupportedBufferSize::Range { min, max } = *supported.buffer_size() {
            config.buffer_size = cpal::BufferSize::Fixed((rate / 100).clamp(min, max.max(min)));
        }
        let format = supported.sample_format();
        let mut render = Some(render);
        let mut build = |config: cpal::StreamConfig| -> Result<cpal::Stream, String> {
            let Some(r) = render.take() else { return Err("The output couldn't be opened".into()) };
            let (delay, lost) = (Arc::clone(&delay), Arc::clone(&lost));
            match format {
                cpal::SampleFormat::F32 => stream::<f32>(&device, config, r, delay, lost),
                cpal::SampleFormat::F64 => stream::<f64>(&device, config, r, delay, lost),
                cpal::SampleFormat::I16 => stream::<i16>(&device, config, r, delay, lost),
                cpal::SampleFormat::I24 => stream::<cpal::I24>(&device, config, r, delay, lost),
                cpal::SampleFormat::I32 => stream::<i32>(&device, config, r, delay, lost),
                cpal::SampleFormat::U16 => stream::<u16>(&device, config, r, delay, lost),
                other => Err((format!("The output wants {other} samples, which the app doesn't make"), Some(r))),
            }
            .map_err(|(e, r)| {
                render = r;
                e
            })
        };
        let stream = match build(config) {
            Ok(s) => s,
            // Some outputs won't take a set buffer size: let them choose.
            Err(_) if config.buffer_size != cpal::BufferSize::Default => {
                build(cpal::StreamConfig { buffer_size: cpal::BufferSize::Default, ..config })?
            }
            Err(e) => return Err(e),
        };
        stream.play().map_err(|e| e.to_string())?;
        Ok(Opened {
            name: name_of(&device).unwrap_or_else(|| name.unwrap_or("The default output").to_owned()),
            sample_rate: rate,
            channels,
            delay,
            stream: Box::new(stream),
        })
    }
}

/// Builds a stream that renders in f32 and hands the output its own kind of sample. On failure, gives back `render`
/// so a second try can use it.
fn stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    render: Render,
    delay: Arc<AtomicU64>,
    lost: Arc<Lost>,
) -> Result<cpal::Stream, (String, Option<Render>)>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let rate = config.sample_rate;
    let channels = usize::from(config.channels);
    // The render is shared with the callback so a failed build can hand it back; the callback is the only user once
    // the stream runs, so the lock never waits.
    let shared = Arc::new(std::sync::Mutex::new(Some(render)));
    let in_callback = Arc::clone(&shared);
    let mut mixed: Vec<f32> = vec![0.0; 4096 * channels.max(1)];
    let built = device.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
            if mixed.len() < out.len() {
                mixed.resize(out.len(), 0.0);
            }
            let Some(mixed) = mixed.get_mut(..out.len()) else { return };
            match in_callback.try_lock().ok().as_deref_mut().and_then(Option::as_mut) {
                Some(render) => render(mixed, rate, channels),
                None => mixed.fill(0.0),
            }
            for (o, m) in out.iter_mut().zip(mixed.iter()) {
                *o = T::from_sample(*m);
            }
            let ts = info.timestamp();
            if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
                delay.store(u64::try_from(d.as_micros()).unwrap_or(u64::MAX), Ordering::Relaxed);
            }
        },
        move |e: cpal::Error| match e.kind() {
            // A glitch, a refused priority, or the system moving the sound for us: it plays on.
            cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged => {}
            _ => lost(e.to_string()),
        },
        None,
    );
    built.map_err(|e| (e.to_string(), shared.lock().ok().and_then(|mut r| r.take())))
}
