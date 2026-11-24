//! Fills the Settings page from `mo_app::settings` and answers its choices. The settings are kept by
//! `mo_app::app_settings`: this app's own `rust-settings.json` over the C# app's choices in library.db, which is only
//! read. Writing the file, reading the folders from the library and listing the sound outputs all happen off the
//! window's thread (rules 12 and 21); a change shows at once and is written in the background, newest first.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mo_app::activity::Feed;
use mo_app::app_settings::{self, AppSettings, PACE_KEY, SOUND_OUTPUT_KEY, START_PAGE_KEY, SWEEP_HOLD_KEY, TAB_KEY};
use mo_app::background::{self, Newest, Pace};
use mo_app::log;
use mo_app::midi::{SMALLEST_PART_CHOICES, SMALLEST_PART_KEY};
use mo_app::places::Place;
use mo_app::settings::{self as page, Outputs, Tab};
use mo_app::shell::{self, Os};
use mo_app::themes::{
    ACCENT_KEY, FONT_KEY, FONTS, Look, SIZE_KEY, SIZES, THEME_KEY, THEMES, size_from_setting, theme_named,
};
use mo_dawlink::scripts::{self, ScriptState};
use mo_library::Library;
use mo_library::folders::{self, FolderList};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    AccentInfo, AppWindow, DawProgramInfo, DawRowInfo, FolderGroupInfo, KeyLineInfo, SettingsData, SettingsFact,
    ThemeCardInfo,
};

/// What the page knows, on the window's thread.
struct Page {
    settings: AppSettings,
    /// The app's folder, where the settings file is.
    folder: PathBuf,
    library: PathBuf,
    /// The pace the background pool started with (a change takes over at the next start).
    running_pace: Pace,
    outputs: Outputs,
    /// Whether to look at the computer's sound outputs and the library (not for the screenshots).
    live: bool,
    /// Whether changes are written to the file (not when it couldn't be read, nor for the screenshots).
    writable: bool,
    /// A change not yet handed to the writer's finished save, for the last write as the app closes.
    unsaved: bool,
    /// Counts the reads of the folders, so an older one finishing late is dropped (newest wins).
    folders_asked: u64,
    /// The action Shortcuts is listening for a new key for.
    listening: Option<&'static str>,
    /// The words in Shortcuts' search box: only the actions with them show.
    keys_search: String,
    /// What the last key change did.
    keys_message: String,
    /// How Bitwig's link stood when last checked, whether a check is running, and how many were asked for (newest
    /// wins).
    bitwig: page::DawStatus,
    /// The list each folder group changes, by its number on the tab, from the last read.
    folder_lists: Vec<Option<FolderList>>,
    /// Whether a folder change is being kept now.
    folders_busy: bool,
    folders_message: String,
    /// The folder exports go into, as last read (for its Open button), or empty.
    export_folder: String,
    daws_checking: bool,
    daws_asked: u64,
    /// FL Studio's script: whether it's installed (none while being looked at), the folders it goes into, and whether
    /// Install is running; what the last Install did.
    fl_state: Option<ScriptState>,
    fl_folders: Vec<PathBuf>,
    /// Bitwig's extension: the app's own copy and its version, whether Bitwig's folders have it, where it goes, the
    /// user folders those are in, and where Bitwig said it loaded it from.
    bw_source: Option<PathBuf>,
    bw_version: Option<String>,
    bw_state: Option<ScriptState>,
    bw_folders: Vec<PathBuf>,
    bw_users: Vec<PathBuf>,
    bw_loaded_from: Option<String>,
    /// The app's own files of each link found in the DAWs' folders, for Take out (FL Studio's, Bitwig's).
    fl_files: Vec<PathBuf>,
    bw_files: Vec<PathBuf>,
    /// Counts the looks for the DAWs' folders, so an older one finishing late is dropped (newest wins).
    scripts_asked: u64,
    daws_installing: bool,
    /// Whether what runs while `daws_installing` is a Take out (its words on the buttons).
    daws_taking_out: bool,
    daws_message: String,
    /// What the last choice on the "Opening your songs" card did, said under its lines.
    programs_message: String,
    /// The programs chosen for the DAWs that weren't there when the tab last looked ([`look_at_scripts`]).
    programs_missing: HashSet<String>,
    /// Moving the data folder: set while the copy runs (its Stop), and what the last copy did.
    data_moving: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    data_message: String,
    /// The folder a finished copy waits to switch to at the next start (read once, then kept here).
    data_pending: Option<PathBuf>,
    /// What the folder the data was moved out of still holds for the app to put away, as last looked when the page
    /// showed or the old databases were put away, away from the window's thread ([`look_left_behind`]): not at every
    /// redraw (the whole-app review of 14 Nov, AP-18). `None` until the first look is in.
    data_left: Option<Vec<&'static str>>,
    /// Which look at it is the newest, so an older one that took longer (a drive waking up) never shows over it.
    data_left_asked: u64,
    feed: Feed,
}

/// One settings save at a time in this app (review F-5): the writer thread and the last save as the app closes both
/// write through the same `rust-settings.json.mo-part`, so two at once would spoil each other's part. Holding it, the
/// close sets the `bool` to true: the writer then skips any save it hadn't started, which can only be older than the
/// close's own.
static SAVING: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// Saves `json` unless the app has closed (see [`SAVING`]); `None` means it was skipped.
fn save_unless_closed(folder: &Path, json: &str) -> Option<std::io::Result<PathBuf>> {
    let closed = SAVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    (!*closed).then(|| app_settings::save(folder, json))
}

thread_local! {
    static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
    /// The settings [`load`] read before the window opened, for the pages connected before this one (the Sounds page
    /// opens on the view you left it on, and may start reading at once): [`value`] goes by them until [`connect`]
    /// keeps them in [`PAGE`] (review of 7cc595f).
    static EARLY: RefCell<Option<AppSettings>> = const { RefCell::new(None) };
    /// Writes the settings file, newest first.
    static WRITER: RefCell<Option<Newest<(PathBuf, String)>>> = const { RefCell::new(None) };
    /// The size of everything the window is drawn at now, as a part of the system's own scale (1 is 100%).
    static SIZED: Cell<f32> = const { Cell::new(1.0) };
    /// The system's own scale when the size was last applied (0 before that).
    static SYSTEM: Cell<f32> = const { Cell::new(0.0) };
    /// Checks the size of everything is still on, every second.
    static SIZE_CHECK: slint::Timer = slint::Timer::default();
}

/// Reads the settings before the window opens: this app's file in `folder` and the C# app's choices from `library`,
/// opened read-only. What went wrong goes to the log; the app opens whatever happens. Also whether changes may be
/// saved this run (not when the file is there but couldn't be read).
pub fn load(folder: &Path, library: &Path) -> (AppSettings, bool) {
    let lib = match Library::open_read_only(library) {
        Ok(lib) => Some(lib),
        // No library yet (a new computer): only this app's own settings.
        Err(e) if !library.exists() => {
            log::info("Settings", &format!("No library to read the C# app's settings from yet ({e})"));
            None
        }
        Err(e) => {
            log::warning("Settings", &format!("Couldn't open the library for the C# app's settings: {e}"));
            None
        }
    };
    let loaded = AppSettings::load(folder, lib.as_ref());
    for p in &loaded.problems {
        log::warning("Settings", p);
    }
    // Which program each DAW's songs open in, and whether the C# app chose it (the 19 Nov review, SS-4).
    let s = &loaded.settings;
    log::info("Settings", &page::programs_line(|k| s.get(k).map(str::to_owned), |k| s.changed_here(k), Os::THIS));
    EARLY.with(|e| *e.borrow_mut() = Some(loaded.settings.clone()));
    (loaded.settings, loaded.writable)
}

/// The look the settings choose.
pub fn look_of(settings: &AppSettings) -> Look {
    Look::from_settings(settings.get(THEME_KEY), settings.get(ACCENT_KEY)).with_font(settings.get(FONT_KEY))
}

/// What `connect` needs.
pub struct Setup {
    pub settings: AppSettings,
    pub folder: PathBuf,
    pub library: PathBuf,
    pub running_pace: Pace,
    /// Look at the computer's sound outputs and the library when the page shows (off for the screenshots).
    pub live: bool,
    /// Write changes to the file (see [`load`]).
    pub writable: bool,
    pub feed: Feed,
}

