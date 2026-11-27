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
    PLUGIN_FAILED_KEY, Step,
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

/// Held while a link's files are written or taken out, by the look and by Settings › DAWs alike, so two never write
/// the same part file at once.
static WRITING: Mutex<()> = Mutex::new(());

/// Set when Install or Take out was pressed on Settings › DAWs in this run: the look then leaves the links alone.
static CHOSEN: AtomicBool = AtomicBool::new(false);

/// For Settings › DAWs: Install or Take out was pressed, so the look leaves the links as you chose. Returns the guard
/// to hold while the files are written.
pub(crate) fn chosen_by_hand() -> MutexGuard<'static, ()> {
    CHOSEN.store(true, Ordering::Relaxed);
    WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
}

/// Starts the look. `library`: the song library, for the FL Studio project folders that may have FL's user data
/// folder above them.
pub fn keep_up_to_date(library: PathBuf, feed: Feed) {
    // The installer's copy on Windows; on a Mac, the app run from its app bundle (installer/mac), which brings the
    // extension and the plugin inside it.
    let installed = std::env::current_exe()
        .is_ok_and(|exe| mo_app::update::installed(&exe) || scripts::mac_app_contents(&exe).is_some());
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
            let (step, asked) = put_in_plugin(&plugin, &feed);
            match asked {
                Asked::SaidNo => keep.declined = true,
                Asked::Failed => keep.failed = Some(daw_links::failed_mark(VERSION, &today)),
                Asked::Not | Asked::Went => {}
            }
            done.plugin = step;
        }
        if let Some((title, detail)) = daw_links::card(&done) {
            feed.told(&title, &detail);
        }
        keep.fl = done.fl.as_ref().is_some_and(|(_, d)| !d.went.is_empty());
        keep.bitwig = done.bitwig.as_ref().is_some_and(|(_, d)| !d.installed.went.is_empty());
        keep.plugin = done.plugin.as_ref().is_some_and(|(_, d)| !d.went.is_empty());
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
fn put_in(links: &Links, first_look: bool, newer: (bool, bool), done: &mut Done) {
    let fl = daw_links::step(links.fl_state, first_look, !links.fl_folders.is_empty(), newer.0);
    log::info("DAW links", &format!("FL Studio script: {:?}, so {fl:?}", links.fl_state));
    if fl != Step::Leave {
        let installed = scripts::install_fl(&links.fl_folders);
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
        let installed = scripts::install_bitwig(source, &folders, &users);
        settings::log_bitwig_install(&installed, "DAW links");
        done.bitwig = Some((bw, installed));
    }
}

/// The shared CLAP folder every DAW looks in: on Windows `Common Files\CLAP` (Solomon, 2026-11-25: here, with Windows'
/// Yes); on a Mac the user's own `~/Library/Audio/Plug-Ins/CLAP`, which needs no administrator. Elsewhere none: the app
/// brings no plugin with it there.
fn clap_folder() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("CommonProgramW6432")
            .or_else(|| std::env::var_os("CommonProgramFiles"))
            .map(|c| PathBuf::from(c).join("CLAP"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Audio").join("Plug-Ins").join("CLAP"))
    } else {
        None
    }
}

/// The other CLAP folder DAWs look in: only looked at for a test plugin put in by hand there. On Windows the user's
/// own; on a Mac the one for every user (`/Library/Audio/Plug-Ins/CLAP`).
fn own_clap_folder() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"))
    } else {
        std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs").join("Common").join("CLAP"))
    }
}

/// The administrator step: puts the plugin beside this program into `Common Files\CLAP` (written beside itself, read
/// back, then put in place). Ends with 0 when it went in.
pub fn put_plugin_as_admin() -> i32 {
    let source = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe));
    let (Some(source), Some(folder)) = (source, clap_folder()) else { return 2 };
    if scripts::install_clap(&source, &folder).went.is_empty() { 1 } else { 0 }
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
}

/// How Windows' question went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Not,
    Went,
    SaidNo,
    Failed,
}

/// Puts in or updates the CLAP plugin the installer put beside the app, as [`daw_links::plugin_step`] says, in
/// `Common Files\CLAP`, which needs Windows' Yes. Not asked while a program has the plugin loaded (then at a later
/// start), for a version you said no to, or again the same day after the step failed. Left alone when a test plugin
/// was put in by hand (the same plugin: a DAW would list it twice).
fn put_in_plugin(plugin: &Plugin, feed: &Feed) -> (Option<(Step, Installed)>, Asked) {
    let not = (None, Asked::Not);
    let Some(source) = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe)) else {
        return not;
    };
    let Some(folder) = clap_folder() else { return not };
    let by_hand = [Some(folder.clone()), own_clap_folder()]
        .into_iter()
        .flatten()
        .flat_map(|f| scripts::TEST_CLAP_PLUGIN_NAMES.map(|n| f.join(n)))
        // A file on Windows, a bundle folder on a Mac.
        .find(|f| f.exists());
    if let Some(by_hand) = by_hand {
        log::info("DAW links", &format!("The plugin is left alone: {} is there already", by_hand.display()));
        return not;
    }
    let state = scripts::clap_state(&source, &folder);
    let step = daw_links::plugin_step(state, plugin.before, plugin.newer);
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
    // On a Mac the user's own CLAP folder takes it without asking anyone (a DAW that has the old one loaded keeps it
    // until it loads plugins again).
    if !cfg!(windows) {
        let done = scripts::install_clap(&source, &folder);
        for f in &done.went {
            log::info("DAW links", &format!("Put the CLAP plugin in {}", f.display()));
        }
        for (f, why) in &done.failed {
            log::warning("DAW links", &format!("Couldn't put the CLAP plugin in {}: {why}", f.display()));
        }
        let asked = if done.went.is_empty() { Asked::Failed } else { Asked::Went };
        return (Some((step, done)), asked);
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
