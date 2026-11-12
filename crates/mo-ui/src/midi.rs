//! Fills the MIDI page from the MIDI index and answers its chips, list and card. Everything slow runs off the window's
//! thread at low priority (rules 12 and 21): reading the index once, the first time the page is shown; working out
//! each list (newest ask wins); and reading the picked part's notes for its piano roll and its key. Lines are only
//! made when the list draws them, so 300,000 parts cost what's on screen.
//!
//! "Fits my song" ([`mo_app::midi_fits`]) makes the list the parts that go with a song you're working on, the best
//! first: its chip picks one of the songs saved last (the Songs page's list gives them, [`songs_offered`]), the
//! page's filters stay in force, and the search runs on a thread of its own (newest wins; an older search is told to
//! stop), so the filters' counts never wait for it.
//!
//! Where the parts come from: the Rust app's own `midi-index.db`, read-only, which reading your MIDI fills. The first
//! time it's read, it's brought up to this app's layout and the corrections you made in the C# app are taken in from
//! its `midi.db` ([`mo_midi::store::bring_up_to_date`]); `midi.db` isn't opened after that.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::finder::Kind;
use mo_app::midi::{self as page, Filters, Order, Roll, Target};
use mo_app::midi_fits::{self, Answer, Fit, SongBars, SongChoice};
use mo_app::places::Place;
use mo_app::shortcuts::{Shortcuts, what_of};
use mo_app::{log, to_bitwig};
use mo_formats::NoteEvent;
use mo_harmony::KeyFact;
use mo_library::Library;
use mo_library::songs::thousands;
use mo_midi::fit::Wants;
use mo_midi::indexer;
use mo_midi::playing::Song;
use mo_midi::store::{MidiStore, SlotBook};
use mo_midi::{Catalogue, Hit, Role, StoredPhrase};
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::filters::FilterModels;
use crate::sounds::{new_seed, strings};
use crate::{AppWindow, CardAction, MenuItem, MidiData, MidiFact, MidiLine, PickedPart, YearBar};

/// Where a part's notes are read from.
#[derive(Clone)]
pub enum NotesFrom {
    /// The Rust app's `midi-index.db`.
    Store(PathBuf),
    /// Made-up notes by hash, for the screenshots.
    Made(Arc<HashMap<String, Vec<NoteEvent>>>),
}

/// The index as read: every part, and where it came from.
pub struct Loaded {
    pub cat: Catalogue,
    pub notes: NotesFrom,
    /// Worked out with the reading, off the window's thread: the summary and the year strip.
    pub summary: String,
    /// What Settings' smallest part hides, said after the summary (" · 120 under 3 notes hidden").
    pub hidden: String,
    pub years: Vec<(i32, usize)>,
    /// What Ctrl+K finds: every part.
    pub found: Arc<mo_app::finder::Lines>,
}

impl Loaded {
    pub fn new(cat: Catalogue, notes: NotesFrom) -> Loaded {
        let smallest = SMALLEST.load(Ordering::Relaxed);
        // Said with what Settings hides from the first, so the line doesn't change once the list is worked out.
        let hidden = page::hidden_text(&cat, page::line_count(&cat, smallest), smallest);
        let summary = page::summary(&cat);
        let years = page::year_counts(&cat);
        let found = Arc::new(mo_app::finder::midi_parts(&cat, smallest));
        Loaded { cat, notes, summary, hidden, years, found }
    }
}

#[derive(Default)]
struct Page {
    cat: Arc<Catalogue>,
    notes: Option<NotesFrom>,
    filters: Filters,
    order: Order,
    /// The order turned round: a second click on its heading.
    reversed: bool,
    seed: Option<u64>,
    target: Option<Target>,
    shown: Arc<Vec<Hit>>,
    years: Vec<(i32, usize)>,
    /// How many parts each filter chip's choices would show, from the last list worked out.
    counts: page::ChoiceCounts,
    /// The part playing as a loop (or being made into one), as its place in the catalogue's entries.
    looping: Option<usize>,
    /// The part playing (or starting) in a DAW, as its phrase's ID (which a fresh read of the list keeps): picking
    /// another plays that one there instead (the 6 Nov review's D-3).
    in_daw: Option<i64>,
    /// The picked part, as its place in the catalogue's entries, and its row in the list on show (kept, so stepping
    /// down a long list never searches it).
    picked: Option<usize>,
    picked_row: Option<usize>,
    asked: u64,
    notes_asked: u64,
    /// The index is being read, or has been: it's read once, the first time the page is shown.
    started: bool,
    /// The reading of your MIDI into the Rust index running now: what stops it.
    reading: Option<Arc<AtomicBool>>,
    /// The index has been read once since the app opened (reading again is the Read MIDI again button's).
    read_once: bool,
    library: PathBuf,
    feed: Option<Feed>,
    shortcuts: Shortcuts,
    /// A part Ctrl+K asked for before the index was read, by its notes' fingerprint: shown once it is.
    wanted: Option<String>,
    /// The list scrolls to the picked part once it's worked out (Ctrl+K picked it).
    scroll_to_pick: bool,
    /// Ctrl+K read the index for its own list before the page was first shown.
    finder_read: bool,
    /// That read, kept until the page first shows, so the page doesn't read and group the whole index again (nothing
    /// writes the index before then, so it can't be stale).
    finder_loaded: Option<Result<Loaded, String>>,
    /// The page first showed while that read was still going: it's handed to the page when it ends.
    finder_for_page: bool,
    /// That read was let go unused (the page wasn't shown soon after): the page reads the index itself (M-3).
    finder_let_go: bool,
    /// The summary of the whole index, before what Settings' smallest part hides is said after it.
    summary: String,
    /// "Fits my song": the songs offered, and the one the list is fitted to (none: it's off).
    fit_songs: Vec<SongChoice>,
    fit_song: Option<SongChoice>,
    /// What's looked for, once you've chosen; until then the song decides ([`midi_fits::fitted`]).
    fit_wants: Option<Wants>,
    /// What of the song you picked by hand for the list to be fitted to (its parts, its bars): what the next search
    /// is asked for. Nothing picked: the app picks.
    fit_yours: midi_fits::YourPick,
    /// The picker on show, where that's picked: the last search's, with the presses since ([`fit_pick_pressed`]).
    /// None until a search has read the song.
    fit_picker: Option<midi_fits::Picker>,
    /// What you picked for each of the songs picked for last, as the setting remembers them
    /// ([`midi_fits::PICKS_KEY`]).
    fit_picks: midi_fits::YourPicks,
    /// Goes up each time a reading of the MIDI made slots and no new index to show: the slots a search kept are read
    /// again.
    fit_slots_made: u64,
    /// Stops the search running now: set when a newer list is asked for.
    fit_stop: Arc<AtomicBool>,
    /// The list on show is the parts that fit: each line's fit, in the list's order.
    fits: Option<Arc<Vec<Fit>>>,
    /// The song that list was fitted to, as it was then (which save, written when).
    fits_song: Option<SongChoice>,
    /// How the picked part is moved to fit ([`Page::fit_picked`]); none: it's as it is in the library.
    picked_fit: Option<PickedFit>,
    /// The octaves you've chosen for parts since the index was read ([`Page::change_octave`]).
    your_octaves: midi_fits::YourOctaves,
    /// How far the picked part's card shows it moved (its roll and chords), how far the loop playing was moved when
    /// it was made, and the part playing in a DAW when it was sent: what [`fit_follows`] brings into line with
    /// `picked_fit`.
    card_move: i32,
    loop_move: i32,
    daw_move: i32,
    /// What the picked part's card says and offers about its fit now ([`show_card_fit`]).
    card_fit: CardFit,
    /// While a part is being put in its song in Bitwig ([`put_in_song`]; one at a time): set to have it stopped
    /// ([`PutButton::Stop`]).
    putting: Option<Arc<AtomicBool>>,
    /// "Play with the song": a part found plays with the parts of the song it fits ([`Page::heard_with`]); off: alone,
    /// as in any other list. Remembered ([`midi_fits::WITH_SONG_KEY`]).
    with_song: bool,
    /// What a part in the list on show is heard with: the song's parts and bars the list is fitted to; none in a
    /// list that isn't "Fits my song"'s.
    fit_with: Option<Arc<SongBars>>,
    /// What the picked part's card says it plays with ([`show_sound`]), and what the loop playing was made with:
    /// brought into line by [`fit_follows`], as `card_move` and `loop_move` are.
    card_with: Option<Arc<SongBars>>,
    loop_with: Option<Arc<SongBars>>,
    /// The picked part's sound line for when it plays alone, with its own tempo: none until that's read
    /// ([`tempo_read`]). Kept beside the line with the song, so "Play with the song" changes the card's words with
    /// nothing read again.
    card_alone: Option<String>,
}

/// The song a part found plays with in Bitwig ([`Page::bitwig_song`]).
#[derive(Clone, Debug, PartialEq)]
struct BitwigSong {
    /// Its bars for Bitwig's arranger to go round, or why Bitwig has none (the end of the card's line).
    bars: Result<mo_dawlink::bitwig::SongLoop, String>,
    /// The song's name, and the parts and bars the part was fitted to, for the card's words.
    name: String,
    with: Arc<SongBars>,
}

/// How the picked part is moved to fit a song: what its card says, and how it's shown, played, dragged and sent.
#[derive(Clone, Debug, PartialEq)]
struct PickedFit {
    /// Its line in the list that had it: how far it's moved in all is [`Fit::moved`], into the song's key and to the
    /// octave (the app's, or the one you chose).
    fit: Fit,
    /// The song the list that had it was fitted to, as it was then.
    song: SongChoice,
    /// The parts and bars of the song that list was fitted to: what it's heard with.
    with: Option<Arc<SongBars>>,
}

/// A change to the picked part's octave, from its card or a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OctaveChange {
    Up,
    Down,
    /// Back to the app's pick.
    Apps,
}

impl OctaveChange {
    fn from_id(id: &str) -> Option<OctaveChange> {
        match id {
            "midi.octaveUp" => Some(OctaveChange::Up),
            "midi.octaveDown" => Some(OctaveChange::Down),
            "midi.octaveApp" => Some(OctaveChange::Apps),
            _ => None,
        }
    }
}

/// What the picked part's card says and offers about how it's moved to fit a song.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CardFit {
    /// [`midi_fits::card_text`]; "" when the part is as it is in the library.
    text: String,
    /// Whether it's moved at all (a part can fit as it is).
    moved: bool,
    /// The octave buttons, for a part that's fitted (drums aside).
    octave: Option<OctaveButtons>,
    /// Put in the song, for a part fitted to bars Bitwig's arranger has ([`Page::bitwig_place`]): what the button
    /// does now.
    put: Option<PutButton>,
}

/// What the card's Put in the song button does now ([`CardFit::put`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PutButton {
    /// Has Bitwig record the part into its song.
    Put,
    /// A part is being put in (one at a time): stops Bitwig recording it.
    Stop,
    /// The stop is on its way: the button waits until the card says how it went.
    Stopping,
}

impl PutButton {
    /// What the button reads, and whether it can be pressed.
    fn reads(self) -> (&'static str, bool) {
        match self {
            PutButton::Put => ("Put in the song", true),
            PutButton::Stop => ("Stop the recording", true),
            PutButton::Stopping => ("Stopping the recording\u{2026}", false),
        }
    }
}

/// What a fitted part's octave buttons can do now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct OctaveButtons {
    /// Whether it can go an octave up, and down ([`Fit::octave_step`]).
    up: bool,
    down: bool,
    /// Whether the octave is one you chose, so the app's pick is offered back.
    yours: bool,
}

/// A part's notes as they leave the index, for its card, the speakers, a DAW or a drag: where they're read from, how
/// far they're moved to fit the song, and the name that says so.
#[derive(Clone)]
struct PartOut {
    from: NotesFrom,
    phrase_id: i64,
    hash: String,
    /// How far it's moved to fit the song ([`Page::fit_move`]); 0 for as it is in the library.
    semitones: i32,
    /// Its title, saying how it's moved when it is ([`midi_fits::moved_name`]).
    name: String,
    bars: i32,
}

impl PartOut {
    /// Its notes as they're played, dragged out and sent: read from the index ([`notes_of`]: `open` keeps it open
    /// for the next ask) and moved to fit.
    fn notes(&self, open: &mut Option<(PathBuf, MidiStore)>) -> Result<Vec<NoteEvent>, String> {
        Ok(midi_fits::moved(notes_of(&self.from, self.phrase_id, &self.hash, open)?, self.semitones))
    }
}

impl Page {
    /// Whether the list is "Fits my song"'s: it's on, and there's an index to search.
    fn fitting(&self) -> bool {
        self.fit_song.is_some() && self.notes.is_some()
    }

    /// What's picked by hand for the song the list is fitted to is `yours` from here: it's what the next search is
    /// asked for, and it's remembered for the song ([`Page::fit_picks`]). Whether what's remembered changed, so
    /// the setting is to be written ([`remember_fit_picks`]).
    fn fit_pick_is(&mut self, yours: midi_fits::YourPick) -> bool {
        self.fit_yours = yours;
        let Some(song) = self.fit_song.as_ref() else { return false };
        self.fit_picks.set(&song.key, &self.fit_yours)
    }

    /// A press where the song's parts and bars are picked: the page's picker changes as `press` says, and what's
    /// picked by hand is the next search's and the song's to remember ([`Page::fit_pick_is`]: whether the setting
    /// is to be written). None when the press changed nothing (a handle held still), so nothing is asked for; or
    /// while there's no picker.
    fn fit_pick_press(&mut self, press: impl FnOnce(&mut midi_fits::Picker)) -> Option<bool> {
        let picker = self.fit_picker.as_mut()?;
        let before = picker.clone();
        press(picker);
        if *picker == before {
            return None;
        }
        let yours = picker.pick();
        Some(self.fit_pick_is(yours))
    }

    /// The switch for what's looked for was pressed: the app's pick of the song's chords to play over, or of its
    /// melodies to put chords under. So parts picked by hand go back to the app (they decided what's looked for,
    /// and the switch would do nothing); bars of yours stay. Whether the setting is to be written.
    fn fit_wants_press(&mut self, wants: Wants) -> bool {
        self.fit_wants = Some(wants);
        if let Some(picker) = self.fit_picker.as_mut() {
            picker.apps_parts();
        }
        let yours = midi_fits::YourPick { parts: None, bars: self.fit_yours.bars.clone() };
        yours != self.fit_yours && self.fit_pick_is(yours)
    }

    /// The list is fitted to `song` from here. Whether it's another song than before: that one decides for itself
    /// what's looked for, and is fitted to what you picked of it the last time, if you did. The same one picked
    /// again keeps what you chose.
    fn fit_song_is(&mut self, song: SongChoice) -> bool {
        let another = self.fit_song.as_ref().is_none_or(|was| was.among(std::slice::from_ref(&song)).is_none());
        if another {
            self.fit_wants = None;
            self.fit_yours = self.fit_picks.of(&song.key);
            self.fit_picker = None;
        }
        self.fit_song = Some(song);
        // Ranked by how each part fits now, not by its likeness to one.
        self.target = None;
        another
    }

    /// "Fits my song" is off: a search still running is told to stop. What's picked by hand stays remembered for
    /// the song ([`Page::fit_picks`]), which has it again when it's picked again. Whether it was on.
    fn fit_is_off(&mut self) -> bool {
        self.fit_stop.store(true, Ordering::Relaxed);
        self.fit_wants = None;
        self.fit_yours = midi_fits::YourPick::default();
        self.fit_picker = None;
        self.fit_song.take().is_some()
    }

    /// How the part at `at` in the catalogue fits the song, while "Fits my song" is on and the list on show has it.
    fn fit_of(&self, at: usize) -> Option<Fit> {
        self.fit_song.as_ref()?;
        self.fits.as_ref()?.iter().find(|fit| fit.entry == at).copied()
    }

    /// How far the part at `at` is moved wherever its notes leave the page ([`Page::part_out`]), in semitones: into
    /// the song's key and to the octave, in all ([`Fit::moved`]). The picked part: as its card says
    /// ([`Page::fit_picked`]). A line dragged out: by what the line says. 0 in a list that isn't "Fits my song"'s.
    fn fit_move(&self, at: usize) -> i32 {
        if self.picked == Some(at) {
            self.picked_fit.as_ref().map_or(0, |picked| picked.fit.moved())
        } else {
            self.fit_of(at).map_or(0, |fit| fit.moved())
        }
    }

    /// Works out how the picked part is moved, for the list now on show. In a "Fits my song" list that has it: by
    /// what its line says. In one that doesn't (the filters hide it now, or it's no longer among the best): as it was,
    /// while the list is still fitted to the same save of the same song, so a word typed in the search box or a
    /// filter ticked while it plays changes nothing that's heard. In any other list: not moved.
    fn fit_picked(&mut self) {
        let line = self.picked.and_then(|at| self.fit_of(at));
        let song = self.fits_song.as_ref().filter(|_| self.picked.is_some() && self.fits.is_some());
        self.picked_fit = match (line, song) {
            (Some(fit), Some(song)) => Some(PickedFit { fit, song: song.clone(), with: self.fit_with.clone() }),
            (None, Some(song)) => self.picked_fit.take().filter(|was| was.song == *song),
            _ => None,
        };
    }

    /// What the part at `at` plays with when Play loop plays it: the parts and bars of the song it was fitted to,
    /// while "Play with the song" is on. The picked part: those of the list that had it ([`Page::fit_picked`]), so a
    /// tune found over the song's chords still plays over them once the list looks for chords. None for a part
    /// that isn't fitted, for drums, with the chip off, and when nothing of the song plays in those bars: it plays
    /// alone then.
    fn heard_with(&self, at: usize) -> Option<Arc<SongBars>> {
        if !self.with_song {
            return None;
        }
        self.fitted_with(at).filter(|with| !with.voices.is_empty())
    }

    /// The parts and bars of the song the part at `at` was fitted to, whatever the "Play with the song" chip says:
    /// where it goes in the song. The picked part's are those of the list that had it ([`Page::fit_picked`]). None
    /// for a part that isn't fitted, and for drums.
    fn fitted_with(&self, at: usize) -> Option<Arc<SongBars>> {
        if self.cat.entry(at).is_none_or(|e| e.role() == Role::Drums) {
            return None;
        }
        let with = if self.picked == Some(at) {
            self.picked_fit.as_ref()?.with.clone()
        } else {
            self.fit_of(at).and(self.fit_with.clone())
        };
        with.filter(|with| with.length > 0)
    }

    /// The card's sound line for the part at `at` while it plays with the song ([`Page::heard_with`]), which needs
    /// no tempo read: the song's is known. None when it plays alone.
    fn with_song_line(&self, at: usize) -> Option<String> {
        let with = self.heard_with(at)?;
        let song = self.heard_song(at)?;
        Some(midi_fits::with_song_line(&page::gm_sound(self.cat.entry(at)?), &song.name, &with))
    }

    /// The song whose parts the part at `at` plays with ([`Page::heard_with`]): the picked part's is the song of the
    /// list that had it, as it was then.
    fn heard_song(&self, at: usize) -> Option<&SongChoice> {
        let song = self.picked_fit.as_ref().filter(|_| self.picked == Some(at)).map(|picked| &picked.song);
        song.or(self.fits_song.as_ref())
    }

    /// What Play in Bitwig plays the part at `at` with while "Play with the song" is on: the bars of the song it was
    /// fitted to, for Bitwig's arranger to go round ([`midi_fits::bitwig_loop`]), or why Bitwig has no such bars.
    /// None when the part plays alone in the app too ([`Page::heard_with`]): nothing is said of the song then.
    fn bitwig_song(&self, at: usize) -> Option<BitwigSong> {
        self.in_bitwig(at, self.heard_with(at)?)
    }

    /// Where the part at `at` goes in its song in Bitwig: the bars of the song it was fitted to
    /// ([`Page::fitted_with`]), as Bitwig's arranger has them, for Put in the song and for Send to Bitwig's mark.
    /// Whether it's heard with the song or alone makes no difference to where it goes. None for a part that isn't
    /// fitted.
    fn bitwig_place(&self, at: usize) -> Option<BitwigSong> {
        self.in_bitwig(at, self.fitted_with(at)?)
    }

    /// The bars `with` of the song the part at `at` was fitted to, as Bitwig's arranger has them
    /// ([`midi_fits::bitwig_loop`]), or why it has no such bars.
    fn in_bitwig(&self, at: usize, with: Arc<SongBars>) -> Option<BitwigSong> {
        let song = self.heard_song(at)?;
        Some(BitwigSong { bars: midi_fits::bitwig_loop(&with, song), name: song.name.clone(), with })
    }

    /// The card's sound line for the picked part `at`, as it plays now: what of the song it plays with
    /// ([`Page::with_song_line`]); alone, its sound and its own tempo once that's read (`card_alone`), and until
    /// then its sound with no tempo rather than a made-up one.
    fn sound_line_now(&self, at: usize) -> Option<String> {
        self.with_song_line(at)
            .or_else(|| self.card_alone.clone())
            .or_else(|| Some(page::sound_line(&page::gm_sound(self.cat.entry(at)?), None)))
    }

    /// What the picked part's card says about how it's moved to fit ([`midi_fits::card_text`]); "" when it's as it
    /// is in the library.
    fn picked_fit_text(&self) -> String {
        let picked = self.picked_fit.as_ref();
        let said = |picked: &PickedFit| {
            midi_fits::card_text(picked.fit.moved(), picked.fit.yours.is_some(), &picked.song.name)
        };
        picked.map(said).unwrap_or_default()
    }

    /// What the picked part's card says and offers about its fit. The octave buttons are a fitted part's (drums
    /// aside: their notes pick pads).
    fn card_fit(&self) -> CardFit {
        let Some(picked) = self.picked_fit.as_ref() else { return CardFit::default() };
        let drums = self.picked.and_then(|at| self.cat.entry(at)).is_none_or(|e| e.role() == Role::Drums);
        let fit = &picked.fit;
        let place = self.picked.and_then(|at| self.bitwig_place(at));
        CardFit {
            text: self.picked_fit_text(),
            moved: fit.moved() != 0,
            octave: (!drums).then(|| OctaveButtons {
                up: fit.octave_step(1).is_some(),
                down: fit.octave_step(-1).is_some(),
                yours: fit.yours.is_some(),
            }),
            // Only where Bitwig's arranger has the bars: a song from FL Studio takes the part by a drag.
            put: place.filter(|place| place.bars.is_ok()).map(|_| match &self.putting {
                None => PutButton::Put,
                Some(stop) if stop.load(Ordering::SeqCst) => PutButton::Stopping,
                Some(_) => PutButton::Stop,
            }),
        }
    }

    /// Moves the picked part an octave where it's fitted to a song, or gives its octave back to the app. What you
    /// choose is for its notes, wherever they're fitted ([`midi_fits::YourOctaves`]): its line and its card follow at
    /// once. Gives the notes' fingerprint and the octaves to remember for them (none: the app's again), with the rows
    /// of the list whose lines changed. None when there's nothing to change: no fitted part is picked, it can't go
    /// further that way, or its octave is the app's already.
    fn change_octave(&mut self, change: OctaveChange) -> Option<(String, Option<i32>, Vec<usize>)> {
        let at = self.picked?;
        let e = self.cat.entry(at).filter(|e| e.role() != Role::Drums)?;
        let fit = self.picked_fit.as_ref()?.fit;
        let yours = match change {
            OctaveChange::Up => Some(fit.octave_step(1)?),
            OctaveChange::Down => Some(fit.octave_step(-1)?),
            OctaveChange::Apps => fit.yours.map(|_| None)?,
        };
        let hash = e.phrase.hash.clone();
        self.your_octaves.set(&hash, yours);
        if let Some(picked) = self.picked_fit.as_mut() {
            picked.fit.yours = yours;
        }
        // The list's lines for those notes say it too.
        let mut rows = Vec::new();
        if let Some(fits) = self.fits.as_mut() {
            let before = fits.clone();
            midi_fits::with_yours(Arc::make_mut(fits).as_mut_slice(), &self.cat, &self.your_octaves);
            rows = fits
                .iter()
                .zip(before.iter())
                .enumerate()
                .filter(|(_, (now, was))| now != was)
                .map(|(r, _)| r)
                .collect();
        }
        Some((hash, yours, rows))
    }

    /// The notes of the part at `at` as they leave the page, moved as it fits ([`Page::fit_move`]).
    fn part_out(&self, at: usize) -> Option<PartOut> {
        let e = self.cat.entry(at)?;
        let semitones = self.fit_move(at);
        Some(PartOut {
            from: self.notes.clone()?,
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            semitones,
            name: midi_fits::moved_name(&page::title(e), semitones),
            bars: e.phrase.bars,
        })
    }

    /// The ask for the picked part's notes, for its card (the newest ask is the one the card takes the answer of).
    /// None when there's no index or no such part.
    fn notes_ask(&mut self, at: usize) -> Option<NotesAsk> {
        self.notes_asked += 1;
        let part = self.part_out(at)?;
        self.card_move = part.semitones;
        let e = self.cat.entry(at)?;
        Some(NotesAsk {
            id: self.notes_asked,
            part,
            role: e.role(),
            // As the stored key was read; the clip's name only for a part whose key isn't read yet.
            cut: e.phrase.cut.unwrap_or_else(|| page::is_cut(&e.phrase.clip)),
            key: e.phrase.key_fact,
            source: Arc::clone(&e.phrase.source),
            library: self.library.clone(),
            sound: page::gm_sound(e),
        })
    }
}

/// What "Fits my song" can look for, in the order of its switch.
const WANTS: [Wants; 2] = [Wants::Tune, Wants::Chords];

/// Settings › MIDI's smallest part: parts with fewer notes are hidden ([`page::SMALLEST_PART_KEY`]).
static SMALLEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(page::SMALLEST_PART_DEFAULT);

/// Hides parts under `notes` notes from now on (Settings › MIDI); a list on show is worked out again, off the window's
/// thread, so changing it needs no reading again.
pub fn smallest_part(app: Option<&AppWindow>, notes: usize) {
    if SMALLEST.swap(notes, Ordering::Relaxed) == notes {
        return;
    }
    let Some(app) = app else { return };
    let (cat, hidden_pick) = PAGE.with(|p| {
        let p = p.borrow();
        let hidden = p.picked.and_then(|at| p.cat.entry(at)).is_some_and(|e| !page::big_enough(e, notes));
        (p.cat.clone(), hidden)
    });
    if cat.entries.is_empty() {
        return;
    }
    // The picked part is hidden now: its card goes with it.
    if hidden_pick {
        pick(app, None);
    }
    relist(app);
    // Ctrl+K's lines too, off the window's thread: they're a line for every part.
    let weak = app.as_weak();
    let made = background::spawn("MIDI finder lines", move || {
        let lines: Arc<dyn mo_app::finder::Source> = Arc::new(mo_app::finder::midi_parts(&cat, notes));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade()
                && SMALLEST.load(Ordering::Relaxed) == notes
            {
                crate::finder::add_source(&app, "midi", lines);
            }
        });
    });
    if let Err(e) = made {
        log::error("MIDI", &format!("Couldn't start making Ctrl+K's lines: {e}"));
    }
}

/// How long the Ctrl+K read of the index is kept for the page: long enough to go from Ctrl+K to the MIDI page, short
/// enough that a session that never opens it doesn't hold the whole index (M-3). Ctrl+K keeps its own lines.
const KEEP_FINDER_READ: Duration = Duration::from_secs(120);

struct Ask {
    id: u64,
    cat: Arc<Catalogue>,
    filters: Filters,
    order: Order,
    reversed: bool,
    target: Option<Target>,
    seed: Option<u64>,
    /// With "Fits my song" on, the list's lines are its search's: only what each filter's choice would show is
    /// counted, and no list is made (or put in order) to be thrown away.
    counts_only: bool,
}

impl Ask {
    /// The list asked for and its filters' counts; call it off the window's thread.
    fn work_out(&self) -> (Vec<Hit>, page::ChoiceCounts) {
        if self.counts_only {
            return (Vec::new(), page::choice_counts(&self.cat, &self.filters));
        }
        page::view(&self.cat, &self.filters, self.order, self.reversed, self.target, self.seed)
    }
}

/// An ask for the parts that fit a song.
struct FitAsk {
    /// The list it's for ([`Ask::id`]).
    id: u64,
    cat: Arc<Catalogue>,
    filters: Filters,
    song: SongChoice,
    wanted: Option<Wants>,
    /// What of the song is picked by hand.
    yours: midi_fits::YourPick,
    slots_made: u64,
    from: NotesFrom,
    stop: Arc<AtomicBool>,
}

/// A job for the "Fits my song" thread.
enum FitJob {
    /// In a box: an ask is a few hundred bytes, and the other job is none.
    Search(Box<FitAsk>),
    /// The switch went off: what the searches kept (the song, the index's slots) is let go.
    LetGo,
}

/// An ask for a part's notes: the roll, and its key when the index has no reading of the new key finder.
struct NotesAsk {
    id: u64,
    /// The part, and how far the roll and the chords show it moved.
    part: PartOut,
    role: Role,
    cut: bool,
    /// The stored key reading, which the chord chart is spelled in; none: the key is read now, from the notes.
    key: Option<KeyFact>,
    /// The file it's from, whose tempo is read too (for the card's sound line), and the song library that has a
    /// project's.
    source: Arc<mo_midi::Source>,
    library: PathBuf,
    sound: mo_midi::gm::GmSound,
}

struct NotesRead {
    /// How many notes the index gave (the roll leaves out muted ones).
    read: usize,
    roll: Roll,
    fact: Option<KeyFact>,
    /// The chord chart, read just now from the notes.
    chart: page::ChordChart,
}

/// The card's key while the part's notes are read.
const READING: &str = "Reading…";

