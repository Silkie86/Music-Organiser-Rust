// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use mo_app::activity::Feed;
use mo_app::background::Pace;
use mo_app::log;
use mo_app::shortcuts::Shortcuts;
use mo_catch::Catcher;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    // Started by the Space page as the administrator step: do it and stop, before any window, log or setting. This also
    // tells the Space page it may ask Windows for administrator rights.
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if mo_space::admin::run_step(&args) {
        return Ok(());
    }
    // Started by the uninstaller: the app and the helper running from this program are asked to close, and waited
    // for, so their files can go (UP-12). No window or setting of its own; a crash still gets its crash file. Ends
    // with 1 when one is still running, which the uninstaller's log shows.
    if args.iter().any(|a| a == mo_app::install_choice::CLOSE_FLAG) {
        log_crashes();
        let Ok(exe) = std::env::current_exe() else { return Ok(()) };
        // As long as the app's own close may take, and a little more.
        let wait = mo_app::one_copy::CLOSE_WAIT + std::time::Duration::from_secs(10);
        let closed = mo_app::install_choice::close_for_setup(&exe, wait);
        for line in &closed.lines {
            eprintln!("{line}");
        }
        if !closed.all_ended {
            std::process::exit(1);
        }
        return Ok(());
    }
    // Started as the background helper (at login, or by the app): no window at all, only the helper.
    if mo_ui::helper::asked(&args) {
        log_crashes();
        mo_ui::helper::run();
        return Ok(());
    }
    mo_ui::startup::begin();
    // Before the log starts, so a crash while it starts still gets its crash file.
    log_crashes();
    // One copy at a time (DF-1): a copy of this build that's open already is asked to show its window, and this one
    // stops; a copy of another build is asked to close, and waited for. Before anything of the data folder is opened.
    let one_copy = mo_ui::one_copy::claim();
    if matches!(one_copy, mo_app::one_copy::Claim::Shown) {
        return Ok(());
    }
    mo_ui::startup::step("one copy");
    // The graphics card takes a while to wake up: it's looked for while the log and the settings start.
    mo_ui::gpu::look_ahead();
    // The data folder was moved to a drive that isn't there now: ask what to do before anything opens the old copy
    // (F-2), rather than quietly opening on it.
    while let Some(problem) = mo_app::paths::missing_now() {
        match mo_ui::startup::data_folder_missing(&problem) {
            mo_ui::startup::Missing::TryAgain => {}
            mo_ui::startup::Missing::UseOldCopy => break,
            mo_ui::startup::Missing::Close => return Ok(()),
        }
    }
    // A data folder copied to another drive last run is caught up and switched to now, before any of the app's files is
    // opened (Settings, Move to another drive).
    let moved = mo_app::data_move::finish_at_start();
    let session = log::start_session();
    if let Some(moved) = &moved {
        match moved {
            mo_app::data_move::AtStart::Switched { .. } => log::info("Data folder", &moved.line()),
            _ => log::warning("Data folder", &moved.line()),
        }
    }
    if let Some(problem) = mo_app::paths::problem() {
        log::warning("Data folder", &problem);
    }
    mo_ui::one_copy::log_claim(&one_copy);
    // While this copy runs, another started meanwhile never switches folders under it.
    if let Err(why) = mo_app::data_move::hold_open_lock() {
        log::warning("Data folder", &why);
    }
    // Programs the app opens (DAWs, folders) start at the window's priority, not a background thread's.
    mo_app::shell::get_ready();
    mo_ui::startup::step("log");
    let feed = Feed::default();
    // Using the old copy of the data this run: a card says so until it's put away (Settings says so too).
    if let Some(problem) = mo_app::paths::problem() {
        feed.notice("Using the old copy of your data", &problem);
    }
    // This app's own settings over the C# app's, which library.db keeps and the app only reads.
    let data = mo_app::paths::data_dir();
    // The app's own song library, songs.db, once it's been made and checked; the C# app's library.db until then (and
    // songs.db is made from it in the background, for the next start).
    let songs_db = mo_library::songs_db::at_start(&data, &mo_app::version::version());
    log::info("Song library", &songs_db.line());
    if let Some(notice) = songs_db.notice() {
        log::warning("Song library", &notice);
        feed.notice("Your song library couldn't be read", &notice);
    }
    let library = songs_db.library.clone();
    if matches!(songs_db.state, mo_library::songs_db::State::Own | mo_library::songs_db::State::Switched) {
        // Once, for a songs.db made before them: the indexes the Songs list is read from, made in the background.
        mo_app::song_library::tune_soon(library.clone());
    }
    // Rule 28: what an earlier run or a stopped tool left behind is cleared, a little later, at low priority.
    mo_app::tidy::start(library.clone());
    // The note of what Space's Fix writes begins now if it hasn't, so a "(fixed)" project you save in from now on stays
    // out of the Recycle Bin (the whole-app review of 14 Nov, FX-5).
    mo_app::space_fix_note::begin_soon();
    if !songs_db.is_songs_db() {
        // The saves list is read from a small copy of the app's own while library.db hasn't changed (it's slow to
        // read out of library.db itself; songs.db needs no copy).
        mo_library::list_copy::keep(&library, &data.join(mo_library::list_copy::FILE_NAME));
    }
    let (settings, writable) = mo_ui::settings::load(&data, &library);
    let shortcuts = Shortcuts::from_setting(settings.get(mo_app::shortcuts::SETTING_KEY));
    // Settings' pace: how many low-priority threads background work gets, from now until the app closes.
    let pace = Pace::from_setting(settings.get(mo_app::app_settings::PACE_KEY));
    mo_app::background::set_pace(pace);
    // The Songs list is read from here on, while the graphics card is found and the window is made. (After the pace
    // is set: background work started before it would fix the pool's size without it.)
    let songs_ahead = mo_ui::songs::read_ahead(library.clone(), songs_db.is_songs_db());
    mo_ui::startup::step("settings");
    log::info(
        "Settings",
        &format!("Pace {}: {} background threads", pace.name(), pace.threads(mo_app::background::cores())),
    );
    // The graphics card draws the window and the galaxy; without one, the processor does.
    mo_ui::gpu::choose();
    mo_ui::startup::step("graphics card");
    let look = mo_ui::settings::look_of(&settings);
    let start = mo_app::app_settings::start_place(&settings);
    let full_screen = mo_app::app_settings::opens_full_screen(&settings);
    let app = match mo_ui::new_app(&look, shortcuts.clone(), start, feed.clone()) {
        Ok(app) => app,
        Err(e) => {
            // Rule 17: the reason the window couldn't open reaches the log too.
            log::error("App", &format!("The window couldn't open: {e}"));
            session.close();
            return Err(e);
        }
    };
    mo_ui::startup::step("window");
    if let mo_app::one_copy::Claim::Ours { held, .. } = &one_copy {
        mo_ui::one_copy::connect(&app, held);
    }
    // A close asked from outside closes the app properly, minimised or not.
    mo_ui::close_ask::connect();
    mo_ui::gpu::connect(&app);
    // The window's moves, resizes and hiding, for the freeze lines.
    mo_ui::watchdog::watch_window(&app);
    // Closing isn't a freeze: the watchdog stops as soon as the window is asked to close (Slint takes the window's
    // graphics apart inside the event loop, which took over a second on 15 Nov 2026 and was logged as one).
    app.window().on_close_requested(|| {
        mo_ui::watchdog::stop();
        // A copy started from this moment waits for this one to end, rather than being told this window will show.
        mo_ui::one_copy::closing();
        slint::CloseRequestResponse::HideWindow
    });
    // Dragging sounds and clips out to a DAW says here when it can't.
    mo_ui::drag::connect(feed.clone());
    // The Songs page reads the song library (the C# app's library.db, read-only, until songs.db is the app's own), and
    // keeps what it reads of your saves, and the Bitwig copies it converts, only ever in songs.db.
    mo_ui::songs::connect(&app, songs_ahead, feed.clone());
    mo_ui::startup::step("Songs page");
    mo_ui::sounds::keep_likeness_in(mo_app::paths::data_dir());
    mo_ui::sounds::connect(
        &app,
        mo_app::paths::data_dir().join("sounds.db"),
        library.clone(),
        feed.clone(),
        &shortcuts,
    );
    mo_ui::startup::step("Sounds page");
    // MIDI: the MIDI index beside the library, read-only (the C# app's midi.db until the Rust one holds parts).
    mo_ui::midi::connect(&app, library.clone(), feed.clone(), &shortcuts);
    mo_ui::startup::step("MIDI page");
    // Space, Backup: the backup settings live in a file of their own; the library is only read.
    mo_ui::backup::connect(&app, library.clone(), feed.clone());
    mo_ui::startup::step("Backup page");
    // Settings: changes are kept in the app's own file; the folder lists and the export folder in the song library.
    mo_ui::settings::connect(
        &app,
        mo_ui::settings::Setup {
            settings,
            folder: data.clone(),
            library: library.clone(),
            running_pace: pace,
            live: true,
            writable,
            feed: feed.clone(),
        },
    );
    mo_ui::startup::step("Settings page");
    // Presets: the library is only read; part fixes and curves go in the app's own presets.db and curves.db.
    mo_ui::presets::connect(&app, library.clone(), feed.clone(), &shortcuts);
    mo_ui::startup::step("Presets page");
    // Plugins: read from the C# app's library, read-only.
    mo_ui::plugins::connect(&app, library.clone(), feed.clone(), &shortcuts);
    mo_ui::startup::step("Plugins page");
    // Space: the drives your folders are on and the app's own files, looked at once the page shows.
    mo_ui::space::connect(&app, library.clone(), feed.clone());
    mo_ui::startup::step("Space page");
    // The rail's "In your DAWs": what Bitwig has open, asked on a thread of its own.
    mo_ui::daws::connect(&app);
    // A while after start, once a day: whether a newer version is out (an installed copy only; Settings turns it off).
    mo_ui::update::connect(&app, feed.clone(), true);
    // The window opens maximised. Its size is given first: without one, Slint shrinks the window winit has just
    // maximised back to its preferred size on the first show, and Windows still counts that small window as
    // maximised, so it stayed small in the corner (the 19 Nov review, CL-1). 1440 x 900 is then only the size
    // Restore gives. Full screen, when F11 left it on, is turned on once the window is up (fill_the_screen), so the
    // place winit keeps for F11 is the maximised one and F11 out of full screen leaves the window filling the
    // screen (CL-5).
    app.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    app.window().set_maximized(true);
    mo_ui::settings::full_screen_at_start(full_screen);
    // A little after start, a card if the app's drive or the temp folder's is almost full.
    mo_app::drive_room::watch(data.clone(), feed.clone());
    // A little after start, the MIDI files older drags left in Temp (rule 28).
    mo_ui::drag::sweep_later();
    // Later still, the old app's databases put away, once the app's own have worked without them for a week: never
    // in a run whose pages read library.db.
    if songs_db.is_songs_db() {
        // Settings then looks again at what the folder the data came from still holds for the app (the whole-app
        // review of 14 Nov, AP-18: it looks only when it shows, not at every redraw).
        let settings = app.as_weak();
        mo_app::old_databases::archive_later(data.clone(), feed.clone(), move || {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = settings.upgrade() {
                    mo_ui::settings::look_left_behind(&app);
                }
            });
        });
    }
    // A little after start too, songs.db made from library.db, for the next start.
    if songs_db.make_wanted() {
        mo_app::song_library::make_later(data, feed.clone());
    }
    // Windows' typing service is read from disk now, on a thread of its own, not at the first click in a text box.
    mo_ui::typing::get_ready();
    // The watchdog starts once the window is up, so opening it doesn't count as a freeze.
    let watched = feed.clone();
    let sized = app.as_weak();
    if slint::invoke_from_event_loop(move || {
        // The size of everything, once the window knows the screen's own scale.
        if let Some(app) = sized.upgrade() {
            mo_ui::settings::apply_size(&app);
            // Maximised (or full screen) again now the window is up, and checked half a second later against the
            // screen's width (Solomon, 19 Nov: a small window in the corner of his 3440-wide screen).
            mo_ui::settings::fill_the_screen(&app);
            // A drag to a screen with another scale takes Windows' place for the window (25 Nov: it flickered).
            mo_ui::screen_move::connect(&app);
        }
        mo_ui::watchdog::start(watched);
    })
    .is_err()
    {
        log::warning("Freeze", "Couldn't start the freeze watchdog");
    }
    let held = match &one_copy {
        mo_app::one_copy::Claim::Ours { held, .. } => Some(held.clone()),
        _ => None,
    };
    // Signing out, shutting down or restarting Windows with the app open: Windows ends the app as soon as it has
    // answered, so everything waiting is written there and then (the 19 November 2026 review's CL-2).
    #[cfg(windows)]
    {
        let held = held.clone();
        mo_ui::close_ask::on_session_end(move || {
            log::last(log::Level::Info, "App", "Windows is ending the session: closing");
            mo_ui::watchdog::stop();
            finish(held.as_deref(), session, None);
            end_now(0)
        });
    }
    let ran = app.run();
    // The window's thread answers no more knocks from here: what it does while closing isn't a freeze. (Said already
    // when the window was asked to close; this is for an end that came another way.)
    mo_ui::watchdog::stop();
    finish(held.as_deref(), session, ran.as_ref().err().map(ToString::to_string));
    // Off Windows the usual way out stays: the window is taken apart first, which an exit on the spot there
    // wouldn't wait for.
    #[cfg(windows)]
    {
        if let Err(e) = &ran {
            eprintln!("Error: {e}");
        }
        end_now(u32::from(ran.is_err()))
    }
    #[cfg(not(windows))]
    ran
}

