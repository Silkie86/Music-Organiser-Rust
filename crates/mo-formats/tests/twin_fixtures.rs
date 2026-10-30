//! Writes the made-up project files to a folder, so the twin test can read them with both apps (CI runs this with
//! `MO_TWIN_FIXTURES` set). Without it, it only checks that every file still builds.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::bitwig::*;
use common::midi::*;
use common::studio_one::*;
use common::*;

fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let intro_and_drop = [playlist_item(0, 2, 384, -1), playlist_item(768, 1, 192, 96)].concat();
    let project = fl_studio_project();
    // FL 7: 20-byte notes, so these two divide into five.
    let fl_7 = Flp::new()
        .ascii(199, "7.0.2")
        .word(66, 140)
        .word(64, 0)
        .byte(21, 0)
        .ascii(192, "Kick")
        .ascii(196, r"C:\Samples\Kick.wav")
        .word(65, 1)
        .data(224, &[note(0, 0, 96, 60, 100), note(96, 0, 96, 60, 100)].concat()[..40])
        .data(236, &[0; 12])
        .build();
    let mut garbled = fl_7.clone();
    garbled[14..22].copy_from_slice(&[0x60, 0xfb, 0x12, 0x00, 0xd9, 0x4a, 0x06, 0x01]);
    vec![
        ("fl 7.flp", fl_7),
        ("fl 7 garbled header.flp", garbled),
        ("cut short.flp", project[..project.len() - 30].to_vec()),
        ("version 9 bare.flp", Flp::new().ascii(199, "9").ascii(194, "Kick").build()),
        ("project.flp", fl_studio_project()),
        ("project placed.flp", fl_studio_project_with(Some(&intro_and_drop))),
        ("fl 9 chunk.flp", fl_studio_9_project(&[1; 10])),
        ("fl 9 big chunk.flp", fl_studio_9_project(&[3; 300])),
        ("fl 9 steps.flp", fl_studio_9_steps()),
        ("fl 25 project.flp", fl_studio_25_project()),
        ("fl 25 nothing saved.flp", fl_studio_25_nothing_saved()),
        ("fl 25 unplayed.flp", fl_studio_25_unplayed(false)),
        ("fl 25 unplayed hat plays.flp", fl_studio_25_unplayed(true)),
        ("fl 25 muted tracks.flp", fl_studio_25_muted_tracks()),
        ("fl 25 automation.flp", fl_studio_25_automation(false, false)),
        ("fl 25 automation twice.flp", fl_studio_25_automation(true, false)),
        ("fl 25 automation off.flp", fl_studio_25_automation(false, true)),
        ("fl 25 switched and mixer.flp", fl_studio_25_switched_and_mixer(false)),
        ("fl 25 switched and mixer more.flp", fl_studio_25_switched_and_mixer(true)),
        ("fl 25 back to back.flp", fl_studio_25_back_to_back(false)),
        ("fl 25 back to back linked first.flp", fl_studio_25_back_to_back(true)),
        ("fl 25 two ways.flp", fl_studio_25_one_sample_two_ways(r"C:\Samples\Snare 01.wav")),
        ("fl 25 audio only.flp", fl_studio_25_audio_only()),
    ]
}

/// The Bitwig files, each in a project folder of its own where Bitwig keeps it so (its audio is in the folders beside
/// it), and the audio files in those folders.
fn bitwig_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let project = bitwig_project_bytes();
    let master_plugin = Template { master_plugin: true, ..Template::default() };
    vec![
        ("bitwig/Loop Idea/Loop Idea.bwproject", project.clone()),
        ("bitwig/Loop Idea/samples/Recorded Bass.wav", vec![0]),
        ("bitwig/Loop Idea/samples/Drums/Hat.wav", vec![0]),
        ("bitwig/Loop Idea/bounce/Loop Idea.wav", vec![0]),
        ("bitwig/Undecodable/Undecodable.bwproject", bitwig_undecodable_bytes("Undecodable")),
        ("bitwig/Undecodable/recordings/Syntakt 2-1.wav", vec![0]),
        ("bitwig/Song/Song.bwproject", bitwig_scan_paths_bytes()),
        ("bitwig/Collected/Collected.bwproject", bitwig_collected_bytes(true)),
        ("bitwig/Collected damaged/Collected damaged.bwproject", bitwig_collected_bytes(false)),
        ("bitwig/cut short.bwproject", project[..project.len() / 2].to_vec()),
        ("bitwig/template.bwproject", bitwig_template(Template::default())),
        ("bitwig/template with master plugin.bwproject", bitwig_template(master_plugin)),
        ("bitwig/not bitwig.bwproject", b"Polymer\0\0C:\\Samples\\Kick.wav\0".to_vec()),
    ]
}

