//! The folder tree beside the Sounds page's filters (Solomon, 24 Nov 2026): its lines from the newest counts
//! ([`mo_app::sound_folders::rows`]), and what a click on it does. Its picks and hidden folders are the page's filters
//! (`Filters::folders`, `Filters::hidden_folders`); which branches are open, whether it's folded away and the words in
//! its search box are kept here. Picks, hidden folders, open branches and the fold are kept as settings, so the tree
//! is as you left it at the next start.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

use mo_app::sound_folders::{self as tree, FolderPick, Picked, TreeAsk};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{FEED, PAGE, Page, refresh};
use crate::{AppWindow, MenuItem, SoundsData, TreeLine, TreePick};

#[derive(Default)]
struct Tree {
    /// The folders opened, by [`tree::key`].
    open: HashSet<String>,
    folded: bool,
    search: String,
    /// The folder whose right-click menu is open.
    menu_for: String,
    /// The settings were read (once, as the sounds first show).
    read: bool,
}

thread_local! {
    static TREE: RefCell<Tree> = RefCell::default();
}

/// Your sample folders, the tree's tops, and those whose drive wasn't there as they were last read (by
/// [`tree::key`]): set by the thread that scans them ([`tops_read`]).
static TOPS: Mutex<(Vec<String>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));

/// The sample folders were read for a scan, off the window's thread: each one's drive is looked at there too.
pub(super) fn tops_read(folders: &[String]) {
    let away: Vec<String> =
        folders.iter().filter(|f| !std::path::Path::new(f.as_str()).exists()).map(|f| tree::key(f)).collect();
    let mut tops = TOPS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *tops = (folders.to_vec(), away);
}

/// The picks, hidden folders, open branches and fold as you left them, the first time the sounds show.
pub(super) fn as_left(p: &mut Page) {
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        if t.read {
            return;
        }
        t.read = true;
        let value = |k: &str| crate::settings::value(k).unwrap_or_default();
        p.filters.folders = tree::decode_picks(&value(tree::PICKS_KEY));
        p.filters.hidden_folders = tree::decode_paths(&value(tree::HIDDEN_KEY));
        t.open = tree::decode_paths(&value(tree::OPEN_KEY)).iter().map(|o| tree::key(o)).collect();
        t.folded = value(tree::FOLDED_KEY) == "on";
    });
}

/// Keeps the picks and hidden folders for the next start.
fn keep(p: &Page) {
    crate::settings::remember(tree::PICKS_KEY, &tree::encode_picks(&p.filters.folders));
    crate::settings::remember(tree::HIDDEN_KEY, &tree::encode_paths(&p.filters.hidden_folders));
}

/// The tree's lines, chips and words as the page is now: from the newest counts (none yet: every folder, no
/// numbers).
pub(super) fn show(app: &AppWindow, p: &Page) {
    let data = app.global::<SoundsData>();
    let (tops, away) = TOPS.lock().map(|t| t.clone()).unwrap_or_default();
    let away: HashSet<String> = away.into_iter().collect();
    TREE.with(|t| {
        let t = t.borrow();
        let ask = TreeAsk {
            picks: &p.filters.folders,
            hidden: &p.filters.hidden_folders,
            open: &t.open,
            search: &t.search,
            tops: &tops,
            away: &away,
        };
        let rows = tree::rows(p.list.folders(), p.counts.as_ref().map(|c| &c.folders), ask);
        let lines: Vec<TreeLine> = rows
            .into_iter()
            .map(|r| TreeLine {
                path: r.path.into(),
                label: r.label.into(),
                depth: i32::try_from(r.depth).unwrap_or(i32::MAX),
                count: r.count.into(),
                used: r.used.into(),
                has_children: r.has_children,
                open: r.open,
                picked: match r.picked {
                    Picked::No => 0,
                    Picked::Yes => 1,
                    Picked::JustThis => 2,
                    Picked::Within => 3,
                    Picked::Part => 4,
                },
                hidden: r.hidden,
                away: r.away,
                empty: r.empty,
            })
            .collect();
        data.set_tree_lines(ModelRc::new(VecModel::from(lines)));
        data.set_tree_folded(t.folded);
        if data.get_tree_search().as_str() != t.search {
            data.set_tree_search(t.search.as_str().into());
        }
    });
    let mut picks: Vec<TreePick> = p
        .filters
        .folders
        .iter()
        .map(|f| TreePick { path: f.path.as_str().into(), text: tree::last_name(&f.path).into(), hidden: false })
        .collect();
    picks.extend(p.filters.hidden_folders.iter().map(|h| TreePick {
        path: h.as_str().into(),
        text: tree::last_name(h).into(),
        hidden: true,
    }));
    data.set_tree_picks(ModelRc::new(VecModel::from(picks)));
    let n = p.filters.folders.len();
    data.set_tree_picked(if n == 0 { SharedString::new() } else { format!("{n} picked").into() });
    data.set_tree_any(n > 0);
    data.set_tree_note(
        if n == 0 { "Tick folders to see only their sounds.".to_owned() } else { tree::shown_words(p.shown.len()) }
            .into(),
    );
    data.set_folder(tree::picked_words(&p.filters.folders).into());
}

/// The tree again, for a change of its own (a branch opened, words searched).
fn show_now(app: &AppWindow) {
    PAGE.with(|p| show(app, &p.borrow()));
}

/// A change to the picks or hidden folders: kept, and the list asked for again.
fn change(app: &AppWindow, f: impl FnOnce(&mut Page)) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        f(&mut p);
        keep(&p);
    });
    refresh(app);
}