/// The close: everything still waiting is written, what plays in a DAW is stopped, and the log is closed. Run once
/// the window's loop has ended, or from inside it when Windows ends the session (it ends the app straight after).
/// `held` is this copy's hold on being the one open copy; `error` is how the window stopped, when it stopped with an
/// error. A setup "Restart to update" started waits for this app to end before it touches a file.
fn finish(held: Option<&mo_app::one_copy::OneCopy>, session: &'static log::SessionLog, error: Option<String>) {
    // The window is gone: a copy started from now on waits for this one to end (the port is let go at the very
    // end, once everything below is written) instead of asking it to show.
    if let Some(held) = held {
        held.closing();
    }
    if let Some(e) = &error {
        // A line of how the run ends: it shows even once the log is full.
        log::last(log::Level::Error, "App", &format!("The window stopped with an error: {e}"));
    }
    // The rail's DAW watch stops asking Bitwig.
    mo_ui::daws::finish_up();
    // An update's fetch going stops with the app, waited for: curl isn't left fetching with nothing open (UP-4).
    mo_app::update::stop_fetches_and_wait(std::time::Duration::from_secs(2));
    // A plugin move still copying is told to stop, its copy cleared, rather than leave a half copy on the other drive.
    mo_ui::plugins::stop_moves();
    // Bars picked on the MIDI page's slider a moment ago, not yet in the settings, go into them first.
    mo_ui::midi::write_picks_waiting();
    // A settings change still waiting to be written goes now.
    mo_ui::settings::finish_up();
    // So does a status, stars, heart or tag given on a song's card while the Songs page was busy: waited for up to 10
    // seconds, only while some are waiting.
    mo_ui::songs::finish_up();
    // A Remove still sending things to the Recycle Bin stops, and keeps what went for Put back.
    mo_ui::space::finish_up();
    // A backup still running stops tidily, and its settings and songs list waiting to be written go now.
    mo_ui::backup::finish_up();
    // A note left open keeps its draft for next time, but the launcher shouldn't wait on it any more.
    mo_ui::note::wait_for_saves();
    mo_ui::note::finish_up();
    // The MIDI files this run's drags wrote, once the DAW has long read them (rule 28).
    mo_ui::drag::finish_up();
    // The last plays, drags and sends, hearts, types and tags, and a galaxy map being kept, go into the sound library:
    // waited for up to 10 seconds, only while some are waiting.
    mo_ui::sounds::finish_up();
    // A part playing in Bitwig or FL stops with the app.
    mo_ui::midi::finish_up();
    // So does a preset being auditioned there.
    mo_ui::presets::finish_up();
    // And waited for, a minute at most, once everything else has been told to stop.
    mo_ui::plugins::finish_up();
    // The sweep's and the frames' last few seconds, before the log closes.
    mo_app::sweep_timing::finish();
    mo_app::frame_timing::finish();
    session.close();
    // Everything is written. The port goes now, not with the process, so the copy started next (by the launcher, or
    // by the setup) is never held up by this one taking its time to end.
    if let Some(held) = held {
        held.let_go();
    }
}

