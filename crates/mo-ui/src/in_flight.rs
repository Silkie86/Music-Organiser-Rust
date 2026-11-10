//! Starts and stops in a DAW still on their way (an audition, a part played in Bitwig or FL Studio, their stops),
//! counted so the app can wait a moment for them as it closes: each start no longer wanted then takes back what it put
//! in, inside its own turn, and each stop goes in, before the app goes (the 6 Nov review's D-fp4).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A link to Bitwig for when the app closes or crashes: every wait `longest` at most, so the app never hangs there.
pub(crate) fn closing_link(longest: Duration) -> mo_dawlink::bitwig::Bitwig {
    mo_dawlink::bitwig::Bitwig {
        connect_timeout: longest,
        timeout: longest,
        change_timeout: longest,
        ..mo_dawlink::bitwig::Bitwig::default()
    }
}

/// What a stop in Bitwig failed for once already, and was put back after (an audition, a part): a second failure lets
/// it go, so a stop that never works can't block what comes next or keep swallowing Esc (the 10 Nov review's D-1).
/// Tied to the item itself, so a later one always gets its own second chance.
pub(crate) struct PutBack<T>(std::sync::Mutex<Option<T>>);

impl<T: PartialEq> PutBack<T> {
    pub(crate) const fn new() -> PutBack<T> {
        PutBack(std::sync::Mutex::new(None))
    }

    /// Whether `item` was put back after a failed stop before. Forgets it either way: it's put back at most once.
    pub(crate) fn failed_before(&self, item: &T) -> bool {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take().is_some_and(|was| was == *item)
    }

    /// Notes `item` as put back after a failed stop.
    pub(crate) fn note(&self, item: T) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(item);
    }
}

/// What the threads on their way hold that only they know of (a track a start made in Bitwig while its preset still
/// loads, an audition or part a stop took out of its slot and is waiting its turn to stop), kept where the app's
/// finish_up finds it: once the moment it waits for them is up, it stops each still held itself, as the app closes,
/// so nothing a long job ahead of them held up stays playing in Bitwig (the 10 Nov review's D-2).
pub(crate) struct Held<T> {
    items: std::sync::Mutex<Vec<(u64, T)>>,
    next: std::sync::atomic::AtomicU64,
}

impl<T> Held<T> {
    pub(crate) const fn new() -> Held<T> {
        Held { items: std::sync::Mutex::new(Vec::new()), next: std::sync::atomic::AtomicU64::new(0) }
    }

    fn items(&self) -> std::sync::MutexGuard<'_, Vec<(u64, T)>> {
        self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Holds `item` until [`Held::release`] with the number this gives.
    pub(crate) fn hold(&self, item: T) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.items().push((id, item));
        id
    }

    /// Gives back the item held under `id`: `None` when the app took it as it closes (and stopped it itself).
    pub(crate) fn release(&self, id: u64) -> Option<T> {
        let mut items = self.items();
        let at = items.iter().position(|(held, _)| *held == id)?;
        Some(items.swap_remove(at).1)
    }

    /// Everything still held, taken, for the app to stop as it closes.
    pub(crate) fn take_all(&self) -> Vec<T> {
        self.items().drain(..).map(|(_, item)| item).collect()
    }

    /// [`Held::take_all`] for the crash hook: never waits for the lock (the thread that crashed may hold it).
    pub(crate) fn try_take_all(&self) -> Vec<T> {
        match self.items.try_lock() {
            Ok(mut items) => items.drain(..).map(|(_, item)| item).collect(),
            Err(std::sync::TryLockError::Poisoned(items)) => {
                items.into_inner().drain(..).map(|(_, item)| item).collect()
            }
            Err(std::sync::TryLockError::WouldBlock) => Vec::new(),
        }
    }
}

/// How many starts are on their way.
static ON_THEIR_WAY: AtomicUsize = AtomicUsize::new(0);

/// One start on its way, counted while this lives (also when the start ends in a panic).
pub(crate) struct Start(());

impl Start {
    pub(crate) fn begin() -> Start {
        ON_THEIR_WAY.fetch_add(1, Ordering::SeqCst);
        Start(())
    }
}

impl Drop for Start {
    fn drop(&mut self) {
        ON_THEIR_WAY.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Waits until no start is on its way, or `longest` has gone by. Whether they all ended.
pub(crate) fn wait_for_all(longest: Duration) -> bool {
    let until = Instant::now() + longest;
    while ON_THEIR_WAY.load(Ordering::SeqCst) > 0 {
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_app_took_as_it_closed_isnt_given_back() {
        let held = Held::new();
        let a = held.hold("A");
        let b = held.hold("B");
        assert_eq!(held.release(a), Some("A"));
        assert_eq!(held.release(a), None, "given back once");
        assert_eq!(held.take_all(), ["B"]);
        assert_eq!(held.release(b), None, "the app took it as it closed");
    }

    #[test]
    fn an_item_is_put_back_once() {
        let tries = PutBack::new();
        assert!(!tries.failed_before(&"A"), "the first failure puts it back");
        tries.note("A");
        assert!(!tries.failed_before(&"B"), "another gets its own chance");
        tries.note("A");
        assert!(tries.failed_before(&"A"), "the second failure lets it go");
        assert!(!tries.failed_before(&"A"), "forgotten after");
    }

    #[test]
    fn waits_for_the_starts_on_their_way() {
        let start = Start::begin();
        assert!(!wait_for_all(Duration::from_millis(30)), "one is on its way");
        let ending = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(start);
        });
        assert!(wait_for_all(Duration::from_secs(5)));
        ending.join().unwrap();
    }
}
