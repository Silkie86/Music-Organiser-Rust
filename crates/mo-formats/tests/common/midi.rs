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