pub fn connect(app: &AppWindow, setup: Setup) {
    let setup_live = setup.live;
    let writer = Newest::start("Save settings", |(folder, json): (PathBuf, String)| {
        let Some(saved) = save_unless_closed(&folder, &json) else { return };
        if saved.is_ok() {
            let _ = slint::invoke_from_event_loop(move || {
                PAGE.with(|p| {
                    // Only if nothing changed since: the newer change has its own save on the way.
                    if let Some(p) = p.borrow_mut().as_mut()
                        && p.settings.to_json() == json
                    {
                        p.unsaved = false;
                    }
                });
            });
        }
        if let Err(e) = saved {
            log::error("Settings", &format!("Couldn't save {}: {e}", app_settings::FILE));
            let why = e.to_string();
            let _ = slint::invoke_from_event_loop(move || {
                PAGE.with(|p| {
                    if let Some(p) = p.borrow().as_ref() {
                        p.feed.start("Couldn't save your settings", "").failed(&why);
                    }
                });
            });
        }
    });
    match writer {
        Ok(w) => WRITER.with(|slot| *slot.borrow_mut() = Some(w)),
        Err(e) => log::error("Settings", &format!("Couldn't start the settings writer: {e}")),
    }
    let start_output = setup.settings.get(SOUND_OUTPUT_KEY).filter(|s| !s.is_empty()).map(str::to_owned);
    crate::sounds::choose_output(start_output);
    crate::sounds::hold_sweeps(sweep_hold(&setup.settings));
    // The background helper, when it's wanted: this build's, running, and in the startup list (never for the
    // screenshots' made-up app).
    if setup.live && mo_app::helper_start::wanted(&setup.settings) {
        apply_helper(true, app.as_weak());
    }
    crate::midi::smallest_part(None, smallest_part(&setup.settings));
    crate::beta::connect(app, &setup.folder, setup.live, setup.feed.clone());
    PAGE.with(|p| {
        *p.borrow_mut() = Some(Page {
            outputs: page::outputs(&[], setup.settings.get(SOUND_OUTPUT_KEY), cfg!(windows)),
            settings: setup.settings,
            folder: setup.folder,
            library: setup.library,
            running_pace: setup.running_pace,
            live: setup.live,
            writable: setup.live && setup.writable,
            unsaved: false,
            folders_asked: 0,
            listening: None,
            keys_search: String::new(),
            keys_message: String::new(),
            bitwig: page::bitwig_checking(),
            folder_lists: Vec::new(),
            folders_busy: false,
            folders_message: String::new(),
            export_folder: String::new(),
            daws_checking: false,
            daws_asked: 0,
            fl_state: None,
            fl_folders: Vec::new(),
            bw_source: None,
            bw_version: None,
            bw_state: None,
            bw_folders: Vec::new(),
            bw_users: Vec::new(),
            fl_files: Vec::new(),
            bw_files: Vec::new(),
            bw_loaded_from: None,
            scripts_asked: 0,
            daws_installing: false,
            daws_taking_out: false,
            daws_message: String::new(),
            programs_message: String::new(),
            programs_missing: HashSet::new(),
            data_moving: None,
            data_message: String::new(),
            data_pending: mo_app::data_move::pending(),
            data_left: None,
            data_left_asked: 0,
            feed: setup.feed,
        })
    });
    // The page has them now, changes and all.
    EARLY.with(|e| e.borrow_mut().take());
    app.set_settings_built(true);
    // The start found too much changed since a move's copy to switch before the window opened: catch it up now, so
    // the next start switches.
    // Not at once: after start-up loading has had its turn at the disk (F-7), as tidy waits.
    if let Some(to) = mo_app::data_move::catch_up_wanted().filter(|_| setup_live) {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_secs(30), move || {
            // Not if the move was called off (Don't switch) or changed meanwhile.
            let still = mo_app::data_move::pending().is_some_and(|p| p == to);
            if let (true, Some(app)) = (still, weak.upgrade()) {
                move_data(&app, to, None, false);
            }
        });
    }
    // The installer was asked to put the data somewhere else while there was data already (mo_app::install_choice):
    // the app's own checked move does it, once, after start-up loading as above. The ask goes once the copy has ended
    // (done, stopped or failed), so a copy cut short by closing the app goes on at the next start (DA-3); a move under
    // way already (or called off) wins and the ask just goes.
    else if let Some(to) = mo_app::install_choice::move_asked().filter(|_| setup_live) {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_secs(30), move || {
            // The data folder in use isn't the real one this run (its drive is missing, say): copying it would move
            // the wrong data. The ask stays for a start that finds the data.
            if let Some(why) = mo_app::paths::problem() {
                log::warning("Data folder", &format!("The installer's ask to move to {} waits: {why}", to.display()));
                return;
            }
            if mo_app::data_move::pending().is_some() {
                mo_app::install_choice::take_move_ask();
                log::info(
                    "Data folder",
                    &format!("The installer's ask to move to {} is dropped: a move is under way already", to.display()),
                );
                return;
            }
            let Some(app) = weak.upgrade() else { return };
            log::info("Data folder", &format!("The installer asked for the data to go to {}", to.display()));
            if !move_data(&app, to, None, true) {
                log::info("Data folder", "The installer's ask waits for the next start: a copy is going already");
            }
        });
    }

    let data = app.global::<SettingsData>();
    data.set_tabs(ModelRc::new(VecModel::from(
        Tab::ALL.iter().map(|t| SharedString::from(t.name())).collect::<Vec<_>>(),
    )));
    data.set_fonts(strings(FONTS.iter().copied()));
    data.set_sizes(strings(page::size_labels()));
    data.set_start_pages(strings(page::start_choices()));
    data.set_paces(strings(Pace::ALL.iter().map(|p| p.name())));
    data.set_smallest_parts(strings(SMALLEST_PART_CHOICES.iter().map(|&n| mo_app::midi::smallest_part_words(n))));
    data.set_version(format!("Version {}", mo_app::version::version()).into());
    data.set_credit(page::SLINT_CREDIT.into());
    data.set_folders_note(page::FOLDERS_NOTE.into());
    data.set_exports_title(page::EXPORTS_TITLE.into());
    data.set_exports_note(page::EXPORTS_NOTE.into());
    data.set_keys_note(page::KEYS_NOTE.into());
    data.set_daws_note(page::DAWS_NOTE.into());
    data.set_kept_note("Your changes are saved as you make them".into());
    show(app);

    let weak = app.as_weak();
    data.on_tab_picked(move |i| {
        let Some(tab) = usize::try_from(i).ok().and_then(Tab::from_index) else { return };
        if let Some(app) = weak.upgrade() {
            show_tab(&app, tab);
        }
    });
    let weak = app.as_weak();
    data.on_add_folder(move |group| {
        let Some(list) = folder_list(group) else { return };
        let weak = weak.clone();
        crate::backup::pick_folder(page::pick_title(list), move |folder| {
            if let Some(app) = weak.upgrade() {
                edit_folder(&app, list, folder.display().to_string(), true);
            }
        });
    });
    let weak = app.as_weak();
    data.on_remove_folder(move |group, folder| {
        if let (Some(app), Some(list)) = (weak.upgrade(), folder_list(group)) {
            edit_folder(&app, list, folder.to_string(), false);
        }
    });
    let weak = app.as_weak();
    data.on_choose_export_folder(move || {
        // Not for the screenshots, as the folder lists' Add folder isn't: nothing would be kept.
        if !PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live)) {
            return;
        }
        let weak = weak.clone();
        crate::backup::pick_folder(page::EXPORTS_PICK_TITLE, move |folder| {
            if let Some(app) = weak.upgrade() {
                edit_export(&app, Some(folder.display().to_string()));
            }
        });
    });
    let weak = app.as_weak();
    data.on_documents_exports(move || {
        if let Some(app) = weak.upgrade() {
            edit_export(&app, None);
        }
    });
    data.on_open_export_folder(open_export_folder);
    let weak = app.as_weak();
    data.on_choose_bitwig_folder(move || {
        // Not for the screenshots: nothing would be kept, and they never look at real folders.
        if !PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live)) {
            return;
        }
        let weak = weak.clone();
        crate::backup::pick_folder(page::BITWIG_PICK_TITLE, move |picked| {
            // What's in the folder picked is looked at away from the window (its drive may be asleep).
            let started = background::spawn("Settings Bitwig folder", move || {
                let (folder, looks) = page::bitwig_folder_from_pick(&picked, Path::is_dir);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        choose_bitwig_folder(&app, Some((folder, looks)));
                    }
                });
            });
            if let Err(e) = started {
                log::error("Settings", &format!("Couldn't start looking at the Bitwig folder picked: {e}"));
            }
        });
    });
    let weak = app.as_weak();
    data.on_add_bitwig_library(move || {
        // Not for the screenshots: nothing would be kept, and they never look at real folders.
        if !PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live)) {
            return;
        }
        let weak = weak.clone();
        crate::backup::pick_folder(page::BITWIG_LIBRARY_PICK_TITLE, move |picked| {
            if let Some(app) = weak.upgrade() {
                edit_bitwig_libraries(&app, |list| list.push(page::bitwig_library_from_pick(&picked)));
            }
        });
    });
    let weak = app.as_weak();
    data.on_remove_bitwig_library(move |folder| {
        if let Some(app) = weak.upgrade()
            && PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live))
        {
            let gone = folder.to_lowercase();
            edit_bitwig_libraries(&app, |list| list.retain(|f| f.to_string_lossy().to_lowercase() != gone));
        }
    });
    let weak = app.as_weak();
    data.on_documents_bitwig(move || {
        if let Some(app) = weak.upgrade()
            && PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live))
        {
            choose_bitwig_folder(&app, None);
        }
    });
    let weak = app.as_weak();
    data.on_choose_daw_program(move |daw| {
        // Not for the screenshots: nothing would be kept, and they never look at real files.
        if !PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live)) {
            return;
        }
        let weak = weak.clone();
        let (title, kinds) = page::program_pick(&daw, Os::THIS);
        let asked = slint::spawn_local(async move {
            let mut picker = rfd::AsyncFileDialog::new().set_title(&title);
            if !kinds.is_empty() {
                picker = picker.add_filter("Programs", kinds);
            }
            if let Some(exe) = picker.pick_file().await
                && let Some(app) = weak.upgrade()
            {
                choose_program(&app, &daw, Some(exe.path()));
            }
        });
        if let Err(e) = asked {
            log::error("Settings", &format!("Couldn't open the program picker: {e}"));
        }
    });
    let weak = app.as_weak();
    data.on_reset_daw_program(move |daw| {
        if let Some(app) = weak.upgrade()
            && PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.live))
        {
            choose_program(&app, &daw, None);
        }
    });
    let weak = app.as_weak();
    data.on_check_daws(move || {
        if let Some(app) = weak.upgrade() {
            check_daws(&app);
        }
    });
    let weak = app.as_weak();
    data.on_install_daw(move |bitwig| {
        if let Some(app) = weak.upgrade() {
            if bitwig {
                install_bitwig(&app);
            } else {
                install_fl(&app);
            }
        }
    });
    let weak = app.as_weak();
    data.on_take_out_daw(move |bitwig| {
        if let Some(app) = weak.upgrade() {
            take_out(&app, bitwig);
        }
    });
    let weak = app.as_weak();
    data.on_theme_picked(move |i| {
        if let Some(t) = usize::try_from(i).ok().and_then(|i| THEMES.get(i)) {
            change(THEME_KEY, t.name);
            relook(&weak);
        }
    });
    let weak = app.as_weak();
    data.on_accent_picked(move |i| {
        if let Some(a) = usize::try_from(i).ok().and_then(page::accent_setting) {
            change(ACCENT_KEY, &a);
            relook(&weak);
        }
    });
    let weak = app.as_weak();
    data.on_font_picked(move |i| {
        if let Some(f) = usize::try_from(i).ok().and_then(|i| FONTS.get(i)) {
            change(FONT_KEY, f);
            relook(&weak);
        }
    });
    let weak = app.as_weak();
    data.on_size_picked(move |i| {
        if let Some(s) = usize::try_from(i).ok().and_then(|i| SIZES.get(i)) {
            change(SIZE_KEY, &s.to_string());
            if let Some(app) = weak.upgrade() {
                apply_size(&app);
                show(&app);
            }
        }
    });
    let weak = app.as_weak();
    data.on_back_to_normal(move || {
        change(THEME_KEY, THEMES[0].name);
        change(ACCENT_KEY, "");
        change(FONT_KEY, FONTS[0]);
        change(SIZE_KEY, "100");
        if let Some(app) = weak.upgrade() {
            apply_size(&app);
        }
        relook(&weak);
    });
    let weak = app.as_weak();
    data.on_start_picked(move |name| {
        if let Some(choice) = page::start_choices().into_iter().find(|c| *c == name.as_str()) {
            change(START_PAGE_KEY, choice);
            // "Last page I used" starts from here, Settings.
            page_shown(Place::Settings);
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        }
    });
    let weak = app.as_weak();
    data.on_pace_picked(move |i| {
        if let Some(p) = usize::try_from(i).ok().and_then(|i| Pace::ALL.get(i)) {
            change(PACE_KEY, p.name());
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        }
    });
    let weak = app.as_weak();
    data.on_output_picked(move |label| {
        let name = PAGE.with(|p| {
            let p = p.borrow();
            let o = &p.as_ref()?.outputs;
            o.labels.iter().position(|l| *l == label.as_str()).map(|i| o.names[i].clone())
        });
        let Some(name) = name else { return };
        change(SOUND_OUTPUT_KEY, &name);
        crate::sounds::choose_output((!name.is_empty()).then_some(name));
        if let Some(app) = weak.upgrade() {
            list_outputs(&app);
        }
    });
    let weak = app.as_weak();
    data.on_curve_key_picked(move |role, key| {
        use mo_app::curve_keys::{Key, KeyMap, Role, SETTING};
        let pick = |i: i32, n: usize| usize::try_from(i).ok().filter(|i| *i < n);
        let (Some(role), Some(key)) = (pick(role, Role::ALL.len()), pick(key, Key::ALL.len())) else { return };
        let was = KeyMap::from_setting(value(SETTING).as_deref());
        let now = was.with(Role::ALL[role], Key::ALL[key]);
        change(SETTING, &now.to_setting());
        if let Some(app) = weak.upgrade() {
            crate::curve_editor::set_keys(&app, now);
            // The whole tab: "Every key back to the start" follows this too.
            show(&app);
        }
    });
    let weak = app.as_weak();
    data.on_toggle_sweep_hold(move || {
        let on = !PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| sweep_hold(&p.settings)));
        change(SWEEP_HOLD_KEY, if on { "1" } else { "0" });
        crate::sounds::hold_sweeps(on);
        if let Some(app) = weak.upgrade() {
            app.global::<SettingsData>().set_sweep_hold(on);
        }
    });
    let weak = app.as_weak();
    data.on_toggle_helper(move || {
        let Some((was, live, writable)) =
            PAGE.with(|p| p.borrow().as_ref().map(|p| (mo_app::helper_start::wanted(&p.settings), p.live, p.writable)))
        else {
            return;
        };
        let Some(app) = weak.upgrade() else { return };
        let data = app.global::<SettingsData>();
        // A choice that can't be kept isn't acted on: the helper would start at every login with the switch showing
        // off at the next start.
        if live && !writable {
            data.set_helper_note(
                "Your settings can't be saved this run, so this can't be changed until the app starts again.".into(),
            );
            return;
        }
        let on = !was;
        change(mo_app::helper_start::KEY, if on { "1" } else { "0" });
        data.set_helper_on(on);
        // The screenshots' made-up app starts nothing: there the chip only changes.
        if live {
            data.set_helper_note(if on { "Starting the helper\u{2026}" } else { "Stopping the helper\u{2026}" }.into());
            apply_helper(on, weak.clone());
        }
    });
    let weak = app.as_weak();
    data.on_smallest_picked(move |i| {
        let Some(&notes) = usize::try_from(i).ok().and_then(|i| SMALLEST_PART_CHOICES.get(i)) else { return };
        change(SMALLEST_PART_KEY, &notes.to_string());
        if let Some(app) = weak.upgrade() {
            crate::midi::smallest_part(Some(&app), notes);
            app.global::<SettingsData>().set_smallest_index(i);
        }
    });
    let weak = app.as_weak();
    data.on_refresh_outputs(move || {
        if let Some(app) = weak.upgrade() {
            list_outputs(&app);
        }
    });
    data.on_open_fact(open_fact);
    let weak = app.as_weak();
    data.on_move_data(move || {
        let weak = weak.clone();
        crate::backup::pick_folder("Pick the drive or folder the app's data goes to", move |picked| {
            let Some(app) = weak.upgrade() else { return };
            let to = mo_app::data_move::target_in(&picked);
            // The folder the data moved to can't be found, so this run is on the old copy: copying that leaves the
            // real data behind, so ask first (DA-2).
            let missing = mo_app::paths::missing_folder().filter(|m| !mo_app::data_move::paths_match(m, &to));
            let Some(missing) = missing else {
                move_data(&app, to, None, false);
                return;
            };
            let weak = app.as_weak();
            let asked = slint::spawn_local(async move {
                let answer = rfd::AsyncMessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Copy the old copy of your data?")
                    .set_description(format!(
                        "Your data folder {} can't be found, so this copies the old copy in {}, from before the \
                         move, not your data there. Once it switches, the app stops looking for {}; nothing in it is \
                         removed.\n\nTo move your real data, plug its drive in and start the app again.\n\nCopy \
                         the old copy anyway?",
                        missing.display(),
                        mo_app::paths::data_dir().display(),
                        missing.display()
                    ))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    .await;
                if answer == rfd::MessageDialogResult::Yes
                    && let Some(app) = weak.upgrade()
                {
                    move_data(&app, to, Some(missing), false);
                }
            });
            if let Err(e) = asked {
                log::error("Data folder", &format!("Couldn't ask about copying the old copy: {e}"));
            }
        });
    });
    data.on_stop_data_move(|| {
        PAGE.with(|p| {
            if let Some(stop) = p.borrow().as_ref().and_then(|p| p.data_moving.as_ref()) {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
    });
    let weak = app.as_weak();
    data.on_cancel_data_move(move || {
        let message = match mo_app::data_move::cancel() {
            Ok(()) => {
                log::info("Data folder", "The move is called off: the app keeps using this folder");
                "Called off: the app keeps using this folder. The copy stays where it is until you remove it."
                    .to_owned()
            }
            Err(e) => format!("Couldn't call the move off: {e}"),
        };
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.data_message = message;
                p.data_pending = mo_app::data_move::pending();
            }
        });
        if let Some(app) = weak.upgrade() {
            show(&app);
        }
    });
    let weak = app.as_weak();
    data.on_listen(move |id| {
        let id = mo_app::shortcuts::all_actions().map(|(_, a)| a.id).find(|a| *a == id.as_str());
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                // A second click on the one listening stops it.
                p.listening = if p.listening == id { None } else { id };
                p.keys_message.clear();
            }
        });
        if let Some(app) = weak.upgrade() {
            show(&app);
        }
    });
    let weak = app.as_weak();
    data.on_keys_search_edited(move |words| {
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                // Typing in the search stops a line listening: the new words may hide it, and it would take the next
                // key out of sight (the whole-app review of 14 Nov, AP-3).
                if p.keys_search != words.as_str() {
                    p.listening = None;
                }
                p.keys_search = words.to_string();
            }
        });
        if let Some(app) = weak.upgrade() {
            show(&app);
        }
    });
    let weak = app.as_weak();
    data.on_clear_key(move |id| {
        if let Some(app) = weak.upgrade() {
            give_key(&app, id.as_str(), "");
        }
    });
    let weak = app.as_weak();
    data.on_default_key(move |id| {
        let default = mo_app::shortcuts::all_actions().find(|(_, a)| a.id == id.as_str()).map(|(_, a)| a.default_key);
        if let (Some(app), Some(default)) = (weak.upgrade(), default) {
            give_key(&app, id.as_str(), default);
        }
    });
    let weak = app.as_weak();
    data.on_reset_keys(move || {
        let Some(app) = weak.upgrade() else { return };
        let mut keys = crate::shortcuts();
        keys.reset_all();
        // The curve editor's keys on the same tab go back too.
        let start = mo_app::curve_keys::KeyMap::default();
        change(mo_app::curve_keys::SETTING, &start.to_setting());
        crate::curve_editor::set_keys(&app, start);
        keys_now(&app, &keys, "All keys are back to the ones they started with.".to_owned());
    });
    let weak = app.as_weak();
    data.on_try_job(move || {
        if let Some(app) = weak.upgrade() {
            app.invoke_try_job();
        }
    });
    // The page the app opened on, for "Last page I used" (it was shown before Settings was ready).
    if let Some(place) = usize::try_from(app.get_place()).ok().and_then(Place::from_index) {
        page_shown(place);
        if place == Place::Settings {
            shown(app);
        }
    }
    // The system may set its own scale (the window moved to another screen): the size picked goes back on it.
    if setup_live {
        let weak = app.as_weak();
        SIZE_CHECK.with(|t| {
            t.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
                if let Some(app) = weak.upgrade() {
                    apply_size(&app);
                }
            })
        });
    }
}

