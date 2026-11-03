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
use mo_library::Library;
use slint::platform::WindowEvent;
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AccentInfo, AppWindow, FolderGroupInfo, SettingsData, SettingsFact, ThemeCardInfo};

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
}

/// Reads the settings before the window opens: this app's file in `folder` and the C# app's choices from `library`,
/// opened read-only. What went wrong goes to the log; the app opens whatever happens.
pub fn load(folder: &Path, library: &Path) -> AppSettings {
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
    loaded.settings
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
    pub feed: Feed,
}

pub fn connect(app: &AppWindow, setup: Setup) {
    let writer = Newest::start("Save settings", |(folder, json): (PathBuf, String)| {
        if let Err(e) = app_settings::save(&folder, &json) {
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
    data.on_try_job(move || {
        if let Some(app) = weak.upgrade() {
            app.invoke_try_job();
        }
    });
    // The app opened on Settings (its start page) before the page was ready.
    if app.get_place() == Place::Settings.index() as i32 {
        shown(app);
    }
}

fn strings<S: Into<SharedString>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(Into::into).collect::<Vec<SharedString>>()))
}

fn colour(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Changes a setting and writes the file in the background, when it changed.
fn change(key: &str, value: &str) {
    let save = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut()?;
        // The screenshots change their made-up settings, and never write a file.
        (p.settings.set(key, value) && p.live).then(|| (p.folder.clone(), p.settings.to_json()))
    });
    if let Some(job) = save {
        WRITER.with(|w| {
            if let Some(w) = w.borrow().as_ref() {
                w.ask(job);
            }
        });
    }
}

/// Remembers the page on show, for "Last page I used".
pub fn page_shown(place: Place) {
    change(app_settings::LAST_PAGE_KEY, place.name());
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
    let live = PAGE.with(|p| p.borrow().as_ref().map(|p| (p.live, p.library.clone())));
    let Some((live, library)) = live else { return };
    show(app);
    if !live {
        return;
    }
    list_outputs(app);
    let weak = app.as_weak();
    let started = background::spawn("Settings folders", move || {
        let groups = read_folders(&library);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                show_folders(&app, groups);
            }
        });
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start reading the folders: {e}"));
    }
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
    let defaults = mo_space::plugins::default_folders().iter().map(|p| p.display().to_string()).collect();
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
            let lines: Vec<FolderGroupInfo> = groups
                .into_iter()
                .map(|g| FolderGroupInfo {
                    title: g.title.into(),
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
                background: colour(c.background),
                sidebar: colour(c.sidebar),
                surface: colour(c.surface),
                text: colour(c.text),
                muted: colour(c.muted),
                accent: colour(c.accent),
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
            .map(|a| AccentInfo { name: a.name.into(), colour: colour(a.colour), picked: a.picked })
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
        Some(page::FactButton::Open) => open_folder(&feed, Path::new(&fact.value)),
        Some(page::FactButton::Show) => {
            let dir = Path::new(&fact.value).parent().map(|d| d.display().to_string()).unwrap_or_default();
            crate::files::reveal(&feed, &fact.value, &dir);
        }
        None => {}
    }
}

/// Opens a folder in the system's file browser, off the window's thread.
fn open_folder(feed: &Feed, folder: &Path) {
    let (feed, folder) = (feed.clone(), folder.display().to_string());
    let started = background::spawn("Open folder", move || {
        let launch = mo_app::shell::open(&folder, None, mo_app::shell::Os::THIS);
        if let Err(e) = mo_app::shell::run(&launch) {
            log::warning("Settings", &format!("Couldn't open {folder}: {e}"));
            feed.start("Couldn't open the folder", "").failed(&e.to_string());
        }
    });
    if let Err(e) = started {
        log::error("Settings", &format!("Couldn't start opening a folder: {e}"));
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
