//! The Space page: the drives your projects, samples, backup and the app's own files are on, how full each is, what
//! the app's own files take (part 1), Bitwig's copies of Library multisamples in your projects (part 2), with Fix for
//! the saves you tick, the old copies moving plugin folders to another drive left (part 3) and the spare plugin copies
//! (part 4), each with Send to the Recycle Bin for the ones you tick, what compacting your songs and thinning their
//! older saves could free (part 5), with the same for the old autosaves and caches you tick, the plugins no song has
//! used lately (part 6), the copies of the same sound in your sample folders (information only), and the Recycle Bin,
//! with Put back for what the app put there. mo_app's `space`, `space_bitwig`, `space_old`, `space_spare`, `space_songs`, `space_plugins`,
//! `space_sounds`, `space_removing` and `space_bin` decide every word. It looks the first time the page shows, on
//! low-priority threads, and again when it shows after five minutes or on Measure again, so nothing on the window's
//! thread waits for a disk (rule 12). Fix only writes new "(fixed)" projects beside the saves you ticked; Send sends
//! the old copies, spare copies or songs' old autosaves and caches you ticked to the Recycle Bin, each checked again
//! first, and Put back returns them (`mo_app::binned` keeps what went, so Put back works after a restart too). The
//! library and the sound library are only read.

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::binned::{self, Kept};
use mo_app::places::Place;
use mo_app::plugins::folder_fixes::FolderFix;
use mo_app::shortcuts::what_of;
use mo_app::sounds::{CopiesFound, SoundList};
use mo_app::space::{self, Drive, Holds, OnPlugins, Opens, Part};
use mo_app::space_bin::{self, BIN, CAN_PUT_BACK, PutBacks};
use mo_app::space_bitwig::{self, BITWIG_COPIES, Copies, Fixes, Fixing, ToFix};
use mo_app::space_old::{self, Found, OLD_COPIES};
use mo_app::space_plugins::{self, UNUSED_PLUGINS};
use mo_app::space_removing::{self, Removable, Tally};
use mo_app::space_songs::{self, COMPACT, FromLibrary, SongItem};
use mo_app::space_sounds::{self, SOUND_COPIES};
use mo_app::space_spare::{self, Removing, SPARE_COPIES, Stage};
use mo_app::{backup, log, paths, shell};
use mo_catch::catch_crash;
use mo_library::folders::FolderList;
use mo_space::app_files::{self, AppFilesPart};
use mo_space::bitwig::{self, LibraryIndex};
use mo_space::clean::{self, Bin, Done, Heard, Problem, Removed, SystemBin, What};
use mo_space::old_copies::Leftover;
use mo_space::prints::Prints;
use mo_space::song::CompactSettings;
use mo_space::spare::{SpareCopy, Used};
use mo_space::{plugin_space, plugins};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::plugins as plugins_page;
use crate::sounds::{self as sounds_page, Known};
use crate::{AppWindow, CardAction, CardPart, DriveCard, MenuItem, RoomLine, SpaceData};

/// How long what the page shows stays fresh: shown again after this, it looks again (the C# app looked every five
/// minutes).
const FRESH: Duration = Duration::from_secs(5 * 60);

/// How often a look for Bitwig copies tells the window how far it is, at most (rule 12).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// The buttons' ids, from the one shortcut list (rule 15): Measure again, Show in folder, Fix, Remove, Stop and Put
/// back are the page's own (`mo_app::shortcuts::SPACE_ACTIONS`); the Recycle Bin is the app-wide action, so its button
/// shows the same key.
const MEASURE: &str = "space.measure";
const REVEAL: &str = "space.reveal";
const FIX: &str = "space.fix";
const REMOVE: &str = "space.remove";
const STOP: &str = "space.stop";
const PUT_BACK: &str = "space.putBack";
const RECYCLE_BIN: &str = "app.recycleBin";

/// The name the library knows Bitwig by ("Bitwig Studio"), for the program picked for it in Settings.
const BITWIG: &str = mo_formats::bitwig::reader::DAW;

/// One look at the drives and the app's files.
struct Ask {
    id: u64,
    library: PathBuf,
    app_folder: PathBuf,
    stop: Arc<AtomicBool>,
}

/// One look for Bitwig copies. It reads every copy, far more than a look at the drives, so it has a thread of its own
/// and is only asked for on the page's first look, on Measure again, and while no look has finished: never on the
/// five-minute look.
struct BitwigAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
    feed: Feed,
}

/// One look for old copies. It measures each one, which can take a while for a big sound library, so it has a thread
/// of its own. It's asked for with the page's own looks (the first, the five-minute one and Measure again), unless one
/// is still on.
struct OldAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
}

/// One look for spare plugin copies. It reads every plugin file and measures the spare ones, so it has a thread of its
/// own, and like the look for Bitwig copies it's only asked for on the page's first look, on Measure again, and while
/// no look has finished: plugins seldom change in five minutes.
struct SpareAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
}

/// One look at your songs. It measures every song's folder and compares its saves of the same size, so it has a thread
/// of its own, and like the look for Bitwig copies it's only asked for on the page's first look, on Measure again, and
/// while no look has finished; never while one is on, as nothing it measured would be kept for the next.
struct SongsAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
}

/// One look at the plugins no song has used lately: it reads every plugin your songs use from the library, as the
/// Plugins page does, so it has a thread of its own. It's asked for on the page's first look, once what it shows is
/// five minutes old, on Measure again, and when your say on a plugin's folders on the Plugins page has changed
/// (`fixes`, [`plugins_page::folder_fixes_this_session`]); it reads the library again only when the library has
/// changed since the look on show (`stamp`, [`mo_library::list_copy::stamp_of`]) or the fixes have. What the app
/// removes since is taken off as it shows.
struct PluginsAsk {
    id: u64,
    library: PathBuf,
    stamp: Option<String>,
    fixes: Vec<FolderFix>,
}

/// What the plugins line is worked out from, off the window's thread ([`work_out_plugins`]): what the look on show
/// found and when it counts back from, what's in the Recycle Bin from here (by which change to it the page has: none
/// while the page hasn't looked in the bin this run), what the Plugins page's Clean ups, uninstalls and moves took this
/// session, and the years the Plugins page has picked.
#[derive(Clone)]
struct PluginsOf {
    found: Arc<space_plugins::Found>,
    at: chrono::NaiveDateTime,
    kept: Option<u64>,
    session: Vec<String>,
    years: u32,
}

impl PartialEq for PluginsOf {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.found, &other.found)
            && (self.at, self.kept, self.years) == (other.at, other.kept, other.years)
            && self.session == other.session
    }
}

/// What [`work_out_plugins`] gives its thread: which time it asked, what it's worked out from, and the plugin files in
/// the Recycle Bin from here (none while the page hasn't looked in the bin: [`recorded_plugin_files`]).
type PluginsWork = (u64, PluginsOf, Option<Vec<String>>);

/// One look at the copies of the same sound: the copies the Sounds page's Hide duplicates found in its list of the
/// sounds (`list`, with your songs' use once it's known), grouped as that page keeps them, with the switch on or off
/// (`hiding`), on the page's own low-priority thread. Asked for on the page's first look, once what it shows is five
/// minutes old, on Measure again, when the Sounds page's list changes (as the copies come), and when the switch does.
struct CopiesAsk {
    id: u64,
    list: Arc<SoundList>,
    hiding: bool,
    stop: Arc<AtomicBool>,
}

/// What a look at the plugins no song has used lately brings back.
enum PluginsLooked {
    /// What it found, the library's stamp when it read it, and the folder fixes it counted.
    Found(space_plugins::Found, Option<String>, Vec<FolderFix>),
    /// The library is as the look on show found it: that stands, counted back from now.
    Same(chrono::NaiveDateTime),
}

/// What a look brings back, a step at a time: the drives first (quick), then the app's files once they're measured;
/// from the look for Bitwig copies, how far it is, then what it found; and what the looks for old copies and spare
/// plugin copies found, and what the look at your songs found. A look that crashed (the crash is logged) says so, so
/// the page doesn't wait for it for ever; so do the looks for spare copies and at your songs that couldn't read the
/// library (said in the log). Fix brings back how each save
/// went, then that it has finished, always, even when it crashed; so do Remove and Put back, once each is done. What
/// the app put in the Recycle Bin that's still there comes after a look, a removal or a put back.
enum Step {
    Drives(u64, Vec<Drive>),
    AppFiles(u64, Vec<AppFilesPart>),
    Crashed(u64),
    BitwigLooking(u64, (usize, usize)),
    Bitwig(u64, Copies),
    BitwigCrashed(u64),
    Old(u64, Found),
    OldCrashed(u64),
    Spare(u64, space_spare::Found),
    SpareNone(u64),
    Songs(u64, Box<space_songs::Shown>),
    SongsNone(u64),
    Plugins(u64, Box<PluginsLooked>),
    PluginsNone(u64),
    PluginsNow(u64, Box<space_plugins::Found>),
    Copies(u64, Box<space_sounds::Found>),
    CopiesNone(u64),
    Fixed(PathBuf, Fixing),
    FixDone,
    Kept(Kept),
    /// Remove has finished: when it ran (as the list of what the app put in the Recycle Bin keeps it), what went and
    /// what stayed, and that list with it now (none when keeping it stopped with an error, which is logged).
    SpareSent(i64, Done, Option<Kept>),
    /// The same, for old copies.
    OldSent(i64, Done, Option<Kept>),
    /// The same, for your songs' old autosaves and caches.
    SongsSent(i64, Done, Option<Kept>),
    /// A put back has finished: which clean-up (its kind and when it ran), what couldn't come back, and what the app put
    /// in the bin that's still there, looked at once it was done (none when that stopped with an error).
    PutBack(String, i64, Vec<Problem>, Option<Kept>),
}

/// One of the page's looks: its low-priority thread (started the first time a look is asked for), how many looks were
/// asked for, so an answer to an older one that arrives late is dropped, and the stop of the one still on.
struct Looks<J: Send + 'static> {
    worker: Option<Newest<J>>,
    asked: u64,
    stop: Arc<AtomicBool>,
}

impl<J: Send + 'static> Default for Looks<J> {
    fn default() -> Self {
        Looks { worker: None, asked: 0, stop: Arc::default() }
    }
}

impl<J: Send + 'static> Looks<J> {
    /// A new look's number and stop. The one still on stops: the newest wins (rule 12).
    fn next(&mut self) -> (u64, Arc<AtomicBool>) {
        self.stop.store(true, Ordering::Relaxed);
        self.stop = Arc::new(AtomicBool::new(false));
        self.asked += 1;
        (self.asked, self.stop.clone())
    }

    /// Gives `job` to the thread, which is started called `name`, doing `work`, the first time.
    fn ask(&mut self, name: &str, work: impl FnMut(J) + Send + 'static, job: J) -> std::io::Result<()> {
        if self.worker.is_none() {
            self.worker = Some(Newest::start(name, work)?);
        }
        if let Some(worker) = &self.worker {
            worker.ask(job);
        }
        Ok(())
    }
}

#[derive(Default)]
struct Page {
    library: PathBuf,
    /// Set once the page is wired; it looks at nothing before.
    feed: Option<Feed>,
    /// The look at the drives and the app's files.
    look: Looks<Ask>,
    /// When the newest look was asked for: none before the page first shows.
    looked: Option<Instant>,
    /// Whether a look is still measuring the app's files: Measure again waits for it.
    measuring: bool,
    drives: Vec<Drive>,
    /// The app's own files, once measured. A new look keeps the last sizes on show until its own are in.
    parts: Option<Vec<AppFilesPart>>,
    /// The look for Bitwig copies. Measure again doesn't wait for it: it starts it again, and as every fingerprint
    /// it read is kept, the new one goes quickly over what the last one read.
    bitwig_look: Looks<BitwigAsk>,
    /// What the last look for Bitwig copies found; it stays on show while the next one looks.
    bitwig: Option<Copies>,
    /// While a look for Bitwig copies is on: the project folders looked through, of how many (0 while they're still
    /// being found).
    bitwig_looking: Option<(usize, usize)>,
    /// Something arrived while the page was off show, so it wasn't drawn: it is when the page comes back ([`shown`]).
    redraw: bool,
    /// What Fix did this time, the saves you ticked or unticked, and the projects whose other saves are on show.
    fixes: Fixes,
    /// The look for old copies, what the last one found (it stays on show while the next one looks), and whether one
    /// is on; and how many of the Plugins page's moves had finished when the newest one was asked for: once another
    /// has, it looks again ([`plugin_moved`]).
    old_look: Looks<OldAsk>,
    old: Option<Found>,
    old_looking: bool,
    old_moves: u64,
    /// Remove for old copies: the ticks you changed, and how a removal went.
    old_removing: space_old::Removing,
    /// While old copies are going to the Recycle Bin: what stops their checks (Stop).
    old_stop: Option<Arc<AtomicBool>>,
    /// The look for spare plugin copies, what the last one found (it stays on show while the next one looks), and
    /// whether one is on.
    spare_look: Looks<SpareAsk>,
    spare: Option<space_spare::Found>,
    spare_looking: bool,
    /// Remove for spare copies: the ones you unticked, and how a removal went.
    removing: Removing,
    /// The look at your songs, what the last one found as the page shows it (it stays on show while the next one
    /// looks), and whether one is on.
    songs_look: Looks<SongsAsk>,
    songs: Option<space_songs::Shown>,
    songs_looking: bool,
    /// Remove for your songs' old autosaves and caches: the ticks you changed, and how a removal went.
    songs_removing: space_songs::Removing,
    /// While they're going to the Recycle Bin: what stops their checks (Stop).
    songs_stop: Option<Arc<AtomicBool>>,
    /// The look at the plugins no song has used lately, what the last one found (it stays on show while the next one
    /// looks) with when it counts back from (a later look that found the library as it was moves that on), and
    /// whether one is on.
    plugins_look: Looks<PluginsAsk>,
    plugins: Option<(Arc<space_plugins::Found>, chrono::NaiveDateTime)>,
    plugins_looking: bool,
    /// The library's stamp when the look on show read it, and the Plugins page's folder fixes it counted: a look
    /// finding the same of both doesn't read it again.
    plugins_stamp: Option<String>,
    plugins_fixes: Vec<FolderFix>,
    /// Your plugins were measured again since the last look at them ([`plugins_measured`]): the next show looks again.
    plugins_measured: bool,
    /// What the plugins line shows, as last worked out off the window's thread ([`work_out_plugins`]), what it was
    /// worked out from (it's worked out again once that changes: a look, a Clean up or uninstall on the Plugins page,
    /// the years picked there, or what's in the Recycle Bin from here), and the thread that works it out.
    plugins_shown: Option<space_plugins::Found>,
    plugins_of: Option<PluginsOf>,
    plugins_work: Looks<PluginsWork>,
    /// The look at the copies of the same sound, what the last one found (it stays on show while the next one looks),
    /// and whether one is on; the Sounds page's list the newest look was asked for (held weakly: the list itself is
    /// that page's to let go; none once that look failed), and whether Hide duplicates was on for it.
    copies_look: Looks<CopiesAsk>,
    copies: Option<space_sounds::Found>,
    copies_looking: bool,
    copies_of: Weak<SoundList>,
    copies_hiding: bool,
    /// What the app's clean-ups put in the Recycle Bin that's still there: none until the bin has been looked in. Of
    /// two that arrive, the later change wins ([`Kept::seq`]).
    kept: Option<Kept>,
    /// Put back, from the Recycle Bin card or the spare copies card: the one on, and what couldn't come back.
    put_backs: PutBacks,
    /// The looks that stopped when something started going to the Recycle Bin or coming back ([`hold_looks`]), or that
    /// Measure again asked for meanwhile: each looks again once that's done.
    look_after: LookAfter,
    /// The picked line's id, or "" when none is.
    picked: String,
    /// The parts on the card on show, for its ticks and links (they say which part by its place). The card's rows
    /// are only made again when these change.
    card_parts: Vec<Part>,
}

