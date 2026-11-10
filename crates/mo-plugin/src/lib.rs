//! The Music Organiser test plugin: the first step of the CLAP plugin plan (2026-11-07). It proves the parts every
//! later step stands on, inside a real DAW:
//!
//! - the app's own Slint panel opens in the plugin window the DAW lends ([`window`]);
//! - it lists sounds from the app's sound library, read-only, off the DAW's threads ([`library`]);
//! - a click plays a sound through the track, without ever stalling the DAW's audio ([`audition`]);
//! - it lists every CLAP extension the DAW has ([`check`]), to settle which of the plan's extensions Bitwig 6.1
//!   offers.
//!
//! It shows in the DAW as the instrument "Music Organiser (test)". It takes notes in (for later steps) but doesn't use
//! them yet. It never writes any of the app's files.
//!
//! A DAW calls a plugin from several threads: the main thread (opening the window, starting and stopping the audio)
//! and the audio thread ([`Processor`]). What they share lives in [`Instance`].

// The CLAP plugin wrapper (clack) catches a panic in any call from the DAW and tells the DAW the call failed, so a bug
// here never takes the DAW down with it; background threads catch theirs too. Each is written to the plugin's crash
// note ([`threads`]).

pub mod audition;
pub mod check;
pub mod library;
mod threads;
mod window;

#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ui {
    slint::include_modules!();
}
pub use ui::{ExtensionLine, Panel, SoundLine};

use audition::{Player, Playing};
use check::Offered;
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts, PluginAudioPortsImpl,
};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui, PluginGuiImpl, Window};
use clack_extensions::log::{HostLog, LogSeverity};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts, PluginNotePortsImpl,
};
use clack_extensions::track_info::{HostTrackInfo, PluginTrackInfo, PluginTrackInfoImpl, TrackInfoBuffer};
use clack_plugin::prelude::*;
use std::cell::{Cell, RefCell};
use std::ffi::CString;
use std::sync::{Arc, Mutex};

/// What the plugin's threads share: set up when the DAW makes the plugin.
pub struct Instance {
    /// The DAW and its version: "Bitwig Studio 6.1.6".
    pub host: String,
    /// Which CLAP extensions the DAW has.
    pub offered: Vec<Offered>,
    /// The track the plugin is on, when the DAW says.
    pub track: Mutex<String>,
    pub player: Arc<Player>,
}

pub struct MoPlugin;

impl Plugin for MoPlugin {
    type AudioProcessor<'a> = Processor;
    type Shared<'a> = Shared;
    type MainThread<'a> = MainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginGui>()
            .register::<PluginTrackInfo>();
    }
}

impl DefaultPluginFactory for MoPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::{INSTRUMENT, SAMPLER, STEREO};
        PluginDescriptor::new("com.silkie86.music-organiser.test", "Music Organiser (test)")
            .with_vendor("Music Organiser")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_description("Browse and play your sounds inside the DAW (test version)")
            .with_features([INSTRUMENT, SAMPLER, STEREO])
    }

    fn new_shared(host: HostSharedHandle<'_>) -> Result<Shared, PluginError> {
        let info = host.info();
        Ok(Shared {
            instance: Arc::new(Instance {
                host: check::host_line(&info),
                offered: check::ask(&info),
                track: Mutex::new(String::new()),
                player: Arc::new(Player::default()),
            }),
        })
    }

    fn new_main_thread<'a>(host: HostMainThreadHandle<'a>, shared: &'a Shared) -> Result<MainThread<'a>, PluginError> {
        let main = MainThread {
            host,
            track_info: host.shared().info().get_extension::<HostTrackInfo>(),
            log: host.shared().info().get_extension::<HostLog>(),
            shared,
            window: RefCell::new(None),
            scale: Cell::new(1.0),
        };
        main.read_track();
        Ok(main)
    }
}

pub struct Shared {
    pub instance: Arc<Instance>,
}

impl PluginShared<'_> for Shared {}

pub struct MainThread<'a> {
    host: HostMainThreadHandle<'a>,
    track_info: Option<HostTrackInfo>,
    log: Option<HostLog>,
    shared: &'a Shared,
    window: RefCell<Option<baseview::Window>>,
    /// The screen's scaling the DAW gave (Windows and Linux): the window's size in pixels is the panel's times it.
    scale: Cell<f64>,
}

impl<'a> PluginMainThread<'a, Shared> for MainThread<'a> {}

impl MainThread<'_> {
    /// Asks the DAW which track the plugin is on.
    fn read_track(&self) {
        let Some(track_info) = self.track_info else { return };
        let mut host = self.host;
        let mut buffer = TrackInfoBuffer::new();
        let name = track_info
            .get(&mut host, &mut buffer)
            .and_then(|info| info.name().map(|n| String::from_utf8_lossy(n).trim_end_matches('\0').to_owned()))
            .unwrap_or_default();
        *threads::lock(&self.shared.instance.track) = name;
    }

    /// The panel's size as the DAW counts it: in pixels at the scaling it gave (Windows, Linux), or in points (Mac).
    fn panel_size(&self) -> (u32, u32) {
        let scale = if logical_sizes() { 1.0 } else { self.scale.get() };
        ((window::WIDTH * scale).round() as u32, (window::HEIGHT * scale).round() as u32)
    }

    /// A line in the DAW's log (Bitwig: Help, Show log), when the DAW keeps one.
    fn say(&self, severity: LogSeverity, line: &str) {
        let (Some(log), Ok(line)) = (self.log, CString::new(line.replace('\0', " "))) else { return };
        log.log(&self.host.shared(), severity, &line);
    }
}

