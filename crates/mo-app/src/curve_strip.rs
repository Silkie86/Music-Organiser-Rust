//! The clip strip over the curve editor's picture, worked out with no screen (the screen design of 14 Nov 2026,
//! "Ruler and clip strip"): the curve's clips end to end on the picture's own time line, which one the mouse has,
//! and the highlight's right-click menu. A clip is a remembered highlight: a part of the curve with a name. A curve
//! with no clips shows one tile with its own name across its whole length, so the strip is always there.

use crate::curve_edit::{Edit, Shape, Span, apply};
use crate::curve_hold::length_words;
use crate::curve_picture::Frame;
use crate::presets::{Offer, curve_act};

/// The room between two tiles, in points.
pub const GAP: f64 = 2.0;
/// A tile's pin is this wide at its right end; a tile narrower than [`PIN_FROM`] has no room for it.
pub const PIN: f64 = 22.0;
pub const PIN_FROM: f64 = 48.0;

/// One tile of the strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Its place in [`Shape::clips`]; none for the one tile of a curve with no clips.
    pub clip: Option<usize>,
    pub span: Span,
    /// Where it's drawn, in points from the picture's left: what's in sight of it.
    pub left: f64,
    pub right: f64,
    pub name: String,
    /// Its length in words ("1/2 beat", "2 bars").
    pub words: String,
    pub pinned: bool,
    /// Its part is the highlight.
    pub picked: bool,
    /// Tiles take turns between two plain shades: every second one is the lighter.
    pub lighter: bool,
}

/// The strip's tiles in sight, left to right. `name` is the curve's own, for the one tile of a curve with no clips.
pub fn tiles(shape: &Shape, highlight: Option<Span>, frame: Frame, name: &str) -> Vec<Tile> {
    if !frame.fits() {
        return Vec::new();
    }
    let view = frame.view;
    let tile = |clip: Option<usize>, span: Span, name: &str, pinned: bool, lighter: bool| {
        let from = span.start.max(view.start);
        let to = span.end.min(view.start + view.span);
        (to > from).then(|| Tile {
            clip,
            span,
            left: frame.x(from) + GAP / 2.0,
            right: (frame.x(to) - GAP / 2.0).max(frame.x(from) + GAP / 2.0 + 1.0),
            name: name.to_owned(),
            words: length_words(span.len()),
            pinned,
            picked: highlight.is_some_and(|h| h.same(span)),
            lighter,
        })
    };
    if shape.clips().is_empty() {
        return tile(None, shape.whole(), name, false, false).into_iter().collect();
    }
    shape.clips().iter().enumerate().filter_map(|(i, c)| tile(Some(i), c.span, &c.name, c.pinned, i % 2 == 1)).collect()
}

/// What of the strip the mouse has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnStrip {
    /// A clip's pin.
    Pin(usize),
    /// A clip.
    Clip(usize),
    /// The one tile of a curve with no clips.
    Whole,
    Empty,
}

/// What's at `x` (points from the picture's left) on the strip.
pub fn strip_at(tiles: &[Tile], x: f64) -> OnStrip {
    let Some(t) = tiles.iter().find(|t| x >= t.left && x <= t.right) else { return OnStrip::Empty };
    match t.clip {
        Some(i) if t.right - t.left >= PIN_FROM && x >= t.right - PIN => OnStrip::Pin(i),
        Some(i) => OnStrip::Clip(i),
        None => OnStrip::Whole,
    }
}

/// The clip whose part is exactly `highlight`, if one is.
pub fn clip_of(shape: &Shape, highlight: Option<Span>) -> Option<usize> {
    let h = highlight?;
    shape.clips().iter().position(|c| c.span.same(h))
}

/// The name a highlight kept as a clip gets: "Part 1", "Part 2", the first not taken.
pub fn part_name(shape: &Shape) -> String {
    let mut n = 1;
    loop {
        let name = format!("Part {n}");
        if shape.clips().iter().all(|c| c.name != name) {
            return name;
        }
        n += 1;
    }
}

