//! Made-up Standard MIDI Files, byte by byte, as the C# tests build them.

/// A file around its tracks: type 1 (or 0 with one track) at the given ticks a beat (or SMPTE division).
pub fn smf(division: i16, tracks: &[&[u8]]) -> Vec<u8> {
    let kind: u16 = if tracks.len() == 1 { 0 } else { 1 };
    let mut out = b"MThd".to_vec();
    out.extend(6u32.to_be_bytes());
    out.extend(kind.to_be_bytes());
    out.extend((tracks.len() as u16).to_be_bytes());
    out.extend(division.to_be_bytes());
    for t in tracks {
        out.extend(chunk(b"MTrk", t));
    }
    out
}

/// A chunk: its four-letter kind, its length and its bytes.
pub fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(body);
    out
}

/// A track name (meta event 3) at the start of a track.
pub fn name(text: &str) -> Vec<u8> {
    let mut out = vec![0x00, 0xff, 0x03, text.len() as u8];
    out.extend(text.as_bytes());
    out
}

/// The end of a track.
pub const END: [u8; 4] = [0x00, 0xff, 0x2f, 0x00];

/// One track: a note on the channel, after a program change and a track name.
pub fn track_with_program(channel: u8, program: Option<u8>, track_name: &str) -> Vec<u8> {
    let mut t = name(track_name);
    if let Some(p) = program {
        t.extend([0x00, 0xc0 | channel, p]);
    }
    t.extend([0x00, 0x90 | channel, 36, 100, 0x60, 0x80 | channel, 36, 0]);
    t.extend(END);
    t
}

/// A type 1 file: a tempo track (120.5 a minute) and two named tracks, keys and drums.
pub fn two_tracks() -> Vec<u8> {
    // 60,000,000 / 120.5 = 497,925 microseconds a beat.
    let us: u32 = 497_925;
    let mut tempo = vec![0x00, 0xff, 0x51, 0x03];
    tempo.extend(&us.to_be_bytes()[1..]);
    tempo.extend(END);
    let mut keys = name("Keys");
    keys.extend([0x00, 0xc0, 4, 0x00, 0x90, 64, 90, 0x00, 60, 70, 0x81, 0x40, 0x80, 60, 0, 0x00, 64, 0]);
    keys.extend(END);
    let mut drums = name("Drums");
    drums.extend([0x00, 0x99, 36, 127, 0x30, 0x89, 36, 0]);
    drums.extend(END);
    smf(192, &[&tempo, &keys, &drums])
}

/// A type 0 file, one track named after the file holding two channels.
pub fn one_track_two_channels(track_name: &str) -> Vec<u8> {
    let mut t = name(track_name);
    t.extend([0x00, 0x90, 60, 80, 0x00, 0x91, 48, 81, 0x60, 0x80, 60, 0, 0x00, 0x81, 48, 0]);
    t.extend(END);
    smf(96, &[&t])
}

/// Notes struck twice before release, and one never released (it ends with the track).
pub fn stacked_and_unreleased() -> Vec<u8> {
    let t = [
        0x00, 0x90, 60, 100, // C on
        0x10, 60, 50, // C again, running status
        0x10, 60, 0, // the first C ends
        0x10, 62, 30, // D, never released
        0x10, 60, 0, // the second C ends
        0x20, 0xff, 0x2f, 0x00,
    ];
    smf(96, &[&t])
}

/// A MIDI Clip File: `SMF2CLIP`, then its messages' words high byte first.
pub fn clip_file(words: &[u32]) -> Vec<u8> {
    let mut out = b"SMF2CLIP".to_vec();
    for w in words {
        out.extend(w.to_be_bytes());
    }
    out
}

/// Delta Clockstamp: the ticks before the next message.
pub fn dc(ticks: u32) -> u32 {
    0x0040_0000 | ticks
}

/// Delta Clockstamp Ticks Per Quarter Note.
pub fn dctpq(ppq: u16) -> u32 {
    0x0030_0000 | u32::from(ppq)
}

pub const NOOP: u32 = 0;
pub const START_OF_CLIP: [u32; 4] = [0xf020_0000, 0, 0, 0];
pub const END_OF_CLIP: [u32; 4] = [0xf021_0000, 0, 0, 0];

