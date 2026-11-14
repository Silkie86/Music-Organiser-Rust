//! The Curves page's editor card on the screen: its picture, ruler, grid and play line. The card itself (the curve,
//! what the mouse holds, what it shows) is `mo_app::curve_card`, with no screen, as the plugin's Curves tab uses it
//! too; here its picture is worked out on a background thread (`mo_app::curve_picture`) and drawn there too when the
//! processor draws it; the graphics card draws it on the window's thread, as the MIDI page's piano roll is (Solomon,
//! 2026-11-04: what can slow the screens goes off the window's thread). A new curve, size, zoom, grid or look asks
//! for a new picture, and the newest ask wins (rule 12).

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use mo_app::background::{self, Newest};
use mo_app::curve_card::{self, Card, Heard};
use mo_app::curve_hear::{self, Hear};
use mo_app::curve_picture::{self, Colours};
use mo_app::curve_random;
use mo_app::curve_touch::Keys;
use mo_app::log;
use mo_app::presets::Offer;
use mo_formats::bitwig::curve::CurvePoint;
use mo_presets::curves::Curve;
use slint::{ComponentHandle, Image, ModelRc, VecModel};

use crate::canvas::{self, Canvas, Kind, colour};
use crate::gpu;
use crate::{AppWindow, ClipTile, CurveEditorData, EditorDot, RulerMark, Theme};

/// How often the play line moves while the tone plays.
const PLAY_TICK: Duration = Duration::from_millis(33);

/// A picture to work out: what the card asks for, the colours, pixels to a point, and whether the graphics card
/// draws it (else the processor does, on the drawing thread).
struct Ask {
    of: curve_card::Ask,
    colours: Colours,
    scale: f64,
    on_card: bool,
}

/// A worked out picture with the ruler's marks and where the curve ends, ready for the window.
struct Drawn {
    drawing: canvas::Drawing,
    ruler: Vec<RulerMark>,
    end_x: f32,
}

/// The newest picture the drawing thread has sent the window (as the card counts them).
static SENT: AtomicU64 = AtomicU64::new(0);

/// A tone is being made again for the one playing: one at a time, the newest shape after it.
static MAKING: AtomicBool = AtomicBool::new(false);

/// Clears [`MAKING`] when the thread making a tone is done with it, however it ends.
struct Made;

impl Drop for Made {
    fn drop(&mut self) {
        MAKING.store(false, Ordering::Release);
    }
}

/// Counts the times Hear started a tone: one being made again for an earlier start never takes a later one's place.
static ASKED: AtomicU64 = AtomicU64::new(0);

/// The tone playing: its key, and the length in beats and the way of hearing it was made with. The play line and
/// the place carried over go by this (not by what's wanted next), and it changes just as a new tone takes over.
static SOUNDING: std::sync::Mutex<Option<(String, f64, Hear)>> = std::sync::Mutex::new(None);

/// The tone under `key` is about to play: what it was made with.
pub fn sounding(key: &str, length: f64, hear: Hear) {
    *SOUNDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((key.to_owned(), length, hear));
}

/// What the tone playing under `key` was made with, when that's known.
fn sounding_of(key: &str) -> Option<(f64, Hear)> {
    let now = SOUNDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    now.as_ref().filter(|(k, ..)| k == key).map(|(_, length, hear)| (*length, *hear))
}

/// What the picture's steps on the window's thread are called in the freeze lines.
const STEPS: canvas::Steps = canvas::Steps {
    set_up: "Curve editor: set up the GPU painter",
    draw: "Curve editor: draw on the GPU",
    show: "Curve editor: show the CPU picture",
};

thread_local! {
    static STATE: RefCell<Card> = RefCell::default();
    /// Draw here and now, on the processor, instead: for the screenshots, which have no event loop to post to.
    static HERE: Cell<bool> = const { Cell::new(false) };
    static DRAWER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    /// The painter on the graphics card.
    static CARD: RefCell<gpu::Painter<Canvas>> = const { RefCell::new(gpu::Painter::new()) };
    /// Moves the play line while the tone plays, and stops itself when the tone does.
    static PLAY: slint::Timer = slint::Timer::default();
}

/// Does `what` with the card.
fn card<T>(what: impl FnOnce(&mut Card) -> T) -> T {
    STATE.with(|s| what(&mut s.borrow_mut()))
}

