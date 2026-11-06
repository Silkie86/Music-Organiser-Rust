//! How long the app takes to start: one `[Start]` line in the log once the window has drawn its first frame, with the
//! time each step took, so a slow start on Solomon's PC says where its time went.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use mo_app::log;

/// The steps so far: what each was and how long it took, from when the app started.
struct Steps {
    started: Instant,
    last: Instant,
    done: Vec<(&'static str, Duration)>,
    said: bool,
}

thread_local! {
    static STEPS: RefCell<Option<Steps>> = const { RefCell::new(None) };
}

/// The app starts now (the first thing `main` does). Only the window's thread keeps the steps.
pub fn begin() {
    let now = Instant::now();
    STEPS.with(|s| *s.borrow_mut() = Some(Steps { started: now, last: now, done: Vec::new(), said: false }));
}

/// The step called `what` has just finished: it took the time since the last step did.
pub fn step(what: &'static str) {
    STEPS.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut()
            && !s.said
        {
            let now = Instant::now();
            s.done.push((what, now - s.last));
            s.last = now;
        }
    });
}

/// The window has drawn a frame: the first time, the `[Start]` line goes in the log.
pub fn frame_drawn() {
    let line = STEPS.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut().filter(|s| !s.said)?;
        let now = Instant::now();
        s.done.push(("first frame", now - s.last));
        s.said = true;
        Some(line(now - s.started, &s.done))
    });
    if let Some(line) = line {
        log::info("Start", &line);
    }
}

/// The `[Start]` line: the whole time, then each step's own, in the order they ran.
fn line(total: Duration, steps: &[(&str, Duration)]) -> String {
    let parts: Vec<String> = steps.iter().map(|(what, took)| format!("{what} {}", ms(*took))).collect();
    format!("Window drawn {} after the app started: {}", secs(total), parts.join(", "))
}

fn ms(d: Duration) -> String {
    format!("{} ms", d.as_millis())
}

fn secs(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// What to do when the data folder was moved to a drive that isn't there at start.
pub enum Missing {
    /// Look again (the drive was just plugged in).
    TryAgain,
    /// Open on the old copy in the first folder, knowing changes made now stay there.
    UseOldCopy,
    /// Don't open the app.
    Close,
}

/// Asks, before the window opens and before any file of the app is opened, what to do about a moved data folder that
/// can't be found (F-2): `problem` is why. Windows' own buttons (Yes, No, Cancel) carry the three choices.
pub fn data_folder_missing(problem: &str) -> Missing {
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Music Organiser: data folder not found")
        .set_description(format!(
            "{problem}.\n\nYes: look again (plug the drive in first).\nNo: open on the old copy. It is from before the \
             move, and changes made now stay in it, not in the moved folder.\nCancel: close the app."
        ))
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();
    match answer {
        rfd::MessageDialogResult::Yes => Missing::TryAgain,
        rfd::MessageDialogResult::No => Missing::UseOldCopy,
        _ => Missing::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_names_every_step_in_order() {
        let steps = [("settings", Duration::from_millis(160)), ("graphics card", Duration::from_millis(2400))];
        assert_eq!(
            line(Duration::from_millis(2600), &steps),
            "Window drawn 2.60 s after the app started: settings 160 ms, graphics card 2400 ms"
        );
    }

    #[test]
    fn only_the_first_frame_is_told() {
        begin();
        step("settings");
        frame_drawn();
        step("late");
        STEPS.with(|s| {
            let s = s.borrow();
            let s = s.as_ref().unwrap();
            assert!(s.said);
            let names: Vec<&str> = s.done.iter().map(|(w, _)| *w).collect();
            assert_eq!(names, ["settings", "first frame"]);
        });
    }
}
