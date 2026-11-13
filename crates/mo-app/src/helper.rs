//! The background helper (live-together step 2): a small part of the app that runs on its own, started at login, so
//! the plugins and the full app have one place that's always connected to Bitwig.
//!
//! It's the app's own program started with [`FLAG`] (`music-organiser --helper`): no window, no pages, no sound
//! library in memory. It holds the watch on Bitwig ([`mo_dawlink::bitwig::Watch`]) and answers on its own port on this
//! computer only ([`PORT`]), one JSON line a request, one JSON line an answer:
//!
//! - `{"ask":"hello"}`: which helper this is, whether it has Bitwig, and whether it's paused ([`Hello`]);
//! - `{"ask":"pause","on":true}` stops watching Bitwig (and `false` starts again);
//! - `{"ask":"quit"}` ends it (the launcher asks before it builds, the app when its build is newer).
//!
//! Only one runs at a time: the port is the proof. A second helper finds it taken and stops at once
//! ([`Start::AlreadyRunning`]).
//!
//! Everything here is quiet: the threads run at low priority, and while nothing asks and Bitwig sends nothing, none of
//! them wakes.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mo_dawlink::bitwig::Watch;
use mo_dawlink::bitwig::watch::State;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::log;

/// The helper's port, on this computer only (the Bitwig extension's is 19791).
pub const PORT: u16 = 19792;
/// What the app is started with to run as the helper.
pub const FLAG: &str = "--helper";
/// How long a request may take to arrive once a connection is open, and an answer to come back.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest request line the helper reads: anything longer isn't one of its requests.
const MOST_LINE: u64 = 64 * 1024;

/// What `{"ask":"hello"}` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The helper's version and the commit it was built from ([`crate::version::version`]).
    pub helper: String,
    /// Its process id.
    pub pid: u32,
    /// Whether it has Bitwig, in words ([`bitwig_words`]).
    pub bitwig: String,
    /// Whether watching is paused (from the tray icon).
    pub paused: bool,
}

/// How a helper's start went.
pub enum Start {
    /// This one runs now.
    Started(Arc<Helper>),
    /// Another helper already has the port.
    AlreadyRunning,
}

/// A running helper: the watch on Bitwig, and the thread answering on the port.
pub struct Helper {
    port: u16,
    bitwig_port: u16,
    watch: Mutex<Option<Watch>>,
    paused: AtomicBool,
    quitting: Mutex<bool>,
    quit_now: Condvar,
}