/// Wires the editor card to the page, and starts its drawing thread. Once only: the screenshots wire the page too.
pub fn connect(app: &AppWindow) {
    if DRAWER.with(|d| d.borrow().is_some()) {
        return;
    }
    let data = app.global::<CurveEditorData>();
    show_grid(app);
    show_hear(app);
    let weak = app.as_weak();
    data.on_metronome_toggled(move || {
        if let Some(app) = weak.upgrade() {
            set_hear(&app, None, None);
        }
    });
    let weak = app.as_weak();
    data.on_speed_stepped(move |by| {
        if let Some(app) = weak.upgrade() {
            set_hear(&app, None, Some(by));
        }
    });
    let weak = app.as_weak();
    data.on_laid_out(move |width, height| {
        card(|c| c.laid_out(f64::from(width), f64::from(height)));
        if let Some(app) = weak.upgrade() {
            redraw(&app);
            // The dots sit where the picture's room puts them.
            show_touch(&app);
        }
    });
    let weak = app.as_weak();
    data.on_wheel(move |x, by, along| {
        if let Some(app) = weak.upgrade() {
            wheel(&app, f64::from(x), f64::from(by), along);
        }
    });
    data.set_mac(cfg!(target_os = "macos"));
    let weak = app.as_weak();
    data.on_ruler_mouse(move |kind, x, shift| {
        if let Some(app) = weak.upgrade() {
            ruler(&app, kind, f64::from(x), Keys { shift, ..Keys::default() });
        }
    });
    let weak = app.as_weak();
    data.on_strip(move |kind, x, shift, alt, ctrl| {
        if let Some(app) = weak.upgrade() {
            strip(&app, kind, f64::from(x), Keys { shift, alt, ctrl });
        }
    });
    let weak = app.as_weak();
    data.on_mouse(move |kind, x, y, shift, alt, ctrl| {
        if let Some(app) = weak.upgrade() {
            mouse(&app, kind, f64::from(x), f64::from(y), Keys { shift, alt, ctrl });
        }
    });
    let weak = app.as_weak();
    data.on_undo(move || {
        if let Some(app) = weak.upgrade() {
            step_back(&app, true);
        }
    });
    let weak = app.as_weak();
    data.on_redo(move || {
        if let Some(app) = weak.upgrade() {
            step_back(&app, false);
        }
    });
    let weak = app.as_weak();
    data.on_grid_picked(move |chip| {
        card(|c| c.pick_grid(chip));
        if let Some(app) = weak.upgrade() {
            show_grid(&app);
            redraw(&app);
            // The swing marks stand on the grid's lines.
            show_touch(&app);
        }
    });
    let weak = app.as_weak();
    let drawer = Newest::start("Curve editor drawing", move |ask: Ask| {
        let (shown, number) = (ask.of.shown, ask.of.number);
        let drawn = Drawn::new(&ask);
        SENT.fetch_max(number, Ordering::Relaxed);
        let _ = weak.upgrade_in_event_loop(move |app| put_up(&app, shown, Some(number), drawn));
    });
    match drawer {
        Ok(d) => DRAWER.with(|slot| *slot.borrow_mut() = Some(d)),
        Err(e) => log::error("Curves", &format!("Couldn't start drawing the curve editor: {e}")),
    }
}

/// Puts a curve on the card (as it's sent: at the speed its line shows), or none. The same curve again keeps its
/// editor, its zoom and what's been done to it; another starts afresh, the whole of it in sight. `tone_key` is the
/// player's key its tone plays under.
pub fn open(app: &AppWindow, curve: Option<(&Curve, String)>) {
    let data = app.global::<CurveEditorData>();
    let Some((curve, tone_key)) = curve else {
        if card(Card::close) {
            data.set_picture(Image::default());
            data.set_ruler(ModelRc::default());
            data.set_end_x(-1.0);
            stop_play_line(app);
        }
        // Said here, not by `show_touch`: the page is in the middle of showing its card.
        data.set_changed(false);
        show_touch(app);
        return;
    };
    data.set_name(curve.name.as_str().into());
    if !card(|c| c.open(&curve.id, &curve.name, &curve.points, curve.length_beats)) {
        // Its name may be new (renamed): the strip's tile says it too.
        show_touch(app);
        return;
    }
    data.set_picture(Image::default());
    data.set_ruler(ModelRc::default());
    data.set_end_x(-1.0);
    stop_play_line(app);
    // Said here, not by `show_touch`: the page is in the middle of showing its card.
    data.set_changed(false);
    show_touch(app);
    // Its tone may be playing already (picked again while it goes round): the play line follows it again.
    if crate::sounds::player().playing().as_deref() == Some(tone_key.as_str()) {
        follow_tone(app, tone_key, None);
    }
    redraw(app);
}

