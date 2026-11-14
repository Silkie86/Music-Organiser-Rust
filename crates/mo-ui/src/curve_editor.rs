//! The Curves page's editor card: its picture, ruler, grid and play line. The picked curve's shape is kept in an
//! editor (`mo_app::curve_edit`), and a background thread works out its picture (`mo_app::curve_picture`) and draws it
//! there too when the processor draws it; the graphics card draws it on the window's thread, as the MIDI page's piano
//! roll is (Solomon, 2026-11-04: what can slow the screens goes off the window's thread). A new curve, size, zoom,
//! grid or look asks for a new picture, and the newest ask wins (rule 12).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mo_app::background::{self, Newest};
use mo_app::curve_edit::{Edit, Editor, Grid, Shape, Span, Step};
use mo_app::curve_hear::{self, Hear};
use mo_app::curve_hold::{self, Hold, Over, Part};
use mo_app::curve_picture::{self, Colours, Frame, View};
use mo_app::curve_random;
use mo_app::curve_strip::{self, OnStrip};
use mo_app::curve_touch::{self, Grab, Keys};
use mo_app::log;
use mo_app::presets::{Offer, curve_act};
use mo_formats::bitwig::curve::CurvePoint;
use mo_presets::curves::Curve;
use slint::{ComponentHandle, Image, ModelRc, VecModel};

use crate::canvas::{self, Canvas, Kind, colour};
use crate::gpu;
use crate::{AppWindow, ClipTile, CurveEditorData, EditorDot, RulerMark, Theme};

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

/// What a press took hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    /// A point or a line.
    Touch(Grab),
    /// A handle of the highlight.
    Part(Hold),
    /// Empty space: a drag from there highlights, a click drops the highlight. With what was highlighted at the
    /// press, for Esc to put back.
    Sweep(Option<Span>),
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
    /// How the tone is heard (its speed, the metronome), kept from curve to curve; and the curve as the tone playing
    /// was made from (its points, its length, how it's heard): when the editor's differs, the tone is made again.
    hear: Hear,
    heard: Option<Heard>,
    /// Draw here and now, on the processor, instead: for the screenshots, which have no event loop to post to.
    here: bool,
    /// Where the mouse is on the picture (none when it isn't), and the keys held.
    mouse: Option<(f64, f64)>,
    keys: Keys,
    /// What a press took hold of, where the press was, and whether it has become a drag.
    grab: Option<(Held, (f64, f64), bool)>,
    /// The curve as it was when the drag began: shown faint behind the new one.
    before: Option<Shape>,
    /// The read-out's words while dragging.
    read_out: String,
    /// The point in hand, as the drag has left it in the list.
    held_dot: Option<usize>,
    /// The curve's name, for the strip's one tile when it has no clips.
    name: String,
    /// Where the mouse is on the clip strip, from the picture's left (none when it isn't there).
    on_strip: Option<f64>,
    /// The strip's tiles as they were last put on it, and as `mo_app::curve_strip` worked them out.
    tiles: Vec<ClipTile>,
    strip: Vec<curve_strip::Tile>,
    /// The drag in hand highlighted the whole curve to work on it (a pill round the whole curve): given up or
    /// changing nothing, it leaves nothing highlighted again.
    lit_whole: bool,
    /// The highlight a click on empty space just dropped, and when: a double-click (which adds a point) begins with
    /// such a click, and puts it back.
    dropped: Option<(Span, Instant)>,
    /// The faint old curve's path, and the frame it was made for: made once a drag, not on every move.
    ghost: Option<(Frame, String)>,
    /// The dots as they were last put on the picture: the same ones aren't put there again.
    dots: Vec<EditorDot>,
    /// The curve on the card as it was opened, before any edit.
    origin: Option<Shape>,
    /// Curves with edits not saved that were left for another: each as it was opened, its editor and what was in
    /// sight, the one left last at the end. Picked again (unchanged where it came from), a curve is as it was left.
    kept: Vec<(String, Shape, Editor, View)>,
}

/// How long after a click a double-click may still follow it.
const DOUBLE_CLICK: Duration = Duration::from_millis(700);

