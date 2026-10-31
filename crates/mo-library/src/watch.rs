//! Watching folders (and everything under them) for files appearing, changing, moving or going, as the C# app's
//! `FolderWatch` does: what changed is reported together once the folders have been quiet for a moment, because a DAW
//! writes a project in several steps and a sample pack unzips thousands of files. This is how the app keeps up with
//! your projects while it's open, instead of walking every folder again.
//!
//! When too much changes at once for the system to name every file, or a whole folder moves, the watched folder is
//! reported as overflowed and the caller walks it instead.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use mo_formats::case;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::scan::{is_inside, looks_like_folder};

/// What changed while the folders were busy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Files that appeared, changed, moved or went, sorted ignoring case.
    pub files: Vec<String>,
    /// Watched folders where too much changed to name each file, or a folder moved or went: walk these again.
    pub overflowed: Vec<String>,
}

/// The most files a folder moved in is listed by; past that the watched folder is walked instead.
const MOVED_IN_LIMIT: usize = 5000;

/// Watches some folders until it's dropped.
pub struct FolderWatch {
    /// Dropping the watcher ends its events, and with them the thread that gathers them.
    _watcher: RecommendedWatcher,
    count: usize,
}

impl FolderWatch {
    /// Watches `folders` (ones that aren't there, or can't be watched, are left out: the next scan finds their changes)
    /// for files `wanted` says matter, and calls `report` on a thread of its own once nothing has changed for `quiet`.
    /// That thread waits nearly all the time; `report` should hand any real work to the app's low-priority threads.
    pub fn start(
        folders: &[String],
        wanted: fn(&str) -> bool,
        quiet: Duration,
        report: impl FnMut(Changes) + Send + 'static,
    ) -> notify::Result<FolderWatch> {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut roots: Vec<String> = Vec::new();
        for f in folders.iter().filter(|f| Path::new(f.as_str()).is_dir()) {
            if !roots.iter().any(|r| case::eq_ignore_case(r, f)) {
                roots.push(f.clone());
            }
        }
        // One watcher for every folder; each event is told apart by the folder it's inside.
        let mut watcher = notify::recommended_watcher(move |e: notify::Result<Event>| {
            let _ = tx.send(e);
        })?;
        let mut watched = Vec::new();
        for root in &roots {
            if watcher.watch(Path::new(root), RecursiveMode::Recursive).is_ok() {
                watched.push(root.clone());
            }
        }
        let count = watched.len();
        let gather = Gather { roots: watched, wanted, changes: Pending::default() };
        thread::Builder::new()
            .name("Folder watch".into())
            .spawn(move || gather.run(rx, quiet, report))
            .map_err(|e| notify::Error::generic(&e.to_string()))?;
        Ok(FolderWatch { _watcher: watcher, count })
    }

    /// How many folders are being watched.
    pub fn count(&self) -> usize {
        self.count
    }
}

#[derive(Default)]
struct Pending {
    /// By each path's [`case::key`], so one file named in two cases is one change.
    files: BTreeMap<String, String>,
    overflowed: BTreeSet<String>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.files.is_empty() && self.overflowed.is_empty()
    }

    fn take(&mut self) -> Changes {
        let p = std::mem::take(self);
        Changes { files: p.files.into_values().collect(), overflowed: p.overflowed.into_iter().collect() }
    }
}

struct Gather {
    roots: Vec<String>,
    wanted: fn(&str) -> bool,
    changes: Pending,
}

