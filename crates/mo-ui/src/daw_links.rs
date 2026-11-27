//! A little after each start, the app's links in the DAWs (the FL Studio script and the Bitwig extension) and its
//! CLAP plugin are put in or updated by themselves (`mo_app::daw_links` says when), on a low-priority thread of their
//! own, never the window's (rule 12). A card says what went in. Not for the screenshots or tests, which never reach a
//! real DAW's folders (rule 11): only `main` starts it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use mo_app::activity::Feed;
use mo_app::daw_links::{
    self, BITWIG_BY_KEY, BITWIG_LOADED_FROM_KEY, Done, FIRST_LOOK_KEY, FL_BY_KEY, PLUGIN_BY_KEY, PLUGIN_DECLINED_KEY,
    PLUGIN_FAILED_KEY, PLUGIN_TAKEN_OUT_KEY, PluginChoice, PluginDone, Step,
};
use mo_app::{app_settings, background, log};
use mo_dawlink::scripts::{self, Installed, ScriptState};

use crate::settings::{self, Links};

/// How long after the start the look waits, so it never competes with the first pages being read.
const AFTER: Duration = Duration::from_secs(30);

/// This app's version, kept as the one that last wrote a link.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The flag the app is started with, as administrator, to put its CLAP plugin into `Common Files\CLAP`. It takes no
/// paths: the step finds the plugin beside itself and the folder itself, so nothing it's started with can steer it.
pub const PUT_CLAP_PLUGIN: &str = "--put-clap-plugin";

/// The flag the app is started with, as administrator, to take its own CLAP plugin out of `Common Files\CLAP`
/// (Settings › DAWs' Take out). It takes no paths either: only the app's own `DAWmate.clap` there can go.
pub const TAKE_OUT_CLAP_PLUGIN: &str = "--take-out-clap-plugin";

/// Held while a link's files are written or taken out, by the look and by Settings › DAWs alike, so two never write
/// the same part file at once.
static WRITING: Mutex<()> = Mutex::new(());

/// Held while the plugin is put in or taken out, through Windows' question, by the look and by Settings › DAWs alike,
/// so the two never ask at once. Apart from [`WRITING`]: the question can wait a long time for an answer, which mustn't
/// hold up the links' Install and Take out (DK-3).
static PLUGIN_STEP: Mutex<()> = Mutex::new(());

/// Set when Install or Take out was pressed on Settings › DAWs in this run: the look then leaves the links alone.
static CHOSEN: AtomicBool = AtomicBool::new(false);

/// For Settings › DAWs: Install or Take out was pressed, so the look leaves the links as you chose. Returns the guard
/// to hold while the files are written.
pub(crate) fn chosen_by_hand() -> MutexGuard<'static, ()> {
    CHOSEN.store(true, Ordering::Relaxed);
    WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// For Settings › DAWs' plugin row: Install or Take out was pressed, so the look leaves the links and the plugin as you
/// chose. Returns the guard to hold while Windows is asked: it waits (on its own thread, never the window's) while the
/// look is asking already.
pub(crate) fn plugin_by_hand() -> MutexGuard<'static, ()> {
    CHOSEN.store(true, Ordering::Relaxed);
    PLUGIN_STEP.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the look keeps in the settings once it's done.
#[derive(Debug, Default)]
struct Keep {
    first_look: bool,
    fl: bool,
    bitwig: bool,
    plugin: bool,
    declined: bool,
    failed: Option<String>,
    loaded_from: Option<String>,
    /// A test plugin is in: the mark that a version put the app's own in goes, so that one goes back in once the test
    /// one is out (`tools/dev/install-test-plugin.ps1` takes the app's own out).
    forget_plugin: bool,
}

