//! Fills the MIDI page from the MIDI index and answers its chips, list and card. Everything slow runs off the window's
//! thread at low priority (rules 12 and 21): reading the index once, the first time the page is shown; working out
//! each list (newest ask wins); and reading the picked part's notes for its piano roll and its key. Lines are only
//! made when the list draws them, so 300,000 parts cost what's on screen.
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
use mo_app::places::Place;
use mo_app::shortcuts::{Shortcuts, what_of};
use mo_app::{log, to_bitwig};
use mo_formats::NoteEvent;
use mo_harmony::KeyFact;
use mo_library::Library;
use mo_library::songs::thousands;
use mo_midi::indexer;
use mo_midi::store::MidiStore;
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
    pub years: Vec<(i32, usize)>,
    /// What Ctrl+K finds: every part.
    pub found: Arc<mo_app::finder::Lines>,
}

impl Loaded {
    pub fn new(cat: Catalogue, notes: NotesFrom) -> Loaded {
        let summary = page::summary(&cat);
        let years = page::year_counts(&cat);
        let found = Arc::new(mo_app::finder::midi_parts(&cat));
        Loaded { cat, notes, summary, years, found }
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
}

/// An ask for a part's notes: the roll, and its key when the index has no reading of the new key finder.
struct NotesAsk {
    id: u64,
    from: NotesFrom,
    phrase_id: i64,
    hash: String,
    role: Role,
    cut: bool,
    bars: i32,
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
    /// Works everything out in place, for the screenshots, which have no event loop to send answers back to.
    static HERE: RefCell<bool> = const { RefCell::new(false) };
    /// The filter chips' lines, kept between showings: kinds, From and Key.
    static CHIPS: [FilterModels; 3] = Default::default();
}

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
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
    });
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
    let card = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.shortcuts = shortcuts.clone();
        let e = p.cat.entry(p.picked?)?;
        Some(actions(e, shortcuts))
    });
    if let Some((actions, menu)) = card {
        data.set_actions(ModelRc::new(VecModel::from(actions)));
        data.set_menu(ModelRc::new(VecModel::from(menu)));
    }
}

/// Lights the heading of the column the list goes by, with which way (the UX review of 10 Nov, B5); none while the
/// list is shuffled or ranked (most alike, longest phrase), which no column stands for.
fn show_order(data: &MidiData) {
    let (column, reversed) = PAGE.with(|p| {
        let p = p.borrow();
        if p.seed.is_some() || p.target.is_some() { ("", false) } else { (p.order.column(), p.reversed) }
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
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let lit = p.seed.is_none() && p.target.is_none() && p.order == order;
                p.reversed = lit && !p.reversed;
                p.order = order;
                p.seed = None;
            });
        }
        relist(&app);
    });
    let weak = app.as_weak();
    data.on_shuffle(move || {
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
        // The old index goes on the freeing thread: tens of thousands of parts take a while to let go.
        background::free_later(std::mem::replace(&mut p.cat, Arc::new(loaded.cat)));
        p.notes = Some(loaded.notes);
        p.picked = hash.and_then(|h| p.cat.find(&h));
        let was_looping = p.looping.is_some();
        p.looping = looping.and_then(|h| p.cat.find(&h));
        p.picked_row = None;
        p.target = None;
        // The old list points into the old index: nothing shows until the new one is worked out.
        background::free_later(std::mem::replace(&mut p.shown, Arc::new(Vec::new())));
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
    data.set_summary(summary.into());
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
        let loaded = changed.then(|| load(&library));
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            PAGE.with(|p| p.borrow_mut().reading = None);
            let data = app.global::<MidiData>();
            data.set_reading(false);
            data.set_busy("".into());
            if let Some(loaded) = loaded {
                show(&app, loaded);
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
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.asked += 1;
        Ask {
            id: p.asked,
            cat: p.cat.clone(),
            filters: p.filters.clone(),
            order: p.order,
            reversed: p.reversed,
            target: p.target,
            seed: p.seed,
        }
    });
    app.global::<MidiData>().set_any_filter(PAGE.with(|p| p.borrow().filters.any()));
    if HERE.with(|h| *h.borrow()) {
        let (hits, counts) = page::view(&ask.cat, &ask.filters, ask.order, ask.reversed, ask.target, ask.seed);
        listed(app, ask.id, hits, counts);
        return;
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("MIDI list", move |ask: Ask| {
                let (hits, counts) = background::on_pool(|| {
                    page::view(&ask.cat, &ask.filters, ask.order, ask.reversed, ask.target, ask.seed)
                });
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
            w.ask(ask);
        }
    });
}