/// Ends the app on the spot (Windows), once everything is written and the log is closed. Leaving `main` the usual
/// way has the window's graphics taken apart piece by piece first, and then Windows unloads the graphics card's driver
/// from the app: on 15 Nov 2026 that kept Solomon's app alive more than ten seconds after "Closed" (the card was busy
/// with Bitwig), until the launcher forced it shut. Nothing in that work is the app's to keep: Windows takes back the
/// window, the card's memory and every open file with the process, at once.
///
/// What this gives up: a writer the close above stopped waiting for (the log on a drive that's asleep after 5 s, a
/// backup after 20 s, a plugin move after a minute) used to get the seconds the taking-apart took, by luck. Those
/// waits are unchanged, and each of those writers already copes with being cut off: leaving `main` ended them too.
#[cfg(windows)]
#[allow(unsafe_code)]
fn end_now(code: u32) -> ! {
    use std::io::Write;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: plain calls on this process itself; when the second comes back at all, it failed, and the usual way
    // out below is taken.
    unsafe { TerminateProcess(GetCurrentProcess(), code) };
    std::process::exit(code.cast_signed())
}

/// Rule 17, crashes always log: a panic anywhere goes into the session log, with where it happened, and is written out
/// before the app closes. It also gets its own `logs\crash-<time>.log`, in case the session log couldn't be written
/// (one for each place in the code that crashed, a few places a run, so a crash that keeps happening can't fill the
/// folder; the crash that ends the app always gets its file; rule 28). A panic the app catches on purpose
/// (`mo_catch::catch`: a damaged file tripped a reader) isn't a crash: it's one warning line ([`caught`]). Nothing in
/// here may crash itself: std would end the app on the spot.
fn log_crashes() {
    set_crash_hook(mo_app::paths::logs_dir, mo_ui::note::after_crash, flush_the_log, |fatal| {
        // A crash that ends the app lets go of FL's notes and takes out what plays in Bitwig, as closing does (the 6
        // Nov review's D-10): the threads that would do it go with the app.
        if fatal {
            mo_ui::midi::after_crash();
            mo_ui::presets::after_crash();
        }
    });
}

