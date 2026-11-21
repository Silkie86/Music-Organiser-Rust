//! Uninstall on the Plugins page (Solomon, 4 Nov 2026: "Yes" to running the plugin's own uninstaller, then sending
//! the leftovers he ticks to the Recycle Bin). As the C# app's Uninstall card:
//!
//! - Windows lists an uninstaller for it: that runs, in its own window, asking its own questions. The app waits for
//!   it and for what it starts (many hand over to a copy of themselves), up to 20 minutes or until you press Stop,
//!   then for the plugin's files to go. Once they've all gone, Clean up offers what it left behind (presets, sounds);
//!   the files it left, DAWs may still load, so they're offered for the Recycle Bin instead.
//! - One that looks like another version's ("iZotope Ozone 11" for Ozone 9), or another plugin's whose name holds this
//!   one's ("Massive X" for Massive), is never picked for you: you pick it.
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

    /// Whether this uninstaller looks like another version's than the plugin's ("iZotope Ozone 11" for Ozone 9), or
    /// another plugin's ("Massive X" for Massive).
    pub fn is_other(&self, u: &Uninstaller) -> bool {
        is_other_version(&self.name, u, &self.installed)
    }

    /// The plugin files its uninstaller left (`left`), looked at as [`Ask::plan`] looks at them when there's no
    /// uninstaller, for the Recycle Bin: DAWs may still load them (the whole-app review of 19 Nov, PM-1). Call it away
    /// from the window's thread.
    pub fn left_lines(&self, left: &[String], stop: &AtomicBool) -> Result<Vec<FileLine>, Stopped> {
        let left = Inside::new(left);
        let mut space = self.space.clone();
        space.folders.retain(|f| f.kind == SpaceKind::Program && left.holds(&f.path));
        plugin_files::look(&space, &|k| self.name_of(k), stop)
    }

    /// What the card says over the plugin files its uninstaller left ([`Ask::left_lines`]); `maybe_running`: it handed
    /// over to a copy of itself the app can't watch, which may still be running ([`Waited::Partly`]).
    pub fn left_intro(&self, maybe_running: bool) -> String {
        let admin = if cfg!(windows) { " (Windows may ask for administrator rights)" } else { "" };
        if maybe_running {
            return format!(
                "{}'s uninstaller handed over to a copy of itself that the app can't watch, which may still be running \
                 in its own window, and these plugin files of it are still there. Once it has finished, they can go to \
                 the {BIN} too{admin}:",
                self.name
            );
        }
        format!(
            "{}'s uninstaller ended, but these plugin files of it are still there, so DAWs may still load it. They can \
             go to the {BIN} too{admin}:",
            self.name
        )
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
                        " These look like the uninstallers of other versions of {}, or of other plugins with names \
                         like it, not of this one. Pick one only if you're sure it's the right one.",
                        self.name
                    ));
                }
                s
            }
            // Each with format!, so the bin is named (review AP-13: a plain string showed "{BIN}" as it's written).
            Plan::Files(_) if cfg!(windows) => format!(
                "Windows has no uninstaller for it that's still there. Its plugin files can go to the {BIN} instead \
                 (Windows may ask for administrator rights):"
            ),
            Plan::Files(_) => format!("Its plugin files can go to the {BIN}:"),
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

/// What an uninstaller line says when it looks like another version's, or another plugin's.
pub const OTHER_VERSION: &str = "This looks like the uninstaller of another version, or of another plugin with a \
                                 name like it: pick it only if you're sure.";

/// How long Uninstall waits for an uninstaller (and what it starts) before it stops waiting, and how long the plugin's
/// files get to go once it ended (from when it ended, or from when the last of them went).
pub const WAIT: Duration = Duration::from_secs(20 * 60);
pub const GRACE: Duration = Duration::from_secs(10);
pub const POLL: Duration = Duration::from_millis(500);