/// How many edited curves are kept while others are looked at; the one left longest ago goes first.
const KEPT: usize = 8;

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
            hear: Hear::default(),
            heard: None,
            here: false,
            mouse: None,
            keys: Keys::default(),
            grab: None,
            before: None,
            read_out: String::new(),
            held_dot: None,
            name: String::new(),
            on_strip: None,
            tiles: Vec::new(),
            strip: Vec::new(),
            lit_whole: false,
            dropped: None,
            ghost: None,
            dots: Vec::new(),
            origin: None,
            kept: Vec::new(),
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

    /// Nothing is in hand any more.
    fn hands_off(&mut self) {
        self.grab = None;
        self.before = None;
        self.ghost = None;
        self.read_out.clear();
    }

    /// Takes the curve off the card. One with edits (or steps to redo) is kept for when it's picked again.
    fn put_away(&mut self) {
        self.hands_off();
        let Some((id, mut editor)) = self.open.take() else { return };
        let Some(origin) = self.origin.take() else { return };
        editor.let_go();
        if editor.changed() || editor.can_undo() || editor.can_redo() {
            self.kept.retain(|(kept, ..)| *kept != id);
            self.kept.push((id, origin, editor, self.view));
            if self.kept.len() > KEPT {
                self.kept.remove(0);
            }
        }
    }
}

/// The newest picture the drawing thread has sent the window (as `State::pictures` counts them).
static SENT: AtomicU64 = AtomicU64::new(0);

/// What a tone is made from: the curve's points, its length in beats, and how it's heard.
type Heard = (Vec<CurvePoint>, f64, Hear);

/// A tone is being made again for the one playing: one at a time, the newest shape after it.
static MAKING: AtomicBool = AtomicBool::new(false);

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
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.width = f64::from(width);
            s.height = f64::from(height);
        });
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
            let had = s.open.is_some();
            s.put_away();
            had
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
    STATE.with(|s| s.borrow_mut().name.clone_from(&curve.name));
    let same = STATE.with(|s| s.borrow().open.as_ref().is_some_and(|(id, _)| *id == curve.id));
    if same {
        // Its name may be new (renamed): the strip's tile says it too.
        show_touch(app);
        return;
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.put_away();
        let origin = Shape::from_curve(&curve.points, curve.length_beats);
        // Left with edits not saved, and still the same curve where it came from: as it was left.
        let kept =
            s.kept.iter().position(|(id, from, ..)| *id == curve.id && *from == origin).map(|at| s.kept.remove(at));
        // (One that has changed where it came from starts afresh: its old edits were made on another shape.)
        s.kept.retain(|(id, ..)| *id != curve.id);
        let (mut editor, view) = match kept {
            Some((_, _, editor, view)) => (editor, view),
            None => {
                let editor = Editor::new(origin.clone());
                let view = View::whole(editor.shape().length());
                (editor, view)
            }
        };
        editor.grid = s.grid;
        s.view = view;
        s.open = Some((curve.id.clone(), editor));
        s.origin = Some(origin);
        s.shown += 1;
        s.tone = None;
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
        follow_tone(app, tone_key, None);
    }
    redraw(app);
}

