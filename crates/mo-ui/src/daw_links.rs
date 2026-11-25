//! A little after each start, the app's links in the DAWs (the FL Studio script and the Bitwig extension) are put in
//! or updated by themselves (`mo_app::daw_links` says when), on a low-priority thread of their own, never the
//! window's (rule 12). A card says what went in. Not for the screenshots or tests, which never reach a real DAW's
//! folders (rule 11): only `main` starts it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mo_app::activity::Feed;
use mo_app::daw_links::{self, Done, FIRST_LOOK_KEY, PLUGIN_DECLINED_KEY, Step, WRITTEN_BY_KEY};
use mo_app::{app_settings, background, log};
use mo_dawlink::scripts::{self, ScriptState};

use crate::settings::{self, Links};

/// How long after the start the look waits, so it never competes with the first pages being read.
const AFTER: Duration = Duration::from_secs(30);

/// This app's version, kept as the one that last wrote the links.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Starts the look. `library`: the song library, for the FL Studio project folders that may have FL's user data
/// folder above them.
pub fn keep_up_to_date(library: PathBuf, feed: Feed) {
    let installed = std::env::current_exe().is_ok_and(|exe| mo_app::update::installed(&exe));
    if !installed {
        log::info("DAW links", "A launcher build: the DAWs' links are left to Settings \u{203a} DAWs");
        return;
    }
    let chosen = settings::value(app_settings::BITWIG_FOLDER_KEY);
    let first_look = settings::value(FIRST_LOOK_KEY).is_none();
    let newer = daw_links::newer_than_writer(VERSION, settings::value(WRITTEN_BY_KEY).as_deref());
    let declined = settings::value(PLUGIN_DECLINED_KEY).is_some_and(|v| v == VERSION);
    let started = background::spawn("DAW links", move || {
        std::thread::sleep(AFTER);
        let links = settings::find_links(&library, chosen.as_deref(), None);
        let mut done = put_in(&links, first_look, newer);
        let (plugin, asked) = put_in_plugin(first_look, newer, declined, &feed);
        done.plugin = plugin;
        if let Some((title, detail)) = daw_links::card(&done) {
            feed.told(&title, &detail);
        }
        let wrote = done.fl.as_ref().is_some_and(|(_, d)| !d.went.is_empty())
            || done.bitwig.as_ref().is_some_and(|(_, d)| !d.installed.went.is_empty())
            || done.plugin.as_ref().is_some_and(|(_, d)| !d.went.is_empty());
        let said_no = asked == PluginAsk::SaidNo;
        if first_look || wrote || said_no {
            let _ = slint::invoke_from_event_loop(move || {
                let mut kept = true;
                if first_look {
                    kept &= settings::remember(FIRST_LOOK_KEY, "done");
                }
                if wrote {
                    kept &= settings::remember(WRITTEN_BY_KEY, VERSION);
                }
                if said_no {
                    kept &= settings::remember(PLUGIN_DECLINED_KEY, VERSION);
                }
                if !kept {
                    log::warning("DAW links", "Couldn't keep in the settings what the look at the DAWs did");
                }
            });
        }
    });
    if let Err(e) = started {
        log::error("DAW links", &format!("Couldn't start the look at the DAWs' links: {e}"));
    }
}

/// Puts in or updates each link as [`daw_links::step`] says.
fn put_in(links: &Links, first_look: bool, newer: bool) -> Done {
    let mut done = Done::default();
    let fl = daw_links::step(links.fl_state, first_look, !links.fl_folders.is_empty(), newer);
    log::info("DAW links", &format!("FL Studio script: {:?}, so {fl:?}", links.fl_state));
    if fl != Step::Leave {
        let installed = scripts::install_fl(&links.fl_folders);
        settings::log_fl_install(&installed);
        done.fl = Some((fl, installed));
    }
    // Only into Bitwig user folders that are there: a chosen folder on a drive that isn't plugged in would stop the
    // whole install (it's all or nothing), and a folder that isn't there isn't Bitwig's.
    let users: Vec<PathBuf> = links.bw_users.iter().filter(|u| u.is_dir()).cloned().collect();
    let folders: Vec<PathBuf> =
        links.bw_folders.iter().filter(|f| f.parent().is_some_and(Path::is_dir)).cloned().collect();
    let Some(source) = &links.bw_source else {
        log::info("DAW links", "No Bitwig extension beside the app, so Bitwig's is left as it is");
        return done;
    };
    let bw = daw_links::step(links.bw_state, first_look, !folders.is_empty(), newer);
    log::info("DAW links", &format!("Bitwig extension: {:?}, so {bw:?}", links.bw_state));
    if bw != Step::Leave {
        let installed = scripts::install_bitwig(source, &folders, &users);
        settings::log_bitwig_install(&installed);
        done.bitwig = Some((bw, installed));
    }
    done
}