fn listed(app: &AppWindow, id: u64, hits: Vec<Hit>, counts: page::ChoiceCounts) {
    let (shown, result, target) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.asked != id {
            return (None, String::new(), String::new());
        }
        // The picked part's row in the new list: one look, here, rather than at every step.
        p.picked_row = p.picked.and_then(|at| page::row_of(&p.cat, &hits, at));
        // The old list (one line per part shown) is let go on the freeing thread.
        background::free_later(std::mem::replace(&mut p.shown, Arc::new(hits)));
        p.counts = counts;
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
        let all = p.cat.line_count;
        (Some(shown), page::result_text(shown, all, p.order, p.reversed, p.target, p.seed.is_some()), target_text)
    });
    let Some(shown) = shown else { return };
    show_chips(app);
    LINES.with(|l| l.notify.reset());
    let data = app.global::<MidiData>();
    data.set_shown(shown as i32);
    data.set_result(result.into());
    data.set_target(target.into());
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
}

/// Shows one part, by its notes' fingerprint (Ctrl+K): picked, and in sight; when the filters hide it, they go. Before
/// the index is read it's shown once it is.
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
        p.picked = picked.map(|(at, _)| at);
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
        p.notes_asked += 1;
        let id = p.notes_asked;
        let notes = p.notes.clone()?;
        let shortcuts = p.shortcuts.clone();
        let e = p.cat.entry(at)?;
        let card = card(e);
        let facts: Vec<MidiFact> = page::facts(e, &p.cat.dates)
            .into_iter()
            .map(|(n, v)| MidiFact { name: n.into(), value: v.into() })
            .collect();
        let (actions, menu) = actions(e, &shortcuts);
        data.set_picked(card);
        data.set_facts(ModelRc::new(VecModel::from(facts)));
        data.set_actions(ModelRc::new(VecModel::from(actions)));
        data.set_menu(ModelRc::new(VecModel::from(menu)));
        // Its tempo follows with the notes (it's read from its song or file).
        data.set_sound_line(page::sound_line(&page::gm_sound(e), None).into());
        crate::piano_roll::show(app, None);
        Some(NotesAsk {
            id,
            from: notes,
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            role: e.role(),
            // As the stored key was read; the clip's name only for a part whose key isn't read yet.
            cut: e.phrase.cut.unwrap_or_else(|| page::is_cut(&e.phrase.clip)),
            bars: e.phrase.bars,
            key: e.phrase.key_fact,
            source: Arc::clone(&e.phrase.source),
            library: p.library.clone(),
            sound: page::gm_sound(e),
        })
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
    let Some(ask) = ask else { return };
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

/// A part's notes, from wherever the index is; `open` keeps the index open for the next ask. Not one of an older
/// layout, which can't be copied into this app's while it's open (reading your MIDI copies it), nor one a read failed
/// on: the next ask opens it afresh.
fn notes_of(
    from: &NotesFrom,
    phrase_id: i64,
    hash: &str,
    open: &mut Option<(PathBuf, MidiStore)>,
) -> Result<Vec<NoteEvent>, String> {
    Ok(match from {
        NotesFrom::Made(made) => made.get(hash).cloned().unwrap_or_default(),
        NotesFrom::Store(path) => {
            if open.as_ref().is_none_or(|(p, _)| p != path) {
                *open = Some((path.clone(), MidiStore::open_read_only(path).map_err(|e| e.to_string())?));
            }
            let read = match open.as_ref() {
                Some((_, store)) => store.notes(phrase_id, hash).map_err(|e| e.to_string()),
                None => Ok(Vec::new()),
            };
            if read.is_err() || open.as_ref().is_some_and(|(_, store)| !store.is_current()) {
                *open = None;
            }
            read?
        }
    })
}