/// The right-click menu on the picture and the strip: what can be done to the highlighted part (the whole curve
/// when nothing is highlighted), each line with its key. `step` is a grid square in beats, for the steps. A line
/// that would change nothing (a level part flipped, a part that reads the same backwards) is greyed.
pub fn part_offers(shape: &Shape, highlight: Option<Span>, step: f64) -> Vec<Offer> {
    let lit = highlight.is_some();
    let clip = clip_of(shape, highlight);
    let offer = |text: &str, id: &'static str, enabled: bool| Offer { text: text.to_owned(), id, enabled };
    let does = |edit: Edit| apply(shape, highlight, &edit).is_some();
    let of = if lit { "this part" } else { "the whole curve" };
    vec![
        offer(&format!("Reverse {of}"), curve_act::REVERSE, does(Edit::Reverse)),
        offer(&format!("Flip {of} upside down"), curve_act::FLIP, does(Edit::Flip)),
        offer(&format!("Make {of} into steps"), curve_act::STEPS, does(Edit::Steps { step })),
        match clip {
            Some(_) => offer("Forget this clip (the curve stays)", curve_act::KEEP_CLIP, true),
            None => offer("Keep this part as a clip", curve_act::KEEP_CLIP, lit),
        },
        offer("Cut this time out", curve_act::CUT_TIME, lit && does(Edit::RemoveTime)),
        offer("Highlight the whole curve", curve_act::HIGHLIGHT_ALL, highlight.is_none_or(|h| !h.same(shape.whole()))),
    ]
}

