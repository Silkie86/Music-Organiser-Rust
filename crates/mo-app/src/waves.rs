//! A sound's waveform as the graphics card draws it (mo-ui's `waves`): the thin lines [`crate::sounds::wave_path`]
//! makes for the window, read back and laid out the way the window lays out that path (Slint's `fit: fill`), in pixels,
//! just where the window would draw them. The window draws the path itself where there's no card.

use crate::shapes::{Colour, Scene};

/// How tall the path is in its own units (`wave_path` draws each line about the middle, 50).
const PATH_HEIGHT: f64 = 100.0;

/// One line of a waveform, in the path's own units: across (from 0 to the number of peaks) and from its top to its
/// bottom (0 to 100).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveLine {
    pub x: f32,
    pub top: f32,
    pub bottom: f32,
}

/// The lines of a path [`crate::sounds::wave_path`] made (`M0.5 12.0V88.0M1.5 …`). `None` for no lines or anything
/// else, which the window then draws as it is.
pub fn lines(shape: &str) -> Option<Vec<WaveLine>> {
    let mut out = Vec::new();
    let mut rest = shape.trim();
    while !rest.is_empty() {
        let after = rest.strip_prefix('M')?;
        let (x, after) = after.split_once(' ')?;
        let (top, after) = after.split_once('V')?;
        let end = after.find('M').unwrap_or(after.len());
        let line = WaveLine { x: number(x)?, top: number(top)?, bottom: number(&after[..end])? };
        out.push(line);
        rest = &after[end..];
    }
    (!out.is_empty()).then_some(out)
}

fn number(text: &str) -> Option<f32> {
    text.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

/// The waveform's lines, `line` pixels wide in `tint`, laid out as the window lays out the path in a Waveform `width` by
/// `height` pixels (`points` wide and 100 high, fitted into it less half a line all round), in a picture that holds the
/// Waveform with its corner `at` pixels in from the picture's: from 0 to 1 each way, how far past a whole pixel the
/// Waveform sits in the window. The window puts a picture on a whole pixel, so the lines are drawn that far over, to
/// land just where it would draw the path's. The picture is the Waveform's size in whole pixels and one more each way,
/// whatever `at` is.
pub fn scene(
    lines: &[WaveLine],
    points: u32,
    (width, height): (f64, f64),
    at: (f64, f64),
    line: f64,
    tint: Colour,
) -> Scene {
    let background = u32::from(tint.r) << 16 | u32::from(tint.g) << 8 | u32::from(tint.b);
    let room = |length: f64| if length.is_finite() { length.max(0.0).ceil() + 1.0 } else { 1.0 };
    let mut scene = Scene::new(room(width), room(height), background);
    let over = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let line = if line.is_finite() { line.max(0.0) } else { 0.0 };
    let across = (width - line).max(0.0) / f64::from(points.max(1));
    let down = (height - line).max(0.0) / PATH_HEIGHT;
    let (left, top) = (over(at.0) + line / 2.0, over(at.1) + line / 2.0);
    for l in lines {
        let x = left + f64::from(l.x) * across;
        scene.line((x, top + f64::from(l.top) * down), (x, top + f64::from(l.bottom) * down), line, tint);
    }
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::Shape;
    use crate::sounds::wave_path;

    #[test]
    fn the_lines_read_back_are_the_ones_the_path_was_made_of() {
        let peaks = [0.0, 0.5, -1.0, 0.25];
        let read = lines(&wave_path(&peaks)).expect("its own path");
        // Each peak a line about the middle, as tall as it is loud next to the loudest (at least half a unit each way).
        assert_eq!(
            read,
            [
                WaveLine { x: 0.5, top: 49.5, bottom: 50.5 },
                WaveLine { x: 1.5, top: 25.0, bottom: 75.0 },
                WaveLine { x: 2.5, top: 0.0, bottom: 100.0 },
                WaveLine { x: 3.5, top: 37.5, bottom: 62.5 },
            ]
        );
    }

    #[test]
    fn anything_but_such_a_path_is_left_to_the_window() {
        for shape in ["", "   ", "M0 0L10 10", "M1.5 2V", "M1.5 x V4", "C1 2 3 4"] {
            assert_eq!(lines(shape), None, "{shape:?}");
        }
    }

    #[test]
    fn the_lines_are_laid_out_as_the_window_lays_out_the_path() {
        let read = [WaveLine { x: 0.5, top: 0.0, bottom: 100.0 }, WaveLine { x: 3.5, top: 40.0, bottom: 60.0 }];
        let tint = Colour::rgb(0x4FB3FF, 1.0);
        // 4 peaks in a Waveform 202 by 52 pixels on whole pixels, lines 2 pixels wide: the path fits in 200 by 50, a
        // pixel in from each side, in a picture a pixel bigger each way.
        let s = scene(&read, 4, (202.0, 52.0), (0.0, 0.0), 2.0, tint);
        assert_eq!((s.width, s.height, s.background), (203.0, 53.0, 0x4FB3FF));
        assert_eq!(
            s.shapes,
            [
                Shape::Line { from: (26.0, 1.0), to: (26.0, 51.0), width: 2.0, colour: tint },
                Shape::Line { from: (176.0, 21.0), to: (176.0, 31.0), width: 2.0, colour: tint },
            ]
        );
    }

    /// A Waveform part of a pixel over in the window has its lines drawn that far over in the picture (which the
    /// window puts on a whole pixel), in a picture the same size as at a whole pixel.
    #[test]
    fn the_lines_land_where_the_path_would_past_a_whole_pixel() {
        let read = [WaveLine { x: 0.5, top: 0.0, bottom: 100.0 }];
        let tint = Colour::rgb(0x4FB3FF, 1.0);
        let s = scene(&read, 4, (202.0, 52.0), (0.25, 0.75), 2.0, tint);
        assert_eq!((s.width, s.height), (203.0, 53.0));
        assert_eq!(s.shapes, [Shape::Line { from: (26.25, 1.75), to: (26.25, 51.75), width: 2.0, colour: tint }]);
        // A Waveform part of a pixel wide or high has a picture big enough for it, at any part of a pixel over.
        let s = scene(&read, 4, (201.5, 51.25), (0.984375, 0.984375), 2.0, tint);
        assert_eq!((s.width, s.height), (203.0, 53.0));
        // Less than none over is none, and so is anything that isn't a number.
        let s = scene(&read, 4, (202.0, 52.0), (-3.0, f64::NAN), 2.0, tint);
        assert_eq!(s.shapes, [Shape::Line { from: (26.0, 1.0), to: (26.0, 51.0), width: 2.0, colour: tint }]);
    }
}