/// The mouse on the picture: `kind` 0 moved, 1 pressed, 2 let go, 3 double-clicked, 4 left, 5 the press was taken
/// away (the window lost the mouse: what was being dragged goes back where it was). A press on a dot, a line or a
/// handle of the highlight takes hold of it, and once the mouse has gone [`DRAG_STARTS`] it's dragged; a drag from
/// empty space highlights a stretch of time, and a click there drops the highlight; a double-click adds, removes or
/// straightens (`mo_app::curve_touch` and `mo_app::curve_hold` say which).
fn mouse(app: &AppWindow, kind: i32, x: f64, y: f64, keys: Keys) {
    let drawn = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        if kind < 4 {
            s.keys = keys;
            s.mouse = Some((x, y));
        }
        match kind {
            0 => {
                let Some((held, from, moved)) = s.grab.as_mut() else { return false };
                if !*moved && (from.0 - x).hypot(from.1 - y) < DRAG_STARTS {
                    return false;
                }
                if !*moved {
                    *moved = true;
                    // A handle round the whole curve: the whole curve is what's highlighted from here on.
                    s.dropped = None;
                    if let (Held::Part(hold), None) = (&*held, editor.highlight()) {
                        editor.set_highlight(Some(hold.span));
                        s.lit_whole = true;
                    }
                    if !matches!(held, Held::Sweep(_)) {
                        s.before = Some(editor.shape().clone());
                    }
                }
                let length = s.before.as_ref().map_or_else(|| editor.shape().length(), Shape::length);
                match *held {
                    Held::Touch(grab) => {
                        let before = s.before.as_ref().unwrap_or_else(|| editor.shape());
                        let (edit, words) = curve_touch::drag(grab, before, frame, s.grid, keys, x, y);
                        s.read_out = words;
                        editor.drag(&edit);
                        if matches!(grab, Grab::Dot { .. }) {
                            s.held_dot =
                                s.before.as_ref().and_then(|b| curve_touch::held_point(editor.shape(), b, &edit));
                        }
                    }
                    Held::Part(hold) => {
                        let (edit, words) = curve_hold::drag(hold, length, frame, s.grid, keys, x, y);
                        s.read_out = words;
                        editor.drag(&edit);
                    }
                    Held::Sweep(_) => {
                        let span = curve_hold::swept(frame, s.grid, keys, length, from.0, x);
                        s.read_out = curve_hold::swept_words(span);
                        editor.set_highlight(Some(span));
                    }
                }
                true
            }
            1 => {
                let held = match curve_hold::over(editor.shape(), editor.highlight(), frame, x, y) {
                    Over::Part(part) => {
                        Some(Held::Part(curve_hold::hold(editor.shape(), editor.highlight(), part, x, y)))
                    }
                    Over::Dot(_) | Over::Line(_) => curve_touch::grab(editor.shape(), frame, x, y).map(Held::Touch),
                    Over::Inside | Over::Empty => Some(Held::Sweep(editor.highlight())),
                };
                s.lit_whole = false;
                s.grab = held.map(|h| (h, (x, y), false));
                s.held_dot = match held {
                    Some(Held::Touch(Grab::Dot { point, .. })) => Some(point),
                    _ => None,
                };
                false
            }
            2 => {
                let was = s.grab.take();
                s.before = None;
                s.ghost = None;
                s.read_out.clear();
                match was {
                    Some((Held::Sweep(lit), _, moved)) => {
                        // A click on empty space drops the highlight; a drag has left its own.
                        if !moved {
                            editor.set_highlight(None);
                            if let Some(lit) = lit {
                                s.dropped = Some((lit, Instant::now()));
                            }
                        }
                        true
                    }
                    Some((_, _, true)) => {
                        // A pill round the whole curve dragged to no change: nothing is highlighted, as before.
                        if !editor.let_go() && s.lit_whole {
                            editor.set_highlight(None);
                        }
                        s.lit_whole = false;
                        true
                    }
                    _ => false,
                }
            }
            3 => {
                s.grab = None;
                s.before = None;
                s.ghost = None;
                s.read_out.clear();
                // The click it began with dropped the highlight: it's put back.
                let back = s.dropped.take().filter(|(_, when)| when.elapsed() < DOUBLE_CLICK);
                if let (Some((lit, _)), None) = (back, editor.highlight()) {
                    editor.set_highlight(Some(lit));
                }
                // A handle of the highlight is nothing to double-click: no point is added under it.
                let over = curve_hold::over(editor.shape(), editor.highlight(), frame, x, y);
                if editor.highlight().is_some() && matches!(over, Over::Part(_)) {
                    return back.is_some();
                }
                let added = curve_touch::double_click(editor.shape(), frame, s.grid, keys, x, y)
                    .is_some_and(|e| editor.edit(&e));
                added || back.is_some()
            }
            5 => {
                let was = s.grab.take();
                s.before = None;
                s.ghost = None;
                s.read_out.clear();
                s.mouse = None;
                s.keys = Keys::default();
                given_up(editor, was, &mut s.lit_whole)
            }
            _ => {
                // The mouse left; while something is held the drag goes on outside the picture.
                if s.grab.is_none() {
                    s.mouse = None;
                    // No more is heard of the keys from out there: their lamps go out.
                    s.keys = Keys::default();
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

/// Where a drag begun on the clip strip counts as being on the picture, from its top: in the highlight's bar.
const BAR_Y: f64 = 6.0;

/// The mouse on the clip strip (`kind` as [`mouse`] has it), `x` points from the picture's left. A click on a clip
/// highlights its part, and a drag from there moves it as the highlight's bar does; a click on its pin pins it or
/// lets it go; a double-click keeps the highlight as a clip; `kind` 6 is a right-click, which highlights the clip
/// under it for the menu.
fn strip(app: &AppWindow, kind: i32, x: f64, keys: Keys) {
    // A drag begun here goes on as the bar's. The picture isn't told the mouse is on it for longer than that.
    let dragging = STATE.with(|s| s.borrow().grab.is_some());
    if dragging && matches!(kind, 0 | 2 | 5) {
        mouse(app, kind, x, BAR_Y, keys);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.mouse = None;
            s.on_strip = (kind != 5).then_some(x);
        });
        if kind != 0 {
            show_touch(app);
        }
        return;
    }
    let drawn = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        if kind < 4 {
            s.keys = keys;
            s.on_strip = Some(x);
        }
        let on = curve_strip::strip_at(&s.strip, x);
        match (kind, on) {
            (1, OnStrip::Pin(clip)) => {
                let pinned = editor.shape().clips().get(clip).is_some_and(|c| c.pinned);
                editor.edit(&Edit::Pin { clip, pinned: !pinned })
            }
            (1, OnStrip::Clip(clip)) => {
                let Some(span) = editor.shape().clips().get(clip).map(|c| c.span) else { return false };
                editor.set_highlight(Some(span));
                let hold = curve_hold::hold(editor.shape(), Some(span), Part::Bar, x, BAR_Y);
                s.grab = Some((Held::Part(hold), (x, BAR_Y), false));
                s.held_dot = None;
                s.lit_whole = false;
                true
            }
            // A right-click on a clip: its part is the highlight, so the menu is about the clip under the mouse.
            (6, OnStrip::Clip(clip) | OnStrip::Pin(clip)) => {
                let Some(span) = editor.shape().clips().get(clip).map(|c| c.span) else { return false };
                let was = editor.highlight();
                editor.set_highlight(Some(span));
                editor.highlight() != was
            }
            // Two quick clicks on a pin pin and unpin: nothing more.
            (3, OnStrip::Pin(_)) => false,
            (3, _) => {
                // The highlight is kept as a clip, unless it's one already.
                let lit = editor.highlight();
                if lit.is_none() || curve_strip::clip_of(editor.shape(), lit).is_some() {
                    return false;
                }
                let name = curve_strip::part_name(editor.shape());
                editor.edit(&Edit::Remember { name })
            }
            (4 | 5, _) => {
                s.on_strip = None;
                false
            }
            _ => false,
        }
    });
    if drawn {
        redraw(app);
    }
    show_touch(app);
}