/// The hint line's words for the strip.
pub fn hint(on: OnStrip, tiles: &[Tile]) -> (String, &'static str) {
    let named = |i: usize| tiles.iter().find(|t| t.clip == Some(i)).map(|t| format!("{} · {}", t.name, t.words));
    match on {
        OnStrip::Pin(i) => (named(i).unwrap_or_default(), "click pins it: Randomise leaves a pinned clip as it is"),
        OnStrip::Clip(i) => (named(i).unwrap_or_default(), "click highlights it · drag moves it · Ctrl-drag copies it"),
        OnStrip::Whole => (
            tiles.first().map(|t| t.name.clone()).unwrap_or_default(),
            "no clips yet · double-click keeps the highlight as a clip",
        ),
        OnStrip::Empty => (String::new(), "no clip here · double-click keeps the highlight as a clip"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_edit::{Edit, apply};
    use crate::curve_picture::{SIDE, TOP, View};
    use mo_formats::bitwig::curve::CurvePoint;

    fn shape() -> Shape {
        let p = |x, y, bend| CurvePoint { x, y, bend };
        Shape::from_curve(&[p(0.0, 0.0, 0.0), p(0.5, 0.8, 0.0), p(1.0, 0.2, 0.0)], 8.0)
    }

    /// 50 points a beat.
    fn frame() -> Frame {
        Frame { width: 400.0 + 2.0 * SIDE, height: 200.0 + 2.0 * TOP, view: View::whole(8.0) }
    }

    fn with_clips() -> Shape {
        let s = shape();
        let (s, _) = apply(&s, Some(Span::new(0.0, 4.0)), &Edit::Remember { name: "Rise".into() }).unwrap();
        let (s, _) = apply(&s, Some(Span::new(4.0, 4.5)), &Edit::Remember { name: "Blip".into() }).unwrap();
        s
    }

    #[test]
    fn a_curve_with_no_clips_shows_one_tile_with_its_own_name() {
        let t = tiles(&shape(), None, frame(), "Wobble");
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].clip, t[0].name.as_str(), t[0].words.as_str()), (None, "Wobble", "2 bars"));
        assert!((t[0].left - (SIDE + 1.0)).abs() < 1e-6 && (t[0].right - (SIDE + 399.0)).abs() < 1e-6);
        assert_eq!(strip_at(&t, SIDE + 200.0), OnStrip::Whole);
        assert!(tiles(&shape(), None, Frame { width: 0.0, ..frame() }, "Wobble").is_empty());
    }

    #[test]
    fn clips_are_tiles_on_the_pictures_time_line() {
        let s = with_clips();
        let f = frame();
        let t = tiles(&s, Some(Span::new(4.0, 4.5)), f, "Wobble");
        assert_eq!(t.len(), 2);
        assert_eq!(
            (t[0].name.as_str(), t[0].words.as_str(), t[0].picked, t[0].lighter),
            ("Rise", "1 bar", false, false)
        );
        assert_eq!(
            (t[1].name.as_str(), t[1].words.as_str(), t[1].picked, t[1].lighter),
            ("Blip", "1/2 beat", true, true)
        );
        // 2 points apart at the seam.
        assert!((t[1].left - t[0].right - GAP).abs() < 1e-6);
        // The wide one has a pin at its right end; the narrow one (25 points) has no room for one.
        assert_eq!(strip_at(&t, t[0].right - 5.0), OnStrip::Pin(0));
        assert_eq!(strip_at(&t, t[0].left + 5.0), OnStrip::Clip(0));
        assert_eq!(strip_at(&t, t[1].right - 5.0), OnStrip::Clip(1));
        assert_eq!(strip_at(&t, f.x(6.0)), OnStrip::Empty);
        // Zoomed in on the second bar, only what's in sight of a clip is a tile.
        let close = Frame { view: View { start: 4.0, span: 4.0 }, ..f };
        let t = tiles(&s, None, close, "Wobble");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].clip, Some(1));
    }

    #[test]
    fn the_menu_keeps_or_forgets_a_clip_and_names_parts_in_turn() {
        let s = with_clips();
        assert_eq!(clip_of(&s, Some(Span::new(0.0, 4.0))), Some(0));
        assert_eq!(clip_of(&s, Some(Span::new(0.0, 3.0))), None);
        let on_clip = part_offers(&s, Some(Span::new(0.0, 4.0)), 0.25);
        assert!(on_clip.iter().any(|o| o.id == curve_act::KEEP_CLIP && o.text.starts_with("Forget")));
        let on_part = part_offers(&s, Some(Span::new(5.0, 6.0)), 0.25);
        assert!(on_part.iter().any(|o| o.id == curve_act::KEEP_CLIP && o.text.starts_with("Keep") && o.enabled));
        assert!(on_part.iter().any(|o| o.id == curve_act::CUT_TIME && o.enabled));
        // Beats 5 to 6 of the fall: flipped and reversed it's another shape, and a level part would not be.
        assert!(on_part.iter().any(|o| o.id == curve_act::FLIP && o.enabled));
        let level = [CurvePoint { x: 0.0, y: 0.5, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.5, bend: 0.0 }];
        let on_flat = part_offers(&Shape::from_curve(&level, 4.0), None, 0.25);
        assert!(on_flat.iter().any(|o| o.id == curve_act::FLIP && !o.enabled));
        assert!(on_flat.iter().any(|o| o.id == curve_act::REVERSE && !o.enabled));
        let on_none = part_offers(&s, None, 0.25);
        assert!(on_none.iter().any(|o| o.id == curve_act::KEEP_CLIP && !o.enabled));
        assert!(on_none.iter().any(|o| o.id == curve_act::REVERSE && o.text == "Reverse the whole curve"));
        // The whole curve's time can't be cut out.
        assert!(part_offers(&s, Some(s.whole()), 0.25).iter().any(|o| o.id == curve_act::CUT_TIME && !o.enabled));
        for o in &on_part {
            assert!(crate::shortcuts::CURVES_ACTIONS.iter().any(|a| a.id == o.id), "{}", o.id);
        }
        assert_eq!(part_name(&shape()), "Part 1");
        let (named, _) = apply(&s, Some(Span::new(5.0, 6.0)), &Edit::Remember { name: "Part 1".into() }).unwrap();
        assert_eq!(part_name(&named), "Part 2");
    }

    #[test]
    fn the_strip_has_its_hints() {
        let s = with_clips();
        let t = tiles(&s, None, frame(), "Wobble");
        assert_eq!(hint(OnStrip::Clip(0), &t).0, "Rise · 1 bar");
        assert!(hint(OnStrip::Pin(0), &t).1.contains("pins"));
        assert!(hint(OnStrip::Empty, &t).0.is_empty());
    }
}
