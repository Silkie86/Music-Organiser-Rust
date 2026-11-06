//! Starts and stops in a DAW still on their way (an audition, a part played in Bitwig or FL Studio, their stops),
//! counted so the app can wait a moment for them as it closes: each start no longer wanted then takes back what it put
//! in, inside its own turn, and each stop goes in, before the app goes (the 6 Nov review's D-fp4).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