/// Starts the look. `library`: the song library, for the FL Studio project folders that may have FL's user data
/// folder above them.
pub fn keep_up_to_date(library: PathBuf, feed: Feed) {
    let installed = std::env::current_exe().is_ok_and(|exe| mo_app::update::installed(&exe));
    if !installed {
        log::info("DAW links", "A launcher build: the DAWs' links are left to Settings \u{203a} DAWs");
        return;
    }
    if !settings::writable() {
        log::warning("DAW links", "The settings can't be kept this run, so the DAWs' links are left as they are");
        return;
    }
    let value = |key: &str| settings::value(key).filter(|v| !v.trim().is_empty());
    let chosen = value(app_settings::BITWIG_FOLDER_KEY);
    let first_look = value(FIRST_LOOK_KEY).is_none();
    let newer = |key: &str| daw_links::newer_than_writer(VERSION, value(key).as_deref());
    let (fl_newer, bw_newer) = (newer(FL_BY_KEY), newer(BITWIG_BY_KEY));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let plugin = Plugin {
        before: value(PLUGIN_BY_KEY).is_some(),
        newer: newer(PLUGIN_BY_KEY),
        declined: value(PLUGIN_DECLINED_KEY).is_some_and(|v| v == VERSION),
        failed_today: value(PLUGIN_FAILED_KEY).is_some_and(|v| v == daw_links::failed_mark(VERSION, &today)),
        taken_out: value(PLUGIN_TAKEN_OUT_KEY).is_some(),
    };
    let kept_loaded_from = value(BITWIG_LOADED_FROM_KEY);
    let started = background::spawn("DAW links", move || {
        std::thread::sleep(AFTER);
        if CHOSEN.load(Ordering::Relaxed) {
            log::info("DAW links", "Settings \u{203a} DAWs changed the links this run, so they're left as they are");
            return;
        }
        // Where Bitwig loads the extension from, asked of the link when Bitwig is open, else as last seen.
        let asked = mo_dawlink::bitwig::Bitwig::default().check().ok().map(|h| h.loaded_from).filter(|f| !f.is_empty());
        let loaded_from = asked.clone().or_else(|| kept_loaded_from.clone());
        let mut keep =
            Keep { first_look, loaded_from: asked.filter(|a| Some(a) != kept_loaded_from.as_ref()), ..Keep::default() };
        let mut done = Done::default();
        {
            let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if CHOSEN.load(Ordering::Relaxed) {
                return;
            }
            let links = settings::find_links(&library, chosen.as_deref(), loaded_from.as_deref());
            put_in(&links, first_look, (fl_newer, bw_newer), &mut done);
        }
        // After the links' files, not while they're held: the plugin step writes none of them, and Windows' question
        // can wait a long time for an answer, which would hold up Install and Take out on Settings › DAWs (DK-3).
        let (step, asked) = put_in_plugin(&plugin, &feed);
        match asked {
            Asked::SaidNo => keep.declined = true,
            Asked::Failed => keep.failed = Some(daw_links::failed_mark(VERSION, &today)),
            Asked::TestPlugin => keep.forget_plugin = plugin.before,
            Asked::Not | Asked::Went => {}
        }
        done.plugin = step;
        if let Some((title, detail)) = daw_links::card(&done) {
            feed.told(&title, &detail);
        }
        // Kept only when every folder took it: a folder Bitwig had open is tried again at the next start (DK-1).
        keep.fl = done.fl.as_ref().is_some_and(|(_, d)| daw_links::went_everywhere(d));
        keep.bitwig = done.bitwig.as_ref().is_some_and(|(_, d)| daw_links::went_everywhere(&d.installed));
        keep.plugin = done.plugin.as_ref().is_some_and(|(_, d)| daw_links::went_everywhere(d));
        let _ = slint::invoke_from_event_loop(move || remember(&keep));
    });
    if let Err(e) = started {
        log::error("DAW links", &format!("Couldn't start the look at the DAWs' links: {e}"));
    }
}

/// Keeps what the look did in the settings, on the window's thread.
fn remember(keep: &Keep) {
    let mut kept = true;
    let mut put = |key: &str, value: &str| kept &= settings::remember(key, value);
    if keep.first_look {
        put(FIRST_LOOK_KEY, "done");
    }
    for (wrote, key) in [(keep.fl, FL_BY_KEY), (keep.bitwig, BITWIG_BY_KEY), (keep.plugin, PLUGIN_BY_KEY)] {
        if wrote {
            put(key, VERSION);
        }
    }
    if keep.forget_plugin {
        // Blank counts as none (see keep_up_to_date's `value`).
        put(PLUGIN_BY_KEY, "");
    }
    if keep.declined {
        put(PLUGIN_DECLINED_KEY, VERSION);
    }
    if let Some(failed) = &keep.failed {
        put(PLUGIN_FAILED_KEY, failed);
    }
    if let Some(from) = &keep.loaded_from {
        put(BITWIG_LOADED_FROM_KEY, from);
    }
    if !kept {
        log::warning("DAW links", "Couldn't keep in the settings what the look at the DAWs did");
    }
}