/// The Studio One songs: one with notes, samples and plugins, one with each kind of insert, a song missing its song
/// file and a file that isn't a zip.
fn studio_one_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let song = studio_one_song_bytes();
    let no_song_file = stored_zip(&[("metainfo.xml", br#"<M><Attribute id="Media:Tempo" value="96"/></M>"#)]);
    vec![
        ("studio one/Groove/Groove.song", song.clone()),
        ("studio one/Inserts/Inserts.song", studio_one_inserts_bytes()),
        ("studio one/no song file.song", no_song_file),
        ("studio one/not a zip.song", b"not a zip".to_vec()),
        ("studio one/cut short.song", song[..song.len() / 2].to_vec()),
    ]
}

/// The MIDI files: the same as the MIDI reader's tests, and some damaged ones.
fn midi_fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let two = two_tracks();
    let keys = track_with_program(0, Some(1), "Keys");
    let bass = track_with_program(1, Some(33), "Bass");
    let mut odd_chunk = smf(96, &[&keys, &bass]);
    odd_chunk.splice(14..14, chunk(b"XFIH", b"made by something"));
    let mut rmi = b"RIFF".to_vec();
    rmi.extend(((two.len() + 12) as u32).to_le_bytes());
    rmi.extend(b"RMIDdata");
    rmi.extend((two.len() as u32).to_le_bytes());
    rmi.extend(&two);
    let smpte = i16::from_be_bytes([(-25i8) as u8, 40]);
    let mut latin_1 = vec![0x00, 0xff, 0x03, 5, b'C', 0xe9, b'l', b'l', b'o', 0x00, 0x90, 60, 80, 0x60, 0x80, 60, 0];
    latin_1.extend(END);
    vec![
        ("midi/Idea.mid", two.clone()),
        ("midi/Idea cut short.mid", two[..two.len() - 8].to_vec()),
        ("midi/Idea cut in its header.mid", two[..20].to_vec()),
        ("midi/Idea.rmi.mid", rmi),
        ("midi/Loop One.mid", one_track_two_channels("Loop One")),
        ("midi/Pads.midi", one_track_two_channels("Pads")),
        ("midi/Bass.MID", smf(96, &[&track_with_program(0, Some(38), "")])),
        ("midi/Night drive.kar", smf(96, &[&track_with_program(0, None, "Keyscape")])),
        ("midi/Drums.smf", smf(96, &[&track_with_program(9, Some(0), "Drums")])),
        ("midi/stacked.mid", stacked_and_unreleased()),
        ("midi/odd chunk.mid", odd_chunk),
        ("midi/smpte.mid", smf(smpte, &[&keys])),
        ("midi/latin 1.mid", smf(96, &[&latin_1, &bass])),
        ("midi/not midi.mid", b"RIFF0000WAVE".to_vec()),
        ("midi/._Idea.mid", [&[0u8, 5, 22, 7, 0, 2, 0, 0][..], &[0; 200]].concat()),
    ]
}

#[test]
fn writes_the_twin_test_files() {
    let mut files = fixtures();
    files.extend(bitwig_fixtures());
    files.extend(studio_one_fixtures());
    files.extend(midi_fixtures());
    assert!(files.iter().all(|(_, bytes)| !bytes.is_empty()));
    let Some(dir) = std::env::var_os("MO_TWIN_FIXTURES") else { return };
    for (name, bytes) in files {
        let path = std::path::Path::new(&dir).join(name);
        std::fs::create_dir_all(path.parent().expect("in a folder")).expect("make the folder");
        std::fs::write(path, bytes).expect("write a test file");
    }
}