/// Which looks start again once something has gone to the Recycle Bin or come back.
#[derive(Clone, Copy, Debug, Default)]
struct LookAfter {
    old: bool,
    spare: bool,
    songs: bool,
}

/// What a step that arrived changes on show.
enum Changed {
    /// The drives, the lists and the card.
    All,
    /// Biggest wins first and the card: what a look for Bitwig copies found, or how Fix is doing.
    Wins,
    /// How far a look for Bitwig copies is, while nothing it found is on show yet: only its own line, and its card
    /// when that's open ([`show_bitwig_looking`]).
    BitwigLooking,
    /// Fix has finished: the page looks for copies again, to see what Fix wrote as it is on disk.
    LookAgain,
    /// Also taking room and the card: what the app put in the Recycle Bin that's still there.
    Also,
    /// Biggest wins first, Also taking room and the card: a removal or a put back has finished, with what's in the
    /// Recycle Bin now. The page looks for spare copies, old copies or at your songs again (`spare`, `old`, `songs`):
    /// after a put back of them, and when a look of them stopped for it ([`hold_looks`]) or Measure again asked for one
    /// meanwhile.
    Bin { spare: bool, old: bool, songs: bool },
    /// Nothing: an older look's step, or how far a later look for Bitwig copies is while the last one's stay on show.
    Nothing,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::new(Page::default());
}

/// Wires the page. It reads nothing until it first shows ([`shown`]): the library at `library` (read-only) for your
/// project and sample folders, the program picked for Bitwig and the backup's settings for its drive. Opening a folder
/// or the Recycle Bin reports on `feed`'s cards if it can't, and the look for Bitwig copies and Fix show how far they
/// are there. The buttons' tooltips take their keys from the app's one list as it is now (`crate::shortcuts`).
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
    });
    let data = app.global::<SpaceData>();
    let weak = app.as_weak();
    data.on_pick(move |id| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().picked = id.to_string());
        show_card(&app);
    });
    let weak = app.as_weak();
    data.on_close_card(move || {
        let Some(app) = weak.upgrade() else { return };
        close_card(&app);
    });
    let weak = app.as_weak();
    data.on_chosen(move |id| {
        let Some(app) = weak.upgrade() else { return };
        chosen(&app, &id);
    });
    let weak = app.as_weak();
    data.on_tick_part(move |at, ticked| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            // A save's tick, an old copy's, a spare copy's, or a format's for all its copies that can go; or on the songs
            // card, a kind's for all of its, a song's for its own, or the songs' past the first few for theirs.
            if let Some((paths, _)) = part_at(&p, at).and_then(|part| part.tick.clone()) {
                let p = &mut *p;
                match p.picked.as_str() {
                    SPARE_COPIES => p.removing.tick(&paths, ticked),
                    OLD_COPIES => p.old_removing.tick(&paths, ticked),
                    COMPACT => space_songs::tick(&mut p.songs_removing, p.songs.as_ref(), &paths, ticked),
                    _ => {
                        for save in paths {
                            p.fixes.ticks.insert(save, ticked);
                        }
                    }
                }
            }
            // The tick box ticked itself, which undoes its tie to the card's rows: made again, they tie it again.
            p.card_parts.clear();
        });
        show_card(&app);
    });
    // A fixed project's link opens it in Bitwig; an old copy's, a spare copy's or a sound's file shows it in its
    // folder; a format's opens the folder its spare copies are all in; a clean-up's on the Recycle Bin card puts it
    // back; a plugin's and a sound's show it on its page.
    let weak = app.as_weak();
    data.on_open_part(move |at| {
        let (open, put_back, on_plugins, sound, feed) = PAGE.with(|p| {
            let p = p.borrow();
            let part = part_at(&p, at);
            (
                part.and_then(|part| part.open.clone()),
                part.and_then(|part| part.put_back.clone()),
                part.and_then(|part| part.plugins.clone()),
                part.and_then(|part| part.sound),
                p.feed.clone(),
            )
        });
        if let (Some((kind, when)), Some(feed), Some(app)) = (put_back, feed.clone(), weak.upgrade()) {
            put_back_kept(&app, feed, &kind, when);
            return;
        }
        // A plugin's link shows its card on the Plugins page; the line adding up the rest shows them all there.
        if let (Some(on), Some(app)) = (on_plugins, weak.upgrade()) {
            crate::go_to(&app, Place::Plugins.index() as i32);
            match on {
                OnPlugins::Plugin { key, own, name } => plugins_page::show_plugin(&app, &key, &own, &name),
                OnPlugins::Unused => plugins_page::show_unused(&app),
            }
            return;
        }
        // A sound's link shows it on the Sounds page.
        if let (Some(id), Some(app)) = (sound, weak.upgrade()) {
            // Gone from that page since the copies were counted: they're counted again, so the card shows them as
            // they are now.
            if !sounds_page::show_sound_by_id(&app, id) {
                log::info("Space", "A sound on the copies card isn't on the Sounds page any more: counting again");
                PAGE.with(|p| p.borrow_mut().copies_of = Weak::new());
                look_for_sound_copies(&app, sounds_page::known());
            }
            return;
        }
        match (open, feed) {
            (Some((Opens::InBitwig, file)), _) => open_in_bitwig(file),
            (Some((Opens::InFolder, path)), Some(feed)) => {
                let path = path.to_string_lossy();
                crate::files::reveal(&feed, &path, shell::folder_of(&path));
            }
            (Some((Opens::Folder, folder)), Some(feed)) => crate::files::open_folder(&feed, &folder.to_string_lossy()),
            _ => {}
        }
    });
    let weak = app.as_weak();
    data.on_more_part(move |at| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            if let Some((_, folder)) = part_at(&p, at).and_then(|part| part.more.clone())
                && !p.fixes.shown.remove(&folder)
            {
                p.fixes.shown.insert(folder);
            }
        });
        show_card(&app);
    });
    show(app);
    // The app may start on this page, before it was wired.
    if app.get_place() == Place::Space.index() as i32 {
        shown(app);
    }
}

/// The page came on show: it looks the first time, and again once what it shows is five minutes old, old copies too,
/// so the reminder of them keeps up with what's there; old copies also once a move on the Plugins page has finished
/// since the last look for them. It looks for Bitwig copies, spare plugin copies and at your songs the first time, and
/// again only while no look at them has finished: they read every project, every plugin file and every song's folder.
/// None of those three is looked for while what it finds is going to the Recycle Bin or coming back ([`held`]).
pub fn shown(app: &AppWindow) {
    let sounds = sounds_page::known();
    let moves = plugins_page::moves_this_session();
    let (stale, bitwig, old, spare, songs, plugins, copies) = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let wired = p.feed.is_some();
        let stale = wired && p.looked.is_none_or(|at| at.elapsed() >= FRESH);
        let bitwig = wired && p.bitwig.is_none() && p.bitwig_looking.is_none();
        let held = held(&p);
        let spare = wired && p.spare.is_none() && !p.spare_looking && !held.spare;
        // While old copies are going or coming back, a move's look waits until that's done.
        p.look_after.old |= wired && held.old && moves != p.old_moves;
        let old = wired && old_due(&p, moves, stale, held.old);
        let songs = wired && p.songs.is_none() && !p.songs_looking && !held.songs;
        let fixed = p.plugins.is_some() && plugins_page::folder_fixes_this_session() != p.plugins_fixes;
        let plugins = wired && !p.plugins_looking && (stale || p.plugins.is_none() || fixed || p.plugins_measured);
        let copies = wired && copies_due(&p, &sounds, stale);
        (stale, bitwig, old, spare, songs, plugins, copies)
    });
    if stale {
        look_again(app);
    }
    if bitwig {
        look_for_copies(app);
    }
    if old {
        look_for_old(app);
    }
    if spare {
        look_for_spare(app);
    }
    if songs {
        look_for_songs(app);
    }
    if plugins {
        look_for_plugins(app);
    } else {
        // A Clean up or uninstall on the Plugins page, or the years picked there, show here once worked out.
        work_out_plugins(app);
    }
    if copies {
        look_for_sound_copies(app, sounds);
    }
    // What arrived while the page was off show, unless a look above has drawn it all since.
    if PAGE.with(|p| p.borrow().redraw) {
        show(app);
    }
}

/// The Sounds page's list changed: read, your songs' use counted in, or brought up to date; or there's none to read.
/// While the page is on show and has looked, it counts the copies of the same sound again from the new list, and its
/// line shows what's known meanwhile; else it does when it next shows ([`shown`]).
pub fn sounds_changed(app: &AppWindow) {
    let looked = PAGE.with(|p| {
        let p = p.borrow();
        p.feed.is_some() && p.looked.is_some()
    });
    if !looked || app.get_place() != Place::Space.index() as i32 {
        return;
    }
    let sounds = sounds_page::known();
    if PAGE.with(|p| copies_due(&p.borrow(), &sounds, false)) {
        look_for_sound_copies(app, sounds);
    } else {
        show_also(app);
        show_card(app);
    }
}

/// Whether the copies of the same sound are due a look, with the Sounds page's sounds as `sounds` has them: none is
/// while that page hasn't read them. One is when the newest look was for another of its lists than the one it has now
/// (one with your songs' use counted in, or brought up to date), even while that look is on; else, with none on, when
/// nothing's found yet, or when what's on show is five minutes old (`stale`), or Hide duplicates was turned on or off
/// since (the copies it hides count as hidden).
fn copies_due(p: &Page, sounds: &Known, stale: bool) -> bool {
    let Known::Read(list, _) = sounds else { return false };
    let other = !Weak::ptr_eq(&p.copies_of, &Arc::downgrade(list)) || p.copies_hiding != sounds_page::hides_copies();
    // Measuring while the Sounds page looked, and that look failed: the page stops saying it's measuring.
    let failed = p.copies_looking && sounds_page::copies_found() == CopiesFound::Failed;
    other || failed || (!p.copies_looking && (stale || p.copies.is_none()))
}

/// Whether old copies are due a look as the page comes on show, with `moves` of the Plugins page's moves finished
/// (`stale`: what's on show is five minutes old; `held`: old copies are going to the Recycle Bin or coming back, so
/// none is). One is once a move has finished since the newest look was asked for, even while that's on (the newest
/// wins); else, with none on, when nothing's found yet or what's on show is stale.
fn old_due(p: &Page, moves: u64, stale: bool, held: bool) -> bool {
    !held && (moves != p.old_moves || (!p.old_looking && (stale || p.old.is_none())))
}

/// A move on the Plugins page finished, whatever came of it: it leaves an old copy beside its link, or, cut short,
/// what it had copied. While the page is on show and has looked, it looks for old copies again now (one still on was
/// asked for before the move finished: the newest wins), or, while old copies are going to the Recycle Bin or coming
/// back, once that's done ([`held`]); else it does when it next shows ([`shown`]).
pub fn plugin_moved(app: &AppWindow) {
    if app.get_place() != Place::Space.index() as i32 {
        return;
    }
    let now = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if p.feed.is_none() || p.looked.is_none() {
            return false;
        }
        let held = held(&p).old;
        p.look_after.old |= held;
        !held
    });
    if now {
        look_for_old(app);
    }
}

/// Your plugins were measured again and kept: while this page is on show and has looked, it looks at the plugins no
/// song has used lately again now; else it does when it next shows.
pub fn plugins_measured(app: &AppWindow) {
    let on_show = app.get_place() == Place::Space.index() as i32;
    let now = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.plugins_measured = true;
        on_show && p.feed.is_some() && p.plugins.is_some() && !p.plugins_looking
    });
    if now {
        look_for_plugins(app);
    }
}

/// Closes the picked line's card. Whether one was open.
fn close_card(app: &AppWindow) -> bool {
    let open = PAGE.with(|p| !std::mem::take(&mut p.borrow_mut().picked).is_empty());
    if open {
        show_card(app);
    }
    open
}

/// Esc on Space, after what plays has stopped (the one step back every page takes, review X3-12): the picked line's
/// card closes; the page has no search to clear. Whether it did anything, so the key goes on when it didn't (the
/// whole-app review of 10 Nov, X3-2: Esc did nothing here).
pub fn escape(app: &AppWindow) -> bool {
    close_card(app)
}