fn strings<S: Into<SharedString>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(Into::into).collect::<Vec<SharedString>>()))
}

/// The MIDI page's smallest part, in notes (3 unless picked).
pub fn smallest_part(settings: &AppSettings) -> usize {
    mo_app::midi::smallest_part_from(settings.get(SMALLEST_PART_KEY))
}

/// Starts or stops the background helper as chosen, and puts how it went under the choice in Settings.
fn apply_helper(on: bool, weak: slint::Weak<AppWindow>) {
    crate::helper::apply(on, move |line| {
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                app.global::<SettingsData>().set_helper_note(line.into());
            }
        });
    });
}

/// The curve editor's keys on the Shortcuts tab: each meaning's line and the key it has.
fn show_curve_keys(data: &SettingsData, map: mo_app::curve_keys::KeyMap) {
    use mo_app::curve_keys::{Key, Role};
    use slint::Model;
    // The lines and the keys' names never change: given once, so the rows aren't made again at each pick.
    if data.get_curve_key_lines().row_count() == 0 {
        let mac = cfg!(target_os = "macos");
        let lines: Vec<SharedString> = Role::ALL.iter().map(|r| r.words().1.into()).collect();
        let names: Vec<SharedString> = Key::ALL.iter().map(|k| k.name(mac).into()).collect();
        data.set_curve_key_lines(ModelRc::new(VecModel::from(lines)));
        data.set_curve_key_names(ModelRc::new(VecModel::from(names)));
    }
    let keys: Vec<i32> =
        Role::ALL.iter().map(|r| Key::ALL.iter().position(|k| *k == map.key(*r)).map_or(0, |i| i as i32)).collect();
    data.set_curve_keys(ModelRc::new(VecModel::from(keys)));
}

