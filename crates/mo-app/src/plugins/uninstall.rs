//! Uninstall on the Plugins page (Solomon, 4 Nov 2026: "Yes" to running the plugin's own uninstaller, then sending
//! the leftovers he ticks to the Recycle Bin). As the C# app's Uninstall card:
//!
//! - Windows lists an uninstaller for it: that runs, in its own window, asking its own questions. The app waits for
//!   it and for what it starts (many hand over to a copy of themselves), up to 20 minutes or until you press Stop,
//!   then for the plugin's files to go. Once they've gone, Clean up offers what it left behind (presets, sounds).
//! - One that looks like another version's ("iZotope Ozone 11" for Ozone 9) is never picked for you: you pick it.
//! - None, and it's a Waves plugin: every Waves plugin lives in one WaveShell file, so nothing goes here; Waves Central
//!   removes them.
//! - None otherwise: its plugin files can go to the Recycle Bin instead, each checked again just before
//!   (`mo_space::plugin_files`), with Put back.
//!
//! Nothing is written to library.db: every read of the page leaves out the plugin's files once they've gone.

use crate::space_bin::BIN;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_library::scan::Inside;
use mo_space::plugin_files::{self, FileLine};
use mo_space::plugin_space::{PluginSpace, SpaceKind};
use mo_space::plugins::is_waves;
use mo_space::uninstallers::{Ran, Uninstaller, for_plugin, is_other_version};
use mo_space::{Stopped, size_text};

use super::{Plugins, unused};

/// What Uninstall looks at for one plugin. Made on the window's thread; [`Ask::plan`] runs on a worker.
#[derive(Clone, Debug)]
pub struct Ask {
    pub row_key: (String, String),
    pub name: String,
    pub vendor: String,
    /// How many songs use it (any save), as its card's Used tab counts them, and how many of their newest saves do
    /// (those open with it missing) (review E2-10).
    pub songs: usize,
    pub songs_now: usize,
    /// The missing version whose songs may open with it, as the Install these card says (review E2-6).
    pub stands_in: Option<String>,
    pub space: PluginSpace,
    installed: Vec<String>,
    names: HashMap<String, String>,
}

/// What Uninstall can do for a plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Run one of these uninstallers (best first). `must_pick`: they look like another version's, so none is picked
    /// for you. `files` are the plugin's files, whose going says the uninstaller has finished.
    Run { found: Vec<Uninstaller>, must_pick: bool, files: Vec<String> },
    /// No uninstaller: these plugin files may go to the Recycle Bin instead.
    Files(Vec<FileLine>),
    /// No uninstaller, and it lives in a WaveShell.
    Waves,
    /// No uninstaller, and no plugin files known.
    Nothing,
}

impl Plugins {
    /// Whether the plugin at `row` can be uninstalled here: installed, and measured where it is.
    pub fn can_uninstall(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| r.installed && r.room.is_some())
    }

    /// What Uninstall needs for the plugin at `row`, or `None` ([`Plugins::can_uninstall`]).
    pub fn uninstall_ask(&self, row: usize) -> Option<Ask> {
        if !self.can_uninstall(row) {
            return None;
        }
        let r = self.rows.get(row)?;
        let names = self.plugin_names();
        let installed = self.rows.iter().filter(|r| r.installed).map(|r| r.name.clone()).collect();
        Some(Ask {
            row_key: (r.key.clone(), r.own.clone()),
            name: r.name.clone(),
            vendor: r.vendor.clone(),
            songs: r.songs,
            songs_now: r.songs_now,
            stands_in: r.stands_in.clone(),
            space: r.room.clone()?,
            installed,
            names,
        })
    }

    /// Leaves out these paths (and what's inside them) until the next measure: a plugin's files once its
    /// uninstaller removed them, or what went to the Recycle Bin.
    pub fn forget_paths<S: AsRef<str>>(&mut self, gone: &[S]) {
        let gone = Inside::new(gone);
        if gone.is_empty() {
            return;
        }
        let keep = |path: &str| !gone.holds(path);
        for s in &mut self.spaces {
            s.folders.retain(|f| keep(&f.path));
        }
        let cutoff = self.cutoff();
        for r in &mut self.rows {
            if let Some(room) = r.room.as_mut() {
                room.folders.retain(|f| keep(&f.path));
            }
            r.unused = unused(r, cutoff);
        }
        self.refilter();
    }
}