/// Runs one of the page's own shortcuts ([`mo_app::shortcuts::SPACE_ACTIONS`]). Returns whether it did anything,
/// so an unused key goes on.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    // Only the key's own button is worked out: as the card shows it now.
    let can = PAGE.with(|p| {
        let p = p.borrow();
        let enabled = |button: Option<(String, bool)>| button.is_some_and(|(_, enabled)| enabled);
        match id {
            MEASURE => !p.measuring,
            REVEAL => p.picked == space::APP_FILES,
            FIX => p.picked == BITWIG_COPIES && enabled(p.fixes.button(p.bitwig.as_ref())),
            REMOVE => match p.picked.as_str() {
                SPARE_COPIES => enabled(spare_remove_button(&p)),
                OLD_COPIES => enabled(old_remove_button(&p)),
                COMPACT => enabled(songs_remove_button(&p)),
                _ => false,
            },
            STOP => match p.picked.as_str() {
                OLD_COPIES => enabled(old_stop_button(&p)),
                COMPACT => enabled(songs_stop_button(&p)),
                _ => false,
            },
            PUT_BACK => match p.picked.as_str() {
                SPARE_COPIES => enabled(p.removing.put_back_button(CAN_PUT_BACK, &p.put_backs)),
                OLD_COPIES => enabled(p.old_removing.put_back_button(CAN_PUT_BACK, &p.put_backs)),
                COMPACT => enabled(p.songs_removing.put_back_button(CAN_PUT_BACK, &p.put_backs)),
                space_bin::RECYCLE_BIN => newest_put_back(&p).is_some(),
                _ => false,
            },
            _ => false,
        }
    });
    if can {
        chosen(app, id);
    }
    can
}

/// The newest clean-up the Recycle Bin card can put back now (Put back's key there), by its kind and when it ran.
fn newest_put_back(p: &Page) -> Option<(String, i64)> {
    space_bin::newest_put_back(p.kept.as_ref(), &p.put_backs, CAN_PUT_BACK, removing(p))
}

/// Whether a removal is on (spare copies, old copies or your songs' things), or its put back from its own card.
fn removing(p: &Page) -> bool {
    p.removing.busy() || p.old_removing.busy() || p.songs_removing.busy()
}

/// Whether something is going to the Recycle Bin or coming back from it: one at a time.
fn busy(p: &Page) -> bool {
    removing(p) || p.put_backs.going.is_some()
}

/// What's going to the Recycle Bin or coming back now, by its clean-up's kind: none when nothing is.
fn going(p: &Page) -> Option<&str> {
    if p.removing.busy() {
        Some(SPARE_COPIES)
    } else if p.old_removing.busy() {
        Some(OLD_COPIES)
    } else if p.songs_removing.busy() {
        Some(COMPACT)
    } else {
        p.put_backs.going.as_ref().map(|(kind, _)| kind.as_str())
    }
}

/// Which looks wait while something goes to the Recycle Bin or comes back, as what they'd measure may change under
/// them: for spare copies and old copies, while any plugin's things do (either can hold the other's); at your songs, the
/// longest look, only while their own things do, as nothing else the page removes is in a song's folder, and no song's
/// autosave or cache is a plugin's.
fn held(p: &Page) -> LookAfter {
    let held = |kind: &str| LookAfter { old: kind != COMPACT, spare: kind != COMPACT, songs: kind == COMPACT };
    going(p).map(held).unwrap_or_default()
}

/// The spare copies card's Remove: it waits while a look for them is on, or something else is going or coming back.
fn spare_remove_button(p: &Page) -> Option<(String, bool)> {
    let wait = p.spare_looking || p.old_removing.busy() || p.songs_removing.busy();
    p.removing.remove_button(p.spare.as_ref(), wait, &p.put_backs)
}

/// The old copies card's Remove: it waits while a look for them is on, or something else is going or coming back.
fn old_remove_button(p: &Page) -> Option<(String, bool)> {
    let wait = p.old_looking || p.removing.busy() || p.songs_removing.busy();
    p.old_removing.remove_button(p.old.as_ref(), wait, &p.put_backs)
}

/// The songs card's Remove: it waits while a look at your songs is on, or something else is going or coming back.
fn songs_remove_button(p: &Page) -> Option<(String, bool)> {
    let wait = p.songs_looking || p.removing.busy() || p.old_removing.busy();
    p.songs_removing.remove_button(p.songs.as_ref(), wait, &p.put_backs)
}

/// The old copies card's Stop, while their Remove is on: what isn't checked yet stays. Pressed, it says it's stopping.
fn old_stop_button(p: &Page) -> Option<(String, bool)> {
    stop_button(matches!(p.old_removing.stage(), space_old::Stage::Sending(_)), p.old_stop.as_ref())
}

/// The songs card's Stop, while their Remove is on: what hasn't gone yet stays.
fn songs_stop_button(p: &Page) -> Option<(String, bool)> {
    stop_button(matches!(p.songs_removing.stage(), space_songs::Stage::Sending(_)), p.songs_stop.as_ref())
}

/// A card's Stop while its Remove is on (`sending`), and `stop` what stops it: pressed, it says it's stopping.
fn stop_button(sending: bool, stop: Option<&Arc<AtomicBool>>) -> Option<(String, bool)> {
    if !sending {
        return None;
    }
    let stopping = stop.is_some_and(|stop| stop.load(Ordering::Relaxed));
    Some(if stopping { ("Stopping…".to_owned(), false) } else { ("Stop".to_owned(), true) })
}

/// The part at `at` on the card on show.
fn part_at(p: &Page, at: i32) -> Option<&Part> {
    usize::try_from(at).ok().and_then(|at| p.card_parts.get(at))
}

/// A button on the page or its card.
fn chosen(app: &AppWindow, id: &str) {
    let feed = PAGE.with(|p| p.borrow().feed.clone());
    match (id, feed) {
        (MEASURE, _) => {
            look_again(app);
            look_for_copies(app);
            // A look for old copies, for spare copies or at your songs that's on goes on: it read the library just now,
            // and nothing it measured is kept for a new one. None is asked for while what it finds is being removed or
            // put back ([`held`]): it looks once that's done. Once a removal's result is on show, its card goes back to
            // picking, from what the new look finds.
            let (old, spare, songs) = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let held = held(&p);
                p.look_after.old |= held.old;
                p.look_after.spare |= held.spare;
                p.look_after.songs |= held.songs;
                let spare = !p.spare_looking && !held.spare;
                let old = !p.old_looking && !held.old;
                let songs = !p.songs_looking && !held.songs;
                if spare && p.removing.shows_result() {
                    p.spare = None;
                }
                if spare {
                    p.removing.again();
                }
                if old && p.old_removing.shows_result() {
                    p.old = None;
                }
                if old {
                    p.old_removing.again();
                }
                if songs && p.songs_removing.shows_result() {
                    p.songs = None;
                }
                if songs {
                    p.songs_removing.again();
                }
                (old, spare, songs)
            });
            if songs {
                look_for_songs(app);
            }
            if old {
                look_for_old(app);
            }
            if spare {
                look_for_spare(app);
            }
            // The plugins look only reads the library, so one that's on goes on: it read it just now. So does the look
            // at the copies of the same sound, which only reads the sound library.
            if !PAGE.with(|p| p.borrow().plugins_looking) {
                look_for_plugins(app);
            }
            if !PAGE.with(|p| p.borrow().copies_looking) {
                // The files as they are now: copies made since are found (only files changed since are read).
                sounds_page::copies_look_again(app);
                look_for_sound_copies(app, sounds_page::known());
            }
        }
        (REVEAL, Some(feed)) => crate::files::open_folder(&feed, &paths::data_dir().to_string_lossy()),
        (FIX, Some(feed)) => fix_ticked(app, feed),
        (STOP, _) => {
            // The card on show's: old copies or your songs' things.
            PAGE.with(|p| {
                let p = p.borrow();
                let (stop, what) = match p.picked.as_str() {
                    OLD_COPIES => (&p.old_stop, "old copies"),
                    COMPACT => (&p.songs_stop, "old autosaves and caches"),
                    _ => return,
                };
                if let Some(stop) = stop {
                    log::info("Space", &format!("Removing {what}: Stop pressed"));
                    stop.store(true, Ordering::Relaxed);
                }
            });
            show_card(app);
        }
        (REMOVE, Some(feed)) => {
            let picked = PAGE.with(|p| p.borrow().picked.clone());
            match picked.as_str() {
                SPARE_COPIES => remove_ticked(app, feed),
                OLD_COPIES => remove_old_ticked(app, feed),
                COMPACT => remove_songs_ticked(app, feed),
                _ => {}
            }
        }
        (PUT_BACK, Some(feed)) => {
            // A clean-up's card's: the removal on show. The Recycle Bin card's: its newest clean-up.
            let job = PAGE.with(|p| {
                let p = p.borrow();
                match p.picked.as_str() {
                    SPARE_COPIES => p.removing.to_put_back().map(|sent| (SPARE_COPIES.to_owned(), sent.when)),
                    OLD_COPIES => p.old_removing.to_put_back().map(|sent| (OLD_COPIES.to_owned(), sent.when)),
                    COMPACT => p.songs_removing.to_put_back().map(|sent| (COMPACT.to_owned(), sent.when)),
                    space_bin::RECYCLE_BIN => newest_put_back(&p),
                    _ => None,
                }
            });
            if let Some((kind, when)) = job {
                put_back_kept(app, feed, &kind, when);
            }
        }
        (RECYCLE_BIN, Some(feed)) => crate::files::open_recycle_bin(&feed),
        _ => {}
    }
}

/// A button: its id, words and whether it can be pressed now; the card's main one is in the accent colour (Send to the
/// Recycle Bin, or Put back once it has run). None is in the colour for removing for good: nothing on Space goes for
/// good. What the bin can't keep stays where it is, and so does what it hasn't room for (the bin would remove it, or
/// the oldest things in it, for good), so everything that goes can come back (the whole-app review of 6 Nov, X3-11).
struct Button {
    id: &'static str,
    text: String,
    enabled: bool,
    main: bool,
}

impl Button {
    fn new(id: &'static str, text: impl Into<String>, enabled: bool) -> Button {
        Button { id, text: text.into(), enabled, main: false }
    }

    fn main(self) -> Button {
        Button { main: true, ..self }
    }
}

/// The buttons, each with its tooltip and key from the shortcut list as it is now (rule 15).
fn actions(buttons: Vec<Button>) -> ModelRc<CardAction> {
    let keys = crate::shortcuts();
    let actions: Vec<CardAction> = buttons
        .into_iter()
        .map(|b| CardAction {
            text: b.text.into(),
            tip: what_of(b.id).into(),
            key: keys.key_of(b.id).into(),
            id: b.id.into(),
            enabled: b.enabled,
            main: b.main,
            danger: false,
        })
        .collect();
    ModelRc::new(VecModel::from(actions))
}

/// Measure again, which waits while the drives and the app's files are being measured.
fn measure_button(p: &Page) -> Button {
    Button::new(MEASURE, "Measure again", !p.measuring)
}

/// Opens the Recycle Bin (the Trash on a Mac or Linux).
fn bin_button() -> Button {
    Button::new(RECYCLE_BIN, format!("Open {BIN}"), true)
}

/// Fix: writes a "(fixed)" project beside each ticked save, one after another on a low-priority thread, with an
/// activity card saying how far it is. Each save's line says how it went once it's done; then the page looks for
/// copies again, so it sees the fixed projects as they are on disk. The saves and their copies aren't changed.
fn fix_ticked(app: &AppWindow, feed: Feed) {
    let jobs = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = &mut *p;
        if p.fixes.busy() {
            return Vec::new();
        }
        let jobs = p.bitwig.as_ref().map(|found| p.fixes.to_fix(found)).unwrap_or_default();
        if !jobs.is_empty() {
            p.fixes.start(&jobs);
        }
        jobs
    });
    if jobs.is_empty() {
        return;
    }
    let weak = app.as_weak();
    let started = background::spawn("Space fix", move || {
        // A crash outside a save's own fix (logged as one) still finishes Fix, so the page isn't busy for ever: the
        // saves it didn't get to say so.
        let _ = catch_crash(AssertUnwindSafe(|| fix_each(&jobs, &feed, &weak)));
        arrive(&weak, Step::FixDone);
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start fixing: {e}"));
        PAGE.with(|p| p.borrow_mut().fixes.finished());
    }
    show_wins(app);
}

/// Fix's own work, off the window's thread: each save in turn, inside the low-priority pool (rule 21), with an
/// activity card saying how far it is and how it went, and each save's line told how it went.
fn fix_each(jobs: &[ToFix], feed: &Feed, weak: &slint::Weak<AppWindow>) {
    let card = feed.start(space_bitwig::FIXING, "");
    let (mut written, mut failed) = (0, 0);
    for (n, job) in jobs.iter().enumerate() {
        card.progress(n as f32 / jobs.len() as f32, Some(&space_bitwig::fix_progress_words(n, jobs.len())));
        // A crash (logged as one) fails this save only: the rest are still tried.
        let done = catch_crash(AssertUnwindSafe(|| background::on_pool(|| space_bitwig::fix(job))))
            .unwrap_or_else(|_| Fixing::Failed("it stopped with an error, which the log says".to_owned()));
        let save = job.save.display();
        match &done {
            Fixing::Written(file) => {
                written += 1;
                log::info("Space", &format!("Fixed {save}: wrote {}", file.display()));
            }
            Fixing::Failed(why) => {
                failed += 1;
                log::warning("Space", &format!("Couldn't fix {save}: {why}"));
            }
            Fixing::Writing => {}
        }
        arrive(weak, Step::Fixed(job.save.clone(), done));
    }
    match space_bitwig::fixed_words(written, failed) {
        Ok(words) => card.done(&words),
        Err(words) => card.failed(&words),
    }
}

/// Opens a fixed project in Bitwig: the program picked for it in Settings (shared with the C# app), else whatever opens
/// Bitwig projects. Off the window's thread, with an activity card saying how it went.
fn open_in_bitwig(file: PathBuf) {
    let Some((feed, library)) = PAGE.with(|p| {
        let p = p.borrow();
        Some((p.feed.clone()?, p.library.clone()))
    }) else {
        return;
    };
    let started = background::spawn("Open", move || {
        let path = file.to_string_lossy().into_owned();
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "")
                .failed("It has moved or been deleted since. Measure again to see what's there now.");
            return;
        }
        let exe = open_library(&library)
            .and_then(|lib| lib.setting(&shell::daw_exe_key(BITWIG)).ok().flatten())
            .filter(|e| shell::usable_exe(e));
        let card = feed.start(&format!("Opening {name}"), "");
        match shell::run(&shell::open(&path, exe.as_deref(), shell::Os::THIS)) {
            Ok(()) => card.done("Sent to Bitwig. It opens there in a moment."),
            Err(e) => {
                log::warning("Open", &format!("{path}: {e}"));
                card.failed(&format!("Couldn't open it in Bitwig: {e}"));
            }
        }
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start opening a fixed project: {e}"));
    }
}