thread_local! {
    static PAGE: Rc<RefCell<Page>> = Rc::default();
    static LINES: Rc<Lines> = PAGE.with(|p| Rc::new(Lines { page: p.clone(), notify: ModelNotify::default() }));
    static WORKER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    static NOTES: RefCell<Option<Newest<NotesAsk>>> = const { RefCell::new(None) };
    static FITS: RefCell<Option<Newest<FitJob>>> = const { RefCell::new(None) };
    /// Works everything out in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
    /// The filter chips' lines, kept between showings: kinds, From and Key.
    static CHIPS: [FilterModels; 3] = Default::default();
    /// "Fits my song"'s parts to fit to, kept between showings as the filters' are: as chips, and all in the menu.
    static FIT_PARTS: [FilterModels; 2] = Default::default();
    /// The small picture over its bars' slider, kept so a showing that doesn't change it makes nothing again.
    static FIT_PLAYS: Rc<VecModel<f32>> = Rc::default();
    /// Waits for the bars' slider to rest before the picks are written to the settings ([`remember_fit_picks`]).
    static FIT_PICKS_WRITE: slint::Timer = slint::Timer::default();
}

/// How long the bars' slider rests before the picks are written to the settings: a drag says where it is at every
/// step, and each write is of all the settings.
const FIT_PICKS_REST: Duration = Duration::from_millis(400);

struct Lines {
    page: Rc<RefCell<Page>>,
    notify: ModelNotify,
}

impl Model for Lines {
    type Data = MidiLine;

    fn row_count(&self) -> usize {
        self.page.borrow().shown.len()
    }

    fn row_data(&self, row: usize) -> Option<MidiLine> {
        let p = self.page.borrow();
        let hit = p.shown.get(row)?;
        let e = p.cat.entry(hit.entry)?;
        let mut details = page::details(e);
        if let Some(l) = hit.likeness {
            details = format!("{:.0}% alike · {details}", l * 100.0);
        }
        // In a "Fits my song" list: how well it fits, and how far to move it.
        if let Some(fit) = p.fits.as_ref().and_then(|fits| fits.get(row)) {
            details = format!("{} · {details}", midi_fits::fit_text(fit));
        }
        Some(MidiLine {
            row: row as i32,
            title: page::title(e).into(),
            from: page::from_text(e).into(),
            details: details.into(),
            phrase: page::phrase_text(e).into(),
            key: page::key_text(e).into(),
            date: page::date_text(&p.cat.dates, e.phrase.source.mtime).into(),
            bars: if e.phrase.bars > 0 { e.phrase.bars.to_string() } else { String::new() }.into(),
            files: thousands(e.copies()).into(),
            colour: colour(e.role()),
            why: why_text(&e.phrase).into(),
            picked: p.picked == Some(hit.entry),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// A kind's colour, the Sounds page's colour for the same kind of sound (Keys, Lead, Bass, Vocal, Kick), so the two
/// pages agree; grey for something else.
fn colour(role: Role) -> i32 {
    match role {
        Role::Chords => 11,
        Role::Melody => 9,
        Role::Bass => 8,
        Role::Arp => 14,
        Role::Drums => 0,
        Role::Other => -1,
    }
}

fn why_text(p: &StoredPhrase) -> String {
    if p.your_role.is_some() { "You said so".to_owned() } else { p.role_why.clone() }
}

/// Wires the page up; the index is read the first time the page is shown ([`shown`]).
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed, shortcuts: &Shortcuts) {
    crate::finder::on_open(Kind::MidiPart, |app, hit, _| {
        crate::go_to(app, Place::Midi.index() as i32);
        show_part(app, &hit.id);
        true
    });
    // Ctrl+K finds your parts before this page is first shown: the index is read once for its list, in the
    // background, without the reading of your MIDI the page itself starts.
    crate::finder::when_opening(|app| {
        let library = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let first = !p.started && !p.library.as_os_str().is_empty() && !std::mem::replace(&mut p.finder_read, true);
            first.then(|| p.library.clone())
        });
        let Some(library) = library else { return };
        let weak = app.as_weak();
        let started = background::spawn("MIDI index for Ctrl+K", move || {
            let loaded = load(&library);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(app) = weak.upgrade() else { return };
                // The page showed meanwhile and is waiting for this read: it takes it (and hands Ctrl+K its list).
                if PAGE.with(|p| std::mem::take(&mut p.borrow_mut().finder_for_page)) {
                    show(&app, loaded);
                    return;
                }
                if let Ok(l) = &loaded {
                    crate::finder::add_source(&app, "midi", l.found.clone());
                }
                PAGE.with(|p| p.borrow_mut().finder_loaded = Some(loaded));
                slint::Timer::single_shot(KEEP_FINDER_READ, || {
                    let unused = PAGE.with(|p| {
                        let mut p = p.borrow_mut();
                        let unused = p.finder_loaded.take();
                        if unused.is_some() {
                            p.finder_let_go = true;
                        }
                        unused
                    });
                    if let Some(unused) = unused {
                        background::free_later(unused);
                    }
                });
            });
        });
        if let Err(e) = started {
            log::error("MIDI", &format!("Couldn't start reading the index for Ctrl+K: {e}"));
            // Nothing is coming, so the page reads it itself.
            PAGE.with(|p| p.borrow_mut().finder_read = false);
        }
    });
    // "Play with the song" is on until you turn it off.
    let with_song = crate::settings::value(midi_fits::WITH_SONG_KEY).is_none_or(|v| v != "0");
    // What you picked of each song for "Fits my song" to fit to, the last time.
    let picks = crate::settings::value(midi_fits::PICKS_KEY).unwrap_or_default();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
        p.with_song = with_song;
        p.fit_picks = midi_fits::YourPicks::read(&picks);
    });
    app.global::<MidiData>().set_fit_with_song(with_song);
    wire(app);
    crate::piano_roll::connect(app);
    show_keys(app, shortcuts);
    let data = app.global::<MidiData>();
    data.set_empty_title("Reading your MIDI…".into());
    data.set_empty_text("".into());
    if app.get_place() == mo_app::places::Place::Midi.index() as i32 {
        shown(app);
    }
}

/// The keys as they are now, from the one shortcut list (rule 15): the play row's, and the picked part's card and
/// menu. Again after a key is changed in Settings, so none shows an old key.
pub fn show_keys(app: &AppWindow, shortcuts: &Shortcuts) {
    let data = app.global::<MidiData>();
    data.set_play_key(shortcuts.key_of("app.play").into());
    data.set_bitwig_key(shortcuts.key_of("midi.playBitwig").into());
    data.set_fl_key(shortcuts.key_of("midi.playFl").into());
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.shortcuts = shortcuts.clone();
        show_actions(&data, &p);
    });
}

/// The picked part's buttons, its octave buttons and its right-click menu, as the page has them now: the part, how
/// it's fitted, and the keys.
fn show_actions(data: &MidiData, p: &Page) {
    let Some(e) = p.picked.and_then(|at| p.cat.entry(at)) else { return };
    let fit = p.card_fit();
    let (card, octave, menu) = actions(e, &p.shortcuts, fit.octave, fit.put);
    data.set_actions(ModelRc::new(VecModel::from(card)));
    data.set_octave_actions(ModelRc::new(VecModel::from(octave)));
    data.set_menu(ModelRc::new(VecModel::from(menu)));
}

/// Puts what the page says about the picked part's fit on its card ([`Page::card_fit`]): the line over the roll, the
/// octave buttons and the menu's octave lines. Only when it's another than the card has, so a list that leaves the
/// fit as it was redraws nothing.
fn show_card_fit(data: &MidiData, p: &mut Page) {
    let fit = p.card_fit();
    let mut card = data.get_picked();
    // By the card on screen as well as by what was last put there, so the two can't drift apart.
    let said = card.fit == fit.text.as_str() && card.fit_moved == fit.moved;
    if said && fit == p.card_fit {
        return;
    }
    if !said {
        card.fit = fit.text.as_str().into();
        card.fit_moved = fit.moved;
        data.set_picked(card);
    }
    p.card_fit = fit;
    show_actions(data, p);
}

/// Lights the heading of the column the list goes by, with which way (the UX review of 10 Nov, B5); none while the
/// list is shuffled or ranked (most alike, longest phrase, best fit), which no column stands for.
fn show_order(data: &MidiData) {
    let (column, reversed) = PAGE.with(|p| {
        let p = p.borrow();
        let ranked = p.seed.is_some() || p.target.is_some() || p.fitting();
        if ranked { ("", false) } else { (p.order.column(), p.reversed) }
    });
    data.set_order_column(column.into());
    data.set_order_reversed(reversed);
}

fn wire(app: &AppWindow) {
    let data = app.global::<MidiData>();
    data.set_lines(ModelRc::from(LINES.with(Rc::clone) as Rc<dyn Model<Data = MidiLine>>));
    show_order(&data);
    show_chips(app);

    let weak = app.as_weak();
    data.on_search_edited(move |text| {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.filters.text = text.to_string();
                p.seed = None;
            });
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_toggle(move |name, choice| {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().filters.toggle(&name, &choice));
            show_chips(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear_chip(move |name| {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().filters.clear(&name));
            show_chips(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_pick(move |what, value| {
        let Some(app) = weak.upgrade() else { return };
        if what == "order" {
            // A click on a heading: the list goes by its column, or turns round when it does already; a shuffle
            // ends.
            let order = Order::from_column(&value);
            let fitting = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let lit = p.seed.is_none() && p.target.is_none() && !p.fitting() && p.order == order;
                p.reversed = lit && !p.reversed;
                p.order = order;
                p.seed = None;
                p.fitting()
            });
            // The parts that fit a song are in the order of their fit: the heading counts once that's off, and
            // there's nothing to work out again now.
            if fitting {
                return;
            }
        }
        relist(&app);
    });
    let weak = app.as_weak();
    data.on_shuffle(move || {
        // The parts that fit a song stay in the order of their fit.
        if PAGE.with(|p| p.borrow().fitting()) {
            return;
        }
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().seed = Some(new_seed()));
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear_target(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().target = None);
            relist(&app);
        }
    });
    data.set_fit_wants_words(strings(WANTS.map(midi_fits::wants_words)));
    let weak = app.as_weak();
    data.on_fit_song_picked(move |label| {
        if let Some(app) = weak.upgrade() {
            pick_fit_song(&app, &label);
        }
    });
    let weak = app.as_weak();
    data.on_fit_wants_picked(move |place| {
        if let (Some(app), Some(&wants)) = (weak.upgrade(), usize::try_from(place).ok().and_then(|i| WANTS.get(i))) {
            fit_wants_picked(&app, wants);
        }
    });
    let weak = app.as_weak();
    data.on_fit_part_toggled(move |label| {
        if let Some(app) = weak.upgrade() {
            fit_pick_pressed(&app, true, |picker| picker.toggle_label(&label));
        }
    });
    let weak = app.as_weak();
    data.on_fit_parts_cleared(move || {
        if let Some(app) = weak.upgrade() {
            // The menu's "The app's pick": parts of yours go back to the app. Parts that are the app's already
            // stay as they are (the line is ticked, and a press on it has nothing to change).
            fit_pick_pressed(&app, true, |picker| {
                if picker.your_parts {
                    picker.apps_parts();
                }
            });
        }
    });
    let weak = app.as_weak();
    data.on_fit_bars_picked(move |low, high| {
        if let Some(app) = weak.upgrade() {
            // Said at every step of a drag: the list follows at once, the setting is written when it rests.
            fit_pick_pressed(&app, false, |picker| picker.slide_bars(low.into(), high.into()));
        }
    });
    let weak = app.as_weak();
    data.on_fit_apps_pick(move || {
        if let Some(app) = weak.upgrade() {
            fit_pick_pressed(&app, true, midi_fits::Picker::apps_pick);
        }
    });
    let weak = app.as_weak();
    data.on_fit_with_song_toggled(move || {
        if let Some(app) = weak.upgrade() {
            play_with_song(&app, !PAGE.with(|p| p.borrow().with_song));
        }
    });
    let weak = app.as_weak();
    data.on_years_picked(move |a, b| {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.filters.years = mo_app::year_strip::year_span(&p.years, a.min(b), a.max(b));
            });
            show_years(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_years_cleared(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| p.borrow_mut().filters.years = None);
            show_years(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear(move || {
        if let Some(app) = weak.upgrade() {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.filters = Filters::default();
                p.target = None;
                p.seed = None;
            });
            let data = app.global::<MidiData>();
            data.set_search("".into());
            show_chips(&app);
            show_years(&app);
            relist(&app);
        }
    });
    let weak = app.as_weak();
    data.on_select(move |row| {
        if let (Some(app), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
            select_row(&app, row);
        }
    });
    let weak = app.as_weak();
    data.on_step(move |by| {
        if let Some(app) = weak.upgrade() {
            step(&app, by);
        }
    });
    let weak = app.as_weak();
    data.on_close_panel(move || {
        if let Some(app) = weak.upgrade() {
            pick(&app, None);
        }
    });
    let weak = app.as_weak();
    data.on_read_again(move || {
        if let Some(app) = weak.upgrade() {
            read_midi(&app, true);
        }
    });
    data.on_stop_reading(stop_reading);
    let weak = app.as_weak();
    data.on_menu_chosen(move |id| {
        if let Some(app) = weak.upgrade() {
            run_action(&app, &id);
        }
    });
    let weak = app.as_weak();
    data.on_drag_row(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let at = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().shown.get(r).map(|h| h.entry)));
        if let Some(at) = at {
            carry(&app, at, Spot::Row(row));
        }
    });
    let weak = app.as_weak();
    data.on_drag_picked(move || {
        let Some(app) = weak.upgrade() else { return };
        if let Some(at) = PAGE.with(|p| p.borrow().picked) {
            carry(&app, at, Spot::Picked);
        }
    });
    let weak = app.as_weak();
    data.on_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            crate::drag::let_go(&app);
        }
    });
    let weak = app.as_weak();
    data.on_play_toggled(move || {
        if let Some(app) = weak.upgrade() {
            toggle_play(&app);
        }
    });
    let weak = app.as_weak();
    data.on_choose_soundfont(move || {
        if let Some(app) = weak.upgrade() {
            choose_soundfont(&app);
        }
    });
    let weak = app.as_weak();
    data.on_daw_play(move |daw| {
        if let Some(app) = weak.upgrade()
            && let Some(daw) = Daw::from_id(&daw)
        {
            toggle_daw(&app, daw);
        }
    });
}

/// The page is on show: the first time, the index is read.
pub fn shown(app: &AppWindow) {
    // Another page may have played something since: the Play button says what the player is doing.
    // (Not while the loop is still being made: it hasn't reached the player yet.)
    let asked = PLAY_ASKED.load(std::sync::atomic::Ordering::SeqCst);
    if app.global::<MidiData>().get_playing()
        && PLAY_STARTED.load(std::sync::atomic::Ordering::SeqCst) == asked
        && !loop_playing()
    {
        app.global::<MidiData>().set_playing(false);
        PAGE.with(|p| p.borrow_mut().looping = None);
    }
    // A SoundFont put in its folder since is found now.
    show_soundfont(app);
    let library = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // Before `connect` (the app opening on this page) there's no library yet: `connect` asks again.
        if p.started || p.library.as_os_str().is_empty() {
            return None;
        }
        p.started = true;
        Some(p.library.clone())
    });
    let Some(library) = library else { return };
    // Ctrl+K read the index already: the page takes that read, or waits for it if it's still going. The read goes in a
    // Box: on Windows, where a PathBuf is bigger, it's over clippy's size limit for one arm of an enum.
    enum Kept {
        Read(Box<Result<Loaded, String>>),
        Coming,
        None,
    }
    let kept = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        match p.finder_loaded.take() {
            Some(read) => Kept::Read(Box::new(read)),
            None if p.finder_read && !p.finder_let_go => {
                p.finder_for_page = true;
                Kept::Coming
            }
            None => Kept::None,
        }
    });
    match kept {
        Kept::Read(read) => return show(app, *read),
        Kept::Coming => return,
        Kept::None => {}
    }
    let weak = app.as_weak();
    let started = background::spawn("MIDI index", move || {
        let loaded = load(&library);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                show(&app, loaded);
            }
        });
    });
    if let Err(e) = started {
        show(app, Err(format!("its reader couldn't start ({e})")));
    }
}

/// Reads the Rust app's MIDI index beside the library, read-only: brought up to this app's layout first, with the
/// corrections you made in the old app taken in, the first time ([`mo_midi::store::bring_up_to_date`]). No index yet
/// is an empty list: the page's first showing reads your MIDI into one.
pub fn load(library: &Path) -> Result<Loaded, String> {
    let path = MidiStore::path_beside(library);
    let up = mo_midi::store::bring_up_to_date(&path);
    for line in &up.done {
        log::info("MIDI", line);
    }
    for line in &up.later {
        log::warning("MIDI", line);
    }
    if !path.exists() {
        return Ok(Loaded::new(Catalogue::default(), NotesFrom::Store(path)));
    }
    let store = MidiStore::open_read_only(&path).map_err(|e| e.to_string())?;
    let phrases = store.all(&up.old_roles, &up.old_octaves).map_err(|e| e.to_string())?;
    drop(store);
    let cat = background::on_pool(|| Catalogue::new(phrases));
    Ok(Loaded::new(cat, NotesFrom::Store(path)))
}

/// The index has been read: the page shows it.
pub fn show(app: &AppWindow, loaded: Result<Loaded, String>) {
    let data = app.global::<MidiData>();
    let loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            log::warning("MIDI", &format!("The MIDI index couldn't be read: {e}"));
            data.set_summary("".into());
            data.set_empty_title("The MIDI couldn't be read".into());
            data.set_empty_text(format!("The index said: {e}").into());
            // Reading your MIDI makes a new index of its own, so it still starts the first time.
            let read_now = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let read_now = !p.read_once && !HERE.with(|h| *h.borrow());
                p.read_once = true;
                read_now
            });
            if read_now {
                read_midi(app, false);
            }
            return;
        }
    };
    crate::finder::add_source(app, "midi", loaded.found.clone());
    let summary = loaded.summary;
    let empty = loaded.cat.entries.is_empty();
    log::info("MIDI", &summary);
    let (picked, read_now, loop_gone) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // The picked part stays picked when the index is read again, found by its notes; so does the one playing.
        let hash = p.picked.and_then(|at| p.cat.entry(at)).map(|e| e.phrase.hash.clone());
        let looping = p.looping.and_then(|at| p.cat.entry(at)).map(|e| e.phrase.hash.clone());
        p.years = loaded.years;
        p.summary = summary.clone();
        // The old index goes on the freeing thread: tens of thousands of parts take a while to let go.
        background::free_later(std::mem::replace(&mut p.cat, Arc::new(loaded.cat)));
        p.notes = Some(loaded.notes);
        // Not one Settings now hides.
        let smallest = SMALLEST.load(Ordering::Relaxed);
        p.picked = hash
            .and_then(|h| p.cat.find(&h))
            .filter(|&at| p.cat.entry(at).is_some_and(|e| page::big_enough(e, smallest)));
        // Still picked, it stays moved as it was until the list is worked out again ([`Page::fit_picked`]).
        if p.picked.is_none() {
            p.picked_fit = None;
        }
        let was_looping = p.looping.is_some();
        p.looping = looping.and_then(|h| p.cat.find(&h));
        p.picked_row = None;
        p.target = None;
        // The old list points into the old index: nothing shows until the new one is worked out.
        background::free_later(std::mem::replace(&mut p.shown, Arc::new(Vec::new())));
        p.fits = None;
        p.fits_song = None;
        p.fit_with = None;
        let read_now = !p.read_once && !HERE.with(|h| *h.borrow());
        p.read_once = true;
        (p.picked, read_now, was_looping && p.looping.is_none())
    });
    // The part playing isn't in the index any more: it stops, as its card (with its Stop) goes.
    if loop_gone {
        stop_playing(app);
    }
    LINES.with(|l| l.notify.reset());
    data.set_shown(0);
    data.set_summary(format!("{summary}{}", loaded.hidden).into());
    if empty {
        data.set_empty_title("No MIDI here yet".into());
        data.set_empty_text(
            "Every part in your projects and MIDI folders shows up here once it's been read. The files are only \
             read, never changed. MIDI folders are the ones set in the old app's MIDI page for now."
                .into(),
        );
    } else {
        data.set_empty_title("".into());
        data.set_empty_text("".into());
    }
    show_chips(app);
    show_years(app);
    relist(app);
    match picked {
        Some(at) => pick(app, Some((at, None))),
        None => data.set_panel_shown(false),
    }
    if let Some(hash) = PAGE.with(|p| p.borrow_mut().wanted.take()) {
        show_part(app, &hash);
    }
    if read_now {
        read_midi(app, false);
    }
}

/// Reads your MIDI into the Rust app's own index, `midi-index.db` beside the library: the newest save of each song
/// (every save with the old app's "older saves" setting) and every MIDI file in the MIDI folders (the old app's list
/// for now). Only new and changed files are read, newest first; `retry` reads the ones that failed before too. The
/// files are only read. When it's done the page reads the index again, so every line has the new key finder's key.
pub fn read_midi(app: &AppWindow, retry: bool) {
    let Some((library, feed, stop)) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.reading.is_some() || p.library.as_os_str().is_empty() {
            return None;
        }
        let feed = p.feed.clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        p.reading = Some(stop.clone());
        Some((p.library.clone(), feed, stop))
    }) else {
        return;
    };
    let data = app.global::<MidiData>();
    data.set_reading(true);
    data.set_busy("Looking for new and changed projects and MIDI files…".into());
    data.set_done(0.0);
    let job = feed.start("Reading MIDI", "Looking for new and changed projects and MIDI files");
    let weak = app.as_weak();
    let started = background::spawn("MIDI reading", move || {
        let progress_weak = weak.clone();
        let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        let stage = std::sync::Mutex::new(page::ReadProgress::default());
        let progress = |done: usize, total: usize, path: &str| {
            let text = stage.lock().unwrap_or_else(std::sync::PoisonError::into_inner).text(done, total, path);
            let fraction = if total == 0 { 0.0 } else { done as f32 / total as f32 };
            job.progress(fraction, Some(&text));
            // The page's line at most four times a second.
            let mut last = last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.elapsed() >= Duration::from_millis(250) {
                *last = Instant::now();
                let weak = progress_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        let data = app.global::<MidiData>();
                        if data.get_reading() {
                            data.set_busy(text.into());
                            data.set_done(fraction);
                        }
                    }
                });
            }
        };
        let ran = read_into_index(&library, retry, &stop, &progress);
        match &ran {
            Ok(s) => {
                let (title, detail) = page::read_done_text(s);
                log::info("MIDI", &format!("{title}: {detail}"));
                job.done(&format!("{title}: {detail}"));
            }
            Err(e) => {
                log::warning("MIDI", &format!("Reading MIDI failed: {e}"));
                job.failed(e);
            }
        }
        // What was read shows straight away; after a failed reading too, as it may have saved some before failing.
        let changed = ran.as_ref().map_or(true, |s| s.read > 0 || s.forgotten > 0 || s.rekeyed > 0 || s.shaped > 0);
        // Slots alone change no line of the list, only what "Fits my song" can look at.
        let slots_made = ran.as_ref().is_ok_and(|s| s.slots > 0);
        let loaded = changed.then(|| load(&library));
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            PAGE.with(|p| p.borrow_mut().reading = None);
            let data = app.global::<MidiData>();
            data.set_reading(false);
            data.set_busy("".into());
            if let Some(loaded) = loaded {
                show(&app, loaded);
            } else if slots_made {
                let fitting = PAGE.with(|p| {
                    let mut p = p.borrow_mut();
                    p.fit_slots_made += 1;
                    p.fitting()
                });
                if fitting {
                    relist(&app);
                }
            }
        });
    });
    if let Err(e) = started {
        log::error("MIDI", &format!("The MIDI reader couldn't start: {e}"));
        PAGE.with(|p| p.borrow_mut().reading = None);
        data.set_reading(false);
        data.set_busy("".into());
    }
}

/// The reading itself, on its own thread: what to read, then the indexer on a pool of low-priority threads.
fn read_into_index(
    library: &Path,
    retry: bool,
    stop: &AtomicBool,
    progress: &(dyn Fn(usize, usize, &str) + Sync),
) -> Result<indexer::Summary, String> {
    let lib = Library::open_read_only(library).map_err(|e| e.to_string())?;
    let saves = lib.saves().map_err(|e| e.to_string())?;
    let choices = lib.song_choices().map_err(|e| e.to_string())?;
    // A save waiting on a fix is being changed by the app: it's read once that's done.
    let pending = lib.pending_fixes().map_err(|e| e.to_string())?;
    let older = lib.setting(page::OLDER_SAVES_KEY).ok().flatten().as_deref() == Some("1");
    let listed = lib.list_setting(page::FOLDERS_KEY).unwrap_or_default();
    drop(lib);
    // Only a full path can be an unplugged drive; anything else in the list is never found and so never counted as away.
    let (folders, away): (Vec<String>, Vec<String>) =
        listed.into_iter().filter(|f| mo_formats::paths::is_absolute(f)).partition(|f| Path::new(f).is_dir());
    // The folder walk and the reading run on the bulk pool: a few threads keep one drive (L:, holding every pack) busy,
    // where one per core made it jump between files, and the pages' quick jobs on the main pool aren't held up.
    let found = background::on_bulk_pool(|| {
        mo_library::scan::find_files(
            &folders,
            &|path: &str| mo_formats::midi::is_midi_path(path),
            &|name: &str, _parent: &str| mo_sounds::scan::is_hidden_folder(name),
        )
    });
    // A folder that couldn't be read this time keeps what was read from it, as an unplugged drive does.
    let away: Vec<String> = away.into_iter().chain(found.unreadable).collect();
    let wanted = page::wanted(saves, &pending, &choices, older, &found.files);
    let mut store = MidiStore::open(&MidiStore::path_beside(library)).map_err(|e| e.to_string())?;
    let stamps = store.stamps().map_err(|e| e.to_string())?;
    let plan = indexer::plan(&wanted, &stamps, &away, retry);
    log::info(
        "MIDI",
        &format!("{} files wanted, {} to read, {} to forget", wanted.len(), plan.todo.len(), plan.forget.len()),
    );
    let own;
    let pool = match background::bulk_pool() {
        Some(pool) => pool,
        None => {
            own = rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .thread_name(|i| format!("MIDI reader {i}"))
                .start_handler(|_| background::lower_priority())
                .build()
                .map_err(|e| e.to_string())?;
            &own
        }
    };
    let threads = pool.current_num_threads();
    let started = std::time::Instant::now();
    let summary = indexer::run(&mut store, &plan, &folders, pool, threads, stop, progress).map_err(|e| e.to_string());
    log::info("MIDI", &format!("read with {threads} readers in {:.1} s", started.elapsed().as_secs_f64()));
    summary
}

/// Stops the reading at the next file; what's been read is kept, and the next reading carries on from there.
fn stop_reading() {
    PAGE.with(|p| {
        if let Some(stop) = p.borrow().reading.as_ref() {
            stop.store(true, Ordering::Relaxed);
        }
    });
}

/// The filter chips for the picks now and the last list's counts.
fn show_chips(app: &AppWindow) {
    let chips = PAGE.with(|p| {
        let p = p.borrow();
        page::chips(&p.counts, &p.filters)
    });
    let data = app.global::<MidiData>();
    CHIPS.with(|[kind, from, key]| {
        data.set_kind_filter(kind.view(&chips.kind));
        data.set_from_filter(from.view(&chips.from));
        data.set_key_filter(key.view(&chips.key));
    });
}

fn show_years(app: &AppWindow) {
    let (bars, (from, to)) = PAGE.with(|p| {
        let p = p.borrow();
        let most = p.years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        let bars = p
            .years
            .iter()
            .map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most })
            .collect::<Vec<_>>();
        (bars, mo_app::year_strip::picked_places(&p.years, p.filters.years).unwrap_or((-1, -1)))
    });
    let data = app.global::<MidiData>();
    data.set_years(ModelRc::new(VecModel::from(bars)));
    data.set_years_from(from);
    data.set_years_to(to);
}

/// Works the list out again for the filters now: off the window's thread, newest ask first.
fn relist(app: &AppWindow) {
    let (ask, fit) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.asked += 1;
        let mut ask = Ask {
            id: p.asked,
            cat: p.cat.clone(),
            filters: Filters { smallest: SMALLEST.load(Ordering::Relaxed), ..p.filters.clone() },
            order: p.order,
            reversed: p.reversed,
            target: p.target,
            seed: p.seed,
            counts_only: false,
        };
        // With "Fits my song" on, the list is its search's; this ask still gives the filters their counts.
        let fit = fit_ask(&mut p, &ask);
        ask.counts_only = fit.is_some();
        (ask, fit)
    });
    let data = app.global::<MidiData>();
    data.set_any_filter(PAGE.with(|p| p.borrow().filters.any()));
    if HERE.with(|h| *h.borrow()) {
        let (hits, counts) = ask.work_out();
        listed(app, ask.id, hits, counts);
        if let Some(fit) = fit {
            let answer = fit_search(&fit, &mut FitKept::default());
            fit_listed(app, fit.id, answer);
        }
        return;
    }
    if let Some(fit) = fit {
        // Until the search answers: said under the toolbar, and in the list's place if it has no lines (after the
        // index was read again, or a search that found none).
        data.set_result(midi_fits::LOOKING.into());
        let looking = midi_fits::Empty { title: midi_fits::LOOKING, text: String::new(), filters: false };
        show_fit_none(&data, Some(looking));
        fit_job(app, FitJob::Search(Box::new(fit)));
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("MIDI list", move |ask: Ask| {
                let (hits, counts) = background::on_pool(|| ask.work_out());
                let weak = weak.clone();
                let id = ask.id;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        listed(&app, id, hits, counts);
                    }
                });
            }) {
                Ok(n) => *w = Some(n),
                Err(e) => log::error("MIDI", &format!("The list's worker couldn't start: {e}")),
            }
        }
        if let Some(w) = w.as_ref() {
            // As for the search above: the ask this takes the place of isn't let go on the window's thread.
            w.ask_freeing_later(ask);
        }
    });
}

