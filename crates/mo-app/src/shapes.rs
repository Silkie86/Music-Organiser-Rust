//! Shapes worked out once and drawn by either painter: the graphics card's (mo-ui's `canvas`, spec 9.1) or, where
//! there's no card to draw with (the screenshots, the tests, an old driver, `MO_DRAW_CPU`), the processor's here, with
//! tiny-skia. A view the app draws itself (the arrangement first) says what goes where as a [`Scene`], in points, and
//! both painters draw that same scene, so the picture looks the same either way.

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// A colour and how much of it shows (`a`, 0 to 255). Not premultiplied: each painter does that its own way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Colour {
    /// `rgb` (0xRRGGBB) showing `alpha` (0 to 1).
    pub fn rgb(rgb: u32, alpha: f64) -> Colour {
        let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        Colour { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8, a }
    }

    fn skia(self) -> Color {
        Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    /// As the card reads it (WGSL's `unpack4x8unorm`): red in the lowest byte, how much shows in the highest.
    pub fn packed(self) -> u32 {
        u32::from_le_bytes([self.r, self.g, self.b, self.a])
    }
}

/// One shape, in points from the picture's top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A rectangle filled crisp: a pixel is filled when its middle is inside, so its edges stay sharp (bar lines,
    /// notes, a waveform's columns).
    Rect { x: f32, y: f32, w: f32, h: f32, fill: Colour },
    /// A rectangle with rounded corners (`radius` points) and soft edges, filled, with an edge one point wide along its
    /// border when there is one (clips, flags, scroll bars).
    Rounded { x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Colour, edge: Option<Colour> },
    /// A straight line `width` points wide, with soft edges and square ends (automation curves).
    Line { from: (f32, f32), to: (f32, f32), width: f32, colour: Colour },
}

/// What kind of shape each is on the card (mo-ui's canvas shader has the same numbers).
pub const RECT: u32 = 0;
pub const ROUNDED: u32 = 1;
pub const LINE: u32 = 2;
/// How many bytes a shape takes on the card (see [`Scene::pack`]).
pub const SHAPE_BYTES: usize = 32;
/// The biggest picture either painter makes, in pixels each way.
pub const MOST_PIXELS: u32 = 16384;

/// A picture to draw: its size in points, the colour under everything (0xRRGGBB) and its shapes, drawn in order, each
/// over the ones before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub background: u32,
    pub shapes: Vec<Shape>,
}

impl Scene {
    pub fn new(width: f64, height: f64, background: u32) -> Scene {
        Scene { width, height, background, shapes: Vec::new() }
    }

    /// A crisp rectangle (left, top, width, height); one with no size, or not a number, is left out.
    pub fn rect(&mut self, (x, y, w, h): (f64, f64, f64, f64), fill: Colour) {
        if usable(x, y, w, h) && fill.a > 0 {
            self.shapes.push(Shape::Rect { x: x as f32, y: y as f32, w: w as f32, h: h as f32, fill });
        }
    }

    /// A rounded rectangle (left, top, width, height); its corners are never rounder than half its width or height.
    pub fn rounded(&mut self, (x, y, w, h): (f64, f64, f64, f64), radius: f64, fill: Colour, edge: Option<Colour>) {
        if usable(x, y, w, h) {
            let radius = radius.min(w / 2.0).min(h / 2.0).max(0.0) as f32;
            let edge = edge.filter(|e| e.a > 0);
            if fill.a > 0 || edge.is_some() {
                let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
                self.shapes.push(Shape::Rounded { x, y, w, h, radius, fill, edge });
            }
        }
    }

    /// A line; one of no length (a curve only touching the box it's cut to) is left out, as it would draw nothing.
    pub fn line(&mut self, from: (f64, f64), to: (f64, f64), width: f64, colour: Colour) {
        let finite = [from.0, from.1, to.0, to.1, width].iter().all(|v| v.is_finite());
        let long = (to.0 - from.0).hypot(to.1 - from.1) > 1e-6;
        if finite && long && width > 0.0 && colour.a > 0 {
            let from = (from.0 as f32, from.1 as f32);
            let to = (to.0 as f32, to.1 as f32);
            self.shapes.push(Shape::Line { from, to, width: width as f32, colour });
        }
    }