/// Whether a Galaxy sweep holds the pointer to the star that's playing (off unless turned on).
pub fn sweep_hold(settings: &AppSettings) -> bool {
    settings.get(SWEEP_HOLD_KEY) == Some("1")
}

/// Whether the window should be full screen: set at the start from the setting, and changed only by F11. When the
/// window comes back from the taskbar, [`restored`] checks it still is (Solomon's F12 note of 17 Nov: "clicking on the
/// space tab took the app out of full screen", just after a restore).
static FULL_SCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The window opens full screen (or not), as the setting says.
pub fn full_screen_at_start(on: bool) {
    FULL_SCREEN.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// F11 turned full screen on or off: the window opens that way next time.
pub fn full_screen_changed(on: bool) {
    FULL_SCREEN.store(on, std::sync::atomic::Ordering::Relaxed);
    change(app_settings::FULL_SCREEN_KEY, if on { "1" } else { "0" });
}

/// What winit says about the window: its inside size, the width of the screen it is on, full screen, maximised.
struct WindowLook {
    width: u32,
    height: u32,
    screen: Option<u32>,
    full_screen: bool,
    maximised: bool,
}

impl WindowLook {
    fn words(&self) -> String {
        let state = if self.full_screen {
            "full screen"
        } else if self.maximised {
            "maximised"
        } else {
            "neither maximised nor full screen"
        };
        let screen = self.screen.map_or_else(|| " (screen not known)".to_owned(), |s| format!(" on a screen {s} wide"));
        format!("{} x {}{screen}, {state}", self.width, self.height)
    }

    /// Narrower than its screen: a maximised or full-screen window this small didn't really fill it.
    fn smaller_than_screen(&self) -> bool {
        self.screen.is_some_and(|s| app_settings::smaller_than_screen(self.width, s))
    }
}

fn look_at(window: &slint::winit_030::winit::window::Window) -> WindowLook {
    let size = window.inner_size();
    WindowLook {
        width: size.width,
        height: size.height,
        screen: window.current_monitor().map(|m| m.size().width),
        full_screen: window.fullscreen().is_some(),
        maximised: window.is_maximized(),
    }
}

/// Once the window is up: full screen if F11 left it so (turned on only now, so F11 later goes back to maximised),
/// else maximised. Half a second later a line in the log says what it is, and a window that says it is maximised but
/// is narrower than its screen is maximised again through winit itself: Slint and winit pass a maximise on only when
/// they think it changes something, and Windows ignores one for a window it already counts as maximised (the 19 Nov
/// review, CL-1).
pub fn fill_the_screen(app: &AppWindow) {
    let window = app.window();
    if FULL_SCREEN.load(std::sync::atomic::Ordering::Relaxed) {
        window.set_fullscreen(true);
    } else {
        window.set_maximized(true);
    }
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(500), move || {
        use slint::winit_030::WinitWindowAccessor;
        let Some(app) = weak.upgrade() else { return };
        let full = FULL_SCREEN.load(std::sync::atomic::Ordering::Relaxed);
        let seen = app.window().with_winit_window(|w| {
            let look = look_at(w);
            let mend = !full && !look.full_screen && look.maximised && look.smaller_than_screen();
            if mend {
                w.set_maximized(false);
                w.set_maximized(true);
            }
            (look, mend)
        });
        if let Some((look, _)) = &seen {
            MAXIMISED.store(look.maximised || look.full_screen, std::sync::atomic::Ordering::Relaxed);
        }
        match seen {
            Some((look, true)) => log::info(
                "Window",
                &format!("The window is {}: smaller than the screen, so it was maximised again", look.words()),
            ),
            Some((look, false)) => log::info("Window", &format!("The window is {}", look.words())),
            None => log::info("Window", "The window's size couldn't be read"),
        }
    });
}

/// Whether the window was last seen maximised (or full screen), from half a second after the start on.
static MAXIMISED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The window's size changed: when it stopped being maximised (or full screen), a line in the log says so, whether
/// the user may have done it (see `watchdog::Hands`), and what Slint asked of the window in the 30 s before, from the
/// notes its patched copy keeps (vendor/i-slint-backend-winit/PATCHED.md). On 24 Nov it left maximised by itself 10 s
/// after coming back from the taskbar, on Songs with no page change; these lines are to name what did it.
pub fn resized(app: &AppWindow, by_user: bool) {
    use slint::winit_030::WinitWindowAccessor;
    let Some((look, winit_scale)) = app.window().with_winit_window(|w| (look_at(w), w.scale_factor())) else {
        return;
    };
    let now = look.maximised || look.full_screen;
    let was = MAXIMISED.swap(now, std::sync::atomic::Ordering::Relaxed);
    if !was || now {
        return;
    }
    let place = usize::try_from(app.get_place()).ok().and_then(Place::from_index).map_or("?", Place::name);
    let who = if by_user {
        "perhaps by the user: the pointer was on the title bar or an edge, or the Windows key or Alt was pressed"
    } else {
        "not by the user: the pointer was inside the window or the app wasn't in front, and no Windows key or Alt"
    };
    let asked = i_slint_backend_winit::mo_probe::since(std::time::Duration::from_secs(30));
    let asked = if asked.is_empty() {
        "Slint asked nothing of the window in the 30 s before".to_owned()
    } else {
        format!("what Slint asked of the window in the 30 s before:\n  {}", asked.join("\n  "))
    };
    log::info(
        "Window",
        &format!(
            "The window stopped being maximised (page {place}): it is {}; {who}. Slint draws at scale {}, winit's \
             scale is {winit_scale}; {asked}",
            look.words(),
            app.window().scale_factor(),
        ),
    );
}

/// The window came back from the taskbar: a line in the log with what winit says it is, and full screen put back
/// through winit itself when F11 left it on and the window lost it (narrower than its screen, or no longer full
/// screen). Slint's own is_fullscreen() can't tell: it still says full screen after a restore (the 19 Nov review,
/// CL-6). Done a moment later, on the event loop, so it isn't changed in the middle of the restore's own resize.
pub fn restored(app: &AppWindow) {
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(0), move || {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::window::Fullscreen;
        let Some(app) = weak.upgrade() else { return };
        let full = FULL_SCREEN.load(std::sync::atomic::Ordering::Relaxed);
        let seen = app.window().with_winit_window(|w| {
            let look = look_at(w);
            let mend = full && (!look.full_screen || look.smaller_than_screen());
            if mend {
                w.set_fullscreen(None);
                w.set_fullscreen(Some(Fullscreen::Borderless(None)));
            }
            (look, mend)
        });
        // Only with full screen on: the watchdog already logs each restore.
        if let Some((look, mend)) = seen.filter(|_| full) {
            let after = if mend { ": full screen had gone, so it was put back" } else { "" };
            log::info("Window", &format!("Back from the taskbar, the window is {}{after}", look.words()));
        }
    });
}

/// Changes a setting and writes the file in the background, when it changed.
fn change(key: &str, value: &str) {
    let save = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut()?;
        // The screenshots change their made-up settings, and never write a file.
        let save = p.settings.set(key, value) && p.writable;
        p.unsaved |= save;
        save.then(|| (p.folder.clone(), p.settings.to_json()))
    });
    if let Some(job) = save {
        WRITER.with(|w| {
            if let Some(w) = w.borrow().as_ref() {
                w.ask(job);
            }
        });
    }
}

/// A setting as it stands: this app's change, else the C# app's choice (for the pages that remember things). Before
/// this page is connected, as [`load`] read them.
pub fn value(key: &str) -> Option<String> {
    PAGE.with(|p| match p.borrow().as_ref() {
        Some(p) => p.settings.get(key).map(str::to_owned),
        None => EARLY.with(|e| e.borrow().as_ref().and_then(|s| s.get(key).map(str::to_owned))),
    })
}

/// Changes a setting and keeps it in the app's own settings file (never in `library.db`). False when it can't be
/// kept past this run (the file couldn't be read at the start, so it isn't written over).
pub fn remember(key: &str, value: &str) -> bool {
    change(key, value);
    PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.writable))
}

/// Whether Settings › Shortcuts is listening for a new key, so the next key press is that key.
pub fn listening() -> bool {
    PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.listening.is_some()))
}

/// A key pressed while listening: Esc stops listening, a modifier on its own waits for the rest, any other key is
/// given to the action. Returns whether the press was used (always, so nothing else runs while listening).
pub fn key_pressed(app: &AppWindow, key: &str) -> bool {
    if key.is_empty() {
        return true;
    }
    let id = PAGE.with(|p| p.borrow_mut().as_mut().and_then(|p| p.listening.take()));
    if let Some(id) = id
        && key != "Esc"
    {
        give_key(app, id, key);
    } else {
        show(app);
    }
    true
}

/// Gives an action a key (or none), keeps it and shows it everywhere.
fn give_key(app: &AppWindow, id: &str, key: &str) {
    let mut keys = crate::shortcuts();
    let said = keys.set(id, key);
    keys_now(app, &keys, said);
}

/// The keys changed: kept in the settings, put to work everywhere at once, and the tab shows them.
fn keys_now(app: &AppWindow, keys: &mo_app::shortcuts::Shortcuts, said: String) {
    change(mo_app::shortcuts::SETTING_KEY, &keys.to_setting());
    crate::keys_changed(app, keys);
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.listening = None;
            p.keys_message = said;
        }
    });
    show(app);
}

/// Shows one tab (picked on the page, or by Ctrl+K).
pub fn show_tab(app: &AppWindow, tab: Tab) {
    left_shortcuts(tab);
    change(TAB_KEY, tab.name());
    show(app);
    if tab == Tab::Daws {
        check_daws(app);
    }
}

/// Goes to Settings on `tab` (from Ctrl+K, or the rail's DAW lines): the tab first, so the page asks Bitwig once as
/// it shows when that's the DAWs tab ([`show_tab`] after going there would ask again).
pub fn open_on(app: &AppWindow, tab: Tab) {
    left_shortcuts(tab);
    change(TAB_KEY, tab.name());
    crate::go_to(app, Place::Settings.index() as i32);
}

/// Another tab than Shortcuts on show stops it listening, as leaving Settings does: nothing on the tab says a key is
/// awaited, and the next key there would take a shortcut's place (the whole-app review of 14 Nov, AP-6).
fn left_shortcuts(tab: Tab) {
    if tab != Tab::Shortcuts {
        stop_listening();
    }
}

