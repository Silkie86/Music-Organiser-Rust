//! MIDI Clip Files (SMF2, `.midi2`): MIDI 2.0's own file, from the MIDI Association. The C# app has no reader for
//! them, so this one follows the MIDI Clip File and Universal MIDI Packet specifications.
//!
//! A clip file is the 8 letters `SMF2CLIP`, then Universal MIDI Packets in 32-bit words, each word high byte first:
//! a configuration header (the clip's ticks a quarter note, its tempo and time signature, its name), then Start of
//! Clip, the clip's messages, and End of Clip. Each message is preceded by a Delta Clockstamp saying how many ticks
//! after the one before it comes.
//!
//! Notes keep MIDI 2.0's 16-bit velocity and their attribute; a MIDI 1.0 message in the clip is scaled up by MIDI
//! 2.0's own rule (rule 22). Notes come from each of the 16 groups' 16 channels: a part per channel that has notes,
//! with that channel's controllers, pitch bend and pressure at 32 bits (per-note ones too).

use std::io;
use std::path::Path;

use super::{Controller, ControllerKind, Held, MidiPart};
use crate::model::{NoteAttribute, NoteEvent, scale_up, velocity};
use crate::paths;

/// Goes up by one whenever the reader finds more (or different) things in the same files (rule 7).
pub const VERSION: u32 = 1;
/// The ending the MIDI Association gives clip files, without its dot.
pub const EXTENSIONS: [&str; 1] = ["midi2"];
/// What a clip file starts with.
pub const MAGIC: &[u8; 8] = b"SMF2CLIP";
/// The ticks a quarter note when a damaged clip never says (its Delta Clockstamp Ticks Per Quarter Note is missing).
pub const DEFAULT_PPQ: u16 = 96;

/// Whether a path ends in `.midi2`, letter case aside.
pub fn is_clip_path(path: &str) -> bool {
    paths::has_extension(path, &EXTENSIONS)
}

/// Reads a MIDI Clip File from disk.
pub fn read(path: &Path) -> io::Result<Vec<MidiPart>> {
    read_bytes(&std::fs::read(path)?)
}

/// A note still held: when it started, its velocity and its attribute.
type Struck = (i64, u16, NoteAttribute);

/// Where a note is played: its group, channel and key.
type Key = (u8, u8, u8);

/// The parts in a clip file's bytes. Bytes that aren't a clip file are an error; a clip damaged part-way is read up
/// to the damage, and notes still held there end where it stops.
pub fn read_bytes(data: &[u8]) -> io::Result<Vec<MidiPart>> {
    let Some(body) = data.strip_prefix(MAGIC) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a MIDI clip file"));
    };
    // A last word cut short is left out.
    let mut words = body.as_chunks::<4>().0.iter().map(|w| u32::from_be_bytes(*w));
    let mut clip = Clip::default();
    while let Some(first) = words.next() {
        // A message's size comes from its type, in the first word's top four bits.
        let size = match first >> 28 {
            0x0 | 0x1 | 0x2 | 0x6 | 0x7 => 1,
            0x3 | 0x4 | 0x8 | 0x9 | 0xa => 2,
            0xb | 0xc => 3,
            _ => 4,
        };
        let mut message = [first, 0, 0, 0];
        for slot in message.iter_mut().take(size).skip(1) {
            let Some(w) = words.next() else { return Ok(clip.into_parts()) };
            *slot = w;
        }
        if clip.message(message) == Step::End {
            break;
        }
    }
    Ok(clip.into_parts())
}

/// What a message means for reading on.
#[derive(PartialEq)]
enum Step {
    On,
    End,
}

struct Clip {
    /// Where the clip is, in ticks.
    tick: i64,
    /// The ticks the last Delta Clockstamp gave, which the next message comes after.
    delta: i64,
    ppq: Option<u16>,
    tempo: Option<f64>,
    /// The clip's name, as far as it has come (a long name comes in parts), and whether it's whole.
    name: Vec<u8>,
    name_done: bool,
    held: Held<Key, Struck>,
    /// Notes by group and channel: 16 of each.
    notes: [Vec<NoteEvent>; SLOTS],
    /// Each group and channel's first program change.
    programs: [Option<u8>; SLOTS],
    /// Each group and channel's controllers, pitch bend and pressure, in order.
    controllers: [Vec<Controller>; SLOTS],
}

/// MIDI 2.0's 16 groups of 16 channels.
const SLOTS: usize = 16 * 16;

