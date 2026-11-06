//! Hide duplicates: finding the sounds whose audio is exactly the same ([`mo_sounds::copies`]) away from the window's
//! thread, on the low-priority pool for work that waits on disks, and handing them to the page's list. They're looked
//! for only while the switch is on, once for each library read: what's known already (`sound-copies.db`) comes first,
//! in a moment, then the files not read before (or changed since). Turning the switch off stops a look part way; what
//! it read is kept for next time.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mo_app::sounds::{self as list, CopiesFound, SoundList, SoundsOf};
use mo_app::{background, log};
use mo_sounds::copies::{self, Finding};
use slint::ComponentHandle;

use super::PAGE;
use crate::AppWindow;

/// The look under way, or done, and for which sounds.
#[derive(Default)]
struct Look {
    /// Where the prints are kept: beside the sound library ([`connect`]).
    kept_at: Option<PathBuf>,
    /// The sounds looked through (or being): known without keeping them, so an old list is never kept alive.
    of: Option<SoundsOf>,
    /// Counts the looks, so word from one stopped late is dropped.
    look: u64,
    /// Stops the look under way.
    stop: Option<Arc<AtomicBool>>,
    found: CopiesFound,
    /// The Space page counts the copies too (its Copies of the same sound): they're looked for with the switch off.
    for_space: bool,
}

thread_local! {
    static LOOK: RefCell<Look> = RefCell::default();
}

/// Held by the look under way.
static ONE_LOOK: Mutex<()> = Mutex::new(());

/// Copies are kept beside the sound library at `sounds_db`.
pub(super) fn connect(sounds_db: &std::path::Path) {
    LOOK.with(|l| l.borrow_mut().kept_at = Some(sounds_db.with_file_name(copies::FILE_NAME)));
}

/// How far the look has got, for the chip.
pub(super) fn found() -> CopiesFound {
    LOOK.with(|l| l.borrow().found)
}

/// The Space page wants the copies (its Copies of the same sound, the same copies as Hide duplicates hides): they're
/// looked for from now on whether the switch is on or not, and Space is told as they come ([`crate::space::sounds_changed`]).
pub(crate) fn for_space(app: &AppWindow) {
    LOOK.with(|l| l.borrow_mut().for_space = true);
    wanted(app);
}

/// Whether the look is for (or was for) a list sharing `list`'s sounds.
fn looks_at(l: &Look, list: &SoundList) -> bool {
    l.of.as_ref().is_some_and(|of| of.is_of(list))
}

/// The page's list or switch changed: with Hide duplicates on (or the Space page counting copies) and every sound
/// read, the copies are looked for in them, unless they're already being (or have been, or that look failed: it's
/// tried again when the switch is turned off and on, or the sounds are read again). With it off, a look under way
/// stops.
pub(super) fn wanted(app: &AppWindow) {
    let for_space = LOOK.with(|l| l.borrow().for_space);
    let (list, on, whole) = PAGE.with(|p| {
        let p = p.borrow();
        (p.list.clone(), p.filters.hide_copies || for_space, p.read && p.partial.is_none() && !p.list.is_empty())
    });
    if super::HERE.with(|h| *h.borrow()) {
        // The screenshots and tests that work on the window's thread never read files.
        return;
    }
    if !on {
        LOOK.with(|l| {
            let mut l = l.borrow_mut();
            if let Some(stop) = l.stop.take() {
                stop.store(true, Ordering::Relaxed);
            }
            // Looked for again from what was kept, next time it's on (after a failed look too).
            l.of = None;
            l.found = CopiesFound::NotLooked;
        });
        return;
    }
    if !whole {
        return;
    }
    let start = LOOK.with(|l| {
        let mut l = l.borrow_mut();
        if looks_at(&l, &list) {
            return None;
        }
        if let Some(stop) = l.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        let kept_at = l.kept_at.clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        l.look += 1;
        l.of = Some(list.sounds_of());
        l.stop = Some(stop.clone());
        l.found = CopiesFound::Looking { done: 0, of: 0 };
        Some((l.look, kept_at, stop))
    });
    let Some((look, kept_at, stop)) = start else { return };
    let weak = app.as_weak();
    let tell = move |word: Word| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                heard(&app, look, word);
            }
        });
    };
    let started = background::spawn("Sounds duplicates", move || {
        // One look at a time: a new one waits for the one it replaced to stop (it's told to), so the two never share
        // the file the prints are kept in.
        let _one = ONE_LOOK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let found = background::on_bulk_pool(|| {
            copies::find(list.sounds(), &kept_at, &stop, |f| match f {
                Finding::Known { same, left } => {
                    tell(Word::Copies(list.copies_in(&same), Some(left)));
                }
                Finding::Reading { done, of } => tell(Word::Reading { done, of }),
            })
            .map(|same| list.copies_in(&same))
        });
        match found {
            Ok(copies) => tell(Word::Copies(copies, None)),
            Err(e) => {
                log::warning("Sounds", &format!("Couldn't look for duplicates: {e}"));
                tell(Word::Failed);
            }
        }
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start looking for duplicates: {e}"));
        LOOK.with(|l| l.borrow_mut().found = CopiesFound::Failed);
    }
}

