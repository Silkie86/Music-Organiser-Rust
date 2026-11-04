//! The card that follows the mouse while sounds or MIDI clips are dragged out of the app into a DAW (the C# app's
//! `DragPreview`). It says what's being carried: a line for each thing, at most five and then "+ 2 more", each with
//! its number when they're tracks that land in order, a small picture (a sound's waveform, a clip's notes), its name
//! and a few facts. Under the lines goes a caption ("3 sounds"), and with more than one thing a round badge with the
//! count sits on the card's top right corner.
//!
//! The card flies. When the drag starts it lifts off the page: it grows a little, fades in, slides out from under the
//! mouse to just below and right of it, and its shadow spreads as if it had been picked up. While it follows the mouse
//! it's at full strength over somewhere that takes the drop, and fainter, with a plain edge, where nothing would. When
//! the drag ends it either lands, shrinking into the mouse point while a ring pulses out from it, or, cancelled, fades
//! away where it is.
//!
//! The window that shows the card asks [`Flight::frame`] for a picture about 60 times a second, on a thread of its
//! own. To keep that cheap, the card is drawn once for each screen scale and kept, and a frame only places it (bigger
//! or smaller, as see-through as it should be) and adds the ring. When nothing moves, [`Flight::still`] says so and the
//! window only follows the mouse. Everything is drawn on the processor with tiny-skia, the words with swash in the
//! theme's own font.

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source};
use swash::shape::ShapeContext;
use swash::zeno::{Format, Vector};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::drag_out::{Card, Kind, Payload, Thumb, ThumbNote, loudest, loudness};
use crate::songs::thousands;
use crate::themes::Look;

// ---- The card's measures, in points (a point is `scale` pixels) ------------------------------------------------

/// The card: its width, the room inside its edge and how round its corners are.
const CARD_WIDTH: f32 = 280.0;
const PAD: f32 = 10.0;
const CARD_RADIUS: f32 = 12.0;
/// The most lines the card shows; the rest are counted in "+ 2 more".
const MOST_LINES: usize = 5;
/// A line: how tall (its 1-point edge, 6 inside that, then the name over the facts), the gap under it, how round its
/// corners are and the room inside its edge at the sides and at the top and bottom.
const LINE_HEIGHT: f32 = 46.0;
const LINE_GAP: f32 = 4.0;
const LINE_RADIUS: f32 = 8.0;
const LINE_PAD_X: f32 = 8.0;
const LINE_PAD_Y: f32 = 6.0;
/// A line's number (for tracks), its little picture's box and the gap after the box.
const NUMBER_WIDTH: f32 = 18.0;
const THUMB_WIDTH: f32 = 44.0;
const THUMB_HEIGHT: f32 = 28.0;
const THUMB_RADIUS: f32 = 6.0;
const THUMB_GAP: f32 = 10.0;
/// How strongly a line's colour tints its picture's box, and the room the picture keeps inside the box.
const THUMB_WASH: f32 = 0.18;
const THUMB_INSET_X: f32 = 4.0;
const THUMB_INSET_Y: f32 = 3.0;
/// Text sizes: a line's name and its facts; the numbers, "+ 2 more", the caption and the badge's count.
const TITLE_SIZE: f32 = 13.0;
const DETAIL_SIZE: f32 = 11.0;
const SMALL_SIZE: f32 = 12.0;
/// A line of text is this many times its size tall (IBM Plex's own spacing).
const LINE_SPACING: f32 = 1.3;
/// "+ 2 more" and the caption: how far in from the card's inner edge, how far below what's above them, how tall.
const NOTE_INDENT: f32 = 6.0;
const MORE_GAP: f32 = 6.0;
const CAPTION_GAP: f32 = 8.0;
const NOTE_HEIGHT: f32 = 16.0;
/// The count badge: how big, the room each side of a long count, and where its middle is (this far in from the
/// card's top right corner, so it pokes out).
const BADGE_SIZE: f32 = 26.0;
const BADGE_PAD: f32 = 7.0;
const BADGE_INSET: f32 = 3.0;
/// The most the badge counts to; past it, it says "999+".
const BADGE_MOST: usize = 999;
/// Where the card rests: its top left this far right of and below the mouse, so it never covers the drop point.
const REST_X: f32 = 16.0;
const REST_Y: f32 = 18.0;
/// Room in the picture around the mouse point for the landing ring, and right of and below the card for its shadow
/// (which falls 6 points down and fades out over about 20).
const RING_ROOM: f32 = 36.0;
const RIGHT_ROOM: f32 = 20.0;
const BOTTOM_ROOM: f32 = 26.0;
/// Room around the card in its own picture, for the badge.
const BODY_ROOM: f32 = 14.0;
/// The picture is never more than this many pixels a side: on a very big screen scale the card is drawn a little
/// smaller instead.
const MOST_PIXELS: f32 = 1200.0;
/// The most characters of a name or fact kept, and the most waveform slices and notes a little picture keeps: far
/// more than the card has room for, so a strange payload can't make drawing slow.
const MOST_CHARS: usize = 200;
const MOST_PEAKS: usize = 1024;
const MOST_NOTES: usize = 4096;

// ---- How it flies ------------------------------------------------------------------------------------------------

/// The card's shadow: lying close and sharp as it's picked up, then the full one once it's up (the C# card's box
/// shadow, "0 6 24 0" at 45% black).
const SHADOW_NEAR: Shadow = Shadow { down: 2.0, blur: 6.0 };
const SHADOW_FAR: Shadow = Shadow { down: 6.0, blur: 24.0 };
const SHADOW_DARK: f32 = 0.45;
/// How solid the card is over somewhere that takes the drop, and over somewhere that doesn't.
const FULL: f32 = 0.96;
const FAINT: f32 = 0.7;
/// How long each move takes: lifting off, changing between over a target and not, landing (the card's part of it is
/// shorter, and the dot's shorter still) and going away when cancelled.
const LIFT: Duration = Duration::from_millis(160);
const CROSS_FADE: Duration = Duration::from_millis(120);
const LAND: Duration = Duration::from_millis(320);
const LAND_FADE: Duration = Duration::from_millis(200);
const DOT_FLASH: Duration = Duration::from_millis(150);
const CANCEL: Duration = Duration::from_millis(180);
/// How big the card is (1 is its full size) as it's picked up, once it has landed and once it has settled back.
const LIFT_FROM: f32 = 0.9;
const LAND_TO: f32 = 0.6;
const CANCEL_TO: f32 = 0.96;
/// The landing ring: its radius and line width from start to end, how strong it starts, and the dot in its middle.
const RING_FROM: f32 = 6.0;
const RING_TO: f32 = 34.0;
const RING_LINE_FROM: f32 = 3.0;
const RING_LINE_TO: f32 = 1.0;
const RING_ALPHA: f32 = 0.9;
const DOT_RADIUS: f32 = 5.0;
const DOT_ALPHA: f32 = 0.6;

/// The fonts the card writes in: the theme's text family at a regular and a bold weight (mo-ui hands over the bytes
/// of the font files it ships).
#[derive(Clone, Copy, Debug)]
pub struct Fonts {
    pub regular: &'static [u8],
    pub bold: &'static [u8],
}

/// The card at one moment: premultiplied RGBA, `width` x `height` pixels, with the mouse's point at (`hot_x`,
/// `hot_y`) in it. The window that shows it is placed so that point is under the mouse.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub hot_x: i32,
    pub hot_y: i32,
}

/// How a drag ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Landed,
    Cancelled,
}

/// The card's flight: lifting off, following the mouse, then landing with a pulse or going away.
pub struct Flight {
    face: Face,
    /// When the drag started, and whether the card is over somewhere that takes the drop.
    start: Instant,
    over: Over,
    ending: Option<End>,
    /// The card drawn at the last scale, and what the last frame showed (at what scale, in what pose).
    drawn: Option<Drawn>,
    shown: Option<(f32, Pose)>,
}

impl Flight {
    pub fn new(payload: &Payload, look: &Look, fonts: Fonts, now: Instant) -> Flight {
        Flight {
            face: Face::new(payload, look, fonts),
            start: now,
            // It starts as the C# card always was: at full strength, with the accent on its edge.
            over: Over { yes: true, from: 1.0, since: now },
            ending: None,
            drawn: None,
            shown: None,
        }
    }

    /// Over somewhere that takes the drop (true) or not: the card shows it, fainter and with a plain edge when not.
    pub fn set_over_target(&mut self, yes: bool, now: Instant) {
        if self.ending.is_none() && yes != self.over.yes {
            self.over = Over { yes, from: self.over.at(now), since: now };
        }
    }

    /// The drag ended: the card lands or goes away from `now` on, starting from just as it is (even halfway through
    /// lifting off). A second call changes nothing.
    pub fn end(&mut self, how: Ending, now: Instant) {
        if self.ending.is_none() {
            self.ending = Some(End { how, at: now, from: self.flying(now) });
        }
    }

    /// The picture at `now`, drawn at `scale` pixels to a point (the scale of the screen under the mouse, 1.0 to
    /// 4.0), or None once the flight is over and the window can close.
    pub fn frame(&mut self, now: Instant, scale: f32) -> Option<Picture> {
        let pose = self.pose(now)?;
        let scale = self.fit(scale);
        let geometry = Geometry::new(&self.face.layout, scale);
        // A new scale (or the first frame): the card is drawn again, as sharp as the screen can show it.
        if self.drawn.as_ref().is_none_or(|d| d.geometry != geometry) {
            self.drawn = Drawn::new(geometry);
            self.shown = None;
        }
        let Some(drawn) = self.drawn.as_mut() else { return Some(blank(&geometry)) };
        if self.shown != Some((scale, pose)) {
            drawn.compose(&self.face, &pose);
            self.shown = Some((scale, pose));
        }
        Some(drawn.picture())
    }

    /// True when a frame at `now` and `scale` would look exactly like the last one returned (nothing animating, same
    /// scale): the window then only moves and doesn't send new pixels.
    pub fn still(&self, now: Instant, scale: f32) -> bool {
        self.pose(now).is_some_and(|pose| self.shown == Some((self.fit(scale), pose)))
    }

    /// The scale the card is drawn at: the screen's, kept to 0.5 to 4, and smaller when the picture would be too big.
    fn fit(&self, scale: f32) -> f32 {
        let scale = if scale.is_nan() { 1.0 } else { scale.clamp(0.5, 4.0) };
        let (width, height) = self.face.layout.picture();
        // A few pixels spare, as each part of the picture is rounded to whole pixels.
        scale.min((MOST_PIXELS - 4.0) / width.max(height))
    }

    /// How the card is at `now`, or None once the flight is over.
    fn pose(&self, now: Instant) -> Option<Pose> {
        match self.ending {
            None => Some(self.flying(now)),
            Some(end) => end.pose(now),
        }
    }

    /// How the card is at `now` while the drag goes on: lifting off, then following the mouse.
    fn flying(&self, now: Instant) -> Pose {
        let t = part(self.start, now, LIFT);
        let up = ease_out(t);
        let over = self.over.at(now);
        Pose {
            // Written this way round so it's exactly 1 once lifted, and the card is then laid on whole pixels.
            size: 1.0 - (1.0 - LIFT_FROM) * (1.0 - overshoot(t)),
            slide: 1.0 - up,
            settle: 1.0,
            opacity: up * mix(FAINT, FULL, over),
            lifted: up,
            over,
            ring: None,
            dot: None,
        }
    }
}

/// How the card is at one moment. Two frames in the same pose look exactly alike.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    /// How big the card is (1 is its full size), grown or shrunk about the mouse point.
    size: f32,
    /// How far it still is from its resting place: 1 right at the mouse point, 0 at rest.
    slide: f32,
    /// How big it is about its own middle, as it settles back when the drag is cancelled.
    settle: f32,
    /// How solid it is: 0 gone, 0.96 at full strength.
    opacity: f32,
    /// Its shadow: 0 lying close, 1 the full one.
    lifted: f32,
    /// Its edge: 0 the plain stroke colour (not over a target), 1 the accent.
    over: f32,
    /// How far the landing ring and the dot in it have gone (0 to 1), while they show.
    ring: Option<f32>,
    dot: Option<f32>,
}