/// Flex Data Set Tempo, in beats a minute.
pub fn set_tempo(bpm: f64) -> [u32; 4] {
    [0xd010_0000, (6_000_000_000.0 / bpm).round() as u32, 0, 0]
}

/// Flex Data's clip name (metadata text, status 3), in as many parts of 12 letters as it takes.
pub fn clip_name(name: &str) -> Vec<u32> {
    let parts: Vec<&[u8]> = name.as_bytes().chunks(12).collect();
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let format: u32 = match (parts.len(), i) {
            (1, _) => 0,
            (_, 0) => 1,
            (n, i) if i == n - 1 => 3,
            _ => 2,
        };
        let mut bytes = [0u8; 12];
        bytes[..part.len()].copy_from_slice(part);
        out.push(0xd010_0103 | format << 22);
        for w in bytes.chunks(4) {
            out.push(u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
        }
    }
    out
}

/// A MIDI 2.0 note on: group, channel, key, 16-bit velocity, and an attribute (type, data).
pub fn note_on2(group: u8, channel: u8, key: u8, velocity: u16, attribute: (u8, u16)) -> [u32; 2] {
    let w0 = 0x4090_0000 | u32::from(group) << 24 | u32::from(channel) << 16 | u32::from(key) << 8;
    [w0 | u32::from(attribute.0), u32::from(velocity) << 16 | u32::from(attribute.1)]
}

/// A MIDI 2.0 note off.
pub fn note_off2(group: u8, channel: u8, key: u8) -> [u32; 2] {
    [0x4080_0000 | u32::from(group) << 24 | u32::from(channel) << 16 | u32::from(key) << 8, 0]
}

/// A MIDI 2.0 program change.
pub fn program2(group: u8, channel: u8, program: u8) -> [u32; 2] {
    [0x40c0_0000 | u32::from(group) << 24 | u32::from(channel) << 16, u32::from(program) << 24]
}

/// A MIDI 1.0 message in a packet: group, status byte and its two data bytes.
pub fn midi1(group: u8, status: u8, d1: u8, d2: u8) -> u32 {
    0x2000_0000 | u32::from(group) << 24 | u32::from(status) << 16 | u32::from(d1) << 8 | u32::from(d2)
}

/// A clip: the configuration header (ticks a quarter note, tempo, name), then its messages between Start and End of
/// Clip; `body` is the clip's words with their Delta Clockstamps.
pub fn clip(ppq: u16, bpm: f64, name: &str, body: &[u32], end_after: u32) -> Vec<u8> {
    let mut w = vec![dc(0), dctpq(ppq), dc(0)];
    w.extend(set_tempo(bpm));
    for part in clip_name(name).chunks(4) {
        w.push(dc(0));
        w.extend(part);
    }
    w.push(dc(0));
    w.extend(START_OF_CLIP);
    w.extend(body);
    w.push(dc(end_after));
    w.extend(END_OF_CLIP);
    clip_file(&w)
}

/// A clip of one bar at 480 a beat: a MIDI 2.0 chord on channel 1 (one note with a pitch attribute), a MIDI 1.0
/// bass on channel 2 of group 2, and a kick on channel 10.
pub fn clip_two_groups() -> Vec<u8> {
    let mut b = vec![dc(0)];
    b.extend(program2(0, 0, 4));
    b.push(dc(0));
    b.extend(note_on2(0, 0, 60, 0xc000, (0, 0)));
    b.push(dc(0));
    b.extend(note_on2(0, 0, 64, 0x8000, (3, 64 << 9)));
    b.push(dc(0));
    b.push(midi1(1, 0x91, 36, 100));
    b.push(dc(0));
    b.extend(note_on2(0, 9, 36, 0xffff, (0, 0)));
    b.push(dc(120));
    b.extend(note_off2(0, 9, 36));
    b.push(dc(360));
    b.extend(note_off2(0, 0, 60));
    b.push(dc(0));
    b.extend(note_off2(0, 0, 64));
    b.push(dc(480));
    b.push(midi1(1, 0x91, 36, 0));
    clip(480, 92.5, "Night Drive Verse Chords", &b, 960)
}