impl Default for Clip {
    fn default() -> Self {
        Clip {
            tick: 0,
            delta: 0,
            ppq: None,
            tempo: None,
            name: Vec::new(),
            name_done: false,
            held: Held::default(),
            notes: std::array::from_fn(|_| Vec::new()),
            programs: [None; SLOTS],
            controllers: std::array::from_fn(|_| Vec::new()),
        }
    }
}

impl Clip {
    fn message(&mut self, m: [u32; 4]) -> Step {
        let [w0, w1, ..] = m;
        let kind = w0 >> 28;
        let status = (w0 >> 20) & 0xf;
        // Utility messages: the clock.
        if kind == 0x0 {
            match status {
                // Delta Clockstamp: the ticks before the next message. Two in a row: the second counts (a long
                // wait is written as Delta Clockstamps with a No Op between them).
                0x4 => {
                    self.delta = i64::from(w0 & 0xf_ffff);
                    return Step::On;
                }
                // Delta Clockstamp Ticks Per Quarter Note: the clip's own, the first time it says.
                0x3 if self.ppq.is_none() && w0 & 0xffff > 0 => self.ppq = Some((w0 & 0xffff) as u16),
                _ => {}
            }
        }
        self.tick = self.tick.saturating_add(self.delta);
        self.delta = 0;
        match kind {
            // UMP Stream: Start of Clip is where the clip's own time starts (the header before it takes none), and
            // End of Clip ends it.
            0xf if (w0 >> 16) & 0x3ff == 0x20 => self.tick = 0,
            0xf if (w0 >> 16) & 0x3ff == 0x21 => return Step::End,
            // Flex Data: the tempo and the clip's name.
            0xd => self.flex(m),
            // MIDI 2.0 channel voice messages.
            0x4 => {
                let key = (group(w0), channel(w0), ((w0 >> 8) & 0x7f) as u8);
                match status {
                    // A MIDI 2.0 note on is a note at any velocity, even 0 (unlike MIDI 1.0's).
                    0x9 => {
                        let attribute = match (w0 & 0xff) as u8 {
                            0 => NoteAttribute::NONE,
                            kind => NoteAttribute { kind, data: (w1 & 0xffff) as u16 },
                        };
                        self.held.strike(key, (self.tick, (w1 >> 16) as u16, attribute));
                    }
                    0x8 => self.release(key),
                    0xc => self.program(key.0, key.1, ((w1 >> 24) & 0x7f) as u8),
                    _ => {
                        let index = (w0 & 0xff) as u8;
                        // The registered and assignable controllers carry a bank and an index where a note has its key.
                        let bank_index = (((w0 >> 8) & 0x7f) as u8, (w0 & 0x7f) as u8);
                        let kind = match status {
                            0x0 => ControllerKind::NoteRegistered(key.2, index),
                            0x1 => ControllerKind::NoteAssignable(key.2, index),
                            0x2 => ControllerKind::Registered(bank_index.0, bank_index.1),
                            0x3 => ControllerKind::Assignable(bank_index.0, bank_index.1),
                            0x4 => ControllerKind::RelativeRegistered(bank_index.0, bank_index.1),
                            0x5 => ControllerKind::RelativeAssignable(bank_index.0, bank_index.1),
                            0x6 => ControllerKind::NotePitchBend(key.2),
                            0xa => ControllerKind::KeyPressure(key.2),
                            0xb => ControllerKind::Control(key.2),
                            0xd => ControllerKind::ChannelPressure,
                            0xe => ControllerKind::PitchBend,
                            _ => return Step::On,
                        };
                        self.controller(key.0, key.1, kind, w1);
                    }
                }
            }
            // MIDI 1.0 channel voice messages, in a packet.
            0x2 => {
                let (d1, d2) = (((w0 >> 8) & 0x7f) as u8, (w0 & 0x7f) as u8);
                let key = (group(w0), channel(w0), d1);
                match status {
                    0x9 if d2 > 0 => self.held.strike(key, (self.tick, velocity::from_7(d2), NoteAttribute::NONE)),
                    0x8 | 0x9 => self.release(key),
                    0xc => self.program(key.0, key.1, d1),
                    // Controllers and pressure scaled up from 7 bits, pitch bend from 14, by MIDI 2.0's rule.
                    0xa => self.controller(key.0, key.1, ControllerKind::KeyPressure(d1), scale_up(d2.into(), 7, 32)),
                    0xb => self.controller(key.0, key.1, ControllerKind::Control(d1), scale_up(d2.into(), 7, 32)),
                    0xd => self.controller(key.0, key.1, ControllerKind::ChannelPressure, scale_up(d1.into(), 7, 32)),
                    0xe => {
                        let bend = u32::from(d2) << 7 | u32::from(d1);
                        self.controller(key.0, key.1, ControllerKind::PitchBend, scale_up(bend, 14, 32));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Step::On
    }

    /// Flex Data: Set Tempo (status bank 0, status 0) and the clip's name (bank 1, status 3, in parts of 12 letters).
    fn flex(&mut self, m: [u32; 4]) {
        let [w0, w1, w2, w3] = m;
        let (bank, status) = ((w0 >> 8) & 0xff, w0 & 0xff);
        match (bank, status) {
            // In tens of nanoseconds a quarter note; the first tempo counts, as in a Standard MIDI File.
            (0x00, 0x00) if self.tempo.is_none() && w1 > 0 => {
                self.tempo = Some((6_000_000_000.0 / f64::from(w1) * 100.0).round_ties_even() / 100.0);
            }
            (0x01, 0x03) if !self.name_done => {
                // Complete (0) or start (1) begins it again; continue (2) and end (3) add to it.
                let format = (w0 >> 22) & 0x3;
                if format <= 1 {
                    self.name.clear();
                }
                let text = [w1, w2, w3].into_iter().flat_map(u32::to_be_bytes).filter(|&b| b != 0);
                self.name.extend(text);
                self.name_done = format == 0 || format == 3;
            }
            _ => {}
        }
    }

    fn release(&mut self, key: Key) {
        if let Some(struck) = self.held.release(key) {
            let tick = self.tick;
            self.add(key, struck, tick);
        }
    }

    fn add(&mut self, (group, channel, key): Key, (start, velocity, attribute): Struck, end: i64) {
        let note =
            NoteEvent { start, length: (end - start).max(1), key, velocity, muted: false, step: false, attribute };
        if let Some(notes) = self.notes.get_mut(slot(group, channel)) {
            notes.push(note);
        }
    }

    fn controller(&mut self, group: u8, channel: u8, kind: ControllerKind, value: u32) {
        let tick = self.tick;
        if let Some(list) = self.controllers.get_mut(slot(group, channel)) {
            list.push(Controller { tick, kind, value });
        }
    }

    /// A channel's first program change counts.
    fn program(&mut self, group: u8, channel: u8, program: u8) {
        if let Some(p @ None) = self.programs.get_mut(slot(group, channel)) {
            *p = Some(program);
        }
    }

    fn into_parts(mut self) -> Vec<MidiPart> {
        // Notes still held end where the clip does.
        let end = self.tick;
        for (key, struck) in std::mem::take(&mut self.held).into_held() {
            self.add(key, struck, end);
        }
        let Clip { notes, programs, mut controllers, tempo, name, ppq, .. } = self;
        let name = String::from_utf8_lossy(&name).trim().to_owned();
        let ppq = ppq.unwrap_or(DEFAULT_PPQ);
        let used: Vec<(usize, Vec<NoteEvent>)> = notes.into_iter().enumerate().filter(|(_, n)| !n.is_empty()).collect();
        let groups = used.iter().any(|(slot, _)| *slot >= 16);
        let split = used.len() > 1;
        used.into_iter()
            .map(|(slot, mut notes)| {
                let (group, channel) = (slot / 16, slot % 16);
                notes.sort_by_key(|n| (n.start, n.key));
                let label = match (split, groups) {
                    (false, _) => String::new(),
                    (true, false) => format!("Ch {}", channel + 1),
                    (true, true) => format!("Group {} · Ch {}", group + 1, channel + 1),
                };
                MidiPart {
                    name: label,
                    ppq,
                    notes,
                    drums: channel == 9,
                    tempo,
                    program: programs.get(slot).copied().flatten(),
                    track_name: name.clone(),
                    controllers: controllers.get_mut(slot).map(std::mem::take).unwrap_or_default(),
                }
            })
            .collect()
    }
}

/// Where a group and channel's notes and program are kept.
fn slot(group: u8, channel: u8) -> usize {
    usize::from(group) * 16 + usize::from(channel)
}

fn group(w0: u32) -> u8 {
    ((w0 >> 24) & 0xf) as u8
}

fn channel(w0: u32) -> u8 {
    ((w0 >> 16) & 0xf) as u8
}