/// What tells one clean-up's Remove from another's: its kind (as `mo_app::binned` keeps it), its thread, its activity
/// card, what the log calls one of them, where the page keeps how it goes, and how its result reaches the page.
struct Clean<T: 'static> {
    kind: &'static str,
    thread: &'static str,
    title: &'static str,
    words: fn() -> String,
    removed_words: fn(&Done) -> Result<String, String>,
    /// What the log calls one of them, within a sentence and at its start ("a spare plugin copy", "A spare plugin
    /// copy"), and many ("spare plugin copies").
    one: (&'static str, &'static str),
    many: &'static str,
    removing: fn(&mut Page) -> &mut space_removing::Removing<T>,
    sent: fn(i64, Done, Option<Kept>) -> Step,
}

static SPARE: Clean<SpareCopy> = Clean {
    kind: SPARE_COPIES,
    thread: "Space remove",
    title: space_spare::REMOVING,
    words: space_spare::removing_words,
    removed_words: space_spare::removed_words,
    one: ("a spare plugin copy", "A spare plugin copy"),
    many: "spare plugin copies",
    removing: spare_removing,
    sent: Step::SpareSent,
};

static OLD: Clean<Leftover> = Clean {
    kind: OLD_COPIES,
    thread: "Space remove old copies",
    title: space_old::REMOVING,
    words: space_old::removing_words,
    removed_words: space_old::removed_words,
    one: ("an old copy", "An old copy"),
    many: "old copies",
    removing: old_removing,
    sent: Step::OldSent,
};

static SONGS: Clean<SongItem> = Clean {
    kind: COMPACT,
    thread: "Space remove songs",
    title: space_songs::REMOVING,
    words: space_songs::removing_words,
    removed_words: space_songs::removed_words,
    one: ("an old autosave or cache", "An old autosave or cache"),
    many: "old autosaves and caches",
    removing: songs_removing,
    sent: Step::SongsSent,
};

fn spare_removing(p: &mut Page) -> &mut Removing {
    &mut p.removing
}

fn old_removing(p: &mut Page) -> &mut space_old::Removing {
    &mut p.old_removing
}

fn songs_removing(p: &mut Page) -> &mut space_songs::Removing {
    &mut p.songs_removing
}

/// Remove for spare copies: sends the ticked ones to the Recycle Bin ([`send_to_bin`]), each checked again just before
/// it goes (`space_spare::remove`), against your plugin folders and saves as the library has them now. Not while a
/// look for spare copies is on, nor while anything goes to the bin or comes back.
fn remove_ticked(app: &AppWindow, feed: Feed) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if busy(&p) || p.spare_looking {
            return None;
        }
        let picked = p.removing.ticked_copies(p.spare.as_ref());
        if picked.is_empty() {
            return None;
        }
        p.removing.start(picked.clone());
        hold_looks(&mut p, SPARE_COPIES);
        Some((picked, p.library.clone()))
    });
    let Some((picked, library)) = job else { return };
    // Its card has no Stop (a few copies go quickly); only the app closing stops it ([`finish_up`]).
    let stop = Arc::new(AtomicBool::new(false));
    send_to_bin(app, feed, &SPARE, picked, stop, move |picked, bin, stop| remove_now(picked, &library, bin, stop));
}

/// Remove's own work, off the window's thread: your plugin folders and which plugins your saves use, from the library
/// (read-only, and closed again first), then each picked copy checked again and sent. When those can't be read, every
/// copy stays (said in the log): what's spare can't be checked. What goes, goes to `bin`; `stop` leaves what hasn't
/// gone yet where it is.
fn remove_now(picked: &[SpareCopy], library: &Path, bin: &(dyn Bin + Sync), stop: &AtomicBool) -> Done {
    let Some((folders, used)) = plugin_folders_and_uses(library) else {
        return space_removing::stayed(picked, space_spare::CANT_CHECK);
    };
    let pro_tools = plugins::pro_tools_installed();
    // On the pool for long work that waits on a disk, as the look is (rule 21). Each copy is checked just before it
    // goes, and the one administrator step asks Windows once, at the end.
    background::on_bulk_pool(|| space_spare::remove(picked, &folders, &used, pro_tools, bin, stop))
}

/// Remove for old copies: sends the ticked ones to the Recycle Bin together ([`send_to_bin`]), each found again in the
/// moves as the library keeps them now, checked again and compared file by file with its copy just before it goes
/// (`space_old::remove`). That can take a while for a big sound library, so its card has Stop. Not while a look for old
/// copies is on, nor while anything goes to the bin or comes back.
fn remove_old_ticked(app: &AppWindow, feed: Feed) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if busy(&p) || p.old_looking {
            return None;
        }
        let found = p.old.clone()?;
        let picked = p.old_removing.ticked_copies(Some(&found));
        if picked.is_empty() {
            return None;
        }
        p.old_removing.start(picked.clone());
        hold_looks(&mut p, OLD_COPIES);
        let stop = Arc::new(AtomicBool::new(false));
        p.old_stop = Some(stop.clone());
        Some((picked, found, p.library.clone(), stop))
    });
    let Some((picked, found, library, stop)) = job else { return };
    send_to_bin(app, feed, &OLD, picked, stop, move |picked, bin, stop| {
        remove_old_now(picked, &found, &library, bin, stop)
    });
}

/// Remove for your songs' old autosaves and caches: sends the ticked ones to the Recycle Bin together ([`send_to_bin`]),
/// each song they're of measured again as it is now and each one checked again just before it goes
/// (`space_songs::remove`). Thousands of autosaves can take a while, so its card has Stop. Not while a look at your songs
/// is on, nor while anything goes to the bin or comes back.
fn remove_songs_ticked(app: &AppWindow, feed: Feed) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if busy(&p) || p.songs_looking {
            return None;
        }
        let shown = p.songs.as_ref()?;
        let (picked, settings) = (p.songs_removing.ticked_copies(Some(shown)), shown.settings());
        if picked.is_empty() {
            return None;
        }
        p.songs_removing.start(picked.clone());
        hold_looks(&mut p, COMPACT);
        let stop = Arc::new(AtomicBool::new(false));
        p.songs_stop = Some(stop.clone());
        Some((picked, settings, p.library.clone(), stop))
    });
    let Some((picked, settings, library, stop)) = job else { return };
    let work = move |picked: &[SongItem], bin: &(dyn Bin + Sync), stop: &AtomicBool| {
        remove_songs_now(picked, settings, &library, bin, stop)
    };
    send_to_bin(app, feed, &SONGS, picked, stop, work);
}

/// Remove's own work for your songs' things, off the window's thread: the saves, the samples they use and your folders
/// as the library has them now (read-only, and closed again first), then each song picked from measured again (with
/// the `settings` the look had) and each one checked and sent. When the library can't be read, every one stays (said
/// in the log): what's old can't be told without it. What goes, goes to `bin`; `stop` (Stop) leaves what hasn't gone
/// yet where it is.
fn remove_songs_now(
    picked: &[SongItem],
    settings: CompactSettings,
    library: &Path,
    bin: &(dyn Bin + Sync),
    stop: &AtomicBool,
) -> Done {
    let from = match open_library(library).map(|lib| FromLibrary::read(&lib)) {
        Some(Ok(from)) => from,
        Some(Err(e)) => {
            log::warning("Space", &format!("Couldn't read your songs from the library to check them again: {e}"));
            return space_removing::stayed(picked, space_songs::CANT_CHECK);
        }
        None => {
            // When it's there but couldn't be opened, `open_library` said why.
            if !library.exists() {
                log::warning("Space", "The library isn't there any more, so your songs couldn't be checked again.");
            }
            return space_removing::stayed(picked, space_songs::CANT_CHECK);
        }
    };
    // On the pool for long work that waits on a disk (it walks each song's folder again), as the look is (rule 21).
    background::on_bulk_pool(|| space_songs::remove(picked, &from, settings, bin, stop))
}

/// Remove's own work for old copies, off the window's thread: the moves as the library keeps them now (read-only, and
/// closed again first), then each picked one found again in them, checked and sent. When they can't be read, every one
/// stays (said in the log): it can't be checked against them. What goes, goes to `bin`; `stop` (Stop) leaves what isn't
/// checked yet where it is.
fn remove_old_now(
    picked: &[Leftover],
    found: &Found,
    library: &Path,
    bin: &(dyn Bin + Sync),
    stop: &AtomicBool,
) -> Done {
    let now = match open_library(library) {
        Some(lib) => {
            let read = |key: &str| lib.setting(key);
            match (read(space_old::MOVED_FOLDERS), read(space_old::OLD_HOMES)) {
                (Ok(moved), Ok(homes)) => (moved, homes),
                (Err(e), _) | (_, Err(e)) => {
                    log::warning("Space", &format!("Couldn't read the moved folders to check old copies again: {e}"));
                    return space_removing::stayed(picked, space_old::CANT_CHECK);
                }
            }
        }
        // No library: no moves are kept, so only what isn't a move's (files kept aside) can go.
        None if !library.exists() => (None, None),
        None => return space_removing::stayed(picked, space_old::CANT_CHECK),
    };
    // On the pool for long work that waits on a disk (it compares every file with its copy), as the look is (rule 21).
    // Each one is checked just before it goes, and the one administrator step asks Windows once, at the end.
    background::on_bulk_pool(|| space_old::remove(picked, found, (now.0.as_deref(), now.1.as_deref()), bin, stop))
}

/// Remove's own thread for one clean-up (`clean`): sends `picked` to the Recycle Bin with `work` on a low-priority
/// thread, with an activity card saying how it went; `stop` (its card's Stop, and the app closing: [`finish_up`])
/// leaves what hasn't gone yet where it is. What goes is noted as it goes ([`binned::Going`]), so a close or a crash
/// part way through doesn't lose it; at the end it's all kept (`mo_app::binned`), so Put back works after a restart
/// too, before the page hears it's done, so it shows both at once: what went and what stayed. Call it once `picked`
/// is on its way ([`space_removing::Removing::start`]).
fn send_to_bin<T: Removable + Send + 'static>(
    app: &AppWindow,
    feed: Feed,
    clean: &'static Clean<T>,
    picked: Vec<T>,
    stop: Arc<AtomicBool>,
    work: impl FnOnce(&[T], &(dyn Bin + Sync), &AtomicBool) -> Done + Send + 'static,
) {
    let weak = app.as_weak();
    let folder = paths::data_dir();
    let (when, sending) = (binned::now(), picked.clone());
    // Counted from here, so the app closing now waits for it too, until the thread ends (or never starts).
    let on_its_way = OnItsWay::new(stop);
    let started = background::spawn(clean.thread, move || {
        let on_its_way = on_its_way;
        let words = (clean.words)();
        let card = feed.start(clean.title, &words);
        // The card says how far it has got (thousands of autosaves can take minutes), under what it said at the start.
        let of = sending.len();
        let tally = Mutex::new(Tally::new(SystemBin.admin().can_ask()));
        let hear = |_: &Path, sent: &Result<(), String>| {
            let mut tally = tally.lock().unwrap_or_else(PoisonError::into_inner);
            tally.heard(sent);
            card.progress(tally.fraction(of), Some(&tally.words(&words, of)));
        };
        let note = binned::Going::start(&folder, clean.kind, when);
        let goes = |removed: &Removed| note.went(removed);
        let bin = Heard::new(&SystemBin, &hear).noting(&goes);
        // A crash (logged as one) still finishes Remove, so the page isn't busy for ever: what went before it was noted
        // as it went, and is kept as usual.
        let (done, crashed) = match catch_crash(AssertUnwindSafe(|| work(&sending, &bin, &on_its_way.0))) {
            Ok(done) => (done, false),
            Err(_) => {
                let still_there = |at: &str| std::fs::symlink_metadata(at).is_ok();
                (space_removing::after_crash(&sending, note.went_so_far(), still_there), true)
            }
        };
        for (warning, line) in space_removing::log_lines(&done, clean.one, clean.many) {
            if warning { log::warning("Space", &line) } else { log::info("Space", &line) }
        }
        match (clean.removed_words)(&done) {
            Ok(words) => card.done(&words),
            Err(words) => card.failed(&words),
        }
        let kept = catch_crash(AssertUnwindSafe(|| binned::record(&folder, &SystemBin, clean.kind, when, &done))).ok();
        // Its note goes once it's all kept; after a crash it stays for the next look, as Windows' administrator step
        // goes on by itself and may still send some of what waited for it.
        note.finish(!crashed && kept.as_ref().is_some_and(|(kept, _)| kept.noted));
        let when = kept.as_ref().map_or(when, |(_, at)| *at);
        arrive(&weak, (clean.sent)(when, done, kept.map(|(kept, _)| kept)));
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start removing {}: {e}", clean.many));
        let why = format!("Sending to the {BIN} couldn't start ({e}), so it stays.");
        PAGE.with(|p| (clean.removing)(&mut p.borrow_mut()).sent(when, space_removing::stayed(&picked, &why)));
        looks_after(app);
    }
    show_wins(app);
}

/// The Removes sending things to the Recycle Bin now, each by what stops it: as the app closes, each is stopped and
/// given a little time to keep what went ([`finish_up`]).
static REMOVES: Mutex<Vec<Arc<AtomicBool>>> = Mutex::new(Vec::new());
static REMOVE_ENDED: Condvar = Condvar::new();

/// The longest the app waits, as it closes, for a Remove it stopped to keep what went.
const CLOSE_WAIT: Duration = Duration::from_secs(15);

/// One Remove on its way, in [`REMOVES`] (with what stops it) until it ends, however it ends.
struct OnItsWay(Arc<AtomicBool>);

impl OnItsWay {
    fn new(stop: Arc<AtomicBool>) -> Self {
        REMOVES.lock().unwrap_or_else(PoisonError::into_inner).push(stop.clone());
        OnItsWay(stop)
    }
}

impl Drop for OnItsWay {
    fn drop(&mut self) {
        let mut removes = REMOVES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = removes.iter().position(|stop| Arc::ptr_eq(stop, &self.0)) {
            removes.swap_remove(at);
        }
        REMOVE_ENDED.notify_all();
    }
}

/// As the app closes: a Remove still sending things to the Recycle Bin stops (what hasn't gone yet stays where it is)
/// and is given a little time to keep what went, for Put back (the whole-app review of 6 Nov, E1-1). One that doesn't
/// end in time (Windows' administrator step may still be waiting for you) noted what went as it went: the page's next
/// look keeps it.
pub fn finish_up() {
    let mut removes = REMOVES.lock().unwrap_or_else(PoisonError::into_inner);
    if removes.is_empty() {
        return;
    }
    log::warning(
        "Space",
        &format!(
            "The app closed while things were going to the {BIN}: sending stops (what went stays there, for Put back)"
        ),
    );
    for stop in removes.iter() {
        stop.store(true, Ordering::Relaxed);
    }
    let deadline = Instant::now() + CLOSE_WAIT;
    while !removes.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warning(
                "Space",
                &format!(
                    "Sending didn't stop in time: what went to the {BIN} was noted as it went, and the Space page keeps \
                     it the next time it looks"
                ),
            );
            return;
        }
        removes = REMOVE_ENDED.wait_timeout(removes, left).unwrap_or_else(PoisonError::into_inner).0;
    }
}

