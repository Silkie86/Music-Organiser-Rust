//! Writes the made-up project files to a folder, so the twin test can read them with both apps (CI runs this with
//! `MO_TWIN_FIXTURES` set). Without it, it only checks that every file still builds.

mod common;

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

#[test]
fn writes_the_twin_test_files() {
    let files = fixtures();
    assert!(files.iter().all(|(_, bytes)| !bytes.is_empty()));
    let Some(dir) = std::env::var_os("MO_TWIN_FIXTURES") else { return };
    std::fs::create_dir_all(&dir).expect("make the folder");
    for (name, bytes) in files {
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).expect("write a test file");
    }
}