/// What the right-click menu offers for the highlighted part (the whole curve with none); empty with no curve open.
pub fn part_offers() -> Vec<Offer> {
    STATE.with(|s| {
        let s = s.borrow();
        let step = s.grid.beats().unwrap_or(0.25);
        s.open.as_ref().map_or_else(Vec::new, |(_, e)| curve_strip::part_offers(e.shape(), e.highlight(), step))
    })
}

/// One of the editor's own actions (the right-click menu's, or its key): reverse, flip, steps, keep or forget a
/// clip, cut the time out, highlight all. Whether it did anything.
pub fn act(app: &AppWindow, id: &str) -> bool {
    let done = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let step = s.grid.beats().unwrap_or(0.25);
        let was = s.grab;
        s.hands_off();
        s.mouse = None;
        let Some((_, editor)) = s.open.as_mut() else { return false };
        // A drag in hand is put back first: the action is done to the curve as it was before it.
        given_up(editor, was, &mut s.lit_whole);
        let lit = editor.highlight();
        match id {
            curve_act::REVERSE => editor.edit(&Edit::Reverse),
            curve_act::FLIP => editor.edit(&Edit::Flip),
            curve_act::STEPS => editor.edit(&Edit::Steps { step }),
            curve_act::KEEP_CLIP => match (lit, curve_strip::clip_of(editor.shape(), lit)) {
                (_, Some(clip)) => editor.edit(&Edit::Forget(clip)),
                (Some(_), None) => {
                    let name = curve_strip::part_name(editor.shape());
                    editor.edit(&Edit::Remember { name })
                }
                (None, None) => false,
            },
            curve_act::CUT_TIME => editor.edit(&Edit::RemoveTime),
            curve_act::HIGHLIGHT_ALL => {
                let whole = editor.shape().whole();
                let was = editor.highlight();
                editor.set_highlight(Some(whole));
                editor.highlight() != was
            }
            _ => false,
        }
    });
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
    let done = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let was = s.grab;
        s.hands_off();
        s.mouse = None;
        let Some((_, editor)) = s.open.as_mut() else { return false };
        given_up(editor, was, &mut s.lit_whole);
        let lit = editor.highlight();
        let Some(rolled) = curve_random::roll(editor.shape(), lit, sources, settings, seed) else { return false };
        let length = rolled.length();
        let another = (length - editor.shape().length()).abs() > 1e-9;
        if !editor.put(rolled, lit) {
            return false;
        }
        // Another length: the whole of it in sight.
        if another {
            s.view = View::whole(length);
        }
        true
    });
    // Drawn again either way: a drag in hand was given up above, even when nothing could be rolled.
    redraw(app);
    show_touch(app);
    done
}