/// Before what a clean-up of `kind` removed goes to the Recycle Bin or comes back: a look that's on whose finds that
/// can change ([`held`]) stops, as what it measures may change under it (its finds would show it as it was), and looks
/// again once that's done ([`looks_after`]). None of them starts meanwhile.
fn hold_looks(p: &mut Page, kind: &str) {
    let songs = kind == COMPACT;
    if p.old_looking && !songs {
        p.old_look.next();
        p.old_looking = false;
        p.look_after.old = true;
    }
    if p.spare_looking && !songs {
        p.spare_look.next();
        p.spare_looking = false;
        p.look_after.spare = true;
    }
    if p.songs_looking && songs {
        p.songs_look.next();
        p.songs_looking = false;
        p.look_after.songs = true;
    }
}

/// The looks [`hold_looks`] stopped look again, once nothing goes to the Recycle Bin or comes back.
fn looks_after(app: &AppWindow) {
    let after = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if busy(&p) { LookAfter::default() } else { std::mem::take(&mut p.look_after) }
    });
    if after.old {
        look_for_old(app);
    }
    if after.spare {
        look_for_spare(app);
    }
    if after.songs {
        look_for_songs(app);
    }
}

/// Put back of the clean-up of `kind` that ran at `when`, put back where it was ([`put_back`]): for the removal on show
/// on its card (old copies, spare copies or your songs' things), all it sent that was found in the bin, so its card can
/// say what came back of it (Put back says which aren't there any more); for another, what the bin still holds of it,
/// as `mo_app::binned` keeps it.
fn put_back_kept(app: &AppWindow, feed: Feed, kind: &str, when: i64) {
    let done = PAGE.with(|p| {
        let p = p.borrow();
        let on_show = match kind {
            SPARE_COPIES => p.removing.to_put_back().filter(|sent| sent.when == when).map(|sent| sent.coming_back()),
            OLD_COPIES => p.old_removing.to_put_back().filter(|sent| sent.when == when).map(|sent| sent.coming_back()),
            COMPACT => p.songs_removing.to_put_back().filter(|sent| sent.when == when).map(|sent| sent.coming_back()),
            _ => None,
        };
        on_show
            .or_else(|| p.kept.as_ref()?.list.iter().find(|b| b.kind == kind && b.when == when).map(|b| b.done.clone()))
    });
    if let Some(done) = done {
        put_back(app, feed, kind.to_owned(), when, done);
    }
}

/// Put back: returns what a clean-up sent to the Recycle Bin to where it was, on a low-priority thread, with an
/// activity card saying how it went; then the bin is looked in again, so what came back leaves its line. One at a
/// time, and not while something is being removed.
fn put_back(app: &AppWindow, feed: Feed, kind: String, when: i64, done: Done) {
    let started = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if busy(&p) || done.removed.is_empty() {
            return false;
        }
        p.put_backs.start(&kind, when);
        match kind.as_str() {
            SPARE_COPIES => p.removing.putting_back(when),
            OLD_COPIES => p.old_removing.putting_back(when),
            COMPACT => p.songs_removing.putting_back(when),
            _ => {}
        }
        hold_looks(&mut p, &kind);
        true
    });
    if !started {
        return;
    }
    let weak = app.as_weak();
    let folder = paths::data_dir();
    let job = (kind.clone(), done.clone());
    let spawned = background::spawn("Space put back", move || {
        let (kind, done) = job;
        let card = feed.start(space_bin::putting_back_title(&kind), "");
        let problems = catch_crash(AssertUnwindSafe(|| clean::undo(&done, &SystemBin))).unwrap_or_else(|_| {
            let why = space_bin::put_back_crashed();
            done.removed.iter().map(|r| Problem { path: r.path.clone(), why: why.clone() }).collect()
        });
        for r in done.removed.iter().filter(|r| !problems.iter().any(|p| p.path == r.path)) {
            log::info("Space", &format!("Put back from the {BIN}: {}", r.path));
        }
        for problem in &problems {
            log::warning("Space", &format!("Couldn't put back {}: {}", problem.path, problem.why));
        }
        match space_bin::put_back_words(done.removed.len(), &problems) {
            Ok(words) => card.done(&words),
            Err(words) => card.failed(&words),
        }
        // What came back isn't in the bin any more: looked at before the page hears Put back is done, so its line and
        // card never show it there with Put back on again.
        let kept = catch_crash(AssertUnwindSafe(|| binned::refresh(&folder, &SystemBin))).ok();
        arrive(&weak, Step::PutBack(kind, when, problems, kept));
    });
    if let Err(e) = spawned {
        log::error("Space", &format!("Couldn't start putting back: {e}"));
        let why = format!("Put back couldn't start ({e}).");
        let problems: Vec<Problem> =
            done.removed.iter().map(|r| Problem { path: r.path.clone(), why: why.clone() }).collect();
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match kind.as_str() {
                SPARE_COPIES => p.removing.put_back(when, problems.clone()),
                OLD_COPIES => p.old_removing.put_back(when, problems.clone()),
                COMPACT => p.songs_removing.put_back(when, problems.clone()),
                _ => {}
            }
            p.put_backs.done(&kind, when, problems);
        });
        looks_after(app);
    }
    show_wins(app);
}

/// Asks for a new look at the drives and the app's files, on the page's own low-priority thread. One still measuring
/// stops: the newest wins (rule 12).
fn look_again(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let (id, stop) = p.look.next();
        p.looked = Some(Instant::now());
        p.measuring = true;
        Ask { id, library: p.library.clone(), app_folder: paths::data_dir(), stop }
    });
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look, and Measure again for trying again.
    let work = move |ask: Ask| {
        let id = ask.id;
        if catch_crash(AssertUnwindSafe(|| look(ask, &weak))).is_err() {
            arrive(&weak, Step::Crashed(id));
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().look.ask("Space", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking at the drives: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.looked = None;
            p.measuring = false;
        });
    }
    show(app);
}

/// One look, off the window's thread: the drives your folders are on, then the app's own files.
fn look(ask: Ask, weak: &slint::Weak<AppWindow>) {
    let folders = folders_of(&ask.library, &ask.app_folder);
    let drives = space::drives(&folders, space::look);
    arrive(weak, Step::Drives(ask.id, drives));
    let data = &ask.app_folder;
    // The song and sound libraries and the MIDI index the app reads (its own songs.db and sounds-rust.db once they're
    // made), songs.db while it isn't read yet, the old app's databases it doesn't read any more, the sound library's
    // backup copy and the caches: each a part of its own.
    let databases = space::app_databases(data, ask.library.clone());
    // On the pool for long work that waits on a disk, as Space's other walks are (rule 21): the app's folder can hold
    // thousands of files, and on the pages' quick pool the walk would hold up their asks (Ctrl+K, a filter) until it
    // ended. A look stopped for a newer one says nothing: the newer one's sizes follow.
    if let Ok(parts) = background::on_bulk_pool(|| app_files::measure(data, &databases, &ask.stop)) {
        arrive(weak, Step::AppFiles(ask.id, parts));
    }
    // What the app put in the Recycle Bin that's still there, for its line under Also taking room. The bin is only
    // looked in when the app has put something there.
    arrive(weak, Step::Kept(binned::refresh(data, &SystemBin)));
}