/// Puts in or updates each link as [`daw_links::step`] says. `newer`: whether this app is newer than the one that
/// last wrote FL's link, and Bitwig's.
///
/// Only the folders whose copy isn't the app's own yet are written: after a start when one copy couldn't be (Bitwig
/// had it open), the next writes only that one, not the ones that went in again (DK-1).
fn put_in(links: &Links, first_look: bool, newer: (bool, bool), done: &mut Done) {
    let fl = daw_links::step(links.fl_state, first_look, !links.fl_folders.is_empty(), newer.0);
    log::info("DAW links", &format!("FL Studio script: {:?}, so {fl:?}", links.fl_state));
    if fl != Step::Leave {
        let not_yet: Vec<PathBuf> = links
            .fl_folders
            .iter()
            .filter(|f| scripts::fl_state(std::slice::from_ref(f)) != ScriptState::Installed)
            .cloned()
            .collect();
        let installed = scripts::install_fl(&not_yet);
        settings::log_fl_install(&installed, "DAW links");
        done.fl = Some((fl, installed));
    }
    // Only into Bitwig user folders that are there: a chosen folder on a drive that isn't plugged in would stop the
    // whole install (it's all or nothing), and a folder that isn't there isn't Bitwig's.
    let users: Vec<PathBuf> = links.bw_users.iter().filter(|u| u.is_dir()).cloned().collect();
    let folders: Vec<PathBuf> =
        links.bw_folders.iter().filter(|f| f.parent().is_some_and(Path::is_dir)).cloned().collect();
    let Some(source) = &links.bw_source else {
        log::info("DAW links", "No Bitwig extension beside the app, so Bitwig's is left as it is");
        return;
    };
    let bw = daw_links::step(links.bw_state, first_look, !folders.is_empty(), newer.1);
    log::info("DAW links", &format!("Bitwig extension: {:?}, so {bw:?}", links.bw_state));
    if bw != Step::Leave {
        let not_yet: Vec<PathBuf> = folders
            .iter()
            .filter(|f| scripts::bitwig_state(source, std::slice::from_ref(f)) != ScriptState::Installed)
            .cloned()
            .collect();
        let installed = scripts::install_bitwig(source, &not_yet, &users);
        settings::log_bitwig_install(&installed, "DAW links");
        done.bitwig = Some((bw, installed));
    }
}

/// The shared CLAP folder every DAW looks in, `Common Files\CLAP` (Solomon, 2026-11-25: here, with Windows' Yes).
/// Windows only: the installer that puts the plugin beside the app is Windows only.
pub(crate) fn clap_folder() -> Option<PathBuf> {
    std::env::var_os("CommonProgramW6432")
        .or_else(|| std::env::var_os("CommonProgramFiles"))
        .map(|c| PathBuf::from(c).join("CLAP"))
}

/// The user's own CLAP folder, which DAWs look in too: only looked at for a test plugin put in by hand there.
fn own_clap_folder() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs").join("Common").join("CLAP"))
}

/// A test plugin put in by hand (`tools/dev/install-test-plugin.ps1`), in the shared CLAP folder `folder` or the
/// user's own: the same plugin as the app's (one ID), so a DAW would list the two twice.
pub(crate) fn test_plugin(folder: &Path) -> Option<PathBuf> {
    [Some(folder.to_owned()), own_clap_folder()]
        .into_iter()
        .flatten()
        .flat_map(|f| scripts::TEST_CLAP_PLUGIN_NAMES.map(|n| f.join(n)))
        .find(|f| f.is_file())
}

/// A file's size and when it was last written, to tell a changed one without reading it.
type Stamp = Option<(u64, Option<std::time::SystemTime>)>;

fn stamp(file: &Path) -> Stamp {
    std::fs::metadata(file).ok().map(|m| (m.len(), m.modified().ok()))
}

