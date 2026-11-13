//! The plugin's knobs in the DAW (CLAP's `params` and `remote-controls` extensions): Bitwig shows the eight on its
//! device, under the plugin's own page, "Music Organiser", so they can be turned with the mouse or a controller.
//!
//! - **Sound**, **MIDI part** and **Preset** go through their tab's list: turned, the line that far down the list is
//!   picked and plays (a preset is picked), as a click on it would.
//! - **Sounds like** is the Sounds tab's slider of the same name.
//! - **Volume** turns what the plugin plays up or down.
//! - **Loop**, **Match tempo** and **In time** are the panel's switches.
//!
//! Each knob's value is shared by the DAW's threads and the panel's: plain numbers only, so nothing here waits. What
//! the DAW turns is marked for the panel to act on ([`Knobs::turned`]); what the panel changes (a line clicked, a switch
//! clicked) is marked to tell the DAW, so its knob moves too ([`Knobs::tell`], sent with the next block of sound).

use crate::audition::{Player, Playing};
use clack_extensions::params::{ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter};
use clack_extensions::remote_controls::{RemoteControlsPage, RemoteControlsPageWriter};
use clack_plugin::events::event_types::ParamValueEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Pckn, io::InputEvents, io::OutputEvents};
use clack_plugin::utils::{ClapId, Cookie};
use std::fmt::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// The knobs, in the order the DAW shows them. A knob's number is its CLAP id: never change one (a project keeps it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knob {
    Sound = 1,
    Like = 2,
    Part = 3,
    Preset = 4,
    Volume = 5,
    Loop = 6,
    Match = 7,
    InTime = 8,
}

impl Knob {
    pub const ALL: [Knob; 8] =
        [Knob::Sound, Knob::Like, Knob::Part, Knob::Preset, Knob::Volume, Knob::Loop, Knob::Match, Knob::InTime];

    fn of(id: u32) -> Option<Knob> {
        Knob::ALL.into_iter().find(|k| *k as u32 == id)
    }

    fn id(self) -> ClapId {
        ClapId::new(self as u32)
    }

    fn at(self) -> usize {
        self as usize - 1
    }

    fn bit(self) -> u32 {
        1 << self.at()
    }

    pub fn name(self) -> &'static str {
        match self {
            Knob::Sound => "Sound",
            Knob::Like => "Sounds like",
            Knob::Part => "MIDI part",
            Knob::Preset => "Preset",
            Knob::Volume => "Volume",
            Knob::Loop => "Loop",
            Knob::Match => "Match tempo",
            Knob::InTime => "In time",
        }
    }

    /// An on/off knob: 0 or 1.
    fn switch(self) -> bool {
        matches!(self, Knob::Loop | Knob::Match | Knob::InTime)
    }

    /// Goes through a list: the line that far down it.
    fn list(self) -> Option<usize> {
        match self {
            Knob::Sound => Some(0),
            Knob::Part => Some(1),
            Knob::Preset => Some(2),
            _ => None,
        }
    }

    /// Where it starts, as the panel starts.
    fn default(self) -> f64 {
        match self {
            Knob::Volume | Knob::InTime => 1.0,
            _ => 0.0,
        }
    }
}

/// The knobs' values, shared by the DAW's threads and the panel's.
pub struct Knobs {
    /// Each knob's value (0 to 1), as an `f64`'s bits.
    values: [AtomicU64; 8],
    /// The knobs the DAW turned that the panel hasn't acted on yet, one bit each.
    turned: AtomicU32,
    /// The knobs the panel changed that the DAW hasn't been told of yet.
    to_tell: AtomicU32,
    /// How many lines the Sounds, MIDI and Presets lists have now: for the words the DAW shows by a knob.
    lines: [AtomicUsize; 3],
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs {
            values: Knob::ALL.map(|k| AtomicU64::new(k.default().to_bits())),
            turned: AtomicU32::new(0),
            to_tell: AtomicU32::new(0),
            lines: Default::default(),
        }
    }
}

