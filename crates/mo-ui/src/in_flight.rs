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
