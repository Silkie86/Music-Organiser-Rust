//! Sounds like your favourites: with Favourites on and its slider above "Only them", how close each sound is to your
//! favourites of its type is worked out ([`SoundList::like_favourites_in`]) away from the window's thread, on the
//! low-priority pool, and handed to the page's list. Worked out again when the list changes (a heart given or taken
//! away, the use counts in, the sounds read again) or new fingerprints arrive; the newest ask wins.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::sounds::{LikeFavourites, SoundList};
use mo_sounds::index::SoundIndex;
use slint::ComponentHandle;

use super::PAGE;
use crate::AppWindow;

/// The work under way, or done, and for what.
#[derive(Default)]
struct Look {
    /// The list and fingerprints it was (or is being) worked out for: known without keeping them, so an old list is
    /// never kept alive.
    of: Option<(Weak<SoundList>, Weak<SoundIndex>)>,
    /// Counts the asks, so an older answer that comes late is dropped.
    look: u64,
    /// It's being worked out.
    busy: bool,
    worker: Option<Newest<Ask>>,
}

struct Ask {
    look: u64,
    list: Arc<SoundList>,
    index: Arc<SoundIndex>,
}

thread_local! {
    static LOOK: RefCell<Look> = RefCell::default();
}

/// Whether it's being worked out just now, for the words by the slider.
pub(super) fn busy() -> bool {
    LOOK.with(|l| l.borrow().busy)
}

/// Whether `look` was for `list` and `index`.
fn looks_at(look: &Look, list: &Arc<SoundList>, index: &Arc<SoundIndex>) -> bool {
    look.of.as_ref().is_some_and(|(l, i)| {
        std::ptr::eq(l.as_ptr(), Arc::as_ptr(list)) && std::ptr::eq(i.as_ptr(), Arc::as_ptr(index))
    })
}

/// The page's list, its fingerprints or the filters changed: with Favourites on and the slider above "Only them", and
/// every sound read, how close each is to your favourites is worked out for them, unless it already is (or is being).
pub(super) fn wanted(app: &AppWindow) {
    let (list, index, on) = PAGE.with(|p| {
        let p = p.borrow();
        let whole = p.read && p.partial.is_none() && !p.list.is_empty();
        (p.list.clone(), p.index.clone(), whole && p.filters.wants_like_favourites())
    });
    let Some(index) = index.filter(|_| on) else { return };
    if LOOK.with(|l| looks_at(&l.borrow(), &list, &index)) {
        return;
    }
    if super::HERE.with(|h| *h.borrow()) {
        // The screenshots and tests that work on the window's thread: worked out there and then (a few sounds).
        LOOK.with(|l| l.borrow_mut().of = Some((Arc::downgrade(&list), Arc::downgrade(&index))));
        let like = list.like_favourites_in(&index);
        put(app, &list, &index, like);
        return;
    }
    let ask = LOOK.with(|l| {
        let mut l = l.borrow_mut();
        l.look += 1;
        l.of = Some((Arc::downgrade(&list), Arc::downgrade(&index)));
        l.busy = true;
        if l.worker.is_none() {
            let weak = app.as_weak();
            match Newest::start("Sounds like favourites", move |ask: Ask| {
                let like = background::on_pool(|| ask.list.like_favourites_in(&ask.index));
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        heard(&app, ask, like);
                    }
                });
            }) {
                Ok(worker) => l.worker = Some(worker),
                Err(e) => {
                    log::error("Sounds", &format!("Couldn't start finding sounds like your favourites: {e}"));
                    l.busy = false;
                    return None;
                }
            }
        }
        Some(Ask { look: l.look, list: list.clone(), index: index.clone() })
    });
    if let Some(ask) = ask {
        LOOK.with(|l| {
            if let Some(worker) = l.borrow().worker.as_ref() {
                worker.ask(ask);
            }
        });
    }
}

/// An answer, on the window's thread: onto the page's list, if it's the newest ask and the list is still that one.
fn heard(app: &AppWindow, ask: Ask, like: Arc<LikeFavourites>) {
    let _doing = crate::watchdog::doing("Sounds: sounds like your favourites");
    if LOOK.with(|l| l.borrow().look != ask.look) {
        return;
    }
    LOOK.with(|l| l.borrow_mut().busy = false);
    put(app, &ask.list, &ask.index, like);
}

/// Puts `like` (worked out for `list` and `index`) onto the page's list when it's still `list`, and shows it.
fn put(app: &AppWindow, list: &Arc<SoundList>, index: &Arc<SoundIndex>, like: Arc<LikeFavourites>) {
    let put = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        if !Arc::ptr_eq(&p.list, list) {
            return false;
        }
        p.list = Arc::new(p.list.with_like_favourites(like));
        // The new list is the one it was worked out for: not worked out again for it.
        LOOK.with(|l| l.borrow_mut().of = Some((Arc::downgrade(&p.list), Arc::downgrade(index))));
        true
    });
    if put {
        super::refresh(app);
    } else {
        // The list changed meanwhile: worked out for the new one.
        wanted(app);
    }
}