/// The line `value` (0 to 1) of the way down a list of `lines`, or none in an empty list.
pub fn line_at(value: f64, lines: usize) -> Option<usize> {
    let last = lines.checked_sub(1)?;
    let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
    Some(((value * last as f64).round() as usize).min(last))
}

/// How far down a list of `lines` the line `at` is (0 to 1): what [`line_at`] turns back into `at`.
pub fn value_of_line(at: usize, lines: usize) -> f64 {
    match lines {
        0 | 1 => 0.0,
        _ => (at.min(lines - 1) as f64 / (lines - 1) as f64).clamp(0.0, 1.0),
    }
}

impl Knobs {
    pub fn get(&self, knob: Knob) -> f64 {
        f64::from_bits(self.values[knob.at()].load(Ordering::Acquire))
    }

    fn set(&self, knob: Knob, value: f64) -> f64 {
        let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { knob.default() };
        let value = if knob.switch() { value.round() } else { value };
        self.values[knob.at()].store(value.to_bits(), Ordering::Release);
        value
    }

    /// The panel changed what `knob` shows (a line clicked, a switch clicked): the DAW's knob is moved to `value`
    /// with the next block of sound.
    /// A change the knob already shows (the panel acting on the knob itself) isn't told back, so turning a knob is
    /// never fought by the panel.
    pub fn tell(&self, knob: Knob, value: f64) {
        if (self.get(knob) - value).abs() < 1e-5 {
            return;
        }
        self.set(knob, value);
        self.to_tell.fetch_or(knob.bit(), Ordering::AcqRel);
    }

    /// Line `at` of a list of `lines` was picked in the panel: its knob moves there, unless it already points at that
    /// line (it was the knob that picked it).
    pub fn tell_line(&self, knob: Knob, at: usize, lines: usize) {
        if line_at(self.get(knob), lines) != Some(at) {
            self.tell(knob, value_of_line(at, lines));
        }
    }

    /// The knobs the DAW has turned since the panel last asked, for the panel to act on.
    pub fn turned(&self) -> Vec<Knob> {
        let bits = self.turned.swap(0, Ordering::AcqRel);
        Knob::ALL.into_iter().filter(|k| bits & k.bit() != 0).collect()
    }

    /// How many lines a list has now (for the words by its knob).
    pub fn set_lines(&self, knob: Knob, lines: usize) {
        if let Some(list) = knob.list() {
            self.lines[list].store(lines, Ordering::Release);
        }
    }

    /// The DAW turned `knob` to `value`: the switches and Volume act at once, the lists when the panel next looks.
    /// `playing` is the audio thread's side, when this is on it (the Loop switch changes the sound playing there);
    /// none on the DAW's main thread, which asks the player instead.
    fn turn(&self, knob: Knob, value: f64, player: &Player, playing: Option<&mut Playing>) {
        let value = self.set(knob, value);
        let on = value >= 0.5;
        match knob {
            Knob::Volume => player.set_volume(value),
            Knob::Match => player.set_matching(on),
            Knob::InTime => player.set_in_time(on),
            Knob::Loop => match playing {
                Some(playing) => {
                    player.note_loop(on);
                    playing.switch_loop(on);
                }
                None => player.set_loop(on),
            },
            Knob::Sound | Knob::Like | Knob::Part | Knob::Preset => {}
        }
        self.turned.fetch_or(knob.bit(), Ordering::AcqRel);
    }

    /// The DAW's knob changes in `input`, and the panel's changes told back in `output`: once a block of sound (or
    /// when the DAW asks, between blocks).
    pub fn exchange(
        &self,
        input: &InputEvents,
        output: &mut OutputEvents,
        player: &Player,
        mut playing: Option<&mut Playing>,
    ) {
        for event in input {
            if let Some(CoreEventSpace::ParamValue(change)) = event.as_core_event()
                && let Some(knob) = change.param_id().and_then(|id| Knob::of(id.get()))
            {
                self.turn(knob, change.value(), player, playing.as_deref_mut());
            }
        }
        let bits = self.to_tell.swap(0, Ordering::AcqRel);
        let mut untold = 0;
        for knob in Knob::ALL.into_iter().filter(|k| bits & k.bit() != 0) {
            let event = ParamValueEvent::new(0, knob.id(), Pckn::match_all(), self.get(knob));
            if output.try_push(event).is_err() {
                untold |= knob.bit();
            }
        }
        // No room this time: the next.
        self.to_tell.fetch_or(untold, Ordering::AcqRel);
    }

