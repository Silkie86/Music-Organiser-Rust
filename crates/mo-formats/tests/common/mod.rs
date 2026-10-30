//! Made-up project files for the tests, built byte by byte the way each DAW writes them (none of Solomon's files are
//! in the repo). Each one copies the C# tests' file of the same name, so the twin test can compare them.

#![allow(dead_code)]

pub mod bitwig;

use std::path::PathBuf;

/// A folder of its own for one test, emptied first.
pub fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mo-formats-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("make the test folder");
    dir
}

/// Builds an FL Studio project's events, then the file around them.
pub struct Flp {
    events: Vec<u8>,
    pub ppq: u16,
}

impl Flp {
    pub fn new() -> Flp {
        Flp { events: Vec::new(), ppq: 96 }
    }

    pub fn byte(mut self, id: u8, v: u8) -> Flp {
        self.events.extend([id, v]);
        self
    }

    pub fn word(mut self, id: u8, v: u16) -> Flp {
        self.events.push(id);
        self.events.extend(v.to_le_bytes());
        self
    }

    pub fn dword(mut self, id: u8, v: u32) -> Flp {
        self.events.push(id);
        self.events.extend(v.to_le_bytes());
        self
    }

    pub fn raw(mut self, bytes: &[u8]) -> Flp {
        self.events.extend(bytes);
        self
    }

    pub fn data(mut self, id: u8, data: &[u8]) -> Flp {
        self.events.push(id);
        let mut n = data.len();
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                self.events.push(b);
                break;
            }
            self.events.push(b | 0x80);
        }
        self.events.extend(data);
        self
    }

    /// Text as FL 11.5 and later write it: UTF-16, ending in a zero.
    pub fn text(self, id: u8, s: &str) -> Flp {
        let bytes: Vec<u8> = s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        self.data(id, &bytes)
    }

    /// Text as older FL writes it: a byte a letter, ending in a zero.
    pub fn ascii(self, id: u8, s: &str) -> Flp {
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        self.data(id, &bytes)
    }

    pub fn build(&self) -> Vec<u8> {
        let mut b = b"FLhd".to_vec();
        b.extend(6i32.to_le_bytes());
        b.extend(0i16.to_le_bytes());
        b.extend(3u16.to_le_bytes());
        b.extend(self.ppq.to_le_bytes());
        b.extend(b"FLdt");
        b.extend((self.events.len() as i32).to_le_bytes());
        b.extend(&self.events);
        b
    }
}

/// The Fruity Wrapper's records for a VST.
pub struct Wrapper<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub vendor: &'a str,
    pub state: Vec<u8>,
    pub version: u32,
    pub kind: Option<u8>,
    pub four_cc: Option<&'a str>,
    pub class_id: Option<[u8; 16]>,
}

impl<'a> Wrapper<'a> {
    pub fn new(name: &'a str, path: &'a str, vendor: &'a str, state: &[u8]) -> Wrapper<'a> {
        Wrapper { name, path, vendor, state: state.to_vec(), version: 10, kind: None, four_cc: None, class_id: None }
    }

    pub fn build(&self) -> Vec<u8> {
        let mut b = self.version.to_le_bytes().to_vec();
        let mut field = |id: u32, data: &[u8]| {
            b.extend(id.to_le_bytes());
            b.extend((data.len() as u64).to_le_bytes());
            b.extend(data);
        };
        if let Some(k) = self.kind {
            field(50, &[k, 0, 0, 0]);
        }
        if let Some(cc) = self.four_cc {
            field(51, cc.as_bytes());
        }
        if let Some(id) = self.class_id {
            field(52, &id);
        }
        field(54, self.name.as_bytes());
        field(55, self.path.as_bytes());
        field(56, self.vendor.as_bytes());
        field(53, &self.state);
        b
    }
}

/// A 24-byte note record.
pub fn note(pos: u32, channel: u16, length: u32, key: u16, velocity: u8) -> Vec<u8> {
    let mut b = vec![0u8; 24];
    b[..4].copy_from_slice(&pos.to_le_bytes());
    b[4..6].copy_from_slice(&0x4000u16.to_le_bytes());
    b[6..8].copy_from_slice(&channel.to_le_bytes());
    b[8..12].copy_from_slice(&length.to_le_bytes());
    b[12..14].copy_from_slice(&key.to_le_bytes());
    b[21] = velocity;
    b
}

/// An FL 25 playlist item (80 bytes) on track slot 499 (track 1 of 500), not muted, not trimmed.
pub fn playlist_item_25(pos: u32, index: u16, length: u32) -> Vec<u8> {
    playlist_item_25_on(pos, index, length, 499, false)
}

/// An FL 25 playlist item (80 bytes): index 20480 + n is pattern n, a smaller index a channel's clip; `track` is the
/// slot FL saves (499 is track 1 of 500).
pub fn playlist_item_25_on(pos: u32, index: u16, length: u32, track: u16, muted: bool) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    b[..4].copy_from_slice(&pos.to_le_bytes());
    b[4..6].copy_from_slice(&20480u16.to_le_bytes());
    b[6..8].copy_from_slice(&index.to_le_bytes());
    b[8..12].copy_from_slice(&length.to_le_bytes());
    b[12..14].copy_from_slice(&track.to_le_bytes());
    b[18..20].copy_from_slice(&(if muted { 0x2040u16 } else { 0x0040 }).to_le_bytes());
    b[24..28].copy_from_slice(&(-1f32).to_le_bytes());
    b[28..32].copy_from_slice(&(-1f32).to_le_bytes());
    b
}