fn read_notes(ask: &NotesAsk, open: &mut Option<(PathBuf, MidiStore)>) -> Result<NotesRead, String> {
    let notes = notes_of(&ask.from, ask.phrase_id, &ask.hash, open)?;
    let fact = if ask.key.is_none() {
        mo_midi::keys::with_reader(|r| mo_midi::keys::read(r, &notes, ask.role, ask.cut)).1
    } else {
        None
    };
    // The chords too (their first 16 bars: quick even for a long part), spelled in the part's key.
    let chords = mo_midi::chart::read(&notes, ask.role, ask.cut);
    let chart = page::chord_chart(chords.as_ref(), page::chart_key(ask.key.as_ref().or(fact.as_ref())));
    Ok(NotesRead { read: notes.len(), roll: page::roll(&notes, ask.bars), fact, chart })
}

/// The card's sound line, with the part's tempo read now ([`tempo_of`]).
fn sound_line_of(ask: &NotesAsk) -> String {
    page::sound_line(&ask.sound, Some(tempo_of(&ask.from, &ask.source, &ask.library)))
}

/// The picked part's tempo has been read: its card says it, unless another part was picked meanwhile.
fn tempo_read(app: &AppWindow, id: u64, line: String) {
    if PAGE.with(|p| p.borrow().notes_asked) == id {
        app.global::<MidiData>().set_sound_line(line.into());
    }
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
    }
}

/// The card's buttons and the line's right-click menu: one list (rule 15: each shows its key).
fn actions(e: &mo_midi::Entry, shortcuts: &Shortcuts) -> (Vec<CardAction>, Vec<MenuItem>) {
    // Into your DAW first, by the drag handle above them; then finding more.
    let mut list: Vec<(&str, &str, bool)> =
        vec![("midi.toBitwig", "Send to Bitwig", true), ("midi.moreLike", "More like this", true)];
    if !e.within.is_empty() {
        list.push(("midi.within", "Show the whole phrase", true));
    }
    // A part from one of your songs: its song on the Songs page (Solomon's note, 11 Nov).
    if e.is_project() {
        list.push(("midi.showInSongs", "Show in Songs", true));
    }
    list.push(("midi.source", "Show in folder", true));
    list.push(("midi.copyPath", "Copy path", true));
    let card = list
        .iter()
        .map(|&(id, text, enabled)| CardAction {
            text: text.into(),
            tip: what_of(id).into(),
            key: shortcuts.key_of(id).into(),
            id: id.into(),
            enabled,
            main: false,
            danger: false,
        })
        .collect();
    // The right-click menu starts with playing it, as Sounds' does: the card has those in its play row.
    let play = [
        ("app.play", "Play or stop the loop", true),
        ("midi.playBitwig", "Play in Bitwig, or stop", true),
        ("midi.playFl", "Play in FL Studio, or stop", true),
    ];
    let menu = play
        .iter()
        .chain(&list)
        .map(|&(id, text, enabled)| MenuItem {
            what: text.into(),
            key: shortcuts.key_of(id).into(),
            id: id.into(),
            enabled,
        })
        .collect();
    (card, menu)
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
        ("midi.moreLike", Some((at, _))) => {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.target = Some(Target::Like(at));
                p.seed = None;
            });
            relist(app);
            true
        }
        ("midi.within", Some((at, _))) => {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.target = Some(Target::Within(at));
                p.seed = None;
            });
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
        ("app.play", _) => toggle_play(app),
        ("midi.playBitwig", _) => toggle_daw(app, Daw::Bitwig),
        ("midi.playFl", _) => toggle_daw(app, Daw::Fl),
        ("midi.copyPath", Some((_, path))) => {
            if let Some(feed) = feed {
                crate::files::copy_path(&feed, &path);
            }
            true
        }
        _ => false,
    }
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

