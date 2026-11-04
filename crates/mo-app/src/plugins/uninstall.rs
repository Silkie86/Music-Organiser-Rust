//! Uninstall on the Plugins page (Solomon, 4 Nov 2026: "Yes" to running the plugin's own uninstaller, then sending
//! the leftovers he ticks to the Recycle Bin). As the C# app's Uninstall card:
//!
//! - Windows lists an uninstaller for it: that runs, in its own window, asking its own questions. Many hand over to a
//!   copy of themselves and close at once, so the app waits for the plugin's files to go (up to 20 minutes), not for
//!   the program to close. Once they've gone, Clean up offers what it left behind (presets, sounds).
//! - One that looks like another version's ("iZotope Ozone 11" for Ozone 9) is never picked for you: you pick it.
//! - None, and it's a Waves plugin: every Waves plugin lives in one WaveShell file, so nothing goes here; Waves Central
//!   removes them.
//! - None otherwise: its plugin files can go to the Recycle Bin instead, each checked again just before
//!   (`mo_space::plugin_files`), with Put back.
//!
//! Nothing is written to library.db: the page leaves out the plugin's files once they've gone, until the old app
//! looks for plugins again.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_library::scan::is_inside;
use mo_space::plugin_files::{self, FileLine};
use mo_space::plugin_space::{PluginSpace, SpaceKind};
use mo_space::plugins::is_waves;
use mo_space::uninstallers::{Uninstaller, for_plugin, is_other_version};
use mo_space::{Stopped, size_text};

use super::{Plugins, unused};

/// What Uninstall looks at for one plugin. Made on the window's thread; [`Ask::plan`] runs on a worker.
#[derive(Clone, Debug)]
pub struct Ask {
    pub row_key: (String, String),
    pub name: String,
    pub vendor: String,
    /// How many saves use it.
    pub projects: usize,
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
    /// Whether the plugin at `row` can be uninstalled here: installed, and the old app measured where it is.
    pub fn can_uninstall(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| r.installed && r.room.is_some())
    }

    /// What Uninstall needs for the plugin at `row`, or `None` ([`Plugins::can_uninstall`]).
    pub fn uninstall_ask(&self, row: usize) -> Option<Ask> {
        if !self.can_uninstall(row) {
            return None;
        }
        let r = self.rows.get(row)?;
        let mut names: HashMap<String, String> = self.spaces.iter().map(|s| (s.key.clone(), s.name.clone())).collect();
        for r in &self.rows {
            if let Some(room) = &r.room {
                names.insert(room.key.clone(), r.name.clone());
            }
        }
        let installed = self.rows.iter().filter(|r| r.installed).map(|r| r.name.clone()).collect();
        Some(Ask {
            row_key: (r.key.clone(), r.own.clone()),
            name: r.name.clone(),
            vendor: r.vendor.clone(),
            projects: r.saves.len(),
            space: r.room.clone()?,
            installed,
            names,
        })
    }

    /// Leaves out these paths (and what's inside them) until the old app measures again: a plugin's files once its
    /// uninstaller removed them, or what went to the Recycle Bin.
    pub fn forget_paths(&mut self, gone: &[String]) {
        if gone.is_empty() {
            return;
        }
        let keep = |path: &str| !gone.iter().any(|g| is_inside(path, g));
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

    /// A plugin's name from its name key (the key itself when it isn't known).
    pub fn name_of(&self, key: &str) -> String {
        self.names.get(key).cloned().unwrap_or_else(|| key.to_owned())
    }

    /// The card's title.
    pub fn title(&self) -> String {
        format!("Uninstall {}?", self.name)
    }

    /// What the card warns of first: projects that use it ("" for none).
    pub fn warning(&self) -> String {
        match self.projects {
            0 => String::new(),
            1 => format!("1 project uses it. It'll open with {} missing.", self.name),
            n => format!("{n} projects use it. They'll open with {} missing.", self.name),
        }
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
                                                can go to the Recycle Bin instead (Windows may ask for administrator \
                                                rights):"
                    .to_owned()
            }
            Plan::Files(_) => "Its plugin files can go to the Bin:".to_owned(),
            Plan::Waves => plugin_files::WAVES.to_owned(),
            Plan::Nothing => format!(
                "Windows has no uninstaller for {} and the old app found none of its plugin files, so there's \
                 nothing to remove here.",
                self.name
            ),
        }
    }
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

/// The plugin files ticked (only those that may go).
pub fn picked_files(lines: &[FileLine], ticks: &[bool]) -> Vec<FileLine> {
    lines.iter().zip(ticks).filter(|(l, t)| **t && l.stays.is_none()).map(|(l, _)| l.clone()).collect()
}

/// "Frees 40 MB", or what to do when nothing is ticked.
pub fn files_freed_text(lines: &[FileLine], ticks: &[bool]) -> String {
    let picked = picked_files(lines, ticks);
    if picked.is_empty() {
        if lines.iter().any(|l| l.stays.is_none()) {
            "Tick what to send to the Recycle Bin.".to_owned()
        } else {
            "Nothing here can go: each line says why.".to_owned()
        }
    } else {
        format!("Frees {}", size_text(picked.iter().fold(0u64, |a, l| a.saturating_add(l.bytes))))
    }
}

