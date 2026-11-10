//! The plugin loaded by a small stand-in DAW (clack's host side), as Bitwig would load it: it must say what it is,
//! give one stereo output and one note input, offer its window, and stay silent until something is picked. No real
//! DAW, folder or sound card is touched (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, PluginGui};
use clack_extensions::note_ports::{NotePortInfoBuffer, PluginNotePorts};
use clack_host::factory::plugin::PluginFactory;
use clack_host::prelude::*;
use clack_plugin::entry::SinglePluginEntry;
use mo_plugin::MoPlugin;

fn entry() -> PluginEntry {
    PluginEntry::load_from_clack::<SinglePluginEntry<MoPlugin>>(c"").unwrap()
}

fn instance(entry: &PluginEntry) -> PluginInstance<()> {
    let info = HostInfo::new("Stand-in DAW", "Music Organiser tests", "", "1.0").unwrap();
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
