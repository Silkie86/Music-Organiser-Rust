//! The plugin loaded by a small stand-in DAW (clack's host side), as Bitwig would load it: it must say what it is,
//! give one stereo output and one note input, offer its window, and stay silent until something is picked. No real
//! DAW, folder or sound card is touched (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, PluginGui};
use clack_extensions::note_ports::{NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::state::PluginState;
use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use clack_plugin::entry::SinglePluginEntry;
use mo_plugin::MoPlugin;

fn entry() -> PluginEntry {
    PluginEntry::load_from_clack::<SinglePluginEntry<MoPlugin>>(c"").unwrap()
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
fn one_stereo_output_one_note_input_and_a_window() {
    let entry = entry();
    let mut plugin = instance(&entry);
    let handle = plugin.plugin_handle();

    let audio = handle.get_extension::<PluginAudioPorts>().unwrap();
    assert_eq!((audio.count(&handle, true), audio.count(&handle, false)), (0, 1));
    let mut buffer = AudioPortInfoBuffer::new();
    let port = audio.get(&handle, 0, false, &mut buffer).unwrap();
    assert_eq!(port.channel_count, 2);

    let notes = handle.get_extension::<PluginNotePorts>().unwrap();
    assert_eq!((notes.count(&handle, true), notes.count(&handle, false)), (1, 0));
    let mut buffer = NotePortInfoBuffer::new();
    assert!(notes.get(&handle, 0, true, &mut buffer).is_some());

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