/// The mouse on the picture (`kind` one of `curve_card::mouse`).
fn mouse(app: &AppWindow, kind: i32, x: f64, y: f64, keys: Keys) {
    if card(|c| c.mouse(kind, x, y, keys)) {
        redraw(app);
    }
    show_touch(app);
}

/// The mouse on the ruler, `x` points from the picture's left.
fn ruler(app: &AppWindow, kind: i32, x: f64, keys: Keys) {
    if card(|c| c.ruler(kind, x, keys)) {
        redraw(app);
    }
    show_touch(app);
}

/// The mouse on the clip strip, `x` points from the picture's left.
fn strip(app: &AppWindow, kind: i32, x: f64, keys: Keys) {
    if card(|c| c.strip(kind, x, keys)) {
        redraw(app);
    }
    show_touch(app);
}

/// What the right-click menu offers for the highlighted part (the whole curve with none); empty with no curve open.
pub fn part_offers() -> Vec<Offer> {
    card(|c| c.part_offers())
}

/// One of the editor's own actions (the right-click menu's, or its key): reverse, flip, steps, keep or forget a
/// clip, cut the time out, highlight all. Whether it did anything.
pub fn act(app: &AppWindow, id: &str) -> bool {
    let done = card(|c| c.act(id));
    // Shown again either way: a drag may have been put back.
    redraw(app);
    show_touch(app);
    done
}

/// Randomise: the curve in the editor becomes a roll from `sources`, or with a highlight only that part is rolled
/// again; one step of undo. False when there's no curve open, or nothing could be rolled.
pub fn randomise(
    app: &AppWindow,
    sources: &[curve_random::Source],
    settings: curve_random::Settings,
    seed: u64,
) -> bool {
    let done = card(|c| c.randomise(sources, settings, seed));
    // Drawn again either way: a drag in hand was given up, even when nothing could be rolled.
    redraw(app);
    show_touch(app);
    done
}

/// A curve from the list is added to the one in the editor: in place of the highlighted part, or at its end. False
/// when there's no curve open or it can't be added.
pub fn add(app: &AppWindow, piece: mo_app::curve_edit::Piece) -> bool {
    let done = card(|c| c.add(piece));
    // Drawn again either way: a drag in hand was given up.
    redraw(app);
    show_touch(app);
    done
}

/// Undo (`back`) or Redo.
fn step_back(app: &AppWindow, back: bool) {
    if card(|c| c.step_back(back)) {
        redraw(app);
    }
    show_touch(app);
}

/// Esc: a drag in hand is put back as it was before it began; else the highlight is dropped. Whether it did either.
pub fn escape(app: &AppWindow) -> bool {
    let done = card(Card::escape);
    if done {
        redraw(app);
        show_touch(app);
    }
    done
}

/// Undo or Redo from the page's keys; whether there was a step to take.
pub fn undo(app: &AppWindow, back: bool) -> bool {
    let can = card(|c| c.can_step(back));
    if can {
        step_back(app, back);
    }
    can
}

/// The curve as it's been edited, when it has been: its points and its length in beats.
pub fn edited() -> Option<(Vec<CurvePoint>, f64)> {
    card(|c| c.edited())
}

/// The id of the curve on the card, when it has edits that aren't saved.
pub fn edited_id() -> Option<String> {
    card(|c| c.edited_id())
}

/// A curve's edits were saved as a new curve: the curve they were made on is as it was found again, on the card or
/// kept for later.
pub fn forget(app: &AppWindow, id: &str) {
    if card(|c| c.forget(id)) {
        redraw(app);
        show_touch(app);
    }
}

/// Shows what lies over the picture as the mouse and the edits have it now: the dots, the lit line, the faint old
/// curve, the pointer, the hint line and its lamps, the read-out, and whether there's something to undo or save.
fn show_touch(app: &AppWindow) {
    let data = app.global::<CurveEditorData>();
    let was_changed = data.get_changed();
    let now = card(|c| c.showing(cfg!(target_os = "macos")));
    let changed = now.changed;
    mo_app::show_curve_card!(data, now, EditorDot, ClipTile);
    data.set_changed(changed);
    if changed != was_changed {
        // The card's own buttons follow: some wait for the edits to be saved.
        crate::presets::curve_edits_changed(app);
    }
}

/// The window's theme has new colours: the picture is drawn again in them.
pub fn look_changed(app: &AppWindow) {
    redraw(app);
}

/// Draws here and now from now on: for the screenshots, which have no event loop to post to.
pub fn draw_here() {
    HERE.set(true);
}

/// The grid's chips as the grid is now.
fn show_grid(app: &AppWindow) {
    let (step, triplet, dotted) = card(|c| c.grid_chips());
    let data = app.global::<CurveEditorData>();
    data.set_grid_step(step);
    data.set_grid_triplet(triplet);
    data.set_grid_dotted(dotted);
}