/// Whether the card is over somewhere that takes the drop, and the change to it under way.
#[derive(Clone, Copy, Debug)]
struct Over {
    yes: bool,
    /// Where the change started from (0 not over, 1 over), and when.
    from: f32,
    since: Instant,
}

impl Over {
    /// How far over a target the card looks at `now`: 0 not, 1 fully, in between while it changes.
    fn at(&self, now: Instant) -> f32 {
        mix(self.from, if self.yes { 1.0 } else { 0.0 }, smooth(part(self.since, now, CROSS_FADE)))
    }
}

/// How the drag ended, when, and how the card was just then.
#[derive(Clone, Copy, Debug)]
struct End {
    how: Ending,
    at: Instant,
    from: Pose,
}

impl End {
    /// How the card is at `now` as it lands or goes away, or None once it's gone.
    fn pose(&self, now: Instant) -> Option<Pose> {
        let gone = now.saturating_duration_since(self.at);
        let from = self.from;
        match self.how {
            Ending::Landed => {
                if gone >= LAND {
                    return None;
                }
                // The card shrinks into the mouse point, faster and faster, and fades as it goes; the ring pulses
                // out from the point, and a dot flashes in it.
                let fade = part(self.at, now, LAND_FADE);
                Some(Pose {
                    size: from.size * mix(1.0, LAND_TO, ease_in(fade)),
                    opacity: from.opacity * (1.0 - fade),
                    ring: Some(part(self.at, now, LAND)),
                    dot: (gone < DOT_FLASH).then(|| part(self.at, now, DOT_FLASH)),
                    ..from
                })
            }
            Ending::Cancelled => {
                if gone >= CANCEL {
                    return None;
                }
                // Put back down: it sinks a little where it is and fades away.
                let t = part(self.at, now, CANCEL);
                Some(Pose {
                    settle: from.settle * mix(1.0, CANCEL_TO, ease_out(t)),
                    opacity: from.opacity * (1.0 - t),
                    ..from
                })
            }
        }
    }
}

// ---- What the card shows ---------------------------------------------------------------------------------------

/// What the card shows, made safe to draw, with the look and fonts it's drawn in.
struct Face {
    lines: Vec<Line>,
    /// How many things the drag carries (the badge's count; more than the lines shown when there are lots).
    count: usize,
    caption: String,
    numbered: bool,
    look: Look,
    regular: Option<Font>,
    bold: Option<Font>,
    layout: Layout,
}

impl Face {
    fn new(payload: &Payload, look: &Look, fonts: Fonts) -> Face {
        let lines: Vec<Line> = payload.cards.iter().take(MOST_LINES).map(|c| Line::new(c, look.accent)).collect();
        let count = payload.cards.len();
        let caption = tidy(&payload.caption);
        let layout = Layout::new(lines.len(), count > lines.len(), !caption.is_empty());
        // A font that can't be read leaves the card to the other one; with neither, it has no words.
        let (regular, bold) = (Font::new(fonts.regular), Font::new(fonts.bold));
        Face {
            lines,
            count,
            caption,
            numbered: payload.numbered,
            look: *look,
            regular: regular.or(bold),
            bold: bold.or(regular),
            layout,
        }
    }
}

/// One line on the card, as it's drawn.
struct Line {
    title: String,
    detail: String,
    kind: Kind,
    /// The colour its picture is drawn in (the accent when the card gave none).
    tint: u32,
    thumb: Thumb,
}

impl Line {
    fn new(card: &Card, accent: u32) -> Line {
        Line {
            title: tidy(&card.title),
            detail: tidy(&card.detail),
            kind: card.kind,
            tint: if card.tint & 0xFF_FFFF == 0 { accent } else { card.tint & 0xFF_FFFF },
            thumb: tidy_thumb(&card.thumb),
        }
    }
}

/// A name or a fact as the card shows it: on one line, no longer than the card could ever show, and with the Korean
/// letters swash would join out of range kept apart ([`hangul_join`]).
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut last: Option<char> = None;
    for c in text.chars().take(MOST_CHARS).map(|c| if c.is_control() { ' ' } else { c }) {
        last = match last.map(|l| hangul_join(l, c)) {
            Some(Join::Joined(joined)) => Some(joined),
            Some(Join::Wrong) => {
                // A zero-width non-joiner: shows nothing, and swash doesn't join across it.
                out.push('\u{200C}');
                Some(c)
            }
            _ => Some(c),
        };
        out.push(c);
    }
    out.trim().to_owned()
}

/// What swash 0.2's joining of Korean letters (`text/compose.rs`, as it shapes) makes of `a` then `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    /// Joined into one syllable, which may join the next letter in turn.
    Joined(char),
    /// Left alone.
    Apart,
    /// Joined out of range: a syllable without a final consonant then a loose vowel overflows (it stops the thread
    /// with the app's overflow checks on), and a loose first consonant then a final one makes something that isn't a
    /// letter at all. Text from names that mix Windows' and Mac's ways of writing Korean can hold such pairs.
    Wrong,
}

/// See [`Join`]. The numbers are Unicode's for Korean syllables and their letters (jamo), as swash uses them.
fn hangul_join(a: char, b: char) -> Join {
    const FIRST: u32 = 0x1100;
    const FIRSTS: u32 = 19;
    const VOWEL: u32 = 0x1161;
    const VOWELS: u32 = 21;
    // One before the first final consonant: swash's base for them.
    const FINAL: u32 = 0x11A7;
    const FINALS: u32 = 28;
    const SYLLABLE: u32 = 0xAC00;
    const SYLLABLES: u32 = FIRSTS * VOWELS * FINALS;
    let (a, b) = (u32::from(a), u32::from(b));
    // swash only looks at pairs whose second letter is a vowel or a final consonant.
    if !(VOWEL..FINAL + FINALS).contains(&b) {
        return Join::Apart;
    }
    if (SYLLABLE..SYLLABLE + SYLLABLES).contains(&a) {
        if !(a - SYLLABLE).is_multiple_of(FINALS) {
            return Join::Apart;
        }
        return match b.checked_sub(FINAL) {
            Some(t) => char::from_u32(a + t).map_or(Join::Wrong, Join::Joined),
            None => Join::Wrong,
        };
    }
    if (FIRST..FIRST + FIRSTS).contains(&a) {
        let vowel = b - VOWEL;
        if vowel >= VOWELS {
            return Join::Wrong;
        }
        let joined = SYLLABLE + (a - FIRST) * VOWELS * FINALS + vowel * FINALS;
        return char::from_u32(joined).map_or(Join::Wrong, Join::Joined);
    }
    Join::Apart
}

/// A line's little picture made safe to draw: no broken numbers, and no more of them than its box could show.
fn tidy_thumb(thumb: &Thumb) -> Thumb {
    match thumb {
        // Each kept slice is the loudest of the ones it stands for, so no click is lost.
        Thumb::Wave(peaks) if !peaks.is_empty() => Thumb::Wave(if peaks.len() <= MOST_PEAKS {
            peaks.iter().copied().map(loudness).collect()
        } else {
            loudest(peaks, MOST_PEAKS)
        }),
        Thumb::Notes(notes) => {
            let step = notes.len().div_ceil(MOST_NOTES).max(1);
            let kept: Vec<ThumbNote> = notes
                .iter()
                .step_by(step)
                .filter(|n| n.start.is_finite())
                .map(|n| ThumbNote {
                    start: n.start.clamp(0.0, 1.0),
                    length: if n.length.is_finite() { n.length.clamp(0.0, 1.0) } else { 0.0 },
                    key: n.key,
                })
                .collect();
            if kept.is_empty() { Thumb::None } else { Thumb::Notes(kept) }
        }
        _ => Thumb::None,
    }
}

/// Where the card's parts go down it, in points, and how tall it is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    lines: usize,
    /// The tops of "+ 2 more" and of the caption, when they show.
    more: Option<f32>,
    caption: Option<f32>,
    height: f32,
}

impl Layout {
    fn new(lines: usize, more: bool, caption: bool) -> Layout {
        let mut bottom = if lines == 0 { PAD } else { line_top(lines - 1) + LINE_HEIGHT };
        let mut more_top = None;
        if more {
            let top = if lines == 0 { bottom } else { bottom + MORE_GAP };
            more_top = Some(top);
            bottom = top + NOTE_HEIGHT;
        }
        let mut caption_top = None;
        if caption {
            let top = if bottom > PAD { bottom + CAPTION_GAP } else { bottom };
            caption_top = Some(top);
            bottom = top + NOTE_HEIGHT;
        }
        Layout { lines, more: more_top, caption: caption_top, height: bottom + PAD }
    }

    /// The whole picture's size in points: room for the ring about the mouse point, then the card and its shadow.
    fn picture(&self) -> (f32, f32) {
        (RING_ROOM + REST_X + CARD_WIDTH + RIGHT_ROOM, RING_ROOM + REST_Y + self.height + BOTTOM_ROOM)
    }
}

/// Where line `i` starts down the card, in points.
fn line_top(i: usize) -> f32 {
    PAD + i as f32 * (LINE_HEIGHT + LINE_GAP)
}

/// Where everything goes in the picture at one scale, in whole pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    scale: f32,
    width: u32,
    height: u32,
    /// The pixel the mouse is on (the same across and down).
    hot: i32,
    /// The card at rest: its top left, width and height.
    card_x: i32,
    card_y: i32,
    card_w: i32,
    card_h: i32,
    /// The room around the card in its own picture.
    room: i32,
}

impl Geometry {
    fn new(layout: &Layout, scale: f32) -> Geometry {
        let px = |points: f32| (points * scale).round() as i32;
        let room = |points: f32| (points * scale).ceil() as i32;
        let hot = room(RING_ROOM);
        let (card_x, card_y) = (hot + px(REST_X), hot + px(REST_Y));
        let (card_w, card_h) = (px(CARD_WIDTH).max(1), px(layout.height).max(1));
        Geometry {
            scale,
            width: (card_x + card_w + room(RIGHT_ROOM)).max(1) as u32,
            height: (card_y + card_h + room(BOTTOM_ROOM)).max(1) as u32,
            hot,
            card_x,
            card_y,
            card_w,
            card_h,
            room: room(BODY_ROOM),
        }
    }

    /// The size of the card's own picture: the card and the room around it.
    fn body_size(&self) -> (u32, u32) {
        ((self.card_w + 2 * self.room).max(1) as u32, (self.card_h + 2 * self.room).max(1) as u32)
    }
}

/// A picture with nothing in it, for when one can't be drawn.
fn blank(g: &Geometry) -> Picture {
    let len = g.width as usize * g.height as usize * 4;
    Picture { width: g.width, height: g.height, rgba: vec![0; len], hot_x: g.hot, hot_y: g.hot }
}

// ---- Drawing -----------------------------------------------------------------------------------------------------

/// The card drawn at one scale, kept for the frames, and the frame last made from it.
struct Drawn {
    geometry: Geometry,
    /// What's inside the card's edge (the lines, words and badge), drawn once and laid on each body.
    inside: Option<Pixmap>,
    /// The card with its edge in the stroke colour (not over a target) and in the accent (over one), each drawn the
    /// first time it's needed.
    bodies: [Option<Pixmap>; 2],
    /// The shadow lying close and the full one, likewise.
    shadows: [Option<Shade>; 2],
    /// The frame being shown.
    out: Pixmap,
}

/// A shadow drawn: its picture, drawn small (each of its pixels is `step` of the frame's), and the room around the
/// card in it (as far as its blur reaches, in its own pixels).
struct Shade {
    pixmap: Pixmap,
    step: f32,
    room: f32,
}

impl Drawn {
    fn new(geometry: Geometry) -> Option<Drawn> {
        let out = Pixmap::new(geometry.width, geometry.height)?;
        Some(Drawn { geometry, inside: None, bodies: [None, None], shadows: [None, None], out })
    }

    fn picture(&self) -> Picture {
        let g = &self.geometry;
        Picture { width: g.width, height: g.height, rgba: self.out.data().to_vec(), hot_x: g.hot, hot_y: g.hot }
    }