/// An FL 20 playlist item (32 bytes) placing a pattern; offset -1 = from its start.
pub fn playlist_item(pos: u32, pattern: u16, length: u32, offset: i32) -> Vec<u8> {
    let mut b = vec![0u8; 32];
    b[..4].copy_from_slice(&pos.to_le_bytes());
    b[4..6].copy_from_slice(&20480u16.to_le_bytes());
    b[6..8].copy_from_slice(&(20480 + pattern).to_le_bytes());
    b[8..12].copy_from_slice(&length.to_le_bytes());
    b[24..28].copy_from_slice(&offset.to_le_bytes());
    b[28..32].copy_from_slice(&(-1i32).to_le_bytes());
    b
}

/// An FL 25 playlist track's settings (event 238, 70 bytes): its number (1 at the top), colour, icon, then 1 at byte
/// 12 when it plays, 0 when it's muted.
pub fn playlist_track(number: u32, muted: bool) -> Vec<u8> {
    let mut b = vec![0u8; 70];
    b[..4].copy_from_slice(&number.to_le_bytes());
    b[4..8].copy_from_slice(&0x485156u32.to_le_bytes());
    b[12] = if muted { 0 } else { 1 };
    b[13..17].copy_from_slice(&1f32.to_le_bytes());
    b
}

/// A channel's levels (event 219): pan and volume out of 12800 (128 before FL 6), pitch in cents.
pub fn levels(pan: i32, volume: i32, pitch_cents: i32) -> Vec<u8> {
    let mut b = vec![0u8; 24];
    b[..4].copy_from_slice(&pan.to_le_bytes());
    b[4..8].copy_from_slice(&volume.to_le_bytes());
    b[8..12].copy_from_slice(&pitch_cents.to_le_bytes());
    b[12..16].copy_from_slice(&256i32.to_le_bytes());
    b
}