/// The wheel over the ruler or the picture, `x` points from the picture's left: zooms round the mouse, or moves along
/// with Shift held.
fn wheel(app: &AppWindow, x: f64, by: f64, along: bool) {
    if card(|c| c.wheel(x, by, along)) {
        redraw(app);
        show_play_line(app);
        // The dots, the lit line and the faint old curve move with what's in sight.
        show_touch(app);
    }
}

/// Asks for a picture of the curve as it is now: for the graphics card when there's one to draw on.
fn redraw(app: &AppWindow) {
    let Some(of) = card(Card::ask) else { return };
    let mut ask = Ask { of, colours: colours(app), scale: f64::from(app.window().scale_factor()), on_card: false };
    if HERE.get() {
        let shown = ask.of.shown;
        put_up(app, shown, None, Drawn::new(&ask));
        return;
    }
    ask.on_card = CARD.with(|c| c.borrow().card()).is_some();
    DRAWER.with(|slot| {
        if let Some(drawer) = slot.borrow().as_ref() {
            drawer.ask(ask);
        }
    });
}

/// The picture's colours, as the window's theme has them now.
fn colours(app: &AppWindow) -> Colours {
    let t = app.global::<Theme>();
    Colours {
        well: canvas::rgb(t.get_well()),
        stroke: colour(t.get_stroke()),
        text: colour(t.get_text()),
        muted: colour(t.get_muted()),
        accent: colour(t.get_accent()),
    }
}

impl Drawn {
    fn new(ask: &Ask) -> Drawn {
        let of = &ask.of;
        let scene = curve_picture::picture(&of.shape, of.highlight, of.grid, of.frame, ask.scale, &ask.colours);
        let (marks, end_x) = curve_card::ruler(of.shape.length(), of.frame);
        let ruler = marks.into_iter().map(|(x, text, bar)| RulerMark { x, text: text.into(), bar }).collect();
        Drawn { drawing: canvas::Drawing::new(&scene, ask.scale, ask.on_card), ruler, end_x }
    }
}

/// A picture has been worked out (picture `number`, or here and now): drawn on the card if it's for the card, and put
/// on the page with its ruler, unless another curve has been opened since or a newer picture is on its way already.
fn put_up(app: &AppWindow, shown: u64, number: Option<u64>, drawn: Drawn) {
    if card(|c| c.shown() != shown) || number.is_some_and(|n| n < SENT.load(Ordering::Relaxed)) {
        return;
    }
    let data = app.global::<CurveEditorData>();
    match canvas::show(drawn.drawing, &CARD, &STEPS) {
        Ok(Some(image)) => data.set_picture(image),
        // The card went since this was asked for: drawn again, on the processor.
        Ok(None) => {
            redraw(app);
            return;
        }
        Err(e) => {
            log::error("Curves", &format!("The graphics card couldn't draw the curve, so the processor does: {e}"));
            redraw(app);
            return;
        }
    }
    data.set_ruler(ModelRc::new(VecModel::from(drawn.ruler)));
    data.set_end_x(drawn.end_x);
}

/// The tone under `key` was asked for (Hear, or Space): the play line follows it from when it starts to when it
/// stops. The tone is made on a thread of its own first, so it's waited for a while.
///
/// `heard` is what the tone was made from ([`to_hear`]); none when that isn't known (a tone found playing), and then
/// it's made again from the editor's curve.
pub fn follow_tone(app: &AppWindow, key: String, heard: Option<Heard>) {
    let counts = heard.is_some();
    if !card(|c| c.follow(key, heard)) {
        return;
    }
    if counts {
        ASKED.fetch_add(1, Ordering::AcqRel);
    }
    let weak = app.as_weak();
    PLAY.with(|t| {
        t.start(slint::TimerMode::Repeated, PLAY_TICK, move || {
            if let Some(app) = weak.upgrade() {
                show_play_line(&app);
            }
        });
    });
}

