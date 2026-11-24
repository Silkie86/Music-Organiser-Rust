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
    /// How to do it, step by step, for a step that needs more than its one line (Solomon, 24 Nov: "explained in more
    /// detail so I'm not confused what it's asking me to do"). Not part of the step's words a mark keeps
    /// ([`step_words`]), so adding or changing it never asks for the step to be tried again.
    pub how: &'static str,
}

/// Reads [`LIST`]: a line `== Area` starts an area; a line `id | what to do | what you should see` is a step in it,
/// with `| how, step by step` after it if it needs one; empty lines and lines starting `#` are left out.
pub fn steps() -> Vec<Step> {
    parse(LIST)
}

/// The lines of `text` that are none of those (a step with its `|` left out, say), which [`steps`] leaves out.
pub fn bad_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("=="))
        .filter(|l| {
            let parts: Vec<&str> = l.splitn(4, '|').map(str::trim).collect();
            parts.len() < 3 || parts.iter().any(|p| p.is_empty())
        })
        .collect()
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
        let mut parts = line.splitn(4, '|').map(str::trim);
        let (Some(id), Some(what)) = (parts.next(), parts.next()) else { continue };
        let expect = parts.next().unwrap_or("");
        out.push(Step { id, area, what, expect, how: parts.next().unwrap_or("") });
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
            Mark::Skipped => "Skipped",
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
    /// When the self-check last ran, in seconds since 1970 (0 for never), so it runs once a day.
    #[serde(default)]
    pub self_checked: u64,
    /// Notes on steps with no mark (kept with Enter before marking, or left when a mark was taken off), so they're
    /// saved too (the 19 Nov review, BT-2). A mark takes its step's note from here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notes: BTreeMap<String, String>,
}

/// A step's words as a mark keeps them: "what -> expect".
pub fn step_words(step: &Step) -> String {
    if step.expect.is_empty() { step.what.to_owned() } else { format!("{} -> {}", step.what, step.expect) }
}

fn cut(note: &str) -> String {
    note.trim().chars().take(NOTE_MAX).collect()
}