/// How long the plugin's files get after each one goes, when their uninstaller may have handed over to a copy of itself
/// ([`Watch::handed_over`]): the app can't see that copy end, and it may remove other things for a while (a big sound
/// library) before the rest of them (the whole-app review of 19 Nov, PM-1 and PM-5).
pub const HANDED_OVER_GRACE: Duration = Duration::from_secs(2 * 60);

/// How soon an uninstaller may end, saying it finished, and still be one that handed over to a copy of itself and
/// ended at once (NSIS's Uninstall.exe always does: it copies itself to Temp, starts the copy and ends). Windows
/// PowerShell's wait follows the copy only when it can, and it can't for one that asks for administrator rights, so the
/// plugin's files are watched for the copy instead (the whole-app review of 19 Nov, PM-5). Time for Windows'
/// administrator question too.
pub const QUICK: Duration = Duration::from_secs(60);

/// How [`after_run`] watches the plugin's files once their uninstaller ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    /// How long they get to go, from when it ended or the last of them went (an uninstaller may remove them one by
    /// one).
    pub grace: Duration,
    /// How often they're looked at.
    pub poll: Duration,
    /// For one that ended soon after it started, saying it finished ([`QUICK`]): up to when they're watched while
    /// every one is still there (the wait's end), as the copy it handed over to may still be open.
    pub handed_over: Option<Instant>,
    /// How long they get after each one goes when it may have handed over ([`HANDED_OVER_GRACE`]), up to the wait's
    /// end.
    pub handed_over_grace: Duration,
}

impl Watch {
    /// The watch for an uninstaller that ended as `ran` says, `took` after it started; `until`: when the wait for it
    /// ends ([`WAIT`] after it started).
    pub fn after(ran: &Ran, took: Duration, until: Instant) -> Watch {
        let handed_over = (*ran == Ran::Finished && took < QUICK).then_some(until);
        Watch { grace: GRACE, poll: POLL, handed_over, handed_over_grace: HANDED_OVER_GRACE }
    }
}

/// How it went, from the plugin's files after its uninstaller ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waited {
    /// The plugin's files went (or, with none to tell by, the uninstaller said it finished).
    Gone,
    /// Some went, and `left` of them are still there: DAWs may still load it from those (the whole-app review of 19
    /// Nov, PM-1: it was called uninstalled once any one had gone). `maybe_running`: it may have handed over to a copy
    /// of itself, which may still be running ([`Watch::handed_over`]).
    Partly { left: usize, maybe_running: bool },
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
/// Its own copy may still be finishing, so the files get `watch.grace` to go, and that again after each one that goes:
/// it's uninstalled only once every one has gone. One that ended soon after it started may have handed over to a copy
/// of itself: with every file still there they're watched until `watch.handed_over` then ([`Watch::after`]), and the
/// wait stops there as a wait for the uninstaller does; once one goes, the rest get `watch.handed_over_grace` after
/// each, up to then, as that copy can't be seen to end. Call it away from the window's thread.
pub fn after_run(ran: &Ran, watched: &[String], watch: Watch, stop: &AtomicBool) -> Waited {
    match ran {
        Ran::NothingChanged(_) => return Waited::StillThere,
        Ran::StoppedWaiting => return Waited::Stopped,
        Ran::Finished | Ran::Unclear(_) => {}
    }
    if watched.is_empty() {
        return if *ran == Ran::Finished { Waited::Gone } else { Waited::Unknown };
    }
    let after_change = |now: Instant| match watch.handed_over {
        Some(until) => (now + watch.handed_over_grace).min(until),
        None => now + watch.grace,
    };
    let (mut left, mut quiet_until) = (watched.len(), Instant::now() + watch.grace);
    loop {
        let now_left = there_now(watched).len();
        if now_left == 0 {
            return Waited::Gone;
        }
        if stop.load(Ordering::Relaxed) {
            return Waited::Stopped;
        }
        let now = Instant::now();
        if now_left != left {
            (left, quiet_until) = (now_left, after_change(now));
        }
        if now > quiet_until {
            if left < watched.len() {
                return Waited::Partly { left, maybe_running: watch.handed_over.is_some() };
            }
            match watch.handed_over {
                Some(until) if now < until => {}
                Some(_) => return Waited::Stopped,
                None => return Waited::StillThere,
            }
        }
        sleep_unless_stopped(watch.poll, stop);
    }
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let until = Instant::now() + d;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50).min(d));
    }
}