fn listed(app: &AppWindow, id: u64, hits: Vec<Hit>, counts: page::ChoiceCounts) {
    let (shown, result, target) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.asked != id {
            return (None, String::new(), String::new());
        }
        p.counts = counts;
        // With "Fits my song" on, the list is its search's ([`fit_listed`]): this one only gives the filters'
        // choices their counts.
        if p.fitting() {
            background::free_later(hits);
            return (None, String::new(), String::new());
        }
        // The picked part's row in the new list: one look, here, rather than at every step.
        p.picked_row = p.picked.and_then(|at| page::row_of(&p.cat, &hits, at));
        // The old list (one line per part shown) is let go on the freeing thread.
        background::free_later(std::mem::replace(&mut p.shown, Arc::new(hits)));
        p.fits = None;
        p.fits_song = None;
        p.fit_with = None;
        p.fit_picked();
        // Nothing plays with a song in a plain list.
        forget_song_loop();
        let target = p.target.and_then(|t| {
            let at = match t {
                Target::Like(at) | Target::Within(at) => at,
            };
            p.cat.entry(at).map(|e| (t, page::title(e)))
        });
        let target_text = match &target {
            Some((Target::Like(_), title)) => format!("Parts shaped like {title}"),
            Some((Target::Within(_), title)) => format!("Longer phrases with {title} in them"),
            None => String::new(),
        };
        let shown = p.shown.len();
        let all = p.counts.lines;
        let result = page::result_text(shown, all, p.order, p.reversed, p.target, p.seed.is_some());
        (Some(shown), result, target_text)
    });
    let Some(shown) = shown else {
        // The counts were this list's to give even so.
        if PAGE.with(|p| p.borrow().asked == id) {
            show_counts(app);
        }
        return;
    };
    show_counts(app);
    LINES.with(|l| l.notify.reset());
    let data = app.global::<MidiData>();
    data.set_shown(shown as i32);
    data.set_result(result.into());
    data.set_target(target.into());
    show_fit_none(&data, None);
    show_order(&data);
    let scroll = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let row = p.picked_row.filter(|_| p.scroll_to_pick);
        if row.is_some() {
            p.scroll_to_pick = false;
        }
        row
    });
    if let Some(row) = scroll {
        data.set_show_row(row as i32);
        data.set_show_ask(data.get_show_ask() + 1);
    }
    fit_follows(app);
}

/// What the last list worked out says beside the list: what Settings' smallest part hides, after the summary, and
/// how many parts each filter's choice would show.
fn show_counts(app: &AppWindow) {
    let summary = PAGE.with(|p| {
        let p = p.borrow();
        let smallest = SMALLEST.load(Ordering::Relaxed);
        format!("{}{}", p.summary, page::hidden_text(&p.cat, p.counts.lines, smallest))
    });
    app.global::<MidiData>().set_summary(summary.into());
    show_chips(app);
}

/// The search "Fits my song" wants for the list now asked for; none while it's off (or there's no index yet). A
/// search still running is for an older list: it's told to stop.
fn fit_ask(p: &mut Page, ask: &Ask) -> Option<FitAsk> {
    p.fit_stop.store(true, Ordering::Relaxed);
    let (song, from) = (p.fit_song.clone()?, p.notes.clone()?);
    p.fit_stop = Arc::new(AtomicBool::new(false));
    Some(FitAsk {
        id: ask.id,
        cat: ask.cat.clone(),
        filters: ask.filters.clone(),
        song,
        wanted: p.fit_wants,
        yours: p.fit_yours.clone(),
        slots_made: p.fit_slots_made,
        from,
        stop: p.fit_stop.clone(),
    })
}

/// What the "Fits my song" thread has from one search to the next: what the searches made (the song, the index's
/// slots), the index open, and whether a search has been logged since it had nothing.
#[derive(Default)]
struct FitKept {
    kept: midi_fits::Kept,
    open: Option<(PathBuf, MidiStore)>,
    logged: bool,
}

/// Hands the "Fits my song" thread a job, starting the thread the first time there's a search for it. Every search
/// answers ([`fit_listed`]): one that breaks, or a thread that can't start, says so in the list's place rather than
/// leaving the page looking.
fn fit_job(app: &AppWindow, job: FitJob) {
    let not_started = FITS.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() && matches!(job, FitJob::Search(_)) {
            let weak = app.as_weak();
            let mut state = FitKept::default();
            match Newest::start("MIDI fits", move |job: FitJob| {
                let ask = match job {
                    FitJob::Search(ask) => ask,
                    FitJob::LetGo => {
                        background::free_later(std::mem::take(&mut state.kept));
                        state.open = None;
                        state.logged = false;
                        return;
                    }
                };
                // A search that trips (it's logged as a crash) still answers, and what it kept may be half made:
                // the next starts afresh.
                let answer = background::guarded(|| fit_search(&ask, &mut state)).unwrap_or_else(|broke| {
                    state = FitKept::default();
                    Answer::Nothing(format!("The search didn't finish: {broke}."))
                });
                // The ask ends here, on this thread: it may hold the last of an index read again since.
                let id = ask.id;
                drop(ask);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        fit_listed(&app, id, answer);
                    }
                });
            }) {
                Ok(n) => *w = Some(n),
                Err(e) => {
                    log::error("MIDI", &format!("The search for parts that fit couldn't start: {e}"));
                    return Some((job, e.to_string()));
                }
            }
        }
        if let Some(w) = w.as_ref() {
            // An ask it takes the place of may hold the last of an index read again since: not let go here.
            w.ask_freeing_later(job);
        }
        None
    });
    if let Some((FitJob::Search(ask), why)) = not_started {
        fit_listed(app, ask.id, Answer::Nothing(format!("The search couldn't start ({why}).")));
    }
}

/// A search that took this long is worth a line in the log each time; quicker ones only the first.
const SLOW_SEARCH: Duration = Duration::from_millis(300);

/// One search, on the thread that asks.
fn fit_search(ask: &FitAsk, state: &mut FitKept) -> Answer {
    let started = Instant::now();
    let own;
    let pool = match background::pool() {
        Some(pool) => pool,
        None => {
            let made = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .thread_name(|_| "MIDI fits".to_owned())
                .start_handler(|_| background::lower_priority())
                .build();
            match made {
                Ok(pool) => {
                    own = pool;
                    &own
                }
                Err(e) => return Answer::Nothing(format!("The search couldn't start ({e}).")),
            }
        }
    };
    let mut reads = FitReads { from: &ask.from, cat: &ask.cat, open: &mut state.open };
    let asked = midi_fits::Asked {
        song: &ask.song,
        wanted: ask.wanted,
        yours: &ask.yours,
        cat: &ask.cat,
        filters: &ask.filters,
        slots_made: ask.slots_made,
    };
    let answer = midi_fits::search(&mut state.kept, &mut reads, &asked, pool, &ask.stop);
    // How long a search of the real index takes is measured where it runs. Not a line for every letter typed in the
    // search box: the first search (it reads the song and the index's slots too), and any that's slow.
    if let Answer::Fits { looked, .. } = &answer {
        let whole = started.elapsed();
        if !state.logged || whole >= SLOW_SEARCH {
            state.logged = true;
            let line = midi_fits::log_line(looked);
            log::info("MIDI", &format!("Fits my song: {line}; {:.2} s with what it read first", whole.as_secs_f64()));
        }
    }
    answer
}

/// Where a "Fits my song" search reads from ([`midi_fits::Reads`]): the song's save on disk and the MIDI index, kept
/// open from one search to the next; or, for the screenshots, the made-up parts.
struct FitReads<'a> {
    from: &'a NotesFrom,
    cat: &'a Catalogue,
    open: &'a mut Option<(PathBuf, MidiStore)>,
}

impl midi_fits::Reads for FitReads<'_> {
    fn written(&mut self, save: &str) -> i64 {
        match self.from {
            NotesFrom::Made(_) => 1,
            NotesFrom::Store(_) => written_now(save),
        }
    }

    fn song(&mut self, save: &str) -> Result<Song, String> {
        match self.from {
            NotesFrom::Made(notes) => Ok(made_song(self.cat, notes, save)),
            NotesFrom::Store(_) => mo_midi::playing::read(Path::new(save)).map_err(|e| match e {
                mo_midi::extract::Unread::InUse(why) | mo_midi::extract::Unread::Failed(why) => why,
            }),
        }
    }

    fn slots(&mut self) -> Result<SlotBook, String> {
        match self.from {
            NotesFrom::Made(notes) => {
                let mut book = SlotBook::default();
                for (hash, notes) in notes.iter() {
                    book.add(hash, &mo_midi::fit::slots_of(notes));
                }
                Ok(book)
            }
            NotesFrom::Store(path) => from_index(path, self.open, MidiStore::slots),
        }
    }

    fn notes(&mut self, id: i64, hash: &str) -> Option<Vec<NoteEvent>> {
        notes_of(self.from, id, hash, self.open).ok()
    }
}

/// When the file at `save` was last written, as the disk says now, in milliseconds; 0 when it can't be told.
fn written_now(save: &str) -> i64 {
    let since = std::fs::metadata(save).and_then(|m| m.modified()).ok();
    let since = since.and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());
    since.map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// For the screenshots: the song a made-up save "holds", which is the made-up parts that say they're from it, each
/// on a track of its own from the song's first bar.
fn made_song(cat: &Catalogue, notes: &HashMap<String, Vec<NoteEvent>>, save: &str) -> Song {
    use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewLane, PreviewNote};
    let beats = |ticks: i64| ticks as f64 / mo_midi::PPQ as f64;
    let lanes = cat
        .entries
        .iter()
        .filter(|e| e.phrase.source.path == save)
        .map(|e| {
            let notes = notes.get(&e.phrase.hash).map(Vec::as_slice).unwrap_or_default();
            let notes = notes
                .iter()
                .map(|n| PreviewNote {
                    start: beats(n.start),
                    length: beats(n.length),
                    key: n.key,
                    velocity: n.velocity,
                    ..PreviewNote::default()
                })
                .collect();
            let length = beats(i64::from(e.phrase.bars) * mo_midi::BAR);
            let clip = PreviewClip { name: e.phrase.clip.clone(), start: 0.0, length, notes, ..PreviewClip::default() };
            PreviewLane { clips: vec![clip], ..PreviewLane::new(&e.phrase.part, LaneKind::Instrument) }
        })
        .collect();
    // A made-up Bitwig project is read by its arranger, as a real one with clips there is: its bars are bars Bitwig
    // can go round and record into.
    let bitwig = mo_formats::paths::has_extension(save, &["bwproject"]);
    mo_midi::playing::of(&Arrangement {
        view: if bitwig { mo_formats::bitwig::arrangement::VIEW } else { "Made up" },
        tempo: 92.0,
        beats_per_bar: 4,
        lanes,
        markers: Vec::new(),
        notes: Vec::new(),
    })
}

/// A "Fits my song" search has ended: the list is its parts, the best fit first; or its place says why there are
/// none.
fn fit_listed(app: &AppWindow, id: u64, answer: Answer) {
    if answer == Answer::Stopped {
        return;
    }
    let none = midi_fits::empty_words(&answer);
    let said = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.asked != id || !p.fitting() {
            return None;
        }
        let (fits, line, wants, with, picker) = match answer {
            Answer::Fits { wants, to, looked, with, picker } => {
                let line = midi_fits::page_line(&to, &looked);
                (looked.fits, line, Some(wants), Some(with), Some(picker))
            }
            // Nothing could be fitted to what's picked: the picker stays, to pick something else.
            Answer::CantFit { picker, .. } => (Vec::new(), String::new(), p.fit_wants, None, Some(picker)),
            Answer::Nothing(_) | Answer::Stopped => (Vec::new(), String::new(), p.fit_wants, None, None),
        };
        // The picker as this search left it: the newest ask's, so it has every press so far. A search that never
        // got to the song's parts (it couldn't start, or the song or the index couldn't be read this time) has
        // none: the page keeps the one it shows, so what's picked by hand stays in sight, to be changed.
        if let Some(picker) = picker {
            p.fit_picker = Some(Arc::unwrap_or_clone(picker));
        }
        // A part whose octave you chose goes there, in this list as in any other.
        let mut fits = fits;
        midi_fits::with_yours(&mut fits, &p.cat, &p.your_octaves);
        let hits: Vec<Hit> = fits.iter().map(|fit| Hit { entry: fit.entry, likeness: None }).collect();
        p.picked_row = p.picked.and_then(|at| page::row_of(&p.cat, &hits, at));
        background::free_later(std::mem::replace(&mut p.shown, Arc::new(hits)));
        p.fits = Some(Arc::new(fits));
        p.fits_song = p.fit_song.clone();
        p.fit_with = with;
        p.fit_picked();
        Some((p.shown.len(), line, wants))
    });
    let Some((shown, line, wants)) = said else { return };
    LINES.with(|l| l.notify.reset());
    let data = app.global::<MidiData>();
    data.set_shown(shown as i32);
    data.set_result(line.into());
    data.set_target("".into());
    show_fit_none(&data, none);
    // What was looked for, which the song decided unless you chose.
    data.set_fit_wants(wants_place(wants));
    show_fit_pick(app);
    show_order(&data);
    fit_follows(app);
}

/// The switch for what's looked for was pressed ([`Page::fit_wants_press`]): the list is worked out again for it.
fn fit_wants_picked(app: &AppWindow, wants: Wants) {
    if PAGE.with(|p| p.borrow_mut().fit_wants_press(wants)) {
        remember_fit_picks(true);
    }
    show_fit(app);
    show_fit_pick(app);
    relist(app);
}

/// A press where the song's parts and bars are picked ([`Page::fit_pick_press`]): the page's picker shows it at
/// once, it's remembered for the song (`at_once`, or once the slider rests: [`remember_fit_picks`]), and the list
/// is worked out again (the newest ask wins, so a handle dragged along asks for its last place). A press that
/// changes nothing asks for nothing.
fn fit_pick_pressed(app: &AppWindow, at_once: bool, press: impl FnOnce(&mut midi_fits::Picker)) {
    let changed = PAGE.with(|p| p.borrow_mut().fit_pick_press(press));
    let Some(write) = changed else { return };
    if write {
        remember_fit_picks(at_once);
    }
    show_fit_pick(app);
    relist(app);
}

/// Writes what's picked for each song to the settings ([`midi_fits::PICKS_KEY`]): `at_once` for a press, or once
/// the bars' slider has rested for [`FIT_PICKS_REST`] (a drag changes the pick at every step, and each write is of
/// all the settings). A write at once is of the newest picks, so a write still waiting has nothing left to do. One
/// still waiting when the app closes goes then ([`write_picks_waiting`]).
fn remember_fit_picks(at_once: bool) {
    let write = || {
        let picks = PAGE.with(|p| p.borrow().fit_picks.text());
        crate::settings::remember(midi_fits::PICKS_KEY, &picks);
    };
    if at_once {
        FIT_PICKS_WRITE.with(slint::Timer::stop);
        write();
    } else {
        FIT_PICKS_WRITE.with(|timer| timer.start(slint::TimerMode::SingleShot, FIT_PICKS_REST, write));
    }
}

/// Where the song's parts and bars are picked, as the page's picker has them now: the parts as the shared filter's
/// chips (and all in its menu, for a narrow window), the bars on the slider under the picture of where the parts
/// play. Nothing until a search has read the song, or for a song with no part to fit to.
fn show_fit_pick(app: &AppWindow) {
    let data = app.global::<MidiData>();
    let shown = PAGE.with(|p| {
        let p = p.borrow();
        let picker = p.fit_picker.as_ref().filter(|picker| !picker.parts.is_empty())?;
        Some((
            picker.parts_shown(),
            picker.parts_menu(),
            picker.bars.clone(),
            picker.length,
            picker.plays_shown(),
            picker.bars_text(),
            (picker.your_parts, picker.is_yours()),
        ))
    });
    let Some((chips, menu, bars, length, plays, text, (your_parts, yours))) = shown else {
        data.set_fit_pick_shown(false);
        return;
    };
    let line = |at: i64| i32::try_from(at).unwrap_or(i32::MAX);
    FIT_PARTS.with(|[as_chips, as_menu]| {
        data.set_fit_parts(as_chips.view(&chips));
        data.set_fit_parts_menu(as_menu.view(&menu));
    });
    data.set_fit_parts_yours(your_parts);
    FIT_PLAYS.with(|model| {
        if !model.iter().eq(plays.iter().copied()) {
            model.set_vec(plays);
        }
        data.set_fit_bars_play(ModelRc::from(model.clone()));
    });
    // The slider only with bars fitted to: with none, its handles would stand for bars that aren't.
    data.set_fit_bars_shown(bars.is_some());
    if let Some(bars) = bars {
        data.set_fit_bars_length(line(length));
        data.set_fit_bars_from(line(bars.start));
        data.set_fit_bars_to(line(bars.end));
    }
    data.set_fit_bars_text(text.into());
    data.set_fit_pick_yours(yours);
    data.set_fit_pick_shown(true);
}

/// "Play with the song" turned on or off: remembered, and the picked part's card and the loop playing follow at
/// once ([`fit_follows`]).
fn play_with_song(app: &AppWindow, on: bool) {
    PAGE.with(|p| p.borrow_mut().with_song = on);
    crate::settings::remember(midi_fits::WITH_SONG_KEY, if on { "1" } else { "0" });
    app.global::<MidiData>().set_fit_with_song(on);
    if !on {
        forget_song_loop();
    }
    fit_follows(app);
}

/// Whether two parts' "what it plays with" are the same parts and bars of the same song: mostly the very same one,
/// which is told without looking at a note.
fn same_bars(a: &Option<Arc<SongBars>>, b: &Option<Arc<SongBars>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
        _ => false,
    }
}

/// The picked part's sound line and its Play button, as it plays now: with the song's parts ("Play with the song",
/// and what they are) or alone (its own tempo, once read). Everything it says is on the page already.
fn show_sound(data: &MidiData, p: &mut Page) {
    let Some(at) = p.picked else { return };
    p.card_with = p.heard_with(at);
    if let Some(line) = p.sound_line_now(at) {
        data.set_sound_line(line.into());
    }
    data.set_play_with_song(p.card_with.is_some());
    // Play in Bitwig's tip only promises the song's bars where Bitwig has some to go round.
    data.set_bitwig_with_song(p.bitwig_song(at).is_some_and(|song| song.bars.is_ok()));
}

/// A new list is on show: what's seen and heard of the picked part is brought into line with how it's moved now
/// ([`Page::fit_picked`]) and what of the song it plays with ([`Page::heard_with`]). Its card's lines and headings
/// ([`show_sound`] for what it plays with: nothing is read for that); its roll and chords, read again when the move
/// is another (nothing else on the card changes, so nothing is blanked meanwhile); and the loop playing or the part
/// in a DAW, started again the new way. Each is compared with what it was made with, so a list that leaves both as
/// they were (a filter, a word typed, the song saved with the same chords) touches nothing that plays.
fn fit_follows(app: &AppWindow) {
    let data = app.global::<MidiData>();
    let Some((at, ask, again)) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let at = p.picked?;
        let now = p.fit_move(at);
        let with = p.heard_with(at);
        let ask = if now == p.card_move { None } else { p.notes_ask(at) };
        // What it plays with is said on the card at once: both lines are the page's already, nothing is read.
        if !same_bars(&with, &p.card_with) {
            show_sound(&data, &mut p);
        }
        let looping = p.looping == Some(at) && (now != p.loop_move || !same_bars(&with, &p.loop_with));
        let in_daw = p.in_daw.is_some() && p.in_daw == p.cat.entry(at).map(|e| e.phrase.id) && now != p.daw_move;
        show_card_fit(&data, &mut p);
        Some((at, ask, (looping, in_daw)))
    }) else {
        return;
    };
    if let Some(ask) = ask {
        ask_notes(app, ask);
    }
    let (looping, in_daw) = again;
    if looping && data.get_playing() {
        play(app, at);
    }
    let daw_now = if data.get_daw_starting() != "" { data.get_daw_starting() } else { data.get_daw_playing() };
    if let Some(daw) = Daw::from_id(&daw_now).filter(|_| in_daw) {
        stop_daw(app);
        start_daw(app, daw, at);
    }
}

/// What the list's place says while "Fits my song" has no lines for it (none: nothing of its own).
fn show_fit_none(data: &MidiData, none: Option<midi_fits::Empty>) {
    let none = none.unwrap_or(midi_fits::Empty { title: "", text: String::new(), filters: false });
    data.set_fit_none_title(none.title.into());
    data.set_fit_none_text(none.text.into());
    data.set_fit_none_filters(none.filters);
}

/// Where the switch for what's looked for stands: the place of `wants` in [`WANTS`], or -1 while it isn't known.
fn wants_place(wants: Option<Wants>) -> i32 {
    wants.and_then(|w| WANTS.iter().position(|x| *x == w)).map_or(-1, |at| at as i32)
}

/// The "Fits my song" chip and its switch, as the page has them now.
fn show_fit(app: &AppWindow) {
    let (song, wants) = PAGE.with(|p| {
        let p = p.borrow();
        (p.fit_song.as_ref().map(|s| s.label.clone()), p.fit_wants)
    });
    let data = app.global::<MidiData>();
    // What's looked for is the song's to decide until you choose: the switch shows it once the song is read.
    if wants.is_some() || song.is_none() {
        data.set_fit_wants(wants_place(wants));
    }
    data.set_fit_song(song.unwrap_or_default().into());
}

/// The chip's menu picked a song by its label, or Off (""): the list is the parts that fit it, or the plain list
/// again. The song that's on picked again is searched for again (and read again, if it was saved since).
fn pick_fit_song(app: &AppWindow, label: &str) {
    if label.is_empty() {
        fit_off(app);
        relist(app);
        return;
    }
    let picked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let song = p.fit_songs.iter().find(|s| s.label == label).cloned()?;
        Some(p.fit_song_is(song))
    });
    let Some(another) = picked else { return };
    if another {
        app.global::<MidiData>().set_fit_wants(-1);
        show_fit_pick(app);
    }
    show_fit(app);
    relist(app);
}

/// "Fits my song" goes off (the caller works the list out again): what its searches kept is let go, as the index's
/// slots alone are tens of megabytes.
fn fit_off(app: &AppWindow) {
    let was_on = PAGE.with(|p| p.borrow_mut().fit_is_off());
    show_fit(app);
    show_fit_pick(app);
    if was_on {
        fit_job(app, FitJob::LetGo);
    }
}

/// The songs "Fits my song" offers (the few saved last, [`midi_fits::song_choices`]), as the Songs page's list has
/// them: it tells this page each time it makes its list, at the start and whenever a save in your project folders
/// changes (`crate::songs`). So the menu and the Songs page never disagree about which saves are one song, nothing
/// is read here, and a list on show follows its song: saved again in the DAW (the same file or a new one), it's
/// found here by its saves and its list is worked out again from the save written last.
pub fn songs_offered(app: &AppWindow, songs: Vec<SongChoice>) {
    let (menu, again) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // The song the list is fitted to, as it is now. Not offered any more (eight songs saved since): it stays as
        // it was.
        let now = p.fit_song.as_ref().and_then(|was| was.among(&songs)).cloned();
        let again = now.is_some() && now != p.fit_song;
        if again {
            // Joined to another song on the Songs page since (the library has another key for it): it goes by that
            // song's pick, as the two are one song now.
            let joined = now.as_ref().zip(p.fit_song.as_ref()).is_some_and(|(now, was)| now.key != was.key);
            if let Some(now) = now.as_ref().filter(|_| joined) {
                p.fit_yours = p.fit_picks.of(&now.key);
                // The picker on show still has the old pick: gone until the search for the new one answers, so
                // a press can't put the old one in the new one's place.
                p.fit_picker = None;
            }
            p.fit_song = now;
        }
        // The menu's lines only when they're others: a menu that's open keeps its place.
        let labels = |songs: &[SongChoice]| songs.iter().map(|s| s.label.clone()).collect::<Vec<_>>();
        let menu = (labels(&songs) != labels(&p.fit_songs)).then(|| labels(&songs));
        p.fit_songs = songs;
        (menu, again)
    });
    if let Some(menu) = menu {
        app.global::<MidiData>().set_fit_songs(strings(menu));
    }
    if again {
        // Its label may be another too (a second song of its name).
        show_fit(app);
        show_fit_pick(app);
        relist(app);
    }
}

/// Shows one part, by its notes' fingerprint (Ctrl+K): picked, and in sight; when the filters hide it, they go (and
/// "Fits my song", whose list it may not be in). Before the index is read it's shown once it is.
fn show_part(app: &AppWindow, hash: &str) {
    let Some(at) = PAGE.with(|p| p.borrow().cat.find(hash)) else {
        PAGE.with(|p| p.borrow_mut().wanted = Some(hash.to_owned()));
        return;
    };
    let row = PAGE.with(|p| {
        let p = p.borrow();
        page::row_of(&p.cat, &p.shown, at)
    });
    if let Some(row) = row {
        select_row(app, row);
        let data = app.global::<MidiData>();
        data.set_show_row(row as i32);
        data.set_show_ask(data.get_show_ask() + 1);
        return;
    }
    // Hidden by the filters (or the list isn't worked out yet): every part shows, and the list finds it.
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.filters = Filters::default();
        p.target = None;
        p.seed = None;
        p.scroll_to_pick = true;
    });
    fit_off(app);
    app.global::<MidiData>().set_search("".into());
    show_chips(app);
    show_years(app);
    pick(app, Some((at, None)));
    relist(app);
}

fn select_row(app: &AppWindow, row: usize) {
    let at = PAGE.with(|p| p.borrow().shown.get(row).map(|h| h.entry));
    if let Some(at) = at {
        pick(app, Some((at, Some(row))));
    }
}

fn step(app: &AppWindow, by: i32) {
    let next = PAGE.with(|p| {
        let p = p.borrow();
        if p.shown.is_empty() {
            return None;
        }
        let next = match p.picked_row {
            None => 0,
            Some(r) => (r as i64 + i64::from(by)).clamp(0, p.shown.len() as i64 - 1) as usize,
        };
        Some(next)
    });
    if let Some(row) = next {
        select_row(app, row);
        let data = app.global::<MidiData>();
        data.set_show_row(row as i32);
        data.set_show_ask(data.get_show_ask() + 1);
    }
}

/// Picks a part (or none): its card, then its notes for the roll and its key.
/// Picks part `picked` (none closes the card). `row` is its line when that's known; when it isn't (the index was just
/// read again) the list finds it once it has been worked out.
fn pick(app: &AppWindow, picked: Option<(usize, Option<usize>)>) {
    let row = picked.and_then(|(_, row)| row);
    let before = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let at = picked.map(|(at, _)| at);
        // Another part: moved as its line in the list on show says. The same one picked again (its line clicked, the
        // index read again) stays moved as it was: each new list keeps that right ([`fit_follows`]).
        if p.picked != at {
            p.picked = at;
            p.picked_fit = None;
            p.fit_picked();
        }
        match (picked, row) {
            (Some(_), None) => None,
            _ => std::mem::replace(&mut p.picked_row, row),
        }
    });
    // Only the lines that changed are drawn again.
    for r in [before, row].into_iter().flatten() {
        LINES.with(|l| l.notify.row_changed(r));
    }
    let data = app.global::<MidiData>();
    let Some((at, _)) = picked else {
        data.set_panel_shown(false);
        // The loop's Stop is on the card: closing it stops the loop too, and a part playing in a DAW.
        if data.get_playing() {
            stop_playing(app);
        }
        if data.get_daw_playing() != "" || data.get_daw_starting() != "" {
            stop_daw(app);
        }
        return;
    };
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // Counted even when there's nothing to ask for, so an older ask's answer isn't taken.
        let ask = p.notes_ask(at);
        let e = p.cat.entry(at)?;
        let mut card = card(e);
        let fit = p.card_fit();
        card.fit = fit.text.as_str().into();
        card.fit_moved = fit.moved;
        let facts: Vec<MidiFact> = page::facts(e, &p.cat.dates)
            .into_iter()
            .map(|(n, v)| MidiFact { name: n.into(), value: v.into() })
            .collect();
        data.set_picked(card);
        data.set_facts(ModelRc::new(VecModel::from(facts)));
        // Its own tempo follows with the notes (it's read from its song or file); with the song, it's the song's,
        // known already.
        p.card_alone = None;
        show_sound(&data, &mut p);
        crate::piano_roll::show(app, None);
        p.card_fit = fit;
        show_actions(&data, &p);
        ask
    });
    data.set_panel_shown(true);
    show_soundfont(app);
    // While a loop plays, picking another part plays that one instead (newest wins); the same part again (a click on
    // its line, or the list read again) goes on as it is.
    if data.get_playing() && PAGE.with(|p| p.borrow().looping) != Some(at) {
        play(app, at);
    }
    // A part playing in a DAW follows the pick the same way (the review's D-3). Clicking down the list asks for each
    // in turn, and each start no longer wanted as its turn comes stops before it connects.
    let daw_now = if data.get_daw_starting() != "" { data.get_daw_starting() } else { data.get_daw_playing() };
    let phrase = PAGE.with(|p| p.borrow().cat.entry(at).map(|e| e.phrase.id));
    if let Some(daw) = Daw::from_id(&daw_now)
        && phrase.is_some()
        && PAGE.with(|p| p.borrow().in_daw) != phrase
    {
        stop_daw(app);
        start_daw(app, daw, at);
    }
    if let Some(ask) = ask {
        ask_notes(app, ask);
    }
}

/// Reads the picked part's notes for its card, off the window's thread: its roll, its chords, and its key when the
/// index has no reading of it. Newest wins.
fn ask_notes(app: &AppWindow, ask: NotesAsk) {
    if HERE.with(|h| *h.borrow()) {
        let id = ask.id;
        let read = read_notes(&ask, &mut None);
        notes_read(app, id, read);
        tempo_read(app, id, sound_line_of(&ask));
        return;
    }
    NOTES.with(|n| {
        let mut n = n.borrow_mut();
        if n.is_none() {
            let weak = app.as_weak();
            // The connection stays open from one part to the next.
            let mut open: Option<(PathBuf, MidiStore)> = None;
            match Newest::start("MIDI notes", move |ask: NotesAsk| {
                let id = ask.id;
                let read = read_notes(&ask, &mut open);
                let weak_notes = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak_notes.upgrade() {
                        notes_read(&app, id, read);
                    }
                });
                // Then its tempo, so the roll never waits for it, and the card has it even when the notes failed.
                let line = sound_line_of(&ask);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        tempo_read(&app, id, line);
                    }
                });
            }) {
                Ok(w) => *n = Some(w),
                Err(e) => log::error("MIDI", &format!("The notes reader couldn't start: {e}")),
            }
        }
        if let Some(n) = n.as_ref() {
            n.ask(ask);
        }
    });
}

