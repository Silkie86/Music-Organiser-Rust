//! The Curves page's editor card: its picture, ruler, grid and play line. The picked curve's shape is kept in an
//! editor (`mo_app::curve_edit`), and a background thread works out its picture (`mo_app::curve_picture`) and draws it
//! there too when the processor draws it; the graphics card draws it on the window's thread, as the MIDI page's piano
//! roll is (Solomon, 2026-11-04: what can slow the screens goes off the window's thread). A new curve, size, zoom,
//! grid or look asks for a new picture, and the newest ask wins (rule 12).

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mo_app::background::Newest;
use mo_app::curve_edit::{Editor, Grid, Shape, Span, Step};
use mo_app::curve_picture::{self, Colours, Frame, View};
use mo_app::curve_touch::{self, Grab, Keys, Under};
use mo_app::log;
use mo_app::presets::{HEAR_BPM, TONE_MOST_BEATS};
use mo_formats::bitwig::curve::CurvePoint;
use mo_presets::curves::Curve;
use slint::{ComponentHandle, Image, ModelRc, VecModel};

use crate::canvas::{self, Canvas, Kind, colour};
use crate::gpu;
use crate::{AppWindow, CurveEditorData, EditorDot, RulerMark, Theme};

/// How much one notch of the wheel zooms: a notch is 60 points, and three of them halve what's in sight.
const NOTCH: f64 = 60.0;
const NOTCHES_TO_HALVE: f64 = 3.0;
/// How much of what's in sight one notch moves along with Shift held.
const NOTCH_MOVES: f64 = 0.125;
/// How often the play line moves while the tone plays, and how long it waits for a tone to be made and start.
const PLAY_TICK: Duration = Duration::from_millis(33);
const TONE_WAIT: Duration = Duration::from_secs(5);

/// How far the mouse must go from where it pressed before it's a drag, in points: a click with a shaky hand moves
/// nothing.
const DRAG_STARTS: f64 = 3.0;
/// How many points of width one step of a lit line or the faint old curve is drawn over.
const PATH_STEP: f64 = 3.0;

/// A picture to work out: of which curve and which picture (both are counted), the shape as it is now, what's
/// highlighted, the grid, the room and what's in sight, the colours, pixels to a point, and whether the card draws it
/// (else the processor does, on the drawing thread).
struct Ask {
    shown: u64,
    number: u64,
    shape: Shape,
    highlight: Option<Span>,
    grid: Grid,
    frame: Frame,
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

struct State {
    /// The curve on the card (by its id as it's sent) and its editor.
    open: Option<(String, Editor)>,
    /// The grid, kept from curve to curve.
    grid: Grid,
    /// What's in sight of the curve.
    view: View,
    /// The picture's room on the page, in points.
    width: f64,
    height: f64,
    /// Counts the curves opened, so a picture of one shown before is dropped; and the pictures asked for.
    shown: u64,
    pictures: u64,
    /// The player's key of the tone the play line follows, since when, and whether it's been heard to play yet.
    tone: Option<(String, Instant, bool)>,
    /// Draw here and now, on the processor, instead: for the screenshots, which have no event loop to post to.
    here: bool,
    /// Where the mouse is on the picture (none when it isn't), and the keys held.
    mouse: Option<(f64, f64)>,
    keys: Keys,
    /// What a press took hold of, where the press was, and whether it has become a drag.
    grab: Option<(Grab, (f64, f64), bool)>,
    /// The curve as it was when the drag began: shown faint behind the new one.
    before: Option<Shape>,
    /// The read-out's words while dragging.
    read_out: String,
}

impl Default for State {
    fn default() -> State {
        State {
            open: None,
            grid: Grid::default(),
            view: View::whole(curve_picture::BAR),
            width: 0.0,
            height: 0.0,
            shown: 0,
            pictures: 0,
            tone: None,
            here: false,
            mouse: None,
            keys: Keys::default(),
            grab: None,
            before: None,
            read_out: String::new(),
        }
    }
}

impl State {
    fn length(&self) -> f64 {
        self.open.as_ref().map_or(curve_picture::BAR, |(_, e)| e.shape().length())
    }