/// Puts the play line where the tone is now; when the tone has stopped (or never came), the line goes and the timer
/// stops. A curve edited (or heard another way) since its tone was made has the tone made again, on a thread of its
/// own, and put in the tone's place at the same beat (Solomon, 14 Nov 2026).
fn show_play_line(app: &AppWindow) {
    let player = crate::sounds::player();
    let now = player.playing();
    let line = card(|c| {
        c.play_line(
            now.as_deref(),
            || player.position().unwrap_or(0.0),
            sounding_of,
            || !MAKING.swap(true, Ordering::AcqRel),
        )
    });
    if let Some((key, (points, length, hear))) = line.again {
        let asked = ASKED.load(Ordering::Acquire);
        let made = background::spawn("Curve tone", move || {
            let _made = Made;
            let tone = curve_hear::tone(&points, length, hear);
            crate::sounds::player().swap_looped(std::sync::Arc::new(tone), &key, |at| {
                // Hear was stopped and started again meanwhile: that tone is newer than this one.
                if ASKED.load(Ordering::Acquire) != asked {
                    return None;
                }
                let mut now = SOUNDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let was = now.as_ref().filter(|(k, ..)| *k == key).map_or(hear, |(_, _, was)| *was);
                *now = Some((key.clone(), length, hear));
                Some(curve_hear::carried(at, was, hear, length))
            });
        });
        if let Err(e) = made {
            MAKING.store(false, Ordering::Release);
            log::error("Curves", &format!("Couldn't start making the curve's tone again: {e}"));
        }
    }
    let data = app.global::<CurveEditorData>();
    match line.at {
        Some(x) => {
            data.set_playing(true);
            data.set_playhead(x);
        }
        None if line.over => stop_play_line(app),
        None => {}
    }
}

/// What Hear plays now: the editor's curve (its points and its length in beats) and how it's heard.
pub fn to_hear() -> Option<Heard> {
    card(|c| c.to_hear())
}

/// The metronome on or off (`None` turns it over), or the speed `by` steps up or down (0 for the usual speed).
/// While the tone plays it's made again with it, at the same beat.
pub fn set_hear(app: &AppWindow, click: Option<bool>, speed: Option<i32>) {
    card(|c| c.set_hear(click, speed));
    show_hear(app);
}

/// How the curve is heard, on the card: the metronome's chip and the speed's words.
fn show_hear(app: &AppWindow) {
    let hear = card(|c| c.hear());
    let data = app.global::<CurveEditorData>();
    data.set_metronome(hear.click);
    data.set_speed_words(curve_hear::speed_words(hear.bpm).into());
    data.set_speed_changed((hear.speed() - curve_hear::BPM_USUAL).abs() > 1e-9);
}

fn stop_play_line(app: &AppWindow) {
    PLAY.with(slint::Timer::stop);
    let data = app.global::<CurveEditorData>();
    data.set_playing(false);
    data.set_playhead(-1.0);
}

/// Shows the play line at `beat` as if the tone played, or puts it away (screenshots: nothing plays).
pub fn show_playing_sample(app: &AppWindow, beat: Option<f64>) {
    let Some(beat) = beat else { return stop_play_line(app) };
    let x = card(|c| c.frame().x(beat));
    let data = app.global::<CurveEditorData>();
    data.set_playing(true);
    data.set_playhead(x as f32);
}

/// The mouse on the picture at a beat and a value, as if it were there (screenshots): `kind` as the page says it (0
/// moved, 1 pressed, 2 let go, 3 double-clicked, 4 left).
pub fn mouse_sample(app: &AppWindow, kind: i32, beat: f64, value: f64, keys: Keys) {
    let (x, y) = card(|c| {
        let frame = c.frame();
        (frame.x(beat), frame.y(value))
    });
    mouse(app, kind, x, y, keys);
}

/// The mouse on the ruler at a beat, as if it were there (screenshots): `kind` as the page says it.
pub fn ruler_sample(app: &AppWindow, kind: i32, beat: f64) {
    let x = card(|c| c.frame().x(beat));
    ruler(app, kind, x, Keys::default());
}

/// What's highlighted, in beats: for the screenshots' own checks.
pub fn highlight_sample() -> Option<(f64, f64)> {
    card(|c| c.highlight()).map(|h| (h.start, h.end))
}

/// A graphics card was handed over (the window opened, or the card was reset): the painter is set up on it, away from
/// the window's thread, and the picture is drawn again on it once it's ready (by the processor until then).
pub fn graphics_ready(app: &AppWindow) {
    gpu::prepare(app, "Curve editor", &CARD, |gpu| Canvas::new(&gpu.device, &gpu.queue, Kind::Picture), redraw);
    redraw(app);
}

/// The card went (the window closed, or the card was reset): the painter goes with it, at once.
pub fn card_gone() {
    CARD.with(|c| c.borrow_mut().gone());
}

/// The card went: the picture made on it isn't shown any more. It's drawn again on the next card, or on the processor.
pub fn graphics_gone(app: &AppWindow) {
    let weak = app.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.global::<CurveEditorData>().set_picture(Image::default());
            redraw(&app);
        }
    });
}