/// A drag in hand is given up (Esc, or the press was taken away): the curve and its highlight are as they were
/// before it. Whether there was a drag to give up.
fn given_up(editor: &mut Editor, was: Option<(Held, (f64, f64), bool)>, lit_whole: &mut bool) -> bool {
    let whole = std::mem::take(lit_whole);
    match was {
        Some((Held::Sweep(lit), _, true)) => {
            editor.set_highlight(lit);
            true
        }
        Some((_, _, true)) => {
            editor.give_up();
            if whole {
                editor.set_highlight(None);
            }
            true
        }
        _ => false,
    }
}

/// Undo (`back`) or Redo.
fn step_back(app: &AppWindow, back: bool) {
    let done = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        s.hands_off();
        let Some((_, e)) = s.open.as_mut() else { return false };
        let was = e.shape().length();
        let done = if back { e.undo() } else { e.redo() };
        // A step that changes the curve's length (a roll, the end mark): the whole of it in sight again.
        let length = e.shape().length();
        if done && (length - was).abs() > 1e-9 {
            s.view = View::whole(length);
        }
        done
    });
    if done {
        redraw(app);
    }
    show_touch(app);
}

/// Esc: a drag in hand is put back as it was before it began; else the highlight is dropped. Whether it did either.
pub fn escape(app: &AppWindow) -> bool {
    let done = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let was = s.grab;
        s.hands_off();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        if given_up(editor, was, &mut s.lit_whole) {
            true
        } else if editor.highlight().is_some() {
            editor.set_highlight(None);
            true
        } else {
            false
        }
    });
    if done {
        redraw(app);
        show_touch(app);
    }
    done
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

/// Whether the curve in the editor is another shape than the one opened. Its clips and pins don't count: they're
/// names for stretches of time, and aren't kept with a saved curve yet.
fn differs(editor: &Editor, origin: Option<&Shape>) -> bool {
    origin.is_none_or(|o| editor.shape().points() != o.points() || editor.shape().length() != o.length())
}

/// The curve as it's been edited, when it has been: its points and its length in beats.
pub fn edited() -> Option<(Vec<CurvePoint>, f64)> {
    STATE.with(|s| {
        let s = s.borrow();
        let (_, editor) = s.open.as_ref()?;
        differs(editor, s.origin.as_ref()).then(|| (editor.shape().curve_points(), editor.shape().length()))
    })
}

