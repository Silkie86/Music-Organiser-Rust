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
use slint::{ComponentHandle, Model, ModelNotify, ModelRc, ModelTracker, VecModel};

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
    seed: Option<u64>,
    target: Option<Target>,
    shown: Arc<Vec<Hit>>,
    years: Vec<(i32, usize)>,
    /// How many parts each filter chip's choices would show, from the last list worked out.
    counts: page::ChoiceCounts,
    /// The part playing as a loop (or being made into one), as its place in the catalogue's entries.
    looping: Option<usize>,
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
}

struct Ask {
    id: u64,
    cat: Arc<Catalogue>,
    filters: Filters,
    order: Order,
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
    read_key: bool,
}

struct NotesRead {
    /// How many notes the index gave (the roll leaves out muted ones).
    read: usize,
    roll: Roll,
    fact: Option<KeyFact>,
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
            date: page::date_text(e.phrase.source.mtime).into(),
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
            let Ok(loaded) = load(&library) else { return };
            let _ = slint::invoke_from_event_loop(move || {
                // The page's own read, if it has started since, hands over its list itself.
                if let Some(app) = weak.upgrade()
                    && !PAGE.with(|p| p.borrow().started)
                {
                    crate::finder::add_source(&app, "midi", loaded.found);
                }
            });
        });
        if let Err(e) = started {
            log::error("MIDI", &format!("Couldn't start reading the index for Ctrl+K: {e}"));
        }
    });
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
        p.shortcuts = shortcuts.clone();
    });
    wire(app);
    crate::piano_roll::connect(app);
    let data = app.global::<MidiData>();
    data.set_play_key(shortcuts.key_of("app.play").into());
    data.set_empty_title("Reading your MIDI…".into());
    data.set_empty_text("".into());
    if app.get_place() == mo_app::places::Place::Midi.index() as i32 {
        shown(app);
    }
}

fn wire(app: &AppWindow) {
    let data = app.global::<MidiData>();
    data.set_lines(ModelRc::from(LINES.with(Rc::clone) as Rc<dyn Model<Data = MidiLine>>));
    data.set_orders(strings(Order::ALL.map(Order::name)));
    data.set_order(Order::default().name().into());
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
        let data = app.global::<MidiData>();
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            if what == "order" {
                p.order = Order::from_name(&value);
                p.seed = None;
            }
        });
        if what == "order" {
            data.set_order(value);
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
    let (picked, read_now) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // The picked part stays picked when the index is read again, found by its notes.
        let hash = p.picked.and_then(|at| p.cat.entry(at)).map(|e| e.phrase.hash.clone());
        p.years = loaded.years;
        // The old index goes on the freeing thread: tens of thousands of parts take a while to let go.
        background::free_later(std::mem::replace(&mut p.cat, Arc::new(loaded.cat)));
        p.notes = Some(loaded.notes);
        p.picked = hash.and_then(|h| p.cat.find(&h));
        p.picked_row = None;
        p.target = None;
        // The old list points into the old index: nothing shows until the new one is worked out.
        background::free_later(std::mem::replace(&mut p.shown, Arc::new(Vec::new())));
        let read_now = !p.read_once && !HERE.with(|h| *h.borrow());
        p.read_once = true;
        (p.picked, read_now)
    });
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
            target: p.target,
            seed: p.seed,
        }
    });
    app.global::<MidiData>().set_any_filter(PAGE.with(|p| p.borrow().filters.any()));
    if HERE.with(|h| *h.borrow()) {
        let (hits, counts) = page::view(&ask.cat, &ask.filters, ask.order, ask.target, ask.seed);
        listed(app, ask.id, hits, counts);
        return;
    }
    WORKER.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("MIDI list", move |ask: Ask| {
                let (hits, counts) =
                    background::on_pool(|| page::view(&ask.cat, &ask.filters, ask.order, ask.target, ask.seed));
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
        (Some(shown), page::result_text(shown, all, p.order, p.target, p.seed.is_some()), target_text)
    });
    let Some(shown) = shown else { return };
    show_chips(app);
    LINES.with(|l| l.notify.reset());
    let data = app.global::<MidiData>();
    data.set_shown(shown as i32);
    data.set_result(result.into());
    data.set_target(target.into());
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
        // The loop's Stop is on the card: closing it stops the loop too.
        if data.get_playing() {
            stop_playing(app);
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
        let facts: Vec<MidiFact> =
            page::facts(e).into_iter().map(|(n, v)| MidiFact { name: n.into(), value: v.into() }).collect();
        let (actions, menu) = actions(e, &shortcuts);
        data.set_picked(card);
        data.set_facts(ModelRc::new(VecModel::from(facts)));
        data.set_actions(ModelRc::new(VecModel::from(actions)));
        data.set_menu(ModelRc::new(VecModel::from(menu)));
        data.set_sound_line(page::sound_line(&page::gm_sound(e), page::play_tempo(e)).into());
        crate::piano_roll::show(app, None);
        Some(NotesAsk {
            id,
            from: notes,
            phrase_id: e.phrase.id,
            hash: e.phrase.hash.clone(),
            role: e.role(),
            cut: page::is_cut(&e.phrase.clip),
            bars: e.phrase.bars,
            read_key: e.phrase.key_fact.is_none(),
        })
    });
    data.set_panel_shown(true);
    show_soundfont(app);
    // While a loop plays, picking another part plays that one instead (newest wins); the same part again (a click on
    // its line, or the list read again) goes on as it is.
    if data.get_playing() && PAGE.with(|p| p.borrow().looping) != Some(at) {
        play(app, at);
    }
    let Some(ask) = ask else { return };
    if HERE.with(|h| *h.borrow()) {
        let id = ask.id;
        let read = read_notes(&ask, &mut None);
        notes_read(app, id, read);
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
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        notes_read(&app, id, read);
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
    let fact = if ask.read_key {
        mo_midi::keys::with_reader(|r| mo_midi::keys::read(r, &notes, ask.role, ask.cut)).1
    } else {
        None
    };
    Ok(NotesRead { read: notes.len(), roll: page::roll(&notes, ask.bars), fact })
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
        others: strings(others),
        others_more: if more > 0 { format!("and {} more", thousands(more)).into() } else { "".into() },
    }
}