    /// How big the picture is at `scale` pixels to a point: whole pixels, at least 1 and at most [`MOST_PIXELS`] each
    /// way.
    pub fn pixel_size(&self, scale: f64) -> (u32, u32) {
        let scale = usable_scale(scale);
        let size = |points: f64| (points * scale).ceil().clamp(1.0, f64::from(MOST_PIXELS)) as u32;
        (size(self.width), size(self.height))
    }

    /// The shapes as the graphics card reads them, [`SHAPE_BYTES`] each: where (four f32, in points: a rectangle's
    /// left, top, right and bottom, or a line's two ends), the fill and the edge colours (as [`Colour::packed`]; a line's
    /// colour is its fill, and no edge is 0), the corner radius or the line's width (f32, in points) and the kind
    /// ([`RECT`], [`ROUNDED`] or [`LINE`], u32).
    pub fn pack(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.shapes.len() * SHAPE_BYTES);
        let mut put = |at: [f32; 4], fill: Colour, edge: Option<Colour>, size: f32, kind: u32| {
            for v in at {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&fill.packed().to_le_bytes());
            out.extend_from_slice(&edge.map_or(0, Colour::packed).to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
        };
        for s in &self.shapes {
            match *s {
                Shape::Rect { x, y, w, h, fill } => put([x, y, x + w, y + h], fill, None, 0.0, RECT),
                Shape::Rounded { x, y, w, h, radius, fill, edge } => {
                    put([x, y, x + w, y + h], fill, edge, radius, ROUNDED);
                }
                Shape::Line { from, to, width, colour } => put([from.0, from.1, to.0, to.1], colour, None, width, LINE),
            }
        }
        out
    }

    /// Draws the scene on the processor at `scale` pixels to a point. Crisp rectangles in a row of one colour are
    /// filled as one shape, and so are lines in a row of one colour and width, so where they overlap they don't build
    /// up (the card draws each on its own, which only differs where see-through ones overlap).
    pub fn paint(&self, scale: f64) -> Pixmap {
        let scale = usable_scale(scale);
        let (w, h) = self.pixel_size(scale);
        // 1 by 1 is always possible.
        let mut pixmap = Pixmap::new(w, h).unwrap_or_else(|| Pixmap::new(1, 1).unwrap_or_else(|| unreachable!()));
        pixmap.fill(Colour::rgb(self.background, 1.0).skia());
        let mut i = 0;
        while i < self.shapes.len() {
            // The run of shapes that are drawn together with this one.
            let run = self.shapes[i..].iter().take_while(|s| together(&self.shapes[i], s)).count().max(1);
            paint_run(&mut pixmap, &self.shapes[i..i + run], scale as f32);
            i += run;
        }
        pixmap
    }
}

/// The pixels a crisp rectangle (left, top, right, bottom, in points) fills at `scale` pixels to a point, as whole-pixel
/// edges: the pixels whose middles are inside it, counting a middle on its left or top edge in and one on its right or
/// bottom edge out. `None` when that's no pixels. The card's shader works the edges out the same way (mo-ui's
/// `canvas`), so both painters fill the same pixels, however close an edge comes to a pixel's middle.
pub fn crisp([left, top, right, bottom]: [f32; 4], scale: f32) -> Option<[f32; 4]> {
    let edge = |v: f32| (v * scale - 0.5).ceil();
    let (l, t, r, b) = (edge(left), edge(top), edge(right), edge(bottom));
    (r > l && b > t).then_some([l, t, r, b])
}

fn usable(x: f64, y: f64, w: f64, h: f64) -> bool {
    [x, y, w, h].iter().all(|v| v.is_finite()) && w > 0.0 && h > 0.0
}

fn usable_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 { scale } else { 1.0 }
}

/// Whether `s` is drawn as one shape with `first`: crisp rectangles of one colour, or lines of one colour and width.
fn together(first: &Shape, s: &Shape) -> bool {
    match (first, s) {
        (Shape::Rect { fill: a, .. }, Shape::Rect { fill: b, .. }) => a == b,
        (Shape::Line { colour: a, width: wa, .. }, Shape::Line { colour: b, width: wb, .. }) => a == b && wa == wb,
        _ => false,
    }
}