/// A sampler's settings (event 215) as FL 25 writes them, with its time stretching.
pub fn channel_params(time: i32, cents: i32, multiplier: i32, mode: i32) -> Vec<u8> {
    let v: [i32; 42] = [
        140, 0, 1, -1, 60, 1065353216, 1065353216, 1065353216, 1065353216, 1065353216, 0, 3, 0, 1024, 48, 65536, 1447,
        0, 256, 0, 16777216, 0, 0, 1, time, cents, multiplier, mode, 2, -2, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Like the C# tests' FL 25 header: version, build, and the 3-byte event 172.
fn fl_25() -> Flp {
    Flp::new()
        .ascii(199, "25.2.5.5319")
        .dword(159, 5319)
        .byte(28, 1)
        .raw(&[172, 1, 1, 0])
        .text(192, "FL Studio 25.2.5.5319")
}

/// Three sampler channels: "Snare" playing in pattern 1 (in the playlist), "Kick" with no notes at all, and "Hat"
/// playing in pattern 2, whose only clip in the playlist is muted unless `hat_plays`.
pub fn fl_studio_25_unplayed(hat_plays: bool) -> Vec<u8> {
    let playlist = [playlist_item_25(0, 20481, 384), playlist_item_25_on(384, 20482, 384, 499, !hat_plays)].concat();
    fl_25()
        .dword(156, 140_000)
        .word(65, 1)
        .text(193, "Snares")
        .data(224, &note(0, 0, 48, 60, 100))
        .word(65, 2)
        .text(193, "Hats")
        .data(224, &note(0, 2, 48, 60, 100))
        .word(64, 0)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Snare")
        .text(196, r"C:\Samples\Snare 01.wav")
        .word(64, 1)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Kick")
        .text(196, r"C:\Samples\Kick 01.wav")
        .word(64, 2)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Hat")
        .text(196, r"C:\Samples\Hat 01.wav")
        .data(233, &playlist)
        .build()
}

/// Solomon's "civil_18" laid out small: tracks 2 ("Acapellas") and 4 muted in the first arrangement, track 1 in the
/// second; samplers Snare, Hat (switched off) and Pad, and audio clip channels Acapella and Vox (Vox switched off).
pub fn fl_studio_25_muted_tracks() -> Vec<u8> {
    let playlist = [
        playlist_item_25(0, 20481, 384),
        playlist_item_25_on(384, 20481, 384, 498, false),
        playlist_item_25_on(0, 2, 768, 498, false),
        playlist_item_25_on(768, 2, 384, 496, false),
        playlist_item_25(0, 3, 384),
        playlist_item_25_on(192, 3, 384, 497, false),
        playlist_item_25_on(768, 20482, 384, 496, false),
    ]
    .concat();
    let verse = [note(0, 0, 96, 60, 100), note(0, 1, 48, 66, 100)].concat();
    let mut b = fl_25()
        .dword(156, 120_000)
        .byte(0, 0)
        .word(65, 1)
        .text(193, "Verse")
        .data(224, &verse)
        .word(65, 2)
        .text(193, "Pad")
        .data(224, &note(0, 4, 192, 64, 100))
        .word(64, 0)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Snare")
        .byte(0, 1)
        .text(196, r"C:\Samples\Snare 01.wav")
        .word(64, 1)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Hat")
        .byte(0, 0)
        .text(196, r"C:\Samples\Hat 01.wav")
        .word(64, 2)
        .byte(21, 4)
        .byte(0, 1)
        .data(215, &channel_params(0, 0, 0, 1))
        .text(203, "Acapella")
        .text(196, r"C:\Samples\Acapella.wav")
        .word(64, 3)
        .byte(21, 4)
        .byte(0, 0)
        .data(215, &channel_params(0, 0, 0, 1))
        .text(203, "Vox")
        .text(196, r"C:\Samples\Vox.wav")
        .word(64, 4)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Pad")
        .byte(0, 1)
        .text(196, r"C:\Samples\Pad.wav")
        .word(99, 0)
        .text(241, "Arrangement")
        .data(233, &playlist);
    for n in 1..=500 {
        b = b.data(238, &playlist_track(n, n == 2 || n == 4));
        if n == 2 {
            b = b.text(239, "Acapellas");
        }
    }
    b = b.word(99, 1).text(241, "Arrangement 2").data(233, &playlist_item_25(0, 20481, 384));
    for n in 1..=500 {
        b = b.data(238, &playlist_track(n, n == 1));
    }
    b.build()
}

/// FL's VST2 state: a 21-byte header in front of the plugin's own chunk.
pub fn fl_vst2_state(chunk: &[u8], program: u32) -> Vec<u8> {
    let mut b = vec![0u8; 21];
    b[..4].copy_from_slice(&(-9i32).to_le_bytes());
    b[4] = 0x0d;
    b[5..9].copy_from_slice(&(-2i32).to_le_bytes());
    b[9..13].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
    b[17..21].copy_from_slice(&program.to_le_bytes());
    b.extend(chunk);
    b
}

/// FL's VST2 state for a plugin without chunks that has several programs, as FL 10 and 21 save Tone2's FireBird: the
/// 21-byte header with the selected program, every program's values one after another, then u32 the number of programs
/// and their names, 25 bytes apart.
pub fn fl_vst2_param_bank(programs: &[&[f32]], names: &[&str], selected: i32) -> Vec<u8> {
    let mut b = vec![0u8; 21];
    b[..4].copy_from_slice(&(-9i32).to_le_bytes());
    b[4] = 0x05;
    b[5..9].copy_from_slice(&(-2i32).to_le_bytes());
    b[17..21].copy_from_slice(&selected.to_le_bytes());
    b.extend((programs.iter().map(|p| p.len()).sum::<usize>() as u32).to_le_bytes());
    b.extend(programs.iter().flat_map(|p| p.iter()).flat_map(|v| v.to_le_bytes()));
    b.extend((programs.len() as u32).to_le_bytes());
    for n in names {
        let mut name = [0u8; 25];
        name[..n.len()].copy_from_slice(n.as_bytes());
        b.extend(name);
    }
    b
}

/// FL's VST2 state for a plugin without chunks: the 21-byte header (chunk length 0, flags without 0x08), the
/// parameter count and values, then u32 1 and the program name.
pub fn fl_vst2_param_state(values: &[f32], program: &str) -> Vec<u8> {
    let mut b = vec![0u8; 21];
    b[..4].copy_from_slice(&(-9i32).to_le_bytes());
    b[4] = 0x05;
    b[5..9].copy_from_slice(&(-2i32).to_le_bytes());
    b.extend((values.len() as u32).to_le_bytes());
    b.extend(values.iter().flat_map(|v| v.to_le_bytes()));
    b.extend(1u32.to_le_bytes());
    let mut name = [0u8; 24];
    name[..program.len()].copy_from_slice(program.as_bytes());
    b.extend(name);
    b
}

/// FL 9's VST2 state: a 13-byte header (i32 -9, flags, u32 chunk length, u32 program) in front of the plugin's chunk,
/// or for a plugin without chunks, length 0 and its parameters as in [`fl_vst2_param_state`].
pub fn fl_vst2_state_fl9(chunk: &[u8], program: u32, values: Option<&[f32]>, name: &str) -> Vec<u8> {
    let mut b = (-9i32).to_le_bytes().to_vec();
    b.push(if values.is_none() { 0x0d } else { 0x05 });
    b.extend((chunk.len() as u32).to_le_bytes());
    b.extend(program.to_le_bytes());
    b.extend(chunk);
    if let Some(values) = values {
        b.extend((values.len() as u32).to_le_bytes());
        b.extend(values.iter().flat_map(|v| v.to_le_bytes()));
        b.extend(1u32.to_le_bytes());
        let mut text = [0u8; 24];
        text[..name.len()].copy_from_slice(name.as_bytes());
        b.extend(text);
    }
    b
}

/// FL's VST3 state: version 1, then records 1 (settings), 3 (component), 2 (controller) and 4 (parameter ids).
pub fn fl_vst3_state(component: &[u8], controller: &[u8], parameter_ids: &[u32]) -> Vec<u8> {
    let mut b = 1u32.to_le_bytes().to_vec();
    let mut rec = |id: u32, data: &[u8]| {
        b.extend(id.to_le_bytes());
        b.extend((data.len() as u64).to_le_bytes());
        b.extend(data);
    };
    rec(1, &[0; 64]);
    rec(3, component);
    rec(2, controller);
    let ids: Vec<u8> = (parameter_ids.len() as u32)
        .to_le_bytes()
        .into_iter()
        .chain(parameter_ids.iter().flat_map(|i| i.to_le_bytes()))
        .collect();
    rec(4, &ids);
    b
}

/// Three channels (Serum VST on "Lead", a sampler "Kick", native 3x Osc "Bass"), two patterns with notes, and a mixer
/// with Fruity Limiter and a Pro-Q 3 VST. The C# tests' "Night Drive".
pub fn fl_studio_project() -> Vec<u8> {
    fl_studio_project_with(None)
}

/// [`fl_studio_project`] with a playlist.
pub fn fl_studio_project_with(playlist: Option<&[u8]>) -> Vec<u8> {
    let notes: Vec<u8> = [note(0, 0, 96, 60, 100), note(96, 0, 96, 64, 128), note(0, 1, 24, 36, 100)].concat();
    let b = Flp::new()
        .ascii(199, "21.2.3.4004")
        .dword(156, 128_000)
        .text(194, "Night Drive (title)")
        .word(64, 0)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "Lead")
        .data(
            213,
            &Wrapper::new("Serum", r"C:\Program Files\VstPlugins\Serum_x64.dll", "Xfer Records", &[1, 2, 3, 4]).build(),
        )
        .word(64, 1)
        .byte(21, 0)
        .text(203, "Kick")
        .text(196, r"C:\Samples\Drums\Kick 01.wav")
        .word(64, 2)
        .byte(21, 2)
        .text(201, "3x Osc")
        .text(203, "Bass")
        .data(213, &[9, 9, 9])
        .word(65, 1)
        .text(193, "Intro")
        .data(224, &notes)
        .word(65, 2)
        .text(193, "Drop")
        .data(224, &note(0, 2, 192, 41, 64))
        .data(236, &[0, 0, 0, 0])
        .text(201, "Fruity Limiter")
        .data(213, &[5, 5])
        .word(98, 0)
        .text(201, "Fruity Wrapper")
        .data(213, &Wrapper::new("Pro-Q 3", r"C:\VstPlugins\FabFilter Pro-Q 3.dll", "FabFilter", &[7, 7]).build())
        .word(98, 1);
    match playlist {
        Some(p) => b.data(233, p).build(),
        None => b.build(),
    }
}

/// Like Solomon's funk step_8 (FL 9.0.0, 2010): channel "Cell" plays KORG LegacyCell, whose chunk sits behind FL 9's
/// shorter header with program 42, and the mixer has Fruity Compressor saved as parameters. FL 9 keeps no plugin path
/// or vendor, and its text is one byte a letter.
pub fn fl_studio_9_project(chunk: &[u8]) -> Vec<u8> {
    let cell = Wrapper {
        version: 6,
        kind: Some(0),
        four_cc: Some("VCLK"),
        ..Wrapper::new("LegacyCell", "", "", &fl_vst2_state_fl9(chunk, 42, None, ""))
    };
    let compressor = Wrapper {
        version: 6,
        kind: Some(0),
        four_cc: Some("CFfU"),
        ..Wrapper::new("Fruity Compressor", "", "", &fl_vst2_state_fl9(&[], 0, Some(&[0.5, 0.02, 0.5]), "Full Reset"))
    };
    Flp::new()
        .ascii(199, "9.0.0")
        .dword(156, 98_000)
        .word(64, 0)
        .byte(21, 2)
        .ascii(201, "Fruity Wrapper")
        .ascii(203, "Cell")
        .data(213, &cell.build())
        .data(236, &[0, 0, 0, 0])
        .ascii(201, "Fruity Wrapper")
        .data(213, &compressor.build())
        .word(98, 0)
        .build()
}

/// Like funk step_8's drums (FL 9.0.0): shuffle 36, and sampler "Kick" with step sequencer dots in pattern 1 kept in
/// its channel events: steps 0, 5 (volume 52) and 7 (shifted 24). The playlist (FL 9's 32-byte items, row 999 - n for
/// pattern n) plays pattern 1 from beat 8, one bar long.
pub fn fl_studio_9_steps() -> Vec<u8> {
    let mut item = vec![0u8; 32];
    item[..4].copy_from_slice(&768u32.to_le_bytes());
    item[6..8].copy_from_slice(&0x6000u16.to_le_bytes());
    item[8..12].copy_from_slice(&384u32.to_le_bytes());
    item[12..14].copy_from_slice(&998u16.to_le_bytes());
    item[24..28].copy_from_slice(&(-1f32).to_le_bytes());
    item[28..32].copy_from_slice(&(-1f32).to_le_bytes());
    Flp::new()
        .ascii(199, "9.0.0")
        .byte(11, 36)
        .dword(156, 140_000)
        .word(64, 0)
        .byte(21, 0)
        .ascii(192, "Kick")
        .ascii(196, r"C:\Samples\Kick.wav")
        .word(65, 1)
        .word(72, 5 | 52 << 8)
        .word(91, 0x100)
        .word(91, 0x105)
        .word(92, 7 | 24 << 8)
        .word(91, 0x107)
        .data(233, &item)
        .build()
}

/// Like Solomon's funk step_9 (funk step_8 saved again in FL 25.2.5): KORG LegacyCell on "Cell" gave FL an empty
/// chunk, so its state is FL's 21-byte header with chunk length 0, then 8 zero bytes. A pattern plays it in bar 1.
pub fn fl_studio_25_nothing_saved() -> Vec<u8> {
    let cell = Wrapper {
        version: 12,
        kind: Some(4),
        four_cc: Some("VCLK"),
        ..Wrapper::new(
            "LegacyCell",
            r"C:\Program Files\VstPlugins\KORG\LegacyCell.dll",
            "KORG",
            &[fl_vst2_state(&[], 0), vec![0; 8]].concat(),
        )
    };
    Flp::new()
        .ascii(199, "25.2.5.5319")
        .dword(159, 5319)
        .byte(28, 1)
        .raw(&[172, 1, 1, 0])
        .text(192, "FL Studio 25.2.5.5319")
        .dword(156, 98_000)
        .word(65, 1)
        .data(224, &note(0, 0, 96, 60, 100))
        .word(64, 0)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "Cell")
        .data(213, &cell.build())
        .data(233, &playlist_item_25(0, 20481, 384))
        .build()
}

