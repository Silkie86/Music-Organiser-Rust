//! The beta checklist on Settings › Developer (Solomon, 2026-11-15: "This can be put on the developer part of the app
//! and get ticked off"; "you should be able to see that happen without me telling you"; "log as much as possible, so
//! you can see why it went wrong").
//!
//! The steps are written in `beta/checklist.txt` beside this crate's `src`, built into the app, so a thread that adds a
//! feature adds a line there. What Solomon marks is kept in `beta-checklist.json` in the data folder: each step's mark
//! (Works, Problem or Skipped), his note, when, on which build, the step's words as they were, and for a problem the
//! log's last lines from just before it, so Claude reads the whole story from the folder. Each mark also goes in the
//! session log as a `[Beta]` line.
//!
//! The file holds at most one entry per step ever listed, each with at most [`KEPT_LOG_LINES`] log lines, so it stays
//! small (rule 28); it's Solomon's own record, so the start-up sweep never removes it, only what a crash left of its
//! writing (it's on [`crate::app_settings::WRITTEN_CHECKED`]).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name, in the data folder.
pub const FILE: &str = "beta-checklist.json";

/// The steps, as written (see [`steps`] for the layout).
pub const LIST: &str = include_str!("../beta/checklist.txt");

/// The most log lines kept with a problem.
pub const KEPT_LOG_LINES: usize = 60;
/// The longest note kept, in characters.
pub const NOTE_MAX: usize = 2000;

/// One step to try.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// Its name in the file, which never changes once given (`songs.open`).
    pub id: &'static str,
    /// The part of the app it's in, as headed in the list.
    pub area: &'static str,
    /// What to do.
    pub what: &'static str,
    /// What you should see.
    pub expect: &'static str,
}

/// Reads [`LIST`]: a line `== Area` starts an area; a line `id | what to do | what you should see` is a step in it;
/// empty lines and lines starting `#` are left out.
pub fn steps() -> Vec<Step> {
    parse(LIST)
}

fn parse(text: &'static str) -> Vec<Step> {
    let mut area = "";
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(a) = line.strip_prefix("==") {
            area = a.trim();
            continue;
        }
        let mut parts = line.splitn(3, '|').map(str::trim);
        let (Some(id), Some(what)) = (parts.next(), parts.next()) else { continue };
        out.push(Step { id, area, what, expect: parts.next().unwrap_or("") });
    }
    out
}

/// How a step went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    Works,
    Problem,
    Skipped,
}

impl Mark {
    pub const ALL: [Mark; 3] = [Mark::Works, Mark::Problem, Mark::Skipped];

    pub fn name(self) -> &'static str {
        match self {
            Mark::Works => "Works",
            Mark::Problem => "Problem",
            Mark::Skipped => "Skip",
        }
    }
}

/// What was marked for one step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marked {
    pub mark: Mark,
    #[serde(default)]
    pub note: String,
    /// When, as `2026-11-15 18:04`, local time.
    pub when: String,
    /// The app's version then.
    pub version: String,
    /// The step's words then, so the file reads on its own.
    #[serde(default)]
    pub step: String,
    /// For a problem: the session log's last lines just before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
    /// That run's log file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log_file: String,
}

/// Everything marked, by step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    #[serde(default)]
    pub steps: BTreeMap<String, Marked>,
}

/// What the app knows when a step is marked.
pub struct Moment<'a> {
    pub when: String,
    pub version: &'a str,
    /// The session log's recent lines (only kept for a problem).
    pub recent: Vec<String>,
    pub log_file: Option<&'a Path>,
}