/// Puts in the crash handler. A panic a [`mo_catch::catch`] catches is only a warning line ([`caught`]). Anything else
/// is a crash: what the app does with it ([`on_crash`], into the logs folder `logs` gives, with `keep_note` and
/// `flush`), then `after`, told whether it ends the app (the app stops what plays in a DAW then; a test counts the
/// crashes), then std's own report of it, on the console
/// when there is one. Std's report comes last: it can wait for ever on a console nobody reads.
fn set_crash_hook(
    logs: impl Fn() -> PathBuf + Send + Sync + 'static,
    keep_note: impl Fn(bool) + Send + Sync + 'static,
    flush: impl Fn() + Send + Sync + 'static,
    after: impl Fn(bool) + Send + Sync + 'static,
) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let current = std::thread::current();
        let (catcher, fatal) =
            what_it_is(log::cannot_unwind(info), info.payload_as_str(), current.name(), mo_catch::panic_starting);
        if catcher == Catcher::Catch {
            caught(info, current.name());
            return;
        }
        on_crash(info, fatal, &logs(), &keep_note, &flush);
        after(fatal);
        default(info);
    }));
}

/// What a panic that's starting is: what catches it, and whether it ends the app. `cannot_unwind` and `message` are
/// the panic's ([`log::cannot_unwind`], what it says), `thread` the name of the thread it's on, and `catcher` asks what
/// was to catch it (`mo_catch::panic_starting`, asked only when something can).
fn what_it_is(
    cannot_unwind: bool,
    message: Option<&str>,
    thread: Option<&str>,
    catcher: impl FnOnce() -> Catcher,
) -> (Catcher, bool) {
    // Std ends the app straight after a panic that can't unwind, and after its own words, whatever was to catch it.
    let std_ends_it = cannot_unwind || log::std_ends_the_app(message);
    let catcher = if std_ends_it { Catcher::Nothing } else { catcher() };
    // One on the window's thread ends the app too, unless a catch there lets the window go on.
    (catcher, std_ends_it || catcher == Catcher::Nothing && log::crash_ends_the_app(thread, message))
}

/// How much of what a caught panic said goes in its line, in characters.
const MOST_SAID: usize = 1000;

/// Where in the code a panic happened: `file:line`.
fn place_of(info: &std::panic::PanicHookInfo<'_>) -> String {
    info.location().map_or_else(|| "unknown".to_owned(), |l| format!("{}:{}", l.file(), l.line()))
}