/// An FL Studio 25 project for `<dir>/FL Studio/Projects/Shuffle.flp`: wrapper state version 12 with a VST2 (Serum), a
/// VST3 (Arcade) and a VST2 saved as parameters (Predator 2), and a sample under `%FLStudioUserData%`.
pub fn fl_studio_25_project() -> Vec<u8> {
    let arcade_id = [0x41, 0x54, 0x53, 0x56, 0x43, 0x52, 0x61, 0x44, 0x72, 0x63, 0x61, 0x64, 0x65, 0, 0, 0];
    let serum = Wrapper {
        version: 12,
        kind: Some(4),
        four_cc: Some("XsfX"),
        ..Wrapper::new(
            "Serum",
            r"C:\Program Files\VstPlugins\Xfer\Serum_x64.dll",
            "Xfer Records",
            &fl_vst2_state(&[1, 2, 3], 0),
        )
    };
    let arcade = Wrapper {
        version: 12,
        kind: Some(8),
        class_id: Some(arcade_id),
        ..Wrapper::new(
            "Arcade",
            r"C:\Program Files\Common Files\VST3\Arcade.vst3",
            "Output",
            &fl_vst3_state(&[7, 7, 7], &[8], &[10, 20, 2000003]),
        )
    };
    let predator = Wrapper {
        version: 12,
        kind: Some(0),
        four_cc: Some("2drP"),
        ..Wrapper::new(
            "Predator 2",
            r"C:\VstPlugins\Predator2.dll",
            "Rob Papen",
            &fl_vst2_param_state(&[0.25, 1.0, 0.5], "arp- sine beat"),
        )
    };
    Flp::new()
        .ascii(199, "25.1.0.4868")
        .dword(156, 126_000)
        .word(64, 0)
        .byte(21, 0)
        .text(203, "Snare")
        .text(196, r"%FLStudioUserData%\Projects\Snare 01.wav")
        .word(64, 1)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "Lead")
        .data(213, &serum.build())
        .word(64, 2)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "Keys")
        .data(213, &arcade.build())
        .word(64, 3)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "Arp")
        .data(213, &predator.build())
        .build()
}