/// What a send to Bitwig needs, taken from the page so it can go on a thread of its own.
struct Sending {
    library: PathBuf,
    from: NotesFrom,
    phrase_id: i64,
    hash: String,
    name: String,
    bars: i32,
}

/// Send to Bitwig: the picked part as a new MIDI file in the export folder's "MIDI browser" folder, then as a clip
/// on a new track in the project open in Bitwig (as the C# MIDI page did). The part's own file is never touched.
/// Its card shows at once and says how it went.
fn send_to_bitwig(at: usize) {
    let Some((sending, feed)) = PAGE.with(|p| {
        let p = p.borrow();
        let e = p.cat.entry(at)?;
        let sending = Sending {
            library: p.library.clone(),
            from: p.notes.clone()?,
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            name: page::title(e),
            bars: e.phrase.bars,
        };
        Some((sending, p.feed.clone()?))
    }) else {
        return;
    };
    let job = to_bitwig::part_card(&feed, &sending.name);
    let started = background::spawn("Send to Bitwig", move || {
        // A panic in it (finding the export folder too) is logged as a crash and fails the card as it goes.
        let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
            let export = export_dir(&sending.library);
            send_part(job, &to_bitwig::Bitwig::default(), &sending, export.as_deref().map_err(String::as_str))
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
/// with how it went. Returns the file written, if it was. Waits for Bitwig, so never on the window's thread.
fn send_part(
    job: mo_app::activity::Job,
    to: &to_bitwig::Bitwig,
    part: &Sending,
    export: Result<&Path, &str>,
) -> Option<PathBuf> {
    let failed = |job: mo_app::activity::Job, why: String| {
        log::warning("MIDI", &format!("Sending {} to Bitwig failed: {why}", part.name));
        job.failed_as(mo_dawlink::bitwig::PART_FAILED, &why);
        None
    };
    let dir = match export {
        Ok(dir) => dir,
        Err(why) => return failed(job, why.to_owned()),
    };
    let notes = match notes_of(&part.from, part.phrase_id, &part.hash, &mut None) {
        Ok(notes) if notes.iter().any(|n| !n.muted) => notes,
        Ok(_) => return failed(job, "It has no notes to send (every note in it is muted).".to_owned()),
        Err(e) => return failed(job, format!("Its notes couldn't be read: {e}.")),
    };
    let clip = page::part_clip(&part.name, &notes, part.bars);
    let bytes = match mo_formats::midi::write::write_clip(&clip) {
        Ok(written) => written.bytes,
        Err(e) => return failed(job, format!("Its MIDI file couldn't be made: {e}.")),
    };
    let file = match page::write_part(dir, &part.name, &bytes) {
        Ok(file) => file,
        Err(e) => return failed(job, format!("Its MIDI file couldn't be saved in {}: {e}.", dir.display())),
    };
    to_bitwig::send_part(job, to, &file.to_string_lossy(), &part.name);
    Some(file)
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
            &[69, 72, 74, 76, 74, 72],
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
    from: NotesFrom,
    phrase_id: i64,
    hash: String,
    bars: i32,
    sound: mo_midi::gm::GmSound,
    /// Its file, whose tempo it plays at, and the song library that has a project's.
    source: Arc<mo_midi::Source>,
    library: PathBuf,
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
            from: p.notes.clone()?,
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            bars: e.phrase.bars,
            sound: page::gm_sound(e),
            source: Arc::clone(&e.phrase.source),
            library: p.library.clone(),
        })
    });
    let Some(ask) = ask else {
        stop_playing(app);
        return;
    };
    PAGE.with(|p| p.borrow_mut().looping = Some(at));
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

