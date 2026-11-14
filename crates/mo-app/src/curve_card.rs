//! The curve editor's card with no screen: the curve open in it, what the mouse holds, the grid, what's in sight, how
//! it's heard, and everything the card shows (the dots, the highlight's frame, the hint line, the clip strip, the
//! swing marks, the play line). The app's Curves page and the plugin's Curves tab each keep one [`Card`], tell it
//! what the mouse and the keys did, and copy what it [`Card::showing`] onto their own screen: so both work the same
//! (the plan's step 8: "the same editor in the plugin's Curves tab").

use std::time::{Duration, Instant};

use mo_formats::bitwig::curve::CurvePoint;

use crate::curve_edit::{Edit, Editor, Grid, Shape, Span, Step};
use crate::curve_hear::{self, Hear};
use crate::curve_hold::{self, Hold, Over, Part};
use crate::curve_paint::{self, Stroke};
use crate::curve_picture::{self, Frame, View};
use crate::curve_random;
use crate::curve_strip::{self, OnStrip};
use crate::curve_touch::{self, Grab, Keys};
use crate::presets::{Offer, curve_act};

/// How much one notch of the wheel zooms: a notch is 60 points, and three of them halve what's in sight.
const NOTCH: f64 = 60.0;
const NOTCHES_TO_HALVE: f64 = 3.0;
/// How much of what's in sight one notch moves along with Shift held.
const NOTCH_MOVES: f64 = 0.125;
/// How long the play line waits for a tone to be made and start.
const TONE_WAIT: Duration = Duration::from_secs(5);

/// How far the mouse must go from where it pressed before it's a drag, in points: a click with a shaky hand moves
/// nothing.
const DRAG_STARTS: f64 = 3.0;
/// How many points of width one step of a lit line or the faint old curve is drawn over.
const PATH_STEP: f64 = 3.0;

/// How near the mouse must be to a swing mark to take it, in points either side.
const SWING_REACH: f64 = 8.0;
/// The least room between two swing marks shown, in points.
const SWING_APART: f64 = 28.0;

/// How long after a click a double-click may still follow it.
const DOUBLE_CLICK: Duration = Duration::from_millis(700);

/// How many edited curves are kept while others are looked at; the one left longest ago goes first.
const KEPT: usize = 8;

/// Where a drag begun on the clip strip counts as being on the picture, from its top: in the highlight's bar.
const BAR_Y: f64 = 6.0;

/// What the hint line says with the mouse nowhere on the card.
const HINT_AWAY: &str = "Point at the curve: drag its points and lines, or empty space to highlight";

/// The mouse, as a screen tells the card of it: what it did. (The screens send these as numbers: 0 to 6, in this
/// order.)
pub mod mouse {
    pub const MOVED: i32 = 0;
    pub const PRESSED: i32 = 1;
    pub const LET_GO: i32 = 2;
    pub const DOUBLE_CLICKED: i32 = 3;
    pub const LEFT: i32 = 4;
    /// The press was taken away (the window lost the mouse): what was being dragged goes back where it was.
    pub const TAKEN_AWAY: i32 = 5;
    /// A right-click (the clip strip only).
    pub const RIGHT_CLICKED: i32 = 6;
}

/// What a tone is made from: the curve's points, its length in beats, and how it's heard.
pub type Heard = (Vec<CurvePoint>, f64, Hear);

/// Whether two tones would be made from the same thing, number for number (a number that isn't one is the same as
/// itself here, so a damaged curve isn't made again at every tick).
pub fn same_heard(a: &Heard, b: &Heard) -> bool {
    let bits = |h: &Heard| -> Vec<u64> {
        let mut out = vec![h.1.to_bits(), h.2.bpm.to_bits(), u64::from(h.2.click)];
        out.extend(h.0.iter().flat_map(|p| [p.x.to_bits(), p.y.to_bits(), p.bend.to_bits()]));
        out
    };
    bits(a) == bits(b)
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
    /// A stroke of paint (Ctrl on empty space): steps, or a line by hand. The stroke itself is in the card.
    Paint,
    /// A swing mark on the ruler: every second line of a grid this many beats apart, and the beat it was taken at.
    Swing(f64, f64),
}

/// A point's dot on the picture, in points from its left and top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub x: f32,
    pub y: f32,
    /// The mouse is on it.
    pub hot: bool,
    /// It's in hand.
    pub held: bool,
}

/// A tile of the clip strip as it's shown.
#[derive(Clone, Debug, PartialEq)]
pub struct TileShown {
    pub x: f32,
    pub width: f32,
    pub name: String,
    pub words: String,
    pub pinned: bool,
    pub picked: bool,
    pub lighter: bool,
    /// The mouse is on it; on its pin.
    pub hot: bool,
    pub pin_hot: bool,
    /// It's a clip wide enough to show a pin.
    pub can_pin: bool,
}

/// The mouse pointer over the card (the screens send these as numbers).
pub mod pointer {
    pub const PLAIN: i32 = 0;
    pub const HAND: i32 = 1;
    pub const HOLDING: i32 = 2;
    pub const UP_DOWN: i32 = 3;
    pub const LEFT_RIGHT: i32 = 4;
    pub const SLANT_DOWN: i32 = 5;
    pub const SLANT_UP: i32 = 6;
    pub const COPY: i32 = 7;
    pub const PAINT: i32 = 8;
}

/// Everything that lies over the picture as the mouse and the edits have it now. What is costly to put on a screen
/// (the lists, the faint old curve) is only given when it has changed since the last time it was asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Showing {
    /// A curve is open. With none, the rest is empty: the screen clears its card.
    pub open: bool,
    /// The dots, when they've changed.
    pub dots: Option<Vec<Dot>>,
    /// The highlight's frame and its handles, or the pills round the whole curve.
    pub frame: Option<curve_hold::Box>,
    /// Which handle is lit: 0 none, 1 the bar, 2 left, 3 right, 4 top, 5 bottom, 6 to 9 the corners (top left, top
    /// right, bottom left, bottom right).
    pub handle: i32,
    /// The mouse is on the picture, or something is in hand: the handles show.
    pub near: bool,
    /// The lit line, as path commands in the picture's points; empty with none.
    pub hot_line: String,
    /// The faint old curve behind a drag, when it has changed: as path commands, empty to take it away.
    pub ghost: Option<String>,
    /// The picture's room, in points.
    pub width: f32,
    pub height: f32,
    /// The mouse pointer: one of [`pointer`].
    pub pointer: i32,
    /// The hint line: the name of what's under the mouse, what it does, and what the keys would change.
    pub hint_name: String,
    pub hint_does: String,
    pub hint_keys: String,
    /// The keys held: their lamps.
    pub keys: Keys,
    /// The read-out's words while dragging, and where the mouse is (when it's on the picture).
    pub read_out: String,
    pub mouse: Option<(f32, f32)>,
    /// A swing mark is lit; and the marks, in points from the picture's left, when they've changed.
    pub swing_hot: bool,
    pub swing_marks: Option<Vec<f32>>,
    /// The clip strip's tiles, when they've changed.
    pub tiles: Option<Vec<TileShown>>,
    pub can_undo: bool,
    pub can_redo: bool,
    /// The curve is another shape than the one opened: there's something to save.
    pub changed: bool,
}