impl Gather {
    fn run(mut self, rx: mpsc::Receiver<notify::Result<Event>>, quiet: Duration, mut report: impl FnMut(Changes)) {
        let mut last = Instant::now();
        loop {
            let wait =
                if self.changes.is_empty() { Duration::from_secs(3600) } else { quiet.saturating_sub(last.elapsed()) };
            match rx.recv_timeout(wait) {
                Ok(event) => {
                    self.take_in(event);
                    last = Instant::now();
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !self.changes.is_empty() && last.elapsed() >= quiet {
                        report(self.changes.take());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn root_of(&self, path: &str) -> Option<String> {
        self.roots.iter().filter(|r| is_inside(path, r)).max_by_key(|r| r.len()).cloned()
    }

    fn file(&mut self, path: &str) {
        if (self.wanted)(path) {
            self.changes.files.entry(case::key(path)).or_insert_with(|| path.to_owned());
        }
    }

    fn overflow(&mut self, path: Option<&str>) {
        match path.and_then(|p| self.root_of(p)) {
            Some(root) => {
                self.changes.overflowed.insert(root);
            }
            // Not told where: every watched folder is walked again.
            None => self.changes.overflowed.extend(self.roots.iter().cloned()),
        }
    }

    fn take_in(&mut self, event: notify::Result<Event>) {
        let Ok(event) = event else {
            // The system lost track (its list of changes ran over): walk what it was watching.
            self.overflow(None);
            return;
        };
        if event.need_rescan() {
            let first = event.paths.first().and_then(|p| p.to_str()).map(str::to_owned);
            self.overflow(first.as_deref());
            return;
        }
        let paths: Vec<String> = event.paths.iter().filter_map(|p| p.to_str().map(str::to_owned)).collect();
        match event.kind {
            EventKind::Create(_) => paths.iter().for_each(|p| self.appeared(p)),
            EventKind::Remove(_) => paths.iter().for_each(|p| self.gone(p)),
            EventKind::Modify(ModifyKind::Name(mode)) => match (mode, paths.as_slice()) {
                (RenameMode::Both, [from, to]) => {
                    self.gone(from);
                    self.appeared(to);
                }
                (RenameMode::From, _) => paths.iter().for_each(|p| self.gone(p)),
                (RenameMode::To, _) => paths.iter().for_each(|p| self.appeared(p)),
                // The system couldn't say which end it is: a path that's there now arrived, one that isn't went.
                _ => paths.iter().for_each(|p| if Path::new(p).exists() { self.appeared(p) } else { self.gone(p) }),
            },
            EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                // A folder's own date changes as files come and go in it; its files report themselves.
                paths.iter().filter(|p| !Path::new(p.as_str()).is_dir()).for_each(|p| self.file(p));
            }
            EventKind::Access(_) => {}
        }
    }

    fn appeared(&mut self, path: &str) {
        if !Path::new(path).is_dir() {
            self.file(path);
            return;
        }
        // A folder moved in from elsewhere arrives whole, as one event: its files aren't named one by one, so list
        // them (a folder made empty reports its files itself as they're added, and lists as nothing here).
        let mut files = Vec::new();
        if list_files(Path::new(path), self.wanted, &mut files).is_err() || files.len() > MOVED_IN_LIMIT {
            self.overflow(Some(path));
        } else {
            files.iter().for_each(|f| self.file(f));
        }
    }

    fn gone(&mut self, path: &str) {
        if (self.wanted)(path) {
            self.file(path);
        } else if looks_like_folder(path) {
            // A folder gone: its files aren't named one by one, and it can't be told from a file any more except by
            // its name.
            self.overflow(Some(path));
        }
    }
}

/// The wanted files under `dir`, stopping once there are more than [`MOVED_IN_LIMIT`].
fn list_files(dir: &Path, wanted: fn(&str) -> bool, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        if out.len() > MOVED_IN_LIMIT {
            return Ok(());
        }
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            list_files(&path, wanted, out)?;
        } else if let Some(p) = path.to_str().filter(|p| wanted(p)) {
            out.push(p.to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::is_project_file;
    use notify::event::{CreateKind, RemoveKind};
    use std::path::PathBuf;

    fn gather(roots: &[&str]) -> Gather {
        Gather {
            roots: roots.iter().map(|r| (*r).to_owned()).collect(),
            wanted: is_project_file,
            changes: Pending::default(),
        }
    }

    fn event(kind: EventKind, paths: &[&str]) -> notify::Result<Event> {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        Ok(e)
    }

    #[test]
    fn gathers_wanted_files_and_overflows() {
        let mut g = gather(&["/m", "/m/inner"]);
        g.take_in(event(EventKind::Create(CreateKind::File), &["/m/Song_2.flp", "/m/kick.wav"]));
        g.take_in(event(EventKind::Modify(ModifyKind::Any), &["/m/SONG_2.flp"]));
        g.take_in(event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), &["/m/a.flp", "/m/b.flp"]));
        g.take_in(event(EventKind::Remove(RemoveKind::Any), &["/m/inner/Old songs"]));
        g.take_in(event(EventKind::Remove(RemoveKind::Any), &["/m/kick.wav"]));
        let c = g.changes.take();
        assert_eq!(c.files, ["/m/a.flp", "/m/b.flp", "/m/Song_2.flp"]);
        assert_eq!(c.overflowed, ["/m/inner"]);
        assert!(g.changes.is_empty());
        g.take_in(Err(notify::Error::generic("too many changes")));
        assert_eq!(g.changes.take().overflowed, ["/m", "/m/inner"]);
    }
}
