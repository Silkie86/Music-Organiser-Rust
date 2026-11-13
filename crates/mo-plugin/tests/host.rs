//! The plugin loaded by a small stand-in DAW (clack's host side), as Bitwig would load it: it must say what it is,
//! give one stereo output and one note input, offer its window, and stay silent until something is picked. No real
//! DAW, folder or sound card is touched (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui};
use clack_extensions::note_ports::{NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_extensions::remote_controls::{PluginRemoteControls, RemoteControlsPageBuffer};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::ParamValueEvent;
use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use mo_plugin::entry::MoEntry;

fn entry() -> PluginEntry {
    PluginEntry::load_from_clack::<MoEntry>(c"").unwrap()
}

fn instance(entry: &PluginEntry) -> PluginInstance<()> {
    let info = HostInfo::new("Stand-in DAW", mo_plugin::TESTS_VENDOR, "", "1.0").unwrap();
    PluginInstance::<()>::new(|_| (), |_| (), entry, c"com.silkie86.music-organiser.test", &info).unwrap()
}

#[test]
fn says_what_it_is() {
    let entry = entry();
    let descriptor = entry.get_factory::<PluginFactory>().unwrap().plugin_descriptor(0).unwrap();
    assert_eq!(descriptor.id().unwrap().to_bytes(), b"com.silkie86.music-organiser.test");
    assert_eq!(descriptor.name().unwrap().to_bytes(), b"Music Organiser (test)");
    let features: Vec<_> = descriptor.features().map(|f| f.to_bytes()).collect();
    assert_eq!(features, [&b"instrument"[..], b"sampler", b"stereo"]);
}

#[test]
fn a_stereo_output_and_input_a_note_input_and_output_and_a_window() {
    let entry = entry();
    let mut plugin = instance(&entry);
    let handle = plugin.plugin_handle();

    let audio = handle.get_extension::<PluginAudioPorts>().unwrap();
    assert_eq!((audio.count(&handle, true), audio.count(&handle, false)), (1, 1));
    let mut buffer = AudioPortInfoBuffer::new();
    let port = audio.get(&handle, 0, false, &mut buffer).unwrap();
    assert_eq!(port.channel_count, 2);
    // The input Record takes from (Bitwig's sidechain): stereo, and not the main one.
    let port = audio.get(&handle, 0, true, &mut buffer).unwrap();
    assert_eq!((port.channel_count, port.name), (2, &b"In"[..]));
    assert!(!port.flags.contains(clack_extensions::audio_ports::AudioPortFlags::IS_MAIN));

    let notes = handle.get_extension::<PluginNotePorts>().unwrap();
    // One in, and one out: a MIDI part's notes with Send MIDI on.
    assert_eq!((notes.count(&handle, true), notes.count(&handle, false)), (1, 1));
    let mut buffer = NotePortInfoBuffer::new();
    assert!(notes.get(&handle, 0, true, &mut buffer).is_some());
    let out = notes.get(&handle, 0, false, &mut buffer).unwrap();
    assert_eq!((out.id.get(), out.name), (2, &b"Parts out"[..]));

    let gui = handle.get_extension::<PluginGui>().unwrap();
    let api = GuiApiType::default_for_current_platform().unwrap();
    assert!(gui.is_api_supported(&handle, GuiConfiguration { api_type: api, is_floating: false }));
    assert!(!gui.is_api_supported(&handle, GuiConfiguration { api_type: api, is_floating: true }));

    // The window can be dragged to another size (with no window open yet, the size is kept for when one opens):
    // never smaller than the panel fits in, and it opens again at the size it was left.
    assert!(gui.can_resize(&handle));
    let hints = gui.get_resize_hints(&handle).unwrap();
    assert!(hints.can_resize_horizontally && hints.can_resize_vertically);
    let opens_at = gui.get_size(&handle).unwrap();
    let small = gui.adjust_size(&handle, GuiSize { width: 10, height: 10 }).unwrap();
    assert!(small.width > 10 && small.width <= opens_at.width && small.height <= opens_at.height);
    let bigger = GuiSize { width: opens_at.width + 300, height: opens_at.height + 200 };
    assert_eq!(gui.adjust_size(&handle, bigger), Some(bigger));
    gui.set_size(&handle, bigger).unwrap();
    assert_eq!(gui.get_size(&handle), Some(bigger));
}

/// The DAW saves the plugin's state with its project and hands it back: the window's size comes back with it, and
/// a state that isn't the plugin's is taken without a fuss (the project must still open).
#[test]
fn its_state_goes_into_the_project_and_comes_back() {
    let entry = entry();
    let mut saved = Vec::new();
    let dragged_to = {
        let mut plugin = instance(&entry);
        let handle = plugin.plugin_handle();
        let gui = handle.get_extension::<PluginGui>().unwrap();
        let opens_at = gui.get_size(&handle).unwrap();
        let bigger = GuiSize { width: opens_at.width + 120, height: opens_at.height + 80 };
        gui.set_size(&handle, bigger).unwrap();
        handle.get_extension::<PluginState>().unwrap().save(&handle, &mut saved).unwrap();
        bigger
    };
    assert!(!saved.is_empty());

    let mut plugin = instance(&entry);
    let handle = plugin.plugin_handle();
    let gui = handle.get_extension::<PluginGui>().unwrap();
    let state = handle.get_extension::<PluginState>().unwrap();
    assert_ne!(gui.get_size(&handle), Some(dragged_to));
    state.load(&handle, &mut &saved[..]).unwrap();
    assert_eq!(gui.get_size(&handle), Some(dragged_to));

    state.load(&handle, &mut &b"not this plugin's state \xff\xfe"[..]).unwrap();
    state.load(&handle, &mut &b""[..]).unwrap();
    assert_eq!(gui.get_size(&handle), Some(dragged_to));
}

/// Eight knobs on one page, as Bitwig shows a device's remote controls; turned by the DAW, they take.
#[test]
fn eight_knobs_on_one_page_that_the_daw_can_turn() {
    let entry = entry();
    let mut plugin = instance(&entry);
    let handle = plugin.plugin_handle();
    let params = handle.get_extension::<PluginParams>().unwrap();
    assert_eq!(params.count(&handle), 8);
    let mut buffer = ParamInfoBuffer::new();
    let mut names = Vec::new();
    for index in 0..8 {
        let info = params.get_info(&handle, index, &mut buffer).unwrap();
        assert_eq!(info.id.get(), index + 1);
        assert_eq!((info.min_value, info.max_value), (0.0, 1.0));
        names.push(String::from_utf8_lossy(info.name).into_owned());
    }
    let want = ["Sound", "Sounds like", "MIDI part", "Preset", "Volume", "Loop", "Match tempo", "In time"];
    assert_eq!(names, want);
    let remote = handle.get_extension::<PluginRemoteControls>().unwrap();
    assert_eq!(remote.count(&handle), 1);
    let mut page = RemoteControlsPageBuffer::new();
    let page = remote.get(&handle, 0, &mut page).unwrap();
    assert_eq!(page.param_ids.map(|id| id.map(|id| id.get())), [1, 2, 3, 4, 5, 6, 7, 8].map(Some));
    let in_time = ClapId::new(8);
    assert_eq!(params.get_value(&handle, in_time), Some(1.0));
    let mut words = [0u8; 64];
    assert_eq!(params.value_to_text(&handle, in_time, 1.0, &mut words).unwrap(), b"On");

    // The DAW turns In time off while the track's audio is stopped: it takes, and nothing is told back.
    let mut turned = EventBuffer::new();
    turned.push(&ParamValueEvent::new(0, in_time, Pckn::match_all(), 0.0));
    let mut told = EventBuffer::new();
    let mut inactive = plugin.inactive_plugin_handle().unwrap();
    params.flush(&mut inactive, &turned.as_input(), &mut told.as_output());
    assert!(told.is_empty());
    let handle = plugin.plugin_handle();
    assert_eq!(params.get_value(&handle, in_time), Some(0.0));
}

#[test]
fn silent_until_something_is_picked_and_starts_again_cleanly() {
    let entry = entry();
    let mut plugin = instance(&entry);
    let configuration = PluginAudioConfiguration { sample_rate: 48_000.0, min_frames_count: 1, max_frames_count: 64 };
    // The DAW may start and stop a track's audio many times.
    for _ in 0..20 {
        let processor = plugin.activate(|_, _| (), configuration).unwrap();
        let mut processor = processor.start_processing().unwrap();
        let mut out = [vec![9.0f32; 64], vec![9.0f32; 64]];
        let mut out_ports = AudioPorts::with_capacity(2, 1);
        let mut outputs = out_ports.with_output_buffers([AudioPortBuffer {
            channels: AudioPortBufferType::f32_output_only(out.iter_mut().map(|b| b.as_mut_slice())),
            latency: 0,
        }]);
        let inputs = InputAudioBuffers::empty();
        let (events_in, mut events_out) = (EventBuffer::new(), EventBuffer::new());
        let status = processor
            .process(&inputs, &mut outputs, &events_in.as_input(), &mut events_out.as_output(), None, None)
            .unwrap();
        assert_eq!(status, ProcessStatus::Continue);
        assert!(out.iter().all(|side| side.iter().all(|s| *s == 0.0)));
        plugin.deactivate(processor.stop_processing());
    }
}

/// The stand-in DAW's side of preset discovery: what the plugin declares and lists.
#[derive(Default)]
struct Browser {
    places: Vec<(String, bool)>,
    types: Vec<String>,
    presets: Vec<String>,
}

impl clack_extensions::preset_discovery::prelude::IndexerImpl for Browser {
    fn declare_filetype(
        &mut self,
        file_type: clack_extensions::preset_discovery::prelude::FileType,
    ) -> Result<(), HostError> {
        self.types.push(file_type.file_extension.map(|e| e.to_string_lossy().into_owned()).unwrap_or_default());
        Ok(())
    }

    fn declare_location(
        &mut self,
        location: clack_extensions::preset_discovery::prelude::LocationInfo,
    ) -> Result<(), HostError> {
        use clack_extensions::preset_discovery::prelude::Location;
        self.places.push((location.name.to_string_lossy().into_owned(), location.location == Location::Plugin));
        Ok(())
    }

    fn declare_soundpack(
        &mut self,
        _: clack_extensions::preset_discovery::prelude::Soundpack,
    ) -> Result<(), HostError> {
        Ok(())
    }
}

impl clack_extensions::preset_discovery::prelude::MetadataReceiverImpl for Browser {
    fn on_error(&mut self, _: i32, _: Option<&std::ffi::CStr>) {}
    fn begin_preset(&mut self, name: Option<&std::ffi::CStr>, _: Option<&std::ffi::CStr>) -> Result<(), HostError> {
        self.presets.push(name.map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        Ok(())
    }
    fn add_plugin_id(&mut self, _: clack_extensions::preset_discovery::prelude::UniversalPluginId) {}
    fn set_soundpack_id(&mut self, _: &std::ffi::CStr) {}
    fn set_flags(&mut self, _: clack_extensions::preset_discovery::prelude::Flags) {}
    fn add_creator(&mut self, _: &std::ffi::CStr) {}
    fn set_description(&mut self, _: &std::ffi::CStr) {}
    fn set_timestamps(
        &mut self,
        _: Option<clack_extensions::preset_discovery::prelude::Timestamp>,
        _: Option<clack_extensions::preset_discovery::prelude::Timestamp>,
    ) {
    }
    fn add_feature(&mut self, _: &std::ffi::CStr) {}
    fn add_extra_info(&mut self, _: &std::ffi::CStr, _: &std::ffi::CStr) {}
}

#[test]
fn lists_favourites_in_the_daws_browser() {
    use clack_extensions::preset_discovery::prelude::{Location, PresetDiscoveryFactory, Provider};
    let entry = entry();
    let factory = entry.get_factory::<PresetDiscoveryFactory>().expect("a preset discovery factory");
    let ids: Vec<&[u8]> = factory.provider_descriptors().map(|d| d.id().unwrap().to_bytes()).collect();
    assert_eq!(ids, [mo_plugin::favourites::PROVIDER_ID.as_bytes(), mo_plugin::preset_files::PROVIDER_ID.as_bytes()]);
    let info = HostInfo::new("Stand-in DAW", mo_plugin::TESTS_VENDOR, "", "1.0").unwrap();
    let id = std::ffi::CString::new(mo_plugin::favourites::PROVIDER_ID).unwrap();
    let mut provider = Provider::instantiate(Browser::default(), &entry, &id, &info).unwrap();
    assert_eq!(provider.indexer().places, [("Favourite sounds".to_owned(), true)]);
    // The stand-in DAW has no library (tests never read a real folder): nothing listed, and no error.
    let mut browser = Browser::default();
    provider.get_metadata(Location::Plugin, &mut browser);
    assert!(browser.presets.is_empty());

    // The preset files: the types the DAW is to hand back, and no folders in the stand-in DAW.
    let id = std::ffi::CString::new(mo_plugin::preset_files::PROVIDER_ID).unwrap();
    let mut provider = Provider::instantiate(Browser::default(), &entry, &id, &info).unwrap();
    assert_eq!(provider.indexer().types, ["vstpreset", "fxp", "fxb"]);
    assert!(provider.indexer().places.is_empty());
    // A file that isn't a preset is described as nothing.
    let mut browser = Browser::default();
    let not_one = std::ffi::CString::new(env!("CARGO_MANIFEST_DIR").to_owned() + "/Cargo.toml").unwrap();
    provider.get_metadata(Location::File { path: &not_one }, &mut browser);
    assert!(browser.presets.is_empty());
}

#[test]
fn a_preset_that_isnt_the_plugins_doesnt_load() {
    use clack_extensions::preset_discovery::prelude::{Location, PluginPresetLoad};
    let entry = entry();
    let mut plugin = instance(&entry);
    let mut handle = plugin.plugin_handle();
    let load = handle.get_extension::<PluginPresetLoad>().expect("preset load");
    let handle = &mut handle;
    assert!(load.load_from_location(handle, Location::Plugin, Some(c"not a sound")).is_err());
    assert!(load.load_from_location(handle, Location::File { path: c"/a.wav" }, None).is_err());
}

/// Chord check's Thru: what's played in (CLAP's own notes and MIDI 1.0) goes on out of the note output as it came, in
/// the order of its place in the block, and the plugin stays silent.
#[test]
fn notes_played_in_go_on_out() {
    use clack_host::events::event_types::{MidiEvent, NoteOnEvent};
    use clack_host::events::spaces::CoreEventSpace;
    use clack_host::events::{Match, Pckn};
    let entry = entry();
    let mut plugin = instance(&entry);
    let configuration = PluginAudioConfiguration { sample_rate: 48_000.0, min_frames_count: 1, max_frames_count: 64 };
    let processor = plugin.activate(|_, _| (), configuration).unwrap();
    let mut processor = processor.start_processing().unwrap();
    let mut out = [vec![0.0f32; 64], vec![0.0f32; 64]];
    let mut out_ports = AudioPorts::with_capacity(2, 1);
    let mut outputs = out_ports.with_output_buffers([AudioPortBuffer {
        channels: AudioPortBufferType::f32_output_only(out.iter_mut().map(|b| b.as_mut_slice())),
        latency: 0,
    }]);
    let inputs = InputAudioBuffers::empty();
    let mut events_in = EventBuffer::new();
    events_in.push(&NoteOnEvent::new(3, Pckn::new(0u16, 0u16, 60u16, Match::<u32>::All), 0.8));
    events_in.push(&MidiEvent::new(10, 0, [0x91, 64, 100]));
    let mut events_out = EventBuffer::new();
    processor.process(&inputs, &mut outputs, &events_in.as_input(), &mut events_out.as_output(), None, None).unwrap();
    let out_notes: Vec<(u32, u16, u16)> = events_out
        .iter()
        .filter_map(|e| match e.as_core_event() {
            Some(CoreEventSpace::NoteOn(n)) => {
                Some((n.header().time(), *n.channel().as_specific()?, *n.key().as_specific()?))
            }
            // MIDI goes on as it came.
            Some(CoreEventSpace::Midi(m)) => {
                let [status, key, _] = m.data();
                Some((m.header().time(), u16::from(status & 0x0F), u16::from(key)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(out_notes, [(3, 0, 60), (10, 1, 64)]);
    assert!(out.iter().all(|side| side.iter().all(|s| *s == 0.0)));
    plugin.deactivate(processor.stop_processing());
}