/// A part's notes, from wherever the index is; `open` keeps the index open for the next ask ([`from_index`]).
fn notes_of(
    from: &NotesFrom,
    phrase_id: i64,
    hash: &str,
    open: &mut Option<(PathBuf, MidiStore)>,
) -> Result<Vec<NoteEvent>, String> {
    match from {
        NotesFrom::Made(made) => Ok(made.get(hash).cloned().unwrap_or_default()),
        NotesFrom::Store(path) => from_index(path, open, |store| store.notes(phrase_id, hash)),
    }
}

/// Reads from the index at `path`; `open` keeps it open for the next ask. Not one of an older layout, which can't
/// be copied into this app's while it's open (reading your MIDI copies it), nor one a read failed on: the next ask
/// opens it afresh.
fn from_index<T>(
    path: &Path,
    open: &mut Option<(PathBuf, MidiStore)>,
    read: impl FnOnce(&MidiStore) -> Result<T, mo_midi::MidiError>,
) -> Result<T, String> {
    if open.as_ref().is_none_or(|(p, _)| p != path) {
        *open = None;
        *open = Some((path.to_owned(), MidiStore::open_read_only(path).map_err(|e| e.to_string())?));
    }
    let Some((_, store)) = open.as_ref() else { return Err("The index isn't open.".to_owned()) };
    let got = read(store).map_err(|e| e.to_string());
    if got.is_err() || !store.is_current() {
        *open = None;
    }
    got
}

/// What the picked part's card shows of its notes. Its key is its own, read from the notes as they are in the
/// library; its roll and its chords are of the part as it fits (what plays and drags): the notes moved, and the
/// chords spelled in the key moved with them.
fn read_notes(ask: &NotesAsk, open: &mut Option<(PathBuf, MidiStore)>) -> Result<NotesRead, String> {
    // As they are in the index: the part's own.
    let notes = notes_of(&ask.part.from, ask.part.phrase_id, &ask.part.hash, open)?;
    let fact = if ask.key.is_none() {
        mo_midi::keys::with_reader(|r| mo_midi::keys::read(r, &notes, ask.role, ask.cut)).1
    } else {
        None
    };
    let read = notes.len();
    let (moved, semitones) = (midi_fits::moved(notes, ask.part.semitones), ask.part.semitones);
    let key = page::chart_key(ask.key.as_ref().or(fact.as_ref())).map(|k| midi_fits::moved_key(k, semitones));
    // The chords (their first 16 bars: quick even for a long part).
    let chords = mo_midi::chart::read(&moved, ask.role, ask.cut);
    let chart = page::chord_chart(chords.as_ref(), key);
    Ok(NotesRead { read, roll: page::roll(&moved, ask.part.bars), fact, chart })
}

/// The card's sound line for the part alone, with its tempo read now ([`tempo_of`]).
fn sound_line_of(ask: &NotesAsk) -> String {
    page::sound_line(&ask.sound, Some(tempo_of(&ask.part.from, &ask.source, &ask.library)))
}

/// The picked part's tempo has been read: its card says it when the part plays alone ([`show_sound`]), unless
/// another part was picked meanwhile.
fn tempo_read(app: &AppWindow, id: u64, line: String) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.notes_asked == id {
            p.card_alone = Some(line);
            show_sound(&app.global::<MidiData>(), &mut p);
        }
    });
}

/// The last tempo read, by the file's path and the size and time the index read it at: a part's card, its Play loop
/// and its Play in a DAW read it once between them.
static TEMPO: std::sync::Mutex<Option<(String, i64, i64, page::Tempo)>> = std::sync::Mutex::new(None);

/// A part's tempo ([`page::read_tempo`]), off the window's thread. Parts made up for the screenshots have no file.
fn tempo_of(from: &NotesFrom, source: &mo_midi::Source, library: &Path) -> page::Tempo {
    if matches!(from, NotesFrom::Made(_)) {
        return page::usable_tempo(source.tempo).map_or(page::Tempo::NotGiven, page::Tempo::Own);
    }
    let same = |k: &(String, i64, i64, page::Tempo)| k.0 == source.path && k.1 == source.mtime && k.2 == source.size;
    if let Some(k) = TEMPO.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().filter(|k| same(k)) {
        return k.3;
    }
    let tempo = page::read_tempo(source, library);
    // Not one that couldn't be read, nor a save the song library hasn't read yet: tried again next time.
    if !matches!(tempo, page::Tempo::Unread | page::Tempo::NotListed) {
        *TEMPO.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((source.path.clone(), source.mtime, source.size, tempo));
    }
    tempo
}

fn notes_read(app: &AppWindow, id: u64, read: Result<NotesRead, String>) {
    if PAGE.with(|p| p.borrow().notes_asked) != id {
        return;
    }
    let data = app.global::<MidiData>();
    let mut picked = data.get_picked();
    match read {
        Ok(read) => {
            let roll = &read.roll;
            if read.read == 0 {
                picked.roll_note = "Its notes aren't in the index any more.".into();
            } else if roll.notes.is_empty() {
                picked.roll_note = "Every note in it is muted.".into();
            } else {
                picked.roll_low = page::note_name(roll.low).into();
                picked.roll_high = page::note_name(roll.high).into();
                picked.roll_note = if roll.left_off > 0 {
                    format!(
                        "The first {} notes shown, of {}",
                        thousands(roll.notes.len()),
                        thousands(roll.notes.len() + roll.left_off)
                    )
                    .into()
                } else {
                    "".into()
                };
            }
            picked.chords = read.chart.bars.into();
            picked.chords_why = read.chart.why.into();
            // Drawn off the window's thread, as up to 4,000 notes would be a lot for it to lay out.
            crate::piano_roll::show(app, Some(read.roll));
            let drums = picked.kind == page::role_label(Role::Drums);
            if let Some(fact) = read.fact {
                let words = page::key_card(Some(&fact), drums);
                picked.key = words.answer.into();
                picked.key_sure = words.sure.into();
                picked.key_why = words.why.into();
                picked.key_from = "Read just now from its notes by the new key finder.".into();
            } else if picked.key == READING {
                // No pitched notes to read: the card says so, rather than reading for ever.
                let words = page::key_card(None, drums);
                picked.key = words.answer.into();
                picked.key_why = words.why.into();
            }
        }
        Err(e) => {
            log::warning("MIDI", &format!("A part's notes couldn't be read: {e}"));
            picked.roll_note = format!("Its notes couldn't be read: {e}").into();
            if picked.key == READING {
                picked.key = "Not read".into();
                picked.key_why = "Its notes couldn't be read, so neither could its key.".into();
            }
        }
    }
    data.set_picked(picked);
}

/// The card for a part, before its notes are read (the roll and, without a stored reading, the key follow).
fn card(e: &mo_midi::Entry) -> PickedPart {
    let p = &e.phrase;
    let words = page::key_card(p.key_fact.as_ref(), p.drums || e.role() == Role::Drums);
    let (key, sure, why, from) = if p.key_fact.is_some() || p.drums || e.role() == Role::Drums {
        (words.answer, words.sure, words.why, if p.key_fact.is_some() { "From the new key finder." } else { "" })
    } else {
        (READING.to_owned(), String::new(), String::new(), "")
    };
    let (others, more) = page::other_files(e, 6);
    PickedPart {
        kind: page::role_label(e.role()).into(),
        kind_why: why_text(p).into(),
        colour: colour(e.role()),
        title: page::title(e).into(),
        from: page::from_text(e).into(),
        path: mo_app::shell::wrappable_path(&p.source.path).into(),
        key: key.into(),
        key_sure: sure.into(),
        key_why: why.into(),
        key_from: from.into(),
        instrument: page::instrument_text(e).into(),
        phrase: page::phrase_text(e).into(),
        roll_low: "".into(),
        roll_high: "".into(),
        roll_note: "".into(),
        chords: "".into(),
        chords_why: "".into(),
        others: strings(others),
        others_more: if more > 0 { format!("and {} more", thousands(more)).into() } else { "".into() },
        // The page's to say ([`pick`]): it knows the list the part was picked in.
        fit: "".into(),
        fit_moved: false,
    }
}

/// The card's buttons, its octave buttons (`octave`, [`CardFit::octave`]: a fitted part's, by the line that says how
/// it's moved) and the line's right-click menu, which has both: one list (rule 15: each shows its key). `put`
/// ([`CardFit::put`]): Put in the song, beside Send to Bitwig, for a part fitted to bars Bitwig's arranger has; while
/// one is being put in, the same button stops the recording.
fn actions(
    e: &mo_midi::Entry,
    shortcuts: &Shortcuts,
    octave: Option<OctaveButtons>,
    put: Option<PutButton>,
) -> (Vec<CardAction>, Vec<CardAction>, Vec<MenuItem>) {
    // Down then up, as they sit side by side; the app's pick is offered back once the octave is yours.
    let mut octaves: Vec<(&str, &str, bool)> = Vec::new();
    if let Some(can) = octave {
        octaves.push(("midi.octaveDown", "Octave down", can.down));
        octaves.push(("midi.octaveUp", "Octave up", can.up));
        if can.yours {
            octaves.push(("midi.octaveApp", "The app's octave", true));
        }
    }
    let button = |&(id, text, enabled): &(&str, &str, bool)| CardAction {
        text: text.into(),
        tip: what_of(id).into(),
        key: shortcuts.key_of(id).into(),
        id: id.into(),
        enabled,
        main: false,
        danger: false,
    };
    // Into your DAW first, by the drag handle above them; then finding more.
    let mut list: Vec<(&str, &str, bool)> = vec![("midi.toBitwig", "Send to Bitwig", true)];
    if let Some(put) = put {
        let (text, can) = put.reads();
        list.push(("midi.putInSong", text, can));
    }
    list.push(("midi.moreLike", "More like this", true));
    if !e.within.is_empty() {
        list.push(("midi.within", "Show the whole phrase", true));
    }
    // A part from one of your songs: its song on the Songs page (Solomon's note, 11 Nov).
    if e.is_project() {
        list.push(("midi.showInSongs", "Show in Songs", true));
    }
    list.push(("midi.source", "Show in folder", true));
    list.push(("midi.copyPath", "Copy path", true));
    let card = list.iter().map(button).collect();
    // The right-click menu starts with playing it, as Sounds' does: the card has those in its play row.
    let play = [
        ("app.play", "Play or stop the loop", true),
        ("midi.playBitwig", "Play in Bitwig, or stop", true),
        ("midi.playFl", "Play in FL Studio, or stop", true),
    ];
    let menu = play
        .iter()
        .chain(&octaves)
        .chain(&list)
        .map(|&(id, text, enabled)| MenuItem {
            what: text.into(),
            key: shortcuts.key_of(id).into(),
            id: id.into(),
            enabled,
        })
        .collect();
    (card, octaves.iter().map(button).collect(), menu)
}

/// Opens the Songs page on the song a part's project belongs to, by the save's path, as Presets' Show in Songs does.
fn show_in_songs(app: &AppWindow, save: &str) {
    crate::go_to(app, mo_app::places::Place::Songs.index() as i32);
    if !crate::songs::show_save_path(app, save) {
        log::info("MIDI", "The song isn't on the Songs page yet (its list is still being read)");
    }
}

/// Runs one of the page's actions on the picked part; false when there's nothing for it to do.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let (picked, feed) = PAGE.with(|p| {
        let p = p.borrow();
        (p.picked.and_then(|at| p.cat.entry(at).map(|e| (at, e.phrase.source.path.clone()))), p.feed.clone())
    });
    match (id, picked) {
        // F5 and the Read MIDI again button: reads again, or stops a reading under way (X3-21).
        ("midi.readAgain", _) => {
            if app.global::<MidiData>().get_reading() {
                stop_reading();
            } else {
                read_midi(app, true);
            }
            true
        }
        // The list is ranked one way at a time: by a part's likeness, or (with "Fits my song") by how each fits.
        ("midi.moreLike", Some((at, _))) => {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.target = Some(Target::Like(at));
                p.seed = None;
            });
            fit_off(app);
            relist(app);
            true
        }
        ("midi.within", Some((at, _))) => {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.target = Some(Target::Within(at));
                p.seed = None;
            });
            fit_off(app);
            relist(app);
            true
        }
        ("midi.showInSongs", Some((at, path))) => {
            let project = PAGE.with(|p| p.borrow().cat.entry(at).is_some_and(mo_midi::Entry::is_project));
            if project {
                show_in_songs(app, &path);
            }
            project
        }
        ("midi.source", Some((_, path))) => {
            if let Some(feed) = feed {
                crate::files::reveal(&feed, &path, mo_app::shell::folder_of(&path));
            }
            true
        }
        ("midi.toBitwig", Some((at, _))) => {
            send_to_bitwig(at);
            true
        }
        ("midi.putInSong", Some((at, _))) => put_in_song(app, at),
        ("app.play", _) => toggle_play(app),
        ("midi.playBitwig", _) => toggle_daw(app, Daw::Bitwig),
        ("midi.playFl", _) => toggle_daw(app, Daw::Fl),
        ("midi.copyPath", Some((_, path))) => {
            if let Some(feed) = feed {
                crate::files::copy_path(&feed, &path);
            }
            true
        }
        (id, Some(_)) => OctaveChange::from_id(id).is_some_and(|change| change_octave(app, change)),
        _ => false,
    }
}

/// Octave up, Octave down or The app's octave on the picked part ([`Page::change_octave`]): its line and its card
/// say it at once, what's playing plays there ([`fit_follows`]), and the index is told for the next time. Whether
/// the picked part has octave buttons at all: its key is then this page's even with nothing left to do (3 octaves up
/// already), so it never goes on to do something else.
fn change_octave(app: &AppWindow, change: OctaveChange) -> bool {
    let (changed, has_buttons) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let has_buttons = p.card_fit().octave.is_some();
        let changed = p.change_octave(change).map(|(hash, octaves, rows)| {
            // The screenshots' made-up parts have no index to keep it in.
            let index = match &p.notes {
                Some(NotesFrom::Store(index)) => Some(index.clone()),
                _ => None,
            };
            (hash, octaves, rows, index)
        });
        (changed, has_buttons)
    });
    let Some((hash, octaves, rows, index)) = changed else { return has_buttons };
    for row in rows {
        LINES.with(|l| l.notify.row_changed(row));
    }
    if let Some(index) = index {
        remember_octave(index, hash, octaves);
    }
    fit_follows(app);
    true
}

/// The octaves chosen that the index hasn't been told yet, by its file and the notes' fingerprint: the newest for
/// each, written in turn off the window's thread.
static OCTAVES_SAID: background::Latest<(PathBuf, String), Option<i32>> = background::Latest::new();

/// How many times an octave is given to the index, and how long apart, when the index doesn't take it: the indexer
/// or another copy of the app may be writing for longer than the index waits (5 seconds each time).
const OCTAVE_TRIES: u32 = 3;
const OCTAVE_PAUSE: Duration = Duration::from_secs(2);

/// How long the app waits, as it closes, for an octave chosen just before to reach the index.
const OCTAVE_WAIT: Duration = Duration::from_secs(2);

/// Has the index keep the octave you chose for a set of notes (none: the app's pick again). The page has it already
/// ([`midi_fits::YourOctaves`]); this is for the next time the index is read.
fn remember_octave(index: PathBuf, hash: String, octaves: Option<i32>) {
    let said = OCTAVES_SAID.say("MIDI octaves", (index, hash), octaves, |said| {
        if let Err(why) = background::try_again(OCTAVE_TRIES, OCTAVE_PAUSE, || keep_octaves(&said)) {
            let why = format!("An octave you chose couldn't be kept in the MIDI index: {why}");
            log::error("MIDI", &format!("{why} It holds until the app is closed."));
        }
    });
    if let Err(e) = said {
        log::error("MIDI", &format!("Couldn't start keeping the octave you chose: {e}"));
    }
}

/// Writes octaves chosen into their index (`said`: the index's file and the notes' fingerprint, with the octaves;
/// none for the app's pick again): each index in one go, and only one that's there (`MidiStore::keep_your_octaves`
/// makes none). Every index is tried; Err: why the first that failed did.
fn keep_octaves(said: &[((PathBuf, String), Option<i32>)]) -> Result<(), String> {
    let mut indexes: Vec<&Path> = Vec::new();
    for ((index, _), _) in said {
        if !indexes.contains(&index.as_path()) {
            indexes.push(index);
        }
    }
    let mut failed = None;
    for index in indexes {
        let theirs: Vec<(&str, Option<i32>)> = said
            .iter()
            .filter(|((of, _), _)| of == index)
            .map(|((_, hash), octaves)| (hash.as_str(), *octaves))
            .collect();
        if let Err(e) = MidiStore::keep_your_octaves(index, &theirs) {
            failed.get_or_insert(e.to_string());
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Esc's first step, on any page (`app.escape`): the loop playing stops, and a part playing in a DAW, as both play on
/// after the page goes. Whether either was playing.
pub fn stop_on_escape(app: &AppWindow) -> bool {
    let data = app.global::<MidiData>();
    let looping = data.get_playing() || PAGE.with(|p| p.borrow().looping.is_some());
    let in_daw = data.get_daw_playing() != "" || data.get_daw_starting() != "";
    if looping {
        stop_playing(app);
    }
    if in_daw {
        stop_daw(app);
    }
    looping || in_daw
}

/// Esc on the page (`app.escape`, in the search box or anywhere else), the same order on every page, one step a press
/// once nothing plays ([`stop_on_escape`]; the whole-app review of 6 Nov, X3-12): the list goes back to the whole list
/// from parts shaped like one (or the phrases one is in); else the search is cleared; else the picked part's card
/// closes. Says whether it did any of them (when not, the key goes on).
pub fn escape(app: &AppWindow) -> bool {
    let data = app.global::<MidiData>();
    if PAGE.with(|p| p.borrow_mut().target.take().is_some()) {
        relist(app);
        return true;
    }
    if !data.get_search().is_empty() {
        data.set_search(SharedString::new());
        data.invoke_search_edited(SharedString::new());
        return true;
    }
    if PAGE.with(|p| p.borrow().picked.is_some()) {
        pick(app, None);
        return true;
    }
    false
}

/// Send to Bitwig: the picked part as a new MIDI file in the export folder's "MIDI browser" folder, then as a clip
/// on a new track in the project open in Bitwig (as the C# MIDI page did). In a "Fits my song" list it goes moved as
/// it fits, under a name that says so, and Bitwig's play start moves to the first of the song's bars it was found
/// for, to mark where the clip goes in the arranger ([`Page::bitwig_place`]). The part's own file is never touched.
/// Its card shows at once and says how it went.
fn send_to_bitwig(at: usize) {
    let Some((part, library, feed, song)) = PAGE.with(|p| {
        let p = p.borrow();
        Some((p.part_out(at)?, p.library.clone(), p.feed.clone()?, p.bitwig_place(at)))
    }) else {
        return;
    };
    let job = to_bitwig::part_card(&feed, &part.name);
    let started = background::spawn("Send to Bitwig", move || {
        // A panic in it (finding the export folder too) is logged as a crash and fails the card as it goes.
        let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
            let export = export_dir(&library);
            let export = export.as_deref().map_err(String::as_str);
            send_part(job, &to_bitwig::Bitwig::default(), &part, export, song.as_ref())
        }));
    });
    // Its card was taken into the thread, so it fails by itself ("It stopped before it finished").
    if let Err(e) = started {
        log::error("MIDI", &format!("Couldn't start sending to Bitwig: {e}"));
    }
}

/// Where parts sent to Bitwig are written: "MIDI browser" in the export folder (the one set, else "Music Organiser
/// Exports" in Documents), or why not ([`mo_app::bitwig_convert::export_folder_in`]).
fn export_dir(library: &Path) -> Result<PathBuf, String> {
    Ok(mo_app::bitwig_convert::export_folder_in(library)?.join(page::EXPORT_SUBFOLDER))
}

/// Reads the part's notes, writes them as a new MIDI file in `export`, and sends that to `to`, ending `job`'s card
/// with how it went. With `song` (the part was found for it, and Bitwig's arranger has its bars), Bitwig's play start
/// then marks the first of those bars, and the card says so. Returns the file written, if it was. Waits for Bitwig,
/// so never on the window's thread.
fn send_part(
    job: mo_app::activity::Job,
    to: &to_bitwig::Bitwig,
    part: &PartOut,
    export: Result<&Path, &str>,
    song: Option<&BitwigSong>,
) -> Option<PathBuf> {
    let (file, _) = match part_file(part, export) {
        Ok(written) => written,
        Err(why) => {
            log::warning("MIDI", &format!("Sending {} to Bitwig failed: {why}", part.name));
            job.failed_as(mo_dawlink::bitwig::PART_FAILED, &why);
            return None;
        }
    };
    let path = file.to_string_lossy();
    match song.and_then(|song| Some((song.bars.as_ref().ok()?, song))) {
        Some((bars, song)) => {
            let says = |mark: &mo_dawlink::bitwig::Mark| midi_fits::mark_words(mark, &song.with, &song.name);
            to_bitwig::send_part_for_song(job, to, &path, &part.name, bars, &says)
        }
        None => to_bitwig::send_part(job, to, &path, &part.name),
    };
    Some(file)
}

/// A part as a new MIDI file in `export` (the export folder's "MIDI browser"), under its name, for Bitwig to read:
/// the file, and how long its clip is in ticks. Err: why not, in words for its card.
fn part_file(part: &PartOut, export: Result<&Path, &str>) -> Result<(PathBuf, i64), String> {
    let dir = export.map_err(str::to_owned)?;
    let notes = match part.notes(&mut None) {
        Ok(notes) if notes.iter().any(|n| !n.muted) => notes,
        Ok(_) => return Err("It has no notes to send (every note in it is muted).".to_owned()),
        Err(e) => return Err(format!("Its notes couldn't be read: {e}.")),
    };
    let clip = page::part_clip(&part.name, &notes, part.bars);
    let ticks = clip.end.unwrap_or(0);
    let bytes =
        mo_formats::midi::write::write_clip(&clip).map_err(|e| format!("Its MIDI file couldn't be made: {e}."))?.bytes;
    let file = page::write_part(dir, &part.name, &bytes)
        .map_err(|e| format!("Its MIDI file couldn't be saved in {}: {e}.", dir.display()))?;
    Ok((file, ticks))
}

/// Set as the app closes or crashes: a part still waiting its turn to be put in its song isn't any more.
static CLOSING: AtomicBool = AtomicBool::new(false);

/// Put in the song: the picked part into Bitwig's arranger at the bars of the song it was found for, on the track
/// selected there. Bitwig's controller API can't lay a clip on the timeline, so Bitwig records it in
/// (`mo_dawlink::bitwig::Bitwig::put_in_song`): the part is written as a new MIDI file in the export folder under its
/// own name (the clip's name in Bitwig; the file goes again once Bitwig has it), and what plays in Bitwig from the
/// app stops first, as Bitwig only records from a stop. One at a time: while one is being put in, the same button
/// stops the recording ([`PutButton`]). False when there's nothing to put in or stop.
fn put_in_song(app: &AppWindow, at: usize) -> bool {
    let data = app.global::<MidiData>();
    // One is being put in: this stops it. Its own threads tell Bitwig ([`put_part`]), so the window never waits.
    let stopping = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let Some(stop) = p.putting.clone() else { return false };
        stop.store(true, Ordering::SeqCst);
        show_card_fit(&data, &mut p);
        true
    });
    if stopping {
        return true;
    }
    let Some((part, library, feed, song)) = PAGE.with(|p| {
        let p = p.borrow();
        let song = p.bitwig_place(at).filter(|song| song.bars.is_ok())?;
        Some((p.part_out(at)?, p.library.clone(), p.feed.clone()?, song))
    }) else {
        return false;
    };
    // For the screenshots: nothing leaves the page.
    if HERE.with(|h| *h.borrow()) {
        return true;
    }
    // A part playing in Bitwig from here, or an audition from Presets, stops: its place in the line is before this.
    // One playing in FL Studio plays on.
    stop_in_bitwig(app, "It stopped: Bitwig records a part into your song from a stop.");
    crate::presets::stop_audition_for_another(app, "Put in the song on the MIDI page has Bitwig record from a stop");
    let job = feed.start(&midi_fits::putting(&part.name, &song.name), &midi_fits::putting_line(&song.with));
    let ticket = mo_dawlink::bitwig::ticket();
    let stop = Arc::new(AtomicBool::new(false));
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.putting = Some(Arc::clone(&stop));
        show_card_fit(&data, &mut p);
    });
    let weak = app.as_weak();
    let done = move || {
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            let data = app.global::<MidiData>();
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.putting = None;
                show_card_fit(&data, &mut p);
            });
        });
    };
    let after = done.clone();
    let started = background::spawn("Put in the song", move || {
        // A panic in it is logged as a crash and fails the card as it goes.
        let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
            let export = export_dir(&library);
            let export = export.as_deref().map_err(String::as_str);
            put_part(job, &to_bitwig::Bitwig::default(), &part, export, &song, ticket, &stop)
        }));
        after();
    });
    if let Err(e) = started {
        // Its card was taken into the thread, so it fails by itself.
        log::error("MIDI", &format!("Couldn't start putting the part in the song: {e}"));
        done();
    }
    true
}

/// About how long Bitwig takes to set up a recording and put its settings back, on top of the part's playing: for
/// the card's bar only.
const PUT_SETS_UP: Duration = Duration::from_secs(4);

/// How often the card's bar moves while Bitwig records, and a stop asked for meanwhile is looked for: as often as a
/// card passes its progress on.
const PUT_TICK: Duration = Duration::from_millis(250);

/// Sets its flag when it goes.
struct Over<'a>(&'a AtomicBool);

impl Drop for Over<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Reads the part's notes, writes them as a new MIDI file in `export`, and has `to` record that into the song's bars
/// on the selected track, ending `job`'s card with how it went. Its turn in Bitwig's line is waited for once the
/// file is written, and the file goes again once Bitwig has the notes (rule 28). `stop`, set meanwhile: before
/// Bitwig records, nothing goes in; while it records, Bitwig is asked to stop
/// (`mo_dawlink::bitwig::Bitwig::stop_putting`), and what played so far stays there. Returns whether all of it went
/// in. Waits for Bitwig, for as long as the part plays, so never on the window's thread.
fn put_part(
    job: mo_app::activity::Job,
    to: &to_bitwig::Bitwig,
    part: &PartOut,
    export: Result<&Path, &str>,
    song: &BitwigSong,
    ticket: mo_dawlink::bitwig::Ticket<'_>,
    stop: &Arc<AtomicBool>,
) -> bool {
    use mo_dawlink::bitwig::Error;
    let failed = |job: mo_app::activity::Job, title: &str, why: String| {
        log::warning("MIDI", &format!("Putting {} in {} failed: {why}", part.name, song.name));
        job.failed_as(title, &why);
        false
    };
    let fitted = match &song.bars {
        Ok(bars) => bars,
        Err(why) => return failed(job, midi_fits::PUT_FAILED, format!("{why}.")),
    };
    let (file, ticks) = match part_file(part, export) {
        Ok(written) => written,
        Err(why) => return failed(job, midi_fits::PUT_FAILED, why),
    };
    let place = midi_fits::put_place(fitted, &song.with, ticks);
    job.progress(0.0, Some(&midi_fits::recording_line(&place)));
    let turn = ticket.wait();
    // Bitwig says nothing until the part is through, so the card's bar goes by the clock (the bars at the song's
    // tempo, and a few seconds for Bitwig to set up and put its settings back), on a thread of its own: this one
    // waits for Bitwig's answer. That thread also passes a stop on to Bitwig, on a connection of its own, and says
    // whether a recording heard it.
    let job = Arc::new(job);
    let over = Arc::new(AtomicBool::new(false));
    let clock = {
        let (job, over, stop, to) = (Arc::clone(&job), Arc::clone(&over), Arc::clone(stop), *to);
        let takes = place.seconds + PUT_SETS_UP.as_secs_f64();
        background::spawn("Put in the song's clock", move || {
            let began = Instant::now();
            // Asked until a recording hears it: one asked for just before Bitwig began, or that didn't get
            // through, is asked again.
            let (mut heard, mut warned) = (false, false);
            while !over.load(Ordering::SeqCst) {
                job.progress((began.elapsed().as_secs_f64() / takes).min(0.95) as f32, None);
                if !heard && stop.load(Ordering::SeqCst) {
                    heard = to.stop_putting().unwrap_or_else(|e| {
                        if !warned {
                            log::warning("MIDI", &format!("Couldn't ask Bitwig to stop the recording: {e}"));
                        }
                        warned = true;
                        false
                    });
                }
                std::thread::sleep(PUT_TICK);
            }
            heard
        })
    };
    let wanted = || !CLOSING.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst);
    let put = {
        // Also when it ends in a panic, so the clock's thread always ends.
        let _over = Over(&over);
        to.put_in_song(&file, &place.bars, &turn, &wanted)
    };
    // The next in Bitwig's line needn't wait for the card.
    drop(turn);
    // Bitwig has the notes, or took nothing: the file was new, under a name this gave it, and goes again.
    let _ = std::fs::remove_file(&file);
    // Whether a recording under way was told to stop: what it ends with is the stop's doing then.
    let heard = clock.ok().and_then(|clock| clock.join().ok()).unwrap_or(false);
    // The clock has ended, so the card is this thread's alone again; were it not, it fails by itself as it goes.
    let Some(job) = Arc::into_inner(job) else { return false };
    match put {
        Ok(put) => {
            let (whole, title, line) = midi_fits::put_words(&put, &place, &part.name, &song.name);
            if whole {
                job.done_as(&title, &line);
            } else {
                log::warning("MIDI", &format!("{title}: {line}"));
                job.failed_as(&title, &line);
            }
            whole
        }
        // Called off before Bitwig recorded anything: by you, or by the app closing.
        Err(Error::Cancelled) if stop.load(Ordering::SeqCst) => {
            job.done_as(midi_fits::PUT_STOPPED, midi_fits::PUT_NOTHING);
            false
        }
        Err(Error::Cancelled) => {
            job.failed_as(midi_fits::PUT_FAILED, "The app was closing.");
            false
        }
        // Stopped part way: the link's words say what Bitwig may have of it. A failure the stop had no part in (it
        // was pressed, but no recording heard it) keeps its own headline.
        Err(e) if heard => failed(job, midi_fits::PUT_STOPPED, e.to_string()),
        Err(e) => failed(job, midi_fits::PUT_FAILED, e.to_string()),
    }
}