/// What the card says once the uninstaller is done; `left`: the plugin files it left, as the card offers them
/// ([`Ask::left_lines`]), or `None` when they couldn't be looked at.
pub fn ran_text(name: &str, ran: &Ran, waited: Waited, left: Option<&[FileLine]>) -> String {
    match waited {
        Waited::Gone => format!(
            "{name} is uninstalled, and its files are off the page. It leaves the list of installed plugins, and \
             songs that use it show it missing, at the next measure (Measure again)."
        ),
        Waited::Partly { left: count, maybe_running } => {
            let (are, them) = if count == 1 { ("is", "it") } else { ("are", "them") };
            let head = format!(
                "{name}'s uninstaller ended, but {count} of its plugin files {are} still there, so DAWs may still load \
                 it."
            );
            let running = if maybe_running {
                " It handed over to a copy of itself that the app can't watch, which may still be running in its own \
                 window."
            } else {
                ""
            };
            // What the card does next (the whole-app review of 19 Nov, PM-1): offer the ones that can go, or say why
            // none can.
            let next = match left {
                Some(lines) if lines.iter().any(|l| l.stays.is_none()) => {
                    let first = if maybe_running { " Once it has finished, its" } else { " Its" };
                    format!("{first} card on the Plugins page offers {them} for the {BIN}.")
                }
                Some(lines) if !lines.is_empty() => {
                    let why: Vec<String> = lines
                        .iter()
                        .take(3)
                        .map(|l| {
                            let why = l.stays.as_deref().unwrap_or_default();
                            format!("{}: {why}", mo_formats::paths::file_name(&l.path))
                        })
                        .collect();
                    let more = match lines.len().saturating_sub(3) {
                        0 => String::new(),
                        n => format!(" And {n} more."),
                    };
                    format!(" None of them can go from here. {}{more}", why.join(" "))
                }
                _ => " Measure again shows what's left.".to_owned(),
            };
            format!("{head}{running}{next}")
        }
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
        let short = Watch {
            grace: Duration::from_millis(30),
            poll: Duration::from_millis(5),
            handed_over: None,
            handed_over_grace: Duration::ZERO,
        };
        assert_eq!(after_run(&Ran::Finished, &watched, short, &stop), Waited::StillThere);
        let said_no = Ran::NothingChanged("You said no.".into());
        assert_eq!(after_run(&said_no, &watched, short, &stop), Waited::StillThere);
        assert_eq!(after_run(&Ran::StoppedWaiting, &watched, short, &stop), Waited::Stopped);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(after_run(&Ran::Unclear("Code 2.".into()), &watched, short, &stop), Waited::Gone);
        assert_eq!(gone_now(&files), files);
        // No files to tell by: only its own word.
        assert_eq!(after_run(&Ran::Finished, &[], short, &stop), Waited::Gone);
        assert_eq!(after_run(&Ran::Unclear("Code 2.".into()), &[], short, &stop), Waited::Unknown);
        std::fs::write(&file, b"x").unwrap();
        stop.store(true, Ordering::Relaxed);
        let long =
            Watch { handed_over: Some(Instant::now() + WAIT), ..Watch::after(&Ran::Finished, WAIT, Instant::now()) };
        assert_eq!(after_run(&Ran::Finished, &watched, long, &stop), Waited::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Files made for a wait's test, in a folder of their own ([`tidy`] removes it).
    fn files_in(dir: &str, names: &[&str]) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!("{dir}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        names
            .iter()
            .map(|n| {
                std::fs::write(dir.join(n), b"x").unwrap();
                dir.join(n).to_string_lossy().into_owned()
            })
            .collect()
    }

    /// Removes the folder [`files_in`] made.
    fn tidy(files: &[String]) {
        if let Some(dir) = files.first().and_then(|f| std::path::Path::new(f).parent()) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// The whole-app review of 19 Nov, PM-1: it's uninstalled only once every one of its files has gone. A file its
    /// uninstaller left (a VST2 it never knew of) means it isn't, and one that removes them one by one is waited for.
    #[test]
    fn it_is_uninstalled_only_once_every_file_has_gone() {
        let stop = AtomicBool::new(false);
        let files = files_in("mo-app-uninstall-partly", &["Arcade.vst3", "Arcade.dll", "Arcade.clap"]);
        let watch = Watch {
            grace: Duration::from_millis(300),
            poll: Duration::from_millis(5),
            handed_over: None,
            handed_over_grace: Duration::ZERO,
        };
        std::fs::remove_file(&files[0]).unwrap();
        let partly = |left| Waited::Partly { left, maybe_running: false };
        assert_eq!(after_run(&Ran::Finished, &files, watch, &stop), partly(2));
        // One by one, each well within the grace after the one before: it waits for the last.
        let later = files.clone();
        let remover = std::thread::spawn(move || {
            for f in &later[1..] {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::remove_file(f).unwrap();
            }
        });
        assert_eq!(after_run(&Ran::Finished, &files, watch, &stop), Waited::Gone);
        remover.join().unwrap();
        tidy(&files);
    }

    /// The whole-app review of 19 Nov, PM-1: what the card says of the files an uninstaller left follows what it does
    /// next. It offers the ones that can go, says why when none can, and says "Measure again" when they couldn't be
    /// looked at.
    #[test]
    fn what_the_card_says_of_the_files_left_follows_what_it_offers() {
        // Arcade's VST3, which can go, and a VST2 another plugin uses too, which stays.
        let files = files_in("mo-app-uninstall-left-text", &["Arcade.vst3", "Arcade.dll"]);
        let program = |path: &str, shared_with: Vec<String>| mo_space::plugin_space::SpaceFolder {
            path: path.into(),
            kind: SpaceKind::Program,
            bytes: 1,
            shared_with,
            added: false,
        };
        let space = PluginSpace {
            key: "arcade".into(),
            name: "Arcade".into(),
            vendor: "Output".into(),
            folders: vec![program(&files[0], Vec::new()), program(&files[1], vec!["other".into()])],
        };
        let goes = plugin_files::look(&space, &|_| "Other".to_owned(), &AtomicBool::new(false)).unwrap();
        assert_eq!(goes.iter().map(|l| l.stays.is_none()).collect::<Vec<_>>(), [true, false], "{goes:?}");
        let partly = |left| Waited::Partly { left, maybe_running: false };
        let text = ran_text("Arcade", &Ran::Finished, partly(2), Some(&goes));
        assert!(
            text.contains("2 of its plugin files are still there")
                && text.ends_with(&format!("offers them for the {BIN}.")),
            "{text}"
        );
        let one = ran_text("Arcade", &Ran::Unclear("Code 2.".into()), partly(1), Some(&goes[..1]));
        assert!(one.contains("1 of its plugin files is still there") && one.contains("offers it"), "{one}");
        // None can go: each named, with why.
        let none = ran_text("Arcade", &Ran::Finished, partly(1), Some(&goes[1..]));
        assert!(
            !none.contains(BIN) && none.contains("None of them can go from here. Arcade.dll: Other uses this file too"),
            "{none}"
        );
        // Not looked at: no offer.
        let unknown = ran_text("Arcade", &Ran::Finished, partly(1), None);
        assert!(!unknown.contains(BIN) && unknown.ends_with("Measure again shows what's left."), "{unknown}");
        // It may still be running: said, and the offer waits for it.
        let running = ran_text("Arcade", &Ran::Finished, Waited::Partly { left: 1, maybe_running: true }, Some(&goes));
        assert!(
            running.contains("may still be running") && running.contains("Once it has finished, its card"),
            "{running}"
        );
        tidy(&files);
    }

    /// The whole-app review of 19 Nov, PM-5: an uninstaller that ended soon after it started, saying it finished, may
    /// have handed over to a copy of itself that Windows PowerShell can't wait for (NSIS's Uninstall.exe, asking for
    /// administrator rights): its files are watched until the wait's end, not just for the grace, and Stop still works.
    #[test]
    fn an_uninstaller_that_handed_over_to_a_copy_of_itself_is_watched_for() {
        let until = Instant::now() + WAIT;
        assert_eq!(Watch::after(&Ran::Finished, Duration::from_secs(2), until).handed_over, Some(until));
        assert_eq!(Watch::after(&Ran::Finished, QUICK, until).handed_over, None);
        let failed = Ran::Unclear("Code 2.".into());
        assert_eq!(Watch::after(&failed, Duration::from_secs(2), until).handed_over, None);
        let stop = AtomicBool::new(false);
        let files = files_in("mo-app-uninstall-handed-over", &["Arcade.vst3", "Arcade.clap"]);
        let (grace, poll) = (Duration::from_millis(30), Duration::from_millis(5));
        let handed_over_grace = Duration::from_millis(400);
        let watch =
            |until: Duration| Watch { grace, poll, handed_over: Some(Instant::now() + until), handed_over_grace };
        // Its copy removes the files after the grace has passed.
        let later = files.clone();
        let copy = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            for f in &later {
                std::fs::remove_file(f).unwrap();
            }
        });
        assert_eq!(after_run(&Ran::Finished, &files, watch(Duration::from_secs(30)), &stop), Waited::Gone);
        copy.join().unwrap();
        // Once one goes, the rest get the longer grace after each (its copy can't be seen to end): a file that goes
        // well after the grace, but within that, is waited for.
        std::fs::write(&files[0], b"x").unwrap();
        std::fs::write(&files[1], b"x").unwrap();
        let later = files.clone();
        let copy = std::thread::spawn(move || {
            std::fs::remove_file(&later[0]).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            std::fs::remove_file(&later[1]).unwrap();
        });
        assert_eq!(after_run(&Ran::Finished, &files, watch(Duration::from_secs(30)), &stop), Waited::Gone);
        copy.join().unwrap();
        // One that stays past it: some went, and its copy may still be running.
        std::fs::write(&files[1], b"x").unwrap();
        let partly = Waited::Partly { left: 1, maybe_running: true };
        assert_eq!(after_run(&Ran::Finished, &files, watch(Duration::from_secs(30)), &stop), partly);
        // Nor past the wait's end.
        let started = Instant::now();
        assert_eq!(after_run(&Ran::Finished, &files, watch(Duration::from_millis(100)), &stop), partly);
        assert!(started.elapsed() < handed_over_grace, "{:?}", started.elapsed());
        // Nothing went by the wait's end: its copy may still be open, as when a wait for an uninstaller runs out.
        std::fs::write(&files[0], b"x").unwrap();
        assert_eq!(after_run(&Ran::Finished, &files, watch(Duration::from_millis(100)), &stop), Waited::Stopped);
        assert!(ran_text("Arcade", &Ran::Finished, Waited::Stopped, None).contains("may still be open"));
        // Stop works meanwhile.
        stop.store(true, Ordering::Relaxed);
        assert_eq!(after_run(&Ran::Finished, &files, watch(WAIT), &stop), Waited::Stopped);
        tidy(&files);
    }

    /// Review AP-13: the card's first words name the bin, whatever the plan; nothing in braces is left in them.
    #[test]
    fn the_card_s_words_name_the_bin() {
        let ask = Ask {
            row_key: ("arcade".into(), String::new()),
            name: "Arcade".into(),
            vendor: "Output".into(),
            songs: 0,
            songs_now: 0,
            stands_in: None,
            space: PluginSpace {
                key: "arcade".into(),
                name: "Arcade".into(),
                vendor: "Output".into(),
                folders: Vec::new(),
            },
            installed: Vec::new(),
            names: HashMap::new(),
        };
        let u = Uninstaller { name: "Arcade".into(), publisher: "Output".into(), command: "x".into(), bytes: 2048 };
        let plans = [
            Plan::Run { found: vec![u], must_pick: true, files: Vec::new() },
            Plan::Files(Vec::new()),
            Plan::Waves,
            Plan::Nothing,
        ];
        for plan in &plans {
            let intro = ask.intro(plan);
            assert!(!intro.contains('{') && !intro.contains('}'), "{plan:?}: {intro}");
        }
        assert!(ask.intro(&Plan::Files(Vec::new())).contains(BIN));
        assert!(ask.left_intro(false).contains(BIN));
        assert!(ask.left_intro(true).contains("may still be running") && ask.left_intro(true).contains(BIN));
        // Another version's, or another plugin's, says so.
        assert!(ask.intro(&plans[0]).contains("other plugins with names like it"));
    }

    /// The whole-app review of 19 Nov, PM-1: the plugin files its uninstaller left are offered for the Recycle Bin,
    /// as when there's no uninstaller; the ones that went aren't listed.
    #[test]
    fn the_files_its_uninstaller_left_are_offered_for_the_bin() {
        let files = files_in("mo-app-uninstall-left", &["Arcade.vst3", "Arcade.dll"]);
        std::fs::remove_file(&files[1]).unwrap();
        let program = |path: &str| mo_space::plugin_space::SpaceFolder {
            path: path.into(),
            kind: SpaceKind::Program,
            bytes: 1,
            shared_with: Vec::new(),
            added: false,
        };
        let presets =
            mo_space::plugin_space::SpaceFolder { kind: SpaceKind::Presets, ..program("C:\\Presets\\Arcade") };
        let ask = Ask {
            row_key: ("arcade".into(), String::new()),
            name: "Arcade".into(),
            vendor: "Output".into(),
            songs: 0,
            songs_now: 0,
            stands_in: None,
            space: PluginSpace {
                key: "arcade".into(),
                name: "Arcade".into(),
                vendor: "Output".into(),
                folders: vec![program(&files[0]), program(&files[1]), presets],
            },
            installed: Vec::new(),
            names: HashMap::new(),
        };
        let left = there_now(&files);
        let lines = ask.left_lines(&left, &AtomicBool::new(false)).unwrap();
        assert_eq!(lines.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), [files[0].as_str()]);
        assert_eq!((lines[0].format.as_str(), lines[0].stays.as_deref()), ("VST3", None));
        tidy(&files);
    }

    #[test]
    fn what_the_card_says_once_it_ran() {
        assert!(ran_text("Arcade", &Ran::Finished, Waited::Gone, None).starts_with("Arcade is uninstalled,"));
        let cancelled = Ran::NothingChanged("It was cancelled.".into());
        assert!(ran_text("Arcade", &cancelled, Waited::StillThere, None).ends_with("It was cancelled."));
        assert!(ran_text("Arcade", &Ran::Finished, Waited::StillThere, None).contains("still there"));
        assert!(ran_text("Arcade", &Ran::Unclear("Code 2.".into()), Waited::Unknown, None).contains("(Code 2)"));
        let u = Uninstaller { name: "Arcade".into(), publisher: "Output".into(), command: "x".into(), bytes: 2048 };
        assert_eq!(uninstaller_text(&u), ("Arcade".into(), format!("Output · {}", size_text(2048))));
    }
}