    /// Makes the frame for `pose`: the shadow and the card placed, the whole made as see-through as the card is,
    /// then the landing ring over it.
    fn compose(&mut self, face: &Face, pose: &Pose) {
        let g = self.geometry;
        let shadow_weights = [1.0 - pose.lifted, pose.lifted];
        let body_weights = [1.0 - pose.over, pose.over];
        for (i, (shade, &weight)) in self.shadows.iter_mut().zip(&shadow_weights).enumerate() {
            if weight > 0.0 && shade.is_none() {
                *shade = draw_shadow(&g, if i == 0 { SHADOW_NEAR } else { SHADOW_FAR }, face.look.theme.square);
            }
        }
        if self.inside.is_none() {
            self.inside = draw_inside(face, &g);
        }
        for (i, (body, &weight)) in self.bodies.iter_mut().zip(&body_weights).enumerate() {
            if weight > 0.0 && body.is_none() {
                *body = draw_body(face, &g, i == 1, self.inside.as_ref());
            }
        }

        let out = &mut self.out;
        out.fill(Color::TRANSPARENT);
        // Once a landing card has faded away, only the ring is left to draw.
        if pose.opacity > 0.0 {
            let card = card_place(pose, &g);
            // What the card covers completely: the shadow never shows there, and the two cards only differ at the
            // edge.
            let covered = covered(card, &g, face.look.theme.square);
            // The shadow falls further as the card lifts (on whole pixels: it's soft anyway). Each is drawn small and
            // stretched (see `draw_shadow`).
            let down = (mix(SHADOW_NEAR.down, SHADOW_FAR.down, pose.lifted) * g.scale).round();
            for (shade, &weight) in self.shadows.iter().zip(&shadow_weights) {
                if let Some(shade) = shade.as_ref().filter(|_| weight > 0.0) {
                    let d = g.room as f32 - shade.room * shade.step;
                    let (x, y) = (card.x + card.size * d, card.y + card.size * (d + down));
                    lay(out, &shade.pixmap, Place { x, y, size: card.size * shade.step }, weight, covered);
                }
            }
            // While the edge changes colour, the card it's changing to is laid over the one it's leaving.
            let both = body_weights[0] > 0.0 && body_weights[1] > 0.0;
            if let Some(body) = self.bodies[0].as_ref().filter(|_| body_weights[0] > 0.0) {
                lay(out, body, card, 1.0, Hole::NONE);
            }
            if let Some(body) = self.bodies[1].as_ref().filter(|_| body_weights[1] > 0.0) {
                let (weight, skip) = if both { (body_weights[1], covered) } else { (1.0, Hole::NONE) };
                lay(out, body, card, weight, skip);
            }
            // The shadow and card fade as one, the way a see-through window would.
            fade(out.data_mut(), pose.opacity);
        }
        draw_ring(out, &g, face.look.accent, pose);
    }
}

/// Where one of the card's pictures goes on the frame: its top left, and how big it's drawn (frame pixels for each of
/// its own).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    x: f32,
    y: f32,
    size: f32,
}

/// Where the card's own picture goes on the frame: slid from the mouse point to the resting place, grown or shrunk
/// about the mouse point, and about the card's own middle as it settles. At rest it's on whole pixels at full size.
fn card_place(pose: &Pose, g: &Geometry) -> Place {
    let mouse = g.hot as f32 + 0.5;
    let rest_x = (g.card_x - g.room) as f32 - pose.slide * (g.card_x - g.hot) as f32;
    let rest_y = (g.card_y - g.room) as f32 - pose.slide * (g.card_y - g.hot) as f32;
    // Its top left once grown or shrunk about the mouse point, and its middle then.
    let x = mouse + pose.size * (rest_x - mouse);
    let y = mouse + pose.size * (rest_y - mouse);
    let middle_x = x + pose.size * (g.room as f32 + g.card_w as f32 / 2.0);
    let middle_y = y + pose.size * (g.room as f32 + g.card_h as f32 / 2.0);
    Place {
        x: middle_x + pose.settle * (x - middle_x),
        y: middle_y + pose.settle * (y - middle_y),
        size: pose.size * pose.settle,
    }
}

/// Frame pixels to leave alone: from (`x0`, `y0`) up to (not including) (`x1`, `y1`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hole {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Hole {
    const NONE: Hole = Hole { x0: 0, y0: 0, x1: 0, y1: 0 };
}

/// The frame pixels the card at `place` covers completely: inside its edge (the one part the two cards differ in, and
/// up to 4 pixels wide on a big screen), with 2 more pixels for the smoothing, and clear of its round corners.
fn covered(place: Place, g: &Geometry, square: bool) -> Hole {
    let (left, top) = (place.x + place.size * g.room as f32, place.y + place.size * g.room as f32);
    let (w, h) = (place.size * g.card_w as f32, place.size * g.card_h as f32);
    let r = place.size * corner(square, CARD_RADIUS) * g.scale;
    let inset = place.size * edge_width(g.scale) + 2.0;
    let at = |v: f32| if v.is_finite() { v.max(0.0) as usize } else { 0 };
    Hole {
        x0: at((left + r + inset).ceil()),
        y0: at((top + inset).ceil()),
        x1: at((left + w - r - inset).floor()),
        y1: at((top + h - inset).floor()),
    }
}

/// Lays `picture` over `frame` at `place`, `opacity` strong (both premultiplied). On whole pixels at full size each
/// pixel is copied as it is, so the words stay sharp; grown, shrunk or between pixels, each pixel of the frame is mixed
/// from the four nearest of the picture's, so it moves smoothly. (Done here rather than with tiny-skia's
/// `draw_pixmap`, which took ten times as long and made the frames late on the bigger screens.) The frame pixels in
/// `skip` are left as they are.
fn lay(frame: &mut Pixmap, picture: &Pixmap, place: Place, opacity: f32, skip: Hole) {
    let k = if opacity.is_finite() { (opacity.clamp(0.0, 1.0) * 256.0).round() as u32 } else { 0 };
    let Place { x, y, size } = place;
    if k == 0 || !(size > 0.0 && size.is_finite() && x.abs() < 1e6 && y.abs() < 1e6) {
        return;
    }
    let (pw, ph) = (picture.width() as usize, picture.height() as usize);
    let (fw, fh) = (frame.width() as usize, frame.height() as usize);
    let from = picture.data().as_chunks::<4>().0;
    let to = frame.data_mut().as_chunks_mut::<4>().0;
    if size == 1.0 && x.fract() == 0.0 && y.fract() == 0.0 {
        // Row by row, each of the picture's pixels over the frame's under it.
        let (ox, oy) = (x as i64, y as i64);
        let (x0, x1) = (ox.clamp(0, fw as i64), (ox + pw as i64).clamp(0, fw as i64));
        let (y0, y1) = (oy.clamp(0, fh as i64), (oy + ph as i64).clamp(0, fh as i64));
        for fy in y0..y1 {
            for (a, b) in runs(x0 as usize, x1 as usize, fy as usize, skip) {
                let under = fy as usize * fw + a;
                let over = (fy - oy) as usize * pw + (a as i64 - ox) as usize;
                let n = b - a;
                let (Some(under), Some(over)) = (to.get_mut(under..under + n), from.get(over..over + n)) else {
                    continue;
                };
                for (u, o) in under.iter_mut().zip(over) {
                    blend_pixel(u, u32::from_le_bytes(*o), k);
                }
            }
        }
        return;
    }
    // Which of the picture's pixels a frame pixel's middle falls after, and how far towards the next (0 to 256). The
    // outermost pixels of each picture are always clear (each keeps room around what's drawn), so only frame pixels
    // whose four nearest are all inside it need anything.
    let source = |f: usize, at: f32, len: usize| {
        let u = (f as f32 + 0.5 - at) / size - 0.5;
        let whole = u.floor();
        (whole >= 0.0 && whole + 2.0 <= len as f32).then(|| (whole as usize, ((u - whole) * 256.0).round() as u32))
    };
    let reach = |at: f32, len: usize, most: usize| {
        (at.floor() - 1.0).clamp(0.0, most as f32) as usize
            ..(at + len as f32 * size + 1.0).ceil().clamp(0.0, most as f32) as usize
    };
    let across: Vec<(usize, usize, u32)> =
        reach(x, pw, fw).filter_map(|fx| source(fx, x, pw).map(|(i, along)| (fx, i, along))).collect();
    // Where the columns to skip start and end in that list (it's in order across).
    let (cut0, cut1) = (across.partition_point(|c| c.0 < skip.x0), across.partition_point(|c| c.0 < skip.x1));
    for fy in reach(y, ph, fh) {
        let Some((j, down)) = source(fy, y, ph) else { continue };
        let (Some(upper), Some(lower), Some(row)) =
            (from.get(j * pw..(j + 1) * pw), from.get((j + 1) * pw..(j + 2) * pw), to.get_mut(fy * fw..(fy + 1) * fw))
        else {
            continue;
        };
        let holed = (skip.y0..skip.y1).contains(&fy) && cut0 < cut1;
        let (left, right) = if holed { (&across[..cut0], &across[cut1..]) } else { (&across[..], &across[..0]) };
        for &(fx, i, along) in left.iter().chain(right) {
            let (Some([a, b]), Some([c, d]), Some(under)) = (upper.get(i..i + 2), lower.get(i..i + 2), row.get_mut(fx))
            else {
                continue;
            };
            let [a, b, c, d] = [a, b, c, d].map(|p| u32::from_le_bytes(*p));
            if (a | b | c | d) != 0 {
                blend_pixel(under, mix_pixel(mix_pixel(a, b, along), mix_pixel(c, d, along), down), k);
            }
        }
    }
}

/// The runs of frame columns from `x0` to `x1` in row `y` that are outside `skip`.
fn runs(x0: usize, x1: usize, y: usize, skip: Hole) -> [(usize, usize); 2] {
    if (skip.y0..skip.y1).contains(&y) && skip.x0 < skip.x1 {
        [(x0, skip.x0.clamp(x0, x1)), (skip.x1.clamp(x0, x1), x1)]
    } else {
        [(x0, x1), (x1, x1)]
    }
}

/// Lays one premultiplied pixel (its RGBA bytes in a number) over the frame's pixel `under`, `k` / 256 strong.
fn blend_pixel(under: &mut [u8; 4], pixel: u32, k: u32) {
    if pixel == 0 {
        return;
    }
    let pixel = if k >= 256 { pixel } else { scale_pixel(pixel, k) };
    // What shows through is what's under, times how see-through the pixel is (255 less its alpha, as 0 to 256).
    let clear = 255 - (pixel >> 24);
    if clear == 0 {
        *under = pixel.to_le_bytes();
        return;
    }
    // Premultiplied, no channel can pass 255; wrapping only so a broken pixel could never stop the app.
    *under = pixel.wrapping_add(scale_pixel(u32::from_le_bytes(*under), clear + (clear >> 7))).to_le_bytes();
}

/// A premultiplied pixel's four channels each times `k` / 256 (`k` up to 256), two at a time.
fn scale_pixel(pixel: u32, k: u32) -> u32 {
    let red_blue = (((pixel & 0x00FF_00FF) * k) >> 8) & 0x00FF_00FF;
    let green_alpha = (((pixel >> 8) & 0x00FF_00FF) * k) & 0xFF00_FF00;
    red_blue | green_alpha
}

/// From pixel `a` to pixel `b`, `t` / 256 of the way (`t` up to 256), two channels at a time.
fn mix_pixel(a: u32, b: u32, t: u32) -> u32 {
    if a == b {
        return a;
    }
    let t = t.min(256);
    let keep = 256 - t;
    let red_blue = ((((a & 0x00FF_00FF) * keep) + ((b & 0x00FF_00FF) * t)) >> 8) & 0x00FF_00FF;
    let green_alpha = ((((a >> 8) & 0x00FF_00FF) * keep) + (((b >> 8) & 0x00FF_00FF) * t)) & 0xFF00_FF00;
    red_blue | green_alpha
}