    fn frame(&self) -> Frame {
        Frame { width: self.width, height: self.height, view: self.view.within(self.length()) }
    }
}

/// The newest picture the drawing thread has sent the window (as `State::pictures` counts them).
static SENT: AtomicU64 = AtomicU64::new(0);

/// What the picture's steps on the window's thread are called in the freeze lines.
const STEPS: canvas::Steps = canvas::Steps {
    set_up: "Curve editor: set up the GPU painter",
    draw: "Curve editor: draw on the GPU",
    show: "Curve editor: show the CPU picture",
};

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static DRAWER: RefCell<Option<Newest<Ask>>> = const { RefCell::new(None) };
    /// The painter on the graphics card.
    static CARD: RefCell<gpu::Painter<Canvas>> = const { RefCell::new(gpu::Painter::new()) };
    /// Moves the play line while the tone plays, and stops itself when the tone does.
    static PLAY: slint::Timer = slint::Timer::default();
}

/// Wires the editor card to the page, and starts its drawing thread. Once only: the screenshots wire the page too.
pub fn connect(app: &AppWindow) {
    if DRAWER.with(|d| d.borrow().is_some()) {
        return;
    }
    let data = app.global::<CurveEditorData>();
    show_grid(app);
    let weak = app.as_weak();
    data.on_laid_out(move |width, height| {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.width = f64::from(width);
            s.height = f64::from(height);
        });
        if let Some(app) = weak.upgrade() {
            redraw(&app);
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
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            match usize::try_from(chip).ok() {
                // Triplet and Dotted: one at most, and only with a step.
                Some(4) => {
                    s.grid.triplet = s.grid.step.is_some() && !s.grid.triplet;
                    s.grid.dotted = false;
                }
                Some(5) => {
                    s.grid.dotted = s.grid.step.is_some() && !s.grid.dotted;
                    s.grid.triplet = false;
                }
                Some(6) => s.grid.step = None,
                Some(i) => {
                    if let Some(step) = Step::ALL.get(i) {
                        s.grid.step = Some(*step);
                    }
                }
                None => {}
            }
            let grid = s.grid;
            if let Some((_, editor)) = s.open.as_mut() {
                editor.grid = grid;
            }
        });
        if let Some(app) = weak.upgrade() {
            show_grid(&app);
            redraw(&app);
        }
    });
    let weak = app.as_weak();
    let drawer = Newest::start("Curve editor drawing", move |ask: Ask| {
        let (shown, number) = (ask.shown, ask.number);
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
        let had = STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.shown += 1;
            s.tone = None;
            s.grab = None;
            s.before = None;
            s.read_out.clear();
            s.open.take().is_some()
        });
        if had {
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
    let same = STATE.with(|s| s.borrow().open.as_ref().is_some_and(|(id, _)| *id == curve.id));
    if same {
        return;
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mut editor = Editor::new(Shape::from_curve(&curve.points, curve.length_beats));
        editor.grid = s.grid;
        s.view = View::whole(editor.shape().length());
        s.open = Some((curve.id.clone(), editor));
        s.shown += 1;
        s.tone = None;
        s.grab = None;
        s.before = None;
        s.read_out.clear();
    });
    data.set_picture(Image::default());
    data.set_ruler(ModelRc::default());
    data.set_end_x(-1.0);
    stop_play_line(app);
    // Said here, not by `show_touch`: the page is in the middle of showing its card.
    data.set_changed(false);
    show_touch(app);
    // Its tone may be playing already (picked again while it goes round): the play line follows it again.
    if crate::sounds::player().playing().as_deref() == Some(tone_key.as_str()) {
        follow_tone(app, tone_key);
    }
    redraw(app);
}