/// An automation clip's points (event 234), each (beats since the previous point, value).
pub fn automation_points(points: &[(f64, f64)]) -> Vec<u8> {
    let mut b = vec![0u8; 21];
    b[..4].copy_from_slice(&1u32.to_le_bytes());
    b[4..8].copy_from_slice(&64u32.to_le_bytes());
    b[9..13].copy_from_slice(&4u32.to_le_bytes());
    b[13..17].copy_from_slice(&3u32.to_le_bytes());
    b[17..21].copy_from_slice(&(points.len() as u32).to_le_bytes());
    for &(step, value) in points {
        b.extend(step.to_le_bytes());
        b.extend(value.to_le_bytes());
        b.extend([0; 8]);
    }
    b
}

/// The mixer's settings (event 225): u32 0, u32 target, i32 value for each.
pub fn mixer_settings(settings: &[(u32, i32)]) -> Vec<u8> {
    settings.iter().flat_map(|&(t, v)| [0u32.to_le_bytes(), t.to_le_bytes(), v.to_le_bytes()].concat()).collect()
}

/// What an automation clip moves (event 227): its channel, and the target (channel << 16 | control).
pub fn automation_link(clip: u16, target: u32) -> Vec<u8> {
    let mut b = vec![0u8; 20];
    b[2..4].copy_from_slice(&clip.to_le_bytes());
    b[8..12].copy_from_slice(&target.to_le_bytes());
    b[12..16].copy_from_slice(&8u32.to_le_bytes());
    b
}