/// The part's loop: its notes read, the SoundFont read (once), and the two made into sound.
fn make_loop(ask: &PlayAsk, open: &mut Option<(PathBuf, MidiStore)>) -> Result<mo_audio::AudioData, String> {
    let notes = notes_of(&ask.from, ask.phrase_id, &ask.hash, open)?;
    let (heard, length) = page::loop_notes(&notes, ask.bars);
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
    let notes: Vec<mo_audio::gm::Note> = heard
        .iter()
        .map(|n| mo_audio::gm::Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity7() })
        .collect();
    let part = mo_audio::gm::Part {
        notes: &notes,
        ppq: mo_midi::PPQ,
        length,
        tempo: tempo_of(&ask.from, &ask.source, &ask.library).bpm(),
        program: ask.sound.program,
        drums: ask.sound.drums,
    };
    mo_audio::gm::render_loop(&font, &part)
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
    let Err(e) = mo_dawlink::bitwig::Bitwig::default().stop_playing(&bitwig, turn) else { return };
    log::warning("MIDI", &format!("The part playing in Bitwig couldn't be stopped: {e}"));
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
             Bitwig."
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
        let place = (Arc::clone(&e.phrase.source), p.library.clone());
        Some((p.notes.clone()?, e.phrase.id, e.phrase.hash.clone(), e.phrase.bars, place, page::title(e)))
    });
    let Some((from, phrase_id, hash, bars, (source, library), name)) = ask else {
        return false;
    };
    // The loop with a General MIDI sound would play over it.
    stop_playing(app);
    let asked = DAW_ASKED.load(std::sync::atomic::Ordering::SeqCst);
    PAGE.with(|p| p.borrow_mut().in_daw = Some(phrase_id));
    data.set_daw_starting(daw.id().into());
    if HERE.with(|h| *h.borrow()) {
        data.set_daw_starting("".into());
        data.set_daw_playing(daw.id().into());
        return true;
    }
    if daw == Daw::Bitwig {
        // One thing plays in Bitwig at a time: an audition on the Presets page stops (the review's D-8).
        crate::presets::stop_audition_for_another(app);
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
            let notes = notes_of(&from, phrase_id, &hash, &mut None)
                .map_err(|e| Some(format!("Its notes couldn't be read: {e}.")))?;
            Ok((notes, tempo_of(&from, &source, &library).bpm()))
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
                start_in_daw(daw, &notes, bars, tempo, &name, turn.as_ref(), &wanted)
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
                    &format!("{name} couldn't play in {}: {why}", if daw == Daw::Fl { "FL Studio" } else { "Bitwig" }),
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
    let data = app.global::<MidiData>();
    if data.get_daw_playing() != Daw::Bitwig.id() && data.get_daw_starting() != Daw::Bitwig.id() {
        return;
    }
    stop_daw(app);
    data.set_daw_note("It stopped: an audition on the Presets page is playing in Bitwig now.".into());
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

/// When the app closes: stops a part playing in a DAW, so Bitwig's clip doesn't loop on and no note hangs in FL.
/// Waits a moment at most (Bitwig answers on this computer).
pub fn finish_up() {
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

/// Starts the part in the DAW, and says where it plays. Waits for Bitwig, so never on the window's thread.
///
/// `Err(None)` when it was no longer `wanted` (it put nothing in, or took it back); `Err(Some)` says why it can't play.
fn start_in_daw(
    daw: Daw,
    notes: &[NoteEvent],
    bars: i32,
    tempo: f64,
    name: &str,
    turn: Option<&mo_dawlink::bitwig::Turn<'_>>,
    wanted: &dyn Fn() -> bool,
) -> Result<(InDaw, String), Option<String>> {
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
            let played = mo_dawlink::bitwig::Bitwig::default().play_on_selected(&file, turn, wanted);
            let _ = std::fs::remove_file(&file);
            let playing = played.map_err(|e| (e != mo_dawlink::bitwig::Error::Cancelled).then(|| e.to_string()))?;
            let note = format!("Playing {name} through \u{201c}{}\u{201d} in Bitwig.", playing.track);
            Ok((InDaw::Bitwig(playing), note))
        }
    }
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
    let Some((from, phrase_id, hash, name, bars, colour)) = PAGE.with(|p| {
        let p = p.borrow();
        let e = p.cat.entry(at)?;
        Some((p.notes.clone()?, e.phrase.id, e.phrase.hash.clone(), page::title(e), e.phrase.bars, colour(e.role())))
    }) else {
        return;
    };
    let tint = tint(app, colour);
    let under_way = crate::drag::start(
        app,
        crate::drag::Drag {
            make: Box::new(move || {
                let notes = notes_of(&from, phrase_id, &hash, &mut None).map_err(|e| {
                    log::warning("MIDI", &format!("A dragged part's notes couldn't be read: {e}"));
                    format!("Its notes couldn't be read: {e}.")
                })?;
                mo_app::drag_out::from_part(&name, &notes, bars, tint)?
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
    let (from, phrase_id, hash, name, bars, colour) = PAGE.with(|p| {
        let p = p.borrow();
        let e = p.cat.entry(p.shown.get(row)?.entry)?;
        Some((p.notes.clone()?, e.phrase.id, e.phrase.hash.clone(), page::title(e), e.phrase.bars, colour(e.role())))
    })?;
    let notes = notes_of(&from, phrase_id, &hash, &mut None).ok()?;
    let out = mo_app::drag_out::from_part(&name, &notes, bars, tint(app, colour)).ok()?;
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

    #[test]
    fn notes_are_read_and_keyed_for_the_card() {
        let s = sample();
        let e = &s.cat.entries[0];
        let ask = NotesAsk {
            id: 1,
            from: s.notes.clone(),
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            role: e.role(),
            cut: false,
            bars: e.phrase.bars,
            key: None,
            source: Arc::clone(&e.phrase.source),
            library: PathBuf::new(),
            sound: page::gm_sound(e),
        };
        let Ok(read) = read_notes(&ask, &mut None) else { panic!("the made-up notes should read") };
        // Made up, so its tempo is the sample's own, with no file opened for it.
        let line = sound_line_of(&ask);
        assert!(line.ends_with(" BPM") || line.contains("BPM, as it gives no tempo"), "{line}");
        assert_eq!(read.roll.notes.len(), 12);
        assert!(read.fact.is_some());
        assert!(!read.chart.bars.is_empty(), "its chords are read too");
    }

    #[test]
    fn a_part_goes_to_bitwig_as_a_new_midi_file_on_a_new_track() {
        let s = sample();
        let e = &s.cat.entries[0];
        let part = Sending {
            library: PathBuf::new(),
            from: s.notes.clone(),
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            name: page::title(e),
            bars: e.phrase.bars,
        };
        let dir = std::env::temp_dir().join(format!("mo-ui-midi-to-bitwig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bitwig = mo_dawlink::bitwig::fake::FakeBitwig::start().unwrap();
        let feed = mo_app::activity::Feed::default();
        let file = send_part(feed.start("Sending", ""), &bitwig.bitwig(), &part, Ok(dir.as_path())).unwrap();
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

        // An export folder that isn't there: nothing is written or sent, and the card says why.
        let feed = mo_app::activity::Feed::default();
        let gone = "The export folder isn't there.";
        assert!(send_part(feed.start("Sending", ""), &bitwig.bitwig(), &part, Err(gone)).is_none());
        assert_eq!(bitwig.world().commands(), ["newtrack", "clip"]);
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
    fn a_missing_index_is_an_empty_list_not_an_error() {
        let dir = std::env::temp_dir().join("mo-ui-midi-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let Ok(l) = load(&dir.join("library.db")) else { panic!("no index should read as empty") };
        assert!(l.cat.entries.is_empty());
    }
}
