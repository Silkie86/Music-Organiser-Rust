//! The MIDI tab: the parts of the app's MIDI index, read-only, on a background thread.
//!
//! The plugin never writes the app's files. It opens the app's MIDI index (`midi-index.db` in the app's data folder),
//! reads every part in one read, without their notes, and closes it again at once, as the app's MIDI page does. The
//! list, its filters and its orders are the MIDI page's own (`mo_app::midi`), so the two agree. A part dragged out is
//! the same MIDI file the app's own drag writes (`mo_app::drag_out::from_part`), in a drag folder in Temp that the
//! app's sweep clears a day later (rule 28).
//!
//! The index isn't brought up to date here (an older layout copied into a new one, the C# app's corrections taken
//! in): that writes, so the app's MIDI page does it. An index of an older layout is read as it is.
//!
//! Like the sound library, the parts are shared by every copy of the plugin in the DAW while any of them holds them.

use crate::threads::lock;
use mo_app::midi::{self as page, Chips, ChoiceCounts, Filters, Order};
use mo_midi::catalogue::{Catalogue, Entry};
use mo_midi::store::MidiStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

/// The orders the tab offers, as the app's MIDI page names them.
pub const ORDERS: [Order; 4] = [Order::Newest, Order::Name, Order::Key, Order::Bars];

/// The MIDI index's parts, as the app's MIDI page holds them.
pub struct Parts {
    pub cat: Catalogue,
    /// The index they were read from: a dragged part's notes are read from it.
    pub index: PathBuf,
    /// Settings › MIDI's smallest part, as the app has it: smaller parts are left out of the list.
    pub smallest: usize,
    /// The years the parts' files were saved in, newest first, with how many each: the year strip.
    pub years: Vec<(i32, usize)>,
}

/// How reading the index went.
pub enum Read {
    Ready(Arc<Parts>),
    /// Nothing to read, or it couldn't be read: why, in plain words.
    Not(String),
}

/// What the search box, the filter column and the order ask of the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asked {
    pub filters: Filters,
    pub order: Order,
}

static SHARED: Mutex<Weak<Parts>> = Mutex::new(Weak::new());

/// The app's MIDI parts, read once for every copy of the plugin while any of them holds them. Slow the first time
/// (call it off the panel's thread).
pub fn read() -> Read {
    if let Some(parts) = lock(&SHARED).upgrade() {
        return Read::Ready(parts);
    }
    static READING: Mutex<()> = Mutex::new(());
    let _reading = lock(&READING);
    if let Some(parts) = lock(&SHARED).upgrade() {
        return Read::Ready(parts);
    }
    let started = std::time::Instant::now();
    let data = mo_app::paths::data_dir();
    let read = read_from(&MidiStore::path_beside(&data.join("library.db")), smallest_part(&data));
    match &read {
        Read::Ready(parts) => {
            *lock(&SHARED) = Arc::downgrade(parts);
            crate::log::took("MIDI", &format!("Read {} parts", parts.cat.entries.len()), started);
        }
        Read::Not(why) => crate::log::took("MIDI", &format!("Not read: {why}"), started),
    }
    // Drag files the app or the plugin left a day or more ago go (only the app's own drag folders). On Windows only,
    // as the app sweeps: elsewhere Temp is shared by everyone on the computer.
    #[cfg(windows)]
    let swept = mo_app::drag_out::sweep(&std::env::temp_dir(), std::time::SystemTime::now());
    #[cfg(windows)]
    if swept.removed > 0 || swept.failed > 0 {
        crate::log::line("Drag", &format!("Cleared {} old drag files ({} couldn't be)", swept.removed, swept.failed));
    }
    read
}

/// Settings › MIDI's smallest part, read from the app's settings file without changing it (the app's own loading
/// puts a damaged file aside, which is a write); the app's default when it can't be read.
fn smallest_part(data: &Path) -> usize {
    page::smallest_part_from(setting(data, page::SMALLEST_PART_KEY).as_deref())
}

/// One of the app's settings, read from its settings file in `data` without changing it; none when it can't be read.
fn setting(data: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(data.join(mo_app::app_settings::FILE)).ok();
    let settings = mo_app::app_settings::AppSettings::from_parts(text.as_deref(), HashMap::new()).ok()?;
    settings.get(key).map(str::to_owned)
}

/// The SoundFont parts play with: the one picked on the app's MIDI page, else the one the app keeps in its data
/// folder; none when neither is there. Reads files: call it off the panel's thread.
pub fn soundfont() -> Option<PathBuf> {
    let data = mo_app::paths::data_dir();
    page::soundfont_path(setting(&data, page::SOUNDFONT_KEY).as_deref(), &data)
}

/// What's said when there's no SoundFont to play parts with.
pub const NO_SOUNDFONT: &str = "No SoundFont to play MIDI with: pick one on the app's MIDI page";

/// The SoundFont last opened, kept while the plugin is loaded: opening one takes a while.
static FONT: Mutex<Option<(PathBuf, mo_audio::gm::Font)>> = Mutex::new(None);

fn font(path: &Path) -> Result<mo_audio::gm::Font, String> {
    let mut kept = lock(&FONT);
    if let Some((at, font)) = kept.as_ref()
        && at == path
    {
        return Ok(font.clone());
    }
    let font = mo_audio::gm::open(path)?;
    *kept = Some((path.to_owned(), font.clone()));
    Ok(font)
}

