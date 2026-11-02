//! An FL Studio project's events, in file order, before any of them is made sense of.
//!
//! After the `FLhd` header comes an `FLdt` chunk holding a stream of events. The event's id decides how much data it
//! has: ids 0 to 63 one byte, 64 to 127 two, 128 to 191 four, and from 192 a length (seven bits a byte) then that many
//! bytes.

use std::borrow::Cow;

/// What's said of a file named .flp that holds no FL project (empty, or overwritten by disk damage).
pub const DAMAGED: &str = "This file is empty or damaged: it doesn't start the way an FL Studio project does, so there's nothing in it to read";
/// The warning for a save FL stopped writing part-way.
pub const STOPPED_PART_WAY: &str = "FL Studio stopped part-way through writing this save, so FL won't open it. \
    What it got as far as is shown here; Fix can finish it from an earlier save.";
/// The warning for a save whose data header FL didn't finish writing, though everything after it is there.
pub const GARBLED_HEADER: &str =
    "Saved by an old FL Studio that garbled the start of its data; everything after it was read as usual.";
/// The warning for a save that ends part-way through an event, or before its data does (cut short, or damaged).
pub const CUT_SHORT: &str = "This save ends part-way through (the file is shorter than it says, or damaged there); everything before that was read.";

pub const EV_SHUFFLE: u8 = 11;
pub const EV_CHANNEL_TYPE: u8 = 21;
pub const EV_CHANNEL_NEW: u8 = 64;
pub const EV_PATTERN_NEW: u8 = 65;
pub const EV_TEMPO_COARSE: u8 = 66;
pub const EV_DOT_VOLUME: u8 = 72;
pub const EV_DOT: u8 = 91;
pub const EV_DOT_SHIFT: u8 = 92;
/// Closes a mixer slot's events.
pub const EV_MIXER_SLOT: u8 = 98;
/// Tempo × 1000.
pub const EV_TEMPO: u8 = 156;
/// A four-byte id, but FL 25 and later write it with three bytes (meaning unknown).
pub const EV_3_BYTES: u8 = 172;
pub const EV_CHANNEL_NAME_OLD: u8 = 192;
pub const EV_PATTERN_NAME: u8 = 193;
pub const EV_TITLE: u8 = 194;
pub const EV_SAMPLE_PATH: u8 = 196;
pub const EV_VERSION: u8 = 199;
pub const EV_PLUGIN_INTERNAL: u8 = 201;
pub const EV_PLUGIN_NAME: u8 = 203;
/// An insert's name, just before the flags that start it.
pub const EV_INSERT_NAME: u8 = 204;
pub const EV_GENRE: u8 = 206;
pub const EV_ARTISTS: u8 = 207;
/// A plugin's state; for the Fruity Wrapper, the VST's details and state.
pub const EV_PLUGIN_STATE: u8 = 213;
/// A pattern's edit events: knob moves recorded into the pattern (PyFLP's pattern controllers), 12 bytes each: u32
/// tick in the pattern, u32 target (as an automation link's), f32 value 0..1. Found in 120 of 744 of Solomon's
/// projects, 12 to 243888 bytes.
pub const EV_PATTERN_CONTROLLERS: u8 = 223;
/// Note records.
pub const EV_PATTERN_NOTES: u8 = 224;
/// Playlist items (the arrangement).
pub const EV_PLAYLIST: u8 = 233;
/// Where a mixer insert sends its sound, just after its flags and slots: one byte per insert from the master (0), 1
/// where it goes there. FL 25 trims trailing zeros (so its length varies); older saves have 105 or 127 bytes. Seen
/// once per insert in 744 of Solomon's projects from FL 12 to 26, every set byte 1.
pub const EV_INSERT_ROUTES: u8 = 235;
/// Starts each mixer insert (0 is the master).
pub const EV_INSERT_FLAGS: u8 = 236;
/// When the project was made and how long it has been open, as two Delphi dates.
pub const EV_PROJECT_TIME: u8 = 237;