/// Shortcuts stops listening for a key, so the next key works as a key again.
fn stop_listening() {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.listening = None;
        }
    });
}

/// Whether the Shortcuts tab is the one on show, so Ctrl+F has its search box to go to.
pub fn on_shortcuts() -> bool {
    PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| Tab::from_setting(p.settings.get(TAB_KEY)) == Tab::Shortcuts))
}

/// Esc on Settings (`app.escape`, once a line listening has had its Esc): the Shortcuts search's words come off, as
/// every page's search clears first (the whole-app review of 14 Nov, AP-7). Says whether it did (when not, the key
/// goes on).
pub fn escape(app: &AppWindow) -> bool {
    let data = app.global::<SettingsData>();
    if !on_shortcuts() || data.get_keys_search().is_empty() {
        return false;
    }
    data.set_keys_search(SharedString::new());
    data.invoke_keys_search_edited(SharedString::new());
    true
}

/// Remembers the page on show, for "Last page I used": only while that's the start page, so other clicks on the rail
/// write nothing.
pub fn page_shown(place: Place) {
    // Leaving Settings stops Shortcuts listening, so the next key on another page works as a key again.
    if place != Place::Settings {
        stop_listening();
    }
    let wanted = PAGE.with(|p| {
        p.borrow().as_ref().is_some_and(|p| p.settings.get(START_PAGE_KEY) == Some(mo_app::places::LAST_PAGE_CHOICE))
    });
    if wanted {
        change(app_settings::LAST_PAGE_KEY, place.name());
    }
}

/// As the app closes: a change the writer hasn't saved yet is saved now, so the last one isn't lost.
pub fn finish_up() {
    crate::beta::finish_up();
    let job = PAGE.with(|p| {
        let p = p.borrow();
        let p = p.as_ref()?;
        (p.unsaved && p.writable).then(|| (p.folder.clone(), p.settings.to_json()))
    });
    // Waits for a save the writer is part way through, and stops it starting another.
    let mut closed = SAVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *closed = true;
    if let Some((folder, json)) = job
        && let Err(e) = app_settings::save(&folder, &json)
    {
        log::error("Settings", &format!("Couldn't save {} as the app closed: {e}", app_settings::FILE));
    }
}

/// Puts the look the settings now choose on the whole window, and on this page.
fn relook(weak: &slint::Weak<AppWindow>) {
    let Some(app) = weak.upgrade() else { return };
    let look = PAGE.with(|p| p.borrow().as_ref().map(|p| look_of(&p.settings)));
    if let Some(look) = look {
        crate::apply_look(&app, &look);
    }
    show(&app);
}

/// The page is on show: the folders are read from the library, and the sound outputs listed, each time (a drive or an
/// audio interface may have been plugged in).
pub fn shown(app: &AppWindow) {
    let live = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut()?;
        p.folders_asked += 1;
        Some((p.live, p.library.clone(), p.folders_asked))
    });
    let Some((live, library, asked)) = live else { return };
    show(app);
    if !live {
        return;
    }
    look_left_behind(app);
    list_outputs(app);
    // Bitwig is only asked while its tab is on show.
    let on_daws =
        PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| Tab::from_setting(p.settings.get(TAB_KEY)) == Tab::Daws));
    if on_daws {
        check_daws(app);
    }
    read_folders_later(app, library, asked);
}

/// Looks away from the window at what the folder the data was moved out of still holds for the app to put away (it
/// may have put some away since the last look), and shows the data folder's note again when that changed. The newest
/// look wins. Called when the page shows, and once the old databases are put away.
pub fn look_left_behind(app: &AppWindow) {
    let Some(from) = mo_app::paths::moved_from() else { return };
    let Some(asked) = PAGE.with(|p| {
        p.borrow_mut().as_mut().map(|p| {
            p.data_left_asked += 1;
            p.data_left_asked
        })
    }) else {
        return;
    };
    let weak = app.as_weak();
    let started = background::spawn("Settings data folder", move || {
        let left = mo_app::data_move::left_to_put_away(&mo_app::paths::data_dir(), Some(&from));
        let _ = slint::invoke_from_event_loop(move || {
            let changed = PAGE.with(|p| match p.borrow_mut().as_mut() {
                Some(p) if p.data_left_asked == asked && p.data_left.as_ref() != Some(&left) => {
                    p.data_left = Some(left);
                    true
                }
                _ => false,
            });
            if changed && let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start looking in the folder the data was moved out of: {e}"));
    }
}

/// Reads the folders away from the window, and shows them if no newer read was asked for since.
fn read_folders_later(app: &AppWindow, library: PathBuf, asked: u64) {
    let weak = app.as_weak();
    let started = background::spawn("Settings folders", move || {
        let groups = read_folders(&library);
        let exports = read_exports(&library);
        let _ = slint::invoke_from_event_loop(move || {
            let newest = PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.folders_asked == asked));
            if let Some(app) = weak.upgrade()
                && newest
            {
                show_folders(&app, groups);
                show_exports(&app, &exports);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start reading the folders: {e}"));
    }
}

/// The standard plugin folders, which an empty plugin list stands for, as the page shows them.
fn plugin_defaults() -> Vec<String> {
    mo_space::plugins::default_folders().iter().map(|p| p.display().to_string()).collect()
}

/// The list a folder group changes, by its number on the tab.
fn folder_list(group: i32) -> Option<FolderList> {
    let group = usize::try_from(group).ok()?;
    PAGE.with(|p| p.borrow().as_ref().filter(|p| p.live).and_then(|p| p.folder_lists.get(group).copied().flatten()))
}

/// Adds a folder to a list or takes it off, in library.db, away from the window (the C# app may hold the library for
/// a moment), then reads the folders again. One change at a time: the buttons wait while one is kept.
fn edit_folder(app: &AppWindow, list: FolderList, folder: String, add: bool) {
    let library = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live)?;
        if p.folders_busy {
            // A folder picked while another change was being kept: say so rather than drop it.
            p.folders_message =
                format!("{folder} wasn't changed: another folder change was being saved. Try it again.");
            return None;
        }
        p.folders_busy = true;
        Some(p.library.clone())
    });
    let Some(library) = library else {
        show(app);
        return;
    };
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings folder change", move || {
        let defaults = if list == FolderList::Plugins { plugin_defaults() } else { Vec::new() };
        let done = if add {
            folders::add_folder(&library, list, &folder, &defaults)
        } else {
            folders::remove_folder(&library, list, &folder, &defaults)
        };
        let mut scanning = false;
        match &done {
            Ok(folders::Changed::Done(_)) => {
                log::info(
                    "Settings",
                    &format!("{} {folder} ({list:?} folders)", if add { "Added" } else { "Removed" }),
                );
                // Sounds scans a new sample folder now, not at the next start (review X2-4), and the note says so when
                // it will. One taken off needs no scan: its sounds are kept as they are.
                if add && list == FolderList::Samples {
                    scanning = crate::sounds::sample_folders_changed();
                }
            }
            Ok(folders::Changed::AsItWas) => {}
            Err(e) => log::warning("Settings", &format!("Couldn't change the {list:?} folders: {e}")),
        }
        let done = done.map(|d| matches!(d, folders::Changed::Done(_))).map_err(|e| e.to_string());
        changed_folders(weak, page::folder_change_note(list, add, &folder, done, scanning));
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start changing the folders: {e}"));
        not_changing_folders(app);
    }
}

/// Chooses the folder exports go into (`Some`), or goes back to the app's own folder in Documents (`None`), in
/// the library, away from the window; then reads the folders again. One change at a time, with the folder lists'.
/// Only the setting changes: no folder is made, moved or emptied, and what was exported before stays where it is.
fn edit_export(app: &AppWindow, folder: Option<String>) {
    let library = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live)?;
        if p.folders_busy {
            // Picked while another change was being kept: say so rather than drop it.
            p.folders_message =
                "The export folder wasn't changed: a folder change was being saved. Try it again.".to_owned();
            return None;
        }
        p.folders_busy = true;
        Some(p.library.clone())
    });
    let Some(library) = library else {
        show(app);
        return;
    };
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings export folder", move || {
        let done = folders::set_export_folder(&library, folder.as_deref());
        match (&done, &folder) {
            (Ok(true), Some(folder)) => log::info("Settings", &format!("Exports go into {folder}")),
            (Ok(true), None) => log::info("Settings", "Exports go into the app's own folder in Documents again"),
            (Ok(false), _) => {}
            (Err(e), _) => log::warning("Settings", &format!("Couldn't change the export folder: {e}")),
        }
        changed_folders(weak, page::export_change_note(folder.as_deref(), done.map_err(|e| e.to_string())));
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start changing the export folder: {e}"));
        not_changing_folders(app);
    }
}

/// A folder change is done (from its thread): its words show, and the folders are read again.
fn changed_folders(weak: slint::Weak<AppWindow>, note: Option<String>) {
    let _ = slint::invoke_from_event_loop(move || {
        let asked = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            let p = p.as_mut()?;
            p.folders_busy = false;
            p.folders_message = note.unwrap_or_default();
            p.folders_asked += 1;
            Some((p.library.clone(), p.folders_asked))
        });
        if let (Some(app), Some((library, asked))) = (weak.upgrade(), asked) {
            show(&app);
            read_folders_later(&app, library, asked);
        }
    });
}

/// A folder change couldn't start: the buttons are free again.
fn not_changing_folders(app: &AppWindow) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.folders_busy = false;
        }
    });
    show(app);
}

/// Asks the link in Bitwig how it stands, away from the window (it may wait a few seconds for a busy Bitwig). Only
/// reads: nothing goes into the project. A check asked for while one runs replaces its answer (newest wins).
fn check_daws(app: &AppWindow) {
    let asked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live)?;
        p.daws_asked += 1;
        p.daws_checking = true;
        Some(p.daws_asked)
    });
    let Some(asked) = asked else { return };
    look_at_scripts(app);
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings Bitwig check", move || {
        let checked = mo_dawlink::bitwig::Bitwig::default().check();
        let loaded_from = checked.as_ref().ok().map(|h| h.loaded_from.clone()).filter(|f| !f.is_empty());
        let status = page::bitwig_status(&checked);
        let _ = slint::invoke_from_event_loop(move || {
            let newest = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let Some(p) = p.as_mut().filter(|p| p.daws_asked == asked) else { return false };
                p.bitwig = status;
                p.bw_loaded_from = loaded_from;
                p.daws_checking = false;
                true
            });
            if let Some(app) = weak.upgrade()
                && newest
            {
                look_at_scripts(&app);
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start checking Bitwig: {e}"));
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.daws_checking = false;
            }
        });
        show(app);
    }
}