    /// What the DAW shows of `knob` (its name, its range).
    pub fn info(index: u32, writer: &mut ParamInfoWriter) {
        let Some(knob) = Knob::ALL.get(index as usize) else { return };
        let mut flags = ParamInfoFlags::IS_AUTOMATABLE;
        if knob.switch() {
            flags |= ParamInfoFlags::IS_STEPPED;
        }
        writer.set(&ParamInfo {
            id: knob.id(),
            flags,
            cookie: Cookie::empty(),
            name: knob.name().as_bytes(),
            module: b"",
            min_value: 0.0,
            max_value: 1.0,
            default_value: knob.default(),
        });
    }

    pub fn value(&self, id: ClapId) -> Option<f64> {
        Knob::of(id.get()).map(|k| self.get(k))
    }

    /// The words the DAW shows by a knob: "line 12 of 340", "-6.0 dB", "On".
    pub fn words(&self, id: ClapId, value: f64, writer: &mut ParamDisplayWriter) -> std::fmt::Result {
        let Some(knob) = Knob::of(id.get()) else { return Err(std::fmt::Error) };
        write!(writer, "{}", self.words_for(knob, value))
    }

    fn words_for(&self, knob: Knob, value: f64) -> String {
        if let Some(list) = knob.list() {
            let lines = self.lines[list].load(Ordering::Acquire);
            return match line_at(value, lines) {
                Some(at) => format!("line {} of {lines}", at + 1),
                None => "nothing listed".into(),
            };
        }
        match knob {
            _ if knob.switch() => if value >= 0.5 { "On" } else { "Off" }.into(),
            Knob::Volume if value <= 0.001 => "Silent".into(),
            Knob::Volume => format!("{:.1} dB", 20.0 * value.log10()),
            _ => format!("{:.0}%", value * 100.0),
        }
    }

    /// The words a DAW was typed back into a value: "On", "50%", "-6 dB", a number.
    pub fn read_words(id: ClapId, text: &str) -> Option<f64> {
        let knob = Knob::of(id.get())?;
        let text = text.trim().to_lowercase();
        if knob.switch() {
            return match text.as_str() {
                "on" | "1" | "yes" => Some(1.0),
                "off" | "0" | "no" => Some(0.0),
                _ => None,
            };
        }
        if knob == Knob::Volume {
            if text == "silent" {
                return Some(0.0);
            }
            let db = text.trim_end_matches("db").trim().parse::<f64>().ok()?;
            return Some(10f64.powf(db / 20.0).clamp(0.0, 1.0));
        }
        let number = text.trim_end_matches('%').trim().parse::<f64>().ok()?;
        Some(if text.ends_with('%') || number > 1.0 { number / 100.0 } else { number }.clamp(0.0, 1.0))
    }