/// An FL 25 song at 132 bpm: a sampler "Snare" (root key A4, pitch -2 semitones, pan 25%, volume 100%) with an
/// automation clip on its volume in bar 2, and a Serum VST (root key E5) with an automation clip on parameter 205 in
/// bar 1. The pattern sits in bar 2. The volume's clip can be switched off in the Channel Rack.
pub fn fl_studio_25_automation(volume_placed_twice: bool, volume_off: bool) -> Vec<u8> {
    let serum = Wrapper {
        version: 12,
        kind: Some(4),
        four_cc: Some("XfsX"),
        ..Wrapper::new("Serum", r"C:\VstPlugins\Serum_x64.dll", "Xfer Records", &fl_vst2_state(&[1, 2, 3], 0))
    };
    let mut playlist = [
        playlist_item_25(384, 20481, 384),
        playlist_item_25_on(384, 1, 384, 498, false),
        playlist_item_25_on(0, 3, 384, 497, false),
    ]
    .concat();
    if volume_placed_twice {
        playlist.extend(playlist_item_25_on(1152, 1, 384, 498, false));
    }
    fl_25()
        .dword(156, 132_000)
        .word(65, 1)
        .data(224, &[note(0, 0, 48, 72, 100), note(0, 2, 96, 60, 100)].concat())
        .data(227, &automation_link(1, 0))
        .data(227, &automation_link(3, 2 << 16 | 0x80cd))
        .word(64, 0)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Snare")
        .dword(135, 57)
        .data(219, &levels(3200, 12800, -200))
        .text(196, r"C:\Samples\Snare 01.wav")
        .word(64, 1)
        .byte(21, 5)
        .text(201, "")
        .text(203, "Snare - Channel volume")
        .byte(0, if volume_off { 0 } else { 1 })
        .data(219, &levels(0, 12800, 0))
        .data(234, &automation_points(&[(0.0, 0.0), (4.0, 1.0)]))
        .word(64, 2)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "Serum")
        .data(213, &serum.build())
        .dword(135, 64)
        .word(64, 3)
        .byte(21, 5)
        .text(201, "")
        .text(203, "Serum - Cutoff")
        .data(234, &automation_points(&[(0.0, 0.2), (2.0, 1.0), (2.0, 0.5)]))
        .data(233, &playlist)
        .build()
}