/// Finds the DAWs' folders and whether the scripts in them are the app's own, away from the window (a project folder
/// on a drive that's asleep may take a moment).
fn look_at_scripts(app: &AppWindow) {
    let asked = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live)?;
        p.scripts_asked += 1;
        // Your Bitwig Studio folder as the card chose it, else as the C# app had it.
        let chosen = p.settings.get(app_settings::BITWIG_FOLDER_KEY).map(str::to_owned);
        // The programs chosen for the DAWs, to see they're still there (a version uninstalled since).
        let programs: Vec<String> = shell::DAW_PROGRAMS
            .iter()
            .filter_map(|(_, key, _)| p.settings.get(key).map(str::trim).filter(|e| !e.is_empty()).map(str::to_owned))
            .collect();
        Some((p.library.clone(), p.bw_loaded_from.clone(), chosen, programs, p.scripts_asked))
    });
    let Some((library, loaded_from, chosen, programs, asked)) = asked else { return };
    let weak = app.as_weak();
    let started = background::spawn("Settings DAW scripts", move || {
        let roots = match Library::open_read_only(&library) {
            Ok(l) => l.roots().unwrap_or_default(),
            Err(e) => {
                log::warning("Settings", &format!("Couldn't read the library, to find the DAWs' folders: {e}"));
                Vec::new()
            }
        };
        let documents = crate::files::documents();
        let folders = scripts::fl_script_folders(&scripts::fl_user_data_folders(&documents, &roots));
        let state = scripts::fl_state(&folders);
        let users = scripts::bitwig_user_folders(chosen.as_deref(), std::slice::from_ref(&documents));
        let bw_folders = scripts::bitwig_extension_folders(&users, loaded_from.as_deref());
        let source = std::env::current_exe().ok().and_then(|exe| scripts::bitwig_extension_source(&exe));
        let bw_version = source.as_deref().and_then(scripts::extension_version);
        let bw_state = source.as_deref().map_or(ScriptState::NotInstalled, |s| scripts::bitwig_state(s, &bw_folders));
        let fl_files = scripts::fl_installed_files(&folders);
        let bw_files = scripts::bitwig_installed_files(&bw_folders, &users);
        let missing: HashSet<String> = programs.into_iter().filter(|exe| !shell::usable_exe(exe)).collect();
        let _ = slint::invoke_from_event_loop(move || {
            let newest = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let Some(p) = p.as_mut().filter(|p| p.scripts_asked == asked) else { return false };
                p.programs_missing = missing;
                p.fl_state = Some(state);
                p.fl_folders = folders;
                p.bw_source = source;
                p.bw_version = bw_version;
                p.bw_state = Some(bw_state);
                p.bw_folders = bw_folders;
                p.bw_users = users;
                p.fl_files = fl_files;
                p.bw_files = bw_files;
                true
            });
            if newest && let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start looking for FL Studio's folders: {e}"));
        // After what Install or a choice said, if anything.
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                let couldnt = "Couldn't look for the DAWs' folders just now. Press Check again.";
                p.daws_message = format!("{} {couldnt}", p.daws_message).trim_start().to_owned();
            }
        });
        show(app);
    }
}

/// Keeps your Bitwig Studio folder (`None`: the one in Documents), with whether it looks like one, in the app's own
/// settings file, never in library.db (the C# app's choice stays as it was there), and looks for Bitwig's folders
/// again. Not while the extension is being installed, which goes into the folders found before: the picker doesn't
/// hold up the window, so Install may have been pressed meanwhile.
fn choose_bitwig_folder(app: &AppWindow, chosen: Option<(PathBuf, bool)>) {
    if PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.daws_installing)) {
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                let doing =
                    if p.daws_taking_out { "a link was being taken out" } else { "the extension was being installed" };
                p.daws_message = format!("Your Bitwig Studio folder wasn't changed: {doing}. Choose it again.");
            }
        });
        show(app);
        return;
    }
    let (folder, looks) = chosen.map_or((None, true), |(f, looks)| (Some(f), looks));
    let value = folder.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
    let kept = remember(app_settings::BITWIG_FOLDER_KEY, &value);
    match &folder {
        Some(f) => log::info("Settings", &format!("Bitwig Studio folder chosen: {}", f.display())),
        None => log::info("Settings", "Bitwig Studio folder: the one in Documents again"),
    }
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.daws_message = page::bitwig_folder_note(folder.as_deref(), looks, kept);
            // Looked for again: no Install into the folders found before meanwhile.
            p.bw_state = None;
            p.bw_folders.clear();
            p.bw_files.clear();
        }
    });
    look_at_scripts(app);
    show(app);
}

/// Keeps the program `daw`'s songs open in (`None`: the system's choice, kept as "" so the C# app's choice no longer
/// counts) in the app's own settings file. Open reads it at each click, so it counts from the next one.
fn choose_program(app: &AppWindow, daw: &str, exe: Option<&Path>) {
    let key = shell::daw_exe_key(daw);
    let value = exe.map(|e| e.display().to_string()).unwrap_or_default();
    // The one in use already (the C# app's, say): nothing changes, and the card says so.
    let already = exe.is_some() && self::value(&key).is_some_and(|v| v.trim() == value);
    let kept = remember(&key, &value);
    match exe {
        Some(e) => log::info("Settings", &format!("{daw} songs open in {} from now on", e.display())),
        None => log::info("Settings", &format!("{daw} songs open in the system's choice from now on")),
    }
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.programs_message = page::program_note(daw, exe.map(|_| value.as_str()), already, kept, Os::THIS);
            // Just picked, so it's there.
            p.programs_missing.remove(&value);
        }
    });
    show(app);
}

/// Changes the list of more Bitwig Library folders and keeps it in the app's own settings file. Space reads it at its
/// next look for Bitwig copies.
fn edit_bitwig_libraries(app: &AppWindow, edit: impl FnOnce(&mut Vec<PathBuf>)) {
    let mut list = value(app_settings::BITWIG_LIBRARIES_KEY).map(|v| app_settings::folder_list(&v)).unwrap_or_default();
    edit(&mut list);
    let value = app_settings::folder_list_value(&list);
    let kept = remember(app_settings::BITWIG_LIBRARIES_KEY, &value);
    let said = if value.is_empty() { "none".to_owned() } else { value.replace('\n', "; ") };
    log::info("Settings", &format!("More Bitwig Library folders: {said}"));
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.daws_message = page::bitwig_libraries_note(kept);
        }
    });
    show(app);
}

/// Puts the app's FL Studio script in FL's folders (Solomon's OK, 2026-11-03), away from the window, then looks again.
fn install_fl(app: &AppWindow) {
    let folders = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live && !p.daws_installing && !p.fl_folders.is_empty())?;
        p.daws_installing = true;
        Some(p.fl_folders.clone())
    });
    let Some(folders) = folders else { return };
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings FL install", move || {
        let done = scripts::install_fl(&folders);
        for f in &done.went {
            log::info("Settings", &format!("Installed the FL Studio script in {}", f.display()));
        }
        for (f, why) in &done.failed {
            log::warning("Settings", &format!("Couldn't install the FL Studio script in {}: {why}", f.display()));
        }
        for f in &done.removed {
            log::info("Settings", &format!("Took out the FL Studio script from before the rename, {}", f.display()));
        }
        for (f, why) in &done.left {
            log::warning("Settings", &format!("Couldn't take out the older FL Studio script {}: {why}", f.display()));
        }
        let note = page::installed_note(&done);
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.daws_installing = false;
                    p.daws_message = note;
                }
            });
            if let Some(app) = weak.upgrade() {
                look_at_scripts(&app);
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start installing the FL Studio script: {e}"));
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.daws_installing = false;
            }
        });
        show(app);
    }
}

/// Copies the app's data folder to `to` and checks it, away from the window, with a card that shows how far it is.
/// The switch is made at the next start (`mo_app::data_move`).
///
/// `leaving`: the missing folder an earlier move went to, which you said yes to leaving behind (DA-2). `asked`: the
/// installer asked for this move, so its ask goes once the copy has ended, not as it starts (DA-3).
///
/// Whether the copy started: not while one is going already, or before the page is built.
fn move_data(app: &AppWindow, to: PathBuf, leaving: Option<PathBuf>, asked: bool) -> bool {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let feed = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live && p.data_moving.is_none())?;
        p.data_moving = Some(stop.clone());
        p.data_message = format!("Copying to {}\u{2026}", to.display());
        Some(p.feed.clone())
    });
    let Some(feed) = feed else { return false };
    show(app);
    let from = mo_app::paths::data_dir();
    log::info("Data folder", &format!("Copying {} to {}", from.display(), to.display()));
    let weak = app.as_weak();
    let job = feed.start(&format!("Copying the app's data to {}", to.display()), "Looking at what to copy");
    let started = background::spawn("Data folder move", move || {
        // Gigabytes over the disk: low in the system's memory too, as all bulk disk work is (F-7). On this thread
        // itself, not a bulk pool, where an hour's copy would hold up the other jobs behind it.
        background::lower_memory_priority();
        let done = mo_app::data_move::copy_leaving(&from, &to, leaving.as_deref(), &stop, &mut |done, total| {
            let fraction = if total == 0 { 1.0 } else { done as f32 / total as f32 };
            let detail = format!("{} of {}", mo_app::data_move::size_text(done), mo_app::data_move::size_text(total));
            job.progress(fraction, Some(&detail));
        });
        // The installer's ask is done with once its copy has ended, however it ended: the card and Settings say how.
        if asked {
            mo_app::install_choice::take_move_ask();
        }
        let message = match &done {
            Ok(c) => {
                let mut line = format!(
                    "Copied and checked {} files ({}) to {}",
                    c.files,
                    mo_app::data_move::size_text(c.bytes),
                    to.display()
                );
                if c.at_start > 0 {
                    line.push_str(&format!("; {} still changing go at the next start", c.at_start));
                }
                if !c.notes.is_empty() {
                    line.push_str(&format!("; {}", c.notes.join("; ")));
                }
                log::info("Data folder", &line);
                job.done_as("The app's data is copied", "It switches to the new folder when the app next starts");
                String::new()
            }
            Err(why) if why == "Stopped" => {
                log::info("Data folder", "Copy stopped; the copy so far stays and a new Move goes on from there");
                job.done_as("Copy stopped", "Move again to the same place to go on");
                "Stopped. The copy so far stays: move to the same place again to go on from there.".to_owned()
            }
            Err(why) => {
                log::warning("Data folder", &format!("Couldn't copy the data folder to {}: {why}", to.display()));
                job.failed_as("Couldn't copy the app's data", why);
                format!("Couldn't copy: {why}")
            }
        };
        let pending = mo_app::data_move::pending();
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.data_moving = None;
                    p.data_message = message;
                    p.data_pending = pending;
                }
            });
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Data folder", &format!("Couldn't start copying the data folder: {e}"));
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.data_moving = None;
                p.data_message = format!("Couldn't start the copy: {e}");
            }
        });
        show(app);
        return false;
    }
    true
}