/// Draws shapes drawn together (see [`together`]) at `scale` pixels to a point.
fn paint_run(pixmap: &mut Pixmap, run: &[Shape], scale: f32) {
    let transform = Transform::from_scale(scale, scale);
    match run {
        // In pixels, with edges on whole pixels (see `crisp`), so no middle of a pixel is ever on an edge.
        [Shape::Rect { fill, .. }, ..] => {
            let mut pb = PathBuilder::new();
            for s in run {
                if let Shape::Rect { x, y, w, h, .. } = *s
                    && let Some([l, t, r, b]) = crisp([x, y, x + w, y + h], scale)
                    && let Some(r) = Rect::from_ltrb(l, t, r, b)
                {
                    pb.push_rect(r);
                }
            }
            if let Some(path) = pb.finish() {
                let paint =
                    Paint { shader: tiny_skia::Shader::SolidColor(fill.skia()), anti_alias: false, ..Paint::default() };
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
        [Shape::Rounded { x, y, w, h, radius, fill, edge }] => {
            let (x, y, w, h, rad) = (*x, *y, *w, *h, *radius);
            let mut pb = PathBuilder::new();
            pb.move_to(x + rad, y);
            pb.line_to(x + w - rad, y);
            pb.quad_to(x + w, y, x + w, y + rad);
            pb.line_to(x + w, y + h - rad);
            pb.quad_to(x + w, y + h, x + w - rad, y + h);
            pb.line_to(x + rad, y + h);
            pb.quad_to(x, y + h, x, y + h - rad);
            pb.line_to(x, y + rad);
            pb.quad_to(x, y, x + rad, y);
            pb.close();
            let Some(path) = pb.finish() else { return };
            let mut paint =
                Paint { shader: tiny_skia::Shader::SolidColor(fill.skia()), anti_alias: true, ..Paint::default() };
            if fill.a > 0 {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
            if let Some(e) = edge {
                paint.shader = tiny_skia::Shader::SolidColor(e.skia());
                let stroke = Stroke { width: 1.0, ..Stroke::default() };
                pixmap.stroke_path(&path, &paint, &stroke, transform, None);
            }
        }
        [Shape::Line { width, colour, .. }, ..] => {
            let mut pb = PathBuilder::new();
            for s in run {
                if let Shape::Line { from, to, .. } = *s {
                    pb.move_to(from.0, from.1);
                    pb.line_to(to.0, to.1);
                }
            }
            let Some(path) = pb.finish() else { return };
            let paint =
                Paint { shader: tiny_skia::Shader::SolidColor(colour.skia()), anti_alias: true, ..Paint::default() };
            let stroke = Stroke { width: *width, ..Stroke::default() };
            pixmap.stroke_path(&path, &paint, &stroke, transform, None);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).expect("inside");
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn colours_pack_red_first_for_the_card() {
        let c = Colour::rgb(0x112233, 1.0);
        assert_eq!((c.r, c.g, c.b, c.a), (0x11, 0x22, 0x33, 255));
        assert_eq!(c.packed(), 0xFF33_2211);
        assert_eq!(Colour::rgb(0xFFFFFF, 0.5).a, 128);
        assert_eq!(Colour::rgb(0, 7.0).a, 255, "how much shows is kept to 0 to 1");
    }

    #[test]
    fn shapes_with_nothing_to_draw_are_left_out() {
        let mut s = Scene::new(10.0, 10.0, 0);
        let red = Colour::rgb(0xFF0000, 1.0);
        s.rect((0.0, 0.0, 0.0, 5.0), red);
        s.rect((0.0, 0.0, 5.0, f64::NAN), red);
        s.rect((0.0, 0.0, 5.0, 5.0), Colour::rgb(0xFF0000, 0.0));
        s.rounded((0.0, 0.0, 5.0, 5.0), 2.0, Colour::default(), None);
        s.line((1.0, 1.0), (1.0, 1.0), 1.5, red);
        assert!(s.shapes.is_empty(), "{:?}", s.shapes);
        // A rounded rectangle's corners fit it.
        s.rounded((0.0, 0.0, 4.0, 2.0), 3.0, red, None);
        assert!(matches!(s.shapes[0], Shape::Rounded { radius, .. } if radius == 1.0));
    }

    #[test]
    fn the_card_gets_32_bytes_a_shape() {
        let mut s = Scene::new(10.0, 10.0, 0);
        s.rect((1.0, 2.0, 3.0, 4.0), Colour::rgb(0x102030, 1.0));
        s.rounded((1.0, 1.0, 6.0, 6.0), 2.0, Colour::rgb(0xFFFFFF, 0.5), Some(Colour::rgb(0, 1.0)));
        s.line((0.0, 0.0), (5.0, 5.0), 1.5, Colour::rgb(0xFF0000, 1.0));
        let bytes = s.pack();
        assert_eq!(bytes.len(), 3 * SHAPE_BYTES);
        let f32_at = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        // The rectangle: left, top, right, bottom.
        assert_eq!([f32_at(0), f32_at(4), f32_at(8), f32_at(12)], [1.0, 2.0, 4.0, 6.0]);
        assert_eq!(u32_at(16), Colour::rgb(0x102030, 1.0).packed());
        assert_eq!((u32_at(20), u32_at(28)), (0, RECT));
        // The rounded one has an edge and a radius.
        assert_eq!((u32_at(52), f32_at(56), u32_at(60)), (Colour::rgb(0, 1.0).packed(), 2.0, ROUNDED));
        // The line's ends and width.
        assert_eq!([f32_at(64), f32_at(76), f32_at(88)], [0.0, 5.0, 1.5]);
        assert_eq!(u32_at(92), LINE);
    }

    #[test]
    fn the_processor_paints_each_shape_in_order() {
        let mut s = Scene::new(20.0, 10.0, 0x000000);
        s.rect((0.0, 0.0, 10.0, 10.0), Colour::rgb(0xFF0000, 1.0));
        // Over the red, half see-through.
        s.rect((5.0, 0.0, 10.0, 10.0), Colour::rgb(0x0000FF, 0.5));
        s.rounded((16.0, 2.0, 3.0, 6.0), 1.0, Colour::rgb(0x00FF00, 1.0), None);
        let p = s.paint(2.0);
        assert_eq!((p.width(), p.height()), (40, 20));
        assert_eq!(pixel(&p, 2, 10), [255, 0, 0, 255]);
        let mixed = pixel(&p, 14, 10);
        assert!(mixed[0] > 100 && mixed[2] > 100 && mixed[1] == 0, "{mixed:?}");
        assert_eq!(pixel(&p, 25, 10), [0, 0, 128, 255]);
        assert_eq!(pixel(&p, 35, 10), [0, 255, 0, 255]);
        // Outside every shape: the background.
        assert_eq!(pixel(&p, 39, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn a_crisp_rectangle_fills_the_pixels_whose_middles_are_inside() {
        assert_eq!(crisp([0.0, 0.0, 1.0, 1.0], 1.0), Some([0.0, 0.0, 1.0, 1.0]));
        // A middle on the left or top edge is in, one on the right or bottom edge out.
        assert_eq!(crisp([0.5, 0.5, 1.5, 2.5], 1.0), Some([0.0, 0.0, 1.0, 2.0]));
        // Narrower than a pixel and over no pixel's middle: nothing.
        assert_eq!(crisp([0.6, 0.0, 1.4, 1.0], 1.0), None);
        // At 150%: 1.5 to 3 pixels across (the middles of pixels 1 and 2), 0 to 1.5 down (only row 0's middle).
        assert_eq!(crisp([1.0, 0.0, 2.0, 1.0], 1.5), Some([1.0, 0.0, 3.0, 1.0]));
        // The processor fills just those pixels.
        let mut s = Scene::new(4.0, 2.0, 0x000000);
        s.rect((0.5, 0.0, 1.0, 2.0), Colour::rgb(0xFFFFFF, 1.0));
        let p = s.paint(1.0);
        let row: Vec<_> = (0..4).map(|x| pixel(&p, x, 1)[0]).collect();
        assert_eq!(row, [255, 0, 0, 0]);
    }

    #[test]
    fn a_picture_is_whole_pixels_and_never_empty() {
        let s = Scene::new(10.2, 0.0, 0);
        assert_eq!(s.pixel_size(1.5), (16, 1));
        assert_eq!(s.pixel_size(f64::NAN), (11, 1), "a scale that isn't a number counts as 1");
        assert_eq!(Scene::new(1e9, 5.0, 0).pixel_size(1.0), (MOST_PIXELS, 5));
    }
}