/// For the screenshots: everything is worked out in place.
pub fn work_here() {
    HERE.with(|h| *h.borrow_mut() = true);
    crate::piano_roll::draw_here();
}

/// For the screenshots: picks the line at `row` of the list on show.
pub fn pick_row(app: &AppWindow, row: usize) {
    select_row(app, row);
}

/// For the screenshots: "Fits my song" on, fitted to the made-up song the sample's first parts came from; or (`on`
/// false) off again with no songs offered, as the other pictures have the page.
pub fn fit_sample(app: &AppWindow, on: bool) {
    if on {
        songs_offered(app, sample_songs());
        if let Some(first) = PAGE.with(|p| p.borrow().fit_songs.first().map(|s| s.label.clone())) {
            pick_fit_song(app, &first);
        }
    } else {
        pick_fit_song(app, "");
        songs_offered(app, Vec::new());
    }
}

/// For the screenshots, with [`fit_sample`] on: fitted to the made-up Bitwig song instead ("Glass"), whose bars
/// Bitwig's arranger has.
pub fn fit_sample_bitwig(app: &AppWindow) {
    if let Some(song) = PAGE.with(|p| p.borrow().fit_songs.iter().find(|s| s.name == "Glass").map(|s| s.label.clone()))
    {
        pick_fit_song(app, &song);
    }
}

/// For the screenshots' checks, with [`fit_sample`] on: the Songs page makes its list again with the fitted song
/// saved once more, and called another name in the menu (a second song of its name turned up). Whether this page
/// followed it: the chip has the new name, and the list was worked out again.
pub fn fit_sample_follows(app: &AppWindow) -> bool {
    let mut songs = sample_songs();
    let Some(first) = songs.first_mut() else { return false };
    first.label = format!("{} · Projects", first.name);
    first.mtime += 60;
    let label = first.label.clone();
    let before = PAGE.with(|p| p.borrow().asked);
    songs_offered(app, songs);
    let followed = PAGE.with(|p| {
        let p = p.borrow();
        p.asked > before && p.fit_song.as_ref().is_some_and(|s| s.label == label)
    });
    followed && app.global::<MidiData>().get_fit_song() == label.as_str()
}

/// The songs [`sample`]'s parts came from, as "Fits my song" would offer them.
fn sample_songs() -> Vec<SongChoice> {
    let song = |name: &str, saves: &[&str]| SongChoice {
        name: name.to_owned(),
        key: name.to_lowercase(),
        label: name.to_owned(),
        save: saves.first().copied().unwrap_or_default().to_owned(),
        mtime: 1_762_100_000,
        saves: saves.iter().map(|s| (*s).to_owned()).collect(),
    };
    vec![
        song(
            "Night drive",
            &[
                "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
                "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 11.flp",
            ],
        ),
        song("Glass", &["K:\\Bitwig Studio\\Projects\\Glass\\Glass.bwproject"]),
    ]
}

/// Made-up parts for the screenshots, with their notes: a few songs' parts and a pack's.
pub fn sample() -> Loaded {
    use mo_midi::{Key, Source, SourceKind};
    let now = 1_762_100_000;
    let day = 86_400;
    let source = |path: &str, mtime: i64| {
        let file = path.ends_with(".mid");
        Arc::new(Source {
            path: path.to_owned(),
            kind: if file { SourceKind::File } else { SourceKind::Project },
            daw: if file {
                "MIDI".into()
            } else if path.ends_with(".flp") {
                "FL Studio".into()
            } else {
                "Bitwig Studio".into()
            },
            mtime,
            size: 2000,
            tempo: if file { None } else { Some(92.0) },
        })
    };
    let mut phrases = Vec::new();
    let mut notes = HashMap::new();
    // Path, part, clip, kind, key (tonic, minor), bars, days old, keys played.
    type Made<'a> = (&'a str, &'a str, &'a str, Role, u8, bool, i32, i64, &'a [u8]);
    let parts: [Made; 9] = [
        (
            "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
            "Keys",
            "Pattern 3",
            Role::Chords,
            9,
            true,
            8,
            0,
            &[57, 60, 64, 53, 57, 60, 48, 52, 55, 55, 59, 62],
        ),
        (
            "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
            "Sub",
            "Pattern 4",
            Role::Bass,
            9,
            true,
            8,
            0,
            &[33, 33, 29, 29, 24, 24, 31, 31],
        ),
        (
            "I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 12.flp",
            "Lead",
            "Pattern 7 · bars 9–16",
            Role::Melody,
            9,
            true,
            8,
            0,
            &[69, 72, 76, 74, 72, 71, 69, 67, 69],
        ),
        (
            "K:\\Bitwig Studio\\Projects\\Glass\\Glass.bwproject",
            "Arp",
            "Arp clip",
            Role::Arp,
            2,
            false,
            4,
            2,
            &[62, 66, 69, 74, 69, 66],
        ),
        (
            "L:\\New Midi\\Dark Keys\\Dark keys 01 Cm.mid",
            "Piano",
            "Dark keys 01 Cm",
            Role::Chords,
            0,
            true,
            8,
            5,
            &[60, 63, 67, 56, 60, 63, 55, 58, 62],
        ),
        (
            "L:\\New Midi\\Dark Keys\\Dark keys 02 Fm.mid",
            "Piano",
            "Dark keys 02 Fm",
            Role::Chords,
            5,
            true,
            8,
            5,
            &[53, 56, 60, 49, 53, 56],
        ),
        (
            "L:\\New Midi\\Trap Bass\\808 slide 03.mid",
            "Bass",
            "808 slide 03",
            Role::Bass,
            7,
            false,
            4,
            40,
            &[31, 31, 34, 29],
        ),
        (
            "L:\\New Midi\\Drums\\Boom bap 90.mid",
            "Drums",
            "Boom bap 90",
            Role::Drums,
            0,
            false,
            2,
            60,
            &[36, 42, 38, 42, 36, 36, 38, 42],
        ),
        (
            "L:\\New Midi\\Melodies\\Flute hook A.mid",
            "Flute",
            "Flute hook A",
            Role::Melody,
            9,
            true,
            4,
            400,
            // High, as a flute's hook is written: fitted to a song it comes down an octave.
            &[81, 84, 86, 88, 86, 84],
        ),
    ];
    for (i, (path, part, clip, role, tonic, minor, bars, age, keys)) in parts.into_iter().enumerate() {
        let hash = format!("{i:016X}");
        let step = (i64::from(bars) * mo_midi::BAR) / keys.len().max(1) as i64;
        let made: Vec<NoteEvent> = keys
            .iter()
            .enumerate()
            .map(|(j, &key)| NoteEvent {
                start: j as i64 * step,
                length: step * 9 / 10,
                key,
                velocity: 30_000 + (j as u16 % 4) * 8_000,
                muted: false,
                step: false,
                attribute: mo_formats::NoteAttribute::NONE,
            })
            .collect();
        notes.insert(hash.clone(), made);
        phrases.push(StoredPhrase {
            id: i as i64,
            source: source(path, now - age * day),
            part: part.to_owned(),
            clip: clip.to_owned(),
            role,
            role_why: format!("Its notes play like {}", page::role_label(role).to_lowercase()),
            your_role: None,
            bars,
            notes: keys.len() as i32,
            lowest: keys.iter().copied().min().unwrap_or(60),
            highest: keys.iter().copied().max().unwrap_or(60),
            key: (role != Role::Drums).then_some(Key { tonic, minor, confidence: 0.6 }),
            key_fact: None,
            chroma: [0; 12],
            chords: Vec::new(),
            features: (0..48).map(|j| ((j * 37 + i * 53) % 200 + 40) as u8).collect(),
            hash,
            members: String::new(),
            program: None,
            drums: role == Role::Drums,
            track_name: String::new(),
            your_octave: None,
            shape: None,
            cut: None,
        });
    }
    // An older save of the song with the same keys part: one line, "in 2 files".
    if let Some(first) = phrases.first().cloned() {
        phrases.push(StoredPhrase {
            source: source("I:\\Fl Studio Data\\FL Studio\\Projects\\Night drive\\Night drive 11.flp", now - 3 * day),
            ..first
        });
    }
    Loaded::new(Catalogue::new(phrases), NotesFrom::Made(Arc::new(notes)))
}

/// The SoundFont read last, kept for the next part: reading GeneralUser GS takes a moment and 31 MB.
static FONT: std::sync::Mutex<Option<(PathBuf, mo_audio::gm::Font)>> = std::sync::Mutex::new(None);
/// The newest play asked for: a loop made for an older ask (or after Stop) isn't played.
static PLAY_ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The newest play that reached the player.
static PLAY_STARTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// What the player calls the MIDI page's loops: "midi:" and the ask's number.
const PLAY_KEY: &str = "midi:";

/// An ask to make a part into a loop and play it.
struct PlayAsk {
    id: u64,
    font: PathBuf,
    part: PartOut,
    sound: mo_midi::gm::GmSound,
    /// Its file, whose tempo it plays at, and the song library that has a project's.
    source: Arc<mo_midi::Source>,
    library: PathBuf,
    /// The song's parts it plays with, at the song's tempo ([`Page::heard_with`]); none: alone.
    with: Option<Arc<SongBars>>,
}

thread_local! {
    static PLAYS: RefCell<Option<Newest<PlayAsk>>> = const { RefCell::new(None) };
}

thread_local! {
    /// Where the SoundFont is, as last looked for off the window's thread (a look at the disk); none: not looked for
    /// yet. Once found it's kept; while it's missing it's looked for again each time the page is shown or a part is
    /// picked, so one put in its folder is found without a restart.
    static SOUNDFONT: RefCell<Option<Option<PathBuf>>> = const { RefCell::new(None) };
    /// A look for the SoundFont is on its way.
    static LOOKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The SoundFont parts play with now, as last looked for ([`look_for_soundfont`]): the one picked in the app's
/// settings, else GeneralUser GS in the app's folder.
fn soundfont() -> Option<PathBuf> {
    SOUNDFONT.with(|s| s.borrow().clone().flatten())
}

/// Looks for the SoundFont off the window's thread, unless it's been found already; the card follows when it's
/// done.
fn look_for_soundfont(app: &AppWindow) {
    if soundfont().is_some() || LOOKING.with(std::cell::Cell::get) {
        return;
    }
    let Some(folder) = PAGE.with(|p| p.borrow().library.parent().map(Path::to_path_buf)) else { return };
    let chosen = crate::settings::value(page::SOUNDFONT_KEY);
    if HERE.with(|h| *h.borrow()) {
        // The screenshots: at once, so the picture shows the answer.
        SOUNDFONT.with(|s| *s.borrow_mut() = Some(page::soundfont_path(chosen.as_deref(), &folder)));
        return;
    }
    LOOKING.with(|l| l.set(true));
    let weak = app.as_weak();
    let started = background::spawn("Look for the SoundFont", move || {
        let found = page::soundfont_path(chosen.as_deref(), &folder);
        let _ = slint::invoke_from_event_loop(move || {
            LOOKING.with(|l| l.set(false));
            // One chosen meanwhile wins.
            if soundfont().is_some() {
                return;
            }
            let found_one = found.is_some();
            SOUNDFONT.with(|s| *s.borrow_mut() = Some(found));
            if let Some(app) = weak.upgrade() {
                say_soundfont(&app);
                // A Play pressed while it was being looked for: played now, or told what to do.
                let data = app.global::<MidiData>();
                if data.get_play_note() == LOOKING_NOTE {
                    data.set_play_note("".into());
                    if found_one {
                        toggle_play(&app);
                    } else {
                        data.set_play_note(CHOOSE_NOTE.into());
                    }
                }
            }
        });
    });
    if let Err(e) = started {
        LOOKING.with(|l| l.set(false));
        log::warning("MIDI", &format!("Couldn't look for the SoundFont: {e}"));
    }
}

/// Whether the MIDI page's loop is what the player is playing.
fn loop_playing() -> bool {
    crate::sounds::player().playing().is_some_and(|k| k.starts_with(PLAY_KEY))
}

/// Space and the Play button: stops the loop if it's playing, else plays the picked part as one. False when there's
/// nothing to play or stop.
pub fn toggle_play(app: &AppWindow) -> bool {
    let playing = app.global::<MidiData>().get_playing();
    let (picked, looping) = PAGE.with(|p| (p.borrow().picked, p.borrow().looping));
    // Stops the loop of the part picked (or of none); with another part picked (the menu of the line right-clicked),
    // plays that one instead.
    if playing && (picked.is_none() || picked == looping) {
        stop_playing(app);
        return true;
    }
    match picked {
        Some(at) => {
            play(app, at);
            true
        }
        None => false,
    }
}

fn stop_playing(app: &AppWindow) {
    // Counted first: a loop being made checks the count as it starts (under the player's lock), so it either sees
    // this and doesn't start, or started already and is stopped here.
    PLAY_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PAGE.with(|p| p.borrow_mut().looping = None);
    if loop_playing() {
        crate::sounds::player().stop();
    }
    let data = app.global::<MidiData>();
    data.set_playing(false);
    data.set_play_note("".into());
}

/// Another page is about to play (a sound on the Sounds page): the loop playing stops, and one still being made
/// doesn't start on top of it, so the Play button doesn't go on saying it plays.
pub fn stop_for_another(app: &AppWindow) {
    if app.global::<MidiData>().get_playing() {
        stop_playing(app);
    }
}

/// Whether there's a SoundFont to play with, and if not what to do.
fn show_soundfont(app: &AppWindow) {
    look_for_soundfont(app);
    say_soundfont(app);
}

/// What the play note says while a Play waits for the SoundFont to be looked for, and once there's none.
const LOOKING_NOTE: &str = "Looking for the SoundFont…";
const CHOOSE_NOTE: &str = "Choose a SoundFont first.";

/// The card says whether there's a SoundFont, as last looked for (no look at the disk).
fn say_soundfont(app: &AppWindow) {
    let data = app.global::<MidiData>();
    // Not said missing until it's been looked for.
    let missing = SOUNDFONT.with(|s| matches!(*s.borrow(), Some(None)));
    data.set_soundfont_missing(missing);
    if missing {
        let folder = PAGE.with(|p| p.borrow().library.parent().map(Path::to_path_buf)).unwrap_or_default();
        let usual = folder.join(page::SOUNDFONT_FOLDER).join(page::SOUNDFONT_FILE);
        // One line on the card; the file's place is in the button's tip (the UX review of 10 Nov, C6).
        data.set_soundfont_note(
            "To hear parts, choose a General MIDI SoundFont (.sf2), such as the free GeneralUser GS.".into(),
        );
        data.set_soundfont_where(
            format!("Pick a .sf2 file. One saved as {} is found without asking.", usual.display()).into(),
        );
    }
}

/// Makes the part at `at` into a loop with its matched sound and plays it round and round, off the window's thread.
fn play(app: &AppWindow, at: usize) {
    let data = app.global::<MidiData>();
    let Some(font) = soundfont() else {
        stop_playing(app);
        let looked = SOUNDFONT.with(|s| s.borrow().is_some());
        show_soundfont(app);
        data.set_play_note(if looked { CHOOSE_NOTE } else { LOOKING_NOTE }.into());
        return;
    };
    let ask = PAGE.with(|p| {
        let p = p.borrow();
        let e = p.cat.entry(at)?;
        Some(PlayAsk {
            id: PLAY_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1,
            font,
            part: p.part_out(at)?,
            sound: page::gm_sound(e),
            source: Arc::clone(&e.phrase.source),
            library: p.library.clone(),
            with: p.heard_with(at),
        })
    });
    let Some(ask) = ask else {
        stop_playing(app);
        return;
    };
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.looping = Some(at);
        p.loop_move = ask.part.semitones;
        p.loop_with = ask.with.clone();
    });
    // A part playing in a DAW would play over it.
    if app.global::<MidiData>().get_daw_playing() != "" || app.global::<MidiData>().get_daw_starting() != "" {
        stop_daw(app);
    }
    crate::sounds::stop_for_another(app);
    data.set_playing(true);
    data.set_play_note("".into());
    if HERE.with(|h| *h.borrow()) {
        // The screenshots show it playing, with no sound card to play through.
        return;
    }
    PLAYS.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            let mut open: Option<(PathBuf, MidiStore)> = None;
            match Newest::start("MIDI play", move |ask: PlayAsk| {
                let id = ask.id;
                let made = make_loop(&ask, &mut open);
                let problem = match made {
                    Ok(sound) => {
                        // Only if it's still the newest ask as it starts: a Stop or another part meanwhile wins.
                        let _ = crate::sounds::player().play_looped_if(
                            Arc::new(sound),
                            format!("{PLAY_KEY}{id}"),
                            0.0,
                            || {
                                if PLAY_ASKED.load(std::sync::atomic::Ordering::SeqCst) == id {
                                    PLAY_STARTED.store(id, std::sync::atomic::Ordering::SeqCst);
                                    Ok(())
                                } else {
                                    Err(())
                                }
                            },
                        );
                        return;
                    }
                    Err(e) => e,
                };
                log::warning("MIDI", &format!("A part couldn't be played: {problem}"));
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade()
                        && PLAY_ASKED.load(std::sync::atomic::Ordering::SeqCst) == id
                    {
                        let data = app.global::<MidiData>();
                        data.set_playing(false);
                        data.set_play_note(problem.into());
                        PAGE.with(|p| p.borrow_mut().looping = None);
                    }
                });
            }) {
                Ok(n) => *w = Some(n),
                Err(e) => log::error("MIDI", &format!("The player for parts couldn't start: {e}")),
            }
        }
        if let Some(w) = w.as_ref() {
            w.ask(ask);
        }
    });
}

/// How loud the song's parts go in beside a part found: a little under it, so the part is what's heard first. When
/// the song's parts together are louder than the part (several of them, or one low and full), they're turned down
/// to this much of the part's loudness ([`song_gain`]), never under [`SONG_LEAST`] of their own.
const SONG_GAIN: f32 = 0.7;
const SONG_LEAST: f32 = 0.2;

/// How much of itself the song's loop goes in at, beside a part as loud as `part`, the song's loop as loud as `song`
/// (`mo_audio::gm::level`): [`SONG_GAIN`] of its own when it's no louder than the part; louder, as much as leaves
/// it at [`SONG_GAIN`] of the part's loudness, so six parts of a song don't drown the one being tried.
fn song_gain(part: f32, song: f32) -> f32 {
    if part > 0.0 && song > part { (SONG_GAIN * part / song).max(SONG_LEAST) } else { SONG_GAIN }
}

/// The song's bars made into sound last ([`song_loop`]), by the SoundFont and the bars: every part of a list plays
/// with the same ones, so stepping down the list makes each part's own loop and no more. One loop at the most (two
/// minutes of sound), let go when nothing plays with a song any more ([`forget_song_loop`]).
static SONG_LOOP: std::sync::Mutex<SongKept> = std::sync::Mutex::new(SongKept { forgotten: 0, kept: None });

/// A song's loop as it's kept: the SoundFont's path, the bars, and their sound.
type SongSound = (PathBuf, Arc<SongBars>, Arc<mo_audio::AudioData>);

/// What [`SONG_LOOP`] holds: the loop, and how many times it has been let go, so that a loop still being made on
/// the player's thread when that happens isn't kept after all.
#[derive(Default)]
struct SongKept {
    forgotten: u64,
    kept: Option<SongSound>,
}

impl SongKept {
    /// The loop kept, when it's of these bars made with this SoundFont.
    fn of(&self, font_path: &Path, with: &Arc<SongBars>) -> Option<Arc<mo_audio::AudioData>> {
        let (path, bars, sound) = self.kept.as_ref()?;
        (path == font_path && (Arc::ptr_eq(bars, with) || bars == with)).then(|| Arc::clone(sound))
    }

    /// Keeps a loop whose making started when `forgotten` was what it is now; one let go of meanwhile is dropped.
    fn keep(&mut self, forgotten: u64, sound: SongSound) {
        if self.forgotten == forgotten {
            self.kept = Some(sound);
        }
    }

    /// Lets go: gives back the loop to free, and any being made now won't be kept.
    fn forget(&mut self) -> Option<SongSound> {
        self.forgotten = self.forgotten.wrapping_add(1);
        self.kept.take()
    }
}

/// Lets go of the song's loop kept for the next part ([`SONG_LOOP`]): "Play with the song" went off, or the list on
/// show isn't "Fits my song"'s.
fn forget_song_loop() {
    let kept = SONG_LOOP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).forget();
    if let Some(kept) = kept {
        background::free_later(kept);
    }
}

/// The part's loop: its notes read, the SoundFont read (once), and the two made into sound. With the song
/// ([`PlayAsk::with`]): at the song's tempo, over the song's parts in the bars it fits ([`with_song`]). If the song's
/// parts can't be made into sound, the part still plays, alone (the log says why).
fn make_loop(ask: &PlayAsk, open: &mut Option<(PathBuf, MidiStore)>) -> Result<mo_audio::AudioData, String> {
    let notes = ask.part.notes(open)?;
    let (heard, length) = page::loop_notes(&notes, ask.part.bars);
    if heard.is_empty() {
        return Err("Every note in it is muted, so there's nothing to hear.".to_owned());
    }
    let font = {
        let mut kept = FONT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match kept.as_ref() {
            Some((path, font)) if *path == ask.font => font.clone(),
            _ => {
                let font = mo_audio::gm::open(&ask.font)?;
                *kept = Some((ask.font.clone(), font.clone()));
                font
            }
        }
    };
    let tempo = match &ask.with {
        Some(with) => with.bpm(),
        None => tempo_of(&ask.part.from, &ask.source, &ask.library).bpm(),
    };
    let alone = mo_audio::gm::render_loop(&font, &gm_part(&gm_notes(&heard), length, tempo, &ask.sound))?;
    // Nothing of the song to play (the page never asks for that): alone, with nothing to say about it.
    let Some(with) = ask.with.as_ref().filter(|with| with.length > 0 && !with.voices.is_empty()) else {
        return Ok(alone);
    };
    match with_song(&font, &ask.font, with, &alone, length, tempo) {
        Ok(both) => Ok(both),
        Err(e) => {
            log::warning("MIDI", &format!("A part plays without the song's parts, which couldn't be played: {e}"));
            Ok(alone)
        }
    }
}

/// A part's loop (`alone`, `length` ticks long, made at the song's `tempo`) with the song's parts under it: each
/// goes round from its own bar lines until both are back at their starts ([`SongBars::round`], held to what can be
/// made into sound), the song's a little under the part ([`song_gain`]).
fn with_song(
    font: &mo_audio::gm::Font,
    font_path: &Path,
    with: &Arc<SongBars>,
    alone: &mo_audio::AudioData,
    length: i64,
    tempo: f64,
) -> Result<mo_audio::AudioData, String> {
    use mo_audio::gm::{Layer, level};
    let song = song_loop(font, font_path, with)?;
    let layers = [
        Layer { sound: alone, length, gain: 1.0 },
        Layer { sound: &song, length: with.length, gain: song_gain(level(alone), level(&song)) },
    ];
    let round = with.round(length, mo_audio::gm::longest_ticks(mo_midi::PPQ, tempo));
    mo_audio::gm::layered(&layers, round, mo_midi::PPQ, tempo)
}

/// A part's notes as the SoundFont's player takes them.
fn gm_notes(notes: &[NoteEvent]) -> Vec<mo_audio::gm::Note> {
    notes
        .iter()
        .map(|n| mo_audio::gm::Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity7() })
        .collect()
}

/// `notes` as a loop of `length` ticks at `tempo`, played with `sound`.
fn gm_part<'a>(
    notes: &'a [mo_audio::gm::Note],
    length: i64,
    tempo: f64,
    sound: &mo_midi::gm::GmSound,
) -> mo_audio::gm::Part<'a> {
    mo_audio::gm::Part { notes, ppq: mo_midi::PPQ, length, tempo, program: sound.program, drums: sound.drums }
}

/// The song's parts over the bars a list is fitted to, as one loop at the song's tempo: each with the sound matched
/// to it, all made in one pass (`mo_audio::gm::render_together`). Kept for the next part of the list
/// ([`SONG_LOOP`]).
fn song_loop(
    font: &mo_audio::gm::Font,
    font_path: &Path,
    with: &Arc<SongBars>,
) -> Result<Arc<mo_audio::AudioData>, String> {
    let forgotten = {
        let song = SONG_LOOP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(kept) = song.of(font_path, with) {
            return Ok(kept);
        }
        song.forgotten
    };
    let notes: Vec<Vec<mo_audio::gm::Note>> = with.voices.iter().map(|voice| gm_notes(&voice.notes)).collect();
    let parts: Vec<mo_audio::gm::Part> = with
        .voices
        .iter()
        .zip(&notes)
        .map(|(voice, notes)| gm_part(notes, with.length, with.bpm(), &voice.sound))
        .collect();
    let made = Arc::new(mo_audio::gm::render_together(font, &parts)?);
    SONG_LOOP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keep(forgotten, (font_path.to_owned(), Arc::clone(with), Arc::clone(&made)));
    Ok(made)
}

/// A DAW a part can play in, through what's on its selected track or channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Daw {
    Bitwig,
    Fl,
}

impl Daw {
    fn from_id(id: &str) -> Option<Daw> {
        match id {
            "bitwig" => Some(Daw::Bitwig),
            "fl" => Some(Daw::Fl),
            _ => None,
        }
    }

    /// What the page's buttons call it.
    fn id(self) -> &'static str {
        match self {
            Daw::Bitwig => "bitwig",
            Daw::Fl => "fl",
        }
    }
}

/// What's playing in a DAW: stopping it in Bitwig waits for Bitwig, so it's done off the window's thread.
enum InDaw {
    Bitwig(mo_dawlink::bitwig::Playing),
    Fl(mo_dawlink::fl::Playing),
}

/// The part playing in a DAW, if one is. Shared with the threads that start and stop it.
static IN_DAW: std::sync::Mutex<Option<InDaw>> = std::sync::Mutex::new(None);
/// The newest Play or Stop in a DAW: a start that finishes after a newer one stops what it started.
static DAW_ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn in_daw() -> std::sync::MutexGuard<'static, Option<InDaw>> {
    IN_DAW.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What plays in a DAW, taken, and a newer ask counted under the same lock: a start still on its way sees it.
fn take_in_daw() -> Option<InDaw> {
    let mut slot = in_daw();
    DAW_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    slot.take()
}

/// Stops what plays in a DAW, without the window waiting: FL's notes let go by themselves, Bitwig's clip comes out
/// on a thread of its own, in its place in the line taken now (so a Play pressed after it goes after it).
fn stop_in_daw(playing: InDaw, part: Option<i64>, weak: Option<slint::Weak<AppWindow>>) {
    match playing {
        InDaw::Fl(fl) => fl.stop(),
        InDaw::Bitwig(bitwig) => {
            let ticket = mo_dawlink::bitwig::ticket();
            // Counted, so the app waits a moment for it as it closes (the review's D-fp4).
            let on_its_way = crate::in_flight::Start::begin();
            // Held while it waits its turn, so the app closing behind a long job still stops it (the 10 Nov
            // review's D-2).
            let held = HELD.hold(bitwig);
            let started = background::spawn("Stop in Bitwig", move || {
                let _on_its_way = on_its_way;
                // In its turn, behind what was asked for before it, so it never stops Bitwig in the middle of a newer
                // start (the 6 Nov review's D-fp3).
                let turn = ticket.wait();
                // Not there when the app took it as it closed, and stopped it.
                let Some(bitwig) = HELD.release(held) else { return };
                stop_bitwig_in(bitwig, part, &turn, weak);
            });
            if let Err(e) = started {
                log::error("MIDI", &format!("Couldn't start stopping the part in Bitwig: {e}"));
            }
        }
    }
}

/// Stops a part playing in Bitwig, inside `turn`. When it can't, the part is put back, so Stop can be pressed again,
/// unless something else plays from this page by now; and a start in Bitwig still on its way behind it (from this
/// page or Presets) is called off, so one thing plays there at a time (the review's D-2 and D-8). Both are settled
/// before the turn ends, so the start behind sees it. The card says why.
fn stop_bitwig_in(
    bitwig: mo_dawlink::bitwig::Playing,
    part: Option<i64>,
    turn: &mo_dawlink::bitwig::Turn<'_>,
    weak: Option<slint::Weak<AppWindow>>,
) {
    let e = match mo_dawlink::bitwig::Bitwig::default().stop_playing(&bitwig, turn) {
        Ok(out) => {
            if out.loop_back == Some(false) {
                loop_not_back(weak);
            }
            return;
        }
        Err(e) => e,
    };
    log::warning("MIDI", &format!("The part playing in Bitwig couldn't be stopped: {e}"));
    // What's left behind in Bitwig when the app lets go of it: its clip, and, for a part that played with its song's
    // bars, the loop and the play start as the link set them.
    let left = if bitwig.with_song == mo_dawlink::bitwig::WithSong::Looping { midi_fits::BITWIG_LOOP_LEFT } else { "" };
    // Put back once, so Stop can be pressed again once its project is in front; let go when the stop can never work
    // (Bitwig is gone) or it failed again after being put back, so a new Play isn't called off and Esc goes on (the 10
    // Nov review's D-1).
    let again = PUT_BACK.failed_before(&bitwig);
    let put_back = {
        let mut slot = in_daw();
        if slot.is_none() && !e.bitwig_gone() && !again {
            PUT_BACK.note(bitwig.clone());
            *slot = Some(InDaw::Bitwig(bitwig));
            DAW_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some((DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst), crate::presets::call_off_start()))
        } else {
            None
        }
    };
    if put_back.is_none() {
        log::info("MIDI", "The app let go of the part playing in Bitwig, which couldn't be stopped");
    }
    let Some(weak) = weak else { return };
    let words = if put_back.is_none() {
        format!(
            "It couldn't be stopped in Bitwig: {e} The app has let it go: if its clip still plays there, stop it in \
             Bitwig.{left}"
        )
    } else {
        format!(
            "It couldn't be stopped in Bitwig: {e} Bring its project to the front in Bitwig (with changes on), then \
             press Stop again. If it fails again, the app lets it go."
        )
    };
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        let data = app.global::<MidiData>();
        if let Some((at, audition)) = put_back {
            if DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst) == at {
                data.set_daw_starting("".into());
                data.set_daw_playing(Daw::Bitwig.id().into());
                // Picking its part again leaves it as it is.
                PAGE.with(|p| p.borrow_mut().in_daw = part);
            }
            if let Some(audition) = audition {
                crate::presets::called_off(&app, audition);
            }
        }
        data.set_daw_note(words.into());
    });
}