/// The flag the app is started with, as administrator, to put its CLAP plugin into `Common Files\CLAP`. It takes no
/// paths: the step finds the plugin beside itself and the folder itself, so nothing it's started with can steer it.
pub const PUT_CLAP_PLUGIN: &str = "--put-clap-plugin";

/// The shared CLAP folder every DAW looks in, `Common Files\CLAP` (Solomon, 2026-11-25: here, with Windows' Yes).
/// Windows only: the installer that puts the plugin beside the app is Windows only.
fn clap_folder() -> Option<PathBuf> {
    std::env::var_os("CommonProgramW6432")
        .or_else(|| std::env::var_os("CommonProgramFiles"))
        .map(|c| PathBuf::from(c).join("CLAP"))
}

/// The administrator step: puts the plugin beside this program into `Common Files\CLAP` (written beside itself, read
/// back, then put in place). Ends with 0 when it went in.
pub fn put_plugin_as_admin() -> i32 {
    let source = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe));
    let (Some(source), Some(folder)) = (source, clap_folder()) else { return 2 };
    if scripts::install_clap(&source, &folder).went.is_empty() { 1 } else { 0 }
}

/// Whether a DAW has the plugin open now: Windows then lets nobody replace it, so asking for administrator rights would
/// only fail. (Opened with no sharing, which a loaded plugin refuses.)
fn in_use(file: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        file.is_file() && std::fs::OpenOptions::new().read(true).share_mode(0).open(file).is_err()
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        false
    }
}

/// Where the CLAP plugin stands this start, for what the look keeps in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluginAsk {
    /// Windows wasn't asked.
    NotAsked,
    /// You said no to Windows' question: not asked again for this version.
    SaidNo,
}

/// Puts in or updates the CLAP plugin the installer put beside the app, as [`daw_links::step`] says, in
/// `Common Files\CLAP`, which needs Windows' Yes: asked once for each version, and not while a DAW has the plugin
/// open (then at a later start). Left alone when a test plugin was put in by hand (the same plugin: a DAW would list
/// it twice), and when you said no for this version (`declined`).
fn put_in_plugin(
    first_look: bool,
    newer: bool,
    declined: bool,
    feed: &Feed,
) -> (Option<(Step, scripts::Installed)>, PluginAsk) {
    let not = (None, PluginAsk::NotAsked);
    let Some(source) = std::env::current_exe().ok().and_then(|exe| scripts::clap_plugin_source(&exe)) else {
        return not;
    };
    let Some(folder) = clap_folder() else { return not };
    let test_names: Vec<PathBuf> = scripts::TEST_CLAP_PLUGIN_NAMES.iter().map(|n| folder.join(n)).collect();
    if let Some(by_hand) = test_names.iter().find(|f| f.is_file()) {
        log::info("DAW links", &format!("The plugin is left alone: {} is there already", by_hand.display()));
        return not;
    }
    let state = scripts::clap_state(&source, &folder);
    let step = daw_links::step(state, first_look, true, newer);
    log::info("DAW links", &format!("CLAP plugin: {state:?}, so {step:?}"));
    if step == Step::Leave {
        return not;
    }
    if declined {
        log::info("DAW links", "You said no to putting this version's plugin in, so Windows isn't asked again");
        return not;
    }
    let target = folder.join(scripts::CLAP_PLUGIN_NAME);
    if in_use(&target) {
        log::info("DAW links", "A DAW has the CLAP plugin open: it's updated at a start when the DAW is closed");
        return not;
    }
    let admin = mo_space::admin::AsAdmin;
    if !mo_space::admin::Admin::can_ask(&admin) {
        return not;
    }
    feed.told(
        "Windows asks to let DAWmate put its plugin in",
        &format!("It goes in {}, where every DAW finds it. Say No to leave it for now.", folder.display()),
    );
    let asked = mo_space::admin::Admin::ask(&admin, &[PUT_CLAP_PLUGIN.to_owned()]);
    // What's there now counts, not the step's word.
    let mut done = scripts::Installed::default();
    match asked {
        Err(why) if why.starts_with("You said no") => {
            log::info("DAW links", "You said no to Windows' question for the CLAP plugin");
            return (None, PluginAsk::SaidNo);
        }
        Err(why) => done.failed.push((folder.clone(), why)),
        Ok(()) if scripts::clap_state(&source, &folder) == ScriptState::Installed => done.went.push(folder.clone()),
        Ok(()) => done.failed.push((folder.clone(), "it isn't the app's copy after the step".to_owned())),
    }
    for f in &done.went {
        log::info("DAW links", &format!("Put the CLAP plugin in {}", f.display()));
    }
    for (f, why) in &done.failed {
        log::warning("DAW links", &format!("Couldn't put the CLAP plugin in {}: {why}", f.display()));
    }
    (Some((step, done)), PluginAsk::NotAsked)
}
