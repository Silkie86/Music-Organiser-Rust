//! Standard MIDI Files (.mid, types 0, 1 and 2, and .rmi's RIFF wrapping), as the C# app's `MidiFileReader` reads them.
//! Velocities are scaled up to MIDI 2.0's 16 bits by its own rule (rule 22), so each comes back as it was written.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;

use super::MidiPart;
use crate::model::{NoteEvent, velocity};
use crate::paths;

/// The endings the MIDI library looks for.
pub const EXTENSIONS: [&str; 4] = [".mid", ".midi", ".smf", ".kar"];

/// Whether a path ends in one of [`EXTENSIONS`], letter case aside.
pub fn is_midi_path(path: &str) -> bool {
    let ext = paths::extension(path);
    EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(ext))
}

/// Reads a MIDI file from disk. Its parts are named against the file's own name (a track named after the file isn't
/// named again).
pub fn read(path: &Path) -> io::Result<Vec<MidiPart>> {
    let raw = std::fs::read(path)?;
    read_bytes(&raw, paths::stem(&path.to_string_lossy()))
}

/// The parts in a MIDI file's bytes. Bytes that aren't a MIDI file are an error; a file damaged part-way is read up
/// to the damage.
pub fn read_bytes(data: &[u8], file_name: &str) -> io::Result<Vec<MidiPart>> {
    // RIFF-wrapped MIDI (.rmi): the file sits in the "data" chunk.
    let data = match data {
        [b'R', b'I', b'F', b'F', _, _, _, _, b'R', b'M', b'I', b'D', ..] if data.len() > 20 => {
            data.get(20..).unwrap_or_default()
        }
        _ => data,
    };
    let not_midi = || io::Error::new(io::ErrorKind::InvalidData, "Not a MIDI file");
    if data.len() < 14 || !data.starts_with(b"MThd") {
        return Err(not_midi());
    }
    let header_length = i32_at(data, 4);
    let track_count = u16::from_be_bytes([byte(data, 10), byte(data, 11)]);
    let division = i16::from_be_bytes([byte(data, 12), byte(data, 13)]);
    // SMPTE timing (a negative division) is rare in loop packs: its ticks a frame are taken as ticks a beat at 120.
    let ppq = if division > 0 {
        division as u16
    } else {
        let frames = -i32::from(division >> 8);
        (frames * i32::from(division & 0xff) / 2).max(24) as u16
    };

    let mut tracks = Vec::new();
    // A channel's sound is usually set once, in whichever track: the first program change for each channel counts.
    let mut programs = HashMap::new();
    let mut tempo = None;
    // Where the next chunk starts, as a wider number so a damaged length can't wrap it round.
    let mut pos = 8 + i64::from(header_length);
    let mut t = 0;
    while t < track_count {
        let (Ok(at), true) = (usize::try_from(pos), pos + 8 <= data.len() as i64) else { break };
        let length = i32_at(data, at + 4);
        let is_track = data.get(at..at + 4) == Some(b"MTrk");
        let body_len = (length.max(0) as usize).min(data.len() - at - 8);
        let body = data.get(at + 8..at + 8 + body_len).unwrap_or_default();
        pos += 8 + i64::from(length.max(0));
        // A chunk of another kind isn't a track: skip it.
        if is_track {
            tracks.push(read_track(body, &mut tempo, &mut programs));
            t += 1;
        }
    }

    let named = tracks.iter().filter(|t| !t.by_channel.is_empty()).count();
    let mut parts = Vec::new();
    for track in tracks {
        let mut channels: Vec<(u8, Vec<NoteEvent>)> = track.by_channel.into_iter().collect();
        channels.sort_by_key(|(c, _)| *c);
        let split = channels.len() > 1;
        for (channel, mut notes) in channels {
            if notes.is_empty() {
                continue;
            }
            let mut label = if !track.name.is_empty() && !eq_ignore_case(&track.name, file_name) {
                track.name.clone()
            } else {
                String::new()
            };
            if split {
                if !label.is_empty() {
                    label += " · ";
                }
                label += &format!("Ch {}", channel + 1);
            }
            // Stable, as .NET's OrderBy is: notes at the same place and key keep the order they ended in.
            notes.sort_by_key(|n| (n.start, n.key));
            parts.push(MidiPart {
                name: if named == 1 && !split { String::new() } else { label },
                ppq,
                notes,
                drums: channel == 9,
                tempo,
                program: programs.get(&channel).copied(),
                track_name: track.name.clone(),
            });
        }
    }
    Ok(parts)
}

/// A track's name and its notes by channel.
struct Track {
    name: String,
    by_channel: HashMap<u8, Vec<NoteEvent>>,
}

/// When a held note started, and how hard (MIDI 1.0's velocity).
type Struck = (i64, u8);

/// Notes still held, by (channel, key), with when each started and how hard; in the order the keys were first
/// struck, so notes never released end in the same order as in the C# app.
#[derive(Default)]
struct Held {
    index: HashMap<(u8, u8), usize>,
    keys: Vec<((u8, u8), VecDeque<Struck>)>,
}