/// Copies what a card shows (`$now`: a [`Showing`]) onto a screen's `CurveEditorData` (`$data`): the app's and the
/// plugin's are two Slint types with the same properties, so this is written once for both. `$dot` and `$tile` are
/// the screen's own `EditorDot` and `ClipTile`. Whether there's something to save (`changed`) is left to the
/// screen: its other buttons follow that.
#[macro_export]
macro_rules! show_curve_card {
    ($data:expr, $now:expr, $dot:ident, $tile:ident) => {{
        if !$now.open {
            $data.set_clips(::slint::ModelRc::default());
            $data.set_dots(::slint::ModelRc::default());
            $data.set_swing_marks(::slint::ModelRc::default());
            $data.set_hl_shown(false);
            $data.set_hot_line("".into());
            $data.set_ghost("".into());
            $data.set_read_out("".into());
            $data.set_can_undo(false);
            $data.set_can_redo(false);
        } else {
            if let Some(dots) = $now.dots {
                let dots: Vec<$dot> =
                    dots.into_iter().map(|d| $dot { x: d.x, y: d.y, hot: d.hot, held: d.held }).collect();
                $data.set_dots(::slint::ModelRc::new(::slint::VecModel::from(dots)));
            }
            // The highlight's frame and its handles, or the pills round the whole curve.
            match $now.frame {
                Some(b) => {
                    $data.set_hl_shown(true);
                    $data.set_hl_whole(b.whole);
                    $data.set_hl_left(b.left as f32);
                    $data.set_hl_right(b.right as f32);
                    $data.set_hl_top(b.top as f32);
                    $data.set_hl_bottom(b.bottom as f32);
                }
                None => $data.set_hl_shown(false),
            }
            $data.set_hl_hot($now.handle);
            $data.set_hl_near($now.near);
            $data.set_hot_line($now.hot_line.into());
            if let Some(ghost) = $now.ghost {
                $data.set_ghost(ghost.into());
            }
            $data.set_picture_width($now.width);
            $data.set_picture_height($now.height);
            $data.set_pointer($now.pointer);
            $data.set_hint_name($now.hint_name.into());
            $data.set_hint_does($now.hint_does.into());
            $data.set_hint_keys($now.hint_keys.into());
            $data.set_swing_hot($now.swing_hot);
            if let Some(marks) = $now.swing_marks {
                $data.set_swing_marks(::slint::ModelRc::new(::slint::VecModel::from(marks)));
            }
            if let Some(tiles) = $now.tiles {
                let tiles: Vec<$tile> = tiles
                    .into_iter()
                    .map(|t| $tile {
                        x: t.x,
                        width: t.width,
                        name: t.name.into(),
                        words: t.words.into(),
                        pinned: t.pinned,
                        picked: t.picked,
                        lighter: t.lighter,
                        hot: t.hot,
                        pin_hot: t.pin_hot,
                        can_pin: t.can_pin,
                    })
                    .collect();
                $data.set_clips(::slint::ModelRc::new(::slint::VecModel::from(tiles)));
            }
            $data.set_shift($now.keys.shift);
            $data.set_alt($now.keys.alt);
            $data.set_ctrl($now.keys.ctrl);
            $data.set_read_out($now.read_out.into());
            if let Some((x, y)) = $now.mouse {
                $data.set_read_x(x);
                $data.set_read_y(y);
            }
            $data.set_can_undo($now.can_undo);
            $data.set_can_redo($now.can_redo);
        }
    }};
}

/// A picture of the curve to work out: of which curve and which picture (both are counted), the shape as it is now,
/// what's highlighted, the grid, and the room and what's in sight.
#[derive(Clone, Debug)]
pub struct Ask {
    pub shown: u64,
    pub number: u64,
    pub shape: Shape,
    pub highlight: Option<Span>,
    pub grid: Grid,
    pub frame: Frame,
}

/// Where the play line is, and what to do about the tone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayLine {
    /// The tone is playing: the line is here, in points from the picture's left (-1 when that's out of sight).
    pub at: Option<f32>,
    /// The curve was edited, or is heard another way, since the tone was made: make it again from this, and put it
    /// in the tone's place (under this key) at the same beat.
    pub again: Option<(String, Heard)>,
    /// The tone has stopped (or never came): the line goes, and there's no more to follow.
    pub over: bool,
}

/// The curve editor's card.
pub struct Card {
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
    /// The stroke of paint in hand; whether the mouse is on a swing mark of the ruler; how far the swing in hand
    /// has moved its lines, of a step; and the marks as last shown.
    stroke: Option<Stroke>,
    on_swing: bool,
    swing: f64,
    swing_marks: Option<Vec<f32>>,
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
    /// The strip's tiles as they were last shown.
    tiles: Option<Vec<TileShown>>,
    /// The drag in hand highlighted the whole curve to work on it (a pill round the whole curve): given up or
    /// changing nothing, it leaves nothing highlighted again.
    lit_whole: bool,
    /// The highlight a click on empty space just dropped, and when: a double-click (which adds a point) begins with
    /// such a click, and puts it back.
    dropped: Option<(Span, Instant)>,
    /// The frame the faint old curve was last made for: made once a drag, not on every move. And whether one is
    /// shown at all.
    ghost: Option<Frame>,
    ghost_shown: bool,
    /// The dots as they were last shown: the same ones aren't given again.
    dots: Option<Vec<Dot>>,
    /// The curve on the card as it was opened, before any edit.
    origin: Option<Shape>,
    /// Curves with edits not saved that were left for another: each as it was opened, its editor and what was in
    /// sight, the one left last at the end. Picked again (unchanged where it came from), a curve is as it was left.
    kept: Vec<(String, Shape, Editor, View)>,
}

