//! The MIDI page's piano roll as a picture (spec 3.4, 9.1): a row for each key (the black keys' rows darker), the
//! keyboard down the side, a line at each bar and each note a bar of the accent colour, as strong as it's played. It's
//! worked out here as a [`Scene`], which the graphics card draws (mo-ui's canvas), or the processor where there's no
//! card, and the window lays the keys' and notes' names on it as words of its own.
//!
//! It's laid out as the window laid out the roll when it drew each row, key and note itself, in midi.slint's
//! `PianoRoll` (which still draws the well round it and the words): the same places and sizes, in the same order. The
//! one change: each edge of a row, key, line or note is on an edge between two pixels, so it stays sharp at any size
//! (only a note's rounded corners are soft).

use crate::midi::{Roll, is_black, note_name};
use crate::shapes::{Colour, Scene, usable_scale};

/// Where the rows start across the roll, in points: past the keyboard, or with no keyboard past the room for the
/// highest and lowest keys' names (which the window writes there itself).
const SIDE_WITH_KEYBOARD: f64 = 52.0;
const SIDE: f64 = 28.0;
/// How much of a black key's width is black; the rest is the white keys beside it.
const BLACK_PART: f64 = 0.6;
/// The narrowest note with room for its name, in points.
const NAMED_NOTE: f64 = 20.0;

/// The room the window gives the roll, as midi.slint's `PianoRoll` lays it out, in points: the whole roll's size (its
/// well, edge and margins and all), its margins, and the size of the names it lays on the picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Room {
    pub width: f64,
    pub height: f64,
    /// How far in from the roll's left the picture starts (`Space.xs`).
    pub left: f64,
    /// How far in from the roll's top the picture starts, and from its right and bottom it ends (`Space.sm`).
    pub margin: f64,
    /// The names' size (`Type.tiny`): a key's row this tall has room for one.
    pub name: f64,
}

impl Room {
    /// Whether the picture has room for a point each way.
    pub fn fits(&self) -> bool {
        self.width - self.left - self.margin >= 1.0 && self.height - 2.0 * self.margin >= 1.0
    }
}

/// The roll's two choices, as the window keeps them (`MidiData.roll-keyboard` and `roll-names`): a keyboard down its
/// side, and each note's name on it (each key's too, where its row has room).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub keyboard: bool,
    pub names: bool,
}

/// The colours of the roll, as the window's theme has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Colours {
    /// Under everything (0xRRGGBB): the well the roll sits in (`Theme.well`).
    pub well: u32,
    /// Over the white keys' rows, so the black keys' rows show darker (`Theme.shine`).
    pub shine: Colour,
    /// The lines under each key, under each C across the roll, and at each bar (`Theme.edge`).
    pub edge: Colour,
    /// A piano's keys in either theme: white keys light, black keys dark.
    pub white_key: Colour,
    pub black_key: Colour,
    /// The notes, each showing as much of it as it's played strongly (`Theme.accent`).
    pub accent: Colour,
    /// The notes' names (`Theme.accent-text`).
    pub accent_text: Colour,
}

/// A name laid on the picture: the box it goes in, in points from the roll's top left (not the picture's), its words
/// and colour, and whether it's a key's name, which goes at the bottom of its box on the right, or a note's, which goes
/// halfway down from the left. A name too long for its box is cut at the box's edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub text: String,
    pub colour: Colour,
    pub key: bool,
}

/// The roll worked out: its picture, which goes `left` points in from the roll's left and `margin` from its top (see
/// [`Room`]), and the names on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picture {
    pub scene: Scene,
    pub labels: Vec<Label>,
}