/// What the right-click menu of a line offers: pick it (with its folders, or alone), hide it or show it again, open it
/// in Explorer, and scan the sample folders again. None has a key of its own: each is for the line clicked, and the
/// keys go by the picked sound.
fn menu_for(p: &Page, path: &str) -> Vec<MenuItem> {
    let k = tree::key(path);
    let pick = p.filters.folders.iter().find(|f| tree::key(&f.path) == k);
    let hidden = p.filters.hidden_folders.iter().any(|h| tree::key(h) == k);
    let line = |what: &str, id: &str, enabled: bool| MenuItem {
        what: what.into(),
        key: SharedString::new(),
        id: id.into(),
        enabled,
    };
    vec![
        line(if pick.is_some() { "Take this folder off" } else { "Pick this folder" }, "pick", !hidden),
        line(
            if pick.is_some_and(|f| f.just_this) {
                "With the folders inside it"
            } else {
                "Just this folder (not the ones inside)"
            },
            "just",
            !hidden,
        ),
        line(if hidden { "Show this folder again" } else { "Hide this folder" }, "hide", true),
        MenuItem { enabled: true, ..MenuItem::default() },
        line("Open in Explorer", "reveal", true),
        line("Scan your sample folders again", "scan", true),
        MenuItem { enabled: true, ..MenuItem::default() },
        line("Open every folder inside it", "open-all", true),
    ]
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let weak = app.as_weak();
    data.on_tree_toggled(move |path| {
        let Some(app) = weak.upgrade() else { return };
        // A hidden folder's line isn't picked: it's shown again from its chip or menu.
        change(&app, |p| {
            let k = tree::key(&path);
            if !p.filters.hidden_folders.iter().any(|h| tree::key(h) == k) {
                tree::toggle_pick(&mut p.filters.folders, &path);
            }
        });
    });
    let weak = app.as_weak();
    data.on_tree_opened(move |path| {
        let Some(app) = weak.upgrade() else { return };
        TREE.with(|t| {
            let mut t = t.borrow_mut();
            let k = tree::key(&path);
            if !t.open.remove(&k) {
                t.open.insert(k);
            }
            crate::settings::remember(tree::OPEN_KEY, &tree::encode_paths(&t.open));
        });
        show_now(&app);
    });
    let weak = app.as_weak();
    data.on_tree_taken_off(move |path| {
        let Some(app) = weak.upgrade() else { return };
        change(&app, |p| {
            let k = tree::key(&path);
            if p.filters.hidden_folders.iter().any(|h| tree::key(h) == k) {
                p.filters.hidden_folders.retain(|h| tree::key(h) != k);
            } else {
                p.filters.folders.retain(|f| tree::key(&f.path) != k);
            }
        });
    });
    let weak = app.as_weak();
    data.on_tree_cleared(move || {
        if let Some(app) = weak.upgrade() {
            change(&app, |p| p.filters.folders.clear());
        }
    });
    let weak = app.as_weak();
    data.on_tree_fold(move || {
        let Some(app) = weak.upgrade() else { return };
        TREE.with(|t| {
            let mut t = t.borrow_mut();
            t.folded = !t.folded;
            crate::settings::remember(tree::FOLDED_KEY, if t.folded { "on" } else { "off" });
        });
        show_now(&app);
    });
    let weak = app.as_weak();
    data.on_tree_search_edited(move |words| {
        let Some(app) = weak.upgrade() else { return };
        TREE.with(|t| t.borrow_mut().search = words.to_string());
        show_now(&app);
    });
    let weak = app.as_weak();
    data.on_tree_menu_asked(move |path| {
        let Some(app) = weak.upgrade() else { return };
        TREE.with(|t| t.borrow_mut().menu_for = path.to_string());
        let items = PAGE.with(|p| menu_for(&p.borrow(), &path));
        app.global::<SoundsData>().set_tree_menu(ModelRc::new(VecModel::from(items)));
    });
    let weak = app.as_weak();
    data.on_tree_menu_chosen(move |id| {
        let Some(app) = weak.upgrade() else { return };
        let path = TREE.with(|t| t.borrow().menu_for.clone());
        if path.is_empty() {
            return;
        }
        match id.as_str() {
            "pick" => change(&app, |p| tree::toggle_pick(&mut p.filters.folders, &path)),
            "just" => change(&app, |p| tree::toggle_just_this(&mut p.filters.folders, &path)),
            "hide" => {
                change(&app, |p| tree::toggle_hidden(&mut p.filters.hidden_folders, &mut p.filters.folders, &path))
            }
            "reveal" => {
                if let Some(feed) = FEED.with(|f| f.borrow().clone()) {
                    crate::files::open_folder(&feed, &path);
                }
            }
            "scan" => {
                super::sample_folders_changed();
            }
            "open-all" => {
                PAGE.with(|p| {
                    let p = p.borrow();
                    let folders = p.list.folders();
                    if let Some(at) = folders.find(&path) {
                        let inside = folders.inside(at);
                        TREE.with(|t| {
                            let mut t = t.borrow_mut();
                            t.open.extend(inside.into_iter().map(|n| tree::key(folders.path(n))));
                            t.open.insert(tree::key(&path));
                            crate::settings::remember(tree::OPEN_KEY, &tree::encode_paths(&t.open));
                        });
                    }
                });
                show_now(&app);
            }
            _ => {}
        }
    });
}

/// Folds the tree away or opens it, as its tab does, for the screenshots (not kept as a setting).
pub(super) fn set_folded(app: &AppWindow, folded: bool) {
    TREE.with(|t| t.borrow_mut().folded = folded);
    show_now(app);
}

/// Rediscover's forgotten folder clicked: that folder alone is picked.
pub(super) fn pick_alone(p: &mut Page, folder: &str) {
    p.filters.folders = vec![FolderPick { path: folder.to_owned(), just_this: true }];
    keep(p);
}