/// Puts the app's own Bitwig extension in Bitwig's folders and takes out the older script (Solomon's OK, 2026-11-03),
/// away from the window, then looks again.
fn install_bitwig(app: &AppWindow) {
    let job = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live && !p.daws_installing && !p.bw_folders.is_empty())?;
        let source = p.bw_source.clone()?;
        p.daws_installing = true;
        Some((source, p.bw_folders.clone(), p.bw_users.clone()))
    });
    let Some((source, folders, users)) = job else { return };
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings Bitwig install", move || {
        let done = scripts::install_bitwig(&source, &folders, &users);
        for f in &done.installed.went {
            log::info("Settings", &format!("Installed the Bitwig extension in {}", f.display()));
        }
        for f in &done.removed {
            let what = if f.extension().is_some_and(|e| e == "bwextension") { "extension" } else { "script" };
            log::info("Settings", &format!("Took out the older Bitwig {what} {}", f.display()));
        }
        for (f, why) in &done.installed.left {
            log::warning("Settings", &format!("Couldn't take out the older Bitwig copy {}: {why}", f.display()));
        }
        for (f, why) in &done.installed.failed {
            log::warning("Settings", &format!("Couldn't install the Bitwig extension in {}: {why}", f.display()));
        }
        let note = page::bitwig_installed_note(&done);
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.daws_installing = false;
                    p.daws_message = note;
                }
            });
            if let Some(app) = weak.upgrade() {
                look_at_scripts(&app);
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start installing the Bitwig extension: {e}"));
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.daws_installing = false;
            }
        });
        show(app);
    }
}

/// Takes the app's link out of a DAW (Take out, once asked): the files found there, away from the window.
fn take_out(app: &AppWindow, bitwig: bool) {
    let files = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live && !p.daws_installing)?;
        let files = if bitwig { p.bw_files.clone() } else { p.fl_files.clone() };
        if files.is_empty() {
            return None;
        }
        p.daws_installing = true;
        p.daws_taking_out = true;
        Some(files)
    });
    app.global::<SettingsData>().set_take_out_asking(SharedString::new());
    let Some(files) = files else { return };
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings take out", move || {
        let done = scripts::take_out(&files);
        for f in &done.removed {
            log::info("Settings", &format!("Took the DAW link out: {}", f.display()));
        }
        for (f, why) in &done.failed {
            log::warning("Settings", &format!("Couldn't take the DAW link out: {}: {why}", f.display()));
        }
        let note = page::taken_out_note(bitwig, &done);
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.daws_installing = false;
                    p.daws_taking_out = false;
                    p.daws_message = note;
                }
            });
            if let Some(app) = weak.upgrade() {
                look_at_scripts(&app);
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start taking the DAW link out: {e}"));
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.daws_installing = false;
                p.daws_taking_out = false;
            }
        });
        show(app);
    }
}

/// Shows Bitwig's extension as found, for the screenshots.
pub fn show_bitwig_extension(app: &AppWindow, state: ScriptState, version: &str, folders: Vec<PathBuf>) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.bw_source = Some(PathBuf::from(scripts::BITWIG_EXTENSION_NAME));
            p.bw_version = Some(version.to_owned());
            p.bw_files = if state == ScriptState::NotInstalled {
                Vec::new()
            } else {
                folders.iter().map(|f| f.join(scripts::BITWIG_EXTENSION_NAME)).collect()
            };
            p.bw_state = Some(state);
            p.bw_folders = folders;
        }
    });
    show(app);
}

/// Shows FL Studio's script as found, for the screenshots.
pub fn show_fl(app: &AppWindow, state: ScriptState, folders: Vec<PathBuf>) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.fl_files = if state == ScriptState::NotInstalled {
                Vec::new()
            } else {
                folders.iter().map(|f| f.join(scripts::FL_SCRIPT_NAME)).collect()
            };
            p.fl_state = Some(state);
            p.fl_folders = folders;
        }
    });
    show(app);
}

/// Shows how Bitwig's link stands, for the screenshots.
pub fn show_bitwig(app: &AppWindow, status: page::DawStatus) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.bitwig = status;
        }
    });
    show(app);
}

/// The folders as the C# app keeps them, read-only.
fn read_folders(library: &Path) -> Result<Vec<page::FolderGroup>, String> {
    let lib = Library::open_read_only(library).map_err(|e| e.to_string())?;
    let list = |key: &str| lib.list_setting(key).map_err(|e| e.to_string());
    let moved = lib
        .setting("space_moved_folders")
        .map_err(|e| e.to_string())?
        .map(|json| mo_space::folder_move::parse_moved(&json))
        .unwrap_or_default();
    let defaults = plugin_defaults();
    Ok(page::folder_groups(
        lib.roots().map_err(|e| e.to_string())?,
        list("sample_folders")?,
        list("midi_folders")?,
        list("plugin_folders")?,
        defaults,
        &moved,
    ))
}

/// Shows the folder groups (or why they couldn't be read).
pub fn show_folders(app: &AppWindow, groups: Result<Vec<page::FolderGroup>, String>) {
    let data = app.global::<SettingsData>();
    match groups {
        Ok(groups) => {
            let lists = groups.iter().map(|g| g.list).collect();
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.folder_lists = lists;
                }
            });
            let lines: Vec<FolderGroupInfo> = groups
                .into_iter()
                .map(|g| FolderGroupInfo {
                    title: g.title.into(),
                    editable: g.list.is_some(),
                    note: g.note.into(),
                    folders: strings(g.folders),
                    empty: g.empty.into(),
                })
                .collect();
            data.set_folder_groups(ModelRc::new(VecModel::from(lines)));
            data.set_folders_note(page::FOLDERS_NOTE.into());
        }
        Err(e) => {
            log::warning("Settings", &format!("Couldn't read the folders from the library: {e}"));
            data.set_folder_groups(ModelRc::default());
            data.set_folders_note(format!("Couldn't read your folders from the library: {e}").into());
        }
    }
}

/// Where exports go, as every export finds it ([`mo_app::bitwig_convert::export_folder_in`]): the setting from the
/// library, checked on the disk. Opens the library and looks at the disk, so never on the window's thread.
fn read_exports(library: &Path) -> page::ExportsCard {
    use mo_app::bitwig_convert as bitwig;
    let setting = match bitwig::export_setting_in(library) {
        Ok(setting) => setting,
        Err(why) => return page::exports_card(None, &Err(why), false),
    };
    let checked = bitwig::export_folder(setting.as_deref(), bitwig::documents().as_deref()).and_then(|folder| {
        // Documents' own isn't looked at by an export until it makes it: something else by its name stops that.
        match std::fs::metadata(&folder) {
            Ok(m) if !m.is_dir() => {
                Err(format!("{} is a file, not a folder, so nothing can be exported there.", folder.display()))
            }
            _ => Ok(folder),
        }
    });
    let there = checked.as_ref().is_ok_and(|folder| folder.is_dir());
    page::exports_card(setting.as_deref(), &checked, there)
}

/// Shows the Folders tab's Exports card.
pub fn show_exports(app: &AppWindow, card: &page::ExportsCard) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.export_folder = if card.there { card.folder.clone() } else { String::new() };
        }
    });
    let data = app.global::<SettingsData>();
    data.set_export_folder(card.folder.clone().into());
    data.set_export_line(card.note.clone().into());
    data.set_export_chosen(card.chosen);
    data.set_export_there(card.there);
}

/// The Exports card's Open: shows the folder exports go into, off the window's thread.
fn open_export_folder() {
    let found = PAGE.with(|p| {
        let p = p.borrow();
        let p = p.as_ref().filter(|p| p.live && !p.export_folder.is_empty())?;
        Some((p.feed.clone(), p.export_folder.clone()))
    });
    if let Some((feed, folder)) = found {
        crate::files::open_folder(&feed, &folder);
    }
}

/// Lists the sound outputs there are now, off the window's thread (it waits for the player).
fn list_outputs(app: &AppWindow) {
    app.global::<SettingsData>().set_output_busy(true);
    let weak = app.as_weak();
    let started = background::spawn("Sound outputs", move || {
        let player = crate::sounds::player();
        player.check();
        let names = player.outputs();
        let info = player.info();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.outputs = page::outputs(&names, p.settings.get(SOUND_OUTPUT_KEY), cfg!(windows));
                }
            });
            let data = app.global::<SettingsData>();
            data.set_output_busy(false);
            data.set_output_note(page::output_note(info.name.as_deref(), info.problem.as_deref()).into());
            show(&app);
        });
    });
    if let Err(e) = started {
        app.global::<SettingsData>().set_output_busy(false);
        log::error("Settings", &format!("Couldn't start listing the sound outputs: {e}"));
    }
}

/// The made-up sound outputs, for the screenshots.
pub fn show_outputs(app: &AppWindow, names: &[String], note: &str) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.outputs = page::outputs(names, p.settings.get(SOUND_OUTPUT_KEY), true);
        }
    });
    app.global::<SettingsData>().set_output_note(note.into());
    show(app);
}