/// A part that played with its song's bars stopped in Bitwig, but Bitwig didn't show its loop and play start back as
/// they were: the log says so, and the card too unless something else plays or starts from this page by now.
fn loop_not_back(weak: Option<slint::Weak<AppWindow>>) {
    log::warning("MIDI", midi_fits::BITWIG_LOOP_NOT_BACK);
    let Some(weak) = weak else { return };
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        let data = app.global::<MidiData>();
        if data.get_daw_playing() == "" && data.get_daw_starting() == "" {
            data.set_daw_note(midi_fits::BITWIG_LOOP_NOT_BACK.into());
        }
    });
}

/// The part put back after a failed stop, if one is: a second failure lets it go.
static PUT_BACK: crate::in_flight::PutBack<mo_dawlink::bitwig::Playing> = crate::in_flight::PutBack::new();

/// Parts a stop took out of the slot and is waiting its turn to stop, for [`finish_up`] to stop as the app closes
/// (the 10 Nov review's D-2).
static HELD: crate::in_flight::Held<mo_dawlink::bitwig::Playing> = crate::in_flight::Held::new();

/// Calls off a start still on its way from this page, for a part from Presets that couldn't be stopped in Bitwig (so
/// one thing plays there at a time). Run inside that stop's turn; gives back the ask to hand to [`called_off`] on the
/// window's thread. None when a part plays from here already (so none was starting).
pub(crate) fn call_off_start() -> Option<u64> {
    let slot = in_daw();
    if slot.is_some() {
        return None;
    }
    DAW_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Some(DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst))
}

/// The buttons and card after [`call_off_start`]: a start called off says why, unless something newer was asked for.
pub(crate) fn called_off(app: &AppWindow, at: u64) {
    let data = app.global::<MidiData>();
    if DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst) != at || data.get_daw_starting() == "" {
        return;
    }
    data.set_daw_starting("".into());
    data.set_daw_note(
        "It didn't start: the audition from the Presets page couldn't be stopped in Bitwig (see there).".into(),
    );
}

/// Play in Bitwig / Play in FL, and their keys: stops the part playing in that DAW, else plays the picked part there
/// (stopping one playing in the other first). False when there's nothing to play or stop.
fn toggle_daw(app: &AppWindow, daw: Daw) -> bool {
    let data = app.global::<MidiData>();
    // What's playing or starting now: pressing its button again stops it (a start still on its way stops what it
    // started as soon as it's in).
    let was = if data.get_daw_starting() != "" { data.get_daw_starting() } else { data.get_daw_playing() };
    stop_daw(app);
    if was == daw.id() {
        return true;
    }
    let Some(at) = PAGE.with(|p| p.borrow().picked) else {
        return !was.is_empty();
    };
    start_daw(app, daw, at)
}

/// Plays the part at `at` in the catalogue's entries in that DAW (after [`stop_daw`]). False when there's nothing to
/// play.
fn start_daw(app: &AppWindow, daw: Daw, at: usize) -> bool {
    let data = app.global::<MidiData>();
    let ask = PAGE.with(|p| {
        let p = p.borrow();
        let e = p.cat.entry(at)?;
        // Moved as it fits the song, in a "Fits my song" list; its name in the DAW says so. In Bitwig it plays with
        // the song's bars going round, as Play with the song plays it here.
        let song = if daw == Daw::Bitwig { p.bitwig_song(at) } else { None };
        Some((p.part_out(at)?, Arc::clone(&e.phrase.source), p.library.clone(), song))
    });
    let Some((part, source, library, song)) = ask else {
        return false;
    };
    let phrase_id = part.phrase_id;
    // The loop with a General MIDI sound would play over it.
    stop_playing(app);
    let asked = DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.in_daw = Some(phrase_id);
        p.daw_move = part.semitones;
    });
    data.set_daw_starting(daw.id().into());
    if HERE.with(|h| *h.borrow()) {
        data.set_daw_starting("".into());
        data.set_daw_playing(daw.id().into());
        return true;
    }
    if daw == Daw::Bitwig {
        // One thing plays in Bitwig at a time: an audition on the Presets page stops (the review's D-8).
        crate::presets::stop_audition_for_another(app, "Play in Bitwig on the MIDI page took over");
    }
    // Its place in Bitwig's line, taken now, behind the stops just asked for (FL needs none).
    let ticket = (daw == Daw::Bitwig).then(mo_dawlink::bitwig::ticket);
    let weak = app.as_weak();
    let on_its_way = crate::in_flight::Start::begin();
    let work = move || {
        let _on_its_way = on_its_way;
        let wanted = move || DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst) == asked;
        let stopped = || Some("It stopped before it started playing.".to_owned());
        // The part's notes and tempo are read before its turn, and not at all once it's no longer wanted, so a part
        // passed over while stepping down the list holds up nothing behind it (the 10 Nov review's D-5).
        let read = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
            if !wanted() {
                return Err(None);
            }
            let notes = part.notes(&mut None).map_err(|e| Some(format!("Its notes couldn't be read: {e}.")))?;
            Ok((notes, tempo_of(&part.from, &source, &library).bpm()))
        }))
        .unwrap_or_else(|_| Err(stopped()));
        // Its place in the line is given up when it won't start.
        let turn = match read {
            Ok(_) => ticket.map(mo_dawlink::bitwig::Ticket::wait),
            Err(_) => {
                drop(ticket);
                None
            }
        };
        let made = read.and_then(|(notes, tempo)| {
            mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
                let clip = DawClip { notes: &notes, bars: part.bars, tempo, name: &part.name };
                start_in_daw(daw, &clip, song.as_ref(), turn.as_ref(), &wanted)
            }))
            .unwrap_or_else(|_| Err(stopped()))
        });
        let note = match made {
            // Something newer was asked for while it waited: it took back what it put in, and says nothing.
            Err(None) => return,
            Ok((playing, note)) => {
                // Checked and kept under the lock a Stop takes, so a Stop either finds it or is seen here.
                let mut slot = in_daw();
                if DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst) == asked {
                    *slot = Some(playing);
                    Ok(note)
                } else {
                    drop(slot);
                    // Stopped (or another part asked for) just as it went in: stopped inside its own turn.
                    match (playing, &turn) {
                        (InDaw::Bitwig(bitwig), Some(turn)) => {
                            stop_bitwig_in(bitwig, Some(phrase_id), turn, Some(weak));
                        }
                        (playing, _) => stop_in_daw(playing, Some(phrase_id), Some(weak)),
                    }
                    return;
                }
            }
            Err(Some(why)) => {
                log::warning(
                    "MIDI",
                    &format!(
                        "{} couldn't play in {}: {why}",
                        part.name,
                        if daw == Daw::Fl { "FL Studio" } else { "Bitwig" }
                    ),
                );
                Err(why)
            }
        };
        drop(turn);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            // A Stop since cleared the buttons itself.
            if DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst) != asked {
                return;
            }
            let data = app.global::<MidiData>();
            data.set_daw_starting("".into());
            match note {
                Ok(note) => {
                    data.set_daw_playing(daw.id().into());
                    data.set_daw_note(note.into());
                    if daw == Daw::Fl {
                        watch_fl(&app);
                    }
                }
                Err(why) => data.set_daw_note(why.into()),
            }
        });
    };
    // FL's notes are timed on a thread the start makes, which takes the priority of the thread that makes it on Linux
    // and Mac: so FL's start runs at normal priority, not the background's lowest, or notes come late (the 6 Nov
    // review's D-11). It's short: a read of the part's notes and the port opening.
    let started = if daw == Daw::Fl {
        std::thread::Builder::new().name("Play in FL".to_owned()).spawn(work)
    } else {
        background::spawn("Play in Bitwig", work)
    };
    if let Err(e) = started {
        data.set_daw_starting("".into());
        log::error("MIDI", &format!("Couldn't start playing in a DAW: {e}"));
    }
    true
}

/// Stops what plays (or is starting) in a DAW, and the buttons say so at once. A start still on its way sees the
/// newer ask and stops what it started.
fn stop_daw(app: &AppWindow) {
    let part = PAGE.with(|p| p.borrow_mut().in_daw.take());
    if let Some(playing) = take_in_daw() {
        stop_in_daw(playing, part, Some(app.as_weak()));
    }
    let data = app.global::<MidiData>();
    data.set_daw_starting("".into());
    data.set_daw_playing("".into());
    data.set_daw_note("".into());
}

/// Stops a part playing (or starting) in Bitwig from the MIDI page, for an audition on the Presets page that takes
/// Bitwig over (the review's D-8): one thing plays there at a time, and the card says why it stopped.
pub fn stop_bitwig_for_another(app: &AppWindow) {
    stop_in_bitwig(app, "It stopped: an audition on the Presets page is playing in Bitwig now.");
}

/// Stops a part playing (or starting) in Bitwig from this page, and has its card say why (`note`). One playing in FL
/// Studio plays on.
fn stop_in_bitwig(app: &AppWindow, note: &str) {
    let data = app.global::<MidiData>();
    if data.get_daw_playing() != Daw::Bitwig.id() && data.get_daw_starting() != Daw::Bitwig.id() {
        return;
    }
    stop_daw(app);
    data.set_daw_note(note.into());
}

thread_local! {
    /// Looks every second whether FL's notes still play: they stop by themselves when the MIDI port goes.
    static FL_WATCH: slint::Timer = slint::Timer::default();
}

fn watch_fl(app: &AppWindow) {
    let weak = app.as_weak();
    FL_WATCH.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            let gone = matches!(&*in_daw(), Some(InDaw::Fl(fl)) if !fl.is_playing());
            let playing_fl = matches!(&*in_daw(), Some(InDaw::Fl(_)));
            if !playing_fl {
                FL_WATCH.with(slint::Timer::stop);
                return;
            }
            if gone && let Some(app) = weak.upgrade() {
                stop_daw(&app);
                app.global::<MidiData>().set_daw_note("It stopped: the Music Organiser MIDI port went away.".into());
                FL_WATCH.with(slint::Timer::stop);
            }
        });
    });
}

/// When the app closes, before the settings are written for the last time (`settings::finish_up`): bars picked on
/// the slider a moment ago, still waiting for it to rest ([`remember_fit_picks`]), go to the settings now. Nothing
/// when none is waiting (so a page that never read its picks can't write over them).
pub fn write_picks_waiting() {
    if FIT_PICKS_WRITE.with(slint::Timer::running) {
        remember_fit_picks(true);
    }
}

/// When the app closes: stops a part playing in a DAW, so Bitwig's clip doesn't loop on and no note hangs in FL.
/// Waits a moment at most (Bitwig answers on this computer).
pub fn finish_up() {
    // A part still waiting its turn to be put in its song isn't put in.
    CLOSING.store(true, Ordering::SeqCst);
    // An octave chosen just before closing still reaches the index: waited for a moment.
    if !OCTAVES_SAID.wait(OCTAVE_WAIT) {
        log::warning("MIDI", "An octave you chose was still on its way to the MIDI index when the app closed.");
    }
    // A start still on its way sees it's no longer wanted and takes back what it put in: waited for a moment (the
    // review's D-fp4; the Presets page's finish_up waits for its own the same way).
    let first = take_in_daw();
    // The Presets page's starts are called off now too, so the app waits once for both pages (its finish_up then finds
    // none on their way).
    crate::presets::call_off_for_closing();
    crate::in_flight::wait_for_all(Duration::from_secs(3));
    let mut in_bitwig = HELD.take_all();
    match first.or_else(take_in_daw) {
        Some(InDaw::Fl(fl)) => fl.finish(),
        Some(InDaw::Bitwig(bitwig)) => in_bitwig.insert(0, bitwig),
        None => {}
    }
    // With those a stop still on its way holds, behind a job longer than the wait (the 10 Nov review's D-2).
    let to = crate::in_flight::closing_link(Duration::from_secs(3));
    for bitwig in in_bitwig {
        // Without waiting its turn: what would go first is closing too, and the extension still makes the stop once
        // the app has gone.
        if let Err(e) = to.stop_playing_as_app_closes(&bitwig) {
            log::warning("MIDI", &format!("The part playing in Bitwig couldn't be stopped as the app closed: {e}"));
            // Bitwig gone or not answering: the rest would only make the app slower to close.
            if e.bitwig_gone() || e == mo_dawlink::bitwig::Error::TimedOut {
                break;
            }
        }
    }
}

/// In the crash hook, when the crash ends the app: FL's notes let go (waited for a moment, so the offs go out before
/// the app does) and Bitwig's clip comes out, a second at most. Never waits for the lock: the thread that crashed may
/// hold it.
pub fn after_crash() {
    CLOSING.store(true, Ordering::SeqCst);
    // A start still on its way takes back what it put in, as when the app closes.
    DAW_ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut slot = match IN_DAW.try_lock() {
        Ok(slot) => slot,
        Err(std::sync::TryLockError::Poisoned(slot)) => slot.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    let mut in_bitwig = HELD.try_take_all();
    match slot.take() {
        Some(InDaw::Fl(fl)) => fl.finish(),
        Some(InDaw::Bitwig(bitwig)) => in_bitwig.insert(0, bitwig),
        None => {}
    }
    // With what a stop still on its way holds (the 10 Nov review's D-2).
    let to = crate::in_flight::closing_link(Duration::from_millis(500));
    for bitwig in in_bitwig {
        if to.stop_playing_as_app_closes(&bitwig).is_err() {
            break;
        }
    }
}

/// FL starts go one at a time ([`start_in_daw`]).
static FL_STARTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Opens the MIDI port to FL Studio. The part playing before lets go of it a moment after it's stopped (on Windows a
/// port takes one program, and one part, at a time), so it's tried again for a second while the start is still
/// wanted.
fn fl_port(wanted: &dyn Fn() -> bool) -> Result<Box<dyn mo_dawlink::fl::NoteOut>, Option<String>> {
    let tried = std::time::Instant::now();
    loop {
        match mo_dawlink::fl::open_port() {
            Ok(port) => return Ok(port),
            Err(_) if tried.elapsed() < Duration::from_secs(1) && wanted() => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) if !wanted() => return Err(None),
            Err(e) => return Err(Some(e.to_string())),
        }
    }
}

/// A part to play in a DAW ([`start_in_daw`]): its notes as they leave the page, the bars the index counts, the tempo
/// it plays at where the DAW takes one, and its name there.
struct DawClip<'a> {
    notes: &'a [NoteEvent],
    bars: i32,
    tempo: f64,
    name: &'a str,
}

/// Starts the part in the DAW, and says where it plays. Waits for Bitwig, so never on the window's thread.
///
/// In Bitwig with `song`, the bars of the song the part was fitted to go round with it
/// (`mo_dawlink::bitwig::Bitwig::play_with_song`); the words say so, or why it plays alone.
///
/// `Err(None)` when it was no longer `wanted` (it put nothing in, or took it back); `Err(Some)` says why it can't play.
fn start_in_daw(
    daw: Daw,
    clip: &DawClip<'_>,
    song: Option<&BitwigSong>,
    turn: Option<&mo_dawlink::bitwig::Turn<'_>>,
    wanted: &dyn Fn() -> bool,
) -> Result<(InDaw, String), Option<String>> {
    let DawClip { notes, bars, tempo, name } = *clip;
    match daw {
        Daw::Fl => {
            // One start at a time: clicking down the list asks for each part, and the port takes one at a time.
            let _one_at_a_time = FL_STARTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if !wanted() {
                return Err(None);
            }
            let part = page::fl_part(notes, bars, tempo);
            let port = fl_port(wanted)?;
            let playing = mo_dawlink::fl::play(port, &part).map_err(|e| Some(e.to_string()))?;
            let bpm = (tempo * 10.0).round() / 10.0;
            Ok((InDaw::Fl(playing), format!("Playing {name} on the channel selected in FL Studio, at {bpm} BPM.")))
        }
        Daw::Bitwig => {
            let Some(turn) = turn else { return Err(Some("It couldn't take its turn in Bitwig.".to_owned())) };
            if !notes.iter().any(|n| !n.muted) {
                return Err(Some("Every note in it is muted, so there's nothing to play.".to_owned()));
            }
            let clip = page::part_clip(name, notes, bars);
            let bytes = mo_formats::midi::write::write_clip(&clip)
                .map_err(|e| Some(format!("Its MIDI file couldn't be made: {e}.")))?
                .bytes;
            let file = std::env::temp_dir().join(page::play_file_name());
            std::fs::write(&file, &bytes)
                .map_err(|e| Some(format!("Its MIDI file couldn't be saved in Temp: {e}.")))?;
            // Bitwig has read the file into its clip once this answers, so it goes at once (rule 28).
            let bars = song.and_then(|song| song.bars.as_ref().ok());
            let played = mo_dawlink::bitwig::Bitwig::default().play_with_song(&file, bars, turn, wanted);
            let _ = std::fs::remove_file(&file);
            let playing = played.map_err(|e| (e != mo_dawlink::bitwig::Error::Cancelled).then(|| e.to_string()))?;
            let note = bitwig_note(name, &playing, song);
            Ok((InDaw::Bitwig(playing), note))
        }
    }
}

/// What the card says once a part plays in Bitwig: through which track, and, when it was to play with `song`, that
/// the song's bars go round with it or why it plays alone ([`midi_fits::bitwig_words`]).
fn bitwig_note(name: &str, playing: &mo_dawlink::bitwig::Playing, song: Option<&BitwigSong>) -> String {
    let with_song = song.map_or_else(String::new, |song| {
        let heard = match &song.bars {
            Ok(_) => Ok(&playing.with_song),
            Err(why) => Err(why.as_str()),
        };
        midi_fits::bitwig_words(heard, &song.with, &song.name)
    });
    format!("Playing {name} through \u{201c}{}\u{201d} in Bitwig{with_song}.", playing.track)
}

/// Opens the system's file picker for a SoundFont, without holding up the window; one that is one is kept.
fn choose_soundfont(app: &AppWindow) {
    let weak = app.as_weak();
    let asked = slint::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("Choose a General MIDI SoundFont")
            .add_filter("SoundFont", &["sf2"])
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        let Some(app) = weak.upgrade() else { return };
        let data = app.global::<MidiData>();
        if !mo_audio::gm::looks_like_soundfont(&path) {
            data.set_play_note(format!("{} isn't a SoundFont (.sf2).", path.display()).into());
            return;
        }
        let kept = crate::settings::remember(page::SOUNDFONT_KEY, &path.to_string_lossy());
        SOUNDFONT.with(|s| *s.borrow_mut() = Some(Some(path.clone())));
        data.set_soundfont_missing(false);
        data.set_play_note(
            if kept {
                ""
            } else {
                "Playing with it now; your settings file couldn't be read, so it's only kept until the app closes."
            }
            .into(),
        );
        log::info("MIDI", &format!("Parts play with the SoundFont {}", path.display()));
        // A loop playing goes on with the new sounds.
        if data.get_playing()
            && let Some(at) = PAGE.with(|p| p.borrow().looping)
        {
            PAGE.with(|p| p.borrow_mut().looping = None);
            play(&app, at);
        }
    });
    if let Err(e) = asked {
        log::error("MIDI", &format!("Couldn't open the file picker: {e}"));
    }
}

// ---- Dragging a part out into a DAW (`crate::drag` runs the drag) ------------------------------------------------

/// Where a part was picked up, so it's dimmed there while it's carried and glows there once it has landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    /// A line of the list, by its row.
    Row(i32),
    /// The picked part's card, by its handle.
    Picked,
}

/// Drags the part at `at` in the catalogue out as a new MIDI file of its notes, written to a drag folder in Temp
/// (cleared later, rule 28). Its notes are read and the file written off the window's thread, as the drag gets going.
fn carry(app: &AppWindow, at: usize, spot: Spot) {
    // Moved as it fits the song, in a "Fits my song" list; the file's name says so.
    let Some((part, colour)) = PAGE.with(|p| {
        let p = p.borrow();
        Some((p.part_out(at)?, colour(p.cat.entry(at)?.role())))
    }) else {
        return;
    };
    let tint = tint(app, colour);
    let under_way = crate::drag::start(
        app,
        crate::drag::Drag {
            make: Box::new(move || {
                let notes = part.notes(&mut None).map_err(|e| {
                    log::warning("MIDI", &format!("A dragged part's notes couldn't be read: {e}"));
                    format!("Its notes couldn't be read: {e}.")
                })?;
                mo_app::drag_out::from_part(&part.name, &notes, part.bars, tint)?
                    .write(&std::env::temp_dir())
                    .map_err(|e| format!("Couldn't write the MIDI file in Temp: {e}."))
            }),
            done: Box::new(move |app, outcome| {
                lift(app, spot, false);
                if outcome == crate::drag::Outcome::Landed {
                    crate::drag::glow(app, move |app, on| glow(app, spot, on));
                }
            }),
        },
    );
    if under_way {
        lift(app, spot, true);
    }
}

/// For the screenshots: what dragging the line at `row` carries (its card), without writing anything. None when
/// there's no part there or its notes can't be read.
pub fn card_at_row(app: &AppWindow, row: usize) -> Option<mo_app::drag_out::Payload> {
    let (part, colour) = PAGE.with(|p| {
        let p = p.borrow();
        let at = p.shown.get(row)?.entry;
        Some((p.part_out(at)?, colour(p.cat.entry(at)?.role())))
    })?;
    let notes = part.notes(&mut None).ok()?;
    let out = mo_app::drag_out::from_part(&part.name, &notes, part.bars, tint(app, colour)).ok()?;
    Some(mo_app::drag_out::Payload { files: Vec::new(), cards: out.cards, caption: out.caption, numbered: false })
}

/// A kind's colour as the drag card takes it: 0xRRGGBB.
fn tint(app: &AppWindow, colour: i32) -> u32 {
    let c = app.global::<crate::Theme>().invoke_sound_type(colour);
    u32::from_be_bytes([0, c.red(), c.green(), c.blue()])
}

/// Dims the part at `spot` while it's carried, or brings it back.
fn lift(app: &AppWindow, spot: Spot, on: bool) {
    let data = app.global::<MidiData>();
    match spot {
        Spot::Row(i) => data.set_lifted_row(if on { i } else { -1 }),
        Spot::Picked => data.set_lifted_picked(on),
    }
}