/// Makes a premultiplied picture `opacity` strong: every channel of every pixel scaled alike.
fn fade(data: &mut [u8], opacity: f32) {
    let k = if opacity.is_finite() { (opacity.clamp(0.0, 1.0) * 256.0).round() as u16 } else { 0 };
    if k >= 256 {
        return;
    }
    // In 16 bits (255 x 256 + 128 still fits), so the processor does many at once.
    for v in data.iter_mut() {
        *v = ((u16::from(*v) * k + 128) >> 8) as u8;
    }
}

/// The landing ring pulsing out from the mouse point, and the dot flashing in it.
fn draw_ring(out: &mut Pixmap, g: &Geometry, accent: u32, pose: &Pose) {
    let (s, middle) = (g.scale, g.hot as f32 + 0.5);
    if let Some(t) = pose.dot
        && let Some(dot) = PathBuilder::from_circle(middle, middle, DOT_RADIUS * s)
    {
        fill(out, &dot, accent, DOT_ALPHA * (1.0 - t));
    }
    if let Some(t) = pose.ring
        && let Some(ring) = PathBuilder::from_circle(middle, middle, mix(RING_FROM, RING_TO, ease_out(t)) * s)
    {
        let stroke = Stroke { width: mix(RING_LINE_FROM, RING_LINE_TO, ease_out(t)) * s, ..Stroke::default() };
        let alpha = RING_ALPHA * (1.0 - t) * (1.0 - t);
        out.stroke_path(&ring, &paint(accent, alpha), &stroke, Transform::identity(), None);
    }
}

/// The card with its edge: the edge's colour, the card's own inside it, then what's inside laid on top.
fn draw_body(face: &Face, g: &Geometry, over: bool, inside: Option<&Pixmap>) -> Option<Pixmap> {
    let theme = &face.look.theme;
    let (width, height) = g.body_size();
    let mut body = Pixmap::new(width, height)?;
    let card = Area { x: g.room as f32, y: g.room as f32, w: g.card_w as f32, h: g.card_h as f32 };
    let (radius, edge) = (corner(theme.square, CARD_RADIUS) * g.scale, edge_width(g.scale));
    fill_round(&mut body, card, radius, if over { face.look.accent } else { theme.stroke }, 1.0);
    fill_round(&mut body, card.inset(edge, edge), (radius - edge).max(0.0), theme.sidebar, 1.0);
    if let Some(inside) = inside {
        lay(&mut body, inside, Place { x: 0.0, y: 0.0, size: 1.0 }, 1.0, Hole::NONE);
    }
    Some(body)
}

/// The card's shadow: its shape in see-through black, blurred. The blur hides any detail, so it's drawn small (a
/// pixel for every `step` of the frame's) and stretched when it's laid: it looks the same and blurs many times
/// quicker.
fn draw_shadow(g: &Geometry, shadow: Shadow, square: bool) -> Option<Shade> {
    let sigma = shadow.sigma() * g.scale;
    let step = (sigma / 2.0).floor().clamp(1.0, 16.0);
    let (sigma, s) = (sigma / step, g.scale / step);
    // Three box blurs reach three times the blur's spread.
    let room = (3.0 * sigma).ceil() + 2.0;
    let (w, h) = (g.card_w as f32 / step, g.card_h as f32 / step);
    let mut pixmap = Pixmap::new((w + 2.0 * room).ceil() as u32, (h + 2.0 * room).ceil() as u32)?;
    let card = Area { x: room, y: room, w, h };
    fill_round(&mut pixmap, card, corner(square, CARD_RADIUS) * s, 0x000000, SHADOW_DARK);
    blur(&mut pixmap, sigma);
    Some(Shade { pixmap, step, room })
}

/// What's inside the card's edge: each line (its box, number, picture, name and facts), "+ 2 more", the caption and
/// the badge.
fn draw_inside(face: &Face, g: &Geometry) -> Option<Pixmap> {
    let (width, height) = g.body_size();
    let mut pm = Pixmap::new(width, height)?;
    let mut writer = Writer::new();
    let theme = &face.look.theme;
    let s = g.scale;
    // From points on the card to pixels in this picture, on whole pixels so edges and baselines are sharp.
    let origin = g.room as f32;
    let at = |points: f32| origin + (points * s).round();
    let edge = edge_width(s);
    for (i, line) in face.lines.iter().enumerate() {
        let top = line_top(i);
        let middle = top + LINE_HEIGHT / 2.0;
        let (x0, y0, x1, y1) = (at(PAD), at(top), at(CARD_WIDTH - PAD), at(top + LINE_HEIGHT));
        let area = Area { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
        let radius = corner(theme.square, LINE_RADIUS) * s;
        fill_round(&mut pm, area, radius, theme.stroke, 1.0);
        fill_round(&mut pm, area.inset(edge, edge), (radius - edge).max(0.0), theme.surface_raised, 1.0);

        let mut x = PAD + 1.0 + LINE_PAD_X;
        if face.numbered {
            if let Some(font) = face.bold {
                let style = Style { font, size: SMALL_SIZE * s, rgb: theme.faint };
                let words = writer.fit(&style, &(i + 1).to_string(), NUMBER_WIDTH * s);
                writer.draw(
                    &mut pm,
                    &words,
                    &style,
                    at(x),
                    (origin + middle * s + font.cap * style.size / 2.0).round(),
                );
            }
            x += NUMBER_WIDTH;
        }
        let (tx, ty) = (at(x), at(middle - THUMB_HEIGHT / 2.0));
        let thumb = Area { x: tx, y: ty, w: at(x + THUMB_WIDTH) - tx, h: at(middle + THUMB_HEIGHT / 2.0) - ty };
        draw_thumb(&mut pm, line, face, thumb, s);
        x += THUMB_WIDTH + THUMB_GAP;

        // The name over the facts, the pair in the line's middle; just the name when there are no facts.
        let room = at(CARD_WIDTH - PAD - 1.0 - LINE_PAD_X) - at(x);
        let inner_top = top + 1.0 + LINE_PAD_Y;
        let inner_height = LINE_HEIGHT - 2.0 * (1.0 + LINE_PAD_Y);
        let title_height = TITLE_SIZE * LINE_SPACING;
        let detail_height = if line.detail.is_empty() { 0.0 } else { DETAIL_SIZE * LINE_SPACING };
        let block_top = inner_top + (inner_height - title_height - detail_height) / 2.0;
        if let Some(font) = face.bold {
            let style = Style { font, size: TITLE_SIZE * s, rgb: theme.text };
            let words = writer.fit(&style, &line.title, room);
            writer.draw(&mut pm, &words, &style, at(x), at(block_top + font.baseline(TITLE_SIZE)));
        }
        if let Some(font) = face.regular.filter(|_| !line.detail.is_empty()) {
            let style = Style { font, size: DETAIL_SIZE * s, rgb: theme.muted };
            let words = writer.fit(&style, &line.detail, room);
            writer.draw(&mut pm, &words, &style, at(x), at(block_top + title_height + font.baseline(DETAIL_SIZE)));
        }
    }

    let note_x = PAD + NOTE_INDENT;
    let note_room = at(CARD_WIDTH - PAD - NOTE_INDENT) - at(note_x);
    if let (Some(top), Some(font)) = (face.layout.more, face.regular) {
        let style = Style { font, size: SMALL_SIZE * s, rgb: theme.muted };
        let more = format!("+ {} more", thousands(face.count.saturating_sub(face.lines.len())));
        let words = writer.fit(&style, &more, note_room);
        writer.draw(&mut pm, &words, &style, at(note_x), at(top + font.baseline(SMALL_SIZE)));
    }
    if let (Some(top), Some(font)) = (face.layout.caption, face.bold) {
        let style = Style { font, size: SMALL_SIZE * s, rgb: theme.text };
        let words = writer.fit(&style, &face.caption, note_room);
        writer.draw(&mut pm, &words, &style, at(note_x), at(top + font.baseline(SMALL_SIZE)));
    }
    draw_badge(&mut pm, face, g, &mut writer);
    Some(pm)
}

/// The count badge over the card's top right corner, when it carries more than one thing. A long count stretches it
/// to the left.
fn draw_badge(pm: &mut Pixmap, face: &Face, g: &Geometry, writer: &mut Writer) {
    if face.count < 2 {
        return;
    }
    let s = g.scale;
    let count = if face.count > BADGE_MOST { format!("{BADGE_MOST}+") } else { face.count.to_string() };
    let style = face.bold.map(|font| Style { font, size: SMALL_SIZE * s, rgb: face.look.accent_text });
    let words = style.as_ref().map(|style| writer.fit(style, &count, f32::INFINITY)).unwrap_or_default();
    let height = (BADGE_SIZE * s).round();
    let width = height.max((words.width + 2.0 * BADGE_PAD * s).round());
    let poke = ((BADGE_SIZE / 2.0 - BADGE_INSET) * s).round();
    let right = (g.room + g.card_w) as f32 + poke;
    let badge = Area { x: right - width, y: g.room as f32 - poke, w: width, h: height };
    let radius = if face.look.theme.square { corner(true, BADGE_SIZE / 2.0) * s } else { height / 2.0 };
    fill_round(pm, badge, radius, face.look.accent, 1.0);
    if let Some(style) = style {
        let x = (badge.x + (width - words.width) / 2.0).round();
        let baseline = (badge.y + height / 2.0 + style.font.cap * style.size / 2.0).round();
        writer.draw(pm, &words, &style, x, baseline);
    }
}

/// A line's little picture: its box tinted with the line's colour, and in it the waveform, the notes or a plain sign.
fn draw_thumb(pm: &mut Pixmap, line: &Line, face: &Face, area: Area, s: f32) {
    let theme = &face.look.theme;
    let wash = blend(theme.surface_raised, line.tint, THUMB_WASH);
    fill_round(pm, area, corner(theme.square, THUMB_RADIUS) * s, wash, 1.0);
    let inner = area.inset((THUMB_INSET_X * s).round(), (THUMB_INSET_Y * s).round());
    match &line.thumb {
        Thumb::Wave(peaks) => draw_wave(pm, peaks, inner, line.tint, s),
        Thumb::Notes(notes) => draw_notes(pm, notes, inner, line.tint, s),
        Thumb::None => draw_sign(pm, line.kind, inner, line.tint, s, theme.square),
    }
}

/// A sound's waveform: an upright line for each pixel across, about the middle, as tall as the loudest the sound
/// gets there; the loudest reaches the edges and the quietest is still a sliver (as on the Sounds page).
fn draw_wave(pm: &mut Pixmap, peaks: &[f32], area: Area, rgb: u32, s: f32) {
    let columns = area.w.round().max(1.0) as usize;
    let top = peaks.iter().copied().map(loudness).fold(0.0f32, f32::max).max(f32::EPSILON);
    let (middle, half) = (area.y + area.h / 2.0, area.h / 2.0);
    let sliver = (0.5 * s).max(0.5).min(half);
    let mut path = PathBuilder::new();
    for (column, loud) in loudest(peaks, columns).into_iter().enumerate() {
        let reach = (loud / top * half).clamp(sliver, half.max(sliver));
        if let Some(r) = Rect::from_xywh(area.x + column as f32, middle - reach, 1.0, 2.0 * reach) {
            path.push_rect(r);
        }
    }
    if let Some(path) = path.finish() {
        fill(pm, &path, rgb, 1.0);
    }
}

/// A clip's notes as a tiny piano roll: each note a bar from its start for its length, the lowest key at the bottom
/// and the highest at the top.
fn draw_notes(pm: &mut Pixmap, notes: &[ThumbNote], area: Area, rgb: u32, s: f32) {
    let (low, high) = notes.iter().fold((u8::MAX, u8::MIN), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
    let span = f32::from(high.saturating_sub(low));
    let bar = (area.h / (span + 1.0)).clamp(2.0 * s, 4.0 * s).min(area.h);
    let shortest = (1.5 * s).min(area.w);
    let mut path = PathBuilder::new();
    for note in notes {
        let x = (area.x + note.start * area.w).min(area.x + area.w - shortest);
        let w = (note.length * area.w).max(shortest).min(area.x + area.w - x);
        let up = if span > 0.0 { f32::from(note.key.saturating_sub(low)) / span } else { 0.5 };
        let y = area.y + area.h - bar - up * (area.h - bar);
        // On whole pixels, so each bar is sharp; never thinner than a pixel.
        let (left, top) = (x.round(), y.round());
        let (right, bottom) = ((x + w).round().max(left + 1.0), (y + bar).round().max(top + 1.0));
        if let Some(r) = Rect::from_ltrb(left, top, right, bottom) {
            path.push_rect(r);
        }
    }
    if let Some(path) = path.finish() {
        fill(pm, &path, rgb, 1.0);
    }
}

/// The plain sign for a line without a picture of its own: level bars for a sound, note dashes for MIDI.
fn draw_sign(pm: &mut Pixmap, kind: Kind, area: Area, rgb: u32, s: f32, square: bool) {
    let (mx, my) = (area.x + area.w / 2.0, area.y + area.h / 2.0);
    let radius = if square { 0.5 } else { 1.5 } * s;
    match kind {
        Kind::Audio => {
            // Four bars of different heights, side by side, in points.
            const BARS: [f32; 4] = [8.0, 16.0, 11.0, 5.0];
            let (w, gap) = (3.0 * s, 2.5 * s);
            let left = mx - (4.0 * w + 3.0 * gap) / 2.0;
            for (i, h) in BARS.iter().enumerate() {
                let bar = Area { x: left + i as f32 * (w + gap), y: my - h * s / 2.0, w, h: h * s };
                fill_round(pm, bar, radius, rgb, 1.0);
            }
        }
        Kind::Midi => {
            // Three notes stepping up and down: each dash's left and middle, from the box's middle, in points.
            const DASHES: [(f32, f32); 3] = [(-11.0, 4.0), (-3.5, -4.0), (4.0, 1.0)];
            let (w, h) = (7.0 * s, 3.0 * s);
            for (dx, dy) in DASHES {
                fill_round(pm, Area { x: mx + dx * s, y: my + dy * s - h / 2.0, w, h }, radius, rgb, 1.0);
            }
        }
    }
}

// ---- Shapes and colours ------------------------------------------------------------------------------------------

/// A box in pixels: its left, top, width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Area {
    /// The box made smaller by `dx` at each side and `dy` at the top and bottom.
    fn inset(self, dx: f32, dy: f32) -> Area {
        Area { x: self.x + dx, y: self.y + dy, w: self.w - 2.0 * dx, h: self.h - 2.0 * dy }
    }
}

/// A corner's radius in points: as given, or nearly square in a theme with square corners.
fn corner(square: bool, round: f32) -> f32 {
    if square { (round / 6.0).clamp(1.0, 2.0) } else { round }
}

/// A 1-point edge in whole pixels, so it's sharp.
fn edge_width(scale: f32) -> f32 {
    scale.round().max(1.0)
}

/// Fills a box with rounded corners.
fn fill_round(pm: &mut Pixmap, area: Area, radius: f32, rgb: u32, alpha: f32) {
    if let Some(path) = round_box(area, radius) {
        fill(pm, &path, rgb, alpha);
    }
}

/// A box with rounded corners as a path, or None when it's too small to have one.
fn round_box(area: Area, radius: f32) -> Option<Path> {
    let Area { x, y, w, h } = area;
    if !(w > 0.0 && h > 0.0 && x.is_finite() && y.is_finite()) {
        return None;
    }
    let r = if radius.is_finite() { radius.clamp(0.0, w.min(h) / 2.0) } else { 0.0 };
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    // Each corner a quarter circle, drawn as a curve: `k` is how far its handles sit from the corner.
    let k = r * (1.0 - 0.552_284_8);
    let (right, bottom) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(right - r, y);
    pb.cubic_to(right - k, y, right, y + k, right, y + r);
    pb.line_to(right, bottom - r);
    pb.cubic_to(right, bottom - k, right - k, bottom, right - r, bottom);
    pb.line_to(x + r, bottom);
    pb.cubic_to(x + k, bottom, x, bottom - k, x, bottom - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + k, x + k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Fills a path in a colour, `alpha` strong, with smooth edges.
fn fill(pm: &mut Pixmap, path: &Path, rgb: u32, alpha: f32) {
    pm.fill_path(path, &paint(rgb, alpha), FillRule::Winding, Transform::identity(), None);
}

/// A paint of one colour (0xRRGGBB), `alpha` strong.
fn paint(rgb: u32, alpha: f32) -> Paint<'static> {
    let [r, g, b] = channels(rgb);
    let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 0.0 };
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, (alpha * 255.0).round() as u8);
    paint.anti_alias = true;
    paint
}