/// The parts in the MIDI index at `index`.
pub fn read_from(index: &Path, smallest: usize) -> Read {
    if !index.is_file() {
        return Read::Not("No MIDI yet: open the MIDI page in the app once to read your MIDI".into());
    }
    let store = match MidiStore::open_read_only(index) {
        Ok(store) => store,
        Err(e) => return Read::Not(format!("Couldn't open the MIDI index: {e}")),
    };
    // One read, then the file is let go (`store` drops here).
    let phrases = match store.all(&HashMap::new(), &HashMap::new()) {
        Ok(phrases) => phrases,
        Err(e) => return Read::Not(format!("Couldn't read the MIDI index: {e}")),
    };
    drop(store);
    let cat = Catalogue::new(phrases);
    let years = page::year_counts(&cat);
    Read::Ready(Arc::new(Parts { cat, index: index.to_owned(), smallest, years }))
}

impl Parts {
    /// The parts `asked` lets through, in order, as places in the catalogue, and the filter column's counts. Runs on
    /// every core: call it off the panel's thread.
    pub fn view(&self, asked: &Asked) -> (Vec<usize>, ChoiceCounts) {
        let filters = Filters { smallest: self.smallest, ..asked.filters.clone() };
        let (hits, counts) = page::view(&self.cat, &filters, asked.order, false, None, None);
        (hits.into_iter().map(|h| h.entry).collect(), counts)
    }

    /// How many lines the whole list has (the "of" in "40 of 900 parts").
    pub fn lines(&self) -> usize {
        page::line_count(&self.cat, self.smallest)
    }

    pub fn entry(&self, at: usize) -> Option<&Entry> {
        self.cat.entry(at)
    }

    /// The part at `at` as a loop of sound played with the SoundFont at `font`, at `tempo` (the DAW's; or, when it
    /// hasn't said, the part's own file's, else 120), with the General MIDI sound the app's MIDI page matches to it.
    /// Reads the index and makes the sound: call it off the panel's thread.
    pub fn sound(
        &self,
        at: usize,
        font_path: Option<&Path>,
        tempo: Option<f64>,
    ) -> Result<mo_audio::AudioData, String> {
        let font_path = font_path.ok_or(NO_SOUNDFONT)?;
        let e = self.cat.entry(at).ok_or("That part isn't in the list any more")?;
        let notes = self.notes(e)?;
        let (heard, length) = page::loop_notes(&notes, e.phrase.bars);
        if heard.is_empty() {
            return Err("Every note in it is muted, so there's nothing to hear".to_owned());
        }
        let tempo = tempo.unwrap_or_else(|| {
            let library = self.index.parent().map(|d| d.join("library.db")).unwrap_or_default();
            page::read_tempo(&e.phrase.source, &library).bpm()
        });
        let font = font(font_path)?;
        let notes: Vec<mo_audio::gm::Note> = heard
            .iter()
            .map(|n| mo_audio::gm::Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity7() })
            .collect();
        let sound = page::gm_sound(e);
        let part = mo_audio::gm::Part {
            notes: &notes,
            ppq: mo_midi::PPQ,
            length,
            tempo,
            program: sound.program,
            drums: sound.drums,
        };
        mo_audio::gm::render_loop(&font, &part)
    }

    /// A part's notes, read from the index (opened, read and let go again).
    fn notes(&self, e: &Entry) -> Result<Vec<mo_formats::NoteEvent>, String> {
        let store = MidiStore::open_read_only(&self.index).map_err(|e| format!("Couldn't open the MIDI index: {e}"))?;
        store.notes(e.phrase.id, &e.phrase.hash).map_err(|e| format!("Its notes couldn't be read: {e}"))
    }

    /// The part at `at` as one MIDI file in a new drag folder in `temp` (the system's Temp), as the app's own drag
    /// writes it. Reads the index and writes a file: call it off the panel's thread.
    pub fn file(&self, at: usize, temp: &Path) -> Result<PathBuf, String> {
        let e = self.cat.entry(at).ok_or("That part isn't in the list any more")?;
        let notes = self.notes(e)?;
        // The plugin hands over the file alone (no card), so the card's colour doesn't matter.
        let out = mo_app::drag_out::from_part(&page::title(e), &notes, e.phrase.bars, 0)?;
        let payload = out.write(temp).map_err(|e| format!("Couldn't write the MIDI file in Temp: {e}"))?;
        payload.files.into_iter().next().ok_or_else(|| "There was nothing to drag".to_owned())
    }
}

/// The filter column for these counts (none before the first list is worked out) and picks.
pub fn chips(counts: Option<&ChoiceCounts>, asked: &Asked) -> Chips {
    page::chips(counts.unwrap_or(&ChoiceCounts::default()), &asked.filters)
}

/// The list's status line: "1,234 parts", or "40 of 1,234 parts" when something narrows it.
pub fn count_line(total: usize, found: Option<usize>) -> String {
    let parts = if total == 1 { "part" } else { "parts" };
    let thousands = mo_app::backup::thousands;
    match found {
        Some(found) => format!("{} of {} {parts}", thousands(found), thousands(total)),
        None => format!("{} {parts}", thousands(total)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_index_says_open_the_midi_page() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("no such folder");
        match read_from(&dir.join("midi-index.db"), 3) {
            Read::Not(why) => assert!(why.contains("MIDI page"), "{why}"),
            Read::Ready(_) => panic!("there's no index there"),
        }
    }

    #[test]
    fn count_lines() {
        assert_eq!(count_line(1, None), "1 part");
        assert_eq!(count_line(1234, Some(40)), "40 of 1,234 parts");
    }
}