/// The key FL gives a channel that doesn't say (C5).
pub const DEFAULT_ROOT: u16 = 60;

/// One event: its id, where its data starts in the file, and the data.
#[derive(Clone, Debug, PartialEq)]
pub struct Event<'a> {
    pub id: u8,
    /// Where the data starts in the file; `None` for events the reader made itself (old step sequencer dots read as
    /// notes), which a writer copying the file must leave out.
    pub offset: Option<usize>,
    pub data: Cow<'a, [u8]>,
}

impl Event<'_> {
    /// The value of a one-, two-, three- or four-byte event (ids below 192).
    pub fn int(&self) -> u32 {
        match *self.data {
            [a] => a as u32,
            [a, b] => u16::from_le_bytes([a, b]) as u32,
            [a, b, c] => u32::from_le_bytes([a, b, c, 0]),
            [a, b, c, d, ..] => u32::from_le_bytes([a, b, c, d]),
            [] => 0,
        }
    }
}

/// A project's events, and what the file says about reading them.
#[derive(Clone, Debug)]
pub struct Flp<'a> {
    pub ppq: u16,
    /// FL's version as the file writes it ("21.2.3.4004"), or "" when it doesn't say.
    pub version: String,
    /// FL's major version (21 for "21.2.3.4004"), or 0 when the project doesn't say.
    pub major: u32,
    /// FL's minor version (2 for "21.2.3.4004"), or 0 when the project doesn't say.
    pub minor: u32,
    /// Text is UTF-16 from FL 11.5, one byte a letter before that.
    pub utf16: bool,
    pub events: Vec<Event<'a>>,
    /// Said about the project when it could still be read but something about the file was off.
    pub warning: Option<&'static str>,
}

/// The file wasn't an FL Studio project (see [`DAMAGED`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Damaged;

impl std::fmt::Display for Damaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(DAMAGED)
    }
}

impl std::error::Error for Damaged {}

/// The `N` bytes at `at`, or zeros past the end. Callers check the length first, so a test run stops on one that
/// forgot (a 0 read from nothing would look like channel 0 or insert 0); a release build reads on.
fn zero_filled<const N: usize>(d: &[u8], at: usize) -> [u8; N] {
    debug_assert!(at.checked_add(N).is_some_and(|end| end <= d.len()), "{N} bytes at {at} of {}", d.len());
    bytes_at(d, at).unwrap_or([0; N])
}

/// The `N` bytes at `at`, if they're all there.
pub(crate) fn bytes_at<const N: usize>(d: &[u8], at: usize) -> Option<[u8; N]> {
    d.get(at..)?.first_chunk().copied()
}

/// The byte at `at`, or 0 past the end.
pub(crate) fn byte_at(d: &[u8], at: usize) -> u8 {
    d.get(at).copied().unwrap_or(0)
}

/// The number at `at` (see [`zero_filled`] for past the end).
pub(crate) fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(zero_filled(d, at))
}

/// The number at `at` (see [`zero_filled`] for past the end).
pub(crate) fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(zero_filled(d, at))
}

/// The double at `at`, if the bytes are there.
pub(crate) fn f64_at(d: &[u8], at: usize) -> Option<f64> {
    bytes_at(d, at).map(f64::from_le_bytes)
}

/// The 64-bit number at `at`, if the bytes are there.
pub(crate) fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    bytes_at(d, at).map(u64::from_le_bytes)
}

/// The number at `at` (see [`zero_filled`] for past the end).
pub(crate) fn i32_at(d: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(zero_filled(d, at))
}

/// The float at `at` (see [`zero_filled`] for past the end).
pub(crate) fn f32_at(d: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(zero_filled(d, at))
}

/// The number before the first dot of a version, or 0.
fn major_of(version: &str) -> u32 {
    version.trim_end_matches('\0').split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0)
}