impl PluginTrackInfoImpl for MainThread<'_> {
    fn changed(&self) {
        // Shown the next time the panel opens.
        self.read_track();
    }
}

/// The audio thread's part: plays what the panel picked.
pub struct Processor {
    playing: Playing,
    /// Room for the right side when the DAW gives only one output channel (made here, not on the audio thread).
    spare: Vec<f32>,
}

impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Processor {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &MainThread<'a>,
        shared: &'a Shared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let rate = audio_config.sample_rate.round().clamp(1.0, f64::from(u32::MAX)) as u32;
        Ok(Processor {
            playing: shared.instance.player.start(rate),
            spare: vec![0.0; audio_config.max_frames_count as usize],
        })
    }

    fn process(&mut self, _process: Process, mut audio: Audio, _events: Events) -> Result<ProcessStatus, PluginError> {
        let Some(mut port) = audio.output_port(0) else { return Ok(ProcessStatus::Continue) };
        let Some(mut channels) = port.channels()?.into_f32() else { return Ok(ProcessStatus::Continue) };
        if channels.channel_count() >= 2 {
            let (mut left, mut right) = channels.split_at_mut(1);
            if let (Some(left), Some(right)) = (left.channel_mut(0), right.channel_mut(0)) {
                let len = left.len().min(right.len());
                self.playing.fill(&mut left[..len], &mut right[..len]);
            }
        } else if let Some(only) = channels.channel_mut(0) {
            let len = only.len().min(self.spare.len());
            self.playing.fill(&mut only[..len], &mut self.spare[..len]);
            // More frames than the DAW said it would ever ask for: the rest stays quiet.
            only[len..].fill(0.0);
        }
        // Always on: a pick can start playing at any moment.
        Ok(ProcessStatus::Continue)
    }

    fn deactivate(self, main_thread: &MainThread<'a>) {
        main_thread.shared.instance.player.stop_all();
    }
}

impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 0 } else { 1 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if !is_input && index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"Sounds",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for MainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 1 } else { 0 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if is_input && index == 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(1),
                name: b"Notes",
                preferred_dialect: Some(NoteDialect::Clap),
                supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
            });
        }
    }
}

/// Whether the DAW gives and takes window sizes in points before the screen's scaling (Mac) rather than in pixels.
fn logical_sizes() -> bool {
    GuiApiType::default_for_current_platform().is_some_and(|api| api.uses_logical_size())
}

/// The window kinds the plugin can draw in: the system's own (Windows, Mac, Linux X11), inside the DAW's window.
fn embedded(configuration: &GuiConfiguration) -> bool {
    !configuration.is_floating
        && GuiApiType::default_for_current_platform().is_some_and(|api| configuration.api_type == api)
}

impl PluginGuiImpl for MainThread<'_> {
    fn is_api_supported(&self, configuration: GuiConfiguration) -> bool {
        embedded(&configuration)
    }

    fn get_preferred_api(&self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration { api_type: GuiApiType::default_for_current_platform()?, is_floating: false })
    }

    fn create(&self, configuration: GuiConfiguration) -> Result<(), PluginError> {
        if embedded(&configuration) { Ok(()) } else { Err(PluginError::Message("This window kind isn't supported")) }
    }

    fn destroy(&self) {
        // Its window goes, and Slint's panel with it.
        self.window.borrow_mut().take();
    }

    fn set_scale(&self, scale: f64) -> Result<(), PluginError> {
        if scale.is_finite() && scale > 0.0 {
            self.scale.set(scale);
        }
        Ok(())
    }

    fn get_size(&self) -> Option<GuiSize> {
        let (width, height) = self.panel_size();
        Some(GuiSize { width, height })
    }

    fn set_size(&self, _size: GuiSize) -> Result<(), PluginError> {
        // One size for now (it can't be dragged bigger).
        Ok(())
    }

    fn set_parent(&self, parent: Window) -> Result<(), PluginError> {
        self.read_track();
        let (width, height) = self.panel_size();
        let size = if logical_sizes() {
            baseview::dpi::LogicalSize::new(f64::from(width), f64::from(height)).into()
        } else {
            baseview::dpi::PhysicalSize::new(width, height).into()
        };
        match window::open(&parent, size, Arc::clone(&self.shared.instance)) {
            Ok(window) => {
                *self.window.borrow_mut() = Some(window);
                Ok(())
            }
            Err(why) => {
                self.say(LogSeverity::Error, &format!("Music Organiser: {why}"));
                Err(PluginError::Message("Couldn't open the plugin's window"))
            }
        }
    }

    fn set_transient(&self, _window: Window) -> Result<(), PluginError> {
        Ok(())
    }

    fn show(&self) -> Result<(), PluginError> {
        if let Some(window) = self.window.borrow().as_ref() {
            let _ = window.show();
        }
        Ok(())
    }

    fn hide(&self) -> Result<(), PluginError> {
        if let Some(window) = self.window.borrow().as_ref() {
            let _ = window.hide();
        }
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<MoPlugin>);
