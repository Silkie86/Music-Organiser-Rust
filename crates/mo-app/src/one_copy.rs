//! One copy of the app at a time (the 14 Nov review's DF-1): two copies on one data folder write over each other's
//! settings and song fields, and the helper's icon and the launcher can each start a second one.
//!
//! The open app answers on a port of its own on this computer only ([`PORT`]), one JSON line a request, one JSON line
//! an answer, as the helper does ([`crate::helper`]):
//!
//! - `{"ask":"hello"}`: which build it is, its process and its program ([`Hello`]);
//! - `{"ask":"show"}` brings its window to the front;
//! - `{"ask":"close"}` closes it, as closing its window does.
//!
//! The port is the proof that a copy is open. A copy that starts ([`claim`]) and finds it taken asks who has it:
//!
//! - the same build: it asks that copy to show its window, and stops ([`Claim::Shown`]). When that copy is closing
//!   already (its window is gone, it's writing its last settings), it's waited for instead;
//! - another build: it asks that copy to close, waits for it, and carries on in its place. The copy just started is
//!   the one that's wanted: the launcher starts each new build that way, and a plain "one copy only" would leave
//!   Solomon on the old build while the launcher says the new one is open.
//!
//! When the port's holder doesn't answer as the app, or the other build doesn't close in time, the copy opens anyway
//! and says so in its log ([`Claim::Unguarded`]): never opening would be worse than what the guard is for.
//!
//! The port is one for the whole computer, not one for each Windows user: with two people signed in at once, the
//! second one's start would reach the first one's app. Solomon's PC has one user.
//!
//! As with the helper, a web page in a browser can send the port lines: a connection ends at its first line that
//! isn't a request (an HTTP request's first line never is), and only a few are answered at once.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::helper::{ASK_TIMEOUT, ask};
use crate::log;

/// The open app's port, on this computer only (the Bitwig extension's is 19791, the helper's 19792).
pub const PORT: u16 = 19793;
/// How long a closing copy gets to end: it writes its settings and stops its work first, and waits up to a minute for
/// a plugin move to stop.
pub const CLOSE_WAIT: Duration = Duration::from_secs(80);
/// How long [`OneCopy::let_go`] waits for the answering thread to let go of the port.
const LET_GO_WAIT: Duration = Duration::from_secs(1);
/// The longest request line read: anything longer isn't a request.
const MOST_LINE: u64 = 64 * 1024;
/// The most connections answered at once; more are closed at once.
const MOST_CONNECTIONS: usize = 8;

/// What `{"ask":"hello"}` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The app's version and the commit it was built from ([`crate::version::version`]).
    pub app: String,
    /// Its process id (the helper's icon lets that process come to the front on Windows).
    pub pid: u32,
    /// The program it runs from (the launcher puts each build in a folder of its own). Empty when it can't be told.
    #[serde(default)]
    pub exe: String,
    /// Whether it's closing: its window is gone (or going), and the port is let go once its last files are written.
    #[serde(default)]
    pub closing: bool,
}

/// What another copy, or the helper's icon, asks of the open app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Bring the window to the front.
    Show,
    /// Close, as closing the window does.
    Close,
}

type Handler = Box<dyn Fn(Asked) + Send + Sync>;

/// The open app's hold on the port, kept until the app ends: the port is let go at the very end ([`OneCopy::let_go`]), once its last
/// settings are written, so a copy waiting for it never reads them half-way.
pub struct OneCopy {
    hello: Hello,
    closing: AtomicBool,
    port: u16,
    /// Set by [`OneCopy::let_go`]: the thread that answers stops and lets go of the port; and whether it has.
    going: AtomicBool,
    gone: AtomicBool,
    connections: AtomicUsize,
    /// What the window does when asked, once there is a window; what was asked before then waits.
    handler: Mutex<(Option<Handler>, Vec<Asked>)>,
}

/// How a start's [`claim`] went.
pub enum Claim {
    /// This copy is the open one. `closed` names the build of a copy that was asked to close first.
    Ours { held: Arc<OneCopy>, closed: Option<String> },
    /// A copy of this build was open already and was asked to show its window: this one stops.
    Shown,
    /// No hold, for the reason given: the copy opens anyway.
    Unguarded(String),
}

