//! Fills the Settings page from `mo_app::settings` and answers its choices. The settings are kept by
//! `mo_app::app_settings`: this app's own `rust-settings.json` over the C# app's choices in library.db, which is only
//! read. Writing the file, reading the folders from the library and listing the sound outputs all happen off the
//! window's thread (rules 12 and 21); a change shows at once and is written in the background, newest first.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use mo_app::activity::Feed;
use mo_app::app_settings::{self, AppSettings, PACE_KEY, SOUND_OUTPUT_KEY, START_PAGE_KEY, TAB_KEY};
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
    daws_checking: bool,
    daws_asked: u64,
    /// FL Studio's script: whether it's installed (none while being looked at), the folders it goes into, and whether
    /// Install is running; what the last Install did.
    fl_state: Option<ScriptState>,
    fl_folders: Vec<PathBuf>,
    daws_installing: bool,
    daws_message: String,
    feed: Feed,
}

thread_local! {
    static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
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
        let saved = app_settings::save(&folder, &json);
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
            keys_message: String::new(),
            bitwig: page::bitwig_checking(),
            folder_lists: Vec::new(),
            folders_busy: false,
            folders_message: String::new(),
            daws_checking: false,
            daws_asked: 0,
            fl_state: None,
            fl_folders: Vec::new(),
            daws_installing: false,
            daws_message: String::new(),
            feed: setup.feed,
        })
    });
    app.set_settings_built(true);

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
    data.set_keys_note(page::KEYS_NOTE.into());
    data.set_daws_note(page::DAWS_NOTE.into());
    data.set_kept_note(
        format!("Your changes are kept in {}; the C# app's settings are only read", app_settings::FILE).into(),
    );
    show(app);

    let weak = app.as_weak();
    data.on_tab_picked(move |i| {
        let Some(tab) = usize::try_from(i).ok().and_then(Tab::from_index) else { return };
        change(TAB_KEY, tab.name());
        if let Some(app) = weak.upgrade() {
            show(&app);
            if tab == Tab::Daws {
                check_daws(&app);
            }
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
    data.on_check_daws(move || {
        if let Some(app) = weak.upgrade() {
            check_daws(&app);
        }
    });
    let weak = app.as_weak();
    data.on_install_fl(move || {
        if let Some(app) = weak.upgrade() {
            install_fl(&app);
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
    data.on_refresh_outputs(move || {
        if let Some(app) = weak.upgrade() {
            list_outputs(&app);
        }
    });
    data.on_open_fact(open_fact);
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
        let _ = slint::invoke_from_event_loop(move || {
            let newest = PAGE.with(|p| p.borrow().as_ref().is_some_and(|p| p.folders_asked == asked));
            if let Some(app) = weak.upgrade()
                && newest
            {
                show_folders(&app, groups);
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
        match &done {
            Ok(folders::Changed::Done(_)) => {
                log::info("Settings", &format!("{} {folder} ({list:?} folders)", if add { "Added" } else { "Removed" }))
            }
            Ok(folders::Changed::AsItWas) => {}
            Err(e) => log::warning("Settings", &format!("Couldn't change the {list:?} folders: {e}")),
        }
        let done = done.map(|d| matches!(d, folders::Changed::Done(_))).map_err(|e| e.to_string());
        let note = page::folder_change_note(add, &folder, done);
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
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start changing the folders: {e}"));
        PAGE.with(|p| {
            if let Some(p) = p.borrow_mut().as_mut() {
                p.folders_busy = false;
            }
        });
        show(app);
    }
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
    look_at_fl(app);
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings Bitwig check", move || {
        let status = page::bitwig_status(&mo_dawlink::bitwig::Bitwig::default().check());
        let _ = slint::invoke_from_event_loop(move || {
            let newest = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let Some(p) = p.as_mut().filter(|p| p.daws_asked == asked) else { return false };
                p.bitwig = status;
                p.daws_checking = false;
                true
            });
            if let Some(app) = weak.upgrade()
                && newest
            {
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

/// Finds FL Studio's folders and whether the script in them is the app's own, away from the window (a project folder
/// on a drive that's asleep may take a moment).
fn look_at_fl(app: &AppWindow) {
    let Some(library) = PAGE.with(|p| p.borrow().as_ref().filter(|p| p.live).map(|p| p.library.clone())) else {
        return;
    };
    let weak = app.as_weak();
    let started = background::spawn("Settings FL script", move || {
        let roots = Library::open_read_only(&library).and_then(|l| l.roots()).unwrap_or_else(|e| {
            log::warning("Settings", &format!("Couldn't read the project folders, to find FL Studio's: {e}"));
            Vec::new()
        });
        let folders = scripts::fl_script_folders(&scripts::fl_user_data_folders(&crate::files::documents(), &roots));
        let state = scripts::fl_state(&folders);
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                if let Some(p) = p.borrow_mut().as_mut() {
                    p.fl_state = Some(state);
                    p.fl_folders = folders;
                }
            });
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start looking for FL Studio's folders: {e}"));
    }
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
                look_at_fl(&app);
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
        data.set_start_page(page::start_setting(start).unwrap_or_default().into());
        let pace = Pace::from_setting(s.get(PACE_KEY));
        data.set_pace(Pace::ALL.iter().position(|x| *x == pace).unwrap_or(2) as i32);
        data.set_pace_note(page::pace_note(pace, p.running_pace, background::cores()).into());
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
        let lines: Vec<KeyLineInfo> = page::key_lines(&keys)
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
        data.set_daws(ModelRc::new(VecModel::from(vec![row(p.bitwig.clone(), ""), row(fl, fl_install)])));
        data.set_daws_checking(p.daws_checking);
        data.set_daws_installing(p.daws_installing);
        data.set_daws_message(p.daws_message.clone().into());
        data.set_folders_busy(p.folders_busy);
        data.set_folders_message(p.folders_message.clone().into());
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