/// The card's buttons and the line's right-click menu: one list (rule 15: each shows its key).
fn actions(e: &mo_midi::Entry, shortcuts: &Shortcuts) -> (Vec<CardAction>, Vec<MenuItem>) {
    let mut list: Vec<(&str, &str, bool)> =
        vec![("midi.moreLike", "More like this", true), ("midi.toBitwig", "Send to Bitwig", true)];
    if !e.within.is_empty() {
        list.push(("midi.within", "Show the whole phrase", true));
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
    let menu = list
        .iter()
        .map(|&(id, text, enabled)| MenuItem {
            what: text.into(),
            key: shortcuts.key_of(id).into(),
            id: id.into(),
            enabled,
        })
        .collect();
    (card, menu)
}

/// Runs one of the page's actions on the picked part; false when there's nothing for it to do.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let (picked, feed) = PAGE.with(|p| {
        let p = p.borrow();
        (p.picked.and_then(|at| p.cat.entry(at).map(|e| (at, e.phrase.source.path.clone()))), p.feed.clone())
    });
    match (id, picked) {
        ("midi.back", _) => {
            let had = PAGE.with(|p| p.borrow_mut().target.take().is_some());
            if had {
                relist(app);
            }
            had
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
        ("midi.copyPath", Some((_, path))) => {
            if let Some(feed) = feed {
                crate::files::copy_path(&feed, &path);
            }
            true
        }
        _ => false,
    }
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
            tempo: None,
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
    tempo: f64,
}

thread_local! {
    static PLAYS: RefCell<Option<Newest<PlayAsk>>> = const { RefCell::new(None) };
}

thread_local! {
    /// Where the SoundFont is, looked for once (a look at the disk, so not on every pick) and again when one is chosen.
    static SOUNDFONT: RefCell<Option<Option<PathBuf>>> = const { RefCell::new(None) };
}

/// The SoundFont parts play with now: the one picked in the app's settings, else GeneralUser GS in the app's folder.
fn soundfont() -> Option<PathBuf> {
    if let Some(known) = SOUNDFONT.with(|s| s.borrow().clone()) {
        return known;
    }
    let folder = PAGE.with(|p| p.borrow().library.parent().map(Path::to_path_buf))?;
    let found = page::soundfont_path(crate::settings::value(page::SOUNDFONT_KEY).as_deref(), &folder);
    SOUNDFONT.with(|s| *s.borrow_mut() = Some(found.clone()));
    found
}

/// Whether the MIDI page's loop is what the player is playing.
fn loop_playing() -> bool {
    crate::sounds::player().playing().is_some_and(|k| k.starts_with(PLAY_KEY))
}

/// Space and the Play button: stops the loop if it's playing, else plays the picked part as one. False when there's
/// nothing to play or stop.
pub fn toggle_play(app: &AppWindow) -> bool {
    let playing = app.global::<MidiData>().get_playing();
    if playing {
        stop_playing(app);
        return true;
    }
    match PAGE.with(|p| p.borrow().picked) {
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

/// Whether there's a SoundFont to play with, and if not what to do.
fn show_soundfont(app: &AppWindow) {
    let data = app.global::<MidiData>();
    let missing = soundfont().is_none();
    data.set_soundfont_missing(missing);
    if missing {
        let folder = PAGE.with(|p| p.borrow().library.parent().map(Path::to_path_buf)).unwrap_or_default();
        let usual = folder.join(page::SOUNDFONT_FOLDER).join(page::SOUNDFONT_FILE);
        data.set_soundfont_note(
            format!(
                "To hear parts, choose a General MIDI SoundFont (.sf2). GeneralUser GS, the old app's, goes in {}",
                usual.display()
            )
            .into(),
        );
    }
}

/// Makes the part at `at` into a loop with its matched sound and plays it round and round, off the window's thread.
fn play(app: &AppWindow, at: usize) {
    let data = app.global::<MidiData>();
    let Some(font) = soundfont() else {
        stop_playing(app);
        data.set_soundfont_missing(true);
        data.set_play_note("Choose a SoundFont first.".into());
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
            tempo: page::play_tempo(e),
        })
    });
    let Some(ask) = ask else {
        stop_playing(app);
        return;
    };
    PAGE.with(|p| p.borrow_mut().looping = Some(at));
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
        tempo: ask.tempo,
        program: ask.sound.program,
        drums: ask.sound.drums,
    };
    mo_audio::gm::render_loop(&font, &part)
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
            read_key: true,
        };
        let Ok(read) = read_notes(&ask, &mut None) else { panic!("the made-up notes should read") };
        assert_eq!(read.roll.notes.len(), 12);
        assert!(read.fact.is_some());
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
