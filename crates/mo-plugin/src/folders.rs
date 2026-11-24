//! The folder tree beside the Sounds tab's filter column, as on the app's Sounds page (Solomon, 24 Nov 2026): the lines
//! from mo-app's `sound_folders`, over the list's newest counts. The folders the app hides are hidden here too, read
//! from its settings with the library; picks, open branches and what's hidden or shown again here are this panel's
//! own (the plugin never writes the app's settings).

use std::cell::RefCell;
use std::collections::HashSet;

use mo_app::sound_folders::{self as tree, Picked, TreeAsk};
use mo_app::sounds::SoundCounts;
use slint::{ModelRc, SharedString, VecModel};

use crate::library::{Asked, Library};
use crate::ui::{TreeLine, TreePick};
use crate::{MenuItem, Panel};

/// What the app says about folders, read with the library: your sample folders (the tree's tops), those whose drive
/// isn't there (by [`tree::key`]), and the folders hidden on its Sounds page.
#[derive(Clone, Debug, Default)]
pub struct FolderSettings {
    pub tops: Vec<String>,
    pub away: Vec<String>,
    pub hidden: Vec<String>,
}

/// Reads them: the sample folders from the app's song library, the hidden folders from its settings file, and each
/// sample folder's drive. Reads files and asks drives that may be asleep: on a thread of its own, so the list never
/// waits for it.
pub fn read() -> FolderSettings {
    let data = mo_app::paths::data_dir();
    let tops = mo_library::Library::open_read_only(&mo_library::songs_db::to_read(&data))
        .and_then(|l| l.list_setting("sample_folders"))
        .unwrap_or_default();
    let away = tops.iter().filter(|t| !std::path::Path::new(t.as_str()).exists()).map(|t| tree::key(t)).collect();
    let hidden = tree::decode_paths(&crate::midi::setting(&data, tree::HIDDEN_KEY).unwrap_or_default());
    FolderSettings { tops, away, hidden }
}

/// The tree's own state on this panel: the branches open, whether it's folded away, the words in its search box,
/// the drives away, and the line whose menu is open.
#[derive(Default)]
pub struct TreeState {
    pub open: HashSet<String>,
    pub folded: bool,
    pub search: String,
    pub away: HashSet<String>,
    pub menu_for: String,
}

/// The tree on the panel as `asked` has it, over `library`'s folders and the newest `counts` (none: no numbers).
pub fn show(
    panel: &Panel,
    library: Option<&Library>,
    counts: Option<&SoundCounts>,
    asked: &Asked,
    state: &RefCell<TreeState>,
) {
    let f = &asked.filters;
    let state = state.borrow();
    let lines: Vec<TreeLine> = match library {
        None => Vec::new(),
        Some(library) => {
            let ask = TreeAsk {
                picks: &f.folders,
                hidden: &f.hidden_folders,
                open: &state.open,
                search: &state.search,
                tops: &f.folder_tops,
                away: &state.away,
            };
            tree::rows(library.list.folders(), counts.map(|c| &c.folders), ask)
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
                .collect()
        }
    };
    panel.set_tree_lines(ModelRc::new(VecModel::from(lines)));
    panel.set_tree_folded(state.folded);
    if panel.get_tree_search().as_str() != state.search {
        panel.set_tree_search(state.search.as_str().into());
    }
    let mut picks: Vec<TreePick> = f
        .folders
        .iter()
        .map(|p| TreePick { path: p.path.as_str().into(), text: tree::last_name(&p.path).into(), hidden: false })
        .collect();
    picks.extend(f.hidden_folders.iter().map(|h| TreePick {
        path: h.as_str().into(),
        text: tree::last_name(h).into(),
        hidden: true,
    }));
    panel.set_tree_picks(ModelRc::new(VecModel::from(picks)));
    let n = f.folders.len();
    panel.set_tree_picked(if n == 0 { SharedString::new() } else { format!("{n} picked").into() });
    panel.set_tree_any(n > 0);
    panel.set_tree_note(if n == 0 {
        SharedString::from("Tick folders to see only their sounds.")
    } else {
        SharedString::new()
    });
}

