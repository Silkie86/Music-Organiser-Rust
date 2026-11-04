//! What the app has put in the Recycle Bin (the Trash on a Mac or Linux) that's still there, for the Space page to
//! remind you of. The app never empties the bin itself, so what it put there still takes room on its drive until you
//! empty the bin, and until then you can put it back (Solomon, 3 Nov 2026: "leave it to me with a warning").

use std::collections::BTreeMap;

use crate::clean::{Bin, Done, Removed};
use crate::size::size_text;

/// What the bin is called here.
const BIN: &str = if cfg!(windows) { "Recycle Bin" } else { "Trash" };

/// What's still in the bin, of what the app put there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InBin {
    /// How many files and folders.
    pub items: usize,
    /// How much room they take.
    pub bytes: u64,
    /// When the first of them went (Unix seconds).
    pub since: Option<i64>,
    /// The room they take on each drive ("C:"), most first; none when the paths don't name drives (a Mac or Linux).
    pub drives: Vec<(String, u64)>,
    /// Whether the bin could be looked in. A Mac's Trash can't be: then it's all the app put there, emptied or not.
    pub looked: bool,
}

impl InBin {
    /// Whether there's nothing to remind you of.
    pub fn is_empty(&self) -> bool {
        self.items == 0
    }

    /// What the reminder says first: "12.3 GB the app put in the Recycle Bin is still there"; nothing when there's
    /// none.
    pub fn title(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let size = size_text(self.bytes);
        if self.looked {
            format!("{size} the app put in the {BIN} is still there")
        } else {
            format!("The app has put {size} in the {BIN}")
        }
    }

    /// What it says then: "148 files and folders, since 3 Nov 2026. It still takes room on C: (10.1 GB) and D:
    /// (2.2 GB) until you empty the Recycle Bin: the app never empties it, so until then you can put it back."
    pub fn note(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let things =
            if self.items == 1 { "1 file or folder".to_owned() } else { format!("{} files and folders", self.items) };
        let since = self.since.and_then(day).map(|d| format!(", since {d}")).unwrap_or_default();
        let on = match self.drives.as_slice() {
            [] => String::new(),
            [(drive, _)] => format!(" on {drive}"),
            [rest @ .., last] => {
                let each = |(drive, bytes): &(String, u64)| format!("{drive} ({})", size_text(*bytes));
                let rest: Vec<String> = rest.iter().map(each).collect();
                format!(" on {} and {}", rest.join(", "), each(last))
            }
        };
        format!(
            "{things}{since}. It still takes room{on} until you empty the {BIN}: the app never empties it, so until \
             then you can put it back."
        )
    }
}

/// What's still in the bin of what the app's clean-ups put there. `done` is every clean-up's list the app keeps (what
/// each one binned, how big, when). Looks in the bin: call it away from the window's thread.
pub fn remind(done: &[Done], bin: &dyn Bin) -> InBin {
    let removed: Vec<Removed> = done.iter().flat_map(|d| d.removed.iter().cloned()).collect();
    let held = bin.holds(&removed);
    remind_held(&removed, held.as_deref())
}

/// What's still in the bin of `removed` ([`remind`]), from a look in the bin already made: `held` says which of them
/// the bin holds, in the same order, or is `None` when it couldn't be looked in (then it's all of them, emptied or not).
pub fn remind_held<'a>(removed: impl IntoIterator<Item = &'a Removed>, held: Option<&[bool]>) -> InBin {
    let mut found = InBin { looked: held.is_some(), ..InBin::default() };
    let mut drives: BTreeMap<String, u64> = BTreeMap::new();
    for (i, r) in removed.into_iter().enumerate() {
        if held.is_some_and(|held| !held.get(i).copied().unwrap_or(false)) {
            continue;
        }
        found.items += 1;
        found.bytes = found.bytes.saturating_add(r.bytes);
        found.since = Some(found.since.map_or(r.when, |since| since.min(r.when)));
        if let Some(drive) = drive_of(&r.real) {
            let on = drives.entry(drive).or_default();
            *on = on.saturating_add(r.bytes);
        }
    }
    found.drives = drives.into_iter().collect();
    found.drives.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    found
}

/// The drive a path is on, as Windows names it ("C:"), when the path names one.
fn drive_of(path: &str) -> Option<String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut chars = path.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => Some(format!("{}:", letter.to_ascii_uppercase())),
        _ => None,
    }
}

/// A day as the app shows one ("3 Nov 2026"), from Unix seconds, in the computer's own time.
fn day(seconds: i64) -> Option<String> {
    let at = chrono::DateTime::from_timestamp(seconds, 0)?;
    Some(at.with_timezone(&chrono::Local).format("%-d %b %Y").to_string())
}