impl Helper {
    /// Starts a helper answering on `port`, watching the Bitwig extension on `bitwig_port`. Only fails when the system
    /// won't start a thread or the port can't be opened for another reason than a helper having it.
    pub fn start(port: u16, bitwig_port: u16) -> io::Result<Start> {
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => return Ok(Start::AlreadyRunning),
            Err(e) => return Err(e),
        };
        let port = listener.local_addr()?.port();
        let helper = Arc::new(Helper {
            port,
            bitwig_port,
            watch: Mutex::new(Some(Watch::start(bitwig_port))),
            paused: AtomicBool::new(false),
            quitting: Mutex::new(false),
            quit_now: Condvar::new(),
        });
        let serving = Arc::clone(&helper);
        crate::background::spawn("Helper: requests", move || serving.serve(listener))?;
        Ok(Start::Started(helper))
    }

    /// The port it answers on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// What it would say to `hello`.
    pub fn hello(&self) -> Hello {
        Hello {
            helper: crate::version::version(),
            pid: std::process::id(),
            bitwig: bitwig_words(self.bitwig().as_ref()),
            paused: self.paused(),
        }
    }

    /// Whether it has Bitwig now; `None` while paused.
    pub fn bitwig(&self) -> Option<State> {
        self.watch.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|w| w.now().state.clone())
    }

    /// Whether watching is paused.
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Stops watching Bitwig (`true`: its connection closes at once) or starts again (`false`).
    pub fn pause(&self, on: bool) {
        let mut watch = self.watch.lock().unwrap_or_else(PoisonError::into_inner);
        if on == self.paused.swap(on, Ordering::Relaxed) {
            return;
        }
        *watch = if on { None } else { Some(Watch::start(self.bitwig_port)) };
        log::info("Helper", if on { "Paused: not watching Bitwig" } else { "Watching Bitwig again" });
    }

    /// Asks it to end: [`Helper::wait`] returns, and the port stops answering.
    pub fn quit(&self) {
        *self.quitting.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.quit_now.notify_all();
        // Wakes the thread waiting for a connection, so it sees it's time to stop.
        let _ = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, self.port).into(), ASK_TIMEOUT);
    }

    /// Whether it's been asked to end.
    pub fn quitting(&self) -> bool {
        *self.quitting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until it's asked to end, at most `most` (`None`: for ever); says whether it was.
    pub fn wait(&self, most: Option<Duration>) -> bool {
        let until = most.map(|m| Instant::now() + m);
        let mut quitting = self.quitting.lock().unwrap_or_else(PoisonError::into_inner);
        while !*quitting {
            match until {
                None => quitting = self.quit_now.wait(quitting).unwrap_or_else(PoisonError::into_inner),
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return false;
                    }
                    quitting = self.quit_now.wait_timeout(quitting, left).unwrap_or_else(PoisonError::into_inner).0;
                }
            }
        }
        true
    }

    fn serve(self: Arc<Self>, listener: TcpListener) {
        for stream in listener.incoming() {
            if self.quitting() {
                break;
            }
            let Ok(stream) = stream else { continue };
            let helper = Arc::clone(&self);
            // One thread a connection: only the app, the plugins and the launcher connect, a few at a time.
            if crate::background::spawn("Helper: one connection", move || helper.answer(stream)).is_err() {
                log::warning("Helper", "Couldn't start a thread to answer a connection");
            }
        }
        // Dropping the watch closes its connection to Bitwig.
        self.watch.lock().unwrap_or_else(PoisonError::into_inner).take();
    }

    /// Answers one connection's requests, a line each, until it closes, goes quiet for [`ASK_TIMEOUT`] or asks to
    /// quit.
    fn answer(&self, stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(ASK_TIMEOUT));
        let _ = stream.set_write_timeout(Some(ASK_TIMEOUT));
        let Ok(mut out) = stream.try_clone() else { return };
        let mut lines = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match (&mut lines).take(MOST_LINE).read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let (answer, quit) = self.reply(line.trim());
            let sent = writeln!(out, "{answer}").and_then(|()| out.flush());
            if quit {
                log::info("Helper", "Asked to quit");
                let _ = out.shutdown(Shutdown::Both);
                self.quit();
                return;
            }
            if sent.is_err() {
                return;
            }
        }
    }

    /// The answer to one request line, and whether the helper ends after sending it.
    fn reply(&self, line: &str) -> (Value, bool) {
        let Ok(request) = serde_json::from_str::<Value>(line) else {
            return (json!({ "error": "not a request (one JSON object a line)" }), false);
        };
        match request.get("ask").and_then(Value::as_str) {
            Some("hello") => (serde_json::to_value(self.hello()).unwrap_or(Value::Null), false),
            Some("pause") => match request.get("on").and_then(Value::as_bool) {
                Some(on) => {
                    self.pause(on);
                    (json!({ "paused": on }), false)
                }
                None => (json!({ "error": "pause needs on: true or false" }), false),
            },
            Some("quit") => (json!({ "quitting": true }), true),
            Some(other) => (json!({ "error": format!("the helper doesn't know \"{other}\"") }), false),
            None => (json!({ "error": "a request needs an ask" }), false),
        }
    }
}

/// Whether the helper has Bitwig, in words for the tray icon and the log ("Bitwig connected").
pub fn bitwig_words(state: Option<&State>) -> String {
    match state {
        None => "Paused".into(),
        Some(State::Connecting) => "Looking for Bitwig".into(),
        Some(State::NoBitwig) => "Bitwig isn't open".into(),
        Some(State::TooOld(what)) => format!("Bitwig has {what}: install the extension again (Settings > DAWs)"),
        Some(State::Refused(why)) => format!("Bitwig said no: {why}"),
        Some(State::Watching) => "Bitwig connected".into(),
    }
}

