//! When the sounds' files were made and last opened, for the year strip's Made and Opened ([`mo_app::sound_dates`]):
//! read away from the window's thread, on the low-priority pool for work that waits on disks, once for each library
//! read, and only once one of them is picked. Picking another date lets a read under way finish (it's kept for when
//! you pick one of them again); the list it was for going away drops it.

use std::cell::RefCell;
use std::sync::Arc;

use mo_app::sound_dates::{self, FileDates};
use mo_app::sounds::SoundsOf;
use mo_app::{background, log};
use slint::ComponentHandle;

use super::PAGE;
use crate::AppWindow;

/// The read under way, or done, and for which sounds.
#[derive(Default)]
struct Read {
    of: Option<SoundsOf>,
    /// Counts the reads, so word from an older one is dropped.
    read: u64,
}

thread_local! {
    static READ: RefCell<Read> = RefCell::default();
}

/// The page's list or its date changed: with Made or Opened picked and every sound read, the files' dates are read
/// for the list's sounds, unless they're known or being read already.
pub(super) fn wanted(app: &AppWindow) {
    if super::HERE.with(|h| *h.borrow()) {
        // The screenshots and tests that work on the window's thread never read files.
        return;
    }
    let (list, on, whole) = PAGE.with(|p| {
        let p = p.borrow();
        (p.list.clone(), p.filters.date_by.from_files(), p.read && p.partial.is_none() && !p.list.is_empty())
    });
    if !on || !whole || list.knows_file_dates() {
        return;
    }
    let read = READ.with(|r| {
        let mut r = r.borrow_mut();
        if r.of.as_ref().is_some_and(|of| of.is_of(&list)) {
            return None;
        }
        r.read += 1;
        r.of = Some(list.sounds_of());
        Some(r.read)
    });
    let Some(read) = read else { return };
    let weak = app.as_weak();
    let started = background::spawn("Sounds file dates", move || {
        let started = std::time::Instant::now();
        let dates = Arc::new(background::on_bulk_pool(|| sound_dates::read(list.sounds())));
        log::info(
            "Sounds",
            &format!(
                "Read when {} sounds' files were made and opened in {} ms.",
                list.len(),
                started.elapsed().as_millis()
            ),
        );
        // The list is let go here, not on the window's thread.
        drop(list);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                heard(&app, read, dates);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start reading when the sounds' files were made: {e}"));
        READ.with(|r| r.borrow_mut().of = None);
    }
}

/// The dates are read: onto the page's list, if it's still of these sounds.
fn heard(app: &AppWindow, read: u64, dates: Arc<FileDates>) {
    let _doing = crate::watchdog::doing("Sounds: file dates read");
    let put = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let fits = READ.with(|r| {
            let r = r.borrow();
            r.read == read && r.of.as_ref().is_some_and(|of| of.is_of(&p.list))
        });
        if fits {
            p.list = Arc::new(p.list.with_file_dates(dates));
        }
        fits
    });
    if put {
        super::refresh(app);
    }
}