/// Asks for a new look for Bitwig copies, on the page's second low-priority thread. One still looking stops: the
/// newest wins (rule 12).
fn look_for_copies(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let feed = p.feed.clone()?;
        let (id, stop) = p.bitwig_look.next();
        p.bitwig_looking = Some((0, 0));
        Some(BitwigAsk { id, library: p.library.clone(), stop, feed })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // The Library as the last look read it, kept on the thread so a file isn't read again while it's unchanged. A
    // crash in a look (logged as one) leaves the thread for the next look; the Library is read again then, and the
    // fingerprints kept between starts are forgotten, in case the crash came from what was kept.
    let mut index = None;
    let work = move |ask: BitwigAsk| {
        let id = ask.id;
        if catch_crash(AssertUnwindSafe(|| look_for_copies_now(ask, &mut index, &weak))).is_err() {
            // Let go of the store first, so it can be removed.
            index = None;
            mo_space::prints::forget(&mo_app::paths::data_dir().join(mo_space::prints::FILE_NAME));
            arrive(&weak, Step::BitwigCrashed(id));
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().bitwig_look.ask("Space Bitwig", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking for Bitwig copies: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().bitwig_looking = None);
    }
    show(app);
}

/// One look for Bitwig copies, off the window's thread: the project folders from the library (read-only), then each
/// project's copies, with an activity card saying how far it is.
fn look_for_copies_now(ask: BitwigAsk, index: &mut Option<LibraryIndex>, weak: &slint::Weak<AppWindow>) {
    let roots: Vec<PathBuf> = project_roots(&ask.library).into_iter().map(PathBuf::from).collect();
    let documents = bitwig::documents();
    let job = ask.feed.start(space_bitwig::LOOKING, space_bitwig::LOOKING_WHY);
    // How far it is reaches the window at most every PROGRESS_EVERY, and always at the start and the end.
    let mut told: Option<Instant> = None;
    // On the pool for long work over a disk (rule 21): it reads every copy, hundreds of gigabytes, so on the shared
    // pool it would hold up the pages' quick jobs, and its threads are low in the system's memory there.
    let prints = mo_app::paths::data_dir().join(mo_space::prints::FILE_NAME);
    let found = background::on_bulk_pool(|| {
        space_bitwig::look(&roots, documents.as_deref(), index, Some(&prints), &ask.stop, |done, of| {
            if of > 0 {
                job.progress(done as f32 / of as f32, Some(&space_bitwig::progress_words(done, of)));
            }
            if done == 0 || done == of || told.is_none_or(|at| at.elapsed() >= PROGRESS_EVERY) {
                told = Some(Instant::now());
                arrive(weak, Step::BitwigLooking(ask.id, (done, of)));
            }
        })
    });
    match found {
        Ok(copies) => {
            let (headline, words) = space_bitwig::found_words(&copies);
            job.done_as(headline, &words);
            arrive(weak, Step::Bitwig(ask.id, copies));
        }
        // Stopped for a newer look, whose own card follows.
        Err(_) => job.done_as(space_bitwig::LOOK_STOPPED.0, space_bitwig::LOOK_STOPPED.1),
    }
}

/// Asks for a new look for old copies, on the page's third low-priority thread. One still looking stops: the newest
/// wins (rule 12).
fn look_for_old(app: &AppWindow) {
    let moves = plugins_page::moves_this_session();
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.feed.as_ref()?;
        let (id, stop) = p.old_look.next();
        p.old_looking = true;
        // Every move that has finished by now is in the library this look reads.
        p.old_moves = moves;
        Some(OldAsk { id, library: p.library.clone(), stop })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look. A look stopped for a newer one says
    // nothing: the newer one's finds follow.
    let work = move |ask: OldAsk| {
        let id = ask.id;
        match catch_crash(AssertUnwindSafe(|| look_for_old_now(&ask))) {
            Ok(Some(found)) => arrive(&weak, Step::Old(id, found)),
            Ok(None) => {}
            Err(_) => arrive(&weak, Step::OldCrashed(id)),
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().old_look.ask("Space old copies", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking for old copies: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().old_looking = false);
    }
    // The old copies card's Measure again waits for it.
    show_card(app);
}

/// One look for old copies, off the window's thread: the moves as the library keeps them (read-only, and closed
/// again before anything is measured), then each old copy still there, measured, and any other "… (old copy)" folder
/// in the folders plugins keep their data in. None when it was stopped for a newer look.
fn look_for_old_now(ask: &OldAsk) -> Option<Found> {
    let (moved, homes) = match open_library(&ask.library) {
        Some(lib) => {
            let setting = |key: &str| {
                lib.setting(key).unwrap_or_else(|e| {
                    log::warning("Space", &format!("Couldn't read the moved folders ({key}): {e}"));
                    None
                })
            };
            (setting(space_old::MOVED_FOLDERS), setting(space_old::OLD_HOMES))
        }
        None => (None, None),
    };
    let look_in: Vec<String> =
        plugin_space::data_roots().iter().map(|root| root.to_string_lossy().into_owned()).collect();
    // On the pool for long work that waits on a disk (an old copy can be a whole sound library), so the pages' quick
    // jobs don't wait for it (rule 21).
    background::on_bulk_pool(|| space_old::look(moved.as_deref(), homes.as_deref(), &look_in, &ask.stop)).ok()
}

/// Asks for a new look for spare plugin copies, on the page's fourth low-priority thread. One still looking stops: the
/// newest wins (rule 12).
fn look_for_spare(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.feed.as_ref()?;
        let (id, stop) = p.spare_look.next();
        p.spare_looking = true;
        Some(SpareAsk { id, library: p.library.clone(), stop })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look. One that found nothing to show says so
    // too; one stopped for a newer one is passed over then, as the newer one's finds follow.
    let work = move |ask: SpareAsk| {
        let id = ask.id;
        match catch_crash(AssertUnwindSafe(|| look_for_spare_now(&ask))) {
            Ok(Some(found)) => arrive(&weak, Step::Spare(id, found)),
            Ok(None) | Err(_) => arrive(&weak, Step::SpareNone(id)),
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().spare_look.ask("Space spare copies", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking for spare plugin copies: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().spare_looking = false);
    }
    // The first look's line says it's looking, and the card's Measure again waits for it.
    show_wins(app);
}

/// One look for spare plugin copies, off the window's thread: your plugin folders and the formats your saves use, from
/// the library (read-only, and closed again before any plugin file is read), then every plugin file in those folders
/// and the AAX ones, with each spare one measured. None when it was stopped for a newer look, or the library couldn't
/// be read (said in the log): without what your saves use, every copy would look unused, and the page would say none
/// are spare. The page looks again the next time it shows.
fn look_for_spare_now(ask: &SpareAsk) -> Option<space_spare::Found> {
    let (folders, used) = plugin_folders_and_uses(&ask.library)?;
    let pro_tools = plugins::pro_tools_installed();
    // On the pool for long work that waits on a disk (a plugin bundle can hold thousands of files), so the pages'
    // quick jobs don't wait for it (rule 21).
    background::on_bulk_pool(|| space_spare::look(&folders, &used, pro_tools, &ask.stop)).ok()
}

/// Asks for a new look at your songs, on the page's fifth low-priority thread, when none is on ([`SongsAsk`]).
fn look_for_songs(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.feed.as_ref()?;
        let (id, stop) = p.songs_look.next();
        p.songs_looking = true;
        Some(SongsAsk { id, library: p.library.clone(), stop })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look. One that found nothing to show says so
    // too; one stopped for a newer one is passed over then, as the newer one's finds follow. The saves' fingerprints
    // are kept on the thread between looks, and in their store between starts (the whole-app review of 6 Nov, E1-5);
    // after a crash they're forgotten, in case what was kept is why.
    let mut prints: Option<Prints> = None;
    let work = move |ask: SongsAsk| {
        let id = ask.id;
        match catch_crash(AssertUnwindSafe(|| look_for_songs_now(&ask, &mut prints))) {
            Ok(Some(shown)) => arrive(&weak, Step::Songs(id, Box::new(shown))),
            Ok(None) => arrive(&weak, Step::SongsNone(id)),
            Err(_) => {
                // Let go of the store first, so it can be removed.
                prints = None;
                mo_space::prints::forget(&paths::data_dir().join(mo_app::song_room::SPACE_PRINTS));
                arrive(&weak, Step::SongsNone(id));
            }
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().songs_look.ask("Space songs", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start measuring your songs: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().songs_looking = false);
    }
    // The first look's line says it's measuring, and the cards' Measure again waits for it.
    show_wins(app);
}

/// One look at your songs, off the window's thread: the saves, the samples they use and your folders, from the
/// library (read-only, and closed again before any song is measured), then every song measured, and what the page
/// shows of it. `prints` are the saves' fingerprints the last looks read, taken from their store by the first look.
/// None when it was stopped, or the library couldn't be read (said in the log); with no library there are no songs.
fn look_for_songs_now(ask: &SongsAsk, prints: &mut Option<Prints>) -> Option<space_songs::Shown> {
    let from = match open_library(&ask.library) {
        Some(lib) => FromLibrary::read(&lib)
            .map_err(|e| log::warning("Space", &format!("Couldn't read your songs from the library: {e}")))
            .ok()?,
        None if ask.library.exists() => return None,
        None => FromLibrary::default(),
    };
    let prints: &Prints = prints.get_or_insert_with(|| {
        mo_app::song_room::prints_in(&paths::data_dir().join(mo_app::song_room::SPACE_PRINTS), "Space")
    });
    // On the pool for long work that waits on a disk (it walks every song's folder and compares saves of the same
    // size), so the pages' quick jobs don't wait for it (rule 21).
    let found =
        background::on_bulk_pool(|| space_songs::look(&from, CompactSettings::default(), Some(prints), &ask.stop))
            .ok()?;
    Some(space_songs::Shown::of(&found, chrono::Local::now().date_naive()))
}

/// Asks for a new look at the plugins no song has used lately, on the page's sixth low-priority thread ([`PluginsAsk`]).
fn look_for_plugins(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.feed.as_ref()?;
        let (id, _) = p.plugins_look.next();
        p.plugins_looking = true;
        p.plugins_measured = false;
        let fixes = plugins_page::folder_fixes_this_session();
        // The look on show stands only while the fixes it counted are the page's still.
        let stamp = p.plugins.as_ref().filter(|_| fixes == p.plugins_fixes).and(p.plugins_stamp.clone());
        Some(PluginsAsk { id, library: p.library.clone(), stamp, fixes })
    });
    let Some(ask) = ask else { return };
    // Your plugins measured again when the last measure is a day old, once a run (review X1-2): this look shows the
    // last measure meanwhile, and looks again once it's kept ([`plugins_measured`]).
    plugins_page::measure_when_due(app);
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look, and says so, as one that found nothing to
    // show does.
    let work = move |ask: PluginsAsk| {
        let id = ask.id;
        match catch_crash(AssertUnwindSafe(|| look_for_plugins_now(&ask))) {
            Ok(Some(looked)) => arrive(&weak, Step::Plugins(id, Box::new(looked))),
            Ok(None) | Err(_) => arrive(&weak, Step::PluginsNone(id)),
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().plugins_look.ask("Space plugins", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start reading your plugins: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().plugins_looking = false);
    }
    // The first look's line says it's reading, and the card's Measure again waits for it.
    show_wins(app);
}

/// One look at the plugins no song has used lately, off the window's thread: every plugin your songs use and the
/// sizes last measured, from the library (read-only, and closed again first), as the Plugins page reads them;
/// or, when the library is as the look on show found it, only that. None when the library couldn't be read (said in
/// the log); with no library there are no plugins.
fn look_for_plugins_now(ask: &PluginsAsk) -> Option<PluginsLooked> {
    let now = chrono::Local::now().naive_local();
    let stamp = mo_library::list_copy::stamp_of(&ask.library);
    if stamp.is_some() && stamp == ask.stamp {
        return Some(PluginsLooked::Same(now));
    }
    let read = match open_library(&ask.library) {
        Some(lib) => mo_app::plugins::read(&lib)
            .map_err(|e| log::warning("Space", &format!("Couldn't read your plugins from the library: {e}")))
            .ok()?,
        None if ask.library.exists() => return None,
        None => mo_app::plugins::Read {
            unused_years: mo_app::plugins::DEFAULT_UNUSED_YEARS,
            ..mo_app::plugins::Read::default()
        },
    };
    // On the pool for long work, as the page's other looks are (rule 21): it goes through every plugin of every save.
    let found = background::on_bulk_pool(|| space_plugins::look(read, now, &ask.fixes));
    Some(PluginsLooked::Found(found, stamp, ask.fixes.clone()))
}

/// Works out again what the plugins line shows ([`plugins_as_shown`]) when what it's worked out from has changed
/// ([`PluginsOf`]), on the page's own low-priority thread: it copies, sorts and goes through every plugin's folders, so
/// not on the window's thread (the whole-app review of 6 Nov, E1-3). The line and its card show it once it's in, and
/// what they showed stays meanwhile.
fn work_out_plugins(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let of = plugins_of(&p)?;
        if p.plugins_of.as_ref() == Some(&of) {
            return None;
        }
        p.plugins_of = Some(of.clone());
        let (id, _) = p.plugins_work.next();
        Some((id, of, binned_plugin_files(&p)))
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // A crash (logged as one) shows the look's plugins as they were found, until what they're worked out from changes
    // again: the line doesn't say it's reading for ever.
    let work = move |(id, of, binned): PluginsWork| {
        let binned = binned.unwrap_or_else(recorded_plugin_files);
        let found = catch_crash(AssertUnwindSafe(|| plugins_as_shown(&of, &binned)))
            .unwrap_or_else(|_| of.found.as_ref().clone());
        arrive(&weak, Step::PluginsNow(id, Box::new(found)));
    };
    let started = PAGE.with(|p| p.borrow_mut().plugins_work.ask("Space plugins line", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start working out the plugins you haven't used: {e}"));
        // Tried again the next time the page shows.
        PAGE.with(|p| p.borrow_mut().plugins_of = None);
    }
}

/// What the plugins line is worked out from now: none before a look has found anything. Quick: what's in the Recycle
/// Bin is told by which change to it the page has.
fn plugins_of(p: &Page) -> Option<PluginsOf> {
    let (found, at) = p.plugins.clone()?;
    let session = plugins_page::gone_this_session();
    let years = plugins_page::unused_years().unwrap_or(found.years);
    Some(PluginsOf { found, at, kept: p.kept.as_ref().map(|k| k.seq), session, years })
}

/// What the plugins line shows, as `of` says: what the look found, with the years the Plugins page has picked, less
/// what's gone since (`binned`: what the app's clean-ups here sent to the Recycle Bin that's still there; and what the
/// Plugins page's Clean ups and uninstalls took this session). Copies, sorts and goes through every plugin's folders:
/// call it off the window's thread.
fn plugins_as_shown(of: &PluginsOf, binned: &[String]) -> space_plugins::Found {
    let gone: Vec<String> = binned.iter().chain(&of.session).cloned().collect();
    of.found.as_of(of.years, of.at).without(&gone)
}

/// Whether the plugins line says it's reading: while a look is on with nothing found yet, and while what the first
/// look found is still being worked out ([`work_out_plugins`]).
fn plugins_reading(p: &Page) -> bool {
    p.plugins_looking || (p.plugins_shown.is_none() && p.plugins.is_some())
}

/// Asks for a new look at the copies of the same sound in the Sounds page's list as `sounds` has it, on the page's
/// seventh low-priority thread ([`CopiesAsk`]). The copies are the ones the Sounds page's Hide duplicates finds: until
/// that page has found them for this list it's asked to ([`sounds_page::copies_for_space`]), and this page looks again
/// as they come ([`sounds_changed`]). With no list yet, or none coming, it only shows that.
fn look_for_sound_copies(app: &AppWindow, sounds: Known) {
    let Known::Read(list, _) = sounds else {
        show_also(app);
        show_card(app);
        return;
    };
    sounds_page::copies_for_space(app);
    let hiding = sounds_page::hides_copies();
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.feed.as_ref()?;
        p.copies_of = Arc::downgrade(&list);
        p.copies_hiding = hiding;
        let state = sounds_page::copies_found();
        if list.copies().is_none() || matches!(state, CopiesFound::Looking { .. } | CopiesFound::NotLooked) {
            // Not all found for this list yet: measuring while the Sounds page looks (what's on show stays), and a
            // look of this page's still on its way is for an older list, so it's dropped. Nothing new to show if the
            // Sounds page's look failed.
            let _ = p.copies_look.next();
            p.copies_looking = !matches!(state, CopiesFound::Failed);
            return None;
        }
        let (id, stop) = p.copies_look.next();
        p.copies_looking = true;
        Some(CopiesAsk { id, list, hiding, stop })
    });
    if let Some(ask) = ask {
        let weak = app.as_weak();
        // A crash in a look (logged as one) leaves the thread for the next look, and says so, as one that found
        // nothing to show does.
        let work = move |ask: CopiesAsk| {
            let id = ask.id;
            match catch_crash(AssertUnwindSafe(|| look_for_sound_copies_now(&ask))) {
                Ok(Some(found)) => arrive(&weak, Step::Copies(id, Box::new(found))),
                Ok(None) | Err(_) => arrive(&weak, Step::CopiesNone(id)),
            }
        };
        let started = PAGE.with(|p| p.borrow_mut().copies_look.ask("Space copies", work, ask));
        if let Err(e) = started {
            log::error("Space", &format!("Couldn't start counting the copies of your sounds: {e}"));
            // Measure again can try again, and so can the page the next time it shows.
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.copies_looking = false;
                p.copies_of = Weak::new();
            });
        }
    }
    // The first look's line says it's reading, and the card's Measure again waits for it.
    show_also(app);
    show_card(app);
}

/// One look at the copies of the same sound, off the window's thread: the copies the Sounds page's list knows,
/// grouped as that page keeps them. None when it was stopped.
fn look_for_sound_copies_now(ask: &CopiesAsk) -> Option<space_sounds::Found> {
    if ask.stop.load(Ordering::Relaxed) {
        return None;
    }
    Some(space_sounds::look(&ask.list, ask.hiding))
}

/// The plugin files the app's clean-ups here sent to the Recycle Bin that are still there (spare plugin copies): none
/// until the page has looked in the bin this run (then what they kept is read, off the window's thread:
/// [`binned::plugin_files`]).
fn binned_plugin_files(p: &Page) -> Option<Vec<String>> {
    let kept = p.kept.as_ref()?;
    let binned = kept.list.iter().flat_map(|b| &b.done.removed);
    Some(binned.filter(|r| matches!(r.what, What::Plugin(_))).map(|r| r.path.clone()).collect())
}

/// The Plugins page changed what the app keeps of the Recycle Bin (a clean-up, an uninstall, a Put back: review X1-3):
/// the Recycle Bin line and the plugin files left out follow, as a look's finding would.
pub fn bin_changed(app: &AppWindow, kept: Kept) {
    arrive(&app.as_weak(), Step::Kept(kept));
}

/// The same, for the Plugins page: it leaves them out of what it counts, as this page does, so the two pages agree.
pub fn plugin_files_in_bin() -> Option<Vec<String>> {
    PAGE.with(|p| binned_plugin_files(&p.borrow()))
}

/// The plugin files the app's clean-ups here kept as sent to the Recycle Bin, as the app's folder had them when first
/// asked this run ([`binned::plugin_files`]): for before the page has looked in the bin. Only a clean-up here changes
/// what's kept, and once one has, the page has looked. Reads the disk the first time: call it off the window's thread.
pub fn recorded_plugin_files() -> Vec<String> {
    static RECORDED: OnceLock<Vec<String>> = OnceLock::new();
    RECORDED.get_or_init(|| binned::plugin_files(&paths::data_dir())).clone()
}

/// The folders to look for spare copies in and which formats your saves use, from the library (read-only, and closed
/// again before it returns). None when there's no library, or either can't be read (said in the log).
fn plugin_folders_and_uses(library: &Path) -> Option<(Vec<PathBuf>, Used)> {
    let lib = open_library(library)?;
    let chosen = match FolderList::Plugins.setting_key().map(|key| lib.list_setting(key)) {
        Some(Ok(chosen)) => chosen,
        None => Vec::new(),
        Some(Err(e)) => {
            log::warning("Space", &format!("Couldn't read your plugin folders, so spare copies aren't looked at: {e}"));
            return None;
        }
    };
    let used = match lib.plugin_uses() {
        Ok(uses) => space_spare::formats_used(&uses),
        Err(e) => {
            log::warning(
                "Space",
                &format!("Couldn't read which plugins your saves use, so spare copies aren't looked at: {e}"),
            );
            return None;
        }
    };
    drop(lib);
    Some((space_spare::plugin_folders(&chosen, plugins::default_folders(), plugins::aax_folders()), used))
}