/// A colour's red, green and blue.
fn channels(rgb: u32) -> [u8; 3] {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]
}

/// `over` laid on `under`, `amount` strong (0 to 1).
fn blend(under: u32, over: u32, amount: f32) -> u32 {
    let (a, b) = (channels(under), channels(over));
    let mut out = 0u32;
    for (u, o) in a.iter().zip(&b) {
        out = (out << 8) | mix(f32::from(*u), f32::from(*o), amount).round().clamp(0.0, 255.0) as u32;
    }
    out
}

/// A shadow: how far it falls below the card and how blurred it is (Avalonia's blur radius, in points).
#[derive(Clone, Copy, Debug)]
struct Shadow {
    down: f32,
    blur: f32,
}

impl Shadow {
    /// How far the blur spreads (its standard deviation), in points: Avalonia (and Skia) turn a blur radius into one
    /// this way, so the shadow looks as the C# card's did.
    fn sigma(self) -> f32 {
        self.blur * 0.288_675 + 0.5
    }
}

/// Blurs a black picture (only its alpha counts) about as a Gaussian blur of `sigma` pixels would: three box blurs
/// across, then three down.
fn blur(pm: &mut Pixmap, sigma: f32) {
    let w = (pm.width() as usize).max(1);
    let mut alpha: Vec<f32> = pm.data().as_chunks::<4>().0.iter().map(|p| f32::from(p[3])).collect();
    let radii = box_radii(sigma);
    let (mut line, mut out, mut sums) = (Vec::new(), Vec::new(), Vec::new());
    for row in alpha.chunks_exact_mut(w) {
        for &r in &radii {
            box_line(row, &mut out, &mut sums, r);
            row.copy_from_slice(&out);
        }
    }
    for x in 0..w {
        line.clear();
        line.extend(alpha.iter().skip(x).step_by(w).copied());
        for &r in &radii {
            box_line(&line, &mut out, &mut sums, r);
            line.copy_from_slice(&out);
        }
        for (v, &b) in alpha.iter_mut().skip(x).step_by(w).zip(&line) {
            *v = b;
        }
    }
    for (p, a) in pm.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(&alpha) {
        *p = [0, 0, 0, a.round().clamp(0.0, 255.0) as u8];
    }
}

/// The three box blurs' half widths that together blur about as a Gaussian of `sigma` does.
fn box_radii(sigma: f32) -> [usize; 3] {
    let sigma = if sigma.is_finite() { f64::from(sigma.max(0.0)) } else { 0.0 };
    let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
    let mut lower = ideal.floor().max(1.0);
    if lower % 2.0 == 0.0 {
        lower -= 1.0;
    }
    let upper = lower + 2.0;
    let small = ((12.0 * sigma * sigma - 3.0 * lower * lower - 12.0 * lower - 9.0) / (-4.0 * lower - 4.0)).round();
    let half = |width: f64| ((width - 1.0) / 2.0).max(0.0) as usize;
    [0.0, 1.0, 2.0].map(|i| if i < small { half(lower) } else { half(upper) })
}

/// One box blur along a line: each value the average of those within `r` of it (nothing past the ends). `sums` is
/// working space.
fn box_line(line: &[f32], out: &mut Vec<f32>, sums: &mut Vec<f64>, r: usize) {
    sums.clear();
    let mut total = 0.0f64;
    sums.push(total);
    for &v in line {
        total += f64::from(v);
        sums.push(total);
    }
    let width = (2 * r + 1) as f64;
    let n = line.len();
    out.clear();
    out.extend((0..n).map(|i| ((sums[(i + r + 1).min(n)] - sums[i.saturating_sub(r)]) / width) as f32));
}

// ---- Words -------------------------------------------------------------------------------------------------------

/// A font the card writes in, with the measures lines are laid out by (as parts of the text size).
#[derive(Clone, Copy)]
struct Font {
    font: FontRef<'static>,
    ascent: f32,
    descent: f32,
    /// How tall capital letters (and digits) are, for putting a number in the middle of something.
    cap: f32,
    /// What a cut name ends with: "…", or "..." in a font without it.
    dots: &'static str,
}

impl Font {
    /// The font in a file's bytes, or None when they don't hold one (or swash trips over them: see [`Writer`]).
    fn new(bytes: &'static [u8]) -> Option<Font> {
        mo_catch::catch(|| Font::read(bytes)).ok().flatten()
    }

    fn read(bytes: &'static [u8]) -> Option<Font> {
        let font = FontRef::from_index(bytes, 0)?;
        // swash counts on a font's basic tables being there: in a file cut short it can trip over the missing ones
        // and stop the app. So a font without them (or with no glyph widths) isn't used, and the card has no words.
        let table = |tag: &[u8; 4], least: usize| font.table(swash::tag_from_bytes(tag)).filter(|t| t.len() >= least);
        let widths = table(b"hhea", 36).map(|hhea| u16::from_be_bytes([hhea[34], hhea[35]]));
        let basics = [(b"head", 54), (b"maxp", 6), (b"hmtx", 4), (b"cmap", 4)];
        if widths.is_none_or(|n| n == 0) || basics.iter().any(|(tag, least)| table(tag, *least).is_none()) {
            return None;
        }
        let metrics = font.metrics(&[]);
        let em = f32::from(metrics.units_per_em);
        if em <= 0.0 {
            return None;
        }
        let measure = |v: f32, or: f32| if v.is_finite() && v > 0.0 && v < 4.0 * em { v / em } else { or };
        Some(Font {
            font,
            ascent: measure(metrics.ascent, 0.95),
            descent: measure(metrics.descent.abs(), 0.25),
            cap: measure(metrics.cap_height, 0.7),
            dots: if font.charmap().map('…') != 0 { "…" } else { "..." },
        })
    }

    /// How far down a line of text `size` points tall (with the card's line spacing) its baseline is.
    fn baseline(&self, size: f32) -> f32 {
        ((LINE_SPACING - self.ascent - self.descent) / 2.0 + self.ascent) * size
    }
}

/// How words are written: in which font, how big (in pixels) and in what colour.
#[derive(Clone, Copy)]
struct Style {
    font: Font,
    size: f32,
    rgb: u32,
}

/// One glyph of a line of words: which, and where (pixels from the line's start, and up from its baseline).
#[derive(Clone, Copy, Debug)]
struct Glyph {
    id: u16,
    x: f32,
    y: f32,
}

/// Where a cluster of glyphs (a letter and its accents, say) ends: after which glyph, how far along, and whether
/// it's a space.
#[derive(Clone, Copy, Debug)]
struct ClusterEnd {
    glyphs: usize,
    x: f32,
    space: bool,
}

/// A line of words laid out to fit: its glyphs, and how wide they are in all.
#[derive(Debug, Default)]
struct Words {
    glyphs: Vec<Glyph>,
    width: f32,
}

/// Shapes and draws words, keeping swash's working space between them.
///
/// swash can trip over a rare run of letters: a Korean syllable with a loose vowel after it makes it overflow (swash
/// 0.2.10, `text/compose.rs`), and with the app's overflow checks on that would stop the thread. So, as the app's file
/// readers do, each name is shaped and drawn inside `mo_catch::catch`: one swash trips over is left off the card (the
/// log says where, in one warning line), and the rest of the card is drawn as usual.
struct Writer {
    shaping: ShapeContext,
    scaling: ScaleContext,
}

impl Writer {
    fn new() -> Writer {
        Writer { shaping: ShapeContext::new(), scaling: ScaleContext::new() }
    }