/// Like Solomon's civil_18 and its Serum: an FL 25 song at 120 bpm whose "Bass" (Serum, routed to insert 1) is
/// switched off in the Channel Rack, but an automation clip switches it: off for beats 0-4, on 4-8, off 8-12, then
/// on, its last points a few millionths of a beat past the clip's end as FL saves them. Pattern "Bass" (notes on beats
/// 1 and 3) plays at beats 0, 4 and 10. The "Vox" audio clip channel is switched off from beat 2 by another, and plays
/// clips at beats 0-4 and 4-6. Insert 1 has Pro-Q 3 in slot 0 and WOW2, switched off, in slot 1; its fader is moved.
/// Automation clips also move Pro-Q 3's fourth setting, the tempo and insert 1's fader. FL 25 numbers its mixer
/// settings from 0x7000. With `more`: a second clip switches the bass off at beat 16 and on at 20 (the pattern plays
/// at 16 and 20 too), another switches WOW2 on and off, and the bass volume rises 0 to 1 over beats 0-4 and jumps back
/// to 0 at the clip's end, placed twice in a row; and playlist track 3, the vox's, is muted.
pub fn fl_studio_25_switched_and_mixer(more: bool) -> Vec<u8> {
    const INSERT1: u32 = 0x7040 << 16;
    let wrapper = |name: &'static str, path: &'static str, vendor: &'static str, state: u8, four_cc: &'static str| {
        Wrapper {
            version: 12,
            kind: Some(4),
            four_cc: Some(four_cc),
            ..Wrapper::new(name, path, vendor, &fl_vst2_state(&[state], 0))
        }
        .build()
    };
    let serum = Wrapper {
        version: 12,
        kind: Some(4),
        four_cc: Some("XfsX"),
        ..Wrapper::new("Serum", r"C:\VstPlugins\Serum_x64.dll", "Xfer Records", &fl_vst2_state(&[1, 2, 3], 0))
    };
    let mut b = fl_25()
        .dword(156, 120_000)
        .word(65, 1)
        .text(193, "Bass")
        .data(224, &[note(0, 0, 48, 36, 100), note(192, 0, 48, 38, 100)].concat())
        .data(227, &automation_link(1, 7))
        .data(227, &automation_link(3, 2 << 16 | 7))
        .data(227, &automation_link(4, INSERT1 | 0x8003))
        .data(227, &automation_link(5, 0x40000005))
        .data(227, &automation_link(6, INSERT1 | 0x1fc0));
    if more {
        b = b
            .data(227, &automation_link(7, 7))
            .data(227, &automation_link(8, 0x7041 << 16 | 0x1f00))
            .data(227, &automation_link(9, 0));
    }
    let clip = |b: Flp, n: u16, name: &str, points: &[(f64, f64)]| {
        b.word(64, n).byte(21, 5).byte(0, 1).text(201, "").text(203, name).data(234, &automation_points(points))
    };
    b = b
        .word(64, 0)
        .byte(21, 2)
        .byte(0, 0)
        .word(104, 1)
        .text(201, "Fruity Wrapper")
        .text(203, "Bass")
        .data(213, &serum.build());
    b = clip(b, 1, "Bass - Mute / solo", &[(0.0, 0.0), (4.0, 1.0), (4.0, 0.0), (4.000008, 0.0), (0.0, 0.97)]);
    b = b
        .word(64, 2)
        .byte(21, 4)
        .byte(0, 1)
        .data(215, &channel_params(0, 0, 0, 1))
        .text(203, "Vox")
        .text(196, r"C:\Samples\Vox.wav");
    b = clip(b, 3, "Vox - Mute / solo", &[(0.0, 1.0), (2.0, 0.0)]);
    b = clip(b, 4, "Pro-Q 3 - Gain", &[(0.0, 0.25), (4.0, 0.75)]);
    b = clip(b, 5, "Tempo", &[(0.0, 0.5), (4.0, 1.0)]);
    b = clip(b, 6, "Insert 1 - Volume", &[(0.0, 0.8), (4.0, 0.2)]);
    if more {
        b = clip(b, 7, "Bass - Mute / solo #2", &[(0.0, 0.0), (4.0, 1.0)]);
        b = clip(b, 8, "WOW2 - On", &[(0.0, 1.0), (8.0, 0.0)]);
        b = clip(b, 9, "Bass - Volume", &[(0.0, 0.0), (4.0, 1.0), (0.0, 0.0)]);
    }
    let mut playlist = vec![
        playlist_item_25(0, 20481, 384),
        playlist_item_25(384, 20481, 384),
        playlist_item_25(960, 20481, 384),
        playlist_item_25_on(0, 1, 1152, 498, false),
        playlist_item_25_on(0, 2, 384, 497, false),
        playlist_item_25_on(384, 2, 192, 497, false),
        playlist_item_25_on(0, 3, 768, 496, false),
        playlist_item_25_on(0, 4, 384, 495, false),
        playlist_item_25_on(0, 5, 384, 494, false),
        playlist_item_25_on(0, 6, 384, 493, false),
    ];
    if more {
        playlist.extend([
            playlist_item_25(1536, 20481, 384),
            playlist_item_25(1920, 20481, 384),
            playlist_item_25_on(1536, 7, 768, 492, false),
            playlist_item_25_on(0, 8, 1536, 491, false),
            playlist_item_25_on(0, 9, 384, 490, false),
            playlist_item_25_on(384, 9, 384, 490, false),
        ]);
    }
    b = b
        .data(236, &[0, 0, 0, 0])
        .data(236, &[0, 0, 0, 0])
        .text(201, "Fruity Wrapper")
        .data(213, &wrapper("Pro-Q 3", r"C:\VstPlugins\FabFilter Pro-Q 3.dll", "FabFilter", 4, "FQ3p"))
        .word(98, 0)
        .text(201, "Fruity Wrapper")
        .data(213, &wrapper("WOW2", r"C:\VstPlugins\WOW2.dll", "Sugar Bytes", 5, "SBw2"))
        .word(98, 1)
        .data(236, &[0, 0, 0, 0])
        .data(
            225,
            &mixer_settings(&[
                (0x40000000, 12800),
                (0x7000 << 16 | 0x1fc0, 12800),
                (0x7000 << 16 | 0x1fc1, 0),
                (INSERT1 | 0x1fc0, 9600),
                (INSERT1 | 0x1fc1, 0),
                (INSERT1 | 0x1f00, 1),
                (0x7041 << 16 | 0x1f00, 0),
                (0x7080 << 16 | 0x1fc0, 12800),
                (0x7080 << 16 | 0x1fc1, 0),
            ]),
        )
        .data(233, &playlist.concat());
    if more {
        for n in 1..=500 {
            b = b.data(238, &playlist_track(n, n == 3));
        }
    }
    b.build()
}

