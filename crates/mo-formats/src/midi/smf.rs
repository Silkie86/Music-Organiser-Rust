//! Standard MIDI Files (.mid, types 0, 1 and 2, and .rmi's RIFF wrapping), as the C# app's `MidiFileReader` reads them.
//! Velocities are scaled up to MIDI 2.0's 16 bits by its own rule (rule 22), so each comes back as it was written.

use std::io;
use std::path::Path;

use super::{Held, MidiPart};
use crate::fl::events::byte_at as byte;
use crate::model::{NoteAttribute, NoteEvent, velocity};
use crate::paths;

/// Goes up by one whenever the reader finds more (or different) things in the same files, so every MIDI file is read
/// again (rule 7). 1 is what the C# reader finds; 2 reads a file whose header length is damaged up to the damage
/// (DIFFERENCES.md).
pub const VERSION: u32 = 2;
/// The endings the MIDI library looks for, without their dot, as the other readers give theirs.
pub const EXTENSIONS: [&str; 4] = ["mid", "midi", "smf", "kar"];

/// MIDI 1.0's 16 channels, which a Standard MIDI File's notes are on.
const CHANNELS: usize = 16;

/// Whether a path ends in one of [`EXTENSIONS`], letter case aside.
pub fn is_midi_path(path: &str) -> bool {
    paths::has_extension(path, &EXTENSIONS)
}

/// Reads a MIDI file from disk. Its parts are named against the file's own name (a track named after the file isn't
/// named again).
pub fn read(path: &Path) -> io::Result<Vec<MidiPart>> {
    read_from(&std::fs::read(path)?, path)
}

/// Reads a MIDI file's bytes, already read from `path`, naming its parts against the file's own name as [`read`] does.
pub fn read_from(raw: &[u8], path: &Path) -> io::Result<Vec<MidiPart>> {
    read_bytes(raw, paths::stem(&path.to_string_lossy()))
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
    let mut programs = [None; CHANNELS];
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

    let channels_used = |t: &Track| t.by_channel.iter().filter(|n| !n.is_empty()).count();
    let named = tracks.iter().filter(|t| channels_used(t) > 0).count();
    let mut parts = Vec::new();
    for track in tracks {
        let split = channels_used(&track) > 1;
        for (channel, mut notes) in (0u8..).zip(track.by_channel) {
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
                program: programs.get(usize::from(channel)).copied().flatten(),
                track_name: track.name.clone(),
                controllers: Vec::new(),
            });
        }
    }
    Ok(parts)
}

/// A track's name and its notes, by channel.
struct Track {
    name: String,
    by_channel: [Vec<NoteEvent>; CHANNELS],
}

/// When a held note started, and how hard (MIDI 1.0's velocity).
type Struck = (i64, u8);

fn read_track(s: &[u8], tempo: &mut Option<f64>, programs: &mut [Option<u8>; CHANNELS]) -> Track {
    let mut name = String::new();
    let mut by_channel: [Vec<NoteEvent>; CHANNELS] = Default::default();
    // A key struck again before it's released stacks, so each release ends the earliest one.
    let mut held: Held<(u8, u8), Struck> = Held::default();
    let mut tick: i64 = 0;
    let mut i = 0;
    let (mut status, mut running) = (0u8, 0u8);
    let add = |by_channel: &mut [Vec<NoteEvent>; CHANNELS], channel: u8, start: i64, end: i64, key: u8, vel: u8| {
        let Some(notes) = by_channel.get_mut(usize::from(channel)) else { return };
        notes.push(NoteEvent {
            start,
            length: (end - start).max(1),
            key: key & 0x7f,
            velocity: velocity::from_7(vel.clamp(1, 127)),
            muted: false,
            step: false,
            attribute: NoteAttribute::NONE,
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
            if let Some(program @ None) = programs.get_mut(usize::from(channel)) {
                *program = Some(d1 & 0x7f);
            }
        } else if kind == 0x90 && d2 > 0 {
            held.strike((channel, d1), (tick, d2));
        } else if (kind == 0x80 || kind == 0x90)
            && let Some((start, vel)) = held.release((channel, d1))
        {
            add(&mut by_channel, channel, start, tick, d1, vel);
        }
    }
    // Notes never released end with the track.
    for ((channel, key), (start, vel)) in held.into_held() {
        add(&mut by_channel, channel, start, tick, key, vel);
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

fn i32_at(d: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([byte(d, at), byte(d, at + 1), byte(d, at + 2), byte(d, at + 3)])
}

/// Equal letter case aside, as .NET's `OrdinalIgnoreCase` compares: letter by letter, each by its one-letter capital
/// (Unicode's simple mapping, where Rust's `to_uppercase` gives the full one).
fn eq_ignore_case(a: &str, b: &str) -> bool {
    let upper = |c: char| match c {
        // .NET keeps these two as they are, where Unicode makes them I and S.
        '\u{131}' | '\u{17f}' => c,
        // Greek small letters with an iota below: their one-letter capital is 8 on (9 for the three on their own),
        // where the full capital is two letters.
        '\u{1f80}'..='\u{1faf}' if u32::from(c) & 8 == 0 => char::from_u32(u32::from(c) + 8).unwrap_or(c),
        '\u{1fb3}' | '\u{1fc3}' | '\u{1ff3}' => char::from_u32(u32::from(c) + 9).unwrap_or(c),
        _ => {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        }
    };
    a.chars().map(upper).eq(b.chars().map(upper))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_case_is_compared_as_dotnet_does() {
        assert!(eq_ignore_case("Loop One", "loop one"));
        // .NET doesn't take a dotless i for an I, nor a long s for an S.
        assert!(!eq_ignore_case("\u{131}ntro", "Intro"));
        assert!(!eq_ignore_case("ba\u{17f}s", "BASS"));
        // A Greek letter with an iota below and its one-letter capital.
        assert!(eq_ignore_case("\u{1f80}\u{1fb3}", "\u{1f88}\u{1fbc}"));
        assert!(!eq_ignore_case("\u{1f88}", "\u{1f80}\u{399}"));
    }

    #[test]
    fn endings_are_known_with_or_without_folders() {
        assert!(is_midi_path("Idea.MIDI") && is_midi_path("a/b.kar") && !is_midi_path("mid") && !is_midi_path("a."));
    }
}