/// What a look says, on the window's thread.
enum Word {
    /// The copies found so far, and how many files are left to read (`None` once they all have been).
    Copies(Arc<list::Copies>, Option<usize>),
    Reading {
        done: usize,
        of: usize,
    },
    Failed,
}

fn heard(app: &AppWindow, look: u64, word: Word) {
    let _doing = crate::watchdog::doing("Sounds: duplicates found");
    let current = LOOK.with(|l| l.borrow().look == look && l.borrow().stop.is_some());
    if !current {
        return;
    }
    match word {
        Word::Copies(found, left) => {
            let n = found.count();
            // Onto the page's list, if it's still of these sounds (a list with the use counts in shares them).
            let put = PAGE.with(|p| {
                let mut p = p.borrow_mut();
                let fits = LOOK.with(|l| looks_at(&l.borrow(), &p.list));
                if fits {
                    p.list = Arc::new(p.list.with_copies(found));
                }
                fits
            });
            if !put {
                return;
            }
            // The Space page counts these copies too.
            crate::space::sounds_changed(app);
            LOOK.with(|l| {
                let mut l = l.borrow_mut();
                l.found = match left {
                    Some(0) | None => CopiesFound::Found(n),
                    Some(of) => CopiesFound::Looking { done: 0, of },
                };
                if left.is_none() {
                    // Done: nothing to stop.
                    l.stop = None;
                }
            });
            super::refresh(app);
        }
        Word::Reading { done, of } => {
            LOOK.with(|l| l.borrow_mut().found = CopiesFound::Looking { done, of });
            super::show_chips(app);
        }
        Word::Failed => {
            LOOK.with(|l| {
                let mut l = l.borrow_mut();
                l.found = CopiesFound::Failed;
                // Kept as looked at, so a filter or search doesn't start the whole look again (review B-2): tried
                // again when the switch is turned off and on, or the sounds are read again.
                l.stop = None;
            });
            super::show_chips(app);
            crate::space::sounds_changed(app);
        }
    }
}

/// The screenshots' copies: `same` put on the page's list as if a look had found them (none, and not looked for, with
/// `None`). Never reads a file.
pub(super) fn show_for_screens(app: &AppWindow, same: Option<&mo_sounds::copies::SameAudio>) {
    let found = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let copies = p.list.copies_in(same.unwrap_or(&Default::default()));
        let n = copies.count();
        p.list = Arc::new(p.list.with_copies(copies));
        n
    });
    LOOK.with(|l| {
        let mut l = l.borrow_mut();
        l.found = if same.is_some() { CopiesFound::Found(found) } else { CopiesFound::NotLooked };
    });
    super::refresh(app);
}