    /// The one page of knobs the DAW shows (Bitwig: the device's remote controls).
    pub fn page(writer: &mut RemoteControlsPageWriter) {
        writer.set(&RemoteControlsPage {
            section_name: b"Music Organiser",
            page_id: ClapId::new(1),
            page_name: b"Music Organiser",
            param_ids: Knob::ALL.map(|k| Some(k.id())),
            is_for_preset: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_knob_turned_goes_through_a_list_and_back() {
        assert_eq!(line_at(0.5, 0), None);
        assert_eq!(line_at(0.7, 1), Some(0));
        assert_eq!(line_at(0.0, 5), Some(0));
        assert_eq!(line_at(1.0, 5), Some(4));
        assert_eq!(line_at(0.5, 5), Some(2));
        assert_eq!(line_at(f64::NAN, 5), Some(0));
        for lines in [1, 2, 7, 340, 500_000] {
            for at in [0, 1, lines / 2, lines - 1] {
                let at = at.min(lines - 1);
                assert_eq!(line_at(value_of_line(at, lines), lines), Some(at), "{at} of {lines}");
            }
        }
    }

    #[test]
    fn the_daw_turning_a_knob_acts_and_is_marked_for_the_panel() {
        let (knobs, player) = (Knobs::default(), Player::default());
        assert!(player.in_time());
        knobs.turn(Knob::InTime, 0.2, &player, None);
        knobs.turn(Knob::Match, 0.9, &player, None);
        knobs.turn(Knob::Loop, 1.0, &player, None);
        knobs.turn(Knob::Sound, 0.25, &player, None);
        assert!(!player.in_time() && player.matching() && player.looping());
        assert_eq!((knobs.get(Knob::InTime), knobs.get(Knob::Match)), (0.0, 1.0), "switches are 0 or 1");
        assert_eq!(knobs.turned(), vec![Knob::Sound, Knob::Loop, Knob::Match, Knob::InTime]);
        assert!(knobs.turned().is_empty(), "taken once");
        // Told from the panel: kept for the DAW, never marked as turned.
        knobs.tell(Knob::Part, 0.5);
        assert_eq!(knobs.get(Knob::Part), 0.5);
        assert!(knobs.turned().is_empty());
        assert_eq!(knobs.to_tell.load(Ordering::Acquire), Knob::Part.bit());
        // The line the knob already points at, picked by the knob: nothing to tell. Another line: told.
        knobs.to_tell.store(0, Ordering::Release);
        knobs.tell_line(Knob::Part, 2, 5);
        knobs.tell(Knob::Part, 0.5 + 1e-9);
        assert_eq!(knobs.to_tell.load(Ordering::Acquire), 0);
        knobs.tell_line(Knob::Part, 4, 5);
        assert_eq!((knobs.get(Knob::Part), knobs.to_tell.load(Ordering::Acquire)), (1.0, Knob::Part.bit()));
    }

    #[test]
    fn the_words_by_a_knob_and_back() {
        let knobs = Knobs::default();
        knobs.set_lines(Knob::Sound, 340);
        assert_eq!(knobs.words_for(Knob::Sound, 0.0), "line 1 of 340");
        assert_eq!(knobs.words_for(Knob::Sound, 1.0), "line 340 of 340");
        assert_eq!(knobs.words_for(Knob::Preset, 0.5), "nothing listed");
        assert_eq!(knobs.words_for(Knob::Loop, 1.0), "On");
        assert_eq!(knobs.words_for(Knob::Volume, 1.0), "0.0 dB");
        assert_eq!(knobs.words_for(Knob::Volume, 0.5), "-6.0 dB");
        assert_eq!(knobs.words_for(Knob::Volume, 0.0), "Silent");
        assert_eq!(knobs.words_for(Knob::Like, 0.25), "25%");
        let id = |k: Knob| k.id();
        assert_eq!(Knobs::read_words(id(Knob::Loop), " On "), Some(1.0));
        assert_eq!(Knobs::read_words(id(Knob::Loop), "maybe"), None);
        assert!((Knobs::read_words(id(Knob::Volume), "-6 dB").unwrap() - 0.501).abs() < 0.001);
        assert_eq!(Knobs::read_words(id(Knob::Like), "40%"), Some(0.4));
        assert_eq!(Knobs::read_words(id(Knob::Like), "0.4"), Some(0.4));
        assert_eq!(Knobs::read_words(ClapId::new(99), "1"), None);
    }

    #[test]
    fn knob_ids_never_change() {
        // A project keeps a knob by its number: these are fixed.
        let ids: Vec<u32> = Knob::ALL.iter().map(|k| *k as u32).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(Knob::ALL.iter().all(|k| Knob::of(*k as u32) == Some(*k)));
    }
}
