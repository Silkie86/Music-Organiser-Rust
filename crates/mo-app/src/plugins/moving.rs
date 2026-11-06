//! Move to another drive on the Plugins page (Solomon, 4 Nov 2026: "Yes": it leaves a link behind, as Space's folder
//! moves do). One of the plugin's preset or sound folders is copied to the folder you pick on another drive, every
//! file checked, then swapped for a link to the copy, so the plugin still finds everything where it was. The original
//! stays beside it as "… (old copy)" until you remove it on the Space page. The moving is
//! `mo_space::folder_move::move_folder`'s; the move is then added to the library's moved folders
//! (`mo_library::moves`), as the C# app keeps them, so both apps (and Space's old-copy reminders) know of it.

use mo_space::folder_move::MovedFolder;
use mo_space::plugin_space::{SpaceFolder, SpaceKind};
use mo_space::size_text;

use super::Plugins;

/// What Move looks at for one plugin.
#[derive(Clone, Debug)]
pub struct Ask {
    pub row_key: (String, String),
    pub name: String,
    /// Its name key, as the move is kept under.
    pub key: String,
    /// Its preset and sound folders, biggest first: the ones that can move.
    pub folders: Vec<SpaceFolder>,
}

impl Plugins {
    /// Whether the plugin at `row` has a preset or sound folder that could move: installed, measured.
    pub fn can_move(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| {
            r.installed && r.room.as_ref().is_some_and(|s| s.folders.iter().any(|f| f.kind != SpaceKind::Program))
        })
    }

    /// What Move needs for the plugin at `row`, or `None` ([`Plugins::can_move`]).
    pub fn move_ask(&self, row: usize) -> Option<Ask> {
        if !self.can_move(row) {
            return None;
        }
        let r = self.rows.get(row)?;
        let room = r.room.as_ref()?;
        let mut folders: Vec<SpaceFolder> =
            room.folders.iter().filter(|f| f.kind != SpaceKind::Program).cloned().collect();
        folders.sort_by_key(|f| std::cmp::Reverse(f.bytes));
        Some(Ask { row_key: (r.key.clone(), r.own.clone()), name: r.name.clone(), key: room.key.clone(), folders })
    }
}

impl Ask {
    pub fn title(&self) -> String {
        format!("Move {}'s folders to another drive", self.name)
    }

    pub fn intro(&self) -> String {
        "Pick a folder, then where on another drive it goes. It's copied there and every file checked, then a link is \
         left where it was, so the plugin finds everything as before. The folder stays as it is beside the link, \
         marked \"(old copy)\", until you remove it on the Space page. Close your DAW first."
            .to_owned()
    }
}

/// What a folder is, for its line.
pub fn folder_kind(f: &SpaceFolder) -> &'static str {
    match f.kind {
        SpaceKind::Program => "Plugin file",
        SpaceKind::Presets => "Presets and settings",
        SpaceKind::Sounds => "Sounds",
    }
}

/// The moved folders as the library keeps them now, read strictly: a value that isn't a list of moves is an error,
/// never no moves (a move checked against none could go where an earlier one did).
pub fn moved_now(json: Option<&str>) -> Result<Vec<MovedFolder>, String> {
    match json.map(str::trim).filter(|j| !j.is_empty()) {
        None => Ok(Vec::new()),
        Some(j) => serde_json::from_str::<Vec<MovedFolder>>(j)
            .map_err(|_| "The library's list of moved folders couldn't be read.".to_owned()),
    }
}

/// What the card says before the move.
pub fn ready_text(from: &str, to: &str, bytes: u64) -> String {
    format!("Move {from} ({}) to {to}? A link is left where it was.", size_text(bytes))
}

/// What the card says once the move is over: `noted` is how adding it to the library went.
pub fn moved_text(moved: &MovedFolder, noted: &Result<(), String>) -> String {
    let head = format!(
        "Moved to {}. A link is left where it was, so the plugin finds everything as before. The folder is still \
         beside it as {} until you remove it on the Space page.",
        moved.to, moved.old_copy
    );
    match noted {
        Ok(()) => head,
        Err(why) => format!(
            "{head} But the move couldn't be added to the library's moved folders ({why}), so Space and this page \
             don't know of it yet: tell Claude, or note where it went."
        ),
    }
}

/// One move as the library's moved folders keep it (the C# app's shape), for `mo_library::moves::add_moved_folder`.
pub fn moved_entry(moved: &MovedFolder) -> Result<String, String> {
    serde_json::to_string(moved).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_moves_are_read_strictly() {
        assert!(moved_now(None).unwrap().is_empty());
        assert!(moved_now(Some("  ")).unwrap().is_empty());
        assert!(moved_now(Some("oops")).is_err());
        let csharp = r#"[{"From":"C:\\A","To":"E:\\A","OldCopy":"C:\\A (old copy)","Bytes":5,"Moved":"x"}]"#;
        assert_eq!(moved_now(Some(csharp)).unwrap()[0].to, "E:\\A");
    }
}
