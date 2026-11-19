//! The curve editor's card with no screen: the curve open in it, what the mouse holds, the grid, what's in sight, how
//! it's heard, and everything the card shows (the dots, the highlight's frame, the hint line, the clip strip, the
//! swing marks, the play line). The app's Curves page and the plugin's Curves tab each keep one [`Card`], tell it
//! what the mouse and the keys did, and copy what it [`Card::showing`] onto their own screen: so both work the same
//! (the plan's step 8: "the same editor in the plugin's Curves tab").

use std::time::{Duration, Instant};

use mo_formats::bitwig::curve::CurvePoint;

use crate::curve_edit::{self, Edit, Editor, Grid, Piece, Point, Shape, Span, Step};
use crate::curve_hear::{self, Hear};
use crate::curve_hold::{self, Hold, Over, Part};
use crate::curve_keys::{KeyMap, Role};
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
/// How far apart, in points, two presses may be for a screen to call them a double-click.
const SAME_SPOT: f64 = 10.0;

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
    /// A box drawn over empty space (Alt): the points inside it are picked.
    Band,
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
    /// It's one of the picked points.
    pub picked: bool,
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

/// How far from where a stroke of paint ended, in points, a quick press still paints on rather than taking hold of
/// a point: the step just painted, with its corners (two reaches of a dot).
const PAINTS_ON: f64 = 24.0;

/// Whether a stroke of paint was let go a moment ago, close to `x`, `y`.
fn just_painted(painted: Option<(Instant, (f64, f64))>, x: f64, y: f64) -> bool {
    painted.is_some_and(|(when, at)| when.elapsed() < DOUBLE_CLICK && (at.0 - x).hypot(at.1 - y) < PAINTS_ON)
}

/// What "Swing as the song" did ([`Card::swing_as_song`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SongSwing {
    /// The curve has the song's swing now.
    Done,
    /// It had it already.
    Already,
    /// That swing changes nothing here (a flat curve, a part too short), or no curve is open.
    Nothing,
}

/// The swing "Swing as the song" last gave: to which curve, the curve before it, the parts it was pressed for (none
/// is the whole curve), the curve as it left it, and the swing.
#[derive(Clone, Debug)]
struct Swung {
    curve: String,
    before: Shape,
    parts: Vec<Option<Span>>,
    left: Shape,
    step: f64,
    amount: f64,
}