impl Ask {
    /// What Uninstall can do, given every uninstaller Windows lists (`mo_space::uninstallers::all`). Looks at the
    /// plugin's files when there's no uninstaller: call it away from the window's thread.
    pub fn plan(&self, all: &[Uninstaller], stop: &AtomicBool) -> Result<Plan, Stopped> {
        let found = for_plugin(&self.name, &self.vendor, all, &self.installed);
        let program: Vec<&str> =
            self.space.folders.iter().filter(|f| f.kind == SpaceKind::Program).map(|f| f.path.as_str()).collect();
        if let Some(first) = found.first() {
            let must_pick = is_other_version(&self.name, first, &self.installed);
            return Ok(Plan::Run { found, must_pick, files: program.iter().map(|p| (*p).to_owned()).collect() });
        }
        if program.iter().any(|p| is_waves(&self.name, p)) {
            return Ok(Plan::Waves);
        }
        let lines = plugin_files::look(&self.space, &|k| self.name_of(k), stop)?;
        Ok(if lines.is_empty() { Plan::Nothing } else { Plan::Files(lines) })
    }

    /// Whether this uninstaller looks like another version's than the plugin's ("iZotope Ozone 11" for Ozone 9).
    pub fn is_other(&self, u: &Uninstaller) -> bool {
        is_other_version(&self.name, u, &self.installed)
    }

    /// A plugin's name from its name key (the key itself when it isn't known).
    pub fn name_of(&self, key: &str) -> String {
        self.names.get(key).cloned().unwrap_or_else(|| key.to_owned())
    }

    /// The card's title.
    pub fn title(&self) -> String {
        format!("Uninstall {}?", self.name)
    }

    /// What the card warns of first: songs that use it ("" for none).
    pub fn warning(&self) -> String {
        let mut parts = Vec::new();
        match (self.songs, self.songs_now) {
            (0, _) => {}
            (1, 1) => parts.push(format!("1 song uses it. It'll open with {} missing.", self.name)),
            (n, now) if now == n => parts.push(format!("{n} songs use it. They'll open with {} missing.", self.name)),
            (n, 0) => parts.push(format!("{} used it in older saves; their newest saves don't need it.", songs(n))),
            (n, 1) => {
                parts.push(format!("{} used it; 1 still does, and will open with {} missing.", songs(n), self.name))
            }
            (n, now) => {
                parts.push(format!("{} used it; {now} still do, and will open with {} missing.", songs(n), self.name))
            }
        }
        if let Some(other) = &self.stands_in {
            parts.push(format!("Songs that use {other} may open with it now, and won't once it's gone."));
        }
        parts.join(" ")
    }

    /// What the card says over its lines.
    pub fn intro(&self, plan: &Plan) -> String {
        match plan {
            Plan::Run { must_pick, .. } => {
                let mut s = "This runs its own uninstaller, the same one Windows Settings, Apps uses. It opens in its \
                             own window and may ask questions."
                    .to_owned();
                if *must_pick {
                    s.push_str(&format!(
                        " These look like other versions of {}, not this one. Pick one only if you're sure it's the \
                         right one.",
                        self.name
                    ));
                }
                s
            }
            Plan::Files(_) if cfg!(windows) => {
                "Windows has no uninstaller for it that's still there. Its plugin files \
                                                can go to the {BIN} instead (Windows may ask for administrator \
                                                rights):"
                    .to_owned()
            }
            Plan::Files(_) => "Its plugin files can go to the Bin:".to_owned(),
            Plan::Waves => plugin_files::WAVES.to_owned(),
            Plan::Nothing if cfg!(windows) => format!(
                "Windows has no uninstaller for {} and the last measure found none of its plugin files, so \
                 there's nothing to remove here.",
                self.name
            ),
            Plan::Nothing => {
                format!(
                    "The last measure found none of {}'s plugin files, so there's nothing to remove here.",
                    self.name
                )
            }
        }
    }
}