    /// `text` laid out on one line, cut short with "…" when it's wider than `room` pixels (and never ending on a
    /// space before the dots); nothing when swash trips over it.
    fn fit(&mut self, style: &Style, text: &str, room: f32) -> Words {
        let words = mo_catch::catch(AssertUnwindSafe(|| self.cut_to_fit(style, text, room)));
        words.unwrap_or_else(|_| {
            self.start_again();
            Words::default()
        })
    }

    /// Draws laid-out words with their start at `x` and their baseline at `baseline` (pixels).
    fn draw(&mut self, pm: &mut Pixmap, words: &Words, style: &Style, x: f32, baseline: f32) {
        if mo_catch::catch(AssertUnwindSafe(|| self.draw_glyphs(pm, words, style, x, baseline))).is_err() {
            self.start_again();
        }
    }

    /// After swash has tripped, its working space may be left half-changed, so it's made afresh.
    fn start_again(&mut self) {
        *self = Writer::new();
    }

    fn cut_to_fit(&mut self, style: &Style, text: &str, room: f32) -> Words {
        let (mut glyphs, ends, width) = self.shape(style, text);
        if width <= room {
            return Words { glyphs, width };
        }
        let (dots, _, dots_width) = self.shape(style, style.font.dots);
        if dots_width > room {
            return Words::default();
        }
        let mut keep = ends.iter().take_while(|end| end.x + dots_width <= room).count();
        while keep > 0 && ends.get(keep - 1).is_some_and(|end| end.space) {
            keep -= 1;
        }
        let (kept, x) = keep.checked_sub(1).and_then(|k| ends.get(k)).map_or((0, 0.0), |end| (end.glyphs, end.x));
        glyphs.truncate(kept);
        glyphs.extend(dots.into_iter().map(|g| Glyph { x: g.x + x, ..g }));
        Words { glyphs, width: x + dots_width }
    }

    /// Shapes a line of words: its glyphs, where each cluster of them ends, and how wide it is.
    fn shape(&mut self, style: &Style, text: &str) -> (Vec<Glyph>, Vec<ClusterEnd>, f32) {
        let (mut glyphs, mut ends, mut pen) = (Vec::new(), Vec::new(), 0.0f32);
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        let mut shaper = self.shaping.builder(style.font.font).size(style.size).build();
        shaper.add_str(text);
        shaper.shape_with(|cluster| {
            for g in cluster.glyphs {
                glyphs.push(Glyph { id: g.id, x: pen + finite(g.x), y: finite(g.y) });
                pen += finite(g.advance);
            }
            let space = text.get(cluster.source.to_range()).is_some_and(|t| t.chars().all(char::is_whitespace));
            ends.push(ClusterEnd { glyphs: glyphs.len(), x: pen, space });
        });
        (glyphs, ends, pen)
    }

    fn draw_glyphs(&mut self, pm: &mut Pixmap, words: &Words, style: &Style, x: f32, baseline: f32) {
        let mut scaler = self.scaling.builder(style.font.font).size(style.size).hint(true).build();
        for glyph in &words.glyphs {
            let at = x + glyph.x;
            let whole = at.floor();
            // Rendered with its own part of a pixel across, so the spacing stays even.
            let image = Render::new(&[Source::Outline])
                .format(Format::Alpha)
                .offset(Vector::new(at - whole, 0.0))
                .render(&mut scaler, glyph.id);
            let Some(image) = image.filter(|image| image.content == Content::Mask) else { continue };
            let p = image.placement;
            let top = (baseline - glyph.y).round() as i32 - p.top;
            blend_mask(pm, &image.data, (p.width, p.height), (whole as i32 + p.left, top), style.rgb);
        }
    }
}

/// Lays a glyph's coverage (0 to 255 a pixel, `size` across and down) onto the picture with its top left at `at`,
/// in `rgb`, keeping the picture premultiplied.
fn blend_mask(pm: &mut Pixmap, mask: &[u8], size: (u32, u32), at: (i32, i32), rgb: u32) {
    let (pw, ph) = (i64::from(pm.width()), i64::from(pm.height()));
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    let [r, g, b] = channels(rgb).map(u32::from);
    let data = pm.data_mut();
    for row in 0..h {
        let y = i64::from(at.1) + row;
        if !(0..ph).contains(&y) {
            continue;
        }
        for column in 0..w {
            let x = i64::from(at.0) + column;
            let Some(&cover) = mask.get((row * w + column) as usize) else { continue };
            if cover == 0 || !(0..pw).contains(&x) {
                continue;
            }
            let i = ((y * pw + x) * 4) as usize;
            let Some(px) = data.get_mut(i..i + 4) else { continue };
            let (c, keep) = (u32::from(cover), 255 - u32::from(cover));
            let over = |src: u32, dst: u8| ((src * c + u32::from(dst) * keep + 127) / 255) as u8;
            px.copy_from_slice(&[over(r, px[0]), over(g, px[1]), over(b, px[2]), over(255, px[3])]);
        }
    }
}

// ---- Time --------------------------------------------------------------------------------------------------------

/// How far through something taking `total` from `since` it is at `now`: 0 to 1.
fn part(since: Instant, now: Instant, total: Duration) -> f32 {
    let total = total.as_secs_f32();
    if total <= 0.0 {
        return 1.0;
    }
    (now.saturating_duration_since(since).as_secs_f32() / total).clamp(0.0, 1.0)
}

/// From `a` to `b`, `t` of the way (exactly `b` at 1).
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// Quick at first, slowing to a stop.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Slow at first, then faster and faster.
fn ease_in(t: f32) -> f32 {
    t * t
}