/// An FL 25 song whose "Kick" pattern plays in bars 1, 2 and 3 while two clips switch the kick, back to back: "Kick
/// off" (off throughout) in bars 1 and 3, "Kick on" (on throughout) in bar 2. `on_linked_first` swaps the order of
/// their links in the file. "Kick - Volume" (a rise from 0 to 1 over 8 beats) is placed at beat 0 and again at beat
/// 4, over the first one's second half. "Kick - Pan" (the same rise) is placed at beats 0-8, then again at 2-4 inside
/// it; "Kick - Pitch" (the same rise) at 0-8, then again at 0-4.
pub fn fl_studio_25_back_to_back(on_linked_first: bool) -> Vec<u8> {
    let (off, on) = (automation_link(1, 7), automation_link(2, 7));
    let mut b = fl_25()
        .dword(156, 120_000)
        .word(65, 1)
        .text(193, "Kick")
        .data(224, &note(0, 0, 48, 36, 100))
        .data(227, if on_linked_first { &on } else { &off })
        .data(227, if on_linked_first { &off } else { &on })
        .data(227, &automation_link(3, 0))
        .data(227, &automation_link(4, 1))
        .data(227, &automation_link(5, 4))
        .word(64, 0)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Kick")
        .byte(0, 1)
        .text(196, r"C:\Samples\Kick.wav");
    for (n, name, points) in [
        (1, "Kick off", [(0.0, 0.0), (4.0, 0.0)]),
        (2, "Kick on", [(0.0, 1.0), (4.0, 1.0)]),
        (3, "Kick - Volume", [(0.0, 0.0), (8.0, 1.0)]),
        (4, "Kick - Pan", [(0.0, 0.0), (8.0, 1.0)]),
        (5, "Kick - Pitch", [(0.0, 0.0), (8.0, 1.0)]),
    ] {
        b = b.word(64, n).byte(21, 5).byte(0, 1).text(201, "").text(203, name).data(234, &automation_points(&points));
    }
    let playlist = [
        playlist_item_25(0, 20481, 384),
        playlist_item_25(384, 20481, 384),
        playlist_item_25(768, 20481, 384),
        playlist_item_25_on(0, 1, 384, 498, false),
        playlist_item_25_on(384, 2, 384, 498, false),
        playlist_item_25_on(768, 1, 384, 498, false),
        playlist_item_25_on(0, 3, 768, 497, false),
        playlist_item_25_on(384, 3, 768, 496, false),
        playlist_item_25_on(0, 4, 768, 495, false),
        playlist_item_25_on(192, 4, 192, 494, false),
        playlist_item_25_on(0, 5, 768, 493, false),
        playlist_item_25_on(0, 5, 384, 492, false),
    ]
    .concat();
    b.data(233, &playlist).build()
}

/// One snare written two ways: channel 0 plays it as `%FLStudioUserData%\Projects\Snare 01.wav`, channel 1 (no
/// notes) has it as `full`.
pub fn fl_studio_25_one_sample_two_ways(full: &str) -> Vec<u8> {
    fl_25()
        .word(65, 1)
        .data(224, &note(0, 0, 48, 60, 100))
        .word(64, 0)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Snare")
        .text(196, r"%FLStudioUserData%\Projects\Snare 01.wav")
        .word(64, 1)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Snare too")
        .text(196, full)
        .data(233, &playlist_item_25(0, 20481, 384))
        .build()
}

/// A song made of one audio clip (channel 1, "Vox"), with a leftover pattern of notes for "Snare" never placed. Insert
/// 1 is named "Drums"; insert 2's name event holds only its end (two zero bytes).
pub fn fl_studio_25_audio_only() -> Vec<u8> {
    fl_25()
        .word(65, 1)
        .data(224, &note(0, 0, 48, 60, 100))
        .word(64, 0)
        .byte(21, 0)
        .text(201, "")
        .text(203, "Snare")
        .text(196, r"C:\Samples\Snare 01.wav")
        .word(64, 1)
        .byte(21, 4)
        .text(203, "Vox")
        .text(196, r"C:\Samples\Vox.wav")
        .data(233, &playlist_item_25(0, 1, 384))
        .data(236, &[0, 0, 0, 0])
        .text(204, "Drums")
        .data(236, &[0, 0, 0, 0])
        .data(204, &[0, 0])
        .data(236, &[0, 0, 0, 0])
        .build()
}