impl Held {
    /// A key struck: its queue, started when it's the first time.
    fn strike(&mut self, key: (u8, u8), start: i64, vel: u8) {
        let i = *self.index.entry(key).or_insert_with(|| {
            self.keys.push((key, VecDeque::new()));
            self.keys.len() - 1
        });
        if let Some((_, queue)) = self.keys.get_mut(i) {
            queue.push_back((start, vel));
        }
    }

    /// A key released: the earliest of its notes still held, if any (a release of a key never struck starts nothing).
    fn release(&mut self, key: (u8, u8)) -> Option<Struck> {
        let i = *self.index.get(&key)?;
        self.keys.get_mut(i)?.1.pop_front()
    }
}

fn read_track(s: &[u8], tempo: &mut Option<f64>, programs: &mut HashMap<u8, u8>) -> Track {
    let mut name = String::new();
    let mut by_channel: HashMap<u8, Vec<NoteEvent>> = HashMap::new();
    // A key struck again before it's released stacks, so each release ends the earliest one.
    let mut held = Held::default();
    let mut tick: i64 = 0;
    let mut i = 0;
    let (mut status, mut running) = (0u8, 0u8);
    let add = |by_channel: &mut HashMap<u8, Vec<NoteEvent>>, channel: u8, start: i64, end: i64, key: u8, vel: u8| {
        by_channel.entry(channel).or_default().push(NoteEvent {
            start,
            length: (end - start).max(1),
            key: key & 0x7f,
            velocity: velocity::from_7(vel.clamp(1, 127)),
            muted: false,
            step: false,
        });
    };
    while i < s.len() {
        tick += vlq(s, &mut i);
        let Some(&b) = s.get(i) else { break };
        if b >= 0x80 {
            status = b;
            i += 1;
            if b < 0xf0 {
                running = b;
            }
        } else if status < 0x80 {
            // Data with no status to run on: the track is damaged from here.
            break;
        }
        if status == 0xff {
            let Some(&kind) = s.get(i) else { break };
            i += 1;
            let len = vlq(s, &mut i) as usize;
            let Some(payload) = s.get(i..i + len) else { break };
            match kind {
                0x03 if name.is_empty() => name = text(payload),
                0x51 if len == 3 && tempo.is_none() => {
                    let us = u32::from_be_bytes([0, byte(payload, 0), byte(payload, 1), byte(payload, 2)]);
                    if us > 0 {
                        // .NET's Math.Round(x, 2): scaled, rounded half to even, scaled back.
                        *tempo = Some((60_000_000.0 / f64::from(us) * 100.0).round_ties_even() / 100.0);
                    }
                }
                0x2f => break,
                _ => {}
            }
            i += len;
            // Meta and sysex events should end running status, but plenty of files (karaoke files with their lyrics,
            // for one) carry on with it after them: going on with the last notes' status reads those.
            status = running;
            continue;
        }
        if status == 0xf0 || status == 0xf7 {
            let len = vlq(s, &mut i) as usize;
            i = i.saturating_add(len);
            status = running;
            continue;
        }
        let kind = status & 0xf0;
        let channel = status & 0x0f;
        let data_bytes = if kind == 0xc0 || kind == 0xd0 { 1 } else { 2 };
        let Some(d) = s.get(i..i + data_bytes) else { break };
        let (d1, d2) = (byte(d, 0), if data_bytes == 2 { byte(d, 1) } else { 0 });
        i += data_bytes;
        if kind == 0xc0 {
            programs.entry(channel).or_insert(d1 & 0x7f);
        } else if kind == 0x90 && d2 > 0 {
            held.strike((channel, d1), tick, d2);
        } else if (kind == 0x80 || kind == 0x90)
            && let Some((start, vel)) = held.release((channel, d1))
        {
            add(&mut by_channel, channel, start, tick, d1, vel);
        }
    }
    // Notes never released end with the track.
    for ((channel, key), queue) in held.keys {
        for (start, vel) in queue {
            add(&mut by_channel, channel, start, tick, key, vel);
        }
    }
    Track { name: name.trim().to_owned(), by_channel }
}

/// A variable-length number: seven bits a byte, at most four bytes.
fn vlq(s: &[u8], i: &mut usize) -> i64 {
    let mut v = 0i64;
    for _ in 0..4 {
        let Some(&b) = s.get(*i) else { break };
        *i += 1;
        v = (v << 7) | i64::from(b & 0x7f);
        if b & 0x80 == 0 {
            break;
        }
    }
    v
}

/// A track name: UTF-8, or Latin-1 when it isn't (keeping odd bytes readable), without zero bytes or spaces round it.
fn text(b: &[u8]) -> String {
    let s = match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    };
    s.trim_matches(['\0', ' ']).to_owned()
}

fn byte(d: &[u8], at: usize) -> u8 {
    d.get(at).copied().unwrap_or(0)
}

fn i32_at(d: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([byte(d, at), byte(d, at + 1), byte(d, at + 2), byte(d, at + 3)])
}

/// Equal letter case aside, as .NET's `OrdinalIgnoreCase` compares: letter by letter, each by its one-letter capital.
fn eq_ignore_case(a: &str, b: &str) -> bool {
    let upper = |c: char| {
        let mut u = c.to_uppercase();
        match (u.next(), u.next()) {
            (Some(one), None) => one,
            _ => c,
        }
    };
    a.chars().map(upper).eq(b.chars().map(upper))
}