/// Whether a step is one the self-check marks ([`crate::self_check`]), not Solomon.
pub fn is_self_check(id: &str) -> bool {
    id.starts_with("auto.")
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

    /// Marks `step`: `None` takes its mark off. The note is kept (cut to [`NOTE_MAX`]), also when the mark goes.
    pub fn set(&mut self, step: &Step, mark: Option<Mark>, note: &str, at: Moment<'_>) {
        let note = cut(note);
        let Some(mark) = mark else {
            self.steps.remove(step.id);
            if note.is_empty() {
                self.notes.remove(step.id);
            } else {
                self.notes.insert(step.id.to_owned(), note);
            }
            return;
        };
        self.notes.remove(step.id);
        let (log, log_file) = if mark == Mark::Problem {
            let skip = at.recent.len().saturating_sub(KEPT_LOG_LINES);
            (
                at.recent.into_iter().skip(skip).collect(),
                at.log_file.map(|p| p.display().to_string()).unwrap_or_default(),
            )
        } else {
            (Vec::new(), String::new())
        };
        self.steps.insert(
            step.id.to_owned(),
            Marked { mark, note, when: at.when, version: at.version.to_owned(), step: step_words(step), log, log_file },
        );
    }

    /// Changes only a step's note, marked or not; whether it changed.
    pub fn set_note(&mut self, id: &str, note: &str) -> bool {
        let note = cut(note);
        if let Some(m) = self.steps.get_mut(id) {
            let changed = m.note != note;
            m.note = note;
            return changed;
        }
        if note.is_empty() {
            self.notes.remove(id).is_some()
        } else {
            self.notes.insert(id.to_owned(), note.clone()).as_ref() != Some(&note)
        }
    }

    /// A step's note, marked or not.
    pub fn note(&self, id: &str) -> &str {
        self.steps.get(id).map(|m| m.note.as_str()).or_else(|| self.notes.get(id).map(String::as_str)).unwrap_or("")
    }

    /// The step's mark as it stands: none when the step's words changed since it was marked (a step is reworded
    /// under its own name, so its old mark doesn't say the new part works: the 19 Nov review, BT-3).
    pub fn current(&self, step: &Step) -> Option<&Marked> {
        self.steps.get(step.id).filter(|m| m.step.is_empty() || m.step == step_words(step))
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
    let count = |m: Mark| steps.iter().filter(|s| marks.current(s).is_some_and(|x| x.mark == m)).count();
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
    pub how: &'static str,
    pub mark: Option<Mark>,
    pub note: String,
    /// Marked by the self-check, not by hand.
    pub auto: bool,
    /// "Problem on 0.1.0 (abc), 2026-11-15 18:04", or empty.
    pub said: String,
}

/// The card's lines, each area under its heading; with `to_try`, only the steps not yet marked Works or Skipped
/// (a problem stays, to try again). A step whose words changed since its mark shows unmarked, with the old mark said.
pub fn lines(steps: &[Step], marks: &Marks, to_try: bool) -> Vec<Line> {
    let mut out = Vec::new();
    let mut last = None;
    for s in steps {
        let m = marks.current(s);
        let old = if m.is_none() { marks.get(s.id) } else { None };
        if to_try && m.is_some_and(|m| m.mark != Mark::Problem) {
            continue;
        }
        if last != Some(s.area) {
            out.push(Line {
                area: s.area,
                id: "",
                what: "",
                expect: "",
                how: "",
                mark: None,
                note: String::new(),
                auto: false,
                said: String::new(),
            });
            last = Some(s.area);
        }
        out.push(Line {
            area: "",
            id: s.id,
            what: s.what,
            expect: s.expect,
            how: s.how,
            mark: m.map(|m| m.mark),
            note: marks.note(s.id).to_owned(),
            auto: is_self_check(s.id),
            said: match (m, old) {
                (Some(m), _) => format!("{} on {}, {}", m.mark.name(), m.version, m.when),
                (None, Some(o)) => {
                    format!("{} on {}, {}, before this step changed: try it again", o.mark.name(), o.version, o.when)
                }
                (None, None) => String::new(),
            },
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
        assert!(bad_lines(LIST).is_empty(), "not read as steps: {:?}", bad_lines(LIST));
        assert_eq!(bad_lines("== A\na.1 | One | x\na.2 One | x\n"), ["a.2 One | x"]);
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
        assert_eq!(s[0], Step { id: "songs.open", area: "Songs", what: "Open one", expect: "It opens", how: "" });
        assert_eq!((s[1].area, s[1].expect), ("Sounds", ""));
        let h = parse("== A\na.1 | Do it | It works | Open A, then press B\n");
        assert_eq!((h[0].expect, h[0].how), ("It works", "Open A, then press B"));
        // The how isn't part of what a mark keeps, so adding one leaves the step's marks as they are.
        assert_eq!(step_words(&h[0]), "Do it -> It works");
    }

    #[test]
    fn a_problem_keeps_the_last_log_lines_and_others_none() {
        let step = Step { id: "a.b", area: "A", what: "Do it", expect: "It works", how: "" };
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
        let step = Step { id: "a.b", area: "A", what: "Do it", expect: "It works", how: "" };
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
        assert!(log_line(&steps[2], Some(Mark::Problem), "again").contains("Problem"));
    }

    #[test]
    fn a_note_on_an_unmarked_step_is_kept_and_goes_with_its_mark() {
        let step = Step { id: "a.b", area: "A", what: "Do it", expect: "It works", how: "" };
        let mut m = Marks::default();
        assert!(m.set_note("a.b", "  try after the update "));
        assert!(!m.set_note("a.b", "try after the update"), "the same note again isn't a change");
        assert_eq!(m.note("a.b"), "try after the update");
        // Saved in the file too.
        let back: Marks = serde_json::from_str(&m.to_json()).expect("read back");
        assert_eq!(back.note("a.b"), "try after the update");
        // A mark takes it along.
        m.set(&step, Some(Mark::Problem), "try after the update", at(0));
        assert!(m.notes.is_empty());
        assert_eq!(m.get("a.b").expect("marked").note, "try after the update");
        // Taking the mark off keeps the note.
        m.set(&step, None, "try after the update", at(0));
        assert!(m.get("a.b").is_none());
        assert_eq!(m.note("a.b"), "try after the update");
        assert_eq!(lines(std::slice::from_ref(&step), &m, false)[1].note, "try after the update");
        // An emptied box takes it off.
        assert!(m.set_note("a.b", " "));
        assert!(m.notes.is_empty());
        // An old file with no notes reads.
        let old: Marks = serde_json::from_str(r#"{"steps":{},"self_checked":0}"#).expect("old file");
        assert!(old.notes.is_empty());
    }

    #[test]
    fn a_reworded_step_is_to_try_again() {
        let before = Step { id: "a.b", area: "A", what: "Do it", expect: "It works", how: "" };
        let after = Step { id: "a.b", area: "A", what: "Do it, then minimise", expect: "It still works", how: "" };
        let mut m = Marks::default();
        m.set(&before, Some(Mark::Works), "fine", at(0));
        assert_eq!(progress(std::slice::from_ref(&before), &m), "1 of 1 work, 0 to try");
        assert!(lines(std::slice::from_ref(&before), &m, true).is_empty());
        let shown = lines(std::slice::from_ref(&after), &m, true);
        assert_eq!(shown.len(), 2, "back in Only to try");
        assert_eq!(shown[1].mark, None);
        assert_eq!(shown[1].note, "fine");
        assert!(
            shown[1].said.starts_with("Works on 0.1.0 (abc)") && shown[1].said.contains("before this step changed")
        );
        assert_eq!(progress(std::slice::from_ref(&after), &m), "0 of 1 work, 1 to try");
        // The old mark stays in the file until it's marked again.
        assert!(m.get("a.b").is_some());
        m.set(&after, Some(Mark::Works), "fine", at(0));
        assert!(lines(std::slice::from_ref(&after), &m, true).is_empty());
    }
}