impl Default for Card {
    fn default() -> Card {
        Card {
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
            stroke: None,
            on_swing: false,
            swing: 0.0,
            swing_marks: None,
            mouse: None,
            keys: Keys::default(),
            grab: None,
            before: None,
            read_out: String::new(),
            held_dot: None,
            name: String::new(),
            on_strip: None,
            tiles: None,
            lit_whole: false,
            dropped: None,
            ghost: None,
            ghost_shown: false,
            dots: None,
            origin: None,
            kept: Vec::new(),
        }
    }
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

/// Whether the curve in the editor is another shape than the one opened. Its clips and pins don't count: they're
/// names for stretches of time, and aren't kept with a saved curve yet.
fn differs(editor: &Editor, origin: Option<&Shape>) -> bool {
    origin.is_none_or(|o| editor.shape().points() != o.points() || editor.shape().length() != o.length())
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

/// The ruler's marks for a curve `length` beats long in `frame` (where from the picture's left, its number, whether
/// it starts a bar), and where the curve ends (-1 when that's out of sight). The end mark stands where the curve
/// ends: a number there would only be cut off behind it, so that mark has none.
pub fn ruler(length: f64, frame: Frame) -> (Vec<(f32, String, bool)>, f32) {
    let view = frame.view;
    let end_in_sight = view.start + view.span >= length - 1e-9;
    let end_x = if end_in_sight { frame.x(length) } else { -1.0 };
    let marks = curve_picture::ruler(length, frame)
        .into_iter()
        .map(|t| {
            let text = if (t.x - end_x).abs() < 0.5 { String::new() } else { t.label };
            (t.x as f32, text, t.bar)
        })
        .collect();
    (marks, end_x as f32)
}

impl Card {
    /// The open curve's length in beats (a bar with none).
    pub fn length(&self) -> f64 {
        self.open.as_ref().map_or(curve_picture::BAR, |(_, e)| e.shape().length())
    }

    /// The picture's room and what's in sight.
    pub fn frame(&self) -> Frame {
        Frame { width: self.width, height: self.height, view: self.view.within(self.length()) }
    }

    /// Whether a curve is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The count of curves opened: a picture asked for with another count is of a curve shown before.
    pub fn shown(&self) -> u64 {
        self.shown
    }

    /// What's highlighted, in beats.
    pub fn highlight(&self) -> Option<Span> {
        self.open.as_ref().and_then(|(_, e)| e.highlight())
    }

    /// Whether the mouse holds something (a press not let go yet).
    pub fn in_hand(&self) -> bool {
        self.grab.is_some()
    }

    /// The picture has this much room now, in points.
    pub fn laid_out(&mut self, width: f64, height: f64) {
        self.width = width;
        self.height = height;
    }

    /// Nothing is in hand any more.
    fn hands_off(&mut self) {
        self.grab = None;
        self.before = None;
        self.ghost = None;
        self.stroke = None;
        self.swing = 0.0;
        self.read_out.clear();
    }

    /// Where the swing marks stand on the ruler, in points from the picture's left: on every second grid line of
    /// the highlight (or the whole curve), moved along with a swing in hand.
    fn swing_xs(&self) -> Vec<f32> {
        let Some((_, editor)) = self.open.as_ref() else { return Vec::new() };
        let frame = self.frame();
        let step = match self.grab {
            Some((Held::Swing(step, _), _, _)) => Some(step),
            _ => self.grid.beats(),
        };
        let Some(step) = step else { return Vec::new() };
        let span = editor.highlight().unwrap_or(editor.shape().whole());
        let view = frame.view;
        curve_paint::swing_marks(step, span.start, span.end, frame, SWING_APART)
            .into_iter()
            .map(|beat| beat + self.swing * step)
            .filter(|beat| *beat >= view.start - 1e-9 && *beat <= view.start + view.span + 1e-9)
            .map(|beat| frame.x(beat) as f32)
            .collect()
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

    /// Takes the curve off the card; whether there was one.
    pub fn close(&mut self) -> bool {
        self.shown += 1;
        self.tone = None;
        let had = self.open.is_some();
        self.put_away();
        had
    }

    /// Puts a curve on the card (as it's sent: at the speed its line shows). The same curve again keeps its editor,
    /// its zoom and what's been done to it (and only takes its name, which may be new): false then. Another starts
    /// afresh, the whole of it in sight, unless it was left with edits not saved and is still the same curve where
    /// it came from: then it's as it was left.
    pub fn open(&mut self, id: &str, name: &str, points: &[CurvePoint], length_beats: f64) -> bool {
        name.clone_into(&mut self.name);
        if self.open.as_ref().is_some_and(|(open, _)| open == id) {
            return false;
        }
        self.put_away();
        let origin = Shape::from_curve(points, length_beats);
        let kept = self.kept.iter().position(|(k, from, ..)| k == id && *from == origin).map(|at| self.kept.remove(at));
        // (One that has changed where it came from starts afresh: its old edits were made on another shape.)
        self.kept.retain(|(k, ..)| k != id);
        let (mut editor, view) = match kept {
            Some((_, _, editor, view)) => (editor, view),
            None => {
                let editor = Editor::new(origin.clone());
                let view = View::whole(editor.shape().length());
                (editor, view)
            }
        };
        editor.grid = self.grid;
        self.view = view;
        self.open = Some((id.to_owned(), editor));
        self.origin = Some(origin);
        self.shown += 1;
        self.tone = None;
        true
    }

    /// The grid.
    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The grid's chips as the grid is now: which step's chip is lit (its place in [`Step::ALL`], -1 for Off), and
    /// whether Triplet and Dotted are.
    pub fn grid_chips(&self) -> (i32, bool, bool) {
        let step = self.grid.step.and_then(|step| Step::ALL.iter().position(|s| *s == step));
        (step.map_or(-1, |i| i as i32), self.grid.triplet, self.grid.dotted && !self.grid.triplet)
    }

    /// A grid chip was clicked: a step's (its place in [`Step::ALL`]), 4 Triplet, 5 Dotted, 6 Off.
    pub fn pick_grid(&mut self, chip: i32) {
        match usize::try_from(chip).ok() {
            // Triplet and Dotted: one at most, and only with a step.
            Some(4) => {
                self.grid.triplet = self.grid.step.is_some() && !self.grid.triplet;
                self.grid.dotted = false;
            }
            Some(5) => {
                self.grid.dotted = self.grid.step.is_some() && !self.grid.dotted;
                self.grid.triplet = false;
            }
            Some(6) => self.grid.step = None,
            Some(i) => {
                if let Some(step) = Step::ALL.get(i) {
                    self.grid.step = Some(*step);
                }
            }
            None => {}
        }
        let grid = self.grid;
        if let Some((_, editor)) = self.open.as_mut() {
            editor.grid = grid;
        }
    }

    /// The mouse on the picture, `kind` one of [`mouse`]. A press on a dot, a line or a handle of the highlight
    /// takes hold of it, and once the mouse has gone [`DRAG_STARTS`] it's dragged; a drag from empty space highlights
    /// a stretch of time, and a click there drops the highlight; a double-click adds, removes or straightens
    /// (`curve_touch` and `curve_hold` say which). Whether the picture is to be drawn again.
    pub fn mouse(&mut self, kind: i32, x: f64, y: f64, keys: Keys) -> bool {
        let s = self;
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        if kind < mouse::LEFT {
            s.keys = keys;
            s.mouse = Some((x, y));
        }
        match kind {
            mouse::MOVED => {
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
                    if !matches!(held, Held::Sweep(_)) && s.before.is_none() {
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
                    Held::Paint => {
                        if let Some(stroke) = s.stroke.as_mut() {
                            stroke.add(frame, length, x, y);
                            s.read_out = stroke.words();
                            if let Some(edit) = stroke.edit(frame) {
                                editor.drag(&edit);
                            }
                        }
                    }
                    Held::Swing(step, taken) => {
                        s.swing = curve_paint::swing(frame, step, keys, taken, x);
                        s.read_out = curve_paint::swing_words(s.swing);
                        editor.drag(&Edit::Swing { step, amount: s.swing });
                    }
                }
                true
            }
            // Ctrl on empty space paints: the stroke begins under the press, with no wait for a drag.
            mouse::PRESSED
                if keys.ctrl
                    && matches!(
                        curve_hold::over(editor.shape(), editor.highlight(), frame, x, y),
                        Over::Inside | Over::Empty
                    ) =>
            {
                let mut stroke = Stroke::begin(s.grid, keys);
                stroke.add(frame, editor.shape().length(), x, y);
                s.before = Some(editor.shape().clone());
                s.dropped = None;
                s.read_out = stroke.words();
                if let Some(edit) = stroke.edit(frame) {
                    editor.drag(&edit);
                }
                s.stroke = Some(stroke);
                s.lit_whole = false;
                s.held_dot = None;
                s.grab = Some((Held::Paint, (x, y), true));
                true
            }
            mouse::PRESSED => {
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
            mouse::LET_GO => {
                let was = s.grab.take();
                s.before = None;
                s.ghost = None;
                s.stroke = None;
                s.swing = 0.0;
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
            mouse::DOUBLE_CLICKED => {
                s.grab = None;
                s.before = None;
                s.ghost = None;
                s.stroke = None;
                s.swing = 0.0;
                s.read_out.clear();
                // Two quick strokes of paint are two strokes: no point is added under them.
                if keys.ctrl
                    && matches!(
                        curve_hold::over(editor.shape(), editor.highlight(), frame, x, y),
                        Over::Inside | Over::Empty
                    )
                {
                    s.dropped = None;
                    return false;
                }
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
            mouse::TAKEN_AWAY => {
                let was = s.grab.take();
                s.before = None;
                s.ghost = None;
                s.stroke = None;
                s.swing = 0.0;
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
    }

    /// The mouse on the ruler (`kind` one of [`mouse`]), `x` points from the picture's left. A press on a swing mark
    /// takes hold of them all: the drag goes on as the picture's drags do. Whether the picture is to be drawn again.
    pub fn ruler(&mut self, kind: i32, x: f64, keys: Keys) -> bool {
        let mut drawn = false;
        let dragging = matches!(self.grab, Some((Held::Swing(..), _, _)));
        if dragging && matches!(kind, mouse::MOVED | mouse::LET_GO | mouse::TAKEN_AWAY) {
            drawn = self.mouse(kind, x, 0.0, keys);
            if kind == mouse::MOVED {
                return drawn;
            }
            // Let go: the marks stand on the grid again, and the mouse may be on none of them now.
            self.mouse = None;
        }
        let near = kind != mouse::LEFT && self.swing_xs().iter().any(|m| (f64::from(*m) - x).abs() <= SWING_REACH);
        self.on_swing = near;
        if kind == mouse::PRESSED
            && near
            && self.grab.is_none()
            && let (Some(step), Some((_, editor))) = (self.grid.beats(), self.open.as_ref())
        {
            // Taken at the beat the mouse is on, so moving the view along mid-drag doesn't move the swing.
            let taken = self.frame().beat_at(x);
            self.before = Some(editor.shape().clone());
            self.keys = keys;
            self.lit_whole = false;
            self.grab = Some((Held::Swing(step, taken), (x, 0.0), true));
        }
        drawn
    }

    /// The mouse on the clip strip (`kind` one of [`mouse`]), `x` points from the picture's left. A click on a clip
    /// highlights its part, and a drag from there moves it as the highlight's bar does; a click on its pin pins it
    /// or lets it go; a double-click keeps the highlight as a clip; a right-click highlights the clip under it, for
    /// the menu. Whether the picture is to be drawn again.
    pub fn strip(&mut self, kind: i32, x: f64, keys: Keys) -> bool {
        // A drag begun here goes on as the bar's. The picture isn't told the mouse is on it for longer than that.
        if self.grab.is_some() && matches!(kind, mouse::MOVED | mouse::LET_GO | mouse::TAKEN_AWAY) {
            let drawn = self.mouse(kind, x, BAR_Y, keys);
            // While it's dragged the read-out follows the mouse; after that it's on the strip, not the picture.
            if kind != mouse::MOVED {
                self.mouse = None;
            }
            self.on_strip = (kind != mouse::TAKEN_AWAY).then_some(x);
            return drawn;
        }
        let s = self;
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        if kind < mouse::LEFT {
            s.keys = keys;
            s.on_strip = Some(x);
        }
        // The tiles as they are now, not as they were last shown: the curve or what's in sight may be new.
        let tiles = curve_strip::tiles(editor.shape(), editor.highlight(), frame, &s.name);
        let on = curve_strip::strip_at(&tiles, x);
        match (kind, on) {
            (mouse::PRESSED, OnStrip::Pin(clip)) => {
                let pinned = editor.shape().clips().get(clip).is_some_and(|c| c.pinned);
                editor.edit(&Edit::Pin { clip, pinned: !pinned })
            }
            (mouse::PRESSED, OnStrip::Clip(clip)) => {
                let Some(span) = editor.shape().clips().get(clip).map(|c| c.span) else { return false };
                editor.set_highlight(Some(span));
                let hold = curve_hold::hold(editor.shape(), Some(span), Part::Bar, x, BAR_Y);
                s.grab = Some((Held::Part(hold), (x, BAR_Y), false));
                s.held_dot = None;
                s.lit_whole = false;
                true
            }
            // A right-click on a clip: its part is the highlight, so the menu is about the clip under the mouse.
            (mouse::RIGHT_CLICKED, OnStrip::Clip(clip) | OnStrip::Pin(clip)) => {
                let Some(span) = editor.shape().clips().get(clip).map(|c| c.span) else { return false };
                let was = editor.highlight();
                editor.set_highlight(Some(span));
                editor.highlight() != was
            }
            // Two quick clicks on a pin pin and unpin: nothing more.
            (mouse::DOUBLE_CLICKED, OnStrip::Pin(_)) => false,
            (mouse::DOUBLE_CLICKED, _) => {
                // The highlight is kept as a clip, unless it's one already.
                let lit = editor.highlight();
                if lit.is_none() || curve_strip::clip_of(editor.shape(), lit).is_some() {
                    return false;
                }
                let name = curve_strip::part_name(editor.shape());
                editor.edit(&Edit::Remember { name })
            }
            (mouse::LEFT | mouse::TAKEN_AWAY, _) => {
                s.on_strip = None;
                false
            }
            _ => false,
        }
    }

    /// What the right-click menu offers for the highlighted part (the whole curve with none); empty with no curve
    /// open.
    pub fn part_offers(&self) -> Vec<Offer> {
        let step = self.grid.beats().unwrap_or(0.25);
        self.open.as_ref().map_or_else(Vec::new, |(_, e)| curve_strip::part_offers(e.shape(), e.highlight(), step))
    }

    /// One of the editor's own actions (the right-click menu's, or its key): reverse, flip, steps, keep or forget a
    /// clip, cut the time out, highlight all. Whether it did anything. Draw the picture again either way: a drag in
    /// hand is put back first, and the action is done to the curve as it was before it.
    pub fn act(&mut self, id: &str) -> bool {
        let s = self;
        let step = s.grid.beats().unwrap_or(0.25);
        let was = s.grab;
        s.hands_off();
        s.mouse = None;
        let Some((_, editor)) = s.open.as_mut() else { return false };
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
    }

    /// Randomise: the curve in the editor becomes a roll from `sources`, or with a highlight only that part is
    /// rolled again; one step of undo. False when there's no curve open, or nothing could be rolled. Draw the
    /// picture again either way: a drag in hand is given up first.
    pub fn randomise(&mut self, sources: &[curve_random::Source], settings: curve_random::Settings, seed: u64) -> bool {
        let s = self;
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
    }

    /// Esc: a drag in hand is put back as it was before it began; else the highlight is dropped. Whether it did
    /// either.
    pub fn escape(&mut self) -> bool {
        let s = self;
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
    }

    /// Whether there's a step to undo (`back`) or to redo.
    pub fn can_step(&self, back: bool) -> bool {
        self.open.as_ref().is_some_and(|(_, e)| if back { e.can_undo() } else { e.can_redo() })
    }

    /// Undo (`back`) or Redo; whether a step was taken. What was in hand is let go of either way.
    pub fn step_back(&mut self, back: bool) -> bool {
        let s = self;
        s.hands_off();
        s.lit_whole = false;
        let Some((_, e)) = s.open.as_mut() else { return false };
        let was = e.shape().length();
        let done = if back { e.undo() } else { e.redo() };
        // A step that changes the curve's length (a roll, the end mark): the whole of it in sight again.
        let length = e.shape().length();
        if done && (length - was).abs() > 1e-9 {
            s.view = View::whole(length);
        }
        done
    }

    /// The curve as it's been edited, when it has been: its points and its length in beats.
    pub fn edited(&self) -> Option<(Vec<CurvePoint>, f64)> {
        let (_, editor) = self.open.as_ref()?;
        differs(editor, self.origin.as_ref()).then(|| (editor.shape().curve_points(), editor.shape().length()))
    }

    /// The id of the curve on the card, when it has edits that aren't saved.
    pub fn edited_id(&self) -> Option<String> {
        self.open.as_ref().filter(|(_, e)| differs(e, self.origin.as_ref())).map(|(id, _)| id.clone())
    }

    /// A curve's edits were saved as a new curve: the curve they were made on is as it was found again, on the card
    /// or kept for later. Whether it's the one on the card (to be drawn again).
    pub fn forget(&mut self, id: &str) -> bool {
        self.kept.retain(|(kept, ..)| kept != id);
        let open = self.open.as_ref().is_some_and(|(open, _)| open == id);
        if open {
            self.hands_off();
            self.lit_whole = false;
            let origin = self.origin.clone();
            if let (Some((_, editor)), Some(origin)) = (self.open.as_mut(), origin) {
                let grid = editor.grid;
                *editor = Editor::new(origin);
                editor.grid = grid;
            }
        }
        open
    }

    /// The wheel over the ruler or the picture, `x` points from the picture's left: zooms round the mouse, or moves
    /// along (`along`: with Shift held). Whether what's in sight changed.
    pub fn wheel(&mut self, x: f64, by: f64, along: bool) -> bool {
        if self.open.is_none() || !by.is_finite() || by == 0.0 {
            return false;
        }
        let (frame, length) = (self.frame(), self.length());
        if !frame.fits() {
            return false;
        }
        let notches = by / NOTCH;
        let view = if along {
            frame.view.moved(-notches * NOTCH_MOVES * frame.view.span, length)
        } else {
            frame.view.zoomed(frame.beat_at(x), 2f64.powf(notches / NOTCHES_TO_HALVE), length)
        };
        let changed = view != self.view;
        self.view = view;
        changed
    }

    /// A picture of the curve as it is now to work out; none with no curve open or no room for one.
    pub fn ask(&mut self) -> Option<Ask> {
        let frame = self.frame();
        frame.fits().then_some(())?;
        let (shape, highlight) = self.open.as_ref().map(|(_, e)| (e.shape().clone(), e.highlight()))?;
        self.pictures += 1;
        Some(Ask { shown: self.shown, number: self.pictures, shape, highlight, grid: self.grid, frame })
    }

    /// The screen has nothing of the card on it any more (its window was closed and opened again): the next
    /// [`Card::showing`] gives all its lists again, changed or not.
    pub fn show_all(&mut self) {
        self.dots = None;
        self.tiles = None;
        self.swing_marks = None;
        self.ghost = None;
        // A faint old curve may be on the screen still: taken away, or given again if a drag is in hand.
        self.ghost_shown = true;
    }

    /// What lies over the picture as the mouse and the edits have it now. `mac`: the keys are named as a Mac's are.
    pub fn showing(&mut self, mac: bool) -> Showing {
        let s = self;
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_ref() else {
            s.dots = None;
            s.tiles = None;
            s.ghost = None;
            s.ghost_shown = false;
            s.swing_marks = None;
            return Showing::default();
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
            (Some(Held::Sweep(_) | Held::Paint | Held::Swing(..)), _) => Over::Empty,
            (None, Some((x, y))) => curve_hold::over_in(shape, hl, frame, x, y),
            _ => Over::Empty,
        };
        let on_show = match (held, over) {
            (Some(_), Over::Dot(point)) => curve_touch::dots_with(shape, frame, Some(point)),
            (Some(_), _) => curve_touch::dots_with(shape, frame, None),
            (None, _) => curve_touch::dots(shape, frame, s.mouse),
        };
        let dots: Vec<Dot> = on_show
            .into_iter()
            .map(|d| {
                let hot = over == Over::Dot(d.point);
                Dot { x: d.x as f32, y: d.y as f32, hot: hot && held.is_none(), held: hot && held.is_some() }
            })
            .collect();
        let new_dots = (s.dots.as_ref() != Some(&dots)).then(|| dots.clone());
        s.dots = Some(dots);
        let handle = match over {
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
        };
        let hot_line = match over {
            Over::Line(line) => match (shape.points().get(line), shape.points().get(line + 1)) {
                (Some(a), Some(b)) => path(frame, a.beat, b.beat, |beat| shape.line_value(line, beat)),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let ghost = match (&s.before, s.ghost) {
            (Some(_), Some(made_for)) if made_for == frame => None,
            (Some(old), _) => {
                s.ghost = Some(frame);
                s.ghost_shown = true;
                Some(path(frame, 0.0, old.length(), |b| old.value_at(b)))
            }
            (None, _) => {
                s.ghost = None;
                std::mem::take(&mut s.ghost_shown).then(String::new)
            }
        };
        let mut pointer = match (held, over) {
            (Some(Held::Touch(Grab::Dot { .. })), _) => pointer::HOLDING,
            (_, Over::Part(Part::Bar)) if s.keys.ctrl => pointer::COPY,
            (Some(_), Over::Part(Part::Bar)) => pointer::HOLDING,
            (None, Over::Part(Part::Bar)) | (None, Over::Dot(_)) => pointer::HAND,
            (_, Over::Part(Part::Left | Part::Right)) => pointer::LEFT_RIGHT,
            (_, Over::Part(Part::Top | Part::Bottom)) | (_, Over::Line(_)) => pointer::UP_DOWN,
            (_, Over::Part(Part::Corner { left, top })) => {
                if left == top {
                    pointer::SLANT_DOWN
                } else {
                    pointer::SLANT_UP
                }
            }
            _ => pointer::PLAIN,
        };
        let hint = curve_hold::hint(over, mac);
        let on = s.mouse.is_some() || held.is_some();
        let (mut hint_name, mut hint_does, mut hint_keys) = if on {
            (String::from(hint.name), String::from(hint.does), String::from(hint.keys))
        } else {
            (String::new(), String::from(HINT_AWAY), String::new())
        };
        // The paint key held over empty space, or a stroke in hand: what it paints. And the swing mark.
        let painting = held == Some(Held::Paint);
        let may_paint =
            held.is_none() && s.mouse.is_some() && s.keys.ctrl && matches!(over, Over::Inside | Over::Empty);
        if painting || may_paint {
            let keys = Keys { shift: s.stroke.as_ref().map_or(s.keys.shift, Stroke::by_hand), ..s.keys };
            let (does, more) = curve_paint::hint(s.grid, keys, painting);
            hint_name = "Paint".into();
            hint_does = does.into();
            hint_keys = more.into();
            pointer = pointer::PAINT;
        }
        let swing_hot = matches!(held, Some(Held::Swing(..))) || (held.is_none() && s.on_swing);
        if swing_hot {
            hint_name = "Swing".into();
            hint_does = "drag to move every second grid line later or earlier · the curve follows".into();
            hint_keys = "Shift: freely".into();
        }
        // The clip strip: its tiles, and its own hint while the mouse is on it.
        let tiles = curve_strip::tiles(shape, editor.highlight(), frame, &s.name);
        let on_strip = s.on_strip.filter(|_| held.is_none()).map(|x| curve_strip::strip_at(&tiles, x));
        if let Some(on) = on_strip {
            let (name, does) = curve_strip::hint(on, &tiles);
            hint_name = name;
            hint_does = does.into();
            hint_keys = String::new();
        }
        let shown: Vec<TileShown> = tiles
            .iter()
            .map(|t| TileShown {
                x: t.left as f32,
                width: (t.right - t.left) as f32,
                name: t.name.clone(),
                words: t.words.clone(),
                pinned: t.pinned,
                picked: t.picked,
                lighter: t.lighter,
                hot: t.clip.is_some()
                    && matches!(on_strip, Some(OnStrip::Clip(i) | OnStrip::Pin(i)) if Some(i) == t.clip),
                pin_hot: matches!(on_strip, Some(OnStrip::Pin(i)) if Some(i) == t.clip),
                can_pin: t.clip.is_some() && t.right - t.left >= curve_strip::PIN_FROM,
            })
            .collect();
        let can_undo = editor.can_undo();
        let can_redo = editor.can_redo();
        let changed = differs(editor, s.origin.as_ref());
        let new_tiles = (s.tiles.as_ref() != Some(&shown)).then(|| shown.clone());
        s.tiles = Some(shown);
        let marks = s.swing_xs();
        let new_marks = (s.swing_marks.as_ref() != Some(&marks)).then(|| marks.clone());
        s.swing_marks = Some(marks);
        Showing {
            open: true,
            dots: new_dots,
            frame: hl,
            handle,
            near: on,
            hot_line,
            ghost,
            width: frame.width as f32,
            height: frame.height as f32,
            pointer,
            hint_name,
            hint_does,
            hint_keys,
            keys: s.keys,
            read_out: s.read_out.clone(),
            mouse: s.mouse.map(|(x, y)| (x as f32, y as f32)),
            swing_hot,
            swing_marks: new_marks,
            tiles: new_tiles,
            can_undo,
            can_redo,
            changed,
        }
    }

    /// How the curve is heard.
    pub fn hear(&self) -> Hear {
        self.hear
    }

    /// The metronome on or off (`None` turns it over), or the speed `by` steps up or down (0 for the usual speed).
    /// While the tone plays it's made again with it, at the same beat ([`Card::play_line`] says so).
    pub fn set_hear(&mut self, click: Option<bool>, speed: Option<i32>) {
        if speed.is_none() {
            self.hear.click = click.unwrap_or(!self.hear.click);
        }
        match speed {
            Some(0) => self.hear.bpm = curve_hear::BPM_USUAL,
            Some(by) => self.hear.bpm = curve_hear::stepped(self.hear.bpm, by),
            None => {}
        }
    }

    /// What Hear plays now: the editor's curve (its points and its length in beats) and how it's heard.
    pub fn to_hear(&self) -> Option<Heard> {
        self.open.as_ref().map(|(_, e)| (e.shape().curve_points(), e.shape().length(), self.hear))
    }

    /// The tone under `key` was asked for: the play line follows it from when it starts to when it stops. `heard`
    /// is what the tone was made from ([`Card::to_hear`]); none when that isn't known (a tone found playing), and
    /// then it's made again from the editor's curve. False with no curve open: there's nothing to follow.
    pub fn follow(&mut self, key: String, heard: Option<Heard>) -> bool {
        if self.open.is_none() {
            return false;
        }
        self.tone = Some((key, Instant::now(), false));
        self.heard = heard;
        true
    }

    /// Where the play line is now. `playing` is the key of what the player plays, `position` how far into it in
    /// seconds, `sounding` what the tone playing under a key was made with (its length in beats, how it's heard),
    /// when that's known; `may_make` is asked once, when the tone should be made again, whether one may be (one at
    /// a time).
    pub fn play_line(
        &mut self,
        playing: Option<&str>,
        position: impl FnOnce() -> f64,
        sounding: impl FnOnce(&str) -> Option<(f64, Hear)>,
        may_make: impl FnOnce() -> bool,
    ) -> PlayLine {
        let s = self;
        let frame = s.frame();
        let length_now = s.length();
        let wanted = s.to_hear();
        let Some((key, since, seen)) = s.tone.as_mut() else { return PlayLine { over: true, ..PlayLine::default() } };
        if playing != Some(key.as_str()) {
            // Not playing: still being made, or stopped (or another sound took its place).
            if *seen || since.elapsed() > TONE_WAIT {
                s.tone = None;
            }
            return PlayLine { over: s.tone.is_none(), ..PlayLine::default() };
        }
        *seen = true;
        // The curve was edited, or is heard another way, since the tone was made: it's made again and put in the
        // tone's place at the same beat, so what's heard follows the editor as it plays (Solomon, 14 Nov 2026).
        let mut again = None;
        if let Some(wanted) = wanted
            && !s.heard.as_ref().is_some_and(|h| same_heard(h, &wanted))
            && may_make()
        {
            // Counted as heard from here on, even if it can't be made: no trying again at every tick.
            s.heard = Some(wanted.clone());
            again = Some((key.clone(), wanted));
        }
        // The play line goes by the tone playing, until the new one takes over where this one is.
        let (length, hear) = sounding(key).unwrap_or((length_now, s.hear));
        let beat = curve_hear::beat_at(position(), hear, length);
        let view = frame.view;
        let in_sight = frame.fits() && beat >= view.start - 1e-9 && beat <= view.start + view.span + 1e-9;
        PlayLine { at: Some(if in_sight { frame.x(beat) as f32 } else { -1.0 }), again, over: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rise() -> Vec<CurvePoint> {
        vec![CurvePoint { x: 0.0, y: 0.0, bend: 0.0 }, CurvePoint { x: 1.0, y: 1.0, bend: 0.0 }]
    }

    /// A card 400 by 200 points with a rise 4 beats long open in it.
    fn card() -> Card {
        let mut card = Card::default();
        card.laid_out(400.0, 200.0);
        assert!(card.open("a", "Rise", &rise(), 4.0));
        card
    }

    fn at(card: &Card, beat: f64, value: f64) -> (f64, f64) {
        let frame = card.frame();
        (frame.x(beat), frame.y(value))
    }

    fn press_drag(card: &mut Card, from: (f64, f64), to: (f64, f64), keys: Keys) {
        let (a, b) = (at(card, from.0, from.1), at(card, to.0, to.1));
        card.mouse(mouse::MOVED, a.0, a.1, keys);
        card.mouse(mouse::PRESSED, a.0, a.1, keys);
        card.mouse(mouse::MOVED, b.0, b.1, keys);
    }

    #[test]
    fn a_card_with_no_curve_shows_nothing_and_takes_no_mouse() {
        let mut card = Card::default();
        card.laid_out(400.0, 200.0);
        assert!(!card.mouse(mouse::PRESSED, 10.0, 10.0, Keys::default()));
        assert!(!card.act(curve_act::FLIP));
        assert!(!card.escape());
        assert!(card.ask().is_none());
        assert_eq!(card.showing(false), Showing::default());
        assert!(card.to_hear().is_none());
        assert!(!card.follow("k".into(), None));
    }

    #[test]
    fn dragging_a_point_changes_the_curve_and_esc_puts_it_back() {
        let mut card = card();
        assert!(card.edited().is_none());
        press_drag(&mut card, (4.0, 1.0), (4.0, 0.5), Keys::default());
        let shown = card.showing(false);
        assert!(shown.open && shown.ghost.as_ref().is_some_and(|g| !g.is_empty()), "the old curve is shown faint");
        assert!(!shown.read_out.is_empty());
        assert!(card.edited().is_some());
        assert!(card.escape());
        assert!(card.edited().is_none(), "Esc puts the drag back");
        assert_eq!(card.showing(false).ghost.as_deref(), Some(""), "and the faint curve goes");
        // Dragged again and let go: one step to undo.
        press_drag(&mut card, (4.0, 1.0), (4.0, 0.5), Keys::default());
        let b = at(&card, 4.0, 0.5);
        assert!(card.mouse(mouse::LET_GO, b.0, b.1, Keys::default()));
        assert_eq!(card.edited_id().as_deref(), Some("a"));
        assert!(card.showing(false).can_undo);
        assert!(card.step_back(true));
        assert!(card.edited().is_none());
        assert!(card.can_step(false) && !card.can_step(true));
    }

    #[test]
    fn the_lists_are_only_given_when_they_change() {
        let mut card = card();
        let first = card.showing(false);
        assert!(first.dots.is_some() && first.tiles.is_some() && first.swing_marks.is_some());
        let second = card.showing(false);
        assert!(second.dots.is_none() && second.tiles.is_none() && second.swing_marks.is_none());
        assert!(second.ghost.is_none());
        // Another curve: all of them again.
        card.close();
        assert!(!card.showing(false).open);
        assert!(card.open("b", "Rise", &rise(), 4.0));
        assert!(card.showing(false).dots.is_some());
    }

    #[test]
    fn a_drag_over_empty_space_highlights_and_a_click_drops_it() {
        let mut card = card();
        press_drag(&mut card, (1.0, 0.9), (3.0, 0.9), Keys::default());
        let b = at(&card, 3.0, 0.9);
        card.mouse(mouse::LET_GO, b.0, b.1, Keys::default());
        assert_eq!(card.highlight().map(|h| (h.start, h.end)), Some((1.0, 3.0)));
        assert!(card.edited().is_none(), "a highlight is no edit");
        // A click on empty space out of the highlight drops it; Esc would too.
        let c = at(&card, 3.5, 0.05);
        card.mouse(mouse::PRESSED, c.0, c.1, Keys::default());
        card.mouse(mouse::LET_GO, c.0, c.1, Keys::default());
        assert!(card.highlight().is_none());
    }

    #[test]
    fn ctrl_paints_steps_from_the_press_on() {
        let mut card = card();
        let ctrl = Keys { ctrl: true, ..Keys::default() };
        let a = at(&card, 1.1, 0.9);
        card.mouse(mouse::MOVED, a.0, a.1, ctrl);
        let before = card.showing(false);
        assert_eq!((before.hint_name.as_str(), before.pointer), ("Paint", pointer::PAINT));
        assert!(card.mouse(mouse::PRESSED, a.0, a.1, ctrl), "the stroke begins under the press");
        let b = at(&card, 2.1, 0.2);
        card.mouse(mouse::MOVED, b.0, b.1, ctrl);
        card.mouse(mouse::LET_GO, b.0, b.1, ctrl);
        let (points, length) = card.edited().expect("painted");
        assert!(points.len() > 2 && (length - 4.0).abs() < 1e-9);
        assert!(card.step_back(true), "one stroke, one step");
        assert!(card.edited().is_none());
    }

    #[test]
    fn a_swing_mark_on_the_ruler_is_dragged_and_springs_back() {
        let mut card = card();
        let marks = card.showing(false).swing_marks.expect("the first time");
        let mark = f64::from(*marks.first().expect("a 1/16 grid has swing marks"));
        card.ruler(mouse::MOVED, mark, Keys::default());
        assert!(card.showing(false).swing_hot);
        card.ruler(mouse::PRESSED, mark, Keys::default());
        assert!(card.ruler(mouse::MOVED, mark + 4.0, Keys::default()));
        let moved = card.showing(false);
        assert_eq!(moved.hint_name, "Swing");
        assert!(moved.swing_marks.is_some_and(|m| m.first().is_some_and(|x| f64::from(*x) > mark)));
        card.ruler(mouse::LET_GO, mark + 4.0, Keys::default());
        assert!(card.edited().is_some(), "the curve followed");
        let back = card.showing(false).swing_marks.expect("they moved back");
        assert!((f64::from(*back.first().expect("marks")) - mark).abs() < 0.01, "the marks stand on the grid again");
    }

    #[test]
    fn a_curve_left_with_edits_is_as_it_was_left_when_picked_again() {
        let mut card = card();
        assert!(card.act(curve_act::FLIP));
        let flipped = card.edited().expect("flipped");
        assert!(card.open("b", "Other", &rise(), 8.0));
        assert!(card.edited().is_none());
        assert!(card.open("a", "Rise", &rise(), 4.0));
        assert_eq!(card.edited(), Some(flipped));
        assert!(!card.open("a", "Renamed", &rise(), 4.0), "the same curve again only takes its name");
        // Saved as a new curve: the one it was made on is as it was found.
        assert!(card.forget("a"));
        assert!(card.edited().is_none() && !card.can_step(true));
        // One that has changed where it came from starts afresh.
        assert!(card.act(curve_act::FLIP));
        card.close();
        assert!(card.open("a", "Rise", &rise(), 2.0));
        assert!(card.edited().is_none());
    }

    #[test]
    fn a_clip_on_the_strip_is_pressed_as_the_strip_is_now_and_dragged_with_the_read_out() {
        let mut card = card();
        // Beats 1 to 2 kept as a clip, with nothing shown since: the press still finds it.
        press_drag(&mut card, (1.0, 0.9), (2.0, 0.9), Keys::default());
        let b = at(&card, 2.0, 0.9);
        card.mouse(mouse::LET_GO, b.0, b.1, Keys::default());
        assert!(card.act(curve_act::KEEP_CLIP));
        assert!(card.escape(), "the highlight is dropped");
        let x = card.frame().x(1.5);
        assert!(card.strip(mouse::PRESSED, x, Keys::default()));
        assert_eq!(card.highlight().map(|h| (h.start, h.end)), Some((1.0, 2.0)));
        // Dragged along: the read-out is where the mouse is. Let go: the mouse is on the strip, not the picture.
        card.strip(mouse::MOVED, card.frame().x(2.5), Keys::default());
        let shown = card.showing(false);
        assert!(shown.mouse.is_some() && !shown.read_out.is_empty());
        card.strip(mouse::LET_GO, card.frame().x(2.5), Keys::default());
        assert!(card.showing(false).mouse.is_none());
        // A window opened again is given everything again.
        assert!(card.showing(false).dots.is_none());
        card.show_all();
        let all = card.showing(false);
        assert!(all.dots.is_some() && all.tiles.is_some() && all.swing_marks.is_some());
    }

    #[test]
    fn the_grid_chips_pick_a_step_and_one_of_triplet_and_dotted() {
        let mut card = card();
        let (step, triplet, dotted) = card.grid_chips();
        assert!(step >= 0 && !triplet && !dotted);
        card.pick_grid(4);
        assert!(card.grid_chips().1);
        card.pick_grid(5);
        assert_eq!((card.grid_chips().1, card.grid_chips().2), (false, true));
        card.pick_grid(6);
        assert_eq!(card.grid_chips().0, -1);
        card.pick_grid(4);
        assert!(!card.grid_chips().1, "no triplets with the grid off");
        card.pick_grid(0);
        assert_eq!(card.grid_chips().0, 0);
        assert!(card.showing(false).open);
    }

    #[test]
    fn the_wheel_zooms_and_a_new_length_shows_the_whole_curve_again() {
        let mut card = card();
        assert!(card.wheel(200.0, 180.0, false));
        assert!(card.frame().view.span < 4.0);
        assert!(!card.wheel(200.0, 0.0, false));
        let (marks, end) = ruler(card.length(), card.frame());
        assert!(!marks.is_empty() && end < 0.0, "zoomed in on the middle, the end is out of sight");
        let whole = View::whole(4.0);
        card.view = whole;
        let (_, end) = ruler(card.length(), card.frame());
        assert!(end > 0.0);
    }

    #[test]
    fn the_play_line_follows_the_tone_and_asks_for_it_again_after_an_edit() {
        let mut card = card();
        let heard = card.to_hear().expect("a curve is open");
        assert!(card.follow("k".into(), Some(heard)));
        // Still being made: nothing to show, but not over.
        let waiting = card.play_line(None, || 0.0, |_| None, || true);
        assert_eq!(waiting, PlayLine::default());
        // Playing, a beat in at 120: the line is a quarter of the way along, and the tone is the editor's.
        let line = card.play_line(Some("k"), || 0.5, |_| None, || panic!("nothing has changed"));
        assert!(line.again.is_none() && !line.over);
        assert!((f64::from(line.at.expect("playing")) - card.frame().x(1.0)).abs() < 0.01);
        // Edited while it plays: made again, once.
        assert!(card.act(curve_act::FLIP));
        let line = card.play_line(Some("k"), || 0.5, |_| None, || true);
        assert_eq!(line.again.map(|(key, _)| key).as_deref(), Some("k"));
        assert!(card.play_line(Some("k"), || 0.5, |_| None, || panic!("made already")).again.is_none());
        // The metronome: again. Not while another is still being made.
        card.set_hear(Some(true), None);
        assert!(card.play_line(Some("k"), || 0.5, |_| None, || false).again.is_none());
        assert!(card.play_line(Some("k"), || 0.5, |_| None, || true).again.is_some());
        // Stopped: over.
        assert!(card.play_line(None, || 0.0, |_| None, || true).over);
        assert!(card.play_line(None, || 0.0, |_| None, || true).over);
    }

    #[test]
    fn the_speed_steps_and_goes_back_to_the_usual() {
        let mut card = Card::default();
        card.set_hear(None, Some(1));
        assert!((card.hear().bpm - 125.0).abs() < 1e-9 && !card.hear().click);
        card.set_hear(None, Some(0));
        assert!((card.hear().bpm - curve_hear::BPM_USUAL).abs() < 1e-9);
        card.set_hear(None, None);
        assert!(card.hear().click);
        card.set_hear(Some(false), None);
        assert!(!card.hear().click);
    }
}