/// The id of the curve on the card, when it has edits that aren't saved.
pub fn edited_id() -> Option<String> {
    STATE.with(|s| {
        let s = s.borrow();
        s.open.as_ref().filter(|(_, e)| differs(e, s.origin.as_ref())).map(|(id, _)| id.clone())
    })
}

/// A curve's edits were saved as a new curve: the curve they were made on is as it was found again, on the card or
/// kept for later.
pub fn forget(app: &AppWindow, id: &str) {
    let open = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.kept.retain(|(kept, ..)| kept != id);
        let open = s.open.as_ref().is_some_and(|(open, _)| open == id);
        if open {
            s.hands_off();
            let origin = s.origin.clone();
            if let (Some((_, editor)), Some(origin)) = (s.open.as_mut(), origin) {
                let grid = editor.grid;
                *editor = Editor::new(origin);
                editor.grid = grid;
            }
        }
        open
    });
    if open {
        redraw(app);
        show_touch(app);
    }
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
        let s = &mut *s.borrow_mut();
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_ref() else {
            s.dots.clear();
            s.tiles.clear();
            s.strip.clear();
            data.set_clips(ModelRc::default());
            s.ghost = None;
            data.set_dots(ModelRc::default());
            data.set_hl_shown(false);
            data.set_hot_line("".into());
            data.set_ghost("".into());
            data.set_read_out("".into());
            data.set_can_undo(false);
            data.set_can_redo(false);
            return false;
        };
        let shape = editor.shape();
        let held = s.grab.map(|(held, _, _)| held);
        let hl = curve_hold::frame_of(shape, editor.highlight(), frame);
        // What's lit: the thing held (as the drag has left it: a point that joined a neighbour has another place in
        // the list), else the thing under the mouse.
        let over = match (held, s.mouse) {
            (Some(Held::Touch(Grab::Dot { .. })), _) => s.held_dot.map_or(Over::Empty, Over::Dot),
            (Some(Held::Touch(grab @ Grab::Line { .. })), _) => {
                let before = s.before.as_ref().unwrap_or(shape);
                curve_touch::held_line(shape, before, grab).map_or(Over::Empty, Over::Line)
            }
            // A side dragged past the other is the other side now, and so the top and bottom: the handle by the
            // mouse is the one lit.
            (Some(Held::Part(hold)), Some((x, y))) => Over::Part(match (hold.part, hl) {
                (Part::Left | Part::Right, Some(b)) => {
                    if (x - b.left).abs() <= (x - b.right).abs() {
                        Part::Left
                    } else {
                        Part::Right
                    }
                }
                (Part::Top | Part::Bottom, Some(b)) => {
                    if (y - b.top).abs() <= (y - b.bottom).abs() {
                        Part::Top
                    } else {
                        Part::Bottom
                    }
                }
                (part, _) => part,
            }),
            (Some(Held::Part(hold)), None) => Over::Part(hold.part),
            (Some(Held::Sweep(_)), _) => Over::Empty,
            (None, Some((x, y))) => curve_hold::over_in(shape, hl, frame, x, y),
            _ => Over::Empty,
        };
        let on_show = match (held, over) {
            (Some(_), Over::Dot(point)) => curve_touch::dots_with(shape, frame, Some(point)),
            (Some(_), _) => curve_touch::dots_with(shape, frame, None),
            (None, _) => curve_touch::dots(shape, frame, s.mouse),
        };
        let dots: Vec<EditorDot> = on_show
            .into_iter()
            .map(|d| {
                let hot = over == Over::Dot(d.point);
                EditorDot { x: d.x as f32, y: d.y as f32, hot: hot && held.is_none(), held: hot && held.is_some() }
            })
            .collect();
        if dots != s.dots {
            data.set_dots(ModelRc::new(VecModel::from(dots.clone())));
            s.dots = dots;
        }
        // The highlight's frame and its handles, or the pills round the whole curve.
        match hl {
            Some(b) => {
                data.set_hl_shown(true);
                data.set_hl_whole(b.whole);
                data.set_hl_left(b.left as f32);
                data.set_hl_right(b.right as f32);
                data.set_hl_top(b.top as f32);
                data.set_hl_bottom(b.bottom as f32);
            }
            None => data.set_hl_shown(false),
        }
        data.set_hl_hot(match over {
            Over::Part(Part::Bar) => 1,
            Over::Part(Part::Left) => 2,
            Over::Part(Part::Right) => 3,
            Over::Part(Part::Top) => 4,
            Over::Part(Part::Bottom) => 5,
            Over::Part(Part::Corner { left: true, top: true }) => 6,
            Over::Part(Part::Corner { left: false, top: true }) => 7,
            Over::Part(Part::Corner { left: true, top: false }) => 8,
            Over::Part(Part::Corner { left: false, top: false }) => 9,
            _ => 0,
        });
        data.set_hl_near(s.mouse.is_some() || held.is_some());
        let hot_line = match over {
            Over::Line(line) => match (shape.points().get(line), shape.points().get(line + 1)) {
                (Some(a), Some(b)) => path(frame, a.beat, b.beat, |beat| shape.line_value(line, beat)),
                _ => String::new(),
            },
            _ => String::new(),
        };
        data.set_hot_line(hot_line.into());
        match (&s.before, &s.ghost) {
            (Some(_), Some((made_for, _))) if *made_for == frame => {}
            (Some(old), _) => {
                let ghost = path(frame, 0.0, old.length(), |b| old.value_at(b));
                data.set_ghost(ghost.as_str().into());
                s.ghost = Some((frame, ghost));
            }
            (None, _) => {
                data.set_ghost("".into());
                s.ghost = None;
            }
        }
        data.set_picture_width(frame.width as f32);
        data.set_picture_height(frame.height as f32);
        data.set_pointer(match (held, over) {
            (Some(Held::Touch(Grab::Dot { .. })), _) => 2,
            (_, Over::Part(Part::Bar)) if s.keys.ctrl => 7,
            (Some(_), Over::Part(Part::Bar)) => 2,
            (None, Over::Part(Part::Bar)) | (None, Over::Dot(_)) => 1,
            (_, Over::Part(Part::Left | Part::Right)) => 4,
            (_, Over::Part(Part::Top | Part::Bottom)) | (_, Over::Line(_)) => 3,
            (_, Over::Part(Part::Corner { left, top })) => {
                if left == top {
                    5
                } else {
                    6
                }
            }
            _ => 0,
        });
        let hint = curve_hold::hint(over, cfg!(target_os = "macos"));
        let on = s.mouse.is_some() || held.is_some();
        data.set_hint_name(if on { hint.name } else { "" }.into());
        data.set_hint_does(
            if on { hint.does } else { "Point at the curve: drag its points and lines, or empty space to highlight" }
                .into(),
        );
        data.set_hint_keys(if on { hint.keys } else { "" }.into());
        // The clip strip: its tiles, and its own hint while the mouse is on it.
        let tiles = curve_strip::tiles(shape, editor.highlight(), frame, &s.name);
        let on_strip = s.on_strip.filter(|_| held.is_none()).map(|x| curve_strip::strip_at(&tiles, x));
        s.strip.clone_from(&tiles);
        if let Some(on) = on_strip {
            let (name, does) = curve_strip::hint(on, &tiles);
            data.set_hint_name(name.into());
            data.set_hint_does(does.into());
            data.set_hint_keys("".into());
        }
        let shown: Vec<ClipTile> = tiles
            .iter()
            .map(|t| ClipTile {
                x: t.left as f32,
                width: (t.right - t.left) as f32,
                name: t.name.as_str().into(),
                words: t.words.as_str().into(),
                pinned: t.pinned,
                picked: t.picked,
                lighter: t.lighter,
                hot: t.clip.is_some()
                    && matches!(on_strip, Some(OnStrip::Clip(i) | OnStrip::Pin(i)) if Some(i) == t.clip),
                pin_hot: matches!(on_strip, Some(OnStrip::Pin(i)) if Some(i) == t.clip),
                can_pin: t.clip.is_some() && t.right - t.left >= curve_strip::PIN_FROM,
            })
            .collect();
        if shown != s.tiles {
            data.set_clips(ModelRc::new(VecModel::from(shown.clone())));
            s.tiles = shown;
        }
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
        differs(editor, s.origin.as_ref())
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
        // The dots, the lit line and the faint old curve move with what's in sight.
        show_touch(app);
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
///
/// `heard` is what the tone was made from ([`to_hear`]); none when that isn't known (a tone found playing), and then
/// it's made again from the editor's curve.
pub fn follow_tone(app: &AppWindow, key: String, heard: Option<Heard>) {
    let ours = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.open.is_none() {
            return false;
        }
        s.tone = Some((key, Instant::now(), false));
        s.heard = heard;
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
    let mut again: Option<(String, Hear, Heard)> = None;
    let at = STATE.with(|s| {
        let s = &mut *s.borrow_mut();
        let frame = s.frame();
        let (key, since, seen) = s.tone.as_mut()?;
        if now.as_deref() != Some(key.as_str()) {
            // Not playing: still being made, or stopped (or another sound took its place).
            if *seen || since.elapsed() > TONE_WAIT {
                s.tone = None;
            }
            return None;
        }
        *seen = true;
        // The curve was edited, or is heard another way, since the tone was made: it's made again and put in the
        // tone's place at the same beat, so what's heard follows the editor as it plays (Solomon, 14 Nov 2026).
        let wanted = s.open.as_ref().map(|(_, e)| (e.shape().curve_points(), e.shape().length(), s.hear));
        let was = s.heard.as_ref().map_or(s.hear, |h| h.2);
        if let Some(wanted) = wanted
            && s.heard.as_ref() != Some(&wanted)
            && !MAKING.swap(true, Ordering::AcqRel)
        {
            s.heard = Some(wanted.clone());
            again = Some((key.clone(), was, wanted));
        }
        // The play line goes by the tone playing: the new one takes over where this one is.
        let (length, hear) = s.heard.as_ref().map_or((s.length(), s.hear), |h| (h.1, h.2));
        let hear = if again.is_some() { was } else { hear };
        let beat = curve_hear::beat_at(player.position().unwrap_or(0.0), hear, length);
        let view = frame.view;
        let in_sight = frame.fits() && beat >= view.start - 1e-9 && beat <= view.start + view.span + 1e-9;
        Some(if in_sight { frame.x(beat) as f32 } else { -1.0 })
    });
    if let Some((key, was, (points, length, hear))) = again {
        let made = background::spawn("Curve tone", move || {
            let tone = curve_hear::tone(&points, length, hear);
            crate::sounds::player()
                .swap_looped(std::sync::Arc::new(tone), &key, |at| curve_hear::carried(at, was, hear, length));
            MAKING.store(false, Ordering::Release);
        });
        if let Err(e) = made {
            MAKING.store(false, Ordering::Release);
            log::error("Curves", &format!("Couldn't start making the curve's tone again: {e}"));
        }
    }
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

/// What Hear plays now: the editor's curve (its points and its length in beats) and how it's heard.
pub fn to_hear() -> Option<Heard> {
    STATE.with(|s| {
        let s = s.borrow();
        s.open.as_ref().map(|(_, e)| (e.shape().curve_points(), e.shape().length(), s.hear))
    })
}

/// The metronome on or off (`None` turns it over), or the speed `by` steps up or down (0 for the usual speed).
/// While the tone plays it's made again with it, at the same beat.
pub fn set_hear(app: &AppWindow, click: Option<bool>, speed: Option<i32>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if speed.is_none() {
            s.hear.click = click.unwrap_or(!s.hear.click);
        }
        match speed {
            Some(0) => s.hear.bpm = curve_hear::BPM_USUAL,
            Some(by) => s.hear.bpm = curve_hear::stepped(s.hear.bpm, by),
            None => {}
        }
    });
    show_hear(app);
}

/// How the curve is heard, on the card: the metronome's chip and the speed's words.
fn show_hear(app: &AppWindow) {
    let hear = STATE.with(|s| s.borrow().hear);
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

/// What's highlighted, in beats: for the screenshots' own checks.
pub fn highlight_sample() -> Option<(f64, f64)> {
    STATE.with(|s| s.borrow().open.as_ref().and_then(|(_, e)| e.highlight()).map(|h| (h.start, h.end)))
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