/// The plugin's state as last worked out, by both copies' stamps: Settings › DAWs looks each time it shows, and the
/// plugin is megabytes, so it's read again only when one of them changed (rule 12, read big files once).
static CLAP_STATE: Mutex<Option<((Stamp, Stamp), ScriptState)>> = Mutex::new(None);

/// [`scripts::clap_state`], kept by the two files' stamps.
fn clap_state_kept(source: &Path, folder: &Path) -> ScriptState {
    let stamps = (stamp(source), stamp(&folder.join(scripts::CLAP_PLUGIN_NAME)));
    let mut kept = CLAP_STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((was, state)) = *kept
        && was == stamps
        && stamps.0.is_some()
        && stamps.1.is_some()
    {
        return state;
    }
    let state = scripts::clap_state(source, folder);
    *kept = Some((stamps, state));
    state
}

/// How the app's CLAP plugin stands, for Settings › DAWs' row: reads the disk, so off the window's thread.
pub(crate) fn plugin_look() -> mo_app::settings::PluginLook {
    let source = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe));
    let folder = clap_folder();
    let (state, test_plugin, files) = match &folder {
        Some(f) => (
            source.as_deref().map_or(ScriptState::NotInstalled, |s| clap_state_kept(s, f)),
            test_plugin(f),
            scripts::clap_installed_files(f),
        ),
        None => (ScriptState::NotInstalled, None, Vec::new()),
    };
    mo_app::settings::PluginLook {
        source: source.is_some(),
        folder,
        state,
        test_plugin,
        files,
        can_ask: mo_space::admin::Admin::can_ask(&mo_space::admin::AsAdmin),
    }
}

/// The administrator step: puts the plugin beside this program into `Common Files\CLAP` (written beside itself, read
/// back, then put in place). Ends with 0 when it went in.
pub fn put_plugin_as_admin() -> i32 {
    let source = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe));
    let (Some(source), Some(folder)) = (source, clap_folder()) else { return 2 };
    if scripts::install_clap(&source, &folder).went.is_empty() { 1 } else { 0 }
}

/// The administrator step for Take out: takes the app's own plugin (`DAWmate.clap`, and a part file a cut-off install
/// left) out of `Common Files\CLAP`, never a test plugin or anything else there (`scripts::clap_installed_files`).
/// Ends with 0 when nothing of the app's is left there.
pub fn take_plugin_out_as_admin() -> i32 {
    let Some(folder) = clap_folder() else { return 2 };
    let done = scripts::take_out(&scripts::clap_installed_files(&folder));
    i32::from(!done.failed.is_empty() || !scripts::clap_installed_files(&folder).is_empty())
}

/// Settings › DAWs' Install (or Update) and Take out on the plugin row, on a thread of its own (never the window's:
/// Windows' question waits for you). Never while a program has the plugin loaded, as the put-in look does; then the
/// administrator step, and what's in the folder after it says how it went.
pub(crate) fn plugin_by_choice(choice: PluginChoice) -> PluginDone {
    let _asking = plugin_by_hand();
    let Some(folder) = clap_folder() else { return PluginDone::Failed("there's no Common Files\\CLAP folder".into()) };
    let source = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe));
    let plugin = folder.join(scripts::CLAP_PLUGIN_NAME);
    match choice {
        PluginChoice::Install => {
            let Some(source) = &source else {
                return PluginDone::Failed("this copy of the app has no plugin beside it to put in".into());
            };
            // The same plugin: a DAW would list the two twice.
            if let Some(test) = test_plugin(&folder) {
                return PluginDone::Failed(format!("a test plugin is in ({}): delete it first", test.display()));
            }
            // Done already: the look at the start may have put it in while this waited for its turn.
            if scripts::clap_state(source, &folder) == ScriptState::Installed {
                return PluginDone::Went;
            }
        }
        PluginChoice::TakeOut => {
            if !scripts::clap_plugin_there(&folder) && scripts::clap_installed_files(&folder).is_empty() {
                return PluginDone::Went;
            }
            // Something under the plugin's name that the app never takes out (a link, or in a linked folder):
            // Windows isn't asked for a step that would leave it.
            if let Some(why) = not_taken_out(&folder) {
                return PluginDone::Failed(why);
            }
        }
    }
    if in_use(&plugin) {
        log::info("Settings", "A program has the CLAP plugin loaded, so Windows isn't asked");
        return PluginDone::InUse;
    }
    let admin = mo_space::admin::AsAdmin;
    if !mo_space::admin::Admin::can_ask(&admin) {
        return PluginDone::Failed("this run of the app can't ask Windows".into());
    }
    let flag = match choice {
        PluginChoice::Install => PUT_CLAP_PLUGIN,
        PluginChoice::TakeOut => TAKE_OUT_CLAP_PLUGIN,
    };
    let asked = mo_space::admin::Admin::ask(&admin, &[flag.to_owned()]);
    // What's there now counts, not the step's word.
    let done = match asked {
        Err(why) if why.starts_with("You said no") => PluginDone::SaidNo,
        Err(why) => PluginDone::Failed(why),
        Ok(()) => match (choice, &source) {
            (PluginChoice::Install, Some(source)) if scripts::clap_state(source, &folder) == ScriptState::Installed => {
                PluginDone::Went
            }
            // Out only when nothing at all is there under its name, not just nothing the app may take out.
            (PluginChoice::TakeOut, _) if !scripts::clap_plugin_there(&folder) => PluginDone::Went,
            (PluginChoice::TakeOut, _) => not_taken_out(&folder).map_or(PluginDone::NotChanged, PluginDone::Failed),
            _ => PluginDone::NotChanged,
        },
    };
    log::info("Settings", &format!("CLAP plugin, {choice:?} in {}: {done:?}", folder.display()));
    done
}

