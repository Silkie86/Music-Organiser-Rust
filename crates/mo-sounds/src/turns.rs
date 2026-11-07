//! One read of the sound library at a time, across the app: several at once were no faster on a slow disk, and they
//! held up the other pages (Solomon's PC, 2026-11-03; review finding B-4). Each read of [`crate::SoundsDb`] takes a
//! turn, and waits while another read has one.
//!
//! - A short read ([`turn`]: the waveforms of the sounds on show, a few hundred bytes each, or a setting) goes before
//!   the long ones waiting.
//! - A long read ([`long_turn`]: every sound, every fingerprint, a gigabyte for a big library) lets the short ones
//!   waiting in between its rows ([`give_way`]), then carries on before any other long read starts: two long reads
//!   never take it in turns on the disk.
//!
//! A turn is only ever held inside a read of the library, which waits on nothing but the disk (and SQLite's own
//! locks, which time out), so no read can wait for one that waits for it. A read inside another read on the same
//! thread takes no turn of its own.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// Who's reading, and who's waiting.
struct Queue {
    /// A read has the turn.
    reading: bool,
    /// Short reads waiting.
    short: usize,
    /// Long reads that gave way, waiting to carry on: they go before a long read that hasn't started.
    giving_way: usize,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { reading: false, short: 0, giving_way: 0 });
/// Rung each time a turn ends or gives way.
static MOVED_ON: Condvar = Condvar::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Short,
    Long,
}

thread_local! {
    /// The turn this thread holds, if any.
    static HELD: Cell<Option<Kind>> = const { Cell::new(None) };
    /// Rows gone through since this thread last looked for a short read that's waiting.
    static ROWS: Cell<usize> = const { Cell::new(0) };
}

/// How many rows a long read goes through between looks for a short read that's waiting ([`give_way`]).
const ROWS_BETWEEN: usize = 64;

/// A read's turn at the sound library: the next read goes once it's dropped. It stays on the thread that took it.
#[must_use]
pub(crate) struct Turn {
    /// Whether this is the thread's own turn, rather than a read inside one.
    own: bool,
    _here: PhantomData<*const ()>,
}

/// Waits for this thread's turn for a short read ([`Turn`]): only for the read that has the turn now.
pub(crate) fn turn() -> Turn {
    take(Kind::Short)
}

/// Waits for this thread's turn for a long read ([`Turn`]): for the read that has the turn now, and the short ones
/// waiting. It gives way to short ones as it goes ([`give_way`]).
pub(crate) fn long_turn() -> Turn {
    take(Kind::Long)
}

fn take(kind: Kind) -> Turn {
    if HELD.get().is_some() {
        return Turn { own: false, _here: PhantomData };
    }
    let mut queue = lock();
    queue = match kind {
        Kind::Short => {
            queue.short += 1;
            let mut queue = wait_while(queue, |q| q.reading);
            queue.short -= 1;
            queue
        }
        Kind::Long => wait_while(queue, |q| q.reading || q.short > 0 || q.giving_way > 0),
    };
    queue.reading = true;
    drop(queue);
    HELD.set(Some(kind));
    ROWS.set(0);
    Turn { own: true, _here: PhantomData }
}

/// For a long read, after each row: every [`ROWS_BETWEEN`] rows, lets the short reads waiting go first, if any, then
/// carries on. Nothing for a short read, or without a turn.
pub(crate) fn give_way() {
    if HELD.get() != Some(Kind::Long) {
        return;
    }
    let rows = ROWS.get() + 1;
    if rows < ROWS_BETWEEN {
        ROWS.set(rows);
        return;
    }
    ROWS.set(0);
    let mut queue = lock();
    if queue.short == 0 {
        return;
    }
    queue.reading = false;
    queue.giving_way += 1;
    MOVED_ON.notify_all();
    let mut queue = wait_while(queue, |q| q.reading || q.short > 0);
    queue.giving_way -= 1;
    queue.reading = true;
}

impl Drop for Turn {
    fn drop(&mut self) {
        if self.own {
            HELD.set(None);
            lock().reading = false;
            MOVED_ON.notify_all();
        }
    }
}

fn lock() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait_while(queue: MutexGuard<'static, Queue>, busy: impl FnMut(&mut Queue) -> bool) -> MutexGuard<'static, Queue> {
    MOVED_ON.wait_while(queue, busy).unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// A read of `kind` on a thread of its own, saying when it has its turn; it ends once `end` is set.
    fn read_beside(kind: Kind, end: &Arc<AtomicBool>) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let (read, end) = (Arc::new(AtomicBool::new(false)), end.clone());
        let reading = read.clone();
        let thread = std::thread::spawn(move || {
            let _turn = take(kind);
            reading.store(true, Ordering::SeqCst);
            while !end.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        (read, thread)
    }

    /// Goes through rows, giving way, until `until` (or a while, when it's `None`).
    fn rows_until(until: Option<&AtomicBool>) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let pause = Instant::now() + Duration::from_millis(100);
        while until.map_or(Instant::now() < pause, |u| !u.load(Ordering::SeqCst)) {
            assert!(Instant::now() < deadline, "the other read never had its turn");
            for _ in 0..ROWS_BETWEEN {
                give_way();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_long_read_lets_short_ones_in_between_its_rows_but_not_another_long_one() {
        let first = long_turn();
        // A read inside a read doesn't wait for itself.
        drop(turn());
        let end = Arc::new(AtomicBool::new(true));
        let (short, short_thread) = read_beside(Kind::Short, &end);
        let long_end = Arc::new(AtomicBool::new(false));
        let (long, long_thread) = read_beside(Kind::Long, &long_end);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!short.load(Ordering::SeqCst) && !long.load(Ordering::SeqCst), "they wait");
        // The short read goes in between rows, while the first still has its turn; the other long one doesn't.
        rows_until(Some(&short));
        short_thread.join().unwrap();
        rows_until(None);
        assert!(!long.load(Ordering::SeqCst), "another long read waits for the first to end");
        drop(first);
        long_end.store(true, Ordering::SeqCst);
        long_thread.join().unwrap();
        assert!(long.load(Ordering::SeqCst));
        // Without a turn, giving way does nothing.
        give_way();
    }
}