/// The mouse on the picture: `kind` 0 moved, 1 pressed, 2 let go, 3 double-clicked, 4 left. A press on a dot or a
/// line takes hold of it, and once the mouse has gone [`DRAG_STARTS`] it's dragged; a double-click adds, removes or
/// straightens (`mo_app::curve_touch` says which).
fn mouse(app: &AppWindow, kind: i32, x: f64, y: f64, keys: Keys) {
    let drawn = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        if kind != 4 {
            s.keys = keys;
            s.mouse = Some((x, y));
        }
        match kind {
            0 => {
                let Some((grab, from, moved)) = s.grab.as_mut() else { return false };
                if !*moved && (from.0 - x).hypot(from.1 - y) < DRAG_STARTS {
                    return false;
                }
                if !*moved {
                    *moved = true;
                    s.before = Some(editor.shape().clone());
                }
                let (edit, words) = curve_touch::drag(*grab, frame, s.grid, keys, x, y);
                s.read_out = words;
                editor.drag(&edit);
                true
            }
            1 => {
                s.grab = curve_touch::grab(editor.shape(), frame, x, y).map(|g| (g, (x, y), false));
                false
            }
            2 => {
                let dragged = s.grab.take().is_some_and(|(_, _, moved)| moved);
                s.before = None;
                s.read_out.clear();
                if dragged {
                    editor.let_go();
                }
                dragged
            }
            3 => {
                s.grab = None;
                s.before = None;
                s.read_out.clear();
                curve_touch::double_click(editor.shape(), frame, s.grid, keys, x, y).is_some_and(|e| editor.edit(&e))
            }
            _ => {
                // The mouse left; while something is held the drag goes on outside the picture.
                if s.grab.is_none() {
                    s.mouse = None;
                }
                false
            }
        }
    });
    if drawn {
        redraw(app);
    }
    show_touch(app);
}

/// Undo (`back`) or Redo.
fn step_back(app: &AppWindow, back: bool) {
    let done = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        s.grab = None;
        s.before = None;
        s.read_out.clear();
        s.open.as_mut().is_some_and(|(_, e)| if back { e.undo() } else { e.redo() })
    });
    if done {
        redraw(app);
    }
    show_touch(app);
}

/// Undo or Redo from the page's keys; whether there was a step to take.
pub fn undo(app: &AppWindow, back: bool) -> bool {
    let can =
        STATE.with(|s| s.borrow().open.as_ref().is_some_and(|(_, e)| if back { e.can_undo() } else { e.can_redo() }));
    if can {
        step_back(app, back);
    }
    can
}

/// The curve as it's been edited, when it has been: its points and its length in beats.
pub fn edited() -> Option<(Vec<CurvePoint>, f64)> {
    STATE.with(|s| {
        let s = s.borrow();
        let (_, editor) = s.open.as_ref()?;
        editor.changed().then(|| (editor.shape().curve_points(), editor.shape().length()))
    })
}

/// A stretch of the curve as path commands in the picture's points: from `from` to `to` beats (what's in sight of
/// it), a step every [`PATH_STEP`] points, each at the value `at` gives.
fn path(frame: Frame, from: f64, to: f64, at: impl Fn(f64) -> f64) -> String {
    let view = frame.view;
    let (from, to) = (from.max(view.start), to.min(view.start + view.span));
    if !frame.fits() || to <= from {
        return String::new();
    }
    let steps = (((frame.x(to) - frame.x(from)) / PATH_STEP).ceil() as usize).clamp(1, 4000);
    let mut out = String::new();
    for i in 0..=steps {
        let beat = from + (to - from) * i as f64 / steps as f64;
        let (x, y) = (frame.x(beat), frame.y(at(beat)));
        out.push_str(&format!("{} {x:.2} {y:.2} ", if i == 0 { "M" } else { "L" }));
    }
    out
}