/// Why Take out can't take out what's under the plugin's name in `folder`, when it's there but isn't the app's own
/// plain file (`scripts::clap_installed_files`): it's a link, or reached through one, and the app never removes through
/// a link (rule 28). None when there's nothing there or it's the app's own.
fn not_taken_out(folder: &Path) -> Option<String> {
    let plugin = folder.join(scripts::CLAP_PLUGIN_NAME);
    let own = scripts::clap_installed_files(folder).contains(&plugin);
    (scripts::clap_plugin_there(folder) && !own).then(|| {
        format!(
            "{} is a link, or in a linked folder, and the app never takes anything out through a link: take it out by \
             hand",
            plugin.display()
        )
    })
}

/// What the look knows of the plugin before it looks.
struct Plugin {
    /// A version has put it in before.
    before: bool,
    /// This app is newer than the one that last put it in.
    newer: bool,
    /// You said no to Windows' question for this version.
    declined: bool,
    /// The administrator step couldn't put this version in today.
    failed_today: bool,
    /// You took it out on Settings › DAWs: it stays out until Install there.
    taken_out: bool,
}

/// How Windows' question went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Not,
    Went,
    SaidNo,
    Failed,
    /// Not asked: a test plugin was put in by hand.
    TestPlugin,
}

/// Puts in or updates the CLAP plugin the installer put beside the app, as [`daw_links::plugin_step`] says, in
/// `Common Files\CLAP`, which needs Windows' Yes. Not asked while a program has the plugin loaded (then at a later
/// start), for a version you said no to, or again the same day after the step failed. Left alone when a test plugin
/// was put in by hand (the same plugin: a DAW would list it twice); [`Asked::TestPlugin`] then has the look forget that
/// a version put the app's own in, as the test script took it out (DK-2).
fn put_in_plugin(plugin: &Plugin, feed: &Feed) -> (Option<(Step, Installed)>, Asked) {
    let not = (None, Asked::Not);
    let Some(source) = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe)) else {
        return not;
    };
    let Some(folder) = clap_folder() else { return not };
    // One question at a time, and none once the plugin row on Settings › DAWs was used this run (your choice).
    let _asking = PLUGIN_STEP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if CHOSEN.load(Ordering::Relaxed) {
        return not;
    }
    if let Some(by_hand) = test_plugin(&folder) {
        log::info(
            "DAW links",
            &format!(
                "The plugin is left alone: {} is there already. The app's own goes back in once it's gone",
                by_hand.display()
            ),
        );
        return (None, Asked::TestPlugin);
    }
    let state = scripts::clap_state(&source, &folder);
    let step = daw_links::plugin_step(state, plugin.before, plugin.newer, plugin.taken_out);
    log::info("DAW links", &format!("CLAP plugin: {state:?}, so {step:?}"));
    if step == Step::Leave {
        return not;
    }
    if plugin.declined {
        log::info("DAW links", "You said no to putting this version's plugin in, so Windows isn't asked again");
        return not;
    }
    if plugin.failed_today {
        log::info("DAW links", "The plugin couldn't go in earlier today, so Windows is asked again another day");
        return not;
    }
    if in_use(&folder.join(scripts::CLAP_PLUGIN_NAME)) {
        log::info("DAW links", "A program has the CLAP plugin loaded: it's updated at a start when it's closed");
        return not;
    }
    let admin = mo_space::admin::AsAdmin;
    if !mo_space::admin::Admin::can_ask(&admin) {
        return not;
    }
    let job = feed.start(
        "Windows asks to let DAWmate put its plugin in",
        &format!("It goes in {}, where every DAW finds it. Say No to leave it for now.", folder.display()),
    );
    let asked = mo_space::admin::Admin::ask(&admin, &[PUT_CLAP_PLUGIN.to_owned()]);
    // What's there now counts, not the step's word.
    let mut done = Installed::default();
    let outcome = match asked {
        Err(why) if why.starts_with("You said no") => {
            log::info("DAW links", "You said no to Windows' question for the CLAP plugin");
            job.done_as("The plugin wasn't put in", "You said no. The next version asks again.");
            return (None, Asked::SaidNo);
        }
        Err(why) => {
            done.failed.push((folder.clone(), why));
            Asked::Failed
        }
        Ok(()) if scripts::clap_state(&source, &folder) == ScriptState::Installed => {
            done.went.push(folder.clone());
            Asked::Went
        }
        Ok(()) => {
            done.failed.push((folder.clone(), "it isn't the app's copy after the step".to_owned()));
            Asked::Failed
        }
    };
    for f in &done.went {
        log::info("DAW links", &format!("Put the CLAP plugin in {}", f.display()));
    }
    for (f, why) in &done.failed {
        log::warning("DAW links", &format!("Couldn't put the CLAP plugin in {}: {why}", f.display()));
    }
    if outcome == Asked::Went {
        // The card that follows says what to do next.
        job.done("Done");
    } else {
        job.failed_as("Couldn't put the plugin in", "Close your DAWs: the app tries again at a start on another day.");
    }
    (Some((step, done)), outcome)
}