/// The library, opened read-only for the folders it knows; None when there's none yet or it can't be read (said in
/// the log).
fn open_library(library: &Path) -> Option<mo_library::Library> {
    if !library.exists() {
        return None;
    }
    mo_library::Library::open_read_only(library)
        .map_err(|e| log::warning("Space", &format!("Couldn't read the library for its folders: {e}")))
        .ok()
}

/// Your project folders, as the library lists them.
fn roots_in(lib: &mo_library::Library) -> Vec<String> {
    lib.roots().unwrap_or_else(|e| {
        log::warning("Space", &format!("Couldn't read the project folders: {e}"));
        Vec::new()
    })
}

/// Your project folders, from the library (read-only).
fn project_roots(library: &Path) -> Vec<String> {
    open_library(library).as_ref().map(roots_in).unwrap_or_default()
}

/// The folders to look at: your project and sample folders from the library (read-only), the backup drive from the
/// backup's settings, and the app's own folder.
fn folders_of(library: &Path, app_folder: &Path) -> Vec<(Holds, PathBuf)> {
    let lib = open_library(library);
    let roots = lib.as_ref().map(roots_in).unwrap_or_default();
    let samples = lib
        .as_ref()
        .map(|lib| {
            lib.list_setting("sample_folders").unwrap_or_else(|e| {
                log::warning("Space", &format!("Couldn't read the sample folders: {e}"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    let backup_drive = backup::Settings::load(&app_folder.join(backup::SETTINGS_FILE)).drive;
    space::folders(&roots, &samples, backup_drive.as_deref(), app_folder)
}

/// A look's step reaches the window's thread, unless a newer look has been asked for since.
fn arrive(weak: &slint::Weak<AppWindow>, step: Step) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        let _doing = crate::watchdog::doing("Space: a look arrived");
        // A Plugins page clean-up put back from here: that page hears once this one is done with its own state.
        let mut plugins_put_back = None;
        let changed = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match step {
                Step::Drives(id, drives) if id == p.look.asked => {
                    p.drives = drives;
                    Changed::All
                }
                Step::AppFiles(id, parts) if id == p.look.asked => {
                    p.parts = Some(parts);
                    p.measuring = false;
                    Changed::All
                }
                Step::Crashed(id) if id == p.look.asked => {
                    p.measuring = false;
                    Changed::All
                }
                // Only the first look's progress is on the page; the activity card has a later one's.
                Step::BitwigLooking(id, at) if id == p.bitwig_look.asked => {
                    p.bitwig_looking = Some(at);
                    if p.bitwig.is_none() { Changed::BitwigLooking } else { Changed::Nothing }
                }
                Step::Bitwig(id, copies) if id == p.bitwig_look.asked => {
                    p.fixes.looked();
                    p.bitwig = Some(copies);
                    p.bitwig_looking = None;
                    Changed::Wins
                }
                // Shown again, the page looks again.
                Step::BitwigCrashed(id) if id == p.bitwig_look.asked => {
                    p.bitwig_looking = None;
                    Changed::Wins
                }
                Step::Old(id, found) if id == p.old_look.asked => {
                    p.old = Some(found);
                    p.old_looking = false;
                    Changed::Wins
                }
                // Shown again, the page looks again; what the last look found stays on show.
                Step::OldCrashed(id) if id == p.old_look.asked => {
                    p.old_looking = false;
                    Changed::Wins
                }
                Step::Spare(id, found) if id == p.spare_look.asked => {
                    p.spare = Some(found);
                    p.spare_looking = false;
                    Changed::Wins
                }
                // Shown again with nothing found yet, the page looks again; what the last look found stays on show.
                Step::SpareNone(id) if id == p.spare_look.asked => {
                    p.spare_looking = false;
                    Changed::Wins
                }
                Step::Songs(id, shown) if id == p.songs_look.asked => {
                    p.songs = Some(*shown);
                    p.songs_looking = false;
                    Changed::Wins
                }
                // Shown again with nothing found yet, the page looks again; what the last look found stays on show.
                Step::SongsNone(id) if id == p.songs_look.asked => {
                    p.songs_looking = false;
                    Changed::Wins
                }
                Step::Plugins(id, looked) if id == p.plugins_look.asked => {
                    match *looked {
                        PluginsLooked::Found(found, stamp, fixes) => {
                            let now = found.now;
                            p.plugins = Some((Arc::new(found), now));
                            p.plugins_stamp = stamp;
                            p.plugins_fixes = fixes;
                        }
                        // Counted back from now: worked out again as the line shows ([`work_out_plugins`]).
                        PluginsLooked::Same(now) => {
                            if let Some((_, at)) = p.plugins.as_mut() {
                                *at = now;
                            }
                        }
                    }
                    p.plugins_looking = false;
                    Changed::Wins
                }
                // Shown again with nothing found yet, the page looks again; what the last look found stays on show.
                Step::PluginsNone(id) if id == p.plugins_look.asked => {
                    p.plugins_looking = false;
                    Changed::Wins
                }
                Step::PluginsNow(id, found) if id == p.plugins_work.asked => {
                    if p.plugins_shown.as_ref() == Some(&*found) {
                        Changed::Nothing
                    } else {
                        p.plugins_shown = Some(*found);
                        Changed::Wins
                    }
                }
                Step::Copies(id, found) if id == p.copies_look.asked => {
                    p.copies = Some(*found);
                    p.copies_looking = false;
                    Changed::Also
                }
                // What the last look found stays on show, but it isn't of the newest list: shown again, the page looks
                // again.
                Step::CopiesNone(id) if id == p.copies_look.asked => {
                    p.copies_looking = false;
                    p.copies_of = Weak::new();
                    Changed::Also
                }
                Step::Fixed(save, fixing) => {
                    p.fixes.done(save, fixing);
                    Changed::Wins
                }
                Step::FixDone => {
                    p.fixes.finished();
                    Changed::LookAgain
                }
                Step::Kept(kept) => {
                    if keep(&mut p, Some(kept)) {
                        Changed::Also
                    } else {
                        Changed::Nothing
                    }
                }
                Step::SpareSent(when, done, kept) => {
                    keep(&mut p, kept);
                    p.removing.sent(when, done);
                    let LookAfter { spare, old, songs } = std::mem::take(&mut p.look_after);
                    Changed::Bin { spare, old, songs }
                }
                Step::OldSent(when, done, kept) => {
                    keep(&mut p, kept);
                    p.old_removing.sent(when, done);
                    p.old_stop = None;
                    let LookAfter { spare, old, songs } = std::mem::take(&mut p.look_after);
                    Changed::Bin { spare, old, songs }
                }
                Step::SongsSent(when, done, kept) => {
                    keep(&mut p, kept);
                    p.songs_removing.sent(when, done);
                    p.songs_stop = None;
                    let LookAfter { spare, old, songs } = std::mem::take(&mut p.look_after);
                    Changed::Bin { spare, old, songs }
                }
                Step::PutBack(kind, when, problems, kept) => {
                    let plugins = kind == space_bin::PLUGIN_CLEAN || kind == space_bin::PLUGIN_UNINSTALL;
                    let list = kept.as_ref().filter(|_| plugins).and_then(plugins_page::trusted_binned);
                    // A list older than the one the page has may not hold what the Plugins page kept since.
                    let newer = keep(&mut p, kept);
                    if plugins {
                        plugins_put_back = Some((kind.clone(), when, problems.is_empty(), list.filter(|_| newer)));
                    }
                    match kind.as_str() {
                        SPARE_COPIES => p.removing.put_back(when, problems.clone()),
                        OLD_COPIES => p.old_removing.put_back(when, problems.clone()),
                        COMPACT => p.songs_removing.put_back(when, problems.clone()),
                        _ => {}
                    }
                    p.put_backs.done(&kind, when, problems);
                    // What came back is where it was again: while you're picking, it's spare again, an old copy again,
                    // or one of a song's old autosaves or caches again. A look that stopped for it looks again too.
                    let after = std::mem::take(&mut p.look_after);
                    let spare = after.spare || kind == SPARE_COPIES && *p.removing.stage() == Stage::Picking;
                    let old = after.old || kind == OLD_COPIES && *p.old_removing.stage() == space_old::Stage::Picking;
                    let songs =
                        after.songs || kind == COMPACT && *p.songs_removing.stage() == space_songs::Stage::Picking;
                    Changed::Bin { spare, old, songs }
                }
                _ => Changed::Nothing,
            }
        });
        if let Some((kind, when, all, list)) = plugins_put_back {
            plugins_page::put_back_elsewhere(&kind, when, all, list.as_deref());
        }
        // Off show the page draws nothing, as what arrives can take tens of milliseconds to draw and some comes
        // several times a second (rule 12): it's drawn once when it comes back ([`shown`]). The looks a step asks
        // for still start.
        if !matches!(changed, Changed::Nothing) && app.get_place() != Place::Space.index() as i32 {
            PAGE.with(|p| p.borrow_mut().redraw = true);
            match changed {
                Changed::LookAgain => look_for_copies(&app),
                Changed::Bin { spare, old, songs } => {
                    if spare {
                        look_for_spare(&app);
                    }
                    if old {
                        look_for_old(&app);
                    }
                    if songs {
                        look_for_songs(&app);
                    }
                }
                _ => {}
            }
            return;
        }
        match changed {
            Changed::All => show(&app),
            Changed::Wins => show_wins(&app),
            Changed::BitwigLooking => show_bitwig_looking(&app),
            Changed::LookAgain => look_for_copies(&app),
            Changed::Also => {
                show_also(&app);
                show_card(&app);
                // What's in the Recycle Bin from here counts on the plugins line too.
                work_out_plugins(&app);
            }
            Changed::Bin { spare, old, songs } => {
                show_also(&app);
                show_wins(&app);
                if spare {
                    look_for_spare(&app);
                }
                if old {
                    look_for_old(&app);
                }
                if songs {
                    look_for_songs(&app);
                }
            }
            Changed::Nothing => {}
        }
    });
}

/// What the app put in the Recycle Bin that's still there, as `kept` says now, unless what's on show is newer: of two
/// that arrive out of turn (a look's and a removal's, say), the later change wins ([`Kept::seq`]). Whether it's kept.
fn keep(p: &mut Page, kept: Option<Kept>) -> bool {
    match kept {
        Some(kept) if p.kept.as_ref().is_none_or(|k| kept.seq > k.seq) => {
            p.kept = Some(kept);
            true
        }
        _ => false,
    }
}

fn room_line(line: space::Line) -> RoomLine {
    RoomLine { id: line.id.into(), title: line.title.into(), why: line.why.into(), size: line.size.into() }
}

/// Shows what the page knows now: the summary, the drives, the lists and the picked line's card.
pub fn show(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    // It draws it all, so nothing waits to be drawn.
    PAGE.with(|p| p.borrow_mut().redraw = false);
    PAGE.with(|p| {
        let p = p.borrow();
        let app_bytes = p.parts.as_deref().map_or(0, space::app_bytes);
        data.set_summary(space::summary(&p.drives, app_bytes).into());
        let cards: Vec<DriveCard> = p
            .drives
            .iter()
            .map(|d| DriveCard {
                name: d.name.as_str().into(),
                room: space::room_line(d).into(),
                used: d.used(),
                holds: space::holds_line(d).into(),
                note: space::fill_note(d, app_bytes).into(),
                fill: d.fill(app_bytes).index(),
                here: d.here,
            })
            .collect();
        data.set_drives(ModelRc::new(VecModel::from(cards)));
        data.set_actions(actions(vec![measure_button(&p), bin_button()]));
    });
    show_also(app);
    show_wins(app);
}

/// Shows Also taking room: the app's own files, the copies of the same sound in your sample folders (while the Sounds
/// page reads the sounds, that it's reading), then what the app put in the Recycle Bin that's still there. A picked
/// line that's gone (no copies found this time, nothing of the app's left in the bin) has its card closed.
fn show_also(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    let reading = matches!(sounds_page::known(), Known::Reading);
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let app_drive = space::app_drive(&p.drives);
        let files = space::app_files_line(p.parts.as_deref(), app_drive.map(|d| d.name.as_str()));
        let copies = space_sounds::line(p.copies.as_ref(), p.copies_looking || reading);
        let bin = space_bin::line(p.kept.as_ref());
        if (copies.is_none() && p.picked == SOUND_COPIES) || (bin.is_none() && p.picked == space_bin::RECYCLE_BIN) {
            p.picked.clear();
        }
        let also: Vec<RoomLine> = std::iter::once(files).chain(copies).chain(bin).map(room_line).collect();
        data.set_also(ModelRc::new(VecModel::from(also)));
    });
}

/// Shows Biggest wins first and the picked line's card: all that changes while a look for Bitwig copies, old copies or
/// spare copies, or at your songs, goes on. A picked line that's gone from the list (none found this time) has its card
/// closed.
fn show_wins(app: &AppWindow) {
    // The plugins line as last worked out; once what it's worked out from has changed, it's worked out again, and
    // shows when that's in.
    work_out_plugins(app);
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let bitwig = space_bitwig::line(p.bitwig.as_ref(), p.bitwig_looking);
        let old = space_old::line(p.old.as_ref(), &p.old_removing);
        let spare = space_spare::line(p.spare.as_ref(), p.spare_looking, &p.removing);
        let [compact, thin] = space_songs::lines(p.songs.as_ref(), p.songs_looking, &p.songs_removing);
        let found = p.plugins_shown.as_ref();
        let unused = space_plugins::line(found, plugins_reading(&p));
        if (bitwig.is_none() && p.picked == BITWIG_COPIES)
            || (old.is_none() && p.picked == OLD_COPIES)
            || (spare.is_none() && p.picked == SPARE_COPIES)
            || (compact.is_none() && p.picked == COMPACT)
            || (thin.is_none() && p.picked == space_songs::THIN)
            || (unused.is_none() && p.picked == UNUSED_PLUGINS)
        {
            p.picked.clear();
        }
        let lines = space::biggest_first(
            bitwig.into_iter().chain(old).chain(spare).chain(compact).chain(thin).chain(unused).collect(),
        );
        let wins: Vec<RoomLine> = lines.into_iter().map(room_line).collect();
        let shown = wins.len();
        data.set_wins(ModelRc::new(VecModel::from(wins)));
        let unused_none = space_plugins::none_note(p.plugins_shown.as_ref());
        let nones = [
            space_bitwig::none_note(p.bitwig.as_ref()),
            space_spare::none_note(p.spare.as_ref()),
            p.songs.as_ref().map_or("", |s| s.none_note),
            unused_none.as_str(),
        ];
        data.set_wins_note(space::wins_note(&nones, shown).into());
    });
    show_card(app);
}