/// Everything that lies over the picture as the mouse and the edits have it now. What is costly to put on a screen
/// (the lists, the faint old curve) is only given when it has changed since the last time it was asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Showing {
    /// A curve is open. With none, the rest is empty: the screen clears its card.
    pub open: bool,
    /// The dots, when they've changed.
    pub dots: Option<Vec<Dot>>,
    /// The box being drawn to pick points: its left, top, width and height in the picture's points.
    pub band: Option<(f32, f32, f32, f32)>,
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
    /// The keys held, as their meanings (free, bend, make): their lamps. And the names of the keys that have those
    /// meanings.
    pub keys: Keys,
    pub key_names: [&'static str; 3],
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
            $data.set_band_shown(false);
            $data.set_swing_marks(::slint::ModelRc::default());
            $data.set_hl_shown(false);
            $data.set_hot_line("".into());
            $data.set_ghost("".into());
            $data.set_read_out("".into());
            $data.set_can_undo(false);
            $data.set_can_redo(false);
        } else {
            if let Some(dots) = $now.dots {
                let dots: Vec<$dot> = dots
                    .into_iter()
                    .map(|d| $dot { x: d.x, y: d.y, hot: d.hot, held: d.held, picked: d.picked })
                    .collect();
                // The dots that are as they were stay as they are on the screen: only the one under the mouse or in
                // hand is drawn anew on a move, not every dot of the curve.
                let model = $data.get_dots();
                match ::slint::Model::as_any(&model).downcast_ref::<::slint::VecModel<$dot>>() {
                    Some(old) if ::slint::Model::row_count(old) == dots.len() => {
                        for (i, d) in dots.into_iter().enumerate() {
                            if ::slint::Model::row_data(old, i).as_ref() != Some(&d) {
                                ::slint::Model::set_row_data(old, i, d);
                            }
                        }
                    }
                    Some(old) => old.set_vec(dots),
                    None => $data.set_dots(::slint::ModelRc::new(::slint::VecModel::from(dots))),
                }
            }
            match $now.band {
                Some((x, y, width, height)) => {
                    $data.set_band_x(x);
                    $data.set_band_y(y);
                    $data.set_band_width(width);
                    $data.set_band_height(height);
                    $data.set_band_shown(true);
                }
                None => $data.set_band_shown(false),
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
            $data.set_key_free($now.key_names[0].into());
            $data.set_key_bend($now.key_names[1].into());
            $data.set_key_make($now.key_names[2].into());
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

/// Undo or Redo asked for again within this long is a key held down, not a new press.
const KEY_HELD: Duration = Duration::from_millis(600);

/// The curve editor's card.
pub struct Card {
    /// The curve on the card (by its id as it's sent) and its editor.
    open: Option<(String, Editor)>,
    /// The grid, kept from curve to curve.
    grid: Grid,
    /// Which key does what (Settings > Shortcuts).
    key_map: KeyMap,
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
    /// The picked points (their places in the curve's list, in order), and the curve's points as they were when
    /// those were picked: any other change to the curve and nothing is picked. As they were at the press, for a
    /// drag that moves them together (or a box that adds to them); and the box being drawn, corner to corner.
    picked: Vec<usize>,
    picked_of: Vec<Point>,
    picked_from: Vec<usize>,
    band: Option<((f64, f64), (f64, f64))>,
    /// The points picked before the last click on a point, and when: a double-click on one of several picked
    /// points begins with a click that picks it alone.
    clicked: Option<(Vec<usize>, Instant)>,
    /// When and where a stroke of paint was last let go: a quick press with the make key close by paints on, even
    /// right on that stroke's own corners, and the double-click the screen makes of the two does nothing.
    painted: Option<(Instant, (f64, f64))>,
    /// The swing "Swing as the song" last gave.
    swung: Option<Swung>,
    /// The curve's name, for the strip's one tile when it has no clips.
    name: String,
    /// Where the mouse is on the clip strip, from the picture's left (none when it isn't there).
    on_strip: Option<f64>,
    /// The strip's tiles as they were last shown.
    tiles: Option<Vec<TileShown>>,
    /// The drag in hand highlighted the whole curve to work on it (a pill round the whole curve): given up or
    /// changing nothing, it leaves nothing highlighted again.
    lit_whole: bool,
    /// The highlight a click on empty space just dropped, when and where: a double-click (which adds a point) begins
    /// with such a click, and puts it back. (Only a double-click at that spot: one made quickly somewhere else began
    /// with another click.)
    dropped: Option<(Span, Instant, (f64, f64))>,
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
    /// Whether the screen goes back to a curve left ([`Card::back_to`]): one where another curve can be picked
    /// while this one is on the card.
    goes_back: bool,
    /// The curve with edits that was left last for another (its id): Undo with nothing more to undo goes back to
    /// it. The curve Undo went back from: Redo with nothing to redo goes to it again. And the way the screen is
    /// going right now (back, or forward again), till the curve it goes to is opened.
    left: Option<String>,
    ahead: Option<(String, u64)>,
    going: Option<(String, bool)>,
    /// When Undo or Redo was last asked for: a key held down doesn't run on from one curve into another.
    asked: Option<Instant>,
    /// The curve that was last taken off the card.
    gone_from: Option<String>,
}

impl Default for Card {
    fn default() -> Card {
        Card {
            open: None,
            grid: Grid::default(),
            key_map: KeyMap::default(),
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
            picked: Vec::new(),
            picked_of: Vec::new(),
            picked_from: Vec::new(),
            band: None,
            clicked: None,
            painted: None,
            swung: None,
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
            goes_back: false,
            left: None,
            ahead: None,
            going: None,
            asked: None,
            gone_from: None,
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

    /// How many points the open curve has (none with no curve).
    pub fn points(&self) -> usize {
        self.open.as_ref().map_or(0, |(_, e)| e.shape().points().len())
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
        self.band = None;
        self.swing = 0.0;
        self.read_out.clear();
    }

    /// Which key does what from here on.
    pub fn set_keys(&mut self, map: KeyMap) {
        if map != self.key_map {
            self.key_map = map;
            self.keys = Keys::default();
        }
    }

    /// The picked points: none once the curve is another than the one they were picked in.
    fn picked_now(&self) -> &[usize] {
        match self.open.as_ref() {
            Some((_, e)) if !self.picked.is_empty() && e.shape().points() == self.picked_of.as_slice() => &self.picked,
            _ => &[],
        }
    }

    /// How many points are picked.
    pub fn picked(&self) -> usize {
        self.picked_now().len()
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
            self.kept.push((id.clone(), origin, editor, self.view));
            if self.kept.len() > KEPT {
                self.kept.remove(0);
            }
        }
        self.gone_from = Some(id);
    }

    /// The screen goes back to a curve left for another when Undo has nothing more to undo ([`Card::back_to`]).
    pub fn set_goes_back(&mut self, on: bool) {
        self.goes_back = on;
    }

    /// The curve Undo (`back`) or Redo would go to when the curve on the card has no step of its own to take: the
    /// one worked on that was left for this one (picked by accident, say), or the one Undo came back from. The
    /// screen picks it in its list, having said so with [`Card::go`]; it comes back as it was left.
    pub fn back_to(&self, back: bool) -> Option<&str> {
        if self.can_step_here(back) {
            return None;
        }
        self.way(back)
    }

    /// Where Undo (`back`) or Redo leads beyond this curve's own steps.
    fn way(&self, back: bool) -> Option<&str> {
        if !self.goes_back || self.open.is_none() {
            return None;
        }
        if back {
            // Back only to one that's still kept as it was left: a curve that comes back afresh is no step back.
            let to = self.left.as_deref()?;
            self.kept.iter().any(|(kept, ..)| kept == to).then_some(to)
        } else {
            // Forward only while nothing new has been done here since Undo came back: a new edit leaves nothing
            // to redo.
            let (to, made) = self.ahead.as_ref()?;
            let (_, editor) = self.open.as_ref()?;
            (editor.made() == *made && !editor.dragging()).then_some(to.as_str())
        }
    }

    /// Undo (`back`) or Redo was asked for with no step of its own left in the curve on the card: the curve the
    /// screen is to pick ([`Card::back_to`]). Opened next, it's a step back (or forward again), not a curve picked
    /// anew. None when Undo or Redo was asked for only a moment ago: a key held down stops at the curve's last step,
    /// and goes on to the other curve when it's pressed again.
    pub fn go(&mut self, back: bool) -> Option<String> {
        self.go_at(back, Instant::now())
    }

    fn go_at(&mut self, back: bool, now: Instant) -> Option<String> {
        let held = self.asked.replace(now).is_some_and(|last| now.saturating_duration_since(last) < KEY_HELD);
        if held {
            return None;
        }
        let to = self.back_to(back)?.to_owned();
        self.going = Some((to.clone(), back));
        Some(to)
    }

    /// The curve [`Card::go`] led to isn't there to be picked any more: that way is closed.
    pub fn no_way(&mut self, back: bool) {
        self.going = None;
        if back {
            self.left = None;
        } else {
            self.ahead = None;
        }
    }

    /// Whether the curve on the card has a step of its own to undo (`back`) or redo. A drag in hand is one to undo.
    fn can_step_here(&self, back: bool) -> bool {
        self.open.as_ref().is_some_and(|(_, e)| if back { e.can_undo() || e.dragging() } else { e.can_redo() })
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
        if let Some((_, editor)) = self.open.as_mut().filter(|(open, _)| open == id) {
            // The same curve, read again with another height or length (its id is its shape, whatever its height
            // and length; the list gives those of its newest place): with no edits made on the old picture, the
            // card shows it as it's found now (the 19 Nov 2026 review's CU-8). With edits it stays: they were made
            // on what's in sight.
            let found = Shape::from_curve(points, length_beats);
            let worked_on = editor.changed() || editor.can_undo() || editor.can_redo() || editor.dragging();
            if worked_on || self.origin.as_ref() == Some(&found) {
                return false;
            }
            let grid = editor.grid;
            *editor = Editor::new(found.clone());
            editor.grid = grid;
            self.view = View::whole(found.length());
            self.origin = Some(found);
            self.hands_off();
            self.shown += 1;
            self.tone = None;
            return true;
        }
        self.put_away();
        // Where Undo and Redo go once this curve has no step of its own. Picked anew: back to the curve with edits
        // that was left for it (one left without any keeps the way back to the one before it). Come to by Undo or
        // Redo: the other way leads to where it came from.
        let from = self.gone_from.take();
        let was_kept = |id: &String| self.kept.iter().any(|(kept, ..)| kept == id);
        let mut ahead = None;
        match self.going.take().filter(|(to, _)| to == id) {
            Some((_, true)) => (self.left, ahead) = (None, from),
            Some((_, false)) => self.left = from.filter(was_kept),
            None => {
                if let Some(from) = from.filter(was_kept) {
                    self.left = Some(from);
                }
            }
        }
        if self.left.as_deref() == Some(id) {
            self.left = None;
        }
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
        self.ahead = ahead.map(|to| (to, editor.made()));
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
        let keys = self.double_click_keys(kind, keys);
        self.mouse_meant(kind, x, y, keys)
    }

    /// The keys as their meanings. A screen can't say which keys a double-click was made with and sends back the
    /// key lamps, which are meanings already: the double-click goes by the keys of the press before it.
    fn double_click_keys(&self, kind: i32, keys: Keys) -> Keys {
        if kind == mouse::DOUBLE_CLICKED { self.keys } else { self.key_map.roles(keys) }
    }

    /// [`Card::mouse`] with the keys already as their meanings (free, bend, make): the ruler and the strip hand
    /// their drags on to it.
    fn mouse_meant(&mut self, kind: i32, x: f64, y: f64, keys: Keys) -> bool {
        let s = self;
        let frame = s.frame();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        let (was_keys, was_mouse) = (s.keys, s.mouse);
        if kind < mouse::LEFT {
            s.keys = keys;
            s.mouse = Some((x, y));
        }
        // The curve was changed some other way since points were picked: nothing is picked.
        if !s.picked.is_empty() && editor.shape().points() != s.picked_of.as_slice() {
            s.picked.clear();
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
                    if !matches!(held, Held::Sweep(_) | Held::Band) && s.before.is_none() {
                        s.before = Some(editor.shape().clone());
                    }
                }
                let length = s.before.as_ref().map_or_else(|| editor.shape().length(), Shape::length);
                // The free key pressed or let go half way through: the handle goes on from where it has got to.
                if let (Held::Part(hold), Some((_, was_y))) = (&mut *held, was_mouse)
                    && was_keys.shift != keys.shift
                {
                    *hold = hold.fine_changed(frame, was_y, was_keys.shift);
                }
                match *held {
                    Held::Touch(grab) => {
                        let before = s.before.as_ref().unwrap_or_else(|| editor.shape());
                        let (edit, words) = curve_touch::drag(grab, before, frame, s.grid, keys, x, y);
                        // One of several picked points: they all go, by as much as this one.
                        let together = match (grab, &edit) {
                            (Grab::Dot { point, .. }, Edit::MovePoint { beat, value, .. })
                                if s.picked_from.len() > 1 && s.picked_from.contains(&point) =>
                            {
                                before.points().get(point).map(|p| {
                                    let points = s.picked_from.clone();
                                    (point, Edit::MovePoints { points, beats: beat - p.beat, value: value - p.value })
                                })
                            }
                            _ => None,
                        };
                        if let Some((point, edit)) = together {
                            let before = before.clone();
                            editor.drag(&edit);
                            let now = curve_edit::landed(&before, None, &edit, editor.shape());
                            s.held_dot = now.iter().find(|(from, _)| *from == point).map(|(_, to)| *to);
                            s.picked = now.iter().map(|(_, to)| *to).collect();
                            s.picked.dedup();
                            s.picked_of = editor.shape().points().to_vec();
                            let held = s.held_dot.and_then(|i| editor.shape().points().get(i));
                            s.read_out = held.map_or_else(String::new, |p| {
                                let place = curve_touch::place_words(p.beat);
                                format!("{} points · {place} · {}", s.picked_from.len(), curve_touch::percent(p.value))
                            });
                        } else {
                            s.read_out = words;
                            editor.drag(&edit);
                            if matches!(grab, Grab::Dot { .. }) {
                                s.held_dot =
                                    s.before.as_ref().and_then(|b| curve_touch::held_point(editor.shape(), b, &edit));
                                // The one point in hand stays the picked one.
                                s.picked = s.held_dot.into_iter().collect();
                                s.picked_of = editor.shape().points().to_vec();
                            }
                        }
                    }
                    Held::Band => {
                        let mut picked = curve_touch::in_box(editor.shape(), frame, *from, (x, y));
                        picked.extend(s.picked_from.iter().copied());
                        picked.sort_unstable();
                        picked.dedup();
                        s.read_out = match picked.len() {
                            1 => "1 point".to_owned(),
                            n => format!("{n} points"),
                        };
                        s.picked = picked;
                        s.picked_of = editor.shape().points().to_vec();
                        s.band = Some((*from, (x, y)));
                        return false;
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
            // Ctrl on empty space paints: the stroke begins under the press, with no wait for a drag. Straight
            // after a stroke it paints on the curve too: the mouse is on the step just painted, by its corners.
            mouse::PRESSED
                if keys.ctrl
                    && match curve_hold::over(editor.shape(), editor.highlight(), frame, x, y) {
                        Over::Inside | Over::Empty => true,
                        Over::Dot(_) | Over::Line(_) => just_painted(s.painted, x, y),
                        Over::Part(_) => false,
                    } =>
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
            // Alt on empty space draws a box: the points in it are picked (with Shift, as well as those picked).
            mouse::PRESSED
                if keys.alt
                    && matches!(
                        curve_hold::over(editor.shape(), editor.highlight(), frame, x, y),
                        Over::Inside | Over::Empty
                    ) =>
            {
                s.picked_from = if keys.shift { s.picked.clone() } else { Vec::new() };
                s.dropped = None;
                s.lit_whole = false;
                s.held_dot = None;
                s.grab = Some((Held::Band, (x, y), false));
                false
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
                // A point that isn't one of the picked ones is the picked one from here on (with Ctrl it joins
                // them, once the click is let go).
                if let Some(point) = s.held_dot
                    && !keys.ctrl
                    && !s.picked.contains(&point)
                {
                    s.picked = vec![point];
                    s.picked_of = editor.shape().points().to_vec();
                }
                s.picked_from = s.picked.clone();
                false
            }
            mouse::LET_GO => {
                let was = s.grab.take();
                s.painted = matches!(was, Some((Held::Paint, ..))).then(|| (Instant::now(), (x, y)));
                s.before = None;
                s.ghost = None;
                s.stroke = None;
                s.band = None;
                s.swing = 0.0;
                s.read_out.clear();
                match was {
                    // A box leaves its points picked; a click with the box's key picks nothing.
                    Some((Held::Band, _, moved)) => {
                        if !moved {
                            s.picked.clear();
                        }
                        false
                    }
                    // A click on a point picks it: alone, or with Ctrl in or out of the picked ones.
                    Some((Held::Touch(Grab::Dot { point, .. }), _, false)) => {
                        // A double-click begins with such a click: it's done to the points picked before it.
                        s.clicked = Some((s.picked.clone(), Instant::now()));
                        if !keys.ctrl {
                            s.picked = vec![point];
                        } else if let Some(at) = s.picked.iter().position(|p| *p == point) {
                            s.picked.remove(at);
                        } else {
                            s.picked.push(point);
                            s.picked.sort_unstable();
                        }
                        s.picked_of = editor.shape().points().to_vec();
                        false
                    }
                    Some((Held::Sweep(lit), _, moved)) => {
                        // A click on empty space drops the highlight and the picked points; a drag has left its
                        // own highlight.
                        if !moved {
                            s.picked.clear();
                            editor.set_highlight(None);
                            if let Some(lit) = lit {
                                s.dropped = Some((lit, Instant::now(), (x, y)));
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
                // A screen calls a quick click and then a drag (or a second stroke of paint) a double-click too,
                // just before the drag is let go: it's a drag, and letting go ends it as ever.
                if matches!(s.grab, Some((_, _, true))) {
                    s.clicked = None;
                    s.dropped = None;
                    return false;
                }
                s.grab = None;
                s.before = None;
                s.ghost = None;
                s.stroke = None;
                s.swing = 0.0;
                s.read_out.clear();
                // Two quick strokes of paint are two strokes: nothing is added or removed under them.
                if keys.ctrl && just_painted(s.painted, x, y) {
                    s.dropped = None;
                    s.clicked = None;
                    return false;
                }
                // The click it began with dropped the highlight: it's put back.
                let back = s
                    .dropped
                    .take()
                    .filter(|(_, when, at)| when.elapsed() < DOUBLE_CLICK && (at.0 - x).hypot(at.1 - y) < SAME_SPOT);
                if let (Some((lit, ..)), None) = (back, editor.highlight()) {
                    editor.set_highlight(Some(lit));
                }
                // A handle of the highlight, or a pill round the whole curve, is nothing to double-click: no point
                // is added under it.
                let over = curve_hold::over(editor.shape(), editor.highlight(), frame, x, y);
                if matches!(over, Over::Part(_)) {
                    return back.is_some();
                }
                // On one of several picked points: they all go.
                // (As they were before the click this double-click began with, which picked this one alone.)
                let before = s.clicked.take().filter(|(_, when)| when.elapsed() < DOUBLE_CLICK).map(|(p, _)| p);
                let all = before.unwrap_or_else(|| s.picked.clone());
                let edit = match curve_touch::double_click(editor.shape(), frame, s.grid, keys, x, y) {
                    Some(Edit::RemovePoint(point)) if all.len() > 1 && all.contains(&point) => {
                        s.picked.clear();
                        Some(Edit::RemovePoints(all))
                    }
                    edit => edit,
                };
                let added = edit.is_some_and(|e| editor.edit(&e));
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
                s.band = None;
                let given = given_up(editor, was, &mut s.lit_whole);
                // The curve is as it was at the press, and so is what's picked.
                if matches!(was, Some((Held::Touch(_) | Held::Band, ..))) {
                    s.picked = std::mem::take(&mut s.picked_from);
                    s.picked_of = editor.shape().points().to_vec();
                }
                given
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
        let keys = self.key_map.roles(keys);
        let mut drawn = false;
        let dragging = matches!(self.grab, Some((Held::Swing(..), _, _)));
        if dragging && matches!(kind, mouse::MOVED | mouse::LET_GO | mouse::TAKEN_AWAY) {
            drawn = self.mouse_meant(kind, x, 0.0, keys);
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
        let keys = self.double_click_keys(kind, keys);
        // A drag begun here goes on as the bar's. The picture isn't told the mouse is on it for longer than that.
        if self.grab.is_some() && matches!(kind, mouse::MOVED | mouse::LET_GO | mouse::TAKEN_AWAY) {
            let drawn = self.mouse_meant(kind, x, BAR_Y, keys);
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
            // A quick click and then a drag of a clip is a drag: letting go ends it.
            (mouse::DOUBLE_CLICKED, _) if matches!(s.grab, Some((_, _, true))) => false,
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
        let (grid, picked) = (self.grid.beats(), self.picked_now());
        self.open
            .as_ref()
            .map_or_else(Vec::new, |(_, e)| curve_strip::part_offers(e.shape(), e.highlight(), grid, picked))
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
        // The picked points, if the curve is still the one they were picked in.
        let picked = if editor.shape().points() == s.picked_of.as_slice() { s.picked.clone() } else { Vec::new() };
        match id {
            // With the grid off there's nothing to quantise to.
            curve_act::QUANTISE if s.grid.beats().is_none() => false,
            curve_act::QUANTISE => {
                let edit = Edit::Quantise { points: picked.clone(), step };
                let before = editor.shape().clone();
                let done = editor.edit(&edit);
                // The picked points stay picked where they went.
                if done && !picked.is_empty() {
                    s.picked =
                        curve_edit::landed(&before, lit, &edit, editor.shape()).into_iter().map(|p| p.1).collect();
                    s.picked.dedup();
                    s.picked_of = editor.shape().points().to_vec();
                }
                done
            }
            curve_act::REMOVE_POINTS => {
                let done = editor.edit(&Edit::RemovePoints(picked));
                if done {
                    s.picked.clear();
                }
                done
            }
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
            curve_act::NUDGE_LEFT | curve_act::NUDGE_RIGHT => {
                let by = s.grid.beats().unwrap_or(curve_edit::SLIGHTLY);
                editor.edit(&Edit::Nudge { by: if id == curve_act::NUDGE_LEFT { -by } else { by } })
            }
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

    /// A curve from the list is added to the one in the editor (the + on its line): in place of the highlighted
    /// part, fitted to its length, or with nothing highlighted at the curve's end, where it lands as a clip of its
    /// own. One step of undo. One added at the end isn't left highlighted, so the next + joins on after it. When it
    /// isn't added, why, in words that end "X wasn't added: ...". Draw the picture again either way: a drag in hand
    /// is given up first.
    pub fn add(&mut self, piece: Piece) -> Result<(), &'static str> {
        let s = self;
        let was = s.grab;
        s.hands_off();
        s.mouse = None;
        let Some((_, editor)) = s.open.as_mut() else { return Err("no curve is open in the editor") };
        given_up(editor, was, &mut s.lit_whole);
        // As Randomise has it: a curve of one point or none is nothing to add.
        if piece.points.len() < 2 {
            return Err("it has no shape to add");
        }
        let before = editor.shape().length();
        if editor.highlight().is_some() {
            if !editor.edit(&Edit::Swap { piece, keep_length: false }) {
                return Err("the highlighted part is that curve already");
            }
        } else {
            if !editor.edit(&Edit::Insert { at: before, piece }) {
                return Err("the joined curve would be too long, or this one is too short to add");
            }
            editor.set_highlight(None);
            s.lit_whole = false;
        }
        // Longer now: the whole of it in sight.
        let length = editor.shape().length();
        if (length - before).abs() > 1e-9 {
            s.view = View::whole(length);
        }
        Ok(())
    }

    /// "Swing as the song" (the plugin's button): every second line of a grid `step` beats apart moves by `amount`
    /// of a step, in the highlighted part or the whole curve. One step of undo. Draw the picture again whatever it
    /// says: a drag in hand is given up first.
    ///
    /// While the curve's points are still as this button's last swing left them, the song's swing takes that
    /// swing's place, never on top of it: the curve as it was before is swung again, in each part the button was
    /// pressed for (and the one highlighted now), and clips kept since stay.
    pub fn swing_as_song(&mut self, step: f64, amount: f64) -> SongSwing {
        let s = self;
        let was = s.grab;
        s.hands_off();
        s.mouse = None;
        let Some((id, editor)) = s.open.as_mut() else { return SongSwing::Nothing };
        given_up(editor, was, &mut s.lit_whole);
        let edit = Edit::Swing { step, amount };
        // (A highlight of the whole curve is the whole curve, as no highlight is.)
        let whole = editor.shape().whole();
        let lit = editor.highlight().filter(|l| !l.same(whole));
        let ours = s.swung.take().filter(|w| {
            w.curve == *id
                && w.left.points() == editor.shape().points()
                && (w.left.length() - editor.shape().length()).abs() < 1e-9
        });
        let Some(w) = ours else {
            let before = editor.shape().clone();
            if !editor.edit(&edit) {
                return SongSwing::Nothing;
            }
            let left = editor.shape().clone();
            s.swung = Some(Swung { curve: id.clone(), before, parts: vec![lit], left, step, amount });
            return SongSwing::Done;
        };
        // The part highlighted now has the button's swing already when it's inside a part it was pressed for.
        let inside = |part: &Option<Span>| match (part, lit) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(p), Some(l)) => p.start <= l.start + 1e-9 && l.end <= p.end + 1e-9,
        };
        let covered = w.parts.iter().any(inside);
        if covered && (w.step - step).abs() < 1e-9 && (w.amount - amount).abs() < 1e-9 {
            s.swung = Some(w);
            return SongSwing::Already;
        }
        // Each stretch of time is swung once: parts the new highlight covers or overlaps become one part with it
        // (swung one after the other, where they overlap the lines moved twice: the 19 Nov 2026 review's CU-4).
        let parts = match (covered, lit) {
            (true, _) => w.parts.clone(),
            (false, None) => vec![None],
            (false, Some(lit)) => {
                let (mut joined, mut apart) = (lit, w.parts.iter().copied().flatten().collect::<Vec<Span>>());
                while let Some(at) =
                    apart.iter().position(|p| p.start < joined.end - 1e-9 && joined.start < p.end - 1e-9)
                {
                    let p = apart.remove(at);
                    joined = Span::new(joined.start.min(p.start), joined.end.max(p.end));
                }
                apart.into_iter().chain([joined]).map(Some).collect()
            }
        };
        let mut swung = w.before.clone();
        let mut any = false;
        for part in &parts {
            if let Some((next, _)) = curve_edit::apply(&swung, *part, &edit) {
                swung = next;
                any = true;
            }
        }
        if !any || !editor.put(swung.with_clips_of(editor.shape()), editor.highlight()) {
            s.swung = Some(w);
            return if any { SongSwing::Already } else { SongSwing::Nothing };
        }
        s.swung = Some(Swung { parts, left: editor.shape().clone(), step, amount, ..w });
        SongSwing::Done
    }

    /// Esc: a drag in hand is put back as it was before it began; else the picked points are let go; else the
    /// highlight is dropped. Whether it did any of them.
    pub fn escape(&mut self) -> bool {
        let s = self;
        let was = s.grab;
        s.hands_off();
        let Some((_, editor)) = s.open.as_mut() else { return false };
        let same = |picked_of: &[Point], editor: &Editor| editor.shape().points() == picked_of;
        if given_up(editor, was, &mut s.lit_whole) {
            // The curve is as it was at the press, and so is what's picked (a drag of points or a box changed it;
            // after any other drag it's as it was by itself).
            if matches!(was, Some((Held::Touch(_) | Held::Band, ..))) {
                s.picked = std::mem::take(&mut s.picked_from);
                s.picked_of = editor.shape().points().to_vec();
            }
            true
        } else if matches!(was, Some((Held::Band, ..))) {
            s.picked = std::mem::take(&mut s.picked_from);
            s.picked_of = editor.shape().points().to_vec();
            true
        } else if !s.picked.is_empty() && same(&s.picked_of, editor) {
            s.picked.clear();
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
        self.can_step_here(back)
    }

    /// Undo (`back`) or Redo; whether a step was taken. What was in hand is let go of either way.
    pub fn step_back(&mut self, back: bool) -> bool {
        let s = self;
        s.hands_off();
        s.lit_whole = false;
        s.asked = Some(Instant::now());
        let Some((_, e)) = s.open.as_mut() else { return false };
        let was = e.shape().length();
        // A drag in hand is a step of its own: Undo takes back that, not it and the step before it as well. (Redo
        // gives the drag up, as ever: keeping it would throw the steps to redo away.)
        if back {
            e.let_go();
        }
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

    /// A curve's edits were saved as a new curve (`saved`: its points and length as they were saved): the curve
    /// they were made on is as it was found again, on the card or kept for later. Whether it's the one on the card
    /// (to be drawn again). Edited further while the save was on its way, it stays as it is, with its undo steps:
    /// those edits are in neither curve yet.
    pub fn forget(&mut self, id: &str, saved: &(Vec<CurvePoint>, f64)) -> bool {
        // (One kept for later too: only while it holds what was saved. The 19 Nov 2026 review's CU-7.)
        self.kept.retain(|(kept, _, editor, _)| {
            kept != id || (editor.shape().curve_points(), editor.shape().length()) != *saved
        });
        let open = self.open.as_ref().is_some_and(|(open, _)| open == id) && self.edited().as_ref() == Some(saved);
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
        // Not in the middle of a drag: a drag goes by where it was pressed in the picture as it's in sight.
        if self.open.is_none() || self.grab.is_some() || !by.is_finite() || by == 0.0 {
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
            (Some(Held::Sweep(_) | Held::Paint | Held::Swing(..) | Held::Band), _) => Over::Empty,
            (None, Some((x, y))) => curve_hold::over_in(shape, hl, frame, x, y),
            _ => Over::Empty,
        };
        // The picked points, if the curve is still the one they were picked in.
        let picked: &[usize] = if shape.points() == s.picked_of.as_slice() { &s.picked } else { &[] };
        let near = match (held, over, s.mouse) {
            (Some(_), Over::Dot(point), _) => Some(point),
            (Some(_), ..) => None,
            (None, _, Some((x, y))) => match curve_touch::under(shape, frame, x, y) {
                curve_touch::Under::Dot(point) => Some(point),
                _ => None,
            },
            (None, _, None) => None,
        };
        let dots: Vec<Dot> = curve_touch::dots_also(shape, frame, near, picked)
            .into_iter()
            .map(|d| {
                let hot = over == Over::Dot(d.point);
                Dot {
                    x: d.x as f32,
                    y: d.y as f32,
                    hot: hot && held.is_none(),
                    held: hot && held.is_some(),
                    picked: picked.binary_search(&d.point).is_ok(),
                }
            })
            .collect();
        let band = s.band.map(|(a, b)| {
            ((a.0.min(b.0)) as f32, (a.1.min(b.1)) as f32, (a.0 - b.0).abs() as f32, (a.1 - b.1).abs() as f32)
        });
        // Several picked points under the mouse or in hand: what's done to one is done to them all.
        let many = match over {
            Over::Dot(point) if picked.len() > 1 && picked.binary_search(&point).is_ok() => Some(picked.len()),
            _ => None,
        };
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
        if let Some(n) = many {
            hint_name = format!("{n} points");
            hint_does = "drag moves them together · double-click removes them".into();
            hint_keys = if mac {
                "Shift: off the grid · Cmd-click: let this one go"
            } else {
                "Shift: off the grid · Ctrl-click: let this one go"
            }
            .into();
        }
        if held == Some(Held::Band) {
            hint_name = "Pick".into();
            hint_does = "the points in the box are picked".into();
            hint_keys = "Shift: as well as those picked".into();
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
        let can_undo = editor.can_undo() || s.way(true).is_some();
        let can_redo = editor.can_redo() || s.way(false).is_some();
        let changed = differs(editor, s.origin.as_ref());
        let new_tiles = (s.tiles.as_ref() != Some(&shown)).then(|| shown.clone());
        s.tiles = Some(shown);
        let marks = s.swing_xs();
        let new_marks = (s.swing_marks.as_ref() != Some(&marks)).then(|| marks.clone());
        s.swing_marks = Some(marks);
        Showing {
            open: true,
            dots: new_dots,
            band,
            frame: hl,
            handle,
            near: on,
            hot_line,
            ghost,
            width: frame.width as f32,
            height: frame.height as f32,
            pointer,
            hint_name,
            hint_does: s.key_map.words(&hint_does, mac),
            hint_keys: s.key_map.words(&hint_keys, mac),
            keys: s.keys,
            key_names: Role::ALL.map(|r| s.key_map.key(r).name(mac)),
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

    /// The tone was stopped: the play line follows nothing, and no tone is made again.
    pub fn unfollow(&mut self) {
        self.tone = None;
        self.heard = None;
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
        assert_eq!(card.edited(), Some(flipped.clone()));
        assert!(!card.open("a", "Renamed", &rise(), 4.0), "the same curve again only takes its name");
        // Saved as a new curve: the one it was made on is as it was found.
        // (Not while it has been edited on since the save began: those edits are in neither curve.)
        assert!(card.act(curve_act::REVERSE));
        assert!(!card.forget("a", &flipped));
        assert!(card.step_back(true));
        assert!(card.forget("a", &flipped));
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
    fn a_curve_from_the_list_lands_at_the_end_or_in_the_highlights_place() {
        let piece = || Piece { points: rise(), length_beats: 2.0, source: Some("b".into()), name: "Other".into() };
        let mut card = card();
        assert!(card.add(piece()).is_ok());
        assert!((card.length() - 6.0).abs() < 1e-9, "added at the end: 4 beats and its 2");
        assert!((card.frame().view.span - 6.0).abs() < 1e-9, "all of it in sight");
        assert!(card.highlight().is_none(), "not left highlighted: the next one joins on after it");
        let tiles = card.showing(false).tiles.expect("the first time");
        assert!(tiles.iter().any(|t| t.name == "Other"), "it lands as a clip");
        // The same one again joins on after it.
        assert!(card.add(piece()).is_ok());
        assert!((card.length() - 8.0).abs() < 1e-9);
        // With a part highlighted, another takes that part's place, fitted: the curve is no longer. (The same one
        // again would change nothing, and isn't a step.)
        let x = card.frame().x(7.0);
        card.strip(mouse::PRESSED, x, Keys::default());
        card.strip(mouse::LET_GO, x, Keys::default());
        assert_eq!(card.highlight().map(|h| (h.start, h.end)), Some((6.0, 8.0)));
        assert!(card.add(piece()).is_err());
        let fall = vec![CurvePoint { x: 0.0, y: 1.0, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.0, bend: 0.0 }];
        assert!(card.add(Piece { points: fall, length_beats: 8.0, source: None, name: "Fall".into() }).is_ok());
        assert!((card.length() - 8.0).abs() < 1e-9);
        assert!(card.step_back(true) && card.step_back(true) && card.step_back(true));
        assert!(card.edited().is_none(), "each was one step");
        // One point is no shape to add; nor is there anything to add to with no curve open.
        let dot = Piece { points: vec![CurvePoint { x: 0.0, y: 0.5, bend: 0.0 }], ..piece() };
        assert!(card.add(dot).is_err());
        assert!(Card::default().add(piece()).is_err());
    }

    /// A card with five points open: beats 0 to 4, values 0, 0.5, 0.75, 0.25, 1.
    fn five() -> Card {
        let p = |x: f64, y: f64| CurvePoint { x: x / 4.0, y, bend: 0.0 };
        let mut card = Card::default();
        card.laid_out(400.0, 200.0);
        assert!(card.open("f", "Five", &[p(0.0, 0.0), p(1.1, 0.5), p(2.0, 0.75), p(3.0, 0.25), p(4.0, 1.0)], 4.0));
        card
    }

    fn click(card: &mut Card, beat: f64, value: f64, keys: Keys) {
        let (x, y) = at(card, beat, value);
        card.mouse(mouse::MOVED, x, y, keys);
        card.mouse(mouse::PRESSED, x, y, keys);
        card.mouse(mouse::LET_GO, x, y, keys);
    }

    fn picked_dots(card: &mut Card) -> usize {
        card.show_all();
        card.showing(false).dots.expect("all given").iter().filter(|d| d.picked).count()
    }

    #[test]
    fn points_are_picked_by_a_click_with_ctrl_more_and_in_a_box() {
        let (none, ctrl) = (Keys::default(), Keys { ctrl: true, ..Keys::default() });
        let (alt, alt_shift) = (Keys { alt: true, ..none }, Keys { alt: true, shift: true, ..none });
        let mut card = five();
        click(&mut card, 1.1, 0.5, none);
        assert_eq!(card.picked(), 1);
        click(&mut card, 3.0, 0.25, ctrl);
        assert_eq!(card.picked(), 2);
        assert_eq!(picked_dots(&mut card), 2);
        assert!(card.edited().is_none(), "picking is no edit");
        // Ctrl-click on a picked one lets it go; a plain click picks only that one.
        click(&mut card, 1.1, 0.5, ctrl);
        assert_eq!(card.picked(), 1);
        click(&mut card, 2.0, 0.75, none);
        assert_eq!(card.picked(), 1);
        // A box drawn with Alt over empty space: the points in it, with Shift as well as those picked.
        let (a, b) = (at(&card, 2.5, 0.05), at(&card, 3.5, 0.5));
        card.mouse(mouse::PRESSED, a.0, a.1, alt_shift);
        card.mouse(mouse::MOVED, b.0, b.1, alt_shift);
        assert!(card.showing(false).band.is_some());
        assert_eq!(card.picked(), 2);
        card.mouse(mouse::LET_GO, b.0, b.1, alt_shift);
        assert!(card.showing(false).band.is_none());
        assert_eq!(card.picked(), 2);
        assert!(card.highlight().is_none(), "a box highlights no time");
        card.mouse(mouse::PRESSED, a.0, a.1, alt);
        card.mouse(mouse::MOVED, b.0, b.1, alt);
        card.mouse(mouse::LET_GO, b.0, b.1, alt);
        assert_eq!(card.picked(), 1, "without Shift only the box's own");
        // Esc lets them go; so does a click on empty space.
        assert!(card.escape());
        assert_eq!(card.picked(), 0);
        click(&mut card, 2.0, 0.75, none);
        click(&mut card, 0.5, 0.9, none);
        assert_eq!(card.picked(), 0);
    }

    #[test]
    fn picked_points_are_dragged_quantised_and_removed_together() {
        let (none, ctrl) = (Keys::default(), Keys { ctrl: true, ..Keys::default() });
        let mut card = five();
        click(&mut card, 1.1, 0.5, none);
        click(&mut card, 2.0, 0.75, ctrl);
        // Dragging one of them moves both, by the same (the held one goes to the grid).
        press_drag(&mut card, (2.0, 0.75), (2.5, 0.5), none);
        assert!(card.showing(false).read_out.starts_with("2 points"));
        let b = at(&card, 2.5, 0.5);
        card.mouse(mouse::LET_GO, b.0, b.1, none);
        let (points, _) = card.edited().expect("moved");
        let has = |points: &[CurvePoint], beat: f64, value: f64| {
            points.iter().any(|p| (p.x * 4.0 - beat).abs() < 1e-6 && (p.y - value).abs() < 1e-6)
        };
        assert!(has(&points, 1.6, 0.25) && has(&points, 2.5, 0.5), "{points:?}");
        assert_eq!(card.picked(), 2, "they stay picked");
        // Quantise puts the picked ones on the grid (a sixteenth), and leaves the others.
        assert!(card.act(curve_act::QUANTISE));
        let (points, _) = card.edited().expect("quantised");
        assert!(has(&points, 1.5, 0.25) && has(&points, 2.5, 0.5), "{points:?}");
        assert_eq!(card.picked(), 2);
        assert!(!card.act(curve_act::QUANTISE), "on the grid already");
        // Undo: the curve is another, so nothing is picked any more.
        assert!(card.step_back(true));
        assert_eq!(card.picked(), 0);
        assert!(!card.act(curve_act::REMOVE_POINTS), "nothing picked, nothing removed");
        // Removed together, as one step.
        click(&mut card, 3.0, 0.25, none);
        click(&mut card, 1.6, 0.25, ctrl);
        assert_eq!(card.picked(), 2);
        assert!(card.act(curve_act::REMOVE_POINTS));
        assert_eq!(card.edited().expect("removed").0.len(), 3);
        assert_eq!(card.picked(), 0);
        assert!(card.step_back(true));
        assert_eq!(card.edited().expect("back").0.len(), 5);
        // A drag given up leaves the curve and what was picked as they were.
        click(&mut card, 3.0, 0.25, none);
        click(&mut card, 1.6, 0.25, ctrl);
        let before = card.edited();
        press_drag(&mut card, (3.0, 0.25), (3.5, 0.6), none);
        assert_ne!(card.edited(), before);
        assert!(card.escape());
        assert_eq!(card.edited(), before);
        assert_eq!(card.picked(), 2);
        // With nothing picked, Quantise takes every point (the menu says so).
        assert!(card.escape());
        assert!(
            card.part_offers().iter().any(|o| o.id == curve_act::QUANTISE && o.enabled && o.text.contains("whole"))
        );
        assert!(card.part_offers().iter().any(|o| o.id == curve_act::REMOVE_POINTS && !o.enabled));
        assert!(card.act(curve_act::QUANTISE));
        // With the grid off there's nothing to quantise to.
        assert!(card.step_back(true));
        card.pick_grid(6);
        assert!(card.part_offers().iter().any(|o| o.id == curve_act::QUANTISE && !o.enabled));
        assert!(!card.act(curve_act::QUANTISE));
    }

    #[test]
    fn a_double_click_on_one_of_the_picked_points_removes_them_all() {
        let (none, ctrl) = (Keys::default(), Keys { ctrl: true, ..Keys::default() });
        let mut card = five();
        click(&mut card, 1.1, 0.5, none);
        click(&mut card, 2.0, 0.75, ctrl);
        // As a screen sends a double-click: a click, a press, then the double-click.
        click(&mut card, 2.0, 0.75, none);
        let (x, y) = at(&card, 2.0, 0.75);
        card.mouse(mouse::PRESSED, x, y, none);
        assert!(card.mouse(mouse::DOUBLE_CLICKED, x, y, none));
        assert_eq!(card.edited().expect("removed").0.len(), 3);
        assert_eq!(card.picked(), 0);
        assert!(card.step_back(true), "one step");
        assert_eq!(card.edited().map(|e| e.0.len()), None);
    }

    /// As a screen sends a quick click and then a drag: the drag's letting go comes after a double-click.
    fn click_then_drag(card: &mut Card, from: (f64, f64), to: (f64, f64), keys: Keys) {
        click(card, from.0, from.1, keys);
        press_drag(card, from, to, keys);
        let (x, y) = at(card, to.0, to.1);
        let lamps = card.showing(false).keys;
        card.mouse(mouse::DOUBLE_CLICKED, x, y, lamps);
        card.mouse(mouse::LET_GO, x, y, keys);
    }

    #[test]
    fn a_quick_click_and_then_a_drag_is_a_drag_not_a_double_click() {
        let (none, ctrl) = (Keys::default(), Keys { ctrl: true, ..Keys::default() });
        // A point: it moves and stays, and the move is one step of undo.
        let mut card = five();
        click_then_drag(&mut card, (2.0, 0.75), (2.5, 0.5), none);
        let (points, _) = card.edited().expect("moved");
        assert_eq!(points.len(), 5);
        assert!((points[2].x * 4.0 - 2.5).abs() < 1e-6 && (points[2].y - 0.5).abs() < 1e-6);
        assert!(card.step_back(true));
        assert!(card.edited().is_none());
        // Three points picked with Ctrl, then dragged by the last one clicked: all five are still there.
        let mut card = five();
        click(&mut card, 1.1, 0.5, none);
        click(&mut card, 2.0, 0.75, ctrl);
        click(&mut card, 3.0, 0.25, ctrl);
        press_drag(&mut card, (3.0, 0.25), (3.0, 0.5), none);
        let (x, y) = at(&card, 3.0, 0.5);
        card.mouse(mouse::DOUBLE_CLICKED, x, y, none);
        card.mouse(mouse::LET_GO, x, y, none);
        assert_eq!(card.edited().expect("moved").0.len(), 5);
        assert_eq!(card.picked(), 3);
        // A sweep from empty space: a highlight and no new point.
        let mut card = five();
        click_then_drag(&mut card, (0.5, 0.97), (3.5, 0.97), none);
        assert!(card.edited().is_none());
        assert!(card.open.as_ref().expect("open").1.highlight().is_some());
        // The highlight's bar: the move is kept, as its own step of undo.
        let mut card = five();
        press_drag(&mut card, (1.0, 0.97), (2.0, 0.97), none);
        let (x, _) = at(&card, 2.0, 0.97);
        card.mouse(mouse::LET_GO, x, BAR_Y, none);
        let x0 = at(&card, 1.5, 0.0).0;
        card.mouse(mouse::MOVED, x0, BAR_Y, none);
        card.mouse(mouse::PRESSED, x0, BAR_Y, none);
        card.mouse(mouse::LET_GO, x0, BAR_Y, none);
        card.mouse(mouse::PRESSED, x0, BAR_Y, none);
        let x1 = at(&card, 2.5, 0.0).0;
        card.mouse(mouse::MOVED, x1, BAR_Y, none);
        card.mouse(mouse::DOUBLE_CLICKED, x1, BAR_Y, none);
        card.mouse(mouse::LET_GO, x1, BAR_Y, none);
        let moved = card.edited().expect("the part moved");
        click(&mut card, 0.2, 0.97, none);
        assert_eq!(card.edited().expect("still moved"), moved);
        assert!(card.step_back(true));
        assert!(card.edited().is_none(), "the move was one step");
    }

    #[test]
    fn undo_goes_back_to_the_curve_that_was_left_and_redo_forward_again() {
        let mut card = card();
        card.set_goes_back(true);
        let none = Keys::default();
        // An edit to the first curve, then another curve picked (by accident).
        press_drag(&mut card, (0.0, 0.0), (0.0, 0.5), none);
        let (x, y) = at(&card, 0.0, 0.5);
        card.mouse(mouse::LET_GO, x, y, none);
        let edited = card.edited().expect("edited");
        assert_eq!(card.back_to(true), None, "its own step comes first");
        let other = [CurvePoint { x: 0.0, y: 1.0, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.0, bend: 0.0 }];
        assert!(card.open("b", "Fall", &other, 4.0));
        // Nothing of its own to undo: Undo leads back, and the button is lit.
        assert!(!card.can_step(true));
        assert!(card.showing(false).can_undo);
        assert_eq!(card.back_to(true), Some("a"));
        // (A drag in hand is a step of its own: Undo takes that back before it goes anywhere.)
        press_drag(&mut card, (0.0, 1.0), (0.0, 0.5), none);
        assert_eq!(card.back_to(true), None);
        assert!(card.escape());
        assert_eq!(card.back_to(true), Some("a"));
        // A key held down (Undo asked for a moment ago) stops here; pressed again, it goes back.
        let now = Instant::now();
        card.asked = Some(now);
        assert_eq!(card.go_at(true, now + KEY_HELD / 2), None);
        assert_eq!(card.go_at(true, now + KEY_HELD * 2).as_deref(), Some("a"));
        assert!(card.open("a", "Rise", &rise(), 4.0));
        assert_eq!(card.edited().as_ref(), Some(&edited), "as it was left, edits and all");
        assert!(card.can_step(true), "and its own steps are there to undo");
        // Redo, once this curve has nothing to redo, goes forward to the other again; Undo there comes back.
        assert_eq!(card.back_to(false), Some("b"));
        assert_eq!(card.go_at(false, now + KEY_HELD * 4).as_deref(), Some("b"));
        assert!(card.open("b", "Fall", &other, 4.0));
        assert_eq!((card.back_to(true), card.back_to(false)), (Some("a"), None));
        // Back again, and a new edit there: nothing is left to redo, so Redo leads nowhere.
        assert_eq!(card.go_at(true, now + KEY_HELD * 6).as_deref(), Some("a"));
        assert!(card.open("a", "Rise", &rise(), 4.0));
        assert_eq!(card.back_to(false), Some("b"));
        assert!(card.act(curve_act::FLIP));
        assert_eq!(card.back_to(false), None);
        assert!(!card.showing(false).can_redo);
        assert!(card.open("b", "Fall", &other, 4.0));
        // A curve left with no edits is no step back; the way to the one before it stays.
        assert!(card.open("c", "Third", &other, 2.0));
        assert_eq!(card.back_to(true), Some("a"));
        // A screen that doesn't go back (the plugin's) is never told to.
        card.set_goes_back(false);
        assert_eq!(card.back_to(true), None);
        assert!(!card.showing(false).can_undo);
    }

    #[test]
    fn nudge_goes_by_the_grid_or_a_little_with_the_grid_off() {
        let mut card = card();
        assert!(card.act(curve_act::NUDGE_RIGHT));
        let (points, _) = card.edited().expect("moved");
        let step = card.grid().beats().expect("the grid is on");
        assert!(points.iter().any(|p| (p.x * 4.0 - step).abs() < 1e-9), "the start is a grid square later");
        assert!(card.act(curve_act::NUDGE_LEFT));
        assert_eq!(card.edited(), None, "and back where it was");
        assert!(
            card.part_offers().iter().any(|o| o.id == curve_act::NUDGE_LEFT && o.enabled && o.text.contains("grid"))
        );
    }

    #[test]
    fn a_curve_read_again_at_another_length_is_shown_as_found_unless_its_been_worked_on() {
        let mut card = card();
        let shown = card.shown();
        // The same id with the same shape: nothing changes. With another length: the card shows that.
        assert!(!card.open("a", "Rise", &rise(), 4.0));
        assert!(card.open("a", "Rise", &rise(), 8.0));
        assert!((card.length() - 8.0).abs() < 1e-9 && card.shown() != shown && card.edited().is_none());
        // Worked on, it stays as it is: the edits were made on what's in sight.
        assert!(card.act(curve_act::FLIP));
        assert!(!card.open("a", "Rise", &rise(), 4.0));
        assert!((card.length() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn a_curve_kept_with_edits_made_after_the_save_keeps_them_when_the_save_lands() {
        let mut card = card();
        assert!(card.act(curve_act::FLIP));
        let saved = card.edited().expect("edited");
        // One more edit after the save was asked for, then another curve is picked before the save lands.
        assert!(card.act(curve_act::NUDGE_RIGHT));
        let later = card.edited().expect("edited");
        assert!(card.open("b", "Other", &rise(), 2.0));
        assert!(!card.forget("a", &saved));
        assert!(card.open("a", "Rise", &rise(), 4.0));
        assert_eq!(card.edited().as_ref(), Some(&later), "the later edit is still there");
        // Kept just as it was saved, it comes back as found.
        assert!(card.open("b", "Other", &rise(), 2.0));
        assert!(!card.forget("a", &later));
        assert!(card.open("a", "Rise", &rise(), 4.0));
        assert_eq!(card.edited(), None);
    }

    #[test]
    fn undo_in_the_middle_of_a_drag_takes_back_the_drag_alone() {
        let none = Keys::default();
        let mut card = five();
        press_drag(&mut card, (4.0, 1.0), (4.0, 0.5), none);
        let (x, y) = at(&card, 4.0, 0.5);
        card.mouse(mouse::LET_GO, x, y, none);
        let first = card.edited().expect("the end moved");
        press_drag(&mut card, (2.0, 0.75), (2.0, 0.25), none);
        assert!(card.step_back(true));
        assert_eq!(card.edited().expect("the first move stays"), first);
        // Redo in the middle of a drag gives the drag up and redoes: the step to redo isn't thrown away.
        assert!(card.step_back(true));
        press_drag(&mut card, (2.0, 0.75), (2.0, 0.25), none);
        assert!(card.step_back(false));
        assert_eq!(card.edited().expect("redone"), first);
    }

    #[test]
    fn the_wheel_waits_for_a_drag_to_end() {
        let none = Keys::default();
        let mut card = five();
        press_drag(&mut card, (1.0, 0.97), (3.0, 0.97), none);
        assert!(!card.wheel(200.0, 180.0, false));
        let (x, y) = at(&card, 3.0, 0.97);
        card.mouse(mouse::LET_GO, x, y, none);
        assert!(card.wheel(200.0, 180.0, false));
    }

    #[test]
    fn a_double_click_on_a_pill_round_the_whole_curve_adds_no_point() {
        let none = Keys::default();
        let mut card = card();
        // The pill in the middle of the top edge.
        let frame = card.frame();
        let (x, y) = (frame.x(2.0), frame.y(1.0));
        let shape = card.open.as_ref().expect("open").1.shape().clone();
        assert!(matches!(curve_hold::over(&shape, None, frame, x, y), Over::Part(_)), "a pill is there");
        click(&mut card, 0.0, 0.0, none);
        card.mouse(mouse::MOVED, x, y, none);
        card.mouse(mouse::PRESSED, x, y, none);
        card.mouse(mouse::LET_GO, x, y, none);
        card.mouse(mouse::PRESSED, x, y, none);
        card.mouse(mouse::DOUBLE_CLICKED, x, y, none);
        card.mouse(mouse::LET_GO, x, y, none);
        assert!(card.edited().is_none());
    }

    #[test]
    fn a_dropped_highlight_stays_dropped_after_a_double_click_somewhere_else() {
        let none = Keys::default();
        let mut card = five();
        press_drag(&mut card, (2.25, 0.97), (2.75, 0.97), none);
        let (x, y) = at(&card, 2.75, 0.97);
        card.mouse(mouse::LET_GO, x, y, none);
        assert!(card.open.as_ref().expect("open").1.highlight().is_some());
        click(&mut card, 3.6, 0.97, none);
        assert!(card.open.as_ref().expect("open").1.highlight().is_none());
        // A quick double-click on the point at beat 1.1 removes it, and brings no highlight back.
        click(&mut card, 1.1, 0.5, none);
        let (x, y) = at(&card, 1.1, 0.5);
        card.mouse(mouse::PRESSED, x, y, none);
        assert!(card.mouse(mouse::DOUBLE_CLICKED, x, y, none));
        assert_eq!(card.edited().expect("removed").0.len(), 4);
        assert!(card.open.as_ref().expect("open").1.highlight().is_none());
    }

    #[test]
    fn the_free_key_pressed_half_way_through_a_squash_goes_on_from_there() {
        let (none, shift) = (Keys::default(), Keys { shift: true, ..Keys::default() });
        let mut card = card();
        let frame = card.frame();
        let x = frame.x(2.0);
        let top = |card: &Card| card.edited().map_or(1.0, |(points, _)| points.iter().map(|p| p.y).fold(0.0, f64::max));
        card.mouse(mouse::MOVED, x, frame.y(1.0), none);
        card.mouse(mouse::PRESSED, x, frame.y(1.0), none);
        card.mouse(mouse::MOVED, x, frame.y(0.5), none);
        assert!((top(&card) - 0.5).abs() < 0.01, "{}", top(&card));
        // Shift, and the smallest move: it stays where it is, and from there it's finer.
        card.mouse(mouse::MOVED, x, frame.y(0.5) + 0.1, shift);
        assert!((top(&card) - 0.5).abs() < 0.01, "{}", top(&card));
        card.mouse(mouse::MOVED, x, frame.y(0.3), shift);
        assert!((top(&card) - 0.45).abs() < 0.01, "{}", top(&card));
    }

    #[test]
    fn two_quick_strokes_of_paint_side_by_side_are_two_steps() {
        let ctrl = Keys { ctrl: true, ..Keys::default() };
        let mut card = Card::default();
        card.laid_out(700.0, 200.0);
        assert!(card.open("a", "Rise", &rise(), 16.0));
        // The 1/16 grid: cells of about 10 points.
        card.pick_grid(2);
        let (x, y) = at(&card, 5.125, 0.8);
        card.mouse(mouse::MOVED, x, y, ctrl);
        card.mouse(mouse::PRESSED, x, y, ctrl);
        card.mouse(mouse::LET_GO, x, y, ctrl);
        let one = card.edited().expect("painted").0;
        // The second, 8 points to the right, is by the first step's corner: it paints its own cell all the same.
        let x = x + 8.0;
        card.mouse(mouse::MOVED, x, y, ctrl);
        assert!(card.mouse(mouse::PRESSED, x, y, ctrl), "it paints");
        let lamps = card.showing(false).keys;
        card.mouse(mouse::DOUBLE_CLICKED, x, y, lamps);
        card.mouse(mouse::LET_GO, x, y, ctrl);
        let two = card.edited().expect("painted").0;
        let flat = |points: &[CurvePoint], beat: f64| {
            points.iter().filter(|p| (p.x * 16.0 - beat).abs() < 1e-6 && (p.y - 0.8).abs() < 0.02).count()
        };
        assert_eq!((flat(&one, 5.0), flat(&one, 5.25)), (1, 1), "the first step: {one:?}");
        assert_eq!((flat(&two, 5.0), flat(&two, 5.5)), (1, 1), "both steps, square: {two:?}");
        assert!(card.step_back(true));
        assert_eq!(card.edited().expect("the first stroke").0, one);
    }

    #[test]
    fn a_double_click_goes_by_the_chosen_keys_of_its_press() {
        use crate::curve_keys::Key;
        let p = |x: f64, y: f64, bend: f64| CurvePoint { x, y, bend };
        let mut card = Card::default();
        card.laid_out(400.0, 200.0);
        assert!(card.open("b", "Bent", &[p(0.0, 0.0, 0.6), p(1.0, 1.0, 0.0)], 4.0));
        // Alt is the free key now, so Shift bends: a Shift-double-click on the bent line straightens it.
        card.set_keys(KeyMap::default().with(Role::Free, Key::Alt));
        let shift = Keys { shift: true, ..Keys::default() };
        let frame = card.frame();
        let x = frame.x(2.0);
        let y = frame.y(card.open.as_ref().expect("open").1.shape().value_at(2.0));
        card.mouse(mouse::MOVED, x, y, shift);
        card.mouse(mouse::PRESSED, x, y, shift);
        // The page sends the lamps with a double-click.
        let lamps = card.showing(false).keys;
        assert!(card.mouse(mouse::DOUBLE_CLICKED, x, y, lamps));
        let (points, _) = card.edited().expect("straightened");
        assert_eq!(points.len(), 2, "no point added");
        assert!(points[0].bend.abs() < 1e-9);
    }

    #[test]
    fn chosen_keys_change_what_is_held_and_what_the_hint_says() {
        use crate::curve_keys::Key;
        let mut card = five();
        // Alt is the make key now, Ctrl bends: Alt-click picks more, and a Ctrl-drag draws the box.
        card.set_keys(KeyMap::default().with(Role::Make, Key::Alt));
        let (none, alt, ctrl) =
            (Keys::default(), Keys { alt: true, ..Keys::default() }, Keys { ctrl: true, ..Keys::default() });
        click(&mut card, 1.1, 0.5, none);
        click(&mut card, 3.0, 0.25, alt);
        assert_eq!(card.picked(), 2);
        let (a, b) = (at(&card, 1.5, 0.95), at(&card, 2.5, 0.6));
        card.mouse(mouse::PRESSED, a.0, a.1, ctrl);
        card.mouse(mouse::MOVED, b.0, b.1, ctrl);
        let shown = card.showing(false);
        assert!(shown.band.is_some() && shown.keys.alt, "the bend key's lamp");
        assert_eq!(shown.key_names, ["Shift", "Ctrl", "Alt"]);
        card.mouse(mouse::LET_GO, b.0, b.1, ctrl);
        assert_eq!(card.picked(), 1);
        assert!(card.edited().is_none(), "no paint");
        let (x, y) = at(&card, 0.5, 0.9);
        card.mouse(mouse::MOVED, x, y, none);
        let hint = card.showing(false).hint_keys;
        assert!(hint.contains("Alt: paint steps") && hint.contains("Ctrl-drag: pick points"), "{hint}");
        // A drag begun on the ruler goes by the same keys: Ctrl is free now, so a swing mark held with it moves
        // freely (0.2 of a step is caught by nothing), and with no key it catches on triplet swing.
        card.set_keys(KeyMap::default().with(Role::Free, Key::Ctrl));
        assert!(card.act(curve_act::STEPS));
        let swung = |card: &mut Card, keys: Keys, by: f64| {
            let (from, to) = (card.frame().x(0.25), card.frame().x(0.25 + 0.25 * by));
            card.ruler(mouse::MOVED, from, keys);
            card.ruler(mouse::PRESSED, from, keys);
            card.ruler(mouse::MOVED, to, keys);
            let words = card.showing(false).read_out;
            card.ruler(mouse::TAKEN_AWAY, to, keys);
            words
        };
        assert_eq!(swung(&mut card, ctrl, 0.32), curve_paint::swing_words(0.32));
        assert_eq!(swung(&mut card, none, 0.32), curve_paint::swing_words(1.0 / 3.0));
    }

    #[test]
    fn a_stroke_of_paint_given_up_brings_no_old_pick_back() {
        let (none, ctrl) = (Keys::default(), Keys { ctrl: true, ..Keys::default() });
        let mut card = five();
        click(&mut card, 3.0, 0.25, none);
        click(&mut card, 0.5, 0.9, none);
        assert_eq!(card.picked(), 0);
        press_drag(&mut card, (0.4, 0.9), (0.9, 0.8), ctrl);
        assert!(card.escape());
        assert_eq!(card.picked(), 0);
    }

    #[test]
    fn the_songs_swing_takes_its_last_swings_place_and_keeps_clips_made_since() {
        let p = |x: f64, y: f64| CurvePoint { x: x / 4.0, y, bend: 0.0 };
        // A gate on the 1/16 grid, a bar long.
        let gate: Vec<CurvePoint> = (0..16)
            .flat_map(|i| [p(f64::from(i) * 0.25, f64::from(i % 2)), p(f64::from(i + 1) * 0.25, f64::from(i % 2))])
            .collect();
        let open = || {
            let mut card = Card::default();
            card.laid_out(400.0, 200.0);
            assert!(card.open("g", "Gate", &gate, 4.0));
            card
        };
        let mut once = open();
        assert_eq!(once.swing_as_song(0.25, 0.2), SongSwing::Done);
        let once = once.edited().expect("swung");

        let mut card = open();
        assert_eq!(card.swing_as_song(0.25, 0.5), SongSwing::Done);
        assert_eq!(card.swing_as_song(0.25, 0.5), SongSwing::Already);
        // A clip kept in between is a step of its own, and it stays.
        press_drag(&mut card, (1.1, 0.5), (2.1, 0.5), Keys::default());
        let (x, y) = at(&card, 2.1, 0.5);
        card.mouse(mouse::LET_GO, x, y, Keys::default());
        assert!(card.act(curve_act::KEEP_CLIP));
        assert_eq!(card.open.as_ref().expect("open").1.shape().clips().len(), 1);
        assert_eq!(card.swing_as_song(0.25, 0.2), SongSwing::Done);
        assert_eq!(card.edited().expect("swung"), once, "the new swing alone, not on top of the old one");
        assert_eq!(card.open.as_ref().expect("open").1.shape().clips().len(), 1);
        // Another part highlighted: it's swung as well, and the first keeps its swing.
        let mut parts = open();
        press_drag(&mut parts, (1.1, 0.5), (1.9, 0.5), Keys::default());
        let (x, y) = at(&parts, 1.9, 0.5);
        parts.mouse(mouse::LET_GO, x, y, Keys::default());
        assert_eq!(parts.swing_as_song(0.25, 0.2), SongSwing::Done);
        let first = parts.edited().expect("swung");
        assert_eq!(parts.swing_as_song(0.25, 0.2), SongSwing::Already);
        press_drag(&mut parts, (2.6, 0.5), (3.6, 0.5), Keys::default());
        let (x, y) = at(&parts, 3.6, 0.5);
        parts.mouse(mouse::LET_GO, x, y, Keys::default());
        assert_eq!(parts.swing_as_song(0.25, 0.2), SongSwing::Done);
        let both = parts.edited().expect("swung");
        assert_ne!(both, first);
        let early = |e: &(Vec<CurvePoint>, f64)| e.0.iter().filter(|p| p.x < 0.55).copied().collect::<Vec<_>>();
        assert_eq!(early(&both), early(&first), "the first part is as it was swung");
        // A bigger highlight round a part already swung: the same curve as one press on the bigger highlight.
        let wide = |card: &mut Card| {
            press_drag(card, (0.6, 0.5), (3.4, 0.5), Keys::default());
            let (x, y) = at(card, 3.4, 0.5);
            card.mouse(mouse::LET_GO, x, y, Keys::default());
            assert_eq!(card.swing_as_song(0.25, 0.2), SongSwing::Done);
            card.edited().expect("swung")
        };
        let mut round = open();
        press_drag(&mut round, (1.1, 0.5), (1.9, 0.5), Keys::default());
        let (x, y) = at(&round, 1.9, 0.5);
        round.mouse(mouse::LET_GO, x, y, Keys::default());
        assert_eq!(round.swing_as_song(0.25, 0.2), SongSwing::Done);
        assert_eq!(wide(&mut round), wide(&mut open()), "the part inside isn't swung twice");
        // After an edit of the points, the swing is no longer the button's to replace: the next goes on top.
        assert!(card.escape());
        press_drag(&mut card, (4.0, 1.0), (4.0, 0.5), Keys::default());
        let (x, y) = at(&card, 4.0, 0.5);
        card.mouse(mouse::LET_GO, x, y, Keys::default());
        let moved = card.edited().expect("moved");
        assert_eq!(card.swing_as_song(0.25, 0.5), SongSwing::Done);
        assert!(card.step_back(true));
        assert_eq!(card.edited().expect("the move stays"), moved);
    }

    #[test]
    fn swing_given_in_one_go_is_one_step_of_undo() {
        let mut card = card();
        // A straight rise has nothing a swing would move: steps first.
        assert!(card.act(curve_act::STEPS));
        let before = card.edited().expect("steps");
        assert_eq!(card.swing_as_song(0.25, 1.0 / 3.0), SongSwing::Done);
        assert_ne!(card.edited().expect("swung").0, before.0);
        assert!((card.length() - 4.0).abs() < 1e-9, "swing never changes the length");
        assert!(card.step_back(true));
        assert_eq!(card.edited().expect("steps again").0, before.0);
        assert_eq!(card.swing_as_song(0.25, 0.0), SongSwing::Nothing, "no swing is no step");
        assert_eq!(Card::default().swing_as_song(0.25, 0.3), SongSwing::Nothing);
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