/// Slow, quick, then slow again.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Like [`ease_out`], but going a little past the end and back, the way something light springs into place.
fn overshoot(t: f32) -> f32 {
    const PAST: f32 = 1.701_58;
    let u = t - 1.0;
    1.0 + (PAST + 1.0) * u * u * u + PAST * u * u
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::{THEMES, Theme};

    /// The fonts the tests write in: IBM Plex Sans as mo-ui ships it (the path is from this file).
    fn fonts() -> Fonts {
        Fonts {
            regular: include_bytes!("../../mo-ui/fonts/IBMPlexSans-500.ttf"),
            bold: include_bytes!("../../mo-ui/fonts/IBMPlexSans-600.ttf"),
        }
    }

    fn graphite() -> Look {
        Look::from_settings(None, None)
    }

    fn look_of(theme: &Theme) -> Look {
        Look { theme: *theme, accent: theme.accent, accent_text: theme.accent_text }
    }

    fn sound(title: &str) -> Card {
        Card {
            title: title.into(),
            detail: "Kick · C · 0.4 s".into(),
            kind: Kind::Audio,
            tint: 0xF2766B,
            thumb: Thumb::Wave((0..96).map(|i| (-(i as f32) / 20.0).exp()).collect()),
        }
    }

    fn sounds(n: usize) -> Payload {
        let cards = (0..n).map(|i| sound(&format!("Sound {i}"))).collect();
        Payload { files: vec![], cards, caption: format!("{n} sounds"), numbered: false }
    }

    fn tracks(n: usize) -> Payload {
        let notes = vec![
            ThumbNote { start: 0.0, length: 0.25, key: 60 },
            ThumbNote { start: 0.25, length: 0.25, key: 64 },
            ThumbNote { start: 0.5, length: 0.5, key: 67 },
        ];
        let cards = (0..n)
            .map(|i| Card {
                title: format!("Track {i}"),
                detail: "MIDI · 3 notes · 1 bar".into(),
                kind: Kind::Midi,
                tint: 0x4FB3FF,
                thumb: Thumb::Notes(notes.clone()),
            })
            .collect();
        Payload { files: vec![], cards, caption: format!("{n} MIDI tracks"), numbered: true }
    }

    /// Milliseconds after `t0`.
    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    /// The card at rest (over a target, or not) at `scale`.
    fn settled(payload: &Payload, look: &Look, over: bool, scale: f32) -> Picture {
        let t0 = Instant::now();
        let mut flight = Flight::new(payload, look, fonts(), t0);
        flight.set_over_target(over, t0);
        flight.frame(ms(t0, 1000), scale).unwrap()
    }

    fn pixel(p: &Picture, x: i32, y: i32) -> [u8; 4] {
        assert!(x >= 0 && y >= 0 && (x as u32) < p.width && (y as u32) < p.height, "({x}, {y}) is off the picture");
        let i = ((y as u32 * p.width + x as u32) * 4) as usize;
        [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2], p.rgba[i + 3]]
    }

    /// A colour as the card shows it, `opacity` strong over nothing (premultiplied, rounded as `fade` rounds).
    fn seen(rgb: u32, opacity: f32) -> [u8; 4] {
        let k = (opacity * 256.0).round() as u16;
        let f = |v: u8| ((u16::from(v) * k + 128) >> 8) as u8;
        let [r, g, b] = channels(rgb);
        [f(r), f(g), f(b), f(255)]
    }

    fn near(a: [u8; 4], b: [u8; 4], by: u8) -> bool {
        a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= by)
    }

    /// Where things are on a card of `payload` at `scale`, in the picture's pixels.
    fn geometry(payload: &Payload, scale: f32) -> Geometry {
        let face = Face::new(payload, &graphite(), fonts());
        Geometry::new(&face.layout, scale)
    }

    /// A point on the card (in points from its top left) in the picture's pixels, at rest.
    fn on_card(g: &Geometry, x: f32, y: f32) -> (i32, i32) {
        (g.card_x + (x * g.scale).round() as i32, g.card_y + (y * g.scale).round() as i32)
    }

    fn assert_premultiplied(p: &Picture) {
        assert_eq!(p.rgba.len(), (p.width * p.height * 4) as usize);
        for [r, g, b, a] in p.rgba.as_chunks::<4>().0 {
            assert!(r <= a && g <= a && b <= a, "{:?} isn't premultiplied", [r, g, b, a]);
        }
    }

    #[test]
    fn every_frame_is_premultiplied() {
        for payload in [sounds(1), sounds(7), tracks(3)] {
            let t0 = Instant::now();
            let mut flight = Flight::new(&payload, &graphite(), fonts(), t0);
            for t in [0, 30, 60, 100, 160, 400] {
                assert_premultiplied(&flight.frame(ms(t0, t), 1.5).unwrap());
            }
            flight.set_over_target(false, ms(t0, 500));
            for t in [520, 560, 700] {
                assert_premultiplied(&flight.frame(ms(t0, t), 1.5).unwrap());
            }
            flight.end(Ending::Landed, ms(t0, 800));
            for t in [800, 840, 920, 1000, 1100] {
                assert_premultiplied(&flight.frame(ms(t0, t), 1.5).unwrap());
            }
        }
    }

    #[test]
    fn the_mouse_point_is_in_the_picture_and_the_card_below_right_of_it() {
        for scale in [1.0, 1.25, 2.0] {
            let p = settled(&sounds(3), &graphite(), true, scale);
            assert!(p.hot_x > 0 && (p.hot_x as u32) < p.width && p.hot_y > 0 && (p.hot_y as u32) < p.height);
            let solid: Vec<(i32, i32)> = (0..p.height as i32)
                .flat_map(|y| (0..p.width as i32).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&p, x, y)[3] > 200)
                .collect();
            assert!(!solid.is_empty());
            // The card (and the badge poking out of it) is all right of and below the mouse point: it never covers
            // the drop point.
            let left = solid.iter().map(|p| p.0).min().unwrap();
            let top = solid.iter().map(|p| p.1).min().unwrap();
            assert!(left as f32 >= p.hot_x as f32 + 15.0 * scale, "left {left} at scale {scale}");
            assert!(top as f32 >= p.hot_y as f32 + 7.0 * scale, "top {top} at scale {scale}");
        }
    }

    #[test]
    fn twice_the_scale_is_twice_the_pixels() {
        let (one, two) = (settled(&sounds(2), &graphite(), true, 1.0), settled(&sounds(2), &graphite(), true, 2.0));
        assert!(two.width.abs_diff(2 * one.width) <= 3, "{} and {}", one.width, two.width);
        assert!(two.height.abs_diff(2 * one.height) <= 3, "{} and {}", one.height, two.height);
        assert!(two.hot_x.abs_diff(2 * one.hot_x) <= 1);
    }

    #[test]
    fn a_long_name_is_cut_before_the_lines_edge() {
        let mut payload = sounds(1);
        payload.cards[0].title = "A very long name for a sound that goes on and on and never stops at all".into();
        let p = settled(&payload, &graphite(), true, 1.0);
        let g = geometry(&payload, 1.0);
        let theme = graphite().theme;
        let background = seen(theme.surface_raised, FULL);
        let text_end = CARD_WIDTH - PAD - 1.0 - LINE_PAD_X;
        let (_, band_top) = on_card(&g, 0.0, line_top(0) + 1.0 + LINE_PAD_Y);
        let (_, band_bottom) = on_card(&g, 0.0, line_top(0) + LINE_HEIGHT / 2.0);
        let (end, _) = on_card(&g, text_end, 0.0);
        let (past, _) = on_card(&g, text_end + 6.0, 0.0);
        // Nothing of the name past where words may go...
        for y in band_top..band_bottom {
            for x in end..past {
                assert_eq!(pixel(&p, x, y), background, "({x}, {y})");
            }
        }
        // ...but the name (ending with the dots) reaches nearly there.
        let (from, _) = on_card(&g, text_end - 8.0, 0.0);
        let written = (band_top..band_bottom).any(|y| (from..end).any(|x| pixel(&p, x, y) != background));
        assert!(written);
    }

    #[test]
    fn tracks_are_numbered() {
        let payload = tracks(3);
        let p = settled(&payload, &graphite(), true, 1.0);
        let g = geometry(&payload, 1.0);
        let background = seen(graphite().theme.surface_raised, FULL);
        for i in 0..3 {
            let (x0, y0) = on_card(&g, PAD + 1.0 + LINE_PAD_X, line_top(i) + 10.0);
            let (x1, y1) = on_card(&g, PAD + 1.0 + LINE_PAD_X + NUMBER_WIDTH, line_top(i) + LINE_HEIGHT - 10.0);
            let written = (y0..y1).any(|y| (x0..x1).any(|x| pixel(&p, x, y) != background));
            assert!(written, "line {i} has no number");
        }
        // Unnumbered, the same place is where the picture's box starts: no number there.
        let mut plain = payload.clone();
        plain.numbered = false;
        let p = settled(&plain, &graphite(), true, 1.0);
        let (x, y) = on_card(&g, PAD + 1.0 + LINE_PAD_X + NUMBER_WIDTH - 2.0, line_top(0) + LINE_HEIGHT / 2.0);
        let box_colour = seen(blend(graphite().theme.surface_raised, 0x4FB3FF, THUMB_WASH), FULL);
        assert!(near(pixel(&p, x, y), box_colour, 2), "{:?}", pixel(&p, x, y));
    }

    #[test]
    fn seven_things_show_five_lines_then_two_more_and_the_badge() {
        let (one, seven) = (sounds(1), sounds(7));
        let p = settled(&seven, &graphite(), true, 1.0);
        assert!(p.height > settled(&one, &graphite(), true, 1.0).height);
        let g = geometry(&seven, 1.0);
        let theme = graphite().theme;
        // Five line boxes, and no sixth.
        for i in 0..5 {
            let (x, y) = on_card(&g, PAD + 3.0, line_top(i) + LINE_HEIGHT / 2.0);
            assert_eq!(pixel(&p, x, y), seen(theme.surface_raised, FULL), "line {i}");
        }
        let (x, y) = on_card(&g, PAD + 3.0, line_top(5) + LINE_HEIGHT / 2.0);
        assert_eq!(pixel(&p, x, y), seen(theme.sidebar, FULL));
        // "+ 2 more" is written where it goes.
        let face = Face::new(&seven, &graphite(), fonts());
        let top = face.layout.more.unwrap();
        let (x0, y0) = on_card(&g, PAD + NOTE_INDENT, top);
        let (x1, y1) = on_card(&g, PAD + NOTE_INDENT + 50.0, top + NOTE_HEIGHT);
        let written = (y0..y1).any(|y| (x0..x1).any(|x| pixel(&p, x, y) != seen(theme.sidebar, FULL)));
        assert!(written);
        // The badge pokes out over the top right corner.
        let (x, y) = on_card(&g, CARD_WIDTH + 5.0, -5.0);
        assert!(near(pixel(&p, x, y), seen(graphite().accent, FULL), 2), "{:?}", pixel(&p, x, y));
    }

    #[test]
    fn one_thing_has_no_badge() {
        let payload = sounds(1);
        let p = settled(&payload, &graphite(), true, 1.0);
        let (x, y) = on_card(&geometry(&payload, 1.0), CARD_WIDTH + 5.0, -5.0);
        assert!(pixel(&p, x, y)[3] < 100, "{:?}", pixel(&p, x, y));
    }

    #[test]
    fn not_over_a_target_the_card_is_fainter_with_a_plain_edge() {
        let payload = sounds(1);
        let g = geometry(&payload, 1.0);
        let theme = graphite().theme;
        let inside = on_card(&g, CARD_WIDTH / 2.0, 3.0);
        let edge = (g.card_x, g.card_y + g.card_h / 2);
        let over = settled(&payload, &graphite(), true, 1.0);
        assert_eq!(pixel(&over, inside.0, inside.1), seen(theme.sidebar, FULL));
        assert_eq!(pixel(&over, edge.0, edge.1), seen(graphite().accent, FULL));
        let not = settled(&payload, &graphite(), false, 1.0);
        assert_eq!(pixel(&not, inside.0, inside.1), seen(theme.sidebar, FAINT));
        assert_eq!(pixel(&not, edge.0, edge.1), seen(theme.stroke, FAINT));
        // And it changes over about 120 ms, not at once.
        let t0 = Instant::now();
        let mut flight = Flight::new(&payload, &graphite(), fonts(), t0);
        flight.frame(ms(t0, 500), 1.0).unwrap();
        flight.set_over_target(false, ms(t0, 500));
        let halfway = flight.frame(ms(t0, 560), 1.0).unwrap();
        let alpha = pixel(&halfway, inside.0, inside.1)[3];
        assert!(alpha > seen(0, FAINT)[3] + 5 && alpha < seen(0, FULL)[3] - 5, "{alpha}");
        assert_eq!(flight.frame(ms(t0, 620), 1.0).unwrap(), not);
    }

    #[test]
    fn still_once_lifted_and_not_while_moving() {
        let t0 = Instant::now();
        let mut flight = Flight::new(&sounds(2), &graphite(), fonts(), t0);
        assert!(!flight.still(t0, 1.0), "nothing shown yet");
        flight.frame(ms(t0, 50), 1.0).unwrap();
        assert!(!flight.still(ms(t0, 66), 1.0), "lifting");
        let lifted = flight.frame(ms(t0, 170), 1.0).unwrap();
        assert!(flight.still(ms(t0, 186), 1.0));
        assert!(flight.still(ms(t0, 5000), 1.0));
        assert!(!flight.still(ms(t0, 5000), 2.0), "a new scale");
        // A still frame is the same picture again.
        assert_eq!(flight.frame(ms(t0, 5000), 1.0).unwrap(), lifted);
        flight.set_over_target(true, ms(t0, 5000));
        assert!(flight.still(ms(t0, 5010), 1.0), "already over a target: no change");
        flight.set_over_target(false, ms(t0, 5000));
        assert!(!flight.still(ms(t0, 5010), 1.0), "fading");
        flight.frame(ms(t0, 5200), 1.0).unwrap();
        assert!(flight.still(ms(t0, 5300), 1.0));
        flight.end(Ending::Landed, ms(t0, 6000));
        flight.frame(ms(t0, 6016), 1.0).unwrap();
        assert!(!flight.still(ms(t0, 6032), 1.0), "landing");
    }

    #[test]
    fn landing_takes_about_320_ms_and_cancelling_about_180() {
        for (how, lasts) in [(Ending::Landed, 320), (Ending::Cancelled, 180)] {
            let t0 = Instant::now();
            let mut flight = Flight::new(&sounds(1), &graphite(), fonts(), t0);
            flight.frame(ms(t0, 300), 1.0).unwrap();
            flight.end(how, ms(t0, 1000));
            for t in [0, 50, 100, lasts / 2, lasts - 10, lasts - 1] {
                assert!(flight.frame(ms(t0, 1000 + t), 1.0).is_some(), "{how:?} at {t} ms");
            }
            assert!(flight.frame(ms(t0, 1000 + lasts), 1.0).is_none(), "{how:?} over");
            assert!(flight.frame(ms(t0, 1000 + lasts + 500), 1.0).is_none());
            assert!(!flight.still(ms(t0, 1000 + lasts + 500), 1.0));
        }
    }

    #[test]
    fn a_second_end_changes_nothing() {
        let t0 = Instant::now();
        let mut flight = Flight::new(&sounds(1), &graphite(), fonts(), t0);
        flight.end(Ending::Cancelled, ms(t0, 300));
        flight.end(Ending::Landed, ms(t0, 400));
        flight.set_over_target(false, ms(t0, 400));
        assert!(flight.frame(ms(t0, 300 + 179), 1.0).is_some());
        assert!(flight.frame(ms(t0, 300 + 180), 1.0).is_none());
    }

    #[test]
    fn the_ring_pulses_out_from_the_mouse_point_as_it_lands() {
        let t0 = Instant::now();
        let mut flight = Flight::new(&sounds(1), &graphite(), fonts(), t0);
        let before = flight.frame(ms(t0, 300), 1.5).unwrap();
        flight.end(Ending::Landed, ms(t0, 1000));
        let p = flight.frame(ms(t0, 1120), 1.5).unwrap();
        // Up and left of the mouse point, where the card never is, at the ring's radius by then.
        let r = mix(RING_FROM, RING_TO, ease_out(120.0 / 320.0)) * 1.5 / std::f32::consts::SQRT_2;
        let (x, y) = ((p.hot_x as f32 - r).round() as i32, (p.hot_y as f32 - r).round() as i32);
        let strongest =
            (y - 2..=y + 2).flat_map(|y| (x - 2..=x + 2).map(move |x| (x, y))).map(|(x, y)| pixel(&p, x, y)[3]);
        assert!(strongest.max().unwrap() > 20);
        assert_eq!(pixel(&before, x, y)[3], 0);
        // And it's round about the mouse point: as strong to the left as above.
        let r = r * std::f32::consts::SQRT_2;
        let left = (0..p.width as i32).filter(|&x| pixel(&p, x, p.hot_y)[3] > 0).min().unwrap();
        let up = (0..p.height as i32).filter(|&y| pixel(&p, p.hot_x, y)[3] > 0).min().unwrap();
        assert!((p.hot_x - left).abs_diff(p.hot_y - up) <= 1, "{left} {up}");
        assert!(((p.hot_x - left) as f32 - r).abs() < 3.0);
    }

    #[test]
    fn ending_while_lifting_starts_from_where_the_card_is() {
        for how in [Ending::Landed, Ending::Cancelled] {
            let t0 = Instant::now();
            let mut flight = Flight::new(&sounds(1), &graphite(), fonts(), t0);
            let before = flight.pose(ms(t0, 60)).unwrap();
            flight.end(how, ms(t0, 60));
            let after = flight.pose(ms(t0, 60)).unwrap();
            assert_eq!(Pose { ring: None, dot: None, ..after }, before, "{how:?}");
            // A moment later it has moved on from there, not from the resting card.
            let next = flight.pose(ms(t0, 76)).unwrap();
            assert!(next.opacity < before.opacity && next.opacity > 0.0);
        }
        let t0 = Instant::now();
        let mut flight = Flight::new(&sounds(1), &graphite(), fonts(), t0);
        let before = flight.frame(ms(t0, 60), 1.0).unwrap();
        flight.end(Ending::Cancelled, ms(t0, 60));
        assert_eq!(flight.frame(ms(t0, 60), 1.0).unwrap(), before);
    }

    #[test]
    fn the_lift_grows_fades_in_and_slides_out_from_the_mouse() {
        let t0 = Instant::now();
        let flight = Flight::new(&sounds(1), &graphite(), fonts(), t0);
        let start = flight.pose(t0).unwrap();
        assert_eq!((start.size, start.slide, start.opacity, start.lifted), (LIFT_FROM, 1.0, 0.0, 0.0));
        // It springs a little past full size, then settles exactly.
        let most = (0..160).map(|t| flight.pose(ms(t0, t)).unwrap().size).fold(0.0f32, f32::max);
        assert!(most > 1.0 && most < 1.02, "{most}");
        let up = flight.pose(ms(t0, 160)).unwrap();
        assert_eq!(
            up,
            Pose { size: 1.0, slide: 0.0, settle: 1.0, opacity: FULL, lifted: 1.0, over: 1.0, ring: None, dot: None }
        );
        // Time only comes from the calls: one before the start is the start.
        assert_eq!(flight.pose(t0 - Duration::from_millis(5)).unwrap(), start);
    }

    #[test]
    fn nothing_is_cut_off_at_the_pictures_edges() {
        for payload in [sounds(1), sounds(7)] {
            let t0 = Instant::now();
            let mut flight = Flight::new(&payload, &graphite(), fonts(), t0);
            for t in [0, 40, 80, 100, 120, 160, 400] {
                let p = flight.frame(ms(t0, t), 1.5).unwrap();
                let (w, h) = (p.width as i32, p.height as i32);
                let edges = (0..w).flat_map(|x| [(x, 0), (x, h - 1)]).chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]));
                let strongest = edges.map(|(x, y)| pixel(&p, x, y)[3]).max().unwrap();
                assert!(strongest <= 2, "{strongest} at the edge at {t} ms");
            }
        }
    }

    #[test]
    fn square_themes_have_square_corners() {
        let square = THEMES.iter().find(|t| t.square).unwrap();
        let round = THEMES.iter().find(|t| !t.square).unwrap();
        let payload = sounds(1);
        let g = geometry(&payload, 1.0);
        let corner = (g.card_x + 1, g.card_y + 1);
        let p = settled(&payload, &look_of(square), true, 1.0);
        assert!(pixel(&p, corner.0, corner.1)[3] > 200);
        let p = settled(&payload, &look_of(round), true, 1.0);
        assert!(pixel(&p, corner.0, corner.1)[3] < 120);
    }

    #[test]
    fn notes_go_from_the_lowest_key_at_the_bottom_to_the_highest_at_the_top() {
        let mut payload = tracks(1);
        payload.numbered = false;
        payload.cards[0].tint = 0xFF0000;
        payload.cards[0].thumb = Thumb::Notes(vec![
            ThumbNote { start: 0.0, length: 0.4, key: 40 },
            ThumbNote { start: 0.6, length: 0.4, key: 80 },
        ]);
        let p = settled(&payload, &graphite(), true, 2.0);
        let g = geometry(&payload, 2.0);
        let left = PAD + 1.0 + LINE_PAD_X + THUMB_INSET_X;
        let inner_w = THUMB_WIDTH - 2.0 * THUMB_INSET_X;
        let top = line_top(0) + (LINE_HEIGHT - THUMB_HEIGHT) / 2.0 + THUMB_INSET_Y;
        let bottom = top + THUMB_HEIGHT - 2.0 * THUMB_INSET_Y;
        let red = |x: f32, y: f32| {
            let (x, y) = on_card(&g, x, y);
            let [r, g, _, _] = pixel(&p, x, y);
            r > 200 && g < 100
        };
        assert!(red(left + 0.2 * inner_w, bottom - 1.0));
        assert!(!red(left + 0.2 * inner_w, top + 1.0));
        assert!(red(left + 0.8 * inner_w, top + 1.0));
        assert!(!red(left + 0.8 * inner_w, bottom - 1.0));
    }

    #[test]
    fn the_loudest_slice_reaches_the_boxs_edges() {
        let mut payload = sounds(1);
        payload.cards[0].tint = 0xFF0000;
        payload.cards[0].thumb = Thumb::Wave(vec![0.0, 0.0, 0.5, 0.5, 0.25, 0.25, 0.0, 0.0]);
        let p = settled(&payload, &graphite(), true, 2.0);
        let g = geometry(&payload, 2.0);
        let left = PAD + 1.0 + LINE_PAD_X + THUMB_INSET_X;
        let inner_w = THUMB_WIDTH - 2.0 * THUMB_INSET_X;
        let top = line_top(0) + (LINE_HEIGHT - THUMB_HEIGHT) / 2.0 + THUMB_INSET_Y;
        let middle = line_top(0) + LINE_HEIGHT / 2.0;
        let red = |x: f32, y: f32| {
            let (x, y) = on_card(&g, x, y);
            let [r, g, _, _] = pixel(&p, x, y);
            r > 200 && g < 100
        };
        // The loudest (a half) reaches the top; the silent slices are still a sliver about the middle.
        assert!(red(left + 0.3 * inner_w, top + 0.5));
        assert!(!red(left + 0.6 * inner_w, top + 0.5));
        assert!(red(left + 0.05 * inner_w, middle));
        assert!(!red(left + 0.05 * inner_w, middle - 2.0));
    }

    #[test]
    fn the_picture_is_never_too_big() {
        for scale in [4.0, 100.0, f32::INFINITY] {
            let p = settled(&sounds(7), &graphite(), true, scale);
            assert!(p.width <= 1200 && p.height <= 1200, "{}x{} at {scale}", p.width, p.height);
            assert!(p.height > 1000, "and still big on a big screen");
        }
        for scale in [0.0, -3.0, f32::NAN, 1e-9, f32::NEG_INFINITY] {
            let p = settled(&sounds(2), &graphite(), true, scale);
            assert!(p.width > 100 && p.width <= 1200 && p.height > 50, "{}x{} at {scale}", p.width, p.height);
        }
    }

    #[test]
    fn strange_payloads_and_fonts_never_stop_the_app() {
        let mut odd = Payload { numbered: true, ..Payload::default() };
        let notes = |start: f32, length: f32, key: u8| ThumbNote { start, length, key };
        odd.cards = vec![
            Card::default(),
            Card { title: "\n\t\u{0}line\r\nbreaks\u{7f}".into(), tint: 0xFFFF_FFFF, ..Card::default() },
            Card { title: "🥁🎹 鼓 ドラム".repeat(500), detail: "x".repeat(100_000), ..Card::default() },
            Card { title: "가\u{1161} Kick".into(), detail: "\u{AC00}\u{1161}".into(), ..Card::default() },
            Card { thumb: Thumb::Wave(vec![f32::NAN, f32::INFINITY, -3.0, f32::MAX, 0.5]), ..Card::default() },
            Card { thumb: Thumb::Wave(vec![f32::NAN; 3]), ..Card::default() },
            Card {
                kind: Kind::Midi,
                thumb: Thumb::Notes(vec![
                    notes(f32::NAN, 1.0, 0),
                    notes(-5.0, f32::INFINITY, 127),
                    notes(2.0, -1.0, 64),
                    notes(0.999, 1.0, 0),
                ]),
                ..Card::default()
            },
            Card { kind: Kind::Midi, thumb: Thumb::Notes(vec![notes(f32::NAN, 0.0, 9)]), ..Card::default() },
            Card { thumb: Thumb::Wave(vec![]), ..Card::default() },
        ];
        odd.cards.extend((0..100_000).map(|_| Card::default()));
        odd.caption = " ".repeat(10);
        let mut long = sounds(1);
        long.cards[0].thumb = Thumb::Wave((0..1_000_000).map(|i| (i % 7) as f32).collect());
        long.cards[0].kind = Kind::Midi;
        let regular = fonts().regular;
        let broken = [
            fonts(),
            Fonts { regular: b"not a font at all", bold: &[] },
            Fonts { regular: &[], bold: &[] },
            Fonts { regular: &regular[..1000], bold: &regular[..30_000] },
            Fonts { regular: &regular[..12], bold: fonts().bold },
        ];
        for payload in [Payload::default(), odd, long] {
            for f in broken {
                for theme in &THEMES {
                    let t0 = Instant::now();
                    let mut flight = Flight::new(&payload, &look_of(theme), f, t0);
                    for t in [0, 80, 400] {
                        assert_premultiplied(&flight.frame(ms(t0, t), 1.0).unwrap());
                    }
                    flight.end(Ending::Landed, ms(t0, 400));
                    assert!(flight.frame(ms(t0, 500), 1.0).is_some());
                }
            }
        }
    }

    #[test]
    fn korean_letters_swash_would_join_wrong_are_kept_apart() {
        let apart = "\u{200C}";
        // A syllable without a final consonant, then a loose vowel: swash 0.2.10 overflows on these.
        assert_eq!(tidy("\u{AC00}\u{1161}"), format!("\u{AC00}{apart}\u{1161}"));
        // A loose first consonant then a final one: swash makes something that isn't a letter.
        assert_eq!(tidy("\u{1112}\u{11C2}"), format!("\u{1112}{apart}\u{11C2}"));
        // As letters join one after another: first consonant and vowel make a syllable, which then meets a vowel.
        assert_eq!(tidy("\u{1100}\u{1161}\u{1161}"), format!("\u{1100}\u{1161}{apart}\u{1161}"));
        // Pairs that join rightly, a Mac's way of writing 각 (first consonant, vowel, final), and others stay as they
        // are.
        for fine in ["\u{1100}\u{1161}\u{11A8}", "\u{AC00}\u{11A8}", "\u{AC01}\u{1161}", "\u{D55C}\u{AE00}", "Kick 04"]
        {
            assert_eq!(tidy(fine), fine);
        }
        assert_eq!(hangul_join('\u{1100}', '\u{1161}'), Join::Joined('\u{AC00}'));
        assert_eq!(hangul_join('\u{AC00}', '\u{11A8}'), Join::Joined('\u{AC01}'));
    }

    #[test]
    fn a_name_swash_trips_over_is_left_off_and_the_rest_is_drawn() {
        // A Korean syllable with a loose vowel after it, which swash 0.2.10 overflows on, straight to the writer (the
        // card's own tidying keeps them apart): whatever swash makes of it, the writer goes on writing.
        let font = Font::new(fonts().regular).unwrap();
        let style = Style { font, size: 13.0, rgb: 0xFFFFFF };
        let mut writer = Writer::new();
        let _ = writer.fit(&style, "\u{AC00}\u{1161}", 200.0);
        assert!(!writer.fit(&style, "Kick", 200.0).glyphs.is_empty());
        // And on the card, beside a line that's fine.
        let mut payload = sounds(2);
        payload.cards[0].title = "\u{AC00}\u{1161}".into();
        let p = settled(&payload, &graphite(), true, 1.0);
        let g = geometry(&payload, 1.0);
        let background = seen(graphite().theme.surface_raised, FULL);
        let x = PAD + 1.0 + LINE_PAD_X + THUMB_WIDTH + THUMB_GAP;
        let (x0, y0) = on_card(&g, x, line_top(1) + 1.0 + LINE_PAD_Y);
        let (x1, y1) = on_card(&g, x + 40.0, line_top(1) + LINE_HEIGHT / 2.0);
        let written = (y0..y1).any(|y| (x0..x1).any(|x| pixel(&p, x, y) != background));
        assert!(written, "the second line's name");
    }

    #[test]
    fn a_flight_can_go_to_the_drawing_thread() {
        fn sendable<T: Send>() {}
        sendable::<Flight>();
    }

    #[test]
    fn helpers() {
        assert_eq!(tidy("  a\nb\t "), "a b");
        assert_eq!(blend(0x000000, 0xFFFFFF, 0.5), 0x808080);
        assert_eq!(mix_pixel(0, 0xFF00_00FF, 256), 0xFF00_00FF);
        assert_eq!(scale_pixel(0xFFFF_FFFF, 128), 0x7F7F_7F7F);
        for (ease, name) in
            [(ease_out as fn(f32) -> f32, "out"), (ease_in, "in"), (smooth, "smooth"), (overshoot, "back")]
        {
            assert_eq!((ease(0.0).abs() < 1e-6, ease(1.0)), (true, 1.0), "{name}");
        }
    }
}