/// Takes the port for this copy of the app (build `version`, run from `exe`), or finds out who has it: see the
/// module's words. Call it before the window and before anything is read from the data folder. It can wait up to
/// `close_wait` for a closing copy to end. `before_show` is called with the open copy's process id before it's asked
/// to show its window (Windows only lets a window come to the front when the program in front allows it).
pub fn claim(port: u16, version: &str, exe: &str, close_wait: Duration, before_show: impl Fn(u32)) -> Claim {
    let until = Instant::now() + close_wait;
    // The build of a copy that's being waited for, once there is one.
    let mut closing: Option<String> = None;
    let mut asked_to_close = false;
    loop {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => {
                return match OneCopy::start(listener, version, exe) {
                    Ok(held) => Claim::Ours { held, closed: closing.filter(|_| asked_to_close) },
                    Err(e) => Claim::Unguarded(format!("couldn't answer on port {port}: {e}")),
                };
            }
            Err(e) if e.kind() != io::ErrorKind::AddrInUse => {
                return Claim::Unguarded(format!("couldn't open port {port}: {e}"));
            }
            Err(_) => {}
        }
        // Once a copy is closing, only the port is watched: asking it more would only keep it busy.
        if closing.is_none() {
            match hello(port) {
                // Closing by itself (its window was just closed): waited for, whatever its build.
                Some(other) if other.closing => closing = Some(other.app),
                Some(other) if other.app == version && same_program(&other.exe, exe) => {
                    before_show(other.pid);
                    match ask(port, &json!({ "ask": "show" })) {
                        Ok(answer) if answer.get("showing").and_then(Value::as_bool) == Some(true) => {
                            return Claim::Shown;
                        }
                        // It began to close between the two asks.
                        Ok(answer) if answer.get("closing").and_then(Value::as_bool) == Some(true) => {
                            closing = Some(other.app);
                        }
                        _ => {
                            return Claim::Unguarded(
                                "the copy that's open didn't answer when asked to show its window".into(),
                            );
                        }
                    }
                }
                Some(other) => {
                    if ask(port, &json!({ "ask": "close" })).is_err() {
                        return Claim::Unguarded(format!("the copy from another build ({}) didn't answer", other.app));
                    }
                    asked_to_close = true;
                    closing = Some(other.app);
                }
                // Just let go between the two looks: the next try takes it. Otherwise it isn't the app's.
                // A copy at its very last step drops what it's asked and lets go a moment later, so the port gets
                // a few more tries.
                None => {
                    for wait in [0, 50, 100, 150, 200] {
                        std::thread::sleep(Duration::from_millis(wait));
                        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                            return match OneCopy::start(listener, version, exe) {
                                Ok(held) => Claim::Ours { held, closed: None },
                                Err(e) => Claim::Unguarded(format!("couldn't answer on port {port}: {e}")),
                            };
                        }
                    }
                    return Claim::Unguarded(format!(
                        "port {port} is taken by something that doesn't answer as the app"
                    ));
                }
            }
        }
        if Instant::now() >= until {
            let other = closing.unwrap_or_default();
            return Claim::Unguarded(format!("the copy that was closing ({other}) didn't end in time"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether something has `port`: told at once, without asking it anything.
pub fn taken(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err()
}

/// Which app answers on `port`, if one does.
pub fn hello(port: u16) -> Option<Hello> {
    ask(port, &json!({ "ask": "hello" })).ok().and_then(|v| serde_json::from_value(v).ok())
}

/// Asks the app on `port` to bring its window to the front; says whether an app answered that it will.
pub fn ask_to_show(port: u16) -> bool {
    ask(port, &json!({ "ask": "show" })).is_ok_and(|a| a.get("showing").and_then(Value::as_bool) == Some(true))
}

/// Asks the app on `port` to close for `who` (said in its log), as a copy of another build does; says whether an app
/// answered.
pub fn ask_to_close_for(port: u16, who: &str) -> bool {
    ask(port, &json!({ "ask": "close", "for": who })).is_ok()
}

/// Whether two paths name the same program: as Windows compares them (any case), exactly elsewhere. One that can't be
/// told ("") counts as the same.
pub(crate) fn same_program(a: &str, b: &str) -> bool {
    let plain = |p: &str| p.strip_prefix(r"\\?\").unwrap_or(p).to_owned();
    a.is_empty() || b.is_empty() || if cfg!(windows) { plain(a).eq_ignore_ascii_case(&plain(b)) } else { a == b }
}

impl OneCopy {
    fn start(listener: TcpListener, version: &str, exe: &str) -> io::Result<Arc<OneCopy>> {
        let port = listener.local_addr()?.port();
        let held = Arc::new(OneCopy {
            hello: Hello { app: version.to_owned(), pid: std::process::id(), exe: exe.to_owned(), closing: false },
            closing: AtomicBool::new(false),
            port,
            going: AtomicBool::new(false),
            gone: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
            handler: Mutex::new((None, Vec::new())),
        });
        let serving = Arc::clone(&held);
        // At the window's priority, not the background's: a copy that starts takes an answer that doesn't come in
        // time as "not the app", and a busy computer mustn't make it so.
        std::thread::Builder::new().name("One copy: requests".into()).spawn(move || serving.serve(listener))?;
        Ok(held)
    }

    /// The app is closing from now on: its window is gone or going. It no longer says it will show, and a copy that
    /// starts waits for it to end instead. The port itself is let go at the very end ([`OneCopy::let_go`]), once the last
    /// settings are written, or with the process if that's never reached.
    pub fn closing(&self) {
        self.closing.store(true, Ordering::Relaxed);
    }

    /// Lets go of the port now: call it at the very end, once the last settings and the log are written. The port
    /// used to go only with the process, and a process can take a long time to go after its last line: on 15 Nov
    /// 2026 Solomon's still held the port 15 seconds after "Closed" (the graphics card's driver was slow to let go),
    /// and the copy the launcher started next waited two seconds for an answer that couldn't come, then opened
    /// without the guard. Waits a moment for the answering thread to stop; says whether the port is free.
    pub fn let_go(&self) -> bool {
        self.closing();
        self.going.store(true, Ordering::Release);
        // One connection wakes the thread waiting for one, so it sees it's time to stop; then it's only waited for.
        // (Not one after another: one sent after the port is free could reach the next copy.)
        if !self.gone.load(Ordering::Acquire) {
            let _ = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, self.port).into(), LET_GO_WAIT);
        }
        let until = Instant::now() + LET_GO_WAIT;
        while !self.gone.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.gone.load(Ordering::Acquire)
    }

    fn hello(&self) -> Hello {
        Hello { closing: self.closing.load(Ordering::Relaxed), ..self.hello.clone() }
    }

    /// The port it answers on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Says what the window does when asked to show or close. What was asked before now (while the app was still
    /// starting) is done at once, in the order it came.
    pub fn on_asked(&self, handler: impl Fn(Asked) + Send + Sync + 'static) {
        let mut slot = self.handler.lock().unwrap_or_else(PoisonError::into_inner);
        for asked in std::mem::take(&mut slot.1) {
            handler(asked);
        }
        slot.0 = Some(Box::new(handler));
    }

    fn asked(&self, asked: Asked) {
        let mut slot = self.handler.lock().unwrap_or_else(PoisonError::into_inner);
        match &slot.0 {
            Some(handler) => handler(asked),
            None => slot.1.push(asked),
        }
    }

    fn serve(self: Arc<Self>, listener: TcpListener) {
        for stream in listener.incoming() {
            if self.going.load(Ordering::Acquire) {
                break;
            }
            let Ok(stream) = stream else { continue };
            if self.connections.fetch_add(1, Ordering::Relaxed) >= MOST_CONNECTIONS {
                self.connections.fetch_sub(1, Ordering::Relaxed);
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            }
            let held = Arc::clone(&self);
            let answering = std::thread::Builder::new().name("One copy: one connection".into()).spawn(move || {
                held.answer(stream);
                held.connections.fetch_sub(1, Ordering::Relaxed);
            });
            if answering.is_err() {
                self.connections.fetch_sub(1, Ordering::Relaxed);
                log::warning("One copy", "Couldn't start a thread to answer a connection");
            }
        }
        // The port is free from here: the listener goes first, then the word that it has.
        drop(listener);
        self.gone.store(true, Ordering::Release);
    }

    /// Answers one connection's requests, a line each, until it closes, goes quiet for [`ASK_TIMEOUT`] or sends
    /// something that isn't a request.
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
            let request = serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object);
            let Some(request) = request else {
                let _ = writeln!(out, "{}", json!({ "error": "not a request (one JSON object a line)" }));
                let _ = out.shutdown(Shutdown::Both);
                return;
            };
            let answer = match request.get("ask").and_then(Value::as_str) {
                Some("hello") => serde_json::to_value(self.hello()).unwrap_or(Value::Null),
                Some("show") if self.closing.load(Ordering::Relaxed) => json!({ "showing": false, "closing": true }),
                Some("show") => {
                    self.asked(Asked::Show);
                    json!({ "showing": true })
                }
                Some("close") => {
                    // Once is enough: a second ask finds it closing.
                    if !self.closing.swap(true, Ordering::Relaxed) {
                        let who = request.get("for").and_then(Value::as_str).unwrap_or("a copy from another build");
                        log::info("One copy", &format!("Asked to close by {who}"));
                        self.asked(Asked::Close);
                    }
                    json!({ "closing": true })
                }
                Some(other) => json!({ "error": format!("the app doesn't know \"{other}\"") }),
                None => json!({ "error": "a request needs an ask" }),
            };
            if writeln!(out, "{answer}").and_then(|()| out.flush()).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const SOON: Duration = Duration::from_secs(2);

    /// A port nothing listens on: taken, then let go.
    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    /// The first copy, with what it's asked sent on.
    fn open(port: u16, version: &str, exe: &str) -> (Arc<OneCopy>, mpsc::Receiver<Asked>) {
        let Claim::Ours { held, closed: None } = claim(port, version, exe, SOON, |_| {}) else {
            panic!("the port was free")
        };
        let (tell, told) = mpsc::channel();
        let tell = Mutex::new(tell);
        held.on_asked(move |asked| {
            let _ = tell.lock().unwrap().send(asked);
        });
        (held, told)
    }

    #[test]
    fn a_second_copy_of_the_same_build_shows_the_first_and_stops() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        let (held, told) = open(port, "1.0 (abc)", "C:\\builds\\abc\\mo.exe");
        assert_eq!(hello(port).unwrap(), held.hello);
        let pid = Mutex::new(0);
        // The same program, as Windows may write it, counts as the same there.
        let exe = if cfg!(windows) { "\\\\?\\c:\\Builds\\abc\\MO.exe" } else { "C:\\builds\\abc\\mo.exe" };
        assert!(matches!(claim(port, "1.0 (abc)", exe, SOON, |p| *pid.lock().unwrap() = p), Claim::Shown));
        assert_eq!(*pid.lock().unwrap(), std::process::id(), "the open copy may come to the front");
        assert_eq!(told.recv_timeout(SOON), Ok(Asked::Show));
        // The helper's icon asks the same way.
        assert!(ask_to_show(port));
        assert_eq!(told.recv_timeout(SOON), Ok(Asked::Show));
        assert!(told.try_recv().is_err(), "nothing asked it to close");
    }

    /// The launcher's case: a new build starts while an older one is open. The older one is asked to close, and the
    /// new one opens once it has.
    #[test]
    fn a_copy_of_another_build_is_asked_to_close_and_waited_for() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let old = OneCopy::start(listener.try_clone().unwrap(), "1.0 (old)", "C:\\builds\\old\\mo.exe").unwrap();
        let (tell, told) = mpsc::channel();
        let tell = Mutex::new(tell);
        old.on_asked(move |asked| {
            let _ = tell.lock().unwrap().send(asked);
        });
        // The old copy "closes" a moment after it's asked: its process would end, which lets go of the port.
        let closing = std::thread::spawn(move || {
            let asked = told.recv_timeout(SOON);
            std::thread::sleep(Duration::from_millis(300));
            (asked, listener)
        });
        // The stand-in for the old process can't end, so its port stays taken: the new copy says so, having asked.
        let claimed = claim(port, "1.1 (new)", "C:\\builds\\new\\mo.exe", Duration::from_millis(800), |_| {});
        let (asked, _listener) = closing.join().unwrap();
        assert_eq!(asked, Ok(Asked::Close));
        assert!(matches!(&claimed, Claim::Unguarded(why) if why.contains("1.0 (old)") && why.contains("in time")));
        // Asked to close once, and from then on it says so.
        assert!(hello(port).unwrap().closing);
        assert!(!ask_to_show(port));
    }

    #[test]
    fn the_port_let_go_by_a_closing_copy_is_taken() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        // A copy of another build that ends when asked to close: one thread, one request.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let old = std::thread::spawn(move || {
            let mut asks = Vec::new();
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                let mut out = stream.try_clone().unwrap();
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(line.trim()).unwrap();
                let ask = request["ask"].as_str().unwrap().to_owned();
                let answer = match ask.as_str() {
                    "hello" => json!({ "app": "1.0 (old)", "pid": 1, "exe": "" }),
                    _ => json!({ "closing": true }),
                };
                writeln!(out, "{answer}").unwrap();
                asks.push(ask.clone());
                if ask == "close" {
                    break;
                }
            }
            asks
        });
        let claimed = claim(port, "1.1 (new)", "/builds/new/mo", SOON, |_| {});
        assert_eq!(old.join().unwrap(), ["hello", "close"]);
        let Claim::Ours { held, closed } = claimed else { panic!("the port was let go") };
        assert_eq!(closed.as_deref(), Some("1.0 (old)"));
        assert_eq!(hello(port).unwrap().app, "1.1 (new)");
        drop(held);
    }

    /// The app's last step: the port goes before the process does, so the next copy opens at once and guarded even
    /// when the old process takes its time to end.
    #[test]
    fn a_copy_that_let_go_of_the_port_no_longer_holds_up_the_next() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        let (old, told) = open(port, "1.0", "");
        assert!(taken(port));
        assert!(old.let_go(), "the answering thread stopped");
        assert!(!taken(port), "free while the old copy is still there");
        // A second time changes nothing.
        assert!(old.let_go());
        let began = Instant::now();
        let Claim::Ours { held, closed: None } = claim(port, "1.1", "", SOON, |_| panic!("nothing is asked to show"))
        else {
            panic!("the port was let go")
        };
        assert!(began.elapsed() < Duration::from_millis(500), "no wait for an answer");
        assert_eq!(hello(port).unwrap().app, "1.1");
        assert!(told.try_recv().is_err(), "the old copy was asked nothing");
        drop((old, held));
    }

    /// The window was closed and the app is writing its last files, port still held, when it's started again: the
    /// new copy waits for the old one to end instead of asking a window that's gone to show, and then opens.
    #[test]
    fn a_copy_started_while_the_same_build_is_closing_waits_for_it() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let old = OneCopy::start(listener.try_clone().unwrap(), "1.0", "").unwrap();
        let (tell, told) = mpsc::channel();
        let tell = Mutex::new(tell);
        old.on_asked(move |asked| {
            let _ = tell.lock().unwrap().send(asked);
        });
        old.closing();
        let claimed = claim(port, "1.0", "", Duration::from_millis(500), |_| panic!("nothing is asked to show"));
        assert!(matches!(&claimed, Claim::Unguarded(why) if why.contains("didn't end in time")));
        assert!(told.try_recv().is_err(), "it's only waited for");
        drop(listener);
    }

    #[test]
    fn what_was_asked_before_the_window_was_there_is_done_once_it_is() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        assert!(!taken(port));
        let Claim::Ours { held, .. } = claim(port, "1.0", "", SOON, |_| {}) else { panic!("the port was free") };
        assert!(taken(port));
        assert!(ask_to_show(port));
        let (tell, told) = mpsc::channel();
        let tell = Mutex::new(tell);
        held.on_asked(move |asked| {
            let _ = tell.lock().unwrap().send(asked);
        });
        assert_eq!(told.recv_timeout(SOON), Ok(Asked::Show));
    }

    #[test]
    fn something_else_on_the_port_doesnt_stop_the_app_opening() {
        let _one = crate::helper::one_at_a_time();
        // Something that takes connections and never answers.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(hello(port).is_none());
        assert!(!ask_to_show(port));
        let claimed = claim(port, "1.0", "", SOON, |_| {});
        assert!(matches!(&claimed, Claim::Unguarded(why) if why.contains("doesn't answer as the app")));
    }

    #[test]
    fn a_line_that_isnt_a_request_ends_the_connection() {
        let _one = crate::helper::one_at_a_time();
        let port = free_port();
        let (_held, told) = open(port, "1.0", "");
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(SOON)).unwrap();
        writeln!(stream, "POST /close HTTP/1.1").unwrap();
        writeln!(stream, "{}", json!({ "ask": "close" })).unwrap();
        let mut said = String::new();
        let _ = stream.read_to_string(&mut said);
        assert!(said.contains("not a request"), "{said}");
        assert!(told.recv_timeout(Duration::from_millis(200)).is_err(), "the close after it was never read");
    }
}