/// The plugin files ticked that may go.
pub fn ticked_files<'a>(lines: &'a [FileLine], ticks: &'a [bool]) -> impl Iterator<Item = &'a FileLine> {
    lines.iter().zip(ticks).filter(|(l, t)| **t && l.stays.is_none()).map(|(l, _)| l)
}

/// "Frees 40 MB", or what to do when nothing is ticked.
pub fn files_freed_text(lines: &[FileLine], ticks: &[bool]) -> String {
    let (n, bytes) = ticked_files(lines, ticks).fold((0usize, 0u64), |(n, b), l| (n + 1, b.saturating_add(l.bytes)));
    if n > 0 {
        format!("Frees {}", size_text(bytes))
    } else if lines.iter().any(|l| l.stays.is_none()) {
        format!("Tick what to send to the {BIN}.")
    } else {
        "Nothing here can go: each line says why.".to_owned()
    }
}

/// What a plugin file is, for its line: "VST3 plugin file".
pub fn file_kind(line: &FileLine) -> String {
    if line.format.is_empty() { "File".to_owned() } else { format!("{} plugin file", line.format) }
}

/// One uninstaller as the card lists it: its name, and its maker and size when Windows says them.
pub fn uninstaller_text(u: &Uninstaller) -> (String, String) {
    let mut detail = u.publisher.clone();
    if u.bytes > 0 {
        if !detail.is_empty() {
            detail.push_str(" · ");
        }
        detail.push_str(&size_text(u.bytes));
    }
    (u.name.clone(), detail)
}

/// What an uninstaller line says when it looks like another version's.
pub const OTHER_VERSION: &str = "This looks like another version's uninstaller: pick it only if you're sure.";

/// How long Uninstall waits for an uninstaller (and what it starts) before it stops waiting, and how long after it
/// ends for the plugin's files to go.
pub const WAIT: Duration = Duration::from_secs(20 * 60);
pub const GRACE: Duration = Duration::from_secs(10);
pub const POLL: Duration = Duration::from_millis(500);

/// How it went, from the plugin's files after its uninstaller ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waited {
    /// The plugin's files went (or, with none to tell by, the uninstaller said it finished).
    Gone,
    /// They're all still there: it's still installed.
    StillThere,
    /// You stopped waiting, or the wait ran out: the uninstaller may still be open.
    Stopped,
    /// It ended without saying it finished, and there are no files to tell by.
    Unknown,
}

/// The plugin's files that are there now, before its uninstaller runs: the ones whose going says it worked (a file
/// already gone says nothing).
pub fn there_now(files: &[String]) -> Vec<String> {
    files.iter().filter(|f| std::fs::symlink_metadata(f).is_ok()).cloned().collect()
}

/// How it went, once the uninstaller ended (`ran`): `watched` are the plugin's files that were there before it ran.
/// Its own copy may still be finishing, so the files get up to `grace` to go. Call it away from the window's thread.
pub fn after_run(ran: &Ran, watched: &[String], grace: Duration, poll: Duration, stop: &AtomicBool) -> Waited {
    match ran {
        Ran::NothingChanged(_) => return Waited::StillThere,
        Ran::StoppedWaiting => return Waited::Stopped,
        Ran::Finished | Ran::Unclear(_) => {}
    }
    if watched.is_empty() {
        return if *ran == Ran::Finished { Waited::Gone } else { Waited::Unknown };
    }
    let all_left = || watched.iter().all(|f| std::fs::symlink_metadata(f).is_ok());
    let until = Instant::now() + grace;
    while all_left() {
        if stop.load(Ordering::Relaxed) {
            return Waited::Stopped;
        }
        if Instant::now() > until {
            return Waited::StillThere;
        }
        sleep_unless_stopped(poll, stop);
    }
    Waited::Gone
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let until = Instant::now() + d;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50).min(d));
    }
}