/// What a plugin file is, for its line: "VST3 plugin file".
pub fn file_kind(line: &FileLine) -> String {
    if line.format.is_empty() { "File".to_owned() } else { format!("{} plugin file", line.format) }
}

/// How waiting for an uninstaller went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waited {
    /// The plugin's files went (or, with none to watch, the uninstaller closed).
    Gone,
    /// They're all still there after the wait: it's still installed.
    StillThere,
    /// You stopped waiting.
    Stopped,
}

/// How long Uninstall waits for a plugin's files to go, and how often it looks.
pub const WAIT: Duration = Duration::from_secs(20 * 60);
pub const POLL: Duration = Duration::from_secs(2);

/// Waits until any of `files` has gone, for up to `wait`, looking every `poll` (then a moment more, so the
/// uninstaller can finish before the list is read). With no files to watch it's over at once: the uninstaller closing
/// is all there is to go by. Call it away from the window's thread.
pub fn wait_for_files(files: &[String], wait: Duration, poll: Duration, stop: &AtomicBool) -> Waited {
    if files.is_empty() {
        return Waited::Gone;
    }
    let all_left = || files.iter().all(|f| Path::new(f).exists());
    let until = Instant::now() + wait;
    while all_left() {
        if stop.load(Ordering::Relaxed) {
            return Waited::Stopped;
        }
        if Instant::now() > until {
            return Waited::StillThere;
        }
        sleep_unless_stopped(poll, stop);
    }
    sleep_unless_stopped(poll * 2, stop);
    Waited::Gone
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let until = Instant::now() + d;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100).min(d));
    }
}

/// What the card says once the uninstaller is done: what happened, and whether Clean up may be offered next.
pub fn ran_text(name: &str, ran: &Result<(), String>, waited: Waited) -> String {
    match (ran, waited) {
        (_, Waited::Gone) => format!(
            "{name} is uninstalled. The page still lists it as installed until the old app looks for plugins again."
        ),
        (_, Waited::Stopped) => {
            format!("Stopped waiting for {name}'s uninstaller. Once it's done, the old app's Rescan updates the list.")
        }
        (Err(why), Waited::StillThere) => format!("{name} is still installed: {why}"),
        (Ok(()), Waited::StillThere) => {
            format!("{name} is still installed: its files are still there, so nothing on the page changed.")
        }
    }
}

/// Whether to wait for the plugin's files after the uninstaller ended this way: not when it couldn't start, you said
/// no to Windows, or you cancelled it (`mo_space::uninstallers::run` says the plugin list is checked again when it
/// ended some other way).
pub fn worth_waiting(ran: &Result<(), String>) -> bool {
    match ran {
        Ok(()) => true,
        Err(why) => why.contains("checked again anyway"),
    }
}

/// The paths of a plan's plugin files that went, from what's on disk now.
pub fn gone_now(files: &[String]) -> Vec<String> {
    files.iter().filter(|f| !Path::new(f).exists()).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_ends_when_a_file_goes_or_the_wait_runs_out() {
        let stop = AtomicBool::new(false);
        let dir = std::env::temp_dir().join("mo-app-uninstall-wait");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Arcade.vst3");
        std::fs::write(&file, b"x").unwrap();
        let files = vec![file.to_string_lossy().into_owned()];
        let tick = Duration::from_millis(5);
        assert_eq!(wait_for_files(&files, Duration::from_millis(30), tick, &stop), Waited::StillThere);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(wait_for_files(&files, Duration::from_millis(30), tick, &stop), Waited::Gone);
        assert_eq!(gone_now(&files), files);
        assert_eq!(wait_for_files(&[], WAIT, POLL, &stop), Waited::Gone);
        std::fs::write(&file, b"x").unwrap();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(wait_for_files(&files, WAIT, POLL, &stop), Waited::Stopped);
    }

    #[test]
    fn what_the_card_says_once_it_ran() {
        assert!(ran_text("Arcade", &Ok(()), Waited::Gone).starts_with("Arcade is uninstalled."));
        assert!(
            ran_text("Arcade", &Err("It was cancelled.".into()), Waited::StillThere).ends_with("It was cancelled.")
        );
        assert!(ran_text("Arcade", &Ok(()), Waited::StillThere).contains("still there"));
        assert!(
            worth_waiting(&Ok(()))
                && !worth_waiting(&Err("It was cancelled, so the plugin is still installed.".into()))
        );
        assert!(worth_waiting(&Err(
            "It ended with code 2, so it may not have finished. The plugin list is checked again \
                                    anyway."
                .into()
        )));
        let u = Uninstaller { name: "Arcade".into(), publisher: "Output".into(), command: "x".into(), bytes: 2048 };
        assert_eq!(uninstaller_text(&u), ("Arcade".into(), format!("Output · {}", size_text(2048))));
    }
}