/// The picture of `roll` in the `room` the window gives it, with `choices`, in `colours`, to be drawn at `scale` pixels
/// to a point. A roll with no notes is the empty well.
pub fn picture(roll: &Roll, room: Room, scale: f64, choices: Choices, colours: &Colours) -> Picture {
    let Room { width, height, left, margin, name } = room;
    let mut scene = Scene::new(width - left - margin, height - 2.0 * margin, colours.well);
    let mut labels = Vec::new();
    let keys = if roll.notes.is_empty() || roll.high < roll.low { 0 } else { usize::from(roll.high - roll.low) + 1 };
    if keys == 0 {
        return Picture { scene, labels };
    }
    // The rows: as tall as the roll inside its margins, each key's row as tall as the next.
    let rows = height - 2.0 * margin;
    let row = rows / keys as f64;
    // Where the edge above row `i` is, to the picture's own precision (f32), so one row's bottom is exactly the next
    // one's top there too: no pixel is left between two rows, or filled by both.
    let edge = |i: usize| f64::from((rows * i as f64 / keys as f64) as f32);
    let names_fit = choices.names && row >= name;
    let side = if choices.keyboard { SIDE_WITH_KEYBOARD } else { SIDE };
    // The rows and notes start `side` across the roll, and end a margin before its right.
    let across = side - left;
    let long = width - side - margin;
    // Each key, top to bottom: its key, its row's top and bottom (in the picture), and whether it's black.
    let each: Vec<(u8, f64, f64, bool)> = (0..keys)
        .map(|i| {
            let key = roll.high - i as u8;
            (key, edge(i), edge(i + 1), is_black(key))
        })
        .collect();
    // The keyboard: white keys light, black keys dark and shorter, each with a line under it.
    if choices.keyboard {
        let wide = side - margin;
        for black in [false, true] {
            let fill = if black { colours.black_key } else { colours.white_key };
            for &(_, top, bottom, _) in each.iter().filter(|k| k.3 == black) {
                scene.rect((0.0, top, wide, bottom - top), fill);
            }
        }
        for &(_, _, bottom, black) in &each {
            scene.rect((0.0, bottom - 1.0, if black { wide * BLACK_PART } else { wide }, 1.0), colours.edge);
        }
        // A black key's row past its black part is the white keys beside it.
        for &(_, top, bottom, _) in each.iter().filter(|k| k.3) {
            scene.rect((wide * BLACK_PART, top, wide * (1.0 - BLACK_PART), bottom - top), colours.white_key);
        }
        // Each C named (where an octave starts), and every key when the names fit: a black key's on its black part.
        for &(key, top, bottom, black) in each.iter().filter(|k| names_fit || k.0 % 12 == 0) {
            labels.push(Label {
                x: left + 2.0,
                y: margin + top,
                width: wide * if black { BLACK_PART } else { 1.0 } - 4.0,
                height: bottom - top,
                text: note_name(key),
                colour: if black { colours.white_key } else { colours.black_key },
                key: true,
            });
        }
    }
    // The rows across the roll: the white keys' lighter, so the black keys' show darker; a line under each C.
    for &(_, top, bottom, _) in each.iter().filter(|k| !k.3) {
        scene.rect((across, top, long, bottom - top), colours.shine);
    }
    for &(_, _, bottom, _) in each.iter().filter(|k| k.0 % 12 == 0) {
        scene.rect((across, bottom - 1.0, long, 1.0), colours.edge);
    }
    for &bar in &roll.bars {
        scene.rect((across + long * f64::from(bar), 0.0, 1.0, rows), colours.edge);
    }
    // The notes, each over the ones before, at least 2 points each way so the shortest still shows, with a point
    // between one and the next. Each is moved to the nearest edges between pixels (as a crisp rectangle is, see
    // `shapes::crisp`), at least a pixel each way: its top on its row's.
    let scale = usable_scale(scale);
    let pixel = 1.0 / scale;
    let snap = |v: f64| (v * scale - 0.5).ceil() / scale;
    for n in &roll.notes {
        let x = long * f64::from(n.x);
        let y = edge(usize::from(roll.high.saturating_sub(n.key)).min(keys - 1));
        let (w, h) = ((long * f64::from(n.w) - 1.0).max(2.0), (rows * f64::from(n.h) - 1.0).max(2.0));
        let (left, top) = (snap(across + x), snap(y));
        let (right, bottom) = (snap(across + x + w).max(left + pixel), snap(y + h).max(top + pixel));
        scene.rounded((left, top, right - left, bottom - top), 1.0, strength(colours.accent, n.strength), None);
        if names_fit && long * f64::from(n.w) >= NAMED_NOTE {
            labels.push(Label {
                x: side + x + 2.0,
                y: margin + y,
                width: w - 4.0,
                height: h,
                text: note_name(n.key),
                colour: colours.accent_text,
                key: false,
            });
        }
    }
    Picture { scene, labels }
}

