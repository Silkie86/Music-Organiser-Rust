//! Fills the Settings page from `mo_app::settings` and answers its choices. The settings are kept by
//! `mo_app::app_settings`: this app's own `rust-settings.json` over the C# app's choices in library.db, which is only
//! read. Writing the file, reading the folders from the library and listing the sound outputs all happen off the
//! window's thread (rules 12 and 21); a change shows at once and is written in the background, newest first.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use mo_app::activity::Feed;
use mo_app::app_settings::{self, AppSettings, PACE_KEY, SOUND_OUTPUT_KEY, START_PAGE_KEY, SWEEP_HOLD_KEY, TAB_KEY};
use mo_app::background::{self, Newest, Pace};
use mo_app::log;
use mo_app::places::Place;
use mo_app::settings::{self as page, Outputs, Tab};
use mo_app::themes::{
    ACCENT_KEY, FONT_KEY, FONTS, Look, SIZE_KEY, SIZES, THEME_KEY, THEMES, size_from_setting, theme_named,
};
use mo_dawlink::scripts::{self, ScriptState};
use mo_library::Library;
use mo_library::folders::{self, FolderList};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    AccentInfo, AppWindow, DawRowInfo, FolderGroupInfo, KeyLineInfo, SettingsData, SettingsFact, ThemeCardInfo,
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
    /// Counts the looks for the DAWs' folders, so an older one finishing late is dropped (newest wins).
    scripts_asked: u64,
    daws_installing: bool,
    daws_message: String,
    /// Moving the data folder: set while the copy runs (its Stop), and what the last copy did.
    data_moving: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    data_message: String,
    /// The folder a finished copy waits to switch to at the next start (read once, then kept here).
    data_pending: Option<PathBuf>,
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
            bw_loaded_from: None,
            scripts_asked: 0,
            daws_installing: false,
            daws_message: String::new(),
            data_moving: None,
            data_message: String::new(),
            data_pending: mo_app::data_move::pending(),
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
                move_data(&app, to);
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
    data.on_toggle_sweep_hold(move || {
        let on = !PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| sweep_hold(&p.settings)));
        change(SWEEP_HOLD_KEY, if on { "1" } else { "0" });
        crate::sounds::hold_sweeps(on);
        if let Some(app) = weak.upgrade() {
            app.global::<SettingsData>().set_sweep_hold(on);
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
            if let Some(app) = weak.upgrade() {
                move_data(&app, mo_app::data_move::target_in(&picked));
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

/// Whether a Galaxy sweep holds the pointer to the star that's playing (off unless turned on).
pub fn sweep_hold(settings: &AppSettings) -> bool {
    settings.get(SWEEP_HOLD_KEY) == Some("1")
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

/// Remembers the page on show, for "Last page I used": only while that's the start page, so other clicks on the rail
/// write nothing.
/// Shows one tab (picked on the page, or by Ctrl+K).
pub fn show_tab(app: &AppWindow, tab: Tab) {
    change(TAB_KEY, tab.name());
    show(app);
    if tab == Tab::Daws {
        check_daws(app);
    }
}

/// Goes to Settings on `tab` (from Ctrl+K, or the rail's DAW lines): the tab first, so the page asks Bitwig once as
/// it shows when that's the DAWs tab ([`show_tab`] after going there would ask again).
pub fn open_on(app: &AppWindow, tab: Tab) {
    change(TAB_KEY, tab.name());
    crate::go_to(app, Place::Settings.index() as i32);
}

pub fn page_shown(place: Place) {
    // Leaving Settings stops Shortcuts listening, so the next key on another page works as a key again.
    if place != Place::Settings {
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.listening = None;
            }
        });
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
    list_outputs(app);
    // Bitwig is only asked while its tab is on show.
    let on_daws =
        PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| Tab::from_setting(p.settings.get(TAB_KEY)) == Tab::Daws));
    if on_daws {
        check_daws(app);
    }
    read_folders_later(app, library, asked);
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