/// Asks the helper on `port` one thing and returns its answer; an error when no helper answers in time.
pub fn ask(port: u16, request: &Value) -> io::Result<Value> {
    let stream = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), ASK_TIMEOUT)?;
    stream.set_read_timeout(Some(ASK_TIMEOUT))?;
    stream.set_write_timeout(Some(ASK_TIMEOUT))?;
    let mut out = stream.try_clone()?;
    writeln!(out, "{request}")?;
    out.flush()?;
    let mut line = String::new();
    BufReader::new(stream).take(MOST_LINE).read_line(&mut line)?;
    serde_json::from_str(line.trim()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Which helper answers on `port`, if one does.
pub fn hello(port: u16) -> Option<Hello> {
    ask(port, &json!({ "ask": "hello" })).ok().and_then(|v| serde_json::from_value(v).ok())
}

/// Asks the helper on `port` to end; says whether one answered.
pub fn ask_to_quit(port: u16) -> bool {
    ask(port, &json!({ "ask": "quit" })).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port nothing listens on: taken, then let go.
    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    fn started(port: u16) -> Arc<Helper> {
        match Helper::start(port, free_port()).unwrap() {
            Start::Started(h) => h,
            Start::AlreadyRunning => panic!("the port was taken"),
        }
    }

    #[test]
    fn says_hello_and_only_one_runs() {
        let helper = started(0);
        let hello = hello(helper.port()).unwrap();
        assert_eq!(hello.pid, std::process::id());
        assert_eq!(hello.helper, crate::version::version());
        assert!(!hello.paused);
        assert!(matches!(Helper::start(helper.port(), free_port()).unwrap(), Start::AlreadyRunning));
        helper.quit();
    }

    #[test]
    fn says_when_bitwig_isnt_open() {
        let helper = started(0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while helper.bitwig() != Some(State::NoBitwig) {
            assert!(Instant::now() < deadline, "{:?}", helper.bitwig());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(hello(helper.port()).unwrap().bitwig, "Bitwig isn't open");
        helper.quit();
    }

    #[test]
    fn pauses_and_watches_again() {
        let helper = started(0);
        assert_eq!(ask(helper.port(), &json!({ "ask": "pause", "on": true })).unwrap(), json!({ "paused": true }));
        assert!(helper.paused());
        assert_eq!(helper.bitwig(), None);
        assert_eq!(hello(helper.port()).unwrap().bitwig, "Paused");
        ask(helper.port(), &json!({ "ask": "pause", "on": false })).unwrap();
        assert!(!helper.paused());
        assert!(helper.bitwig().is_some());
        assert!(ask(helper.port(), &json!({ "ask": "pause" })).unwrap().get("error").is_some());
        helper.quit();
    }

    #[test]
    fn quits_when_asked_and_lets_the_port_go() {
        let helper = started(0);
        let port = helper.port();
        assert!(ask_to_quit(port));
        assert!(helper.wait(Some(Duration::from_secs(5))));
        // The port is free again once the thread answering it ends.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Start::Started(again)) = Helper::start(port, free_port()) {
                again.quit();
                break;
            }
            assert!(Instant::now() < deadline, "the port stayed taken");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn odd_requests_get_an_error_not_a_hang() {
        let helper = started(0);
        for request in [json!({ "ask": "dance" }), json!({ "nothing": 1 }), json!([1, 2])] {
            let answer = ask(helper.port(), &request).unwrap();
            assert!(answer.get("error").is_some(), "{request}: {answer}");
        }
        // Not JSON at all.
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, helper.port())).unwrap();
        writeln!(stream, "hello there").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        assert!(line.contains("error"));
        assert!(!helper.wait(Some(Duration::from_millis(10))));
        helper.quit();
    }
}