impl Marks {
    /// Reads the file in `folder`; none yet is no marks. A file that can't be read is an error, so the page says so
    /// and doesn't write over it.
    pub fn load(folder: &Path) -> io::Result<Marks> {
        match fs::read_to_string(folder.join(FILE)) {
            Ok(text) => serde_json::from_str(&text).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Marks::default()),
            Err(e) => Err(e),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Marked> {
        self.steps.get(id)
    }

    /// Marks `step`: `None` takes its mark off. The note is kept (cut to [`NOTE_MAX`]).
    pub fn set(&mut self, step: &Step, mark: Option<Mark>, note: &str, at: Moment<'_>) {
        let Some(mark) = mark else {
            self.steps.remove(step.id);
            return;
        };
        let note: String = note.trim().chars().take(NOTE_MAX).collect();
        let (log, log_file) = if mark == Mark::Problem {
            let skip = at.recent.len().saturating_sub(KEPT_LOG_LINES);
            (
                at.recent.into_iter().skip(skip).collect(),
                at.log_file.map(|p| p.display().to_string()).unwrap_or_default(),
            )
        } else {
            (Vec::new(), String::new())
        };
        let step_words =
            if step.expect.is_empty() { step.what.to_owned() } else { format!("{} -> {}", step.what, step.expect) };
        self.steps.insert(
            step.id.to_owned(),
            Marked { mark, note, when: at.when, version: at.version.to_owned(), step: step_words, log, log_file },
        );
    }

    /// Changes only the note of a marked step (a note on an unmarked one waits for its mark).
    pub fn set_note(&mut self, id: &str, note: &str) -> bool {
        match self.steps.get_mut(id) {
            Some(m) => {
                let note: String = note.trim().chars().take(NOTE_MAX).collect();
                let changed = m.note != note;
                m.note = note;
                changed
            }
            None => false,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Writes the file the safe way. Waits for the disk: call it off the window's thread.
    pub fn save(folder: &Path, json: &str) -> io::Result<PathBuf> {
        fs::create_dir_all(folder)?;
        let path = folder.join(FILE);
        crate::app_settings::write_checked(&path, json)?;
        Ok(path)
    }
}

/// The line under the card's title: how far through it is.
pub fn progress(steps: &[Step], marks: &Marks) -> String {
    let count = |m: Mark| steps.iter().filter(|s| marks.get(s.id).is_some_and(|x| x.mark == m)).count();
    let (works, problems, skipped) = (count(Mark::Works), count(Mark::Problem), count(Mark::Skipped));
    let left = steps.len() - works - problems - skipped;
    let mut out = format!("{works} of {} work", steps.len());
    if problems > 0 {
        out.push_str(&format!(", {problems} with a problem"));
    }
    if skipped > 0 {
        out.push_str(&format!(", {skipped} skipped"));
    }
    out.push_str(&format!(", {left} to try"));
    out
}

/// The `[Beta]` line for the session log.
pub fn log_line(step: &Step, mark: Option<Mark>, note: &str) -> String {
    let what = mark.map_or("mark taken off", Mark::name);
    if note.trim().is_empty() {
        format!("{}: {what} ({})", step.id, step.what)
    } else {
        format!("{}: {what} ({}). Note: {}", step.id, step.what, note.trim())
    }
}

/// One line on the card: an area's heading (only `area` set) or a step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub area: &'static str,
    pub id: &'static str,
    pub what: &'static str,
    pub expect: &'static str,
    pub mark: Option<Mark>,
    pub note: String,
    /// "Problem on 0.1.0 (abc), 15 Nov 18:04", or empty.
    pub said: String,
}

/// The card's lines, each area under its heading; with `to_try`, only the steps not yet marked Works or Skipped
/// (a problem stays, to try again).
pub fn lines(steps: &[Step], marks: &Marks, to_try: bool) -> Vec<Line> {
    let mut out = Vec::new();
    let mut last = None;
    for s in steps {
        let m = marks.get(s.id);
        if to_try && m.is_some_and(|m| m.mark != Mark::Problem) {
            continue;
        }
        if last != Some(s.area) {
            out.push(Line {
                area: s.area,
                id: "",
                what: "",
                expect: "",
                mark: None,
                note: String::new(),
                said: String::new(),
            });
            last = Some(s.area);
        }
        out.push(Line {
            area: "",
            id: s.id,
            what: s.what,
            expect: s.expect,
            mark: m.map(|m| m.mark),
            note: m.map(|m| m.note.clone()).unwrap_or_default(),
            said: m.map(|m| format!("{} on {}, {}", m.mark.name(), m.version, m.when)).unwrap_or_default(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(recent: usize) -> Moment<'static> {
        Moment {
            when: "2026-11-15 18:04".into(),
            version: "0.1.0 (abc)",
            recent: (0..recent).map(|i| format!("line {i}")).collect(),
            log_file: Some(Path::new("logs/rust-session-1.log")),
        }
    }

    #[test]
    fn the_list_reads_and_every_name_is_its_own() {
        let steps = steps();
        assert!(steps.len() > 20, "{}", steps.len());
        let mut ids: Vec<_> = steps.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "two steps share a name");
        for s in &steps {
            assert!(!s.area.is_empty(), "{} has no area", s.id);
            assert!(!s.what.is_empty() && !s.expect.is_empty(), "{} needs what to do and what to see", s.id);
            assert!(!s.id.contains(' '), "{}", s.id);
        }
    }

    #[test]
    fn layout_of_the_list() {
        let s = parse("# a note\n== Songs\nsongs.open | Open one | It opens\n\n== Sounds\nsounds.play | Play one\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Step { id: "songs.open", area: "Songs", what: "Open one", expect: "It opens" });
        assert_eq!((s[1].area, s[1].expect), ("Sounds", ""));
    }

    #[test]
    fn a_problem_keeps_the_last_log_lines_and_others_none() {
        let step = Step { id: "a.b", area: "A", what: "Do it", expect: "It works" };
        let mut m = Marks::default();
        m.set(&step, Some(Mark::Problem), "  it froze  ", at(100));
        let got = m.get("a.b").expect("marked");
        assert_eq!(got.note, "it froze");
        assert_eq!(got.log.len(), KEPT_LOG_LINES);
        assert_eq!(got.log.last().map(String::as_str), Some("line 99"));
        assert_eq!(got.step, "Do it -> It works");
        assert!(!got.log_file.is_empty());
        m.set(&step, Some(Mark::Works), "", at(100));
        assert!(m.get("a.b").expect("marked").log.is_empty());
        m.set(&step, None, "", at(0));
        assert!(m.get("a.b").is_none());
    }

    #[test]
    fn saved_and_read_back() {
        let dir = std::env::temp_dir().join(format!("mo-beta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(Marks::load(&dir).expect("none yet"), Marks::default());
        let step = Step { id: "a.b", area: "A", what: "Do it", expect: "It works" };
        let mut m = Marks::default();
        m.set(&step, Some(Mark::Problem), "slow", at(3));
        Marks::save(&dir, &m.to_json()).expect("saved");
        assert_eq!(Marks::load(&dir).expect("read"), m);
        fs::write(dir.join(FILE), "{ not json").expect("spoilt");
        assert!(Marks::load(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_and_lines() {
        let steps = parse("== A\na.1 | One | x\na.2 | Two | x\n== B\nb.1 | Three | x\n");
        let mut m = Marks::default();
        m.set(&steps[0], Some(Mark::Works), "", at(0));
        m.set(&steps[2], Some(Mark::Problem), "", at(0));
        assert_eq!(progress(&steps, &m), "1 of 3 work, 1 with a problem, 1 to try");
        let all = lines(&steps, &m, false);
        assert_eq!(all.len(), 5);
        let to_try = lines(&steps, &m, true);
        assert_eq!(
            to_try.iter().map(|l| if l.id.is_empty() { l.area } else { l.id }).collect::<Vec<_>>(),
            ["A", "a.2", "B", "b.1"]
        );
        assert!(m.set_note("b.1", "again"));
        assert!(!m.set_note("a.2", "not marked"));
        assert!(log_line(&steps[2], Some(Mark::Problem), "again").contains("Problem"));
    }
}