/// What the card says once the uninstaller is done.
pub fn ran_text(name: &str, ran: &Ran, waited: Waited) -> String {
    match waited {
        Waited::Gone => format!(
            "{name} is uninstalled, and its files are off the page. It leaves the list of installed plugins, and \
             songs that use it show it missing, at the next measure (Measure again)."
        ),
        Waited::Stopped => format!(
            "Stopped waiting for {name}'s uninstaller. It may still be open in its own window; once it's done, \
             Measure again updates the list."
        ),
        Waited::Unknown => format!(
            "{name}'s uninstaller ended without saying it finished ({}), and there are no plugin files to tell by. \
             Measure again shows whether it's still installed.",
            ran.text().trim_end_matches('.')
        ),
        Waited::StillThere => match ran {
            Ran::NothingChanged(why) => format!("{name} is still installed: {why}"),
            Ran::Unclear(why) => format!("{name} is still installed: {why} Its files are still there."),
            _ => format!("{name} is still installed: its files are still there, so nothing on the page changed."),
        },
    }
}

/// Which of the plugin's files have gone now.
pub fn gone_now(files: &[String]) -> Vec<String> {
    files.iter().filter(|f| std::fs::symlink_metadata(f).is_err()).cloned().collect()
}

/// "1 song", "3 songs".
fn songs(n: usize) -> String {
    if n == 1 { "1 song".to_owned() } else { format!("{n} songs") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_it_went_comes_from_the_files_that_were_there() {
        let stop = AtomicBool::new(false);
        let dir = std::env::temp_dir().join("mo-app-uninstall-wait");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Arcade.vst3");
        std::fs::write(&file, b"x").unwrap();
        let gone_before = dir.join("Arcade.dll").to_string_lossy().into_owned();
        let files = vec![file.to_string_lossy().into_owned(), gone_before];
        // A file already gone before it ran says nothing.
        let watched = there_now(&files);
        assert_eq!(watched, files[..1]);
        let tick = Duration::from_millis(5);
        let short = Duration::from_millis(30);
        assert_eq!(after_run(&Ran::Finished, &watched, short, tick, &stop), Waited::StillThere);
        let said_no = Ran::NothingChanged("You said no.".into());
        assert_eq!(after_run(&said_no, &watched, short, tick, &stop), Waited::StillThere);
        assert_eq!(after_run(&Ran::StoppedWaiting, &watched, short, tick, &stop), Waited::Stopped);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(after_run(&Ran::Unclear("Code 2.".into()), &watched, short, tick, &stop), Waited::Gone);
        assert_eq!(gone_now(&files), files);
        // No files to tell by: only its own word.
        assert_eq!(after_run(&Ran::Finished, &[], short, tick, &stop), Waited::Gone);
        assert_eq!(after_run(&Ran::Unclear("Code 2.".into()), &[], short, tick, &stop), Waited::Unknown);
        std::fs::write(&file, b"x").unwrap();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(after_run(&Ran::Finished, &watched, WAIT, POLL, &stop), Waited::Stopped);
    }

    #[test]
    fn what_the_card_says_once_it_ran() {
        assert!(ran_text("Arcade", &Ran::Finished, Waited::Gone).starts_with("Arcade is uninstalled,"));
        let cancelled = Ran::NothingChanged("It was cancelled.".into());
        assert!(ran_text("Arcade", &cancelled, Waited::StillThere).ends_with("It was cancelled."));
        assert!(ran_text("Arcade", &Ran::Finished, Waited::StillThere).contains("still there"));
        assert!(ran_text("Arcade", &Ran::Unclear("Code 2.".into()), Waited::Unknown).contains("(Code 2)"));
        let u = Uninstaller { name: "Arcade".into(), publisher: "Output".into(), command: "x".into(), bytes: 2048 };
        assert_eq!(uninstaller_text(&u), ("Arcade".into(), format!("Output · {}", size_text(2048))));
    }
}