/// The right-click menu of the line at `path`: pick it (with its folders, or alone), hide it on this panel or show it
/// again, and open every folder inside it. No line has a key: each is for the line clicked.
pub fn menu(asked: &Asked, library: Option<&Library>, path: &str) -> Vec<MenuItem> {
    let k = tree::key(path);
    let f = &asked.filters;
    let pick = f.folders.iter().find(|p| tree::key(&p.path) == k);
    let hidden = f.hidden_folders.iter().any(|h| tree::key(h) == k);
    // Hidden itself or inside a hidden folder: it can't be picked (it would show nothing).
    let hidden_at = hidden || hidden_within(asked, library, path);
    let line = |what: &str, id: &str, enabled: bool| MenuItem {
        what: what.into(),
        key: SharedString::new(),
        id: id.into(),
        enabled,
    };
    vec![
        line(if pick.is_some() { "Take this folder off" } else { "Pick this folder" }, "pick", !hidden_at),
        line(
            if pick.is_some_and(|p| p.just_this) {
                "With the folders inside it"
            } else {
                "Just this folder (not the ones inside)"
            },
            "just",
            !hidden_at,
        ),
        line(if hidden { "Show this folder again here" } else { "Hide this folder here" }, "hide", true),
        MenuItem { enabled: true, ..MenuItem::default() },
        line("Open every folder inside it", "open-all", true),
    ]
}

/// The folder at `path` is hidden on this panel, itself or as a folder inside one hidden.
pub fn hidden_within(asked: &Asked, library: Option<&Library>, path: &str) -> bool {
    let k = tree::key(path);
    asked.filters.hidden_folders.iter().any(|h| tree::key(h) == k)
        || library.is_some_and(|l| l.list.folder_hidden(&asked.filters, path))
}

/// A tick on the line at `path`: picks it, or takes its pick off. A hidden folder's line (or one inside it) isn't
/// picked: it's shown again from its chip or menu.
pub fn ticked(path: &str, asked: &mut Asked, library: Option<&Library>) {
    if !hidden_within(asked, library, path) {
        tree::toggle_pick(&mut asked.filters.folders, path);
    }
}

/// A chip's ×: its pick taken off, or its hidden folder shown again (`hidden`). Only ever takes off, so a chip
/// clicked after its pick went another way changes nothing.
pub fn taken_off(path: &str, hidden: bool, asked: &mut Asked) {
    let k = tree::key(path);
    let f = &mut asked.filters;
    if hidden {
        f.hidden_folders.retain(|h| tree::key(h) != k);
    } else {
        f.folders.retain(|p| tree::key(&p.path) != k);
    }
}

/// What a chosen menu line (`id`) changes in `asked`, for the line at `path`: picks it, picks it alone or with its
/// folders, or hides it here (or shows it again).
pub fn chosen(id: &str, path: &str, asked: &mut Asked, library: Option<&Library>) {
    if path.is_empty() {
        return;
    }
    let within = hidden_within(asked, library, path);
    let f = &mut asked.filters;
    match id {
        "pick" if !within => tree::toggle_pick(&mut f.folders, path),
        "just" if !within => tree::toggle_just_this(&mut f.folders, path),
        "hide" => tree::toggle_hidden(&mut f.hidden_folders, &mut f.folders, path),
        _ => {}
    }
}

/// Opens the line at `path` and every folder inside it ("Open every folder inside it").
pub fn opened_inside(path: &str, library: Option<&Library>, state: &RefCell<TreeState>) {
    let mut state = state.borrow_mut();
    state.open.insert(tree::key(path));
    if let Some(folders) = library.map(|l| l.list.folders())
        && let Some(at) = folders.find(path)
    {
        state.open.extend(folders.inside(at).into_iter().map(|n| tree::key(folders.path(n))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_lines_pick_alone_and_hide() {
        let mut asked = Asked::default();
        let path = r"G:\Samples\Kicks";
        assert_eq!(menu(&asked, None, path)[0].what, "Pick this folder");
        chosen("pick", path, &mut asked, None);
        assert_eq!(asked.filters.folders.len(), 1);
        assert_eq!(menu(&asked, None, path)[0].what, "Take this folder off");
        chosen("just", path, &mut asked, None);
        assert!(asked.filters.folders[0].just_this);
        assert_eq!(menu(&asked, None, path)[1].what, "With the folders inside it");
        // Hiding it takes its pick off, and its pick lines can't be chosen until it's shown again.
        chosen("hide", path, &mut asked, None);
        assert!(asked.filters.folders.is_empty());
        assert_eq!(asked.filters.hidden_folders.len(), 1);
        let lines = menu(&asked, None, path);
        assert!(!lines[0].enabled && !lines[1].enabled);
        assert_eq!(lines[2].what, "Show this folder again here");
        chosen("pick", path, &mut asked, None);
        ticked(path, &mut asked, None);
        assert!(asked.filters.folders.is_empty(), "a hidden folder isn't picked");
        chosen("hide", path, &mut asked, None);
        assert!(asked.filters.hidden_folders.is_empty());
        // A chip's × only takes off: clicked again, it doesn't pick the folder back.
        ticked(path, &mut asked, None);
        taken_off(path, false, &mut asked);
        taken_off(path, false, &mut asked);
        assert!(asked.filters.folders.is_empty());
        chosen("pick", "", &mut asked, None);
        assert!(asked.filters.folders.is_empty(), "no line, no pick");
    }
}