/// `colour` showing as strongly as a note played `strength` (0 to 1) hard, to the nearest of its 256 steps, as the
/// window had it (`transparentize(1 - strength)`).
fn strength(colour: Colour, strength: f32) -> Colour {
    let shows = (f32::from(colour.a) / 255.0 * strength).clamp(0.0, 1.0);
    Colour { a: (shows * 255.0).round() as u8, ..colour }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::RollNote;
    use crate::shapes::Shape;

    fn colours() -> Colours {
        Colours {
            well: 0x0E0E0E,
            shine: Colour { r: 255, g: 255, b: 255, a: 0x12 },
            edge: Colour::rgb(0x0B0B0B, 1.0),
            white_key: Colour::rgb(0xA3A3A3, 1.0),
            black_key: Colour::rgb(0x121212, 1.0),
            accent: Colour::rgb(0x3DDC97, 1.0),
            accent_text: Colour::rgb(0x06180F, 1.0),
        }
    }

    /// Two notes, C4 half a bar long and a short Db4 halfway, on a roll from B3 (59) to C5 (72): 14 keys, two Cs.
    fn roll() -> Roll {
        let note = |x: f32, key: u8, w: f32, strength: f32| RollNote {
            x,
            y: f32::from(72 - key) / 14.0,
            w,
            h: 1.0 / 14.0,
            strength,
            key,
        };
        Roll {
            notes: vec![note(0.0, 60, 0.5, 1.0), note(0.5, 61, 0.01, 0.4)],
            bars: vec![0.5],
            low: 59,
            high: 72,
            left_off: 0,
        }
    }

    /// A roll `width` by `height` points with midi.slint's margins (4 points at the left, 8 round the rest) and names
    /// 11 points high.
    fn room(width: f64, height: f64) -> Room {
        Room { width, height, left: 4.0, margin: 8.0, name: 11.0 }
    }

    const KEYBOARD: Choices = Choices { keyboard: true, names: false };
    const NAMES: Choices = Choices { keyboard: true, names: true };
    /// A row's height on a roll 200 points tall: 184 inside the margins, for 14 keys.
    const ROW: f64 = 184.0 / 14.0;

    fn rects(p: &Picture, colour: Colour) -> Vec<[f64; 4]> {
        p.scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Shape::Rect { x, y, w, h, fill } if fill == colour => Some([x, y, w, h].map(f64::from)),
                _ => None,
            })
            .collect()
    }

    /// Whether two places agree, to the picture's precision.
    fn near(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    fn has(list: &[[f64; 4]], place: [f64; 4]) -> bool {
        list.iter().any(|r| near(*r, place))
    }

    #[test]
    fn the_picture_fills_the_roll_inside_its_margins_row_by_row() {
        let c = colours();
        let p = picture(&roll(), room(400.0, 200.0), 1.0, KEYBOARD, &c);
        assert_eq!((p.scene.width, p.scene.height, p.scene.background), (388.0, 184.0, 0x0E0E0E));
        // A key for each row, each one ending exactly where the next starts, in the picture's own numbers, so at any
        // scale no pixel is left between two rows or filled by both.
        let mut keys: Vec<(f32, f32)> = p
            .scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Shape::Rect { x: 0.0, y, w: 44.0, h, fill } if fill == c.white_key || fill == c.black_key => {
                    Some((y, h))
                }
                _ => None,
            })
            .collect();
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(keys.len(), 14);
        assert_eq!(keys[0].0, 0.0);
        for pair in keys.windows(2) {
            assert_eq!(pair[0].0 + pair[0].1, pair[1].0, "{pair:?}");
        }
        assert_eq!(keys[13].0 + keys[13].1, 184.0);
        // With other margins, the picture and the names move with them.
        let p = picture(&roll(), Room { left: 6.0, margin: 10.0, ..room(400.0, 200.0) }, 1.0, KEYBOARD, &c);
        assert_eq!((p.scene.width, p.scene.height), (384.0, 180.0));
        assert_eq!((p.labels[0].text.as_str(), p.labels[0].x, p.labels[0].y), ("C5", 8.0, 10.0));
        assert!(!Room { height: 16.5, ..room(400.0, 200.0) }.fits() && room(13.0, 17.0).fits());
    }

    #[test]
    fn the_keyboard_rows_and_lines_are_where_the_window_had_them() {
        let c = colours();
        let p = picture(&roll(), room(400.0, 200.0), 1.0, KEYBOARD, &c);
        // C5, the top key, is white; Db4 (61), the 12th from the top, black, with the white keys beside it past 60%.
        let white = rects(&p, c.white_key);
        assert!(has(&white, [0.0, 0.0, 44.0, ROW]), "{white:?}");
        assert!(has(&rects(&p, c.black_key), [0.0, 11.0 * ROW, 44.0, ROW]));
        assert!(has(&white, [44.0 * 0.6, 11.0 * ROW, 44.0 * 0.4, ROW]));
        // The rows across start past the keyboard (52 points into the roll, 48 into the picture) and end 8 points before
        // the roll's right: one for each white key.
        let shine = rects(&p, c.shine);
        assert_eq!(shine.len(), 9, "B3 C4 D4 E4 F4 G4 A4 B4 C5");
        assert!(shine.iter().all(|r| r[0] == 48.0 && r[2] == 340.0), "{shine:?}");
        // Lines: under each key on the keyboard (a black key's under its black part), under each C across the roll
        // (C4 and C5), and at the bar, halfway across.
        let edges = rects(&p, c.edge);
        assert_eq!(edges.len(), 14 + 2 + 1);
        assert!(has(&edges, [0.0, 12.0 * ROW - 1.0, 44.0 * 0.6, 1.0]), "under Db4's black part");
        assert!(has(&edges, [48.0, ROW - 1.0, 340.0, 1.0]), "under C5, across");
        assert!(has(&edges, [48.0 + 170.0, 0.0, 1.0, 184.0]), "the bar");
    }

    #[test]
    fn notes_are_as_strong_as_they_are_played_and_never_too_small_to_see() {
        let p = picture(&roll(), room(400.0, 200.0), 1.0, KEYBOARD, &colours());
        let notes: Vec<_> = p
            .scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Shape::Rounded { x, y, w, h, radius, fill, edge } => {
                    Some(([x, y, w, h].map(f64::from), radius, fill, edge))
                }
                _ => None,
            })
            .collect();
        assert_eq!(notes.len(), 2);
        // C4: half the bar long and its row's height (157.7 to 170.9 points down), both less a point so the next note
        // shows apart from it, each edge on the nearest edge between two pixels.
        assert_eq!(notes[0].0, [48.0, 158.0, 169.0, 12.0], "{:?}", notes[0]);
        assert_eq!((notes[0].1, notes[0].2.a, notes[0].3), (1.0, 255, None));
        // Db4: 3.4 points long (340 × 0.01), less a point: 218 to 220.4, so 2 pixels; played softly (0.4 of 255 shows).
        assert_eq!(notes[1].0, [218.0, 145.0, 2.0, 12.0], "{:?}", notes[1]);
        assert_eq!(notes[1].2.a, 102);
        // A note smaller than 2 points each way is drawn 2 by 2.
        let tiny = Roll { notes: vec![RollNote { w: 0.001, h: 0.001, ..roll().notes[0] }], ..roll() };
        let p = picture(&tiny, room(400.0, 200.0), 1.0, Choices::default(), &colours());
        assert!(matches!(p.scene.shapes.last(), Some(Shape::Rounded { w, h, .. }) if *w == 2.0 && *h == 2.0));
    }

    #[test]
    fn at_any_scale_a_note_fills_whole_pixels() {
        for scale in [1.0, 1.25, 1.5, 2.0, 0.8] {
            let p = picture(&roll(), room(401.3, 213.7), scale, NAMES, &colours());
            for s in &p.scene.shapes {
                if let Shape::Rounded { x, y, w, h, .. } = *s {
                    let pixels = [x, y, x + w, y + h].map(|v| f64::from(v) * scale);
                    assert!(pixels.iter().all(|v| (v - v.round()).abs() < 1e-3), "{scale}: {pixels:?}");
                    assert!(
                        w as f64 * scale >= 0.999 && h as f64 * scale >= 0.999,
                        "{scale}: at least a pixel each way"
                    );
                }
            }
        }
    }

    #[test]
    fn the_cs_are_named_and_every_key_and_long_note_once_names_fit() {
        let c = colours();
        let p = picture(&roll(), room(400.0, 200.0), 1.0, KEYBOARD, &c);
        assert_eq!(p.labels.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["C5", "C4"]);
        let c4 = &p.labels[1];
        assert!(near([c4.x, c4.y, c4.width, c4.height], [6.0, 8.0 + 12.0 * ROW, 40.0, ROW]), "{c4:?}");
        assert!(c4.key && c4.colour == c.black_key);
        // Names on, and a row (13.1 points) has room for one: every key, a black key's on its black part; and the note
        // long enough for its name (C4, 170 points), not the short one (3.4 points).
        let p = picture(&roll(), room(400.0, 200.0), 1.0, NAMES, &c);
        assert_eq!(p.labels.iter().filter(|l| l.key).count(), 14);
        let db4 = p.labels.iter().find(|l| l.text == "Db4").expect("Db4 named");
        assert!((db4.width - (44.0 * 0.6 - 4.0)).abs() < 1e-9 && db4.colour == c.white_key);
        let notes: Vec<_> = p.labels.iter().filter(|l| !l.key).collect();
        assert_eq!(notes.len(), 1);
        let n = notes[0];
        assert_eq!((n.text.as_str(), n.colour), ("C4", c.accent_text));
        assert!(near([n.x, n.y, n.width, n.height], [54.0, 8.0 + 12.0 * ROW, 165.0, ROW - 1.0]), "{n:?}");
        // A roll too short for a name in each row: only the Cs, and no note's name.
        let p = picture(&roll(), room(400.0, 150.0), 1.0, NAMES, &c);
        assert_eq!(p.labels.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["C5", "C4"]);
    }

    #[test]
    fn with_no_keyboard_the_rows_start_nearer_the_left_and_no_key_is_named() {
        let c = colours();
        let p = picture(&roll(), room(400.0, 200.0), 1.0, Choices { keyboard: false, names: true }, &c);
        assert!(rects(&p, c.black_key).is_empty());
        let shine = rects(&p, c.shine);
        assert!(shine.iter().all(|r| r[0] == 24.0 && r[2] == 364.0), "{shine:?}");
        assert_eq!(rects(&p, c.edge).len(), 2 + 1, "the Cs' lines and the bar's");
        assert!(p.labels.iter().all(|l| !l.key));
        assert_eq!(p.labels.iter().map(|l| l.x).collect::<Vec<_>>(), [30.0], "C4's name, 28 + 2 points in");
    }

    #[test]
    fn a_roll_with_no_notes_is_the_empty_well() {
        let p = picture(&Roll::default(), room(400.0, 200.0), 1.0, NAMES, &colours());
        assert!(p.scene.shapes.is_empty() && p.labels.is_empty());
        assert_eq!((p.scene.width, p.scene.height), (388.0, 184.0));
    }

    #[test]
    fn a_note_shows_as_the_window_showed_it() {
        let accent = Colour::rgb(0x3DDC97, 1.0);
        assert_eq!(strength(accent, 1.0), accent);
        assert_eq!(strength(accent, 0.4).a, 102);
        assert_eq!(strength(accent, 0.8).a, 204);
        assert_eq!(strength(Colour { a: 128, ..accent }, 0.5).a, 64);
        assert_eq!(strength(accent, 2.0).a, 255, "never more than all of it");
    }
}