impl Flp<'_> {
    /// A text event's text.
    pub fn text(&self, data: &[u8]) -> String {
        let text = if self.utf16 {
            let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
            String::from_utf16_lossy(&units)
        } else {
            data.iter().map(|&b| b as char).collect()
        };
        text.trim_end_matches('\0').to_owned()
    }
}

/// Reads the events of an FL Studio project. A file cut short or damaged part-way is read up to where it goes wrong,
/// with a warning.
pub fn load(raw: &[u8]) -> Result<Flp<'_>, Damaged> {
    if raw.len() < 22 || !raw.starts_with(b"FLhd") {
        return Err(Damaged);
    }
    let header_len = i32_at(raw, 4);
    if header_len < 0 || header_len as i64 > raw.len() as i64 - 16 {
        return Err(Damaged);
    }
    let mut flp = Flp {
        ppq: u16_at(raw, 12),
        version: String::new(),
        major: 0,
        minor: 0,
        utf16: true,
        events: Vec::new(),
        warning: None,
    };
    let dt = 8 + header_len as usize;
    let mut pos = dt + 8;
    let end = if raw.get(dt..dt + 4) == Some(b"FLdt") {
        let said = pos as i64 + i32_at(raw, dt + 4) as i64;
        if said > raw.len() as i64 {
            flp.warning = Some(CUT_SHORT);
        }
        said.clamp(pos as i64, raw.len() as i64) as usize
    } else {
        // Some FL Studio 4 to 7 saves have junk (what looks like a leftover memory address and a size) where the FLdt
        // chunk's name and length belong. Their events follow it as usual, to the end of the file.
        flp.warning = Some(GARBLED_HEADER);
        raw.len()
    };

    let mut major = 0;
    while let Some(&id) = raw.get(pos).filter(|_| pos < end) {
        pos += 1;
        let size = match id {
            0..64 => Some(1),
            64..128 => Some(2),
            // FL 25 and 26 write event 172 with three bytes, not the four its id says. Reading four swallows the next
            // event and garbles what follows (the tempo, title and first pattern).
            EV_3_BYTES if major >= 25 => Some(3),
            128..192 => Some(4),
            _ => {
                // At most five bytes of seven bits each; a size that doesn't end by then is junk from a damaged file.
                let mut size = None;
                let mut n = 0usize;
                let mut shift = 0;
                while let Some(&b) = raw.get(pos).filter(|_| shift < 35 && pos < end) {
                    pos += 1;
                    n |= ((b & 0x7F) as usize) << shift;
                    if b & 0x80 == 0 {
                        size = Some(n);
                        break;
                    }
                    shift += 7;
                }
                size
            }
        };
        // A file cut short or damaged: keep what's been read.
        let Some(data) = size.filter(|&s| s <= end - pos).and_then(|s| raw.get(pos..pos + s)) else {
            flp.warning.get_or_insert(CUT_SHORT);
            break;
        };
        let size = data.len();
        if id == EV_VERSION && major == 0 {
            major = major_of(&String::from_utf8_lossy(data));
        }
        flp.events.push(Event { id, offset: Some(pos), data: Cow::Borrowed(data) });
        pos += size;
    }

    // A save with a garbled header that FL stopped writing before the mixer and playlist won't open in FL.
    if flp.warning == Some(GARBLED_HEADER) && !flp.events.iter().any(|e| e.id == EV_INSERT_FLAGS || e.id == EV_PLAYLIST)
    {
        flp.warning = Some(STOPPED_PART_WAY);
    }
    let version = flp.events.iter().find(|e| e.id == EV_VERSION).map(|e| String::from_utf8_lossy(&e.data).into_owned());
    match version {
        Some(v) => {
            flp.version = v.trim_end_matches('\0').to_owned();
            if major == 0 {
                major = major_of(&flp.version);
            }
            // "11" alone is 11.0.
            flp.minor = flp.version.split('.').nth(1).and_then(|m| m.trim().parse().ok()).unwrap_or(0);
            if major > 0 {
                flp.utf16 = major > 11 || (major == 11 && flp.minor >= 5);
            }
        }
        // Without the FLdt chunk to go by, only events that include FL's version show it's really a project.
        None if matches!(flp.warning, Some(GARBLED_HEADER | STOPPED_PART_WAY)) => return Err(Damaged),
        None => {}
    }
    flp.major = major;
    if (1..12).contains(&major) {
        steps_as_notes(&mut flp);
    }
    Ok(flp)
}