/// Shows how far a look for Bitwig copies is: only its own line's words change (it stays where it is, measured as
/// nothing until the look ends), and its card's when that's open. Every line was made again for each step before,
/// about 40 ms on the window's thread, several times a second, and on whatever page was on show (F12 note's log, 5 Nov
/// 2026); off show nothing is drawn now ([`arrive`]).
fn show_bitwig_looking(app: &AppWindow) {
    let (line, picked) = PAGE.with(|p| {
        let p = p.borrow();
        (space_bitwig::line(p.bitwig.as_ref(), p.bitwig_looking), p.picked == BITWIG_COPIES)
    });
    let shown = line.is_some_and(|(_, line)| replace_line(&app.global::<SpaceData>().get_wins(), room_line(line)));
    if !shown {
        // Its line isn't on show yet, or is gone: every line is made again.
        return show_wins(app);
    }
    if picked {
        show_card(app);
    }
}

/// Puts `line` in place of the line with its id among `lines`, where that is. Whether it was there.
fn replace_line(lines: &ModelRc<RoomLine>, line: RoomLine) -> bool {
    let Some(at) = (0..lines.row_count()).find(|&i| lines.row_data(i).is_some_and(|l| l.id == line.id)) else {
        return false;
    };
    lines.set_row_data(at, line);
    true
}

/// Shows which line is picked and its card; picking a line or closing its card changes only this.
fn show_card(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        data.set_picked(p.picked.as_str().into());
        let (card, buttons) = match p.picked.as_str() {
            space::APP_FILES => (
                space::app_files_card(p.parts.as_deref(), space::app_drive(&p.drives)),
                vec![Button::new(REVEAL, "Show in folder", true), measure_button(&p)],
            ),
            BITWIG_COPIES => {
                let fix = p.fixes.button(p.bitwig.as_ref()).map(|(words, enabled)| Button::new(FIX, words, enabled));
                let card = space_bitwig::card(p.bitwig.as_ref(), p.bitwig_looking, &p.fixes);
                (card, fix.into_iter().chain([measure_button(&p)]).collect())
            }
            // Send to the Recycle Bin first (the main button, as Plugins' clean-up is), or once it has run, Put back (the
            // main button then) and the Recycle Bin; Measure again waits while a look is on (which it shows) or something
            // goes or comes back.
            OLD_COPIES => {
                let guarded = |path: &str| plugin_space::is_install_folder(path);
                let card = space_old::card(p.old.as_ref(), &p.old_removing, CAN_PUT_BACK, guarded);
                let mut buttons = Vec::new();
                if let Some((words, enabled)) = old_remove_button(&p) {
                    buttons.push(Button::new(REMOVE, words, enabled).main());
                }
                if let Some((words, enabled)) = old_stop_button(&p) {
                    buttons.push(Button::new(STOP, words, enabled));
                }
                if let Some((words, enabled)) = p.old_removing.put_back_button(CAN_PUT_BACK, &p.put_backs) {
                    buttons.push(Button::new(PUT_BACK, words, enabled).main());
                }
                if p.old_removing.shows_result() {
                    buttons.push(bin_button());
                }
                let measure = measure_button(&p);
                let enabled = measure.enabled && !p.old_looking && !busy(&p);
                buttons.push(Button { enabled, ..measure });
                (card, buttons)
            }
            // Send to the Recycle Bin first (the main button), or once it has run, Put back (the main button then) and
            // the Recycle Bin; Measure again waits while a look is on or something goes or comes back.
            SPARE_COPIES => {
                let guarded = |path: &str| plugin_space::is_install_folder(path);
                let card = space_spare::card(p.spare.as_ref(), p.spare_looking, &p.removing, CAN_PUT_BACK, guarded);
                let mut buttons = Vec::new();
                if let Some((words, enabled)) = spare_remove_button(&p) {
                    buttons.push(Button::new(REMOVE, words, enabled).main());
                }
                if let Some((words, enabled)) = p.removing.put_back_button(CAN_PUT_BACK, &p.put_backs) {
                    buttons.push(Button::new(PUT_BACK, words, enabled).main());
                }
                if p.removing.shows_result() {
                    buttons.push(bin_button());
                }
                let measure = measure_button(&p);
                let enabled = measure.enabled && !p.spare_looking && !busy(&p);
                buttons.push(Button { enabled, ..measure });
                (card, buttons)
            }
            // Send to the Recycle Bin first (the main button) with Stop while it's on, or once it has run, Put back (the
            // main button then) and the Recycle Bin; Measure again waits while a look is on or something goes or comes
            // back.
            COMPACT => {
                let card = space_songs::compact_card(p.songs.as_ref(), &p.songs_removing, CAN_PUT_BACK);
                let mut buttons = Vec::new();
                if let Some((words, enabled)) = songs_remove_button(&p) {
                    buttons.push(Button::new(REMOVE, words, enabled).main());
                }
                if let Some((words, enabled)) = songs_stop_button(&p) {
                    buttons.push(Button::new(STOP, words, enabled));
                }
                if let Some((words, enabled)) = p.songs_removing.put_back_button(CAN_PUT_BACK, &p.put_backs) {
                    buttons.push(Button::new(PUT_BACK, words, enabled).main());
                }
                if p.songs_removing.shows_result() {
                    buttons.push(bin_button());
                }
                let measure = measure_button(&p);
                let enabled = measure.enabled && !p.songs_looking && !busy(&p);
                buttons.push(Button { enabled, ..measure });
                (card, buttons)
            }
            // Read-only for now: Measure again waits while a look at your songs is on.
            space_songs::THIN => {
                let card = space_songs::thin_card(p.songs.as_ref());
                let measure = measure_button(&p);
                let enabled = measure.enabled && !p.songs_looking;
                (card, vec![Button { enabled, ..measure }])
            }
            // Read-only: each plugin's link shows it on the Plugins page. Measure again waits while a look is on.
            // As the line last showed it (the page shows the line again whenever that changes).
            UNUSED_PLUGINS => {
                let card = space_plugins::card(p.plugins_shown.as_ref(), plugins_reading(&p));
                let measure = measure_button(&p);
                let enabled = measure.enabled && !p.plugins_looking;
                (card, vec![Button { enabled, ..measure }])
            }
            // Read-only: each sound's link shows it on the Sounds page, and each of its files' shows it in its folder.
            // Measure again waits while a look is on.
            SOUND_COPIES => {
                let reading = matches!(sounds_page::known(), Known::Reading);
                let card = space_sounds::card(p.copies.as_ref(), p.copies_looking || reading);
                let measure = measure_button(&p);
                let enabled = measure.enabled && !p.copies_looking;
                (card, vec![Button { enabled, ..measure }])
            }
            // Each clean-up's Put back is a link on its line; the bin itself opens from here.
            space_bin::RECYCLE_BIN => {
                let card = space_bin::card(p.kept.as_ref(), &p.put_backs, CAN_PUT_BACK, removing(&p));
                (card, vec![bin_button(), measure_button(&p)])
            }
            // Nothing picked, or the line went: no card, and no menu of its buttons left to choose from.
            _ => {
                set_line_menu(&data, &[]);
                return;
            }
        };
        data.set_card_kind(card.kind.into());
        data.set_card_title(card.title.into());
        data.set_card_big(card.big.into());
        data.set_card_big_note(card.big_note.into());
        data.set_card_lead(card.lead.into());
        data.set_card_ticks(card.ticks);
        // Made again only when they change, so how far a look or Fix is doesn't make every row again.
        if card.parts != p.card_parts {
            let parts: Vec<CardPart> = card.parts.iter().map(card_part).collect();
            data.set_card_parts(ModelRc::new(VecModel::from(parts)));
            p.card_parts = card.parts;
        }
        data.set_card_note(card.note.into());
        data.set_card_room(card.room.into());
        data.set_card_warning(card.warning.into());
        set_line_menu(&data, &buttons);
        data.set_card_actions(actions(buttons));
    });
}

/// The picked line's right-click menu: its card's buttons, each with its key from the shortcut list as it is now
/// (rule 15), as Songs' line menu has its panel's (the whole-app review of 10 Nov, X3-2). Made again only when it
/// changes: the card is shown again as often as a look moves on, and a menu made again while it's open could lose the
/// click on it.
fn set_line_menu(data: &SpaceData, buttons: &[Button]) {
    let keys = crate::shortcuts();
    let items: Vec<MenuItem> = buttons
        .iter()
        .map(|b| MenuItem {
            what: b.text.as_str().into(),
            key: keys.key_of(b.id).into(),
            id: b.id.into(),
            enabled: b.enabled,
        })
        .collect();
    let now = data.get_line_menu();
    if now.row_count() != items.len() || now.iter().zip(&items).any(|(was, item)| was != *item) {
        data.set_line_menu(ModelRc::new(VecModel::from(items)));
    }
}

fn card_part(part: &Part) -> CardPart {
    // A clean-up's Put back is its link that opens: the page knows which by the part.
    let open = match (&part.put_back, &part.open, &part.plugins, part.sound) {
        (Some(_), _, _, _) => space_bin::PUT_BACK,
        (None, Some((opens, _)), _, _) => opens.words(),
        (None, None, Some(on), _) => on.words(),
        (None, None, None, Some(_)) => space::ON_SOUNDS,
        (None, None, None, None) => "",
    };
    CardPart {
        name: part.name.as_str().into(),
        what: part.what.as_str().into(),
        size: part.size.as_str().into(),
        under: part.under,
        can_tick: part.tick.is_some(),
        ticked: part.tick.as_ref().is_some_and(|(_, ticked)| *ticked),
        partly: part.partly,
        status: part.status.as_str().into(),
        problem: part.problem,
        open: open.into(),
        more: part.more.as_ref().map(|(words, _)| words.as_str()).unwrap_or_default().into(),
    }
}

/// Made-up finds for the screenshots: what each of the page's looks would have found (the copies of the same sound
/// too), what Fix did, how Remove went for spare copies, old copies and your songs' things (and the ticks changed), and
/// what the app put in the Recycle Bin.
#[derive(Default)]
pub struct Sample {
    pub drives: Vec<Drive>,
    pub parts: Vec<AppFilesPart>,
    pub bitwig: Copies,
    pub fixes: Fixes,
    pub old: Found,
    pub old_removing: space_old::Removing,
    pub spare: space_spare::Found,
    pub removing: Removing,
    pub songs: space_songs::Found,
    pub songs_removing: space_songs::Removing,
    pub plugins: space_plugins::Found,
    pub copies: space_sounds::Found,
    pub kept: Option<Kept>,
    pub put_backs: PutBacks,
}

/// Shows made-up drives, app files, Bitwig copies (with what Fix did to them), old copies, spare plugin copies and
/// what your songs could free (with how Remove went), and what's in the Recycle Bin, with the line `picked` picked,
/// without reading any disk: for the screenshots.
pub fn show_sample(app: &AppWindow, sample: Sample, picked: &str) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.drives = sample.drives;
        p.parts = Some(sample.parts);
        p.bitwig = Some(sample.bitwig);
        p.fixes = sample.fixes;
        p.old = Some(sample.old);
        p.old_removing = sample.old_removing;
        p.spare = Some(sample.spare);
        p.removing = sample.removing;
        p.songs = Some(space_songs::Shown::of(&sample.songs, chrono::Local::now().date_naive()));
        p.songs_removing = sample.songs_removing;
        let now = sample.plugins.now;
        p.plugins = Some((Arc::new(sample.plugins), now));
        p.copies = Some(sample.copies);
        p.kept = sample.kept;
        // Worked out here, with no thread to send it back, and nothing read from the disk for it: with no made-up
        // bin, nothing in it is a plugin's.
        if let Some(of) = plugins_of(&p) {
            p.plugins_shown = Some(plugins_as_shown(&of, &binned_plugin_files(&p).unwrap_or_default()));
            p.plugins_of = Some(of);
        }
        p.put_backs = sample.put_backs;
        p.picked = picked.to_owned();
        // Shown now, so showing the page doesn't look at the real drives or projects.
        p.looked = Some(Instant::now());
    });
    show(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_is_in_the_shortcut_list() {
        for id in [MEASURE, REVEAL, FIX, REMOVE, STOP, PUT_BACK, RECYCLE_BIN] {
            assert_ne!(what_of(id), "", "{id} isn't in the shortcut list");
        }
        let keys = mo_app::shortcuts::Shortcuts::default();
        assert_eq!(keys.page_action_for("F5", mo_app::shortcuts::SPACE_ACTIONS), Some(MEASURE));
        assert_eq!(keys.page_action_for("Ctrl+Shift+E", mo_app::shortcuts::SPACE_ACTIONS), Some(REVEAL));
    }

    #[test]
    fn how_far_a_look_for_bitwig_copies_is_changes_only_its_own_line() {
        let line = |id: &str, why: &str| RoomLine { id: id.into(), why: why.into(), ..RoomLine::default() };
        let lines = ModelRc::new(VecModel::from(vec![line(OLD_COPIES, "Old"), line(BITWIG_COPIES, "1 of 9")]));
        assert!(replace_line(&lines, line(BITWIG_COPIES, "2 of 9")));
        let whys: Vec<String> = lines.iter().map(|l| l.why.to_string()).collect();
        assert_eq!(whys, ["Old", "2 of 9"]);
        // One that isn't on show: the lines are made again instead.
        assert!(!replace_line(&lines, line(SPARE_COPIES, "Spare")));
        assert_eq!(lines.row_count(), 2);
    }

    #[test]
    fn a_finished_move_on_the_plugins_page_makes_old_copies_due_again() {
        // Nothing found yet: due.
        let mut p = Page::default();
        assert!(old_due(&p, 0, false, false));
        // Found, fresh, and no move since: not due, until it's five minutes old.
        p.old = Some(Found::default());
        assert!(!old_due(&p, 0, false, false));
        assert!(old_due(&p, 0, true, false));
        // A move finished since: due, even while a look is on (it was asked for before the move ended)...
        p.old_looking = true;
        assert!(!old_due(&p, 0, true, false));
        assert!(old_due(&p, 1, false, false));
        // ...but not while old copies are going to the Recycle Bin or coming back.
        assert!(!old_due(&p, 1, false, true));
        // Asked for after it: not due again.
        p.old_moves = 1;
        assert!(!old_due(&p, 1, false, false));
    }
}