/// Lights the glow round the part at `spot` once it has landed, or lets it fade.
fn glow(app: &AppWindow, spot: Spot, on: bool) {
    let data = app.global::<MidiData>();
    match spot {
        Spot::Row(i) => data.set_landed_row(if on { i } else { -1 }),
        Spot::Picked => data.set_landed_picked(on),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_holds_every_kind_and_a_copy() {
        let s = sample();
        assert_eq!(s.cat.entries.len(), 9);
        assert_eq!(s.cat.phrase_count, 10);
        assert!(Role::ALL.iter().filter(|r| **r != Role::Other).all(|r| s.cat.role_counts().contains_key(r)));
    }

    /// The made-up part `e`'s notes as they leave the page, moved by `semitones`.
    fn part_out(s: &Loaded, e: &mo_midi::Entry, semitones: i32) -> PartOut {
        PartOut {
            from: s.notes.clone(),
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            semitones,
            name: midi_fits::moved_name(&page::title(e), semitones),
            bars: e.phrase.bars,
        }
    }

    /// The ask for its card, with no key stored for it.
    fn notes_ask(s: &Loaded, e: &mo_midi::Entry, semitones: i32) -> NotesAsk {
        NotesAsk {
            id: 1,
            part: part_out(s, e, semitones),
            role: e.role(),
            cut: false,
            key: None,
            source: Arc::clone(&e.phrase.source),
            library: PathBuf::new(),
            sound: page::gm_sound(e),
        }
    }

    #[test]
    fn notes_are_read_and_keyed_for_the_card() {
        let s = sample();
        let ask = notes_ask(&s, &s.cat.entries[0], 0);
        let Ok(read) = read_notes(&ask, &mut None) else { panic!("the made-up notes should read") };
        // Made up, so its tempo is the sample's own, with no file opened for it.
        let line = sound_line_of(&ask);
        assert!(line.ends_with(" BPM") || line.contains("BPM, as it gives no tempo"), "{line}");
        assert_eq!(read.roll.notes.len(), 12);
        assert!(read.fact.is_some());
        assert!(!read.chart.bars.is_empty(), "its chords are read too");
    }

    /// A part picked in a "Fits my song" list is shown as it fits: its roll and chords moved by what its line says,
    /// while its key stays the one its notes have in the library (the card's line says how it's moved).
    #[test]
    fn a_part_that_fits_is_shown_moved_with_its_own_key() {
        let s = sample();
        let read = |semitones: i32| read_notes(&notes_ask(&s, &s.cat.entries[0], semitones), &mut None).unwrap();
        let (plain, up, down) = (read(0), read(3), read(-2));
        let keys = |r: &NotesRead| r.roll.notes.iter().map(|n| n.key).collect::<Vec<_>>();
        assert_eq!(keys(&up), keys(&plain).iter().map(|k| k + 3).collect::<Vec<_>>());
        assert_eq!(keys(&down), keys(&plain).iter().map(|k| k - 2).collect::<Vec<_>>());
        assert_eq!([(plain.roll.low, plain.roll.high), (up.roll.low, up.roll.high)], [(44, 69), (47, 72)]);
        // Its chords are the moved notes', spelled in the key moved with them (A minor up three is C minor: flats).
        assert_eq!(plain.chart.bars, "Am | – | F | – | C | – | G | –");
        assert_eq!(up.chart.bars, "Cm | – | Ab | – | Eb | – | Bb | –");
        assert_eq!(down.chart.bars, "Gm | – | Eb | – | Bb | – | F | –");
        // The key read is the part's own, whichever way it's moved.
        assert!(plain.fact.is_some());
        assert_eq!((&up.fact, &down.fact), (&plain.fact, &plain.fact));
        assert_eq!((up.read, down.read), (plain.read, plain.read));
    }

    /// The picked part's move belongs to it: its line's in a "Fits my song" list, kept when a newer list of the same
    /// song no longer has it (so what plays doesn't jump when a filter is ticked), and gone with the song or the
    /// list.
    #[test]
    fn the_picked_part_stays_moved_while_the_list_is_its_songs() {
        let songs = sample_songs();
        let fit = |entry: usize, semitones: i32| Fit {
            entry,
            semitones,
            octaves: 0,
            yours: None,
            edges: (48, 72),
            sits: 0.8,
        };
        let mut p = Page { cat: Arc::new(sample().cat), notes: Some(sample().notes), ..Page::default() };
        let listed = |p: &mut Page, fits: Option<Vec<Fit>>, song: Option<&SongChoice>| {
            p.fits = fits.map(Arc::new);
            p.fits_song = song.cloned();
            p.fit_picked();
        };
        // Nothing picked, or a plain list: nothing is moved.
        p.fit_song = Some(songs[0].clone());
        listed(&mut p, Some(vec![fit(4, 3), fit(2, -2)]), Some(&songs[0]));
        assert_eq!((p.picked_fit.clone(), p.fit_move(4), p.fit_move(7)), (None, 3, 0), "a line is moved as it says");
        // Picked in the list: moved as its line says, and its card says so.
        p.picked = Some(4);
        p.fit_picked();
        assert_eq!((p.fit_move(4), p.fit_move(2)), (3, -2));
        assert_eq!(p.picked_fit_text(), midi_fits::card_text(3, false, "Night drive"));
        assert_eq!(p.part_out(4).map(|part| part.semitones), Some(3));
        assert!(p.part_out(4).is_some_and(|part| part.name.ends_with(" (3 semitones up)")));
        // A newer list of the same save that has it another way: as that one says.
        listed(&mut p, Some(vec![fit(4, 1)]), Some(&songs[0]));
        assert_eq!(p.fit_move(4), 1);
        // One that no longer has it (a filter hides it): as it was.
        listed(&mut p, Some(vec![fit(2, -2)]), Some(&songs[0]));
        assert_eq!((p.fit_move(4), p.picked_fit_text()), (1, midi_fits::card_text(1, false, "Night drive")));
        // Through the index read again too, when no list is on show for a while.
        p.fits = None;
        assert_eq!(p.fit_move(4), 1);
        // The song saved again, and the new list doesn't have it: what it was moved for is gone.
        let saved_again = SongChoice { mtime: songs[0].mtime + 60, ..songs[0].clone() };
        listed(&mut p, Some(vec![fit(2, -2)]), Some(&saved_again));
        assert_eq!((p.fit_move(4), p.picked_fit_text()), (0, String::new()));
        assert!(p.part_out(4).is_some_and(|part| part.semitones == 0 && !part.name.contains("semitone")));
        // Another song's list, or the plain list: as its line says, or not at all.
        listed(&mut p, Some(vec![fit(4, 5)]), Some(&songs[1]));
        assert_eq!(p.picked_fit_text(), midi_fits::card_text(5, false, "Glass"));
        listed(&mut p, None, None);
        assert_eq!((p.fit_move(4), p.picked_fit.clone()), (0, None));
        // A part that goes to another octave as well is moved by both in one: the picked one and a line alike.
        let low = |entry: usize| Fit { octaves: -1, ..fit(entry, 3) };
        listed(&mut p, Some(vec![low(4), low(2)]), Some(&songs[0]));
        assert_eq!((p.fit_move(4), p.fit_move(2)), (-9, -9));
        assert_eq!(p.picked_fit_text(), midi_fits::card_text(-9, false, "Night drive"));
        assert!(p.part_out(4).is_some_and(|part| part.semitones == -9 && part.name.ends_with(" (9 semitones down)")));
        assert!(p.part_out(2).is_some_and(|part| part.semitones == -9 && part.name.ends_with(" (9 semitones down)")));
        // "Fits my song" off while its list is still on show: a line is no longer moved.
        listed(&mut p, Some(vec![fit(4, 3), fit(2, -2)]), Some(&songs[0]));
        p.fit_song = None;
        assert_eq!((p.fit_move(2), p.fit_of(2)), (0, None));
    }

    /// A part found plays with the parts of the song its list was fitted to, while "Play with the song" is on: the
    /// picked one keeps the ones it was found for, and its card says what they are.
    #[test]
    fn a_part_that_fits_plays_with_the_parts_of_the_song_it_was_fitted_to() {
        let songs = sample_songs();
        let fit = |entry: usize| Fit { entry, semitones: 3, octaves: 0, yours: None, edges: (48, 72), sits: 0.8 };
        let bars = |wants: Wants, name: &str| {
            let sound = mo_midi::gm::sound_for(Role::Chords, false, None, &[], None);
            let notes = vec![NoteEvent {
                start: 0,
                length: 4 * mo_midi::BAR,
                key: 57,
                velocity: 40_000,
                muted: false,
                step: false,
                attribute: mo_formats::NoteAttribute::NONE,
            }];
            Arc::new(SongBars {
                voices: vec![midi_fits::SongVoice { name: name.into(), sound, notes }],
                length: 4 * mo_midi::BAR,
                bar: mo_midi::BAR,
                tempo: Some(128.0),
                wants,
                names: name.into(),
                bars: 8..12,
                view: "Test",
            })
        };
        let (chords, lead) = (bars(Wants::Tune, "Keys"), bars(Wants::Chords, "Lead"));
        let mut p =
            Page { cat: Arc::new(sample().cat), notes: Some(sample().notes), with_song: true, ..Page::default() };
        p.fit_song = Some(songs[0].clone());
        let listed = |p: &mut Page, entries: &[usize], with: Option<&Arc<SongBars>>| {
            p.fits = with.map(|_| Arc::new(entries.iter().map(|&entry| fit(entry)).collect()));
            p.fits_song = with.map(|_| songs[0].clone());
            p.fit_with = with.cloned();
            p.fit_picked();
        };
        // A line of the list plays with what the list is fitted to; a part that isn't in it plays alone.
        listed(&mut p, &[4, 2], Some(&chords));
        assert_eq!((p.heard_with(4), p.heard_with(7)), (Some(chords.clone()), None));
        // Picked: its card's sound line says what it plays with, at once (the song's tempo needs no reading).
        p.picked = Some(4);
        p.fit_picked();
        let line = midi_fits::with_song_line(&page::gm_sound(&p.cat.entries[4]), "Night drive", &chords);
        assert!(line.ends_with("with Keys from Night drive (bars 9–12), at the song's 128 BPM"), "{line}");
        assert_eq!((p.heard_with(4), p.with_song_line(4)), (Some(chords.clone()), Some(line.clone())));
        assert_eq!(p.sound_line_now(4), Some(line.clone()));
        // The list now looks for chords under the song's lead and no longer has the part: it still plays over the
        // chords it was found for, while a line of the new list plays with the lead.
        listed(&mut p, &[2], Some(&lead));
        assert_eq!((p.heard_with(4), p.heard_with(2)), (Some(chords.clone()), Some(lead.clone())));
        // The chip off: alone. Its card says its sound, and its own tempo once that's read; nothing is read again
        // for the chip (the ask for its notes is the same either way).
        let alone = page::sound_line(&page::gm_sound(&p.cat.entries[4]), None);
        p.with_song = false;
        assert_eq!((p.heard_with(4), p.heard_with(2), p.with_song_line(4)), (None, None, None));
        assert_eq!(p.sound_line_now(4), Some(alone.clone()));
        p.card_alone = Some(format!("{alone} · 96 BPM"));
        assert_eq!(p.sound_line_now(4), Some(format!("{alone} · 96 BPM")));
        // On again: the song's parts and tempo, whatever the part's own.
        p.with_song = true;
        assert_eq!(p.sound_line_now(4), Some(line));
        // The same parts and bars read again (the song saved with nothing changed) are the same to play with.
        let again = Arc::new((*chords).clone());
        assert!(same_bars(&Some(chords.clone()), &Some(again)) && same_bars(&None, &None));
        assert!(!same_bars(&Some(chords.clone()), &Some(lead.clone())) && !same_bars(&Some(chords.clone()), &None));
        // Never drums, in the list or picked.
        let drums = p.cat.entries.iter().position(|e| e.role() == Role::Drums).unwrap();
        listed(&mut p, &[4, drums], Some(&chords));
        assert_eq!((p.heard_with(drums), p.heard_with(4)), (None, Some(chords.clone())));
        p.picked = Some(drums);
        p.picked_fit = None;
        p.fit_picked();
        assert!(p.picked_fit.is_some());
        assert_eq!((p.heard_with(drums), p.with_song_line(drums)), (None, None));
        // Nothing of the song plays in the bars, or they have no length: alone.
        p.picked = Some(4);
        p.picked_fit = None;
        let silent = Arc::new(SongBars { voices: Vec::new(), ..(*chords).clone() });
        listed(&mut p, &[4, 2], Some(&silent));
        assert_eq!((p.heard_with(4), p.heard_with(2)), (None, None));
        let no_bars = Arc::new(SongBars { length: 0, ..(*chords).clone() });
        listed(&mut p, &[4, 2], Some(&no_bars));
        assert_eq!((p.heard_with(4), p.heard_with(2)), (None, None));
        // The plain list: nothing is fitted, so nothing plays with the song.
        listed(&mut p, &[4, 2], Some(&chords));
        assert_eq!(p.heard_with(4), Some(chords));
        listed(&mut p, &[], None);
        assert_eq!((p.heard_with(4), p.heard_with(2), p.with_song_line(4)), (None, None, None));
    }

    /// Play in Bitwig, with "Play with the song" on: the bars the part was fitted to go round in Bitwig's arranger
    /// when the song is a Bitwig project read by its arranger, and the card says so, or why the part is alone.
    #[test]
    fn a_part_that_fits_plays_in_bitwig_with_its_songs_bars_going_round() {
        use mo_dawlink::bitwig::fake::FakeBitwig;
        use mo_dawlink::bitwig::{SongLoop, WithSong};
        let songs = sample_songs();
        let (night, glass) = (songs[0].clone(), songs[1].clone());
        let fit = |entry: usize| Fit { entry, semitones: 3, octaves: 0, yours: None, edges: (48, 72), sits: 0.8 };
        let sound = mo_midi::gm::sound_for(Role::Chords, false, None, &[], None);
        let held = NoteEvent {
            start: 0,
            length: 4 * mo_midi::BAR,
            key: 57,
            velocity: 40_000,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let arranger = Arc::new(SongBars {
            voices: vec![midi_fits::SongVoice { name: "Keys".into(), sound, notes: vec![held] }],
            length: 4 * mo_midi::BAR,
            bar: mo_midi::BAR,
            tempo: Some(128.0),
            wants: Wants::Tune,
            names: "Keys".into(),
            bars: 8..12,
            view: mo_formats::bitwig::arrangement::VIEW,
        });
        let mut p =
            Page { cat: Arc::new(sample().cat), notes: Some(sample().notes), with_song: true, ..Page::default() };
        let listed = |p: &mut Page, song: &SongChoice, with: &Arc<SongBars>| {
            p.fit_song = Some(song.clone());
            p.fits = Some(Arc::new(vec![fit(4), fit(2)]));
            p.fits_song = Some(song.clone());
            p.fit_with = Some(with.clone());
            p.fit_picked();
        };
        // A Bitwig song read by its arranger: bars 9 to 12 are 16 beats from beat 32, in the project its save names.
        listed(&mut p, &glass, &arranger);
        let song = p.bitwig_song(4).unwrap();
        assert_eq!(song.bars, Ok(SongLoop { project: "Glass".into(), start: 32.0, length: 16.0 }));
        assert_eq!((song.name.as_str(), &song.with), ("Glass", &arranger));
        // Picked, it's the song of the list that had it, as in the app.
        p.picked = Some(4);
        p.fit_picked();
        assert_eq!(p.bitwig_song(4), Some(song.clone()));
        // A song from FL Studio has no bars in Bitwig; nor has one read from Bitwig's scenes.
        listed(&mut p, &night, &arranger);
        let fl = p.bitwig_song(2).unwrap();
        assert_eq!(fl.bars, Err("Night drive isn't a Bitwig project".into()));
        assert_eq!(p.bitwig_song(4), Some(fl.clone()));
        let scenes = Arc::new(SongBars { view: mo_formats::bitwig::arrangement::SCENES_VIEW, ..(*arranger).clone() });
        listed(&mut p, &glass, &scenes);
        assert!(p.bitwig_song(4).unwrap().bars.is_err());
        // The chip off, drums, or a part that isn't fitted: alone, with nothing said of the song.
        let drums = p.cat.entries.iter().position(|e| e.role() == Role::Drums).unwrap();
        assert_eq!((p.bitwig_song(drums), p.bitwig_song(7)), (None, None));
        p.with_song = false;
        assert_eq!(p.bitwig_song(4), None);

        // What the card says, against the stand-in Bitwig (which has "Night Drive" open, not "Glass").
        let bitwig = FakeBitwig::start().unwrap();
        let dir = std::env::temp_dir().join(format!("mo-ui-midi-with-song-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("MO Play with song.mid");
        std::fs::write(&file, scale_file()).unwrap();
        let turn = mo_dawlink::bitwig::ticket().wait();
        let play = |bars: &SongLoop| bitwig.bitwig().play_with_song(&file, Some(bars), &turn, &|| true).unwrap();
        let other = play(song.bars.as_ref().unwrap());
        assert_eq!(other.with_song, WithSong::OtherProject("Night Drive".into()));
        assert_eq!(
            bitwig_note("Saw lead", &other, Some(&song)),
            "Playing Saw lead through \u{201c}Bass\u{201d} in Bitwig, alone: Bitwig has \u{201c}Night Drive\u{201d} \
             open, not Glass."
        );
        let open = BitwigSong {
            bars: Ok(SongLoop { project: "Night Drive".into(), start: 32.0, length: 16.0 }),
            ..song.clone()
        };
        let looping = play(open.bars.as_ref().unwrap());
        assert_eq!(
            bitwig_note("Saw lead", &looping, Some(&open)),
            "Playing Saw lead through \u{201c}Bass\u{201d} in Bitwig, with bars 9\u{2013}12 of Glass going round."
        );
        assert_eq!(
            bitwig_note("Saw lead", &looping, Some(&fl)),
            "Playing Saw lead through \u{201c}Bass\u{201d} in Bitwig, alone: Night drive isn't a Bitwig project."
        );
        // A part alone says nothing of a song.
        assert_eq!(bitwig_note("Saw lead", &looping, None), "Playing Saw lead through \u{201c}Bass\u{201d} in Bitwig.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Put in the song and Send to Bitwig's mark go by where the part was fitted, whether it's heard with the song
    /// or alone: the card offers Put in the song only where Bitwig's arranger has the bars.
    #[test]
    fn a_part_that_fits_is_offered_its_place_in_the_song_in_bitwig() {
        use mo_dawlink::bitwig::SongLoop;
        let songs = sample_songs();
        let (night, glass) = (songs[0].clone(), songs[1].clone());
        let fit = |entry: usize| Fit { entry, semitones: 3, octaves: 0, yours: None, edges: (48, 72), sits: 0.8 };
        let arranger = Arc::new(SongBars {
            voices: Vec::new(),
            length: 4 * mo_midi::BAR,
            bar: mo_midi::BAR,
            tempo: Some(128.0),
            wants: Wants::Tune,
            names: String::new(),
            bars: 8..12,
            view: mo_formats::bitwig::arrangement::VIEW,
        });
        let mut p = Page { cat: Arc::new(sample().cat), notes: Some(sample().notes), ..Page::default() };
        let listed = |p: &mut Page, song: &SongChoice, with: &Arc<SongBars>| {
            p.fit_song = Some(song.clone());
            p.fits = Some(Arc::new(vec![fit(4), fit(2)]));
            p.fits_song = Some(song.clone());
            p.fit_with = Some(with.clone());
            p.fit_picked();
        };
        p.picked = Some(4);
        listed(&mut p, &glass, &arranger);
        // "Play with the song" is off, and nothing of the song sounds in the bars: it plays alone, and still has
        // its place.
        assert_eq!((p.with_song, p.bitwig_song(4)), (false, None));
        let place = p.bitwig_place(4).unwrap();
        assert_eq!(place.bars, Ok(SongLoop { project: "Glass".into(), start: 32.0, length: 16.0 }));
        assert_eq!((place.name.as_str(), &place.with), ("Glass", &arranger));
        assert_eq!(p.card_fit().put, Some(PutButton::Put));
        // While one is being put in, the button stops the recording; once pressed, it waits.
        let stop = Arc::new(AtomicBool::new(false));
        p.putting = Some(stop.clone());
        assert_eq!(p.card_fit().put, Some(PutButton::Stop));
        stop.store(true, Ordering::SeqCst);
        assert_eq!(p.card_fit().put, Some(PutButton::Stopping));
        p.putting = None;
        // A song from FL Studio, or one read from Bitwig's scenes, has no bars in Bitwig's arranger: no button.
        listed(&mut p, &night, &arranger);
        assert!(p.bitwig_place(4).is_some_and(|place| place.bars.is_err()));
        assert_eq!(p.card_fit().put, None);
        let scenes = Arc::new(SongBars { view: mo_formats::bitwig::arrangement::SCENES_VIEW, ..(*arranger).clone() });
        listed(&mut p, &glass, &scenes);
        assert_eq!(p.card_fit().put, None);
        // Drums and a part that isn't fitted have no place.
        let drums = p.cat.entries.iter().position(|e| e.role() == Role::Drums).unwrap();
        listed(&mut p, &glass, &arranger);
        assert_eq!((p.bitwig_place(drums), p.bitwig_place(7)), (None, None));

        // The button sits beside Send to Bitwig, on the card and in the menu, with no key until you give it one.
        let keys = Shortcuts::default();
        let e = p.cat.entry(4).unwrap();
        let ids = |list: &[CardAction]| list.iter().map(|a| a.id.to_string()).collect::<Vec<_>>();
        let (card, _, menu) = actions(e, &keys, None, Some(PutButton::Put));
        assert_eq!(ids(&card)[..3], ["midi.toBitwig", "midi.putInSong", "midi.moreLike"]);
        let put = &card[1];
        assert_eq!((put.text.as_str(), put.key.as_str(), put.enabled), ("Put in the song", "", true));
        assert!(put.tip.contains("record the picked part into your song's arranger"), "{}", put.tip);
        assert!(put.tip.ends_with("while it records, stop the recording"), "{}", put.tip);
        assert!(menu.iter().any(|m| m.id == "midi.putInSong" && m.enabled));
        // The same button, and the same key, while one is being put in.
        let reads = |put| {
            let (card, _, menu) = actions(e, &keys, None, Some(put));
            assert!(menu.iter().any(|m| m.id == "midi.putInSong" && m.enabled == card[1].enabled));
            (card[1].id.to_string(), card[1].text.to_string(), card[1].enabled)
        };
        assert_eq!(reads(PutButton::Stop), ("midi.putInSong".to_owned(), "Stop the recording".to_owned(), true));
        assert_eq!(
            reads(PutButton::Stopping),
            ("midi.putInSong".to_owned(), "Stopping the recording\u{2026}".to_owned(), false)
        );
        let (card, _, menu) = actions(e, &keys, None, None);
        assert!(!ids(&card).contains(&"midi.putInSong".to_owned()));
        assert!(menu.iter().all(|m| m.id != "midi.putInSong"));
    }

    /// A song's bars as the stand-in Bitwig has them: bars 9 to 12 of "Night Drive", which it has open.
    fn open_song() -> BitwigSong {
        BitwigSong {
            bars: Ok(mo_dawlink::bitwig::SongLoop { project: "Night Drive".into(), start: 32.0, length: 16.0 }),
            name: "Night drive".into(),
            with: Arc::new(SongBars {
                voices: Vec::new(),
                length: 4 * mo_midi::BAR,
                bar: mo_midi::BAR,
                // Fast, so the stand-in's answer isn't waited for long where it never comes.
                tempo: Some(240.0),
                wants: Wants::Tune,
                names: String::new(),
                bars: 8..12,
                view: mo_formats::bitwig::arrangement::VIEW,
            }),
        }
    }

    /// The files in a test's export folder now.
    fn files_in(dir: &Path) -> Vec<String> {
        let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
        read.filter_map(|f| Some(f.ok()?.file_name().to_string_lossy().into_owned())).collect()
    }

    #[test]
    fn a_part_is_put_in_its_song_by_bitwig_recording_it_at_the_songs_bars() {
        let s = sample();
        let e = &s.cat.entries[0];
        let part = part_out(&s, e, 3);
        let dir = std::env::temp_dir().join(format!("mo-ui-midi-put-in-song-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bitwig = mo_dawlink::bitwig::fake::FakeBitwig::start().unwrap();
        let song = open_song();
        let feed = Feed::default();
        let put = |feed: &Feed, export: Result<&Path, &str>, song: &BitwigSong| {
            let job = feed.start("Putting", "");
            let go_on = Arc::new(AtomicBool::new(false));
            put_part(job, &bitwig.bitwig(), &part, export, song, mo_dawlink::bitwig::ticket(), &go_on)
        };
        assert!(put(&feed, Ok(dir.as_path()), &song));
        let world = bitwig.world();
        // On the selected track ("Bass"), from bar 9: the bars it was found for, or its own length when longer.
        let bars = i64::from(e.phrase.bars).max(4);
        assert_eq!(world.arranger.len(), 1, "{:?}", world.arranger);
        let take = &world.arranger[0];
        assert_eq!((take.at, take.length), (32.0, (bars * 4) as f64));
        // From a MIDI file in the export folder under the name that says how it fits (the clip's name in Bitwig),
        // which is gone again now Bitwig has the notes.
        let file = Path::new(&take.file);
        assert!(file.starts_with(&dir) && take.file.ends_with("(3 semitones up).mid"), "{file:?}");
        assert_eq!(files_in(&dir), Vec::<String>::new());
        assert_eq!(world.commands().last().map(String::as_str), Some("deleteclip"));
        drop(world);
        let cards = feed.visible(Instant::now());
        let card = cards.iter().find(|c| c.title == format!("{} is in Night drive", part.name)).unwrap();
        let said = format!("In bars 9\u{2013}{} on \u{201c}Bass\u{201d} in Bitwig's arranger.", 8 + bars);
        assert!(card.detail.starts_with(&said), "{}", card.detail);
        assert_eq!(card.state, mo_app::activity::State::Done);

        // Bitwig playing: nothing goes in, the card says what to do, and the file written for it is gone.
        bitwig.world().playing = true;
        let feed = Feed::default();
        assert!(!put(&feed, Ok(dir.as_path()), &song));
        assert_eq!(bitwig.world().arranger.len(), 1);
        assert_eq!(files_in(&dir), Vec::<String>::new());
        let cards = feed.visible(Instant::now());
        let failed = cards.iter().find(|c| c.title == midi_fits::PUT_FAILED).unwrap();
        assert_eq!(failed.detail, "Bitwig is playing. Stop it, then try again.");
        bitwig.world().playing = false;
        // An export folder that isn't there, or a song with no bars in Bitwig: nothing is written or asked.
        let asked = bitwig.world().requests.len();
        let feed = Feed::default();
        assert!(!put(&feed, Err("The export folder isn't there."), &song));
        let no_bars = BitwigSong { bars: Err("Night drive isn't a Bitwig project".into()), ..song.clone() };
        assert!(!put(&feed, Ok(dir.as_path()), &no_bars));
        assert_eq!(bitwig.world().requests.len(), asked);
        let details: Vec<String> = feed.visible(Instant::now()).iter().map(|c| c.detail.clone()).collect();
        assert!(details.contains(&"The export folder isn't there.".to_owned()), "{details:?}");
        assert!(details.contains(&"Night drive isn't a Bitwig project.".to_owned()), "{details:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_being_put_in_its_song_can_be_stopped() {
        let s = sample();
        let part = part_out(&s, &s.cat.entries[0], 3);
        let dir = std::env::temp_dir().join(format!("mo-ui-midi-put-stopped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The stand-in's recording takes 20 seconds, unless it's asked to stop.
        let mut world = mo_dawlink::bitwig::fake::World::default();
        world.late = vec![("record".into(), Duration::from_secs(20))];
        let bitwig = mo_dawlink::bitwig::fake::FakeBitwig::with(world).unwrap();
        let song = open_song();
        let feed = Feed::default();
        let stop = Arc::new(AtomicBool::new(false));
        let asked = Instant::now();
        let whole = std::thread::scope(|scope| {
            // Stop the recording, pressed once Bitwig records.
            scope.spawn(|| {
                while asked.elapsed() < Duration::from_secs(15) && !bitwig.world().commands().contains(&"record".into())
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                stop.store(true, Ordering::SeqCst);
            });
            let job = feed.start("Putting", "");
            put_part(job, &bitwig.bitwig(), &part, Ok(dir.as_path()), &song, mo_dawlink::bitwig::ticket(), &stop)
        });
        assert!(!whole);
        assert!(asked.elapsed() < Duration::from_secs(15), "it ended when asked: {:?}", asked.elapsed());
        let world = bitwig.world();
        // Bitwig was asked to stop, what played so far is in the arranger, and the clip it came from is out again.
        assert!(world.commands().contains(&"recordabort".to_owned()), "{:?}", world.commands());
        assert_eq!(world.arranger.iter().map(|take| take.length).collect::<Vec<_>>(), [4.0]);
        assert_eq!(world.tracks[2].clips, [""]);
        drop(world);
        assert_eq!(files_in(&dir), Vec::<String>::new());
        let cards = feed.visible(Instant::now());
        let card = cards.iter().find(|c| c.title == midi_fits::PUT_STOPPED).unwrap();
        assert_eq!(
            card.detail,
            "The recording was stopped part way. Some of the part may be in the arranger: undo in Bitwig takes it \
             out (it can take a few presses)."
        );
        assert_eq!(card.state, mo_app::activity::State::Failed);

        // Stopped while it still waits its turn: nothing is asked of Bitwig, and the card says nothing was recorded.
        let requests = bitwig.world().requests.len();
        let feed = Feed::default();
        let job = feed.start("Putting", "");
        let ticket = mo_dawlink::bitwig::ticket();
        assert!(!put_part(job, &bitwig.bitwig(), &part, Ok(dir.as_path()), &song, ticket, &stop));
        assert_eq!(bitwig.world().requests.len(), requests);
        assert_eq!(files_in(&dir), Vec::<String>::new());
        let cards = feed.visible(Instant::now());
        let card = cards.iter().find(|c| c.title == midi_fits::PUT_STOPPED).unwrap();
        assert_eq!((card.detail.as_str(), card.state), (midi_fits::PUT_NOTHING, mo_app::activity::State::Done));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_sent_from_a_fits_my_song_list_has_bitwigs_play_start_mark_its_bars() {
        let s = sample();
        let e = &s.cat.entries[0];
        let part = part_out(&s, e, 3);
        let dir = std::env::temp_dir().join(format!("mo-ui-midi-send-for-song-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bitwig = mo_dawlink::bitwig::fake::FakeBitwig::start().unwrap();
        let song = open_song();
        let feed = Feed::default();
        let sent = send_part(feed.start("Sending", ""), &bitwig.bitwig(), &part, Ok(dir.as_path()), Some(&song));
        assert!(sent.is_some());
        assert_eq!(bitwig.world().commands(), ["newtrack", "clip", "playstart"]);
        assert_eq!(bitwig.world().transport.play_start, 32.0);
        let cards = feed.visible(Instant::now());
        let card = cards.iter().find(|c| c.title == format!("{} is in Bitwig", part.name)).unwrap();
        assert_eq!(
            card.detail,
            format!(
                "{} Bitwig's play start is at bar 9 now, where it fits Night drive: drag the clip down to it in the \
                 arranger.",
                mo_dawlink::bitwig::PART_IN
            )
        );
        // A song with no bars in Bitwig's arranger (FL Studio's, say): sent as from any list, the play start left.
        let fl = BitwigSong { bars: Err("Night drive isn't a Bitwig project".into()), ..song.clone() };
        let feed = Feed::default();
        send_part(feed.start("Sending", ""), &bitwig.bitwig(), &part, Ok(dir.as_path()), Some(&fl)).unwrap();
        assert_eq!(bitwig.world().commands().iter().filter(|c| *c == "playstart").count(), 1);
        let cards = feed.visible(Instant::now());
        assert!(cards.iter().any(|c| c.detail == mo_dawlink::bitwig::PART_IN), "{cards:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With a real SoundFont (MO_SOUNDFONT=path): a part's loop with the song is the part and the song's parts added
    /// up, each going round by its own length at the song's tempo, for as long as it takes both to come back to
    /// their starts.
    #[test]
    #[ignore = "needs a SoundFont: MO_SOUNDFONT=path cargo test -p mo-ui --lib -- --ignored with_a_real_soundfont"]
    fn a_part_plays_over_the_songs_parts_with_a_real_soundfont() {
        let Some(font_path) = std::env::var_os("MO_SOUNDFONT").map(PathBuf::from) else { return };
        let s = sample();
        // The four-bar arp, over six bars of the song at 100 beats a minute: they meet again after twelve.
        let e = s.cat.entries.iter().find(|e| e.role() == Role::Arp).unwrap();
        assert_eq!(e.phrase.bars, 4);
        let bar = mo_midi::BAR;
        let held = |start: i64, length: i64, key: u8| NoteEvent {
            start,
            length,
            key,
            velocity: 40_000,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let voice = |name: &str, role: Role, notes: Vec<NoteEvent>| midi_fits::SongVoice {
            name: name.into(),
            sound: mo_midi::gm::sound_for(role, false, None, &[], None),
            notes,
        };
        let keys = (0..6).flat_map(|b| [57u8, 60, 64].map(|key| held(b * bar, bar, key))).collect();
        let sub = (0..6).map(|b| held(b * bar, bar / 2, 33)).collect();
        let with = Arc::new(SongBars {
            voices: vec![voice("Keys", Role::Chords, keys), voice("Sub", Role::Bass, sub)],
            length: 6 * bar,
            bar,
            tempo: Some(100.0),
            wants: Wants::Tune,
            names: "Keys and Sub".into(),
            bars: 8..14,
            view: "Test",
        });
        assert_eq!(with.round(4 * bar, mo_audio::gm::longest_ticks(mo_midi::PPQ, 100.0)), 12 * bar);
        let ask = |with: Option<Arc<SongBars>>| PlayAsk {
            id: 1,
            font: font_path.clone(),
            part: part_out(&s, e, 0),
            sound: page::gm_sound(e),
            source: Arc::clone(&e.phrase.source),
            library: PathBuf::new(),
            with,
        };
        let started = Instant::now();
        let together = make_loop(&ask(Some(Arc::clone(&with))), &mut None).unwrap();
        let first = started.elapsed();
        // A bar at 100 beats a minute is 105,840 frames.
        assert_eq!((together.len(), together.sample_rate), (12 * 105_840, mo_audio::gm::RATE));
        // The song's loop is kept for the next part of the list: the same one again makes only the part's.
        let kept = SONG_LOOP.lock().unwrap().kept.clone().unwrap();
        assert!(kept.0 == font_path && Arc::ptr_eq(&kept.1, &with) && kept.2.len() == 6 * 105_840);
        let started = Instant::now();
        let again = make_loop(&ask(Some(Arc::new((*with).clone()))), &mut None).unwrap();
        println!("first {first:?}, with the song's loop kept {:?}", started.elapsed());
        assert!(
            Arc::ptr_eq(&SONG_LOOP.lock().unwrap().kept.as_ref().unwrap().2, &kept.2),
            "the same bars: not made again"
        );
        assert_eq!(again.channels, together.channels);

        // What's heard is the part at the song's tempo and the song's loop under it, turned down until it's a little
        // under the part ([`song_gain`]), each going round by its own length: three times and twice.
        let font = mo_audio::gm::open(&font_path).unwrap();
        let (heard, length) = page::loop_notes(&ask(None).part.notes(&mut None).unwrap(), 4);
        assert_eq!(length, 4 * bar);
        let heard = gm_notes(&heard);
        let part = mo_audio::gm::render_loop(&font, &gm_part(&heard, length, 100.0, &page::gm_sound(e))).unwrap();
        let song = &kept.2;
        let (four, six) = (4 * 105_840, 6 * 105_840);
        assert_eq!((part.len(), song.len()), (four, six));
        let (part_level, song_level) = (mo_audio::gm::level(&part), mo_audio::gm::level(song));
        let gain = song_gain(part_level, song_level);
        println!("the part is {part_level:.4} loud and the song's parts {song_level:.4}: they go in at {gain:.3}");
        assert!(part_level > 0.0 && song_level > 0.0 && (SONG_LEAST..=SONG_GAIN).contains(&gain));
        let mut loudest = 0.0f32;
        let mut furthest = 0.0f32;
        for i in (0..together.len()).step_by(97) {
            let want = part.channels[0][i % four] + gain * song.channels[0][i % six];
            loudest = loudest.max(want.abs());
            furthest = furthest.max((together.channels[0][i] - want).abs());
        }
        assert!(loudest > 0.05, "there's sound in it: {loudest}");
        assert!(loudest <= 1.0 && furthest < 1e-5, "added up as they are: off by {furthest}, peak {loudest}");
        // The song's parts can't be made into sound (there are none): the part still plays, alone at the song's
        // tempo.
        let no_song = Arc::new(SongBars { voices: Vec::new(), ..(*with).clone() });
        assert_eq!(make_loop(&ask(Some(no_song)), &mut None).unwrap().channels, part.channels);
        // Alone, it plays at its own tempo and for its own four bars.
        let alone = make_loop(&ask(None), &mut None).unwrap();
        assert!(alone.len() != four || tempo_of(&ask(None).part.from, &e.phrase.source, Path::new("")).bpm() == 100.0);
        // The song's loop is let go once nothing plays with a song.
        forget_song_loop();
        assert!(SONG_LOOP.lock().unwrap().kept.is_none());
    }

    /// The song's loop is kept for the next part of the list, by its bars and the SoundFont; one let go while it was
    /// still being made isn't kept after all.
    #[test]
    fn a_songs_loop_let_go_while_its_made_isnt_kept() {
        let bars = |tempo: f64| {
            Arc::new(SongBars {
                voices: Vec::new(),
                length: 4 * mo_midi::BAR,
                bar: mo_midi::BAR,
                tempo: Some(tempo),
                wants: Wants::Tune,
                names: String::new(),
                bars: 0..4,
                view: "Test",
            })
        };
        let (with, other) = (bars(120.0), bars(90.0));
        let sound = Arc::new(mo_audio::AudioData::default());
        let font = Path::new("a.sf2");
        let made = |with: &Arc<SongBars>| (font.to_owned(), Arc::clone(with), Arc::clone(&sound));
        let mut song = SongKept::default();
        // Made with nothing let go meanwhile: kept, for the same bars (the very same, or read again) and SoundFont.
        let started = song.forgotten;
        song.keep(started, made(&with));
        assert!(song.of(font, &with).is_some() && song.of(font, &Arc::new((*with).clone())).is_some());
        assert!(song.of(font, &other).is_none() && song.of(Path::new("b.sf2"), &with).is_none());
        // Let go while the next was being made: that one isn't kept, and the one before is given back to free.
        let started = song.forgotten;
        assert!(song.forget().is_some());
        song.keep(started, made(&other));
        assert!(song.kept.is_none() && song.forget().is_none());
        // The next made after that is kept again.
        let started = song.forgotten;
        song.keep(started, made(&other));
        assert!(song.of(font, &other).is_some());
    }

    #[test]
    fn the_songs_parts_go_in_a_little_under_the_part_found() {
        // No louder than the part: as they are, a little under.
        assert_eq!((song_gain(0.2, 0.1), song_gain(0.2, 0.2)), (SONG_GAIN, SONG_GAIN));
        // Louder (several parts, or one low and full): turned down to that much of the part's loudness.
        assert!((song_gain(0.1, 0.2) - SONG_GAIN / 2.0).abs() < 1e-6);
        // Never to nothing, however quiet the part; and with silence on either side, as they are.
        assert_eq!(song_gain(0.01, 1.0), SONG_LEAST);
        assert_eq!((song_gain(0.0, 0.5), song_gain(0.3, 0.0)), (SONG_GAIN, SONG_GAIN));
    }

    #[test]
    fn an_octave_chosen_for_the_picked_part_moves_it_everywhere_and_is_remembered() {
        use OctaveChange::{Apps, Down, Up};
        let songs = sample_songs();
        // Each line 3 semitones up for the key, and brought down an octave by the app.
        let fit = |entry: usize| Fit { entry, semitones: 3, octaves: -1, yours: None, edges: (48, 72), sits: 0.8 };
        let mut p = Page { cat: Arc::new(sample().cat), notes: Some(sample().notes), ..Page::default() };
        p.fit_song = Some(songs[0].clone());
        // As a search's answer is listed: the octaves you chose go over the app's.
        let listed = |p: &mut Page, entries: &[usize]| {
            let mut fits: Vec<Fit> = entries.iter().map(|&entry| fit(entry)).collect();
            midi_fits::with_yours(&mut fits, &p.cat, &p.your_octaves);
            p.fits = Some(Arc::new(fits));
            p.fits_song = Some(songs[0].clone());
            p.fit_picked();
        };
        listed(&mut p, &[2, 4]);
        // Nothing picked: nothing to change, and no buttons.
        assert_eq!((p.change_octave(Up), p.card_fit()), (None, CardFit::default()));
        p.picked = Some(4);
        p.fit_picked();
        let hash = p.cat.entry(4).map(|e| e.phrase.hash.clone()).unwrap_or_default();
        let card = p.card_fit();
        let can = |up, down, yours| Some(OctaveButtons { up, down, yours });
        assert_eq!((card.octave, card.moved), (can(true, true, false), true));
        assert_eq!(card.text, midi_fits::card_text(-9, false, "Night drive"));

        // Up from where the app put it: back where it's saved, which is what's remembered for its notes. Its line
        // (the second) says so, and the other line stays.
        assert_eq!(p.change_octave(Up), Some((hash.clone(), Some(0), vec![1])));
        assert_eq!((p.fit_move(4), p.fit_move(2)), (3, -9));
        assert_eq!(p.fit_of(4).map(|f| (f.octaves, f.yours)), Some((-1, Some(0))));
        let card = p.card_fit();
        assert_eq!(card.text, midi_fits::card_text(3, true, "Night drive"));
        assert!(card.text.contains("at the octave you chose"), "{}", card.text);
        assert_eq!(card.octave, can(true, true, true), "the app's octave is offered back");
        assert!(p.part_out(4).is_some_and(|part| part.semitones == 3 && part.name.ends_with(" (3 semitones up)")));
        // Up to the top: 3 octaves, and there Octave up has nothing more to do.
        for octaves in 1..=3 {
            assert_eq!(p.change_octave(Up).map(|(_, kept, _)| kept), Some(Some(octaves)));
        }
        assert_eq!((p.change_octave(Up), p.fit_move(4)), (None, 39));
        assert_eq!(p.card_fit().octave, can(false, true, true));

        // A new list (a filter ticked, the song saved again) has the part where you put it.
        listed(&mut p, &[4, 2]);
        assert_eq!((p.fit_move(4), p.fit_move(2)), (39, -9));
        // In a list that no longer has it, the picked part's buttons still work: no line to say it.
        listed(&mut p, &[2]);
        assert_eq!(p.change_octave(Down), Some((hash.clone(), Some(2), Vec::new())));
        assert_eq!(p.fit_move(4), 27);
        // The app's octave: its pick again, remembered as no choice; a second press has nothing to do.
        listed(&mut p, &[2, 4]);
        assert_eq!(p.change_octave(Apps), Some((hash.clone(), None, vec![1])));
        assert_eq!((p.fit_move(4), p.card_fit().octave), (-9, can(true, true, false)));
        assert_eq!(p.change_octave(Apps), None);
        listed(&mut p, &[2, 4]);
        assert_eq!(p.fit_move(4), -9, "and a new list leaves it with the app");
        // Down to the bottom: 3 octaves under how it's saved, and there Octave down has nothing more to do.
        for octaves in [-2, -3] {
            assert_eq!(p.change_octave(Down).map(|(_, kept, _)| kept), Some(Some(octaves)));
        }
        assert_eq!((p.change_octave(Down), p.fit_move(4)), (None, -33));
        assert_eq!(p.card_fit().octave, can(true, false, true));
        assert_eq!(p.change_octave(Apps).map(|(_, kept, _)| kept), Some(None));

        // The buttons and the menu's lines, with their keys: down and up side by side, the app's octave once it's
        // yours; the menu has them after playing.
        let keys = Shortcuts::default();
        let e = p.cat.entry(4).unwrap();
        let said = |list: &[CardAction]| -> Vec<(String, String, bool)> {
            list.iter().map(|a| (a.text.to_string(), a.key.to_string(), a.enabled)).collect()
        };
        let (card, octave, menu) = actions(e, &keys, can(false, true, true), None);
        assert_eq!(
            said(&octave),
            [
                ("Octave down".to_owned(), "Alt+Down".to_owned(), true),
                ("Octave up".to_owned(), "Alt+Up".to_owned(), false),
                ("The app's octave".to_owned(), String::new(), true),
            ]
        );
        let lines: Vec<&str> = menu.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            lines[..6],
            ["app.play", "midi.playBitwig", "midi.playFl", "midi.octaveDown", "midi.octaveUp", "midi.octaveApp"]
        );
        assert!(card.iter().all(|a| !a.id.starts_with("midi.octave")), "the card's own buttons stay as they were");
        // The app's pick still stands: nothing to give back. A part that isn't fitted: no octave buttons at all.
        assert_eq!(actions(e, &keys, can(true, true, false), None).1.len(), 2);
        let (_, octave, menu) = actions(e, &keys, None, None);
        assert!(octave.is_empty() && menu.iter().all(|m| !m.id.starts_with("midi.octave")));

        // Drums aren't moved by octaves (their notes pick pads), wherever they turn up.
        let drums = p.cat.entries.iter().position(|e| e.role() == Role::Drums).unwrap();
        p.picked = Some(drums);
        listed(&mut p, &[drums]);
        assert_eq!((p.card_fit().octave, p.change_octave(Up)), (None, None));
        // In a plain list nothing is fitted: no buttons, nothing to change.
        p.picked = Some(4);
        p.fits = None;
        p.fits_song = None;
        p.fit_picked();
        assert_eq!((p.card_fit(), p.change_octave(Up)), (CardFit::default(), None));
    }

    #[test]
    fn a_part_goes_to_bitwig_as_a_new_midi_file_on_a_new_track() {
        let s = sample();
        let e = &s.cat.entries[0];
        let part = part_out(&s, e, 0);
        let dir = std::env::temp_dir().join(format!("mo-ui-midi-to-bitwig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bitwig = mo_dawlink::bitwig::fake::FakeBitwig::start().unwrap();
        let feed = mo_app::activity::Feed::default();
        let file = send_part(feed.start("Sending", ""), &bitwig.bitwig(), &part, Ok(dir.as_path()), None).unwrap();
        // A real MIDI file of the part, beside nothing else.
        let bytes = std::fs::read(&file).unwrap();
        assert!(bytes.starts_with(b"MThd"));
        assert!(file.starts_with(&dir));
        let world = bitwig.world();
        assert_eq!(world.commands(), ["newtrack", "clip"]);
        let track = world.tracks.iter().find(|t| t.name == part.name).unwrap();
        assert_eq!(track.clips, [file.to_string_lossy().into_owned()]);
        let cards = feed.visible(Instant::now());
        assert!(cards.iter().any(|c| c.title == format!("{} is in Bitwig", part.name)), "{cards:?}");
        drop(world);

        // Sent from a "Fits my song" list it goes as it fits: every note moved by what its line says, in a file and
        // on a track whose name says so. The part's own file above is as it was.
        let keys = |file: &Path| -> Vec<u8> {
            let parts = mo_formats::midi::read(file).unwrap();
            parts.iter().flat_map(|p| p.notes.iter().map(|n| n.key)).collect()
        };
        let own = keys(&file);
        let moved = part_out(&s, e, 3);
        let feed = mo_app::activity::Feed::default();
        let moved_file =
            send_part(feed.start("Sending", ""), &bitwig.bitwig(), &moved, Ok(dir.as_path()), None).unwrap();
        assert!(moved_file.to_string_lossy().ends_with("(3 semitones up).mid"), "{moved_file:?}");
        assert!(!own.is_empty());
        assert_eq!(keys(&moved_file), own.iter().map(|k| k + 3).collect::<Vec<_>>());
        assert_eq!(keys(&file), own);
        assert!(bitwig.world().tracks.iter().any(|t| t.name == moved.name && t.name.ends_with("(3 semitones up)")));

        // An export folder that isn't there: nothing is written or sent, and the card says why.
        let feed = mo_app::activity::Feed::default();
        let gone = "The export folder isn't there.";
        assert!(send_part(feed.start("Sending", ""), &bitwig.bitwig(), &part, Err(gone), None).is_none());
        assert_eq!(bitwig.world().commands(), ["newtrack", "clip", "newtrack", "clip"]);
        let cards = feed.visible(Instant::now());
        assert!(cards.iter().any(|c| c.detail == gone), "{cards:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Standard MIDI File of one track: a C major scale up and down, a beat a note, at 96 ticks a beat.
    fn scale_file() -> Vec<u8> {
        let mut track = Vec::new();
        for key in [60u8, 62, 64, 65, 67, 69, 71, 72, 71, 69, 67, 65, 64, 62, 60] {
            track.extend([0x00, 0x90, key, 100, 0x60, 0x80, key, 0]);
        }
        track.extend([0x00, 0xFF, 0x2F, 0x00]);
        let mut file = b"MThd".to_vec();
        file.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
        file.extend(b"MTrk");
        file.extend((track.len() as u32).to_be_bytes());
        file.extend(track);
        file
    }

    #[test]
    fn reading_fills_the_rust_index_and_the_page_lists_it_with_new_keys() {
        let dir = std::env::temp_dir().join("mo-ui-midi-read-test");
        let _ = std::fs::remove_dir_all(&dir);
        let pack = dir.join("New Midi").join("Keys");
        assert!(std::fs::create_dir_all(&pack).is_ok());
        assert!(std::fs::write(pack.join("Scale in C.mid"), scale_file()).is_ok());
        let library = dir.join("library.db");
        let Ok(c) = rusqlite::Connection::open(&library) else { panic!("couldn't make the library") };
        let folders = format!("[\"{}\"]", dir.join("New Midi").to_string_lossy().replace('\\', "\\\\"));
        let made = c.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
                 daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);",
        );
        assert!(made.is_ok(), "{made:?}");
        assert!(c.execute("INSERT INTO settings (key, value) VALUES ('midi_folders', ?1)", [&folders]).is_ok());
        drop(c);

        let stop = AtomicBool::new(false);
        let ran = read_into_index(&library, false, &stop, &|_, _, _| {});
        let Ok(summary) = ran else { panic!("reading failed: {ran:?}") };
        assert_eq!((summary.read, summary.failed), (1, 0));
        assert!(summary.parts >= 1);
        // Nothing new the second time.
        let again = read_into_index(&library, false, &stop, &|_, _, _| {});
        assert!(again.is_ok_and(|s| s.read == 0));

        let Ok(loaded) = load(&library) else { panic!("the Rust index should read") };
        // The piano roll keeps the index open from one part to the next.
        let mut open = None;
        let (id, hash) = loaded.cat.entries.first().map(|e| (e.phrase.id, e.phrase.hash.clone())).unwrap_or_default();
        assert!(notes_of(&loaded.notes, id, &hash, &mut open).is_ok_and(|n| n.len() == 15));
        assert!(open.is_some());
        let Some(e) = loaded.cat.entries.first() else { panic!("the scale should be listed") };
        assert!(e.phrase.key_fact.is_some(), "the new key finder's reading comes with the part");
        // A scale alone can't say which of a pair is home: C major is named, alone or with A minor.
        assert!(
            page::in_key(e, mo_harmony::Key::parse("C").unwrap_or(mo_harmony::Key::ALL[0])),
            "{}",
            page::key_text(e)
        );
        drop(open);

        // An index of an older layout (as one another copy of the app held open when it was to be copied) is read as
        // it is, and not kept open between parts, so reading your MIDI can copy it.
        let Ok(c) = rusqlite::Connection::open(MidiStore::path_beside(&library)) else {
            panic!("the index should open")
        };
        let older = c.execute_batch(
            "ALTER TABLE notes ADD COLUMN data BLOB NOT NULL DEFAULT x'';
             UPDATE notes SET data = (SELECT d.data FROM note_data d WHERE d.notes = notes.id);
             DROP TABLE note_data; PRAGMA user_version = 1;",
        );
        assert!(older.is_ok(), "{older:?}");
        drop(c);
        let mut open = None;
        assert!(notes_of(&loaded.notes, id, &hash, &mut open).is_ok_and(|n| n.len() == 15));
        assert!(open.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_samples_song_is_searched_for_parts_that_fit_it() {
        let s = sample();
        let songs = sample_songs();
        let Some(song) = songs.first().cloned() else { panic!("the sample should offer a song") };
        // Its saves are ones the sample's parts say they're from.
        let from = |save: &str| s.cat.entries.iter().any(|e| e.phrase.source.path == save);
        assert!(from(&song.save) && songs.iter().all(|song| from(&song.save)));
        let ask = FitAsk {
            id: 1,
            cat: Arc::new(s.cat),
            filters: Filters::default(),
            song,
            wanted: None,
            yours: midi_fits::YourPick::default(),
            slots_made: 0,
            from: s.notes.clone(),
            stop: Arc::default(),
        };
        let mut state = FitKept::default();
        let answer = fit_search(&ask, &mut state);
        let Answer::Fits { wants, to, looked, with, .. } = answer else { panic!("no list: {answer:?}") };
        // The made-up song is read from the sample's own parts: its bassline is what there is to play over (its
        // keys are single notes in a row, which the song's reader doesn't take for chords).
        assert_eq!((wants, to.as_str()), (Wants::Tune, "To play over Sub, bars 1–8"));
        // And it's what a part found plays with: the bassline over those bars, at the song's tempo.
        assert_eq!((with.names.as_str(), with.bars.clone(), with.voices.len()), ("Sub", 0..8, 1));
        assert_eq!((with.length, with.wants, with.tempo.is_some()), (8 * mo_midi::BAR, Wants::Tune, true));
        // The first search is logged, and what it read is kept for the next.
        assert!(state.logged && !state.kept.is_empty());
        // Parts are found, and none is the song's own (from either of its saves).
        assert!(!looked.fits.is_empty());
        let own = |fit: &Fit| ask.cat.entry(fit.entry).is_none_or(|e| e.phrase.source.path.contains("Night drive"));
        assert!(!looked.fits.iter().any(own), "{looked:?}");
        // Each comes with its octave: the arp is where an arp plays once it's in key; the flute hook is written
        // high, and 5 semitones up for the key it would be well above where a tune plays, so it comes down an
        // octave.
        let fit = |name: &str| {
            let named = |f: &&Fit| ask.cat.entry(f.entry).is_some_and(|e| page::title(e).starts_with(name));
            looked.fits.iter().find(named).map(|f| (f.semitones, f.octaves, midi_fits::fit_text(f)))
        };
        assert_eq!(fit("Arp"), Some((3, 0, "Fits 62%, 3 semitones up".to_owned())));
        assert_eq!(fit("Flute hook A"), Some((5, -1, "Fits 38%, 7 semitones down".to_owned())));
        // Told to stop, it gives nothing to show.
        let stopped = FitAsk { stop: Arc::new(AtomicBool::new(true)), ..ask };
        assert_eq!(fit_search(&stopped, &mut FitKept::default()), Answer::Stopped);
    }

    #[test]
    fn the_songs_parts_and_bars_picked_by_hand_are_asked_for_and_remembered() {
        use midi_fits::{Picker, YourPick, YourPicks};
        let s = sample();
        let songs = sample_songs();
        let (cat, notes) = (Arc::new(s.cat), s.notes);
        let mut p = Page { cat: cat.clone(), fit_songs: songs.clone(), ..Page::default() };
        // A search for what the page asks for now, whose picker the page then shows: what it says it's fitted to.
        let search = |p: &mut Page| {
            let Some(song) = p.fit_song.clone() else { panic!("no song is picked") };
            let ask = FitAsk {
                id: 1,
                cat: cat.clone(),
                filters: Filters::default(),
                song,
                wanted: p.fit_wants,
                yours: p.fit_yours.clone(),
                slots_made: 0,
                from: notes.clone(),
                stop: Arc::default(),
            };
            let (to, picker) = match fit_search(&ask, &mut FitKept::default()) {
                Answer::Fits { to, picker, .. } => (to, picker),
                Answer::CantFit { why, picker } => (why, picker),
                other => panic!("no picker: {other:?}"),
            };
            p.fit_picker = Some(Arc::unwrap_or_clone(picker));
            to
        };
        let on = |p: &Page| {
            let picker = p.fit_picker.as_ref().map(|picker| picker.parts.iter().filter(|part| part.on));
            picker.map(|on| on.map(|part| part.label.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default()
        };
        let yours = |p: &Page| p.fit_picker.as_ref().is_some_and(Picker::is_yours);

        // A song picked, with nothing of it picked by hand: the app's pick, its bassline over all eight bars.
        assert!(p.fit_song_is(songs[0].clone()));
        assert_eq!((search(&mut p).as_str(), on(&p).as_str(), yours(&p)), ("To play over Sub, bars 1–8", "Sub", false));
        // The lead's chip pressed: it's on at once, remembered for the song (the setting is to be written), and
        // what the next search is fitted to.
        assert_eq!(p.fit_pick_press(|picker| picker.toggle_label("Lead")), Some(true));
        assert_eq!(p.fit_picks.text(), r#"[{"song":"night drive","parts":[["Sub",0],["Lead",0]]}]"#);
        assert_eq!((on(&p).as_str(), yours(&p)), ("Sub, Lead", true));
        assert_eq!(search(&mut p), "To play over Sub and Lead, bars 1–8");
        // A press that changes nothing (a handle held where it is, words no chip says) asks for nothing.
        assert_eq!(p.fit_pick_press(|picker| picker.slide_bars(0, 8)), None);
        assert_eq!(p.fit_pick_press(|picker| picker.toggle_label("Pad")), None);
        // The slider's higher handle dragged to the fifth bar line: bars 1 to 4, remembered with the parts.
        assert_eq!(p.fit_pick_press(|picker| picker.slide_bars(0, 4)), Some(true));
        let kept = r#"[{"song":"night drive","parts":[["Sub",0],["Lead",0]],"bars":[0,4]}]"#;
        assert_eq!(p.fit_picks.text(), kept);
        assert_eq!(search(&mut p), "To play over Sub and Lead, bars 1–4");
        let picked = p.fit_yours.clone();

        // The switch pressed: the parts go back to the app (its pick of the song's melodies now), the bars stay.
        // Until the search says which parts those are, none is lit.
        assert!(p.fit_wants_press(Wants::Chords));
        assert_eq!(p.fit_picks.text(), r#"[{"song":"night drive","bars":[0,4]}]"#);
        assert_eq!(
            (p.fit_wants, p.fit_yours.clone(), on(&p).as_str()),
            (Some(Wants::Chords), YourPick { parts: None, bars: Some(0..4) }, "")
        );
        assert_eq!(search(&mut p), "Chords for Keys and Lead, bars 1–4");
        assert_eq!((on(&p).as_str(), yours(&p)), ("Keys, Lead", true));
        // Pressed with no parts of yours to give back: nothing to write.
        assert!(!p.fit_wants_press(Wants::Tune));

        // Another song has its own pick (none yet), and decides for itself what's looked for.
        assert!(p.fit_song_is(songs[1].clone()));
        assert_eq!((p.fit_yours.clone(), p.fit_wants, p.fit_picker.is_none()), (YourPick::default(), None, true));
        // The first one again: fitted to what you picked of it the last time.
        assert!(p.fit_song_is(songs[0].clone()));
        assert_eq!(p.fit_yours, YourPick { parts: None, bars: Some(0..4) });
        assert_eq!(search(&mut p), "To play over Sub, bars 1–4");
        // Picked again while it's on, it keeps what's chosen and its picker.
        p.fit_wants = Some(Wants::Tune);
        assert!(!p.fit_song_is(songs[0].clone()));
        assert_eq!((p.fit_wants, p.fit_picker.is_some()), (Some(Wants::Tune), true));
        // Off: nothing is asked for by hand, and the song's pick stays remembered for when it's on again.
        assert!(p.fit_is_off());
        assert_eq!((p.fit_yours.clone(), p.fit_picker.is_none(), p.fit_is_off()), (YourPick::default(), true, false));
        assert!(p.fit_song_is(songs[0].clone()));
        assert_eq!(p.fit_yours.bars, Some(0..4));
        // "The app's pick": nothing is yours, and nothing is remembered for the song. The parts were the app's
        // already, so they stay lit.
        search(&mut p);
        assert_eq!(p.fit_pick_press(Picker::apps_pick), Some(true));
        assert_eq!((p.fit_picks.text().as_str(), on(&p).as_str()), ("[]", "Sub"));
        assert_eq!((p.fit_yours.clone(), yours(&p)), (YourPick::default(), false));
        assert_eq!(search(&mut p), "To play over Sub, bars 1–8");
        // With no picker yet (the song isn't read), a press has nothing to change.
        p.fit_picker = None;
        assert_eq!(p.fit_pick_press(Picker::apps_pick), None);

        // The app started again with the setting as it was after the bars were picked: the song has that pick.
        let mut later = Page { fit_picks: YourPicks::read(kept), ..Page::default() };
        assert!(later.fit_song_is(songs[0].clone()));
        assert_eq!(later.fit_yours, picked);
        assert!(later.fit_song_is(songs[1].clone()));
        assert_eq!(later.fit_yours, YourPick::default());
    }

    #[test]
    fn a_song_on_disk_is_fitted_from_the_index_on_disk() {
        // The song: a recipe save (made by hand for tests), C D E G on FL Keys. The library: one MIDI file, a C
        // major scale, read into a real index.
        let dir = std::env::temp_dir().join("mo-ui-midi-fits-test");
        let _ = std::fs::remove_dir_all(&dir);
        let pack = dir.join("New Midi").join("Keys");
        assert!(std::fs::create_dir_all(&pack).is_ok());
        assert!(std::fs::write(pack.join("Scale in C.mid"), scale_file()).is_ok());
        let library = dir.join("library.db");
        let Ok(c) = rusqlite::Connection::open(&library) else { panic!("couldn't make the library") };
        let folders = format!("[\"{}\"]", dir.join("New Midi").to_string_lossy().replace('\\', "\\\\"));
        let made = c.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
                 daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);",
        );
        assert!(made.is_ok(), "{made:?}");
        assert!(c.execute("INSERT INTO settings (key, value) VALUES ('midi_folders', ?1)", [&folders]).is_ok());
        drop(c);
        let ran = read_into_index(&library, false, &AtomicBool::new(false), &|_, _, _| {});
        assert!(ran.is_ok_and(|s| s.read == 1), "the scale should be read");
        let Ok(loaded) = load(&library) else { panic!("the index should read") };

        // An octave chosen for the scale is kept in the index, for the next time it's read; the app's pick again
        // takes it out.
        let index = MidiStore::path_beside(&library);
        let scale = loaded.cat.entries[0].phrase.hash.clone();
        assert_eq!(loaded.cat.entries[0].phrase.your_octave, None);
        let read_again = || load(&library).ok().and_then(|l| l.cat.entries.first().map(|e| e.phrase.your_octave));
        let said = |octaves| [((index.clone(), scale.clone()), octaves)];
        assert_eq!(keep_octaves(&said(Some(-2))), Ok(()));
        assert_eq!(read_again(), Some(Some(-2)));
        // Several in one go, the index opened once; one for notes it doesn't have is kept too (it has no part to
        // show it on).
        let more = [said(Some(1)).to_vec(), vec![((index.clone(), "no such notes".to_owned()), Some(3))]].concat();
        assert_eq!((keep_octaves(&more), read_again()), (Ok(()), Some(Some(1))));
        assert_eq!((keep_octaves(&said(None)), read_again()), (Ok(()), Some(None)));
        // An index that can't be opened (here its folder is a file) says why, and the others are still written.
        let nowhere = library.join("midi-index.db");
        let both = [vec![((nowhere.clone(), scale.clone()), Some(2))], said(Some(2)).to_vec()].concat();
        assert!(keep_octaves(&both).is_err());
        assert_eq!(read_again(), Some(Some(2)));
        // The other way round: what was meant for the index that didn't open isn't written into the one that did.
        let other_way = [said(Some(1)).to_vec(), vec![((nowhere, scale.clone()), Some(3))]].concat();
        assert!(keep_octaves(&other_way).is_err());
        assert_eq!(read_again(), Some(Some(1)));
        // From the page it goes off the window's thread, the newest said for the notes last.
        remember_octave(index.clone(), scale.clone(), Some(-3));
        remember_octave(index.clone(), scale.clone(), Some(-1));
        let kept = (0..500).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            read_again() == Some(Some(-1))
        });
        assert!(kept, "the octave said last should be the one kept");
        assert_eq!(keep_octaves(&said(None)), Ok(()));

        let save = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mo-formats/tests/fixtures/recipe/fl/MO Recipe 02 notes.flp")
            .to_string_lossy()
            .into_owned();
        let song = SongChoice {
            name: "MO Recipe 02 notes".into(),
            key: "mo recipe 02 notes".into(),
            label: "MO Recipe 02 notes".into(),
            save: save.clone(),
            mtime: 0,
            saves: vec![save.clone()],
        };
        let ask = |wanted| FitAsk {
            id: 1,
            cat: Arc::new(Catalogue::default()),
            filters: Filters::default(),
            song: song.clone(),
            wanted,
            yours: midi_fits::YourPick::default(),
            slots_made: 0,
            from: loaded.notes.clone(),
            stop: Arc::default(),
        };
        let with_index = FitAsk { cat: Arc::new(loaded.cat), ..ask(Some(Wants::Chords)) };
        let mut state = FitKept::default();
        // Chords for its melody: the scale is tried (its slots and its notes come from the index, kept open), and
        // the song's save is read from disk, where its time can be told.
        let answer = fit_search(&with_index, &mut state);
        let Answer::Fits { wants, to, looked, with, .. } = answer else { panic!("no list: {answer:?}") };
        assert_eq!((wants, looked.tried, looked.not_ready, looked.unread), (Wants::Chords, 1, 0, 0));
        assert!(to.starts_with("Chords for "), "{to}");
        assert!(with.wants == Wants::Chords && !with.voices.is_empty() && with.length > 0, "{with:?}");
        assert!(written_now(&save) > 0 && state.open.is_some());
        // A song with a melody alone has nothing to play a tune over: said in plain words.
        let answer = fit_search(&ask(Some(Wants::Tune)), &mut state);
        let cant = matches!(&answer, Answer::CantFit { why, .. } if why.contains("look for chords for that"));
        assert!(cant, "{answer:?}");
        // The same song asked about another index (here an empty one): nothing of the first is offered.
        let answer = fit_search(&ask(Some(Wants::Chords)), &mut state);
        let Answer::Fits { looked, .. } = answer else { panic!("no list: {answer:?}") };
        assert_eq!((looked.tried, looked.fits.len()), (0, 0));
        // A save that isn't there: said with the song's name, and its time can't be told.
        let gone = SongChoice { save: dir.join("Gone.flp").to_string_lossy().into_owned(), ..song.clone() };
        assert_eq!(written_now(&gone.save), 0);
        let answer = fit_search(&FitAsk { song: gone, ..ask(None) }, &mut state);
        assert!(matches!(&answer, Answer::Nothing(why) if why.starts_with("MO Recipe 02 notes couldn't be read. ")));
        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_index_is_an_empty_list_not_an_error() {
        let dir = std::env::temp_dir().join("mo-ui-midi-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let Ok(l) = load(&dir.join("library.db")) else { panic!("no index should read as empty") };
        assert!(l.cat.entries.is_empty());
    }
}
