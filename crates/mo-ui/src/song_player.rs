//! The player bar: a song's bounce (`mo_app::bounces`) playing along the bottom of the window, on every page, through
//! the one player Sounds plays through ([`crate::sounds::player`]). Playing it stops what plays elsewhere (a sound,
//! a MIDI loop, a curve's tone), and a play there pauses it ([`stop_for_another`]): the bar then shows it paused, to
//! play on from where it was.

use std::cell::RefCell;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use mo_app::bounces::{clock, resume_at};
use mo_app::{background, log};
use slint::ComponentHandle;

use crate::{AppWindow, SongPlayer};

/// Counts the plays asked for, so a file that finishes opening after a newer play, a pause or a close stays quiet
/// (newest wins, rule 12).
static ASKED: AtomicU64 = AtomicU64::new(0);

/// The keys of the bar's plays with the player start with this.
const SONG_KEY: &str = "song:";

/// What the bar has: the file, how long it is (0 till known), where it's paused, and the play under way (its key with
/// the player), and whether the player has it yet (it's opened on a thread of its own first).
#[derive(Default)]
struct Bar {
    path: String,
    seconds: f64,
    paused_at: f64,
    key: Option<String>,
    opened: bool,
}

thread_local! {
    static BAR: RefCell<Bar> = RefCell::new(Bar::default());
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Wires the bar's buttons.
pub fn connect(app: &AppWindow) {
    WINDOW.with(|w| *w.borrow_mut() = Some(app.as_weak()));
    let bar = app.global::<SongPlayer>();
    let weak = app.as_weak();
    bar.on_toggle(move || {
        if let Some(app) = weak.upgrade() {
            toggle(&app);
        }
    });
    let weak = app.as_weak();
    bar.on_play(move |path| {
        if let Some(app) = weak.upgrade() {
            let title = app.global::<SongPlayer>().get_title();
            play(&app, &path, &title);
        }
    });
    let weak = app.as_weak();
    bar.on_seek(move |at| {
        if let Some(app) = weak.upgrade() {
            seek(&app, f64::from(at));
        }
    });
    let weak = app.as_weak();
    bar.on_close(move || {
        if let Some(app) = weak.upgrade() {
            pause(&app);
            app.global::<SongPlayer>().set_shown(false);
        }
    });
    let weak = app.as_weak();
    bar.on_tick(move || {
        if let Some(app) = weak.upgrade() {
            tick(&app);
        }
    });
}

/// Plays the bounce at `path`, of the song called `title`, from its start (or on from where it's paused, when it's the
/// one on the bar).
pub fn play(app: &AppWindow, path: &str, title: &str) {
    // What plays elsewhere would be cut off by it, or start over it later.
    crate::sounds::stop_for_another(app);
    crate::midi::stop_for_another(app);
    crate::curve_editor::call_off_tone();
    crate::arrangement_play::stop_for_another(app);
    let from = BAR.with(|b| {
        let mut b = b.borrow_mut();
        if b.path != path {
            *b = Bar { path: path.to_owned(), ..Bar::default() };
        }
        resume_at(b.seconds, b.paused_at)
    });
    let bar = app.global::<SongPlayer>();
    bar.set_shown(true);
    bar.set_playing(true);
    bar.set_path(path.into());
    bar.set_title(title.into());
    bar.set_file(Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into());
    show_place(app, from);
    start(path.to_owned(), from);
}

/// Reads the file on a thread of its own and hands it to the player, unless something newer was asked for meanwhile.
fn start(path: String, from: f64) {
    let ask = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
    let key = format!("{SONG_KEY}{ask}");
    BAR.with(|b| {
        let mut b = b.borrow_mut();
        b.key = Some(key.clone());
        b.opened = false;
    });
    let wanted = move || ASKED.load(Ordering::SeqCst) == ask;
    let started = background::spawn("Song player", move || {
        // How long it is, from the file's header (the stream reads on as it plays).
        let seconds = mo_audio::read(Path::new(&path), 0.001).map_or(0.0, |a| a.file_seconds);
        let read = mo_audio::play::Stream::open(Path::new(&path), from, None, &wanted);
        let went = read.map_err(|e| e.to_string()).and_then(|stream| {
            crate::sounds::player().play_stream_if(stream, key, || if wanted() { Ok(()) } else { Err(String::new()) })
        });
        let _ = slint::invoke_from_event_loop(move || {
            // Overtaken: a newer play, a pause or a close has the bar.
            if !wanted() {
                return;
            }
            match went {
                Ok(()) => {
                    BAR.with(|b| {
                        let mut b = b.borrow_mut();
                        b.seconds = seconds;
                        b.opened = true;
                    });
                    with_window(|app| show_place(app, from));
                }
                Err(why) => {
                    log::warning("Songs", &format!("Couldn't play {path}: {why}"));
                    BAR.with(|b| b.borrow_mut().key = None);
                    with_window(|app| app.global::<SongPlayer>().set_playing(false));
                }
            }
        });
    });
    if let Err(e) = started {
        log::warning("Songs", &format!("Couldn't start playing a bounce: {e}"));
        BAR.with(|b| b.borrow_mut().key = None);
        with_window(|app| app.global::<SongPlayer>().set_playing(false));
    }
}

/// Play or pause.
fn toggle(app: &AppWindow) {
    if app.global::<SongPlayer>().get_playing() {
        pause(app);
    } else {
        let (path, title) = (BAR.with(|b| b.borrow().path.clone()), app.global::<SongPlayer>().get_title());
        if !path.is_empty() {
            play(app, &path, &title);
        }
    }
}

/// Whether the player has the bar's play now.
fn ours(b: &Bar, playing: Option<&str>) -> bool {
    b.opened && b.key.is_some() && playing == b.key.as_deref()
}

/// Stops where it is, to play on from there.
fn pause(app: &AppWindow) {
    let player = crate::sounds::player();
    let playing = player.playing();
    BAR.with(|b| {
        let mut b = b.borrow_mut();
        if ours(&b, playing.as_deref()) {
            b.paused_at = player.position().unwrap_or(b.paused_at);
        }
        b.key = None;
    });
    // A play still opening is let go too.
    ASKED.fetch_add(1, Ordering::SeqCst);
    // Any play of the bar's, even one that opened just as it was let go.
    if playing.is_some_and(|k| k.starts_with(SONG_KEY)) {
        player.stop();
    }
    app.global::<SongPlayer>().set_playing(false);
}

/// Another page plays something: the bar pauses (a play still opening is let go).
pub fn stop_for_another(app: &AppWindow) {
    if app.global::<SongPlayer>().get_playing() {
        pause(app);
    }
}

/// Esc: pauses the bar when it plays. Whether it did.
pub fn stop_on_escape(app: &AppWindow) -> bool {
    let playing = app.global::<SongPlayer>().get_playing();
    if playing {
        pause(app);
    }
    playing
}

/// Jumps to `at` (0 to 1) of the file: playing on from there, or paused there.
fn seek(app: &AppWindow, at: f64) {
    let player = crate::sounds::player();
    let playing = player.playing();
    let (seconds, ours, path) = BAR.with(|b| {
        let mut b = b.borrow_mut();
        let seconds = at.clamp(0.0, 1.0) * b.seconds;
        b.paused_at = seconds;
        (seconds, ours(&b, playing.as_deref()), b.path.clone())
    });
    if ours {
        player.seek(seconds);
    } else if app.global::<SongPlayer>().get_playing() && !path.is_empty() {
        // Still opening: opened again from there.
        start(path, seconds);
    }
    show_place(app, seconds);
}

/// The bar's clock while it plays: where the player is. When the player has something else now (a sound played
/// another way), the bar shows it paused where it was.
fn tick(app: &AppWindow) {
    let player = crate::sounds::player();
    let playing = player.playing();
    let at = BAR.with(|b| {
        let mut b = b.borrow_mut();
        if !b.opened || b.key.is_none() {
            return None;
        }
        if ours(&b, playing.as_deref()) {
            let at = player.position().unwrap_or(b.paused_at);
            b.paused_at = at;
            Some(Ok(at))
        } else {
            Some(Err(b.key.clone().unwrap_or_default()))
        }
    });
    match at {
        Some(Ok(at)) => show_place(app, at),
        Some(Err(key)) => let_go(&key, false),
        None => {}
    }
}

/// The place and the time on the bar.
fn show_place(app: &AppWindow, at: f64) {
    let seconds = BAR.with(|b| b.borrow().seconds);
    let bar = app.global::<SongPlayer>();
    #[allow(clippy::cast_possible_truncation)]
    bar.set_place(if seconds > 0.0 { (at / seconds) as f32 } else { 0.0 });
    bar.set_time(if seconds > 0.0 { format!("{} / {}", clock(at), clock(seconds)) } else { clock(at) }.into());
}

/// The player says the sound under `key` reached its end: the bar shows it paused, at its start again.
pub fn ended(key: &str) {
    let_go(key, true);
}

/// The bar's play under `key` is over: it reached its end (`at_end`), or another sound took its place. The bar shows it
/// paused where the clock last saw it, or at its start when it played to its end.
fn let_go(key: &str, at_end: bool) {
    let ours = BAR.with(|b| {
        let mut b = b.borrow_mut();
        if b.key.as_deref() != Some(key) {
            return None;
        }
        b.key = None;
        // Where the clock last saw it: the player has let it go by now.
        b.paused_at = if at_end && b.seconds <= 0.0 { 0.0 } else { resume_at(b.seconds, b.paused_at) };
        Some(b.paused_at)
    });
    let Some(at) = ours else { return };
    with_window(|app| {
        app.global::<SongPlayer>().set_playing(false);
        show_place(app, at);
    });
}

fn with_window(f: impl FnOnce(&AppWindow)) {
    WINDOW.with(|w| {
        if let Some(app) = w.borrow().as_ref().and_then(slint::Weak::upgrade) {
            f(&app);
        }
    });
}