/// Shows what lies over the picture as the mouse and the edits have it now: the dots, the lit line, the faint old
/// curve, the pointer, the hint line and its lamps, the read-out, and whether there's something to undo or save.
fn show_touch(app: &AppWindow) {
    let data = app.global::<CurveEditorData>();
    let was_changed = data.get_changed();
    let changed = STATE.with(|s| {
        let s = s.borrow();
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_ref() else {
            data.set_dots(ModelRc::default());
            data.set_hot_line("".into());
            data.set_ghost("".into());
            data.set_read_out("".into());
            data.set_can_undo(false);
            data.set_can_redo(false);
            return false;
        };
        let shape = editor.shape();
        let held = s.grab.map(|(grab, _, _)| grab);
        // What's lit: the thing held, else the thing under the mouse.
        let under = match (held, s.mouse) {
            // A held point may have joined a neighbour: the dot nearest the mouse is the one in hand.
            (Some(Grab::Dot { .. }), Some((x, y))) => curve_touch::under(shape, frame, x, y),
            (Some(Grab::Line { line, .. }), _) => Under::Line(line),
            (_, Some((x, y))) => curve_touch::under(shape, frame, x, y),
            _ => Under::Empty,
        };
        let dots: Vec<EditorDot> = curve_touch::dots(shape, frame, s.mouse)
            .into_iter()
            .map(|d| {
                let hot = under == Under::Dot(d.point);
                EditorDot {
                    x: d.x as f32,
                    y: d.y as f32,
                    hot: hot && held.is_none(),
                    held: hot && matches!(held, Some(Grab::Dot { .. })),
                }
            })
            .collect();
        data.set_dots(ModelRc::new(VecModel::from(dots)));
        let hot_line = match under {
            Under::Line(line) => match (shape.points().get(line), shape.points().get(line + 1)) {
                (Some(a), Some(b)) => path(frame, a.beat, b.beat, |beat| shape.line_value(line, beat)),
                _ => String::new(),
            },
            _ => String::new(),
        };
        data.set_hot_line(hot_line.into());
        let ghost =
            s.before.as_ref().map_or_else(String::new, |old| path(frame, 0.0, old.length(), |b| old.value_at(b)));
        data.set_ghost(ghost.into());
        data.set_picture_width(frame.width as f32);
        data.set_picture_height(frame.height as f32);
        data.set_pointer(match (held, under) {
            (Some(Grab::Dot { .. }), _) => 2,
            (Some(Grab::Line { .. }), _) | (None, Under::Line(_)) => 3,
            (None, Under::Dot(_)) => 1,
            _ => 0,
        });
        let hint = curve_touch::hint(under, cfg!(target_os = "macos"));
        let over = s.mouse.is_some() || held.is_some();
        data.set_hint_name(if over { hint.name } else { "" }.into());
        data.set_hint_does(if over { hint.does } else { "Point at the curve: drag its points and lines" }.into());
        data.set_hint_keys(if over { hint.keys } else { "" }.into());
        data.set_shift(s.keys.shift);
        data.set_alt(s.keys.alt);
        data.set_ctrl(s.keys.ctrl);
        data.set_read_out(s.read_out.as_str().into());
        if let Some((x, y)) = s.mouse {
            data.set_read_x(x as f32);
            data.set_read_y(y as f32);
        }
        data.set_can_undo(editor.can_undo());
        data.set_can_redo(editor.can_redo());
        editor.changed()
    });
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
    STATE.with(|s| s.borrow_mut().here = true);
}

/// The grid's chips as the grid is now.
fn show_grid(app: &AppWindow) {
    let grid = STATE.with(|s| s.borrow().grid);
    let data = app.global::<CurveEditorData>();
    let step = grid.step.and_then(|step| Step::ALL.iter().position(|s| *s == step));
    data.set_grid_step(step.map_or(-1, |i| i as i32));
    data.set_grid_triplet(grid.triplet);
    data.set_grid_dotted(grid.dotted && !grid.triplet);
}

/// The wheel over the ruler or the picture, `x` points from the picture's left: zooms round the mouse, or moves along
/// with Shift held.
fn wheel(app: &AppWindow, x: f64, by: f64, along: bool) {
    let changed = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.open.is_none() || !by.is_finite() || by == 0.0 {
            return false;
        }
        let (frame, length) = (s.frame(), s.length());
        if !frame.fits() {
            return false;
        }
        let notches = by / NOTCH;
        let view = if along {
            frame.view.moved(-notches * NOTCH_MOVES * frame.view.span, length)
        } else {
            frame.view.zoomed(frame.beat_at(x), 2f64.powf(notches / NOTCHES_TO_HALVE), length)
        };
        let changed = view != s.view;
        s.view = view;
        changed
    });
    if changed {
        redraw(app);
        show_play_line(app);
    }
}