/// Chooses the folder exports go into (`Some`), or goes back to "Music Organiser Exports" in Documents (`None`), in
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
            (Ok(true), None) => log::info("Settings", "Exports go into Music Organiser Exports in Documents again"),
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
        Some((p.library.clone(), p.bw_loaded_from.clone(), chosen, p.scripts_asked))
    });
    let Some((library, loaded_from, chosen, asked)) = asked else { return };
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
        let _ = slint::invoke_from_event_loop(move || {
            let newest = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let Some(p) = p.as_mut().filter(|p| p.scripts_asked == asked) else { return false };
                p.fl_state = Some(state);
                p.fl_folders = folders;
                p.bw_source = source;
                p.bw_version = bw_version;
                p.bw_state = Some(bw_state);
                p.bw_folders = bw_folders;
                p.bw_users = users;
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
                p.daws_message =
                    "Your Bitwig Studio folder wasn't changed: the extension was being installed. Choose it again."
                        .to_owned();
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
        }
    });
    look_at_scripts(app);
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
fn move_data(app: &AppWindow, to: PathBuf) {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let feed = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().filter(|p| p.live && p.data_moving.is_none())?;
        p.data_moving = Some(stop.clone());
        p.data_message = format!("Copying to {}\u{2026}", to.display());
        Some(p.feed.clone())
    });
    let Some(feed) = feed else { return };
    show(app);
    let from = mo_app::paths::data_dir();
    log::info("Data folder", &format!("Copying {} to {}", from.display(), to.display()));
    let weak = app.as_weak();
    let job = feed.start(&format!("Copying the app's data to {}", to.display()), "Looking at what to copy");
    let started = background::spawn("Data folder move", move || {
        // Gigabytes over the disk: low in the system's memory too, as all bulk disk work is (F-7). On this thread
        // itself, not a bulk pool, where an hour's copy would hold up the other jobs behind it.
        background::lower_memory_priority();
        let done = mo_app::data_move::copy(&from, &to, &stop, &mut |done, total| {
            let fraction = if total == 0 { 1.0 } else { done as f32 / total as f32 };
            let detail = format!("{} of {}", mo_app::data_move::size_text(done), mo_app::data_move::size_text(total));
            job.progress(fraction, Some(&detail));
        });
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
    }
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
            log::info("Settings", &format!("Took out the older Bitwig script {}", f.display()));
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

/// Shows Bitwig's extension as found, for the screenshots.
pub fn show_bitwig_extension(app: &AppWindow, state: ScriptState, version: &str, folders: Vec<PathBuf>) {
    PAGE.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.bw_source = Some(PathBuf::from(scripts::BITWIG_EXTENSION_NAME));
            p.bw_version = Some(version.to_owned());
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
        data.set_keys_changed(keys.any_changed());

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
        let row = |d: page::DawStatus, install: &str| DawRowInfo {
            install: install.into(),
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
        data.set_daws(ModelRc::new(VecModel::from(vec![row(bitwig, bw_install), row(fl, fl_install)])));
        data.set_daws_checking(p.daws_checking);
        data.set_daws_installing(p.daws_installing);
        data.set_daws_message(p.daws_message.clone().into());
        data.set_bitwig_chosen(p.settings.get(app_settings::BITWIG_FOLDER_KEY).is_some_and(|f| !f.trim().is_empty()));
        let libraries = app_settings::folder_list(p.settings.get(app_settings::BITWIG_LIBRARIES_KEY).unwrap_or(""));
        data.set_bitwig_libraries(strings(libraries.iter().map(|f| f.display().to_string())));
        data.set_folders_busy(p.folders_busy);
        data.set_folders_message(p.folders_message.clone().into());

        let pending = p.data_pending.clone();
        let problems: Vec<String> =
            [mo_app::paths::problem(), mo_app::data_move::start_problem()].into_iter().flatten().collect();
        let mut note = mo_app::data_move::status_note(
            &mo_app::paths::data_dir(),
            pending.as_deref(),
            mo_app::paths::moved_from().as_deref(),
            &problems,
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