/// Whether a program has the plugin open or loaded now (a DAW, or its plug-in host): Windows then lets nobody replace
/// it, so asking for administrator rights would only fail. Asked of Windows' Restart Manager, which sees a plugin a
/// program has loaded, not only one it has open. False when it can't tell.
#[cfg(windows)]
#[allow(unsafe_code)]
fn in_use(file: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{
        CCH_RM_SESSION_KEY, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
    };
    const ERROR_MORE_DATA: u32 = 234;
    if !file.is_file() {
        return false;
    }
    let wide: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: the out pointers point at locals that outlive the call; the key buffer is CCH_RM_SESSION_KEY + 1 long,
    // as the call asks.
    if unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) } != 0 {
        return false;
    }
    let names = [wide.as_ptr()];
    // SAFETY: one file name, a NUL-ended wide string alive for the call; no programs or services.
    let registered =
        unsafe { RmRegisterResources(session, 1, names.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) } == 0;
    let (mut needed, mut count, mut reasons) = (0u32, 0u32, 0u32);
    let listed = if registered {
        // SAFETY: asked with room for none, so only the counts are written, into locals.
        Some(unsafe { RmGetList(session, &mut needed, &mut count, std::ptr::null_mut(), &mut reasons) })
    } else {
        None
    };
    // SAFETY: the session this call started.
    unsafe { RmEndSession(session) };
    matches!(listed, Some(r) if r == ERROR_MORE_DATA || (r == 0 && needed > 0))
}