/// Asks for a picture of the curve as it is now: for the graphics card when there's one to draw on.
fn redraw(app: &AppWindow) {
    let colours = colours(app);
    let scale = f64::from(app.window().scale_factor());
    let ask = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let frame = s.frame();
        frame.fits().then_some(())?;
        let (shape, highlight) = s.open.as_ref().map(|(_, e)| (e.shape().clone(), e.highlight()))?;
        s.pictures += 1;
        let ask = Ask {
            shown: s.shown,
            number: s.pictures,
            shape,
            highlight,
            grid: s.grid,
            frame,
            colours,
            scale,
            on_card: false,
        };
        Some((ask, s.here))
    });
    let Some((mut ask, here)) = ask else { return };
    if here {
        let shown = ask.shown;
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
        let scene = curve_picture::picture(&ask.shape, ask.highlight, ask.grid, ask.frame, ask.scale, &ask.colours);
        let length = ask.shape.length();
        let view = ask.frame.view;
        let end_in_sight = view.start + view.span >= length - 1e-9;
        let end_x = if end_in_sight { ask.frame.x(length) } else { -1.0 };
        // The end mark stands where the curve ends: a number there would only be cut off behind it.
        let ruler = curve_picture::ruler(length, ask.frame)
            .into_iter()
            .map(|t| {
                let text = if (t.x - end_x).abs() < 0.5 { String::new() } else { t.label };
                RulerMark { x: t.x as f32, text: text.into(), bar: t.bar }
            })
            .collect();
        Drawn { drawing: canvas::Drawing::new(&scene, ask.scale, ask.on_card), ruler, end_x: end_x as f32 }
    }
}

/// A picture has been worked out (picture `number`, or here and now): drawn on the card if it's for the card, and put
/// on the page with its ruler, unless another curve has been opened since or a newer picture is on its way already.
fn put_up(app: &AppWindow, shown: u64, number: Option<u64>, drawn: Drawn) {
    if STATE.with(|s| s.borrow().shown != shown) || number.is_some_and(|n| n < SENT.load(Ordering::Relaxed)) {
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
pub fn follow_tone(app: &AppWindow, key: String) {
    let ours = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.open.is_none() {
            return false;
        }
        s.tone = Some((key, Instant::now(), false));
        true
    });
    if !ours {
        return;
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
/// stops.
fn show_play_line(app: &AppWindow) {
    let player = crate::sounds::player();
    let now = player.playing();
    let at = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let frame = s.frame();
        let heard = s.length().min(TONE_MOST_BEATS);
        let (key, since, seen) = s.tone.as_mut()?;
        if now.as_deref() != Some(key.as_str()) {
            // Not playing: still being made, or stopped (or another sound took its place).
            if *seen || since.elapsed() > TONE_WAIT {
                s.tone = None;
            }
            return None;
        }
        *seen = true;
        let beat = (player.position().unwrap_or(0.0) * HEAR_BPM / 60.0).rem_euclid(heard.max(1e-6));
        let view = frame.view;
        let in_sight = frame.fits() && beat >= view.start - 1e-9 && beat <= view.start + view.span + 1e-9;
        Some(if in_sight { frame.x(beat) as f32 } else { -1.0 })
    });
    let data = app.global::<CurveEditorData>();
    match at {
        Some(x) => {
            data.set_playing(true);
            data.set_playhead(x);
        }
        None => {
            if STATE.with(|s| s.borrow().tone.is_none()) {
                stop_play_line(app);
            }
        }
    }
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
    let x = STATE.with(|s| s.borrow().frame().x(beat));
    let data = app.global::<CurveEditorData>();
    data.set_playing(true);
    data.set_playhead(x as f32);
}

/// The mouse on the picture at a beat and a value, as if it were there (screenshots): `kind` as the page says it (0
/// moved, 1 pressed, 2 let go, 3 double-clicked, 4 left).
pub fn mouse_sample(app: &AppWindow, kind: i32, beat: f64, value: f64, keys: Keys) {
    let (x, y) = STATE.with(|s| {
        let frame = s.borrow().frame();
        (frame.x(beat), frame.y(value))
    });
    mouse(app, kind, x, y, keys);
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