/// What the app does with a panic a [`mo_catch::catch`] catches on purpose (a damaged file tripped a reader): a warning
/// line in the session log, counted by where in the code it happened, and nothing more. The work it was part of gives
/// an error and the app goes on, so there's no backtrace to take, no crash log, no F12 note to keep and no report from
/// std (which could wait on a console nobody reads).
fn caught(info: &std::panic::PanicHookInfo<'_>, thread: Option<&str>) {
    let place = place_of(info);
    let said = info.payload_as_str().unwrap_or("(it said nothing)");
    // A reader's own words can hold a whole buffer: a line that big would fill the log for the rest of the run.
    let said = match said.char_indices().nth(MOST_SAID) {
        Some((cut, _)) => format!("{}… (cut short)", &said[..cut]),
        None => said.to_owned(),
    };
    let thread = thread.unwrap_or("unnamed");
    log::caught(&place, &format!("{said} (at {place}, on thread {thread}; caught there, and the app went on)"));
}

/// The places in the code this run has written a crash log for.
static CRASH_FILES: log::CrashFiles = log::CrashFiles::new();

/// Waits until what the session log was given so far is on disk (a moment at most).
fn flush_the_log() {
    if let Some(l) = log::current() {
        l.flush();
    }
}

/// What the app does with a crash, before std's own report of it: the crash goes into the session log (`flush` waits
/// for it to be on disk) and into a crash log of its own in `logs`, and the words of an open F12 note are kept
/// (`keep_note`, told whether the crash ends the app: `fatal`). What the crash says goes to disk first, everywhere, and
/// its backtrace only after: taking a backtrace waits for a lock of std's that std holds while it reports another
/// thread's crash on the console, and that can wait for ever (a console that's paused, or a pipe nobody reads). Nothing
/// else in here waits long.
fn on_crash(
    info: &std::panic::PanicHookInfo<'_>,
    fatal: bool,
    logs: &Path,
    keep_note: &dyn Fn(bool),
    flush: &dyn Fn(),
) {
    let current = std::thread::current();
    let thread = current.name().unwrap_or("unnamed");
    let place = place_of(info);
    let text = format!("{} {} crashed on thread {thread}\n{info}\n", mo_app::product::NAME, mo_app::version::version());
    log::crash(&place, &text, fatal);
    // Out to disk now. The log's own writer can't wait for itself to write the crash out.
    let on_disk = || {
        if thread != log::WRITER_THREAD {
            flush();
        }
    };
    on_disk();
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let file = log::write_crash_file(logs, &text, secs, &place, fatal, &CRASH_FILES);
    keep_note(fatal);
    // Then where in the code it came from. Written out rather than formatted: a backtrace that can't be written in
    // full mustn't crash the crash handler.
    let mut trace = String::new();
    let _ = std::fmt::Write::write_fmt(&mut trace, format_args!("{}", std::backtrace::Backtrace::force_capture()));
    if let Some(file) = &file {
        log::add_to_crash_file(file, &format!("\n{trace}\n"));
    }
    log::crash_backtrace(&place, &format!("Backtrace of the crash on thread {thread}:\n{trace}"), fatal);
    on_disk();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// Set for the copy of this test program that crashes while the console is blocked: the folder it logs into.
    const IN_THE_COPY: &str = "MO_TEST_CRASH_WHILE_THE_CONSOLE_WAITS";
    /// The file the copy writes what its crash handler did in, a line each time, in order: "<thread> flushed, on disk:
    /// <true or false>" once the session log was flushed (whether it then held that thread's crash), and "<thread> kept
    /// the note, ends the app: <true or false>, crash log: <true or false>" from its stand-in for keeping an F12 note
    /// (whether that thread's crash log was there by then). The copy itself adds "a backtrace could be taken: <true or
    /// false>" once it has tried to take one while std's report of the first crash waits.
    const DONE: &str = "what the crash handler did.txt";

    /// Adds `line` to [`DONE`] in `folder`, in one go, so two threads crashing at once can't mix their lines.
    fn did(folder: &Path, line: &str) {
        use std::io::Write;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(folder.join(DONE));
        if let Ok(mut f) = file {
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    }

    /// The name of the thread this runs on.
    fn this_thread() -> String {
        std::thread::current().name().unwrap_or("unnamed").to_owned()
    }

    /// A file in `folder` that `is_it` names, and that says the crash on the thread this runs on is in it.
    fn holds_this_crash(folder: &Path, is_it: fn(&str) -> bool) -> bool {
        let said = format!("crashed on thread {}\n", this_thread());
        std::fs::read_dir(folder).into_iter().flatten().flatten().any(|entry| {
            is_it(&entry.file_name().to_string_lossy())
                && std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(&said))
        })
    }

    /// A copy of this test program, ended however the test that started it ends, so it never runs on.
    struct Ends(std::process::Child);

    impl Drop for Ends {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// What a copy wrote in `dir`: the session log's text, the crash logs' text, and what the crash handler did, in
    /// order ([`DONE`]).
    fn logs_in(dir: &Path) -> (String, String, Vec<String>) {
        let (mut session, mut crashes) = (String::new(), String::new());
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if log::is_session_log(&name) {
                session += &text;
            } else if log::is_crash_log(&name) {
                crashes += &text;
            }
        }
        let done = std::fs::read_to_string(dir.join(DONE)).unwrap_or_default();
        // Only whole lines: one may be read as it's being added.
        let whole = done.split_inclusive('\n').filter(|l| l.ends_with('\n'));
        (session, crashes, whole.map(|l| l.trim_end_matches('\n').to_owned()).collect())
    }

    /// Only in the copy that the test below runs; in an ordinary run it does nothing. The console (a pipe the test
    /// never reads) is filled, so std's report of a first crash waits there for ever, holding its backtrace lock; then
    /// two more threads crash, one of them called "main" as the window's thread is, so it's taken for a crash that ends
    /// the app. Last, it writes down whether a backtrace can be taken then (it can't, while that lock is held).
    #[test]
    fn crash_while_the_console_waits_in_a_copy() {
        use std::io::Write;
        let Some(dir) = std::env::var_os(IN_THE_COPY).map(PathBuf::from) else { return };
        let _log = log::start(&dir, "A copy of the test program, to crash in");
        static HANDLED: AtomicUsize = AtomicUsize::new(0);
        // The app's own crash handler, with stand-ins that write down what it did, and when.
        let (logs, kept, flushed) = (dir.clone(), dir.clone(), dir.clone());
        let keep_note = move |fatal: bool| {
            let crash_log = holds_this_crash(&kept, log::is_crash_log);
            did(&kept, &format!("{} kept the note, ends the app: {fatal}, crash log: {crash_log}", this_thread()));
        };
        let flush = move || {
            flush_the_log();
            let on_disk = holds_this_crash(&flushed, log::is_session_log);
            did(&flushed, &format!("{} flushed, on disk: {on_disk}", this_thread()));
        };
        set_crash_hook(
            move || logs.clone(),
            keep_note,
            flush,
            |_| {
                HANDLED.fetch_add(1, Ordering::SeqCst);
            },
        );
        // Fills the console, and waits there for ever.
        std::thread::spawn(|| std::io::stderr().write_all("x".repeat(4 * 1024 * 1024).as_bytes()));
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::thread::Builder::new().name("Crash A".into()).spawn(|| panic!("the first crash"));
        // Once the first crash is handled, std's report of it waits on the console, holding the backtrace lock.
        let until = Instant::now() + Duration::from_secs(100);
        while HANDLED.load(Ordering::SeqCst) == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::thread::Builder::new().name("Crash B".into()).spawn(|| panic!("the second crash"));
        let _ = std::thread::Builder::new().name("main".into()).spawn(|| panic!("the crash that ends the app"));
        // What the test stands on: a backtrace can't be taken while std's report of the first crash waits.
        std::thread::sleep(Duration::from_secs(1));
        static TAKEN: AtomicBool = AtomicBool::new(false);
        std::thread::spawn(|| {
            let _ = std::backtrace::Backtrace::force_capture();
            TAKEN.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_secs(2));
        did(&dir, &format!("a backtrace could be taken: {}", TAKEN.load(Ordering::SeqCst)));
        // The test ends this copy once it has seen what it needs to; if it doesn't, the copy ends by itself.
        std::thread::sleep(Duration::from_secs(180));
    }

    #[test]
    fn a_crash_is_written_before_its_backtrace_is_taken() {
        let dir = std::env::temp_dir().join(format!("mo-ui-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("made");
        let copy = std::process::Command::new(std::env::current_exe().expect("this test program"))
            .args(["--exact", "tests::crash_while_the_console_waits_in_a_copy", "--nocapture", "--test-threads=1"])
            .env(IN_THE_COPY, &dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Never read, so it fills up.
            .stderr(Stdio::piped())
            .spawn()
            .expect("the copy started");
        let mut copy = Ends(copy);
        // The crashes, and whether each ends the app.
        let threads = [("Crash A", false), ("Crash B", false), ("main", true)];
        let until = Instant::now() + Duration::from_secs(120);
        let (seen, (session, crashes, done)) = loop {
            let found = logs_in(&dir);
            let (session, crashes, done) = &found;
            // The first crash was handled in full, and std's report of it waits on the console: the other two are
            // written, and their notes kept, though their backtraces wait.
            let written = |t: &str| {
                let said = format!("crashed on thread {t}\n");
                session.contains(&said) && crashes.contains(&said)
            };
            let seen = session.contains("[Crash] Backtrace of the crash on thread Crash A:")
                && written("Crash B")
                && written("main")
                && threads.iter().all(|(t, _)| done.iter().any(|l| l.starts_with(&format!("{t} kept the note"))))
                && done.iter().any(|l| l.starts_with("a backtrace could be taken"));
            let ended = copy.0.try_wait().ok().flatten().is_some();
            if seen || ended || Instant::now() >= until {
                break (seen, found);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        drop(copy);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seen, "the later crashes are written before their backtraces are taken:\n{session}\n---\n{crashes}");
        // Otherwise nothing waited, and the test shows nothing.
        assert!(done.iter().any(|l| l == "a backtrace could be taken: false"), "the console held std's lock: {done:?}");
        // Each crash was on disk in the session log, and in its crash log, before its note was kept, and the note was
        // told whether the crash ends the app.
        for (thread, ends) in threads {
            let at = |line: String| done.iter().position(|l| *l == line);
            let flushed = at(format!("{thread} flushed, on disk: true"));
            let kept = at(format!("{thread} kept the note, ends the app: {ends}, crash log: true"));
            assert!(matches!((flushed, kept), (Some(f), Some(k)) if f < k), "{thread}: {done:?}");
        }
    }

    #[test]
    fn what_catches_a_panic_and_whether_it_ends_the_app() {
        let asked = &std::cell::Cell::new(0);
        let catcher = |c: Catcher| {
            move || {
                asked.set(asked.get() + 1);
                c
            }
        };
        let is =
            |cant: bool, said: &str, thread: &str, c: Catcher| what_it_is(cant, Some(said), Some(thread), catcher(c));
        assert_eq!(is(false, "index out of bounds", "Scan 1", Catcher::Catch), (Catcher::Catch, false));
        assert_eq!(is(false, "index out of bounds", "main", Catcher::Catch), (Catcher::Catch, false));
        assert_eq!(is(false, "index out of bounds", "Scan 1", Catcher::CatchCrash), (Catcher::CatchCrash, false));
        assert_eq!(is(false, "index out of bounds", "main", Catcher::CatchCrash), (Catcher::CatchCrash, false));
        assert_eq!(is(false, "index out of bounds", "Scan 1", Catcher::Nothing), (Catcher::Nothing, false));
        assert_eq!(is(false, "index out of bounds", "main", Catcher::Nothing), (Catcher::Nothing, true));
        assert_eq!(asked.get(), 6, "asked each time");
        // Std ends the app over these whatever was to catch them, and isn't asked: nothing is marked.
        assert_eq!(
            is(true, "words of a check std has added since", "Scan 1", Catcher::Catch),
            (Catcher::Nothing, true)
        );
        assert_eq!(
            is(false, "panic in a destructor during cleanup", "Scan 1", Catcher::Catch),
            (Catcher::Nothing, true)
        );
        assert_eq!(asked.get(), 6, "not asked");
        assert_eq!(what_it_is(false, None, None, catcher(Catcher::Catch)), (Catcher::Catch, false), "said nothing");
    }

    /// Set for the copy of this test program that panics in each way the app catches a panic (see the test below): the
    /// folder it logs into.
    const CATCHING_IN_THE_COPY: &str = "MO_TEST_CAUGHT_PANICS";

    /// A reader tripping over a damaged file, always at this one place in the code.
    fn damaged(said: &str) {
        panic!("{said}");
    }

    /// Only in the copy that the test below runs; in an ordinary run it does nothing. A panic caught each way the app
    /// catches them, each on a thread of its own, one after another, with the app's own crash handler and stand-ins
    /// that write down what it did ([`DONE`]). Last, a panic in a `Drop` as a caught one goes out, which std ends the
    /// copy over.
    #[test]
    fn panics_caught_in_a_copy() {
        use mo_catch::{catch, catch_crash};
        let Some(dir) = std::env::var_os(CATCHING_IN_THE_COPY).map(PathBuf::from) else { return };
        let _log = log::start(&dir, "A copy of the test program, to catch panics in");
        let (logs, kept, flushed) = (dir.clone(), dir.clone(), dir.clone());
        set_crash_hook(
            move || logs.clone(),
            move |fatal| did(&kept, &format!("{} kept the note, ends the app: {fatal}", this_thread())),
            move || {
                flush_the_log();
                did(&flushed, &format!("{} flushed", this_thread()));
            },
            |_| {},
        );
        let on = |thread: &str, work: fn()| {
            let ran = std::thread::Builder::new().name(thread.into()).spawn(work).expect("started").join();
            did(&dir, &format!("{thread} went on: {}", ran.is_ok()));
        };
        // A folder of damaged files, all tripping the reader at one place.
        on("Reader", || {
            for n in 0..12 {
                assert!(catch(|| damaged(&format!("damaged file {n}"))).is_err());
            }
        });
        // On the window's thread too, where a crash nothing catches ends the app.
        on("main", || assert!(catch(|| panic!("damaged file on the window's thread")).is_err()));
        // Words too long for one line.
        on("Long", || assert!(catch(|| panic!("damaged file {}", "x".repeat(5000))).is_err()));
        on("Worker", || assert!(catch_crash(|| panic!("a bug the worker lives through")).is_err()));
        on("main", || assert!(catch_crash(|| panic!("a bug the window lives through")).is_err()));
        on("Inner catch", || {
            let _ = catch_crash(|| assert!(catch(|| panic!("damaged file inside a catch for crashes")).is_err()));
        });
        on("Inner crash", || {
            let _ = catch(|| assert!(catch_crash(|| panic!("a bug inside a catch")).is_err()));
        });
        on("Uncaught", || panic!("a bug nothing catches"));
        // What std says as it ends the app is taken at its word, even in a catch.
        on("Std", || assert!(catch(|| panic!("std::process::exit called re-entrantly")).is_err()));
        // Only std's own words, not a caught panic's that hold them.
        on("Not std", || assert!(catch(|| panic!("damaged file: panic in a function that cannot unwind")).is_err()));
        struct CatchesItsOwn;
        impl Drop for CatchesItsOwn {
            fn drop(&mut self) {
                let _ = catch(|| panic!("damaged file read in a drop"));
            }
        }
        on("Drop", || {
            let first = catch(|| {
                let _drops = CatchesItsOwn;
                panic!("damaged file, and a drop catches its own");
            });
            assert!(first.is_err());
        });
        struct PanicsAsItGoes;
        impl Drop for PanicsAsItGoes {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    panic!("a bug in a drop as a caught panic goes out");
                }
            }
        }
        on("Cleanup", || {
            let _ = catch(|| {
                let _drops = PanicsAsItGoes;
                panic!("damaged file, then a bug in a drop");
            });
        });
        did(&dir, "the copy went on");
    }

    #[test]
    fn a_panic_caught_on_purpose_is_a_warning_and_a_crash_is_still_a_crash() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("mo-ui-caught-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("made");
        let copy = std::process::Command::new(std::env::current_exe().expect("this test program"))
            .args(["--exact", "tests::panics_caught_in_a_copy", "--nocapture", "--test-threads=1"])
            .env(CATCHING_IN_THE_COPY, &dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the copy started");
        let mut copy = Ends(copy);
        // Std's own reports, read as they come so the pipe never fills.
        let mut console = copy.0.stderr.take().expect("piped");
        let reading = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = console.read_to_string(&mut text);
            text
        });
        let until = Instant::now() + Duration::from_secs(120);
        let ended = loop {
            match copy.0.try_wait() {
                Ok(Some(status)) => break Some(status),
                _ if Instant::now() >= until => break None,
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        drop(copy);
        let console = reading.join().unwrap_or_default();
        let (session, crashes, done) = logs_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let all = format!("{session}\n---\n{crashes}\n---\n{done:?}\n---\n{console}");
        assert!(ended.is_some_and(|status| !status.success()), "std ended the copy: {ended:?}\n{all}");
        // What the crash handler did for each thread in turn: its lines up to the one saying whether it went on.
        let mut on_each: Vec<Vec<&str>> = vec![Vec::new()];
        for line in &done {
            on_each.last_mut().expect("one").push(line.as_str());
            if line.contains(" went on: ") {
                on_each.push(Vec::new());
            }
        }
        let thread_did = |n: usize| on_each.get(n).cloned().unwrap_or_default();
        let caught = |said: &str| session.contains(&format!("Warning [Caught] {said} (at "));
        let crashed = |thread: &str, said: &str| {
            let text = format!("crashed on thread {thread}\npanicked at ");
            let both = |log: &str| log.split(&text).skip(1).any(|after| after.lines().nth(1) == Some(said));
            both(&session) && both(&crashes)
        };

        // Damaged files: a warning each, three times at one place and then only counted; nothing more.
        for n in 0..3 {
            assert!(caught(&format!("damaged file {n}")), "{n}: {all}");
        }
        assert!(!caught("damaged file 3") && !caught("damaged file 11"), "counted, not written: {all}");
        assert!(session.contains("times now) damaged file 9 (at "), "the tenth says how many: {all}");
        assert!(session.contains("[Caught] (caught at ") && session.contains(" 10 times now) "), "{all}");
        assert!(caught("damaged file on the window's thread"), "{all}");
        assert!(caught("damaged file inside a catch for crashes"), "{all}");
        assert!(caught("damaged file, and a drop catches its own") && caught("damaged file read in a drop"), "{all}");
        assert!(caught("damaged file, then a bug in a drop"), "{all}");
        assert!(
            !crashes.contains("damaged file") && !console.contains("damaged file"),
            "no crash log, no report: {all}"
        );
        assert!(!session.contains("crashed on thread Reader") && !session.contains("crashed on thread Drop"), "{all}");
        // Nor flushed, nor a note kept for them, on any thread (the window's too), nor taken as the app's end.
        for (n, thread) in [(0, "Reader"), (1, "main"), (2, "Long"), (5, "Inner catch"), (9, "Not std"), (10, "Drop")] {
            assert_eq!(thread_did(n), [format!("{thread} went on: true")], "{thread}: {all}");
        }
        let long = format!("damaged file {}… (cut short) (at ", "x".repeat(MOST_SAID - "damaged file ".len()));
        assert!(session.contains(&format!("Warning [Caught] {long}")), "only so much of what it said: {all}");
        assert!(caught("damaged file: panic in a function that cannot unwind"), "{all}");

        // Bugs: crashes, in full, whatever catches them; only one nothing catches ends its thread.
        for (thread, said) in [
            ("Worker", "a bug the worker lives through"),
            ("main", "a bug the window lives through"),
            ("Inner crash", "a bug inside a catch"),
            ("Uncaught", "a bug nothing catches"),
            ("Std", "std::process::exit called re-entrantly"),
            ("Cleanup", "a bug in a drop as a caught panic goes out"),
        ] {
            assert!(crashed(thread, said), "{thread}: {all}");
            assert!(console.contains(said), "std's report of it: {all}");
        }
        // Each written out, its note kept, and told whether it ends the app: a bug a catch lets the window go on
        // after doesn't; std's own words do.
        for (n, thread, ends, went_on) in [
            (3, "Worker", false, true),
            (4, "main", false, true),
            (6, "Inner crash", false, true),
            (7, "Uncaught", false, false),
            (8, "Std", true, true),
        ] {
            let flushed = format!("{thread} flushed");
            let kept = format!("{thread} kept the note, ends the app: {ends}");
            let went = format!("{thread} went on: {went_on}");
            assert_eq!(thread_did(n), [&flushed, &kept, &flushed, &went], "{thread}: {all}");
        }

        // A panic in a drop as a caught one goes out: no catch can stop std ending the app (on Windows at once,
        // elsewhere once it has said so, a crash that ends the app).
        let mut cleanup = vec!["Cleanup flushed", "Cleanup kept the note, ends the app: false", "Cleanup flushed"];
        if !cfg!(target_env = "msvc") {
            cleanup.extend(["Cleanup flushed", "Cleanup kept the note, ends the app: true", "Cleanup flushed"]);
        }
        assert_eq!(thread_did(11), cleanup, "{all}");
        assert_eq!(on_each.len(), 12, "the copy went no further: {all}");
    }
}