#[cfg(not(windows))]
fn in_use(_file: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under the build folder, empty (tests never reach a real DAW's folders: rule 11).
    fn fixture(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/mo-ui-daw-links").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The 26 Nov review's DK-1: a start when Bitwig had one copy open put the new one only in the others, so the next
    /// writes only the copy that isn't the app's own yet, and says nothing more of the ones that went in.
    #[test]
    fn a_retry_writes_only_the_copies_that_arent_the_apps_own_yet() {
        let root = fixture("retry");
        let source = root.join(scripts::BITWIG_EXTENSION_NAME);
        std::fs::write(&source, "the app's extension").unwrap();
        let (k, documents) = (root.join("K").join("Extensions"), root.join("Documents").join("Extensions"));
        for (folder, copy) in [(&k, "the app's extension"), (&documents, "an older extension")] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join(scripts::BITWIG_EXTENSION_NAME), copy).unwrap();
        }
        let fl = root.join("FL").join("Settings").join("Hardware").join(scripts::FL_FOLDER);
        std::fs::create_dir_all(&fl).unwrap();
        std::fs::write(fl.join(scripts::FL_SCRIPT_NAME), scripts::FL_SCRIPT).unwrap();
        let bw_folders = vec![k.clone(), documents.clone()];
        let links = Links {
            fl_folders: vec![fl.clone()],
            fl_state: scripts::fl_state(std::slice::from_ref(&fl)),
            bw_users: vec![root.join("K"), root.join("Documents")],
            bw_state: scripts::bitwig_state(&source, &bw_folders),
            bw_folders,
            bw_source: Some(source.clone()),
        };
        assert_eq!((links.fl_state, links.bw_state), (ScriptState::Installed, ScriptState::Outdated));
        let mut done = Done::default();
        put_in(&links, false, (true, true), &mut done);
        assert!(done.fl.is_none(), "FL's script is the app's own: left as it is");
        let (step, bitwig) = done.bitwig.expect("Bitwig's copy in Documents is older");
        assert_eq!(step, Step::Update);
        assert_eq!(bitwig.installed.went, std::slice::from_ref(&documents), "only the copy that was older");
        assert!(daw_links::went_everywhere(&bitwig.installed));
        assert_eq!(std::fs::read(documents.join(scripts::BITWIG_EXTENSION_NAME)).unwrap(), b"the app's extension");
    }
    /// Rule 12: the plugin's state is worked out again only when one of the two copies changed (its size or time), and
    /// then it's right again (the review of 27 Nov).
    #[test]
    fn the_plugins_state_is_kept_until_a_copy_changes() {
        let root = fixture("clap-kept");
        let source = root.join("plugins").join(scripts::CLAP_PLUGIN_NAME);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"plugin 2").unwrap();
        let folder = root.join("CLAP");
        assert_eq!(clap_state_kept(&source, &folder), ScriptState::NotInstalled);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(scripts::CLAP_PLUGIN_NAME), b"plugin 2").unwrap();
        assert_eq!(clap_state_kept(&source, &folder), ScriptState::Installed);
        assert_eq!(clap_state_kept(&source, &folder), ScriptState::Installed, "kept");
        std::fs::write(&source, b"plugin 30").unwrap();
        assert_eq!(clap_state_kept(&source, &folder), ScriptState::Outdated, "the app's copy changed");
        std::fs::remove_file(folder.join(scripts::CLAP_PLUGIN_NAME)).unwrap();
        assert_eq!(clap_state_kept(&source, &folder), ScriptState::NotInstalled, "taken out");
    }

    /// Take out never counts a DAWmate.clap that is a link as gone, and never takes it out (the review of 27 Nov).
    #[cfg(unix)]
    #[test]
    fn a_plugin_that_is_a_link_is_never_taken_out() {
        let root = fixture("clap-linked");
        let folder = root.join("CLAP");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(not_taken_out(&folder), None, "nothing there");
        std::fs::write(folder.join(scripts::CLAP_PLUGIN_NAME), b"plugin").unwrap();
        assert_eq!(not_taken_out(&folder), None, "the app's own plain file");
        std::fs::remove_file(folder.join(scripts::CLAP_PLUGIN_NAME)).unwrap();
        let elsewhere = root.join("elsewhere.clap");
        std::fs::write(&elsewhere, b"someone's").unwrap();
        std::os::unix::fs::symlink(&elsewhere, folder.join(scripts::CLAP_PLUGIN_NAME)).unwrap();
        let why = not_taken_out(&folder).expect("a link is never taken out");
        assert!(why.contains("is a link, or in a linked folder"), "{why}");
        assert!(elsewhere.exists());
    }
}