/// FL 9 and older keep the step sequencer's steps in each channel's events, after the pattern they're in (event 65):
/// a dot (WORD 91: step + 256), the step's volume (WORD 72: step, volume) and its shift (WORD 92: step, shift). Newer
/// FL saves steps as notes with no length.
///
/// This turns the dots into note events (224) placed right after them, so they're read like FL 25's steps: key C5, no
/// length, the step's volume as velocity. Odd steps are swung by the song's shuffle, and a shift moves its step in
/// 192nds of a beat; both checked against FL 25's re-save of Solomon's funk step_8 (shuffle 36 moves odd steps 3 ticks
/// at 96 PPQ, shift 24 moves a step 12). The notes made here are always 24-byte records.
fn steps_as_notes(flp: &mut Flp<'_>) {
    if !flp.events.iter().any(|e| e.id == EV_DOT) {
        return;
    }
    let ppq = flp.ppq as i64;
    let step = ppq / 4;
    let swing = flp.events.iter().find(|e| e.id == EV_SHUFFLE).map_or(0, |e| e.int() as i64) * step / 256;
    let mut events = Vec::with_capacity(flp.events.len());
    let mut channel: Option<u16> = None;
    let mut dots = std::collections::BTreeSet::new();
    let mut volumes = std::collections::HashMap::new();
    let mut shifts = std::collections::HashMap::new();
    let flush = |events: &mut Vec<Event<'_>>,
                 channel: Option<u16>,
                 dots: &mut std::collections::BTreeSet<i64>,
                 volumes: &mut std::collections::HashMap<i64, i64>,
                 shifts: &mut std::collections::HashMap<i64, i64>| {
        if let Some(ch) = channel
            && !dots.is_empty()
        {
            let mut notes = Vec::with_capacity(24 * dots.len());
            for &d in dots.iter() {
                let at =
                    d * step + if d % 2 == 1 { swing } else { 0 } + shifts.get(&d).copied().unwrap_or(0) * ppq / 192;
                let mut n = [0u8; 24];
                n[..4].copy_from_slice(&(at.max(0) as u32).to_le_bytes());
                n[4..6].copy_from_slice(&0x4000u16.to_le_bytes());
                n[6..8].copy_from_slice(&ch.to_le_bytes());
                n[12..14].copy_from_slice(&DEFAULT_ROOT.to_le_bytes());
                // Fine pitch, pan, release and mods centred, as FL writes them.
                (n[16], n[18], n[20], n[22], n[23]) = (120, 64, 64, 128, 128);
                n[21] = volumes.get(&d).copied().unwrap_or(100).clamp(0, 128) as u8;
                notes.extend_from_slice(&n);
            }
            events.push(Event { id: EV_PATTERN_NOTES, offset: None, data: Cow::Owned(notes) });
        }
        dots.clear();
        volumes.clear();
        shifts.clear();
    };
    for e in std::mem::take(&mut flp.events) {
        let v = e.int() as i64;
        match e.id {
            EV_DOT if channel.is_some() => {
                dots.insert(v & 0xff);
            }
            EV_DOT_VOLUME if channel.is_some() => {
                volumes.insert(v & 0xff, v >> 8);
            }
            EV_DOT_SHIFT if channel.is_some() => {
                shifts.insert(v & 0xff, v >> 8);
            }
            _ => {
                flush(&mut events, channel, &mut dots, &mut volumes, &mut shifts);
                if e.id == EV_CHANNEL_NEW {
                    channel = Some(v as u16);
                }
            }
        }
        events.push(e);
    }
    flush(&mut events, channel, &mut dots, &mut volumes, &mut shifts);
    flp.events = events;
}