/// Fills the page from the settings as they are now.
fn show(app: &AppWindow) {
    PAGE.with(|p| {
        let p = p.borrow();
        let Some(p) = p.as_ref() else { return };
        let s = &p.settings;
        let data = app.global::<SettingsData>();
        let tab = Tab::from_setting(s.get(TAB_KEY));
        data.set_tab(tab.index() as i32);
        data.set_summary(tab.summary().into());

        let cards: Vec<ThemeCardInfo> = page::theme_cards(s.get(THEME_KEY))
            .into_iter()
            .map(|c| ThemeCardInfo {
                name: c.name.into(),
                description: c.description.into(),
                background: crate::rgb(c.background),
                sidebar: crate::rgb(c.sidebar),
                surface: crate::rgb(c.surface),
                text: crate::rgb(c.text),
                muted: crate::rgb(c.muted),
                accent: crate::rgb(c.accent),
                square: c.square,
                picked: c.picked,
            })
            .collect();
        let note = cards.iter().find(|c| c.picked).map(|c| c.description.clone()).unwrap_or_default();
        data.set_themes(ModelRc::new(VecModel::from(cards)));
        data.set_theme_note(note);
        let theme = s.get(THEME_KEY).and_then(theme_named).unwrap_or(THEMES[0]);
        let accents: Vec<AccentInfo> = page::accent_choices(&theme, s.get(ACCENT_KEY))
            .into_iter()
            .map(|a| AccentInfo { name: a.name.into(), colour: crate::rgb(a.colour), picked: a.picked })
            .collect();
        data.set_accents(ModelRc::new(VecModel::from(accents)));
        data.set_font(page::font_index(s.get(FONT_KEY)) as i32);
        data.set_size(page::size_index(s.get(SIZE_KEY)) as i32);
        data.set_size_note(page::size_note(size_from_setting(s.get(SIZE_KEY))).into());

        let start = page::start_index(s.get(START_PAGE_KEY));
        data.set_start_index(start as i32);
        let pace = Pace::from_setting(s.get(PACE_KEY));
        data.set_pace(Pace::ALL.iter().position(|x| *x == pace).unwrap_or(2) as i32);
        data.set_pace_note(page::pace_note(pace, p.running_pace, background::cores()).into());
        data.set_sweep_hold(sweep_hold(s));
        show_curve_keys(&data, mo_app::curve_keys::KeyMap::from_setting(s.get(mo_app::curve_keys::SETTING)));
        data.set_helper_on(mo_app::helper_start::wanted(s));
        data.set_helper_intro(page::helper_intro(cfg!(windows)).into());
        let smallest = smallest_part(s);
        data.set_smallest_index(mo_app::midi::smallest_part_index(smallest) as i32);
        data.set_outputs(strings(p.outputs.labels.iter().cloned()));
        data.set_output(p.outputs.labels.get(p.outputs.picked).cloned().unwrap_or_default().into());

        let facts: Vec<SettingsFact> = page::developer_facts(
            &mo_app::version::version(),
            cfg!(debug_assertions),
            &p.folder,
            &mo_app::paths::logs_dir(),
            log::current().and_then(|l| l.path()),
        )
        .into_iter()
        .map(|f| SettingsFact {
            what: f.what.into(),
            value: f.value.into(),
            button: f.button.map_or("", page::FactButton::name).into(),
        })
        .collect();
        data.set_facts(ModelRc::new(VecModel::from(facts)));

        let keys = crate::shortcuts();
        let lines: Vec<KeyLineInfo> = page::key_lines(&keys, &p.keys_search)
            .into_iter()
            .map(|l| KeyLineInfo {
                page: l.page.into(),
                id: l.id.into(),
                what: l.what.into(),
                key: l.key.into(),
                changed: l.changed,
                starts: l.starts.into(),
            })
            .collect();
        data.set_key_lines(ModelRc::new(VecModel::from(lines)));
        data.set_listening(p.listening.unwrap_or_default().into());
        data.set_keys_message(p.keys_message.clone().into());
        let curve_keys = mo_app::curve_keys::KeyMap::from_setting(p.settings.get(mo_app::curve_keys::SETTING));
        data.set_keys_changed(keys.any_changed() || curve_keys != mo_app::curve_keys::KeyMap::default());

        let fl = page::fl_status(p.fl_state, &p.fl_folders);
        let mut bitwig = p.bitwig.clone();
        let built = p.bw_source.as_ref().map(|_| p.bw_version.clone());
        let (more, bw_install) = page::bitwig_extension_facts(p.bw_state, built, &p.bw_folders);
        bitwig.facts.extend(more);
        let fl_install = match p.fl_state {
            Some(ScriptState::NotInstalled) if !p.fl_folders.is_empty() => "Install",
            Some(ScriptState::Outdated) => "Update",
            Some(ScriptState::Installed) => "Install again",
            _ => "",
        };
        let row = |d: page::DawStatus, install: &str, files: &[PathBuf]| DawRowInfo {
            install: install.into(),
            take_out_ask: page::take_out_ask(d.daw == p.bitwig.daw, files).into(),
            bitwig: d.daw == p.bitwig.daw,
            daw: d.daw.into(),
            state: d.state.into(),
            ok: d.ok,
            facts: ModelRc::new(VecModel::from(
                d.facts
                    .into_iter()
                    .map(|f| SettingsFact { what: f.what.into(), value: f.value.into(), button: SharedString::new() })
                    .collect::<Vec<_>>(),
            )),
        };
        data.set_daws(ModelRc::new(VecModel::from(vec![
            row(bitwig, bw_install, &p.bw_files),
            row(fl, fl_install, &p.fl_files),
        ])));
        data.set_daws_checking(p.daws_checking);
        data.set_daws_installing(p.daws_installing);
        data.set_daws_taking_out(p.daws_taking_out);
        // An ask whose DAW has nothing left to take out (looked at again, or another folder chosen) is closed, so it
        // never comes back by itself; so is one while Install or Take out runs.
        let asking = data.get_take_out_asking();
        let still = |files: &[PathBuf]| !files.is_empty() && !p.daws_installing;
        let open = (asking.as_str() == p.bitwig.daw && still(&p.bw_files))
            || (asking.as_str() == "FL Studio" && still(&p.fl_files));
        if !asking.is_empty() && !open {
            data.set_take_out_asking(SharedString::new());
        }
        data.set_daws_message(p.daws_message.clone().into());
        data.set_bitwig_chosen(p.settings.get(app_settings::BITWIG_FOLDER_KEY).is_some_and(|f| !f.trim().is_empty()));
        let libraries = app_settings::folder_list(p.settings.get(app_settings::BITWIG_LIBRARIES_KEY).unwrap_or(""));
        data.set_bitwig_libraries(strings(libraries.iter().map(|f| f.display().to_string())));
        // The screenshots show the words as on Windows, where the app is used.
        let os = if p.live { Os::THIS } else { Os::Windows };
        let programs: Vec<DawProgramInfo> = page::daw_programs(
            |k| p.settings.get(k).map(str::to_owned),
            |k| p.settings.changed_here(k),
            |exe| p.programs_missing.contains(exe),
            os,
        )
        .into_iter()
        .map(|l| DawProgramInfo {
            daw: l.daw.into(),
            program: l.program.into(),
            chosen: l.chosen,
            note: l.note.into(),
            choose_tip: l.choose_tip.into(),
            reset: l.reset.into(),
            reset_tip: l.reset_tip.into(),
        })
        .collect();
        data.set_programs_title(page::PROGRAMS_TITLE.into());
        data.set_programs_intro(page::programs_intro(os).into());
        data.set_daw_programs(ModelRc::new(VecModel::from(programs)));
        data.set_programs_note(p.programs_message.clone().into());
        data.set_folders_busy(p.folders_busy);
        data.set_folders_message(p.folders_message.clone().into());

        let pending = p.data_pending.clone();
        let mut problems: Vec<String> =
            [mo_app::paths::problem(), mo_app::data_move::start_problem()].into_iter().flatten().collect();
        if let Some(left) = mo_app::paths::moved_leaving() {
            problems.push(format!(
                "The app no longer uses {left}: it couldn't be found when this move was made, and its data there is \
                 untouched."
            ));
        }
        let mut note = mo_app::data_move::status_note(
            pending.as_deref(),
            mo_app::paths::moved_from().as_deref(),
            &problems,
            p.data_left.as_deref(),
        );
        if !p.data_message.is_empty() {
            note = if note.is_empty() { p.data_message.clone() } else { format!("{}\n{note}", p.data_message) };
        }
        data.set_data_folder(mo_app::paths::data_dir().display().to_string().into());
        data.set_data_note(note.into());
        data.set_data_moving(p.data_moving.is_some());
        data.set_data_ready(pending.is_some() && p.data_moving.is_none());
    });
}

/// A Developer fact's button: open its folder, or show this run's log in its folder.
fn open_fact(i: i32) {
    let found = PAGE.with(|p| {
        let p = p.borrow();
        let p = p.as_ref()?;
        let facts = page::developer_facts(
            "",
            cfg!(debug_assertions),
            &p.folder,
            &mo_app::paths::logs_dir(),
            log::current().and_then(|l| l.path()),
        );
        let fact = facts.into_iter().nth(usize::try_from(i).ok()?)?;
        Some((p.feed.clone(), fact))
    });
    let Some((feed, fact)) = found else { return };
    match fact.button {
        Some(page::FactButton::Open) => crate::files::open_folder(&feed, &fact.value),
        Some(page::FactButton::Show) => {
            let dir = Path::new(&fact.value).parent().map(|d| d.display().to_string()).unwrap_or_default();
            crate::files::reveal(&feed, &fact.value, &dir);
        }
        None => {}
    }
}

/// Draws everything at the size the settings choose, as a part of the system's own scale (so a 4K screen at 150% and
/// "Size of everything" 80% draws at 1.2). Call once the window is up, and after each change. When the system changes
/// the scale itself (the window moved to another screen), that's its new scale: the next pick of a size starts there.
pub fn apply_size(app: &AppWindow) {
    let size = PAGE.with(|p| p.borrow().as_ref().map(|p| size_from_setting(p.settings.get(SIZE_KEY))));
    let Some(size) = size else { return };
    let window = app.window();
    let now = window.scale_factor();
    let (had, known) = (SIZED.get(), SYSTEM.get());
    // The scale is what this last set, unless the system has set its own since (then that's the system's).
    let system = if known > 0.0 && (now - known * had).abs() < 0.01 { known } else { now };
    let wanted = size as f32 / 100.0;
    let scale = system * wanted;
    SYSTEM.set(system);
    SIZED.set(wanted);
    if (scale - now).abs() < 0.001 {
        return;
    }
    let physical = window.size();
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    window.dispatch_event(WindowEvent::Resized { size: physical.to_logical(scale) });
    log::info("Settings", &format!("Everything drawn at {size}% (scale {scale:.2})"));
}
