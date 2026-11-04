//! The sounds' waveforms on the graphics card. Each `Waveform` on a page (studio.slint) asks `Waves.picture` for a
//! picture of its lines, at its size in pixels: drawn once on the card and kept, the window shows it like any picture,
//! instead of working out every line of every waveform again for each frame. The lines are drawn the way the window's
//! drawing library draws the path, so the picture looks as the path does. Where there's no card (the screenshots, an
//! old driver, `MO_DRAW_CPU`), while the card's painter is being set up, once the card refused, or once a frame has
//! drawn all the new pictures it has time for, there's no picture and the `Waveform` draws its path itself, as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use mo_app::log;
use mo_app::shapes::{Colour, MOST_PIXELS};
use mo_app::waves;
use slint::{ComponentHandle, Image, SharedString};

use crate::canvas::{Canvas, Kind};
use crate::{AppWindow, Waves, gpu};

/// How much room on the card the pictures kept may take: the tiles of two 4K screens (a tile's picture is a few tens of
/// kilobytes), so scrolling back finds them drawn already. A picture asked for in the frame being drawn is on show and
/// stays, even past this.
const KEEP_BYTES: u64 = 64 << 20;

/// How long one frame may spend drawing new pictures. When it needs more (the window being resized, a panel opened, a
/// jump down a long list: every waveform on show at a new size, or new), the rest draw their paths in that frame, which
/// look the same, and ask again for the next one, so the window doesn't wait on the card.
const FRAME_TIME: Duration = Duration::from_millis(4);

/// What a picture shows: a sound's waveform (its path and how many points across), its tint, and which card it's on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Shows {
    shape: SharedString,
    points: i32,
    tint: u32,
    card: u64,
}

/// A picture's width and height, its lines' width and the width of their soft edges, in pixels, to the bit: a picture
/// is only shown again at exactly its size.
type Size = [u32; 4];

/// A picture kept, at `size`, taking `bytes` on the card, last asked for at ask number `asked`.
struct Picture {
    size: Size,
    image: Image,
    bytes: u64,
    asked: u64,
}

/// The pictures drawn: one for each sound's waveform, at the size it was last drawn at (a new size, as the window is
/// resized, takes the old one's place; a sound's tile and its card take turns), with when each was last asked for.
/// Past the room they may take, the least lately asked for go first, but never one asked for in this frame.
#[derive(Default)]
struct Kept {
    pictures: HashMap<Shows, Picture>,
    /// How many bytes the pictures take on the card, in all.
    bytes: u64,
    /// The asks, counted, so it's known which picture was asked for least lately.
    asks: u64,
    /// The count when this frame started: a picture asked for since then is on show.
    frame_started: u64,
}

impl Kept {
    /// The picture of `shows` at `size`, if it's kept.
    fn get(&mut self, shows: &Shows, size: Size) -> Option<Image> {
        self.asks += 1;
        let asks = self.asks;
        match self.pictures.get_mut(shows) {
            Some(kept) if kept.size == size => {
                kept.asked = asks;
                Some(kept.image.clone())
            }
            _ => None,
        }
    }

    /// Keeps the picture of `shows` at `size`, `bytes` on the card, making room for it within `most` bytes.
    fn keep(&mut self, shows: Shows, size: Size, image: Image, bytes: u64, most: u64) {
        if let Some(old) = self.pictures.remove(&shows) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        while self.bytes + bytes > most
            && let Some((oldest, asked)) =
                self.pictures.iter().min_by_key(|(_, p)| p.asked).map(|(k, p)| (k.clone(), p.asked))
            && asked <= self.frame_started
        {
            if let Some(gone) = self.pictures.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(gone.bytes);
            }
        }
        // Counted as asked for now, after everything kept before it.
        self.asks += 1;
        self.bytes += bytes;
        self.pictures.insert(shows, Picture { size, image, bytes, asked: self.asks });
    }

    /// A frame starts: what's asked for from now on is on show in it.
    fn frame_starts(&mut self) {
        self.frame_started = self.asks;
    }

    fn clear(&mut self) {
        self.pictures.clear();
        self.bytes = 0;
    }
}

/// What the frame being drawn has spent drawing new pictures, and whether it left waveforms to ask again.
#[derive(Default)]
struct ThisFrame {
    spent: Duration,
    left_some: bool,
}

impl ThisFrame {
    /// Whether there's time left in this frame to draw another picture.
    fn has_time(&self) -> bool {
        self.spent < FRAME_TIME
    }

    /// A waveform is left to the next frame: true for the first one, so every waveform is asked again once.
    fn leave(&mut self) -> bool {
        !std::mem::replace(&mut self.left_some, true)
    }
}

thread_local! {
    /// The painter on the graphics card.
    static CARD: RefCell<gpu::Painter<Canvas>> = const { RefCell::new(gpu::Painter::new()) };
    static KEPT: RefCell<Kept> = RefCell::default();
    static THIS_FRAME: RefCell<ThisFrame> = RefCell::default();
}

/// A frame starts (Slint is about to draw the window): it has [`FRAME_TIME`] to draw new pictures in, and what it asks
/// for is on show.
pub fn frame_starts() {
    THIS_FRAME.with(|f| *f.borrow_mut() = ThisFrame::default());
    KEPT.with(|k| k.borrow_mut().frame_starts());
}

/// How wide the window's drawing library makes the soft edges of a path's lines, in pixels, at `scale` pixels to a
/// point: a pixel divided by the scale rounded up, which is what Slint hands femtovg as its `dpi` (in
/// i-slint-renderer-femtovg's `set_size`), so a whole pixel at 100% and less, and half a pixel from 125% to 200%.
fn fringe(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 { 1.0 / scale.ceil() } else { 1.0 }
}

/// Answers the `Waveform`s' asks for pictures.
pub fn connect(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<Waves>().on_picture(move |shape, points, tint, width, height, line, _asked| {
        picture(&weak, shape, points, tint, (width, height), line).unwrap_or_default()
    });
}

/// A graphics card was handed over: the painter is set up on it, away from the window's thread, and then every
/// `Waveform` asks again, for a picture drawn on it (drawing its path until then).
pub fn graphics_ready(app: &AppWindow) {
    gpu::prepare(app, "Waveforms", &CARD, painter, ask_again);
}

/// The waveforms' painter on a card.
fn painter(gpu: &gpu::Gpu) -> Canvas {
    Canvas::new(&gpu.device, &gpu.queue, Kind::OneColour)
}

/// The card went: the painter and the pictures kept go with it, now, while wgpu is still here (even as the window
/// itself goes).
pub fn card_gone() {
    CARD.with(|c| c.borrow_mut().gone());
    KEPT.with(|k| k.borrow_mut().clear());
}

/// The card went and the window's still here: every `Waveform` asks again, drawing its path until there's a card again.
pub fn graphics_gone(app: &AppWindow) {
    ask_again_later(app.as_weak());
}

/// Every `Waveform` asks for its picture again.
fn ask_again(app: &AppWindow) {
    let waves = app.global::<Waves>();
    waves.set_asked(waves.get_asked().wrapping_add(1));
}

/// Every `Waveform` asks for its picture again, at the event loop's next turn: not now, while the window may be
/// working out what to show.
fn ask_again_later(app: slint::Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = app.upgrade() {
            ask_again(&app);
        }
    });
}

/// The picture of a waveform's lines (`shape`, as `mo_app::sounds::wave_path` makes it, `points` across) in `tint`,
/// `width` by `height` pixels with lines `line` pixels wide: kept from before, or drawn now. `None` where the window
/// draws the path itself instead.
fn picture(
    app: &slint::Weak<AppWindow>,
    shape: SharedString,
    points: i32,
    tint: slint::Color,
    (width, height): (f32, f32),
    line: f32,
) -> Option<Image> {
    let (gpu, generation) = CARD.with(|c| c.borrow().card())?;
    let pixel_size = (pixels(width)?, pixels(height)?);
    let app_window = app.upgrade()?;
    // The soft edges as the window would draw them (and so a new picture when the window's scale changes).
    let fringe = fringe(app_window.window().scale_factor());
    let shows = Shows { shape, points, tint: tint.as_argb_encoded(), card: generation };
    let size = [width.to_bits(), height.to_bits(), line.to_bits(), fringe.to_bits()];
    if let Some(image) = KEPT.with(|k| k.borrow_mut().get(&shows, size)) {
        return Some(image);
    }
    // The painter is set up away from the window's thread: as the card is handed over, or now, if the window draws a
    // waveform before that (its first frame comes before the card is handed over to the views). The window draws the
    // path until it's ready.
    if !CARD.with(|c| c.borrow().made_on(generation)) {
        gpu::prepare(&app_window, "Waveforms", &CARD, painter, ask_again);
        CARD.with(|c| c.borrow().card())?;
    }
    // Enough drawn in this frame: the window draws this one's path, and every waveform asks again for the next.
    if !THIS_FRAME.with(|f| f.borrow().has_time()) {
        if THIS_FRAME.with(|f| f.borrow_mut().leave()) {
            ask_again_later(app.clone());
        }
        return None;
    }
    let started = Instant::now();
    let lines = waves::lines(&shows.shape)?;
    let points = u32::try_from(points).ok().filter(|p| *p > 0)?;
    let colour = Colour { r: tint.red(), g: tint.green(), b: tint.blue(), a: tint.alpha() };
    let scene = waves::scene(&lines, points, (f64::from(width), f64::from(height)), f64::from(line), colour);
    let drawn = CARD.with(|c| -> Result<Image, String> {
        let mut c = c.borrow_mut();
        let canvas = c.on(&gpu, generation, |gpu| {
            // Set up away from the window's thread (above), so only if that couldn't start does the card build the
            // shader here.
            let _doing = crate::watchdog::doing("Waveforms: set up the GPU painter");
            painter(gpu)
        })?;
        let _doing = crate::watchdog::doing("Waveforms: draw on the GPU");
        let texture =
            gpu::checked(&gpu.device, || canvas.draw_new(pixel_size, 1.0, scene.background, &scene.pack(1.0), fringe))?;
        Image::try_from(texture).map_err(|e| format!("Slint wouldn't show the picture: {e}"))
    });
    THIS_FRAME.with(|f| f.borrow_mut().spent += started.elapsed());
    match drawn {
        Ok(image) => {
            let bytes = u64::from(pixel_size.0) * u64::from(pixel_size.1) * 4;
            KEPT.with(|k| k.borrow_mut().keep(shows, size, image.clone(), bytes, KEEP_BYTES));
            Some(image)
        }
        Err(e) => {
            log::error(
                "Waveforms",
                &format!("The graphics card couldn't draw a waveform, so the window draws them: {e}"),
            );
            CARD.with(|c| c.borrow_mut().refused(generation));
            KEPT.with(|k| k.borrow_mut().clear());
            // Every waveform asks again and draws its path.
            ask_again_later(app.clone());
            None
        }
    }
}

/// A size in pixels for the picture: whole pixels, enough to hold `length`. `None` for less than a pixel.
fn pixels(length: f32) -> Option<u32> {
    (length.is_finite() && length >= 1.0).then(|| (length.ceil() as u32).min(MOST_PIXELS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_app::shapes::{Scene, Shape};
    use slint::wgpu_30::wgpu;

    #[test]
    fn a_picture_is_whole_pixels_and_at_least_one() {
        assert_eq!(pixels(200.0), Some(200));
        assert_eq!(pixels(200.25), Some(201));
        assert_eq!(pixels(1.0), Some(1));
        for none in [0.0, 0.99, -3.0, f32::NAN, f32::INFINITY] {
            assert_eq!(pixels(none), None, "{none}");
        }
        assert_eq!(pixels(1e9), Some(MOST_PIXELS));
    }

    /// The soft edges are as wide as the window's drawing library makes them: a pixel at 100% and less, half a pixel
    /// from 125% to 200%, a third at 250% and 300%.
    #[test]
    fn soft_edges_are_as_wide_as_the_windows() {
        for (scale, wide) in
            [(1.0, 1.0), (0.8, 1.0), (1.25, 0.5), (1.5, 0.5), (2.0, 0.5), (2.5, 1.0 / 3.0), (3.0, 1.0 / 3.0)]
        {
            assert_eq!(fringe(scale), wide, "at {scale}");
        }
        for odd in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(fringe(odd), 1.0, "{odd}");
        }
    }

    fn shows(n: usize) -> Shows {
        Shows { shape: "M0.5 10.0V90.0".into(), points: n as i32, tint: 0xFF4FB3FF, card: 1 }
    }

    fn at(width: f32) -> Size {
        [width.to_bits(), 40f32.to_bits(), 1f32.to_bits(), 1f32.to_bits()]
    }

    /// Once the pictures fill the room they may take, the least lately asked for go first, as many as the new one
    /// needs the room of.
    #[test]
    fn the_pictures_kept_are_the_ones_asked_for_lately() {
        let mut kept = Kept::default();
        for n in 0..10 {
            kept.keep(shows(n), at(200.0), Image::default(), 100, 1000);
        }
        kept.frame_starts();
        // The first is asked for again, so the second is the least lately asked for.
        assert!(kept.get(&shows(0), at(200.0)).is_some());
        kept.frame_starts();
        kept.keep(shows(99), at(200.0), Image::default(), 100, 1000);
        assert_eq!((kept.pictures.len(), kept.bytes), (10, 1000));
        assert!(kept.get(&shows(0), at(200.0)).is_some(), "asked for lately");
        assert!(kept.get(&shows(1), at(200.0)).is_none(), "the least lately asked for went");
        assert!(kept.get(&shows(99), at(200.0)).is_some());
        // A big one takes the room of several.
        kept.frame_starts();
        kept.keep(shows(100), at(200.0), Image::default(), 350, 1000);
        assert_eq!((kept.pictures.len(), kept.bytes), (7, 950));
        // On another card, it's another picture.
        assert!(kept.get(&Shows { card: 2, ..shows(0) }, at(200.0)).is_none());
    }

    /// Pictures asked for in the frame being drawn are on show, so none of them goes to make room, even with more on
    /// show than the room holds (a screen bigger than two 4K ones): a page of tiles all asking again, in order, finds
    /// every one, instead of each pushing out the next.
    #[test]
    fn a_picture_on_show_stays() {
        let mut kept = Kept::default();
        kept.frame_starts();
        for n in 0..20 {
            kept.keep(shows(n), at(200.0), Image::default(), 100, 1000);
        }
        assert_eq!(kept.pictures.len(), 20, "all on show");
        kept.frame_starts();
        for n in 0..20 {
            assert!(kept.get(&shows(n), at(200.0)).is_some(), "{n} found again");
        }
        // Once they aren't on show any more, the room counts again.
        kept.frame_starts();
        kept.keep(shows(99), at(200.0), Image::default(), 100, 1000);
        assert_eq!((kept.pictures.len(), kept.bytes), (10, 1000));
        assert!(kept.get(&shows(99), at(200.0)).is_some());
    }

    /// A waveform drawn at a new size (the window resized) takes the place of its picture at the old size, instead of
    /// filling the room with sizes nothing shows any more.
    #[test]
    fn a_new_size_takes_the_old_ones_place() {
        let mut kept = Kept::default();
        for step in 0..100 {
            let width = 200.0 + step as f32 * 0.25;
            kept.frame_starts();
            assert!(kept.get(&shows(0), at(width)).is_none(), "not drawn at {width} yet");
            kept.keep(shows(0), at(width), Image::default(), 100, 1000);
        }
        assert_eq!((kept.pictures.len(), kept.bytes), (1, 100), "one picture, at the size it's at now");
        let last = at(200.0 + 99.0 * 0.25);
        assert!(kept.get(&shows(0), last).is_some());
        assert!(kept.get(&shows(0), at(200.0)).is_none(), "a hair off is another picture");
        let softer = [last[0], last[1], last[2], 0.5f32.to_bits()];
        assert!(kept.get(&shows(0), softer).is_none(), "so are softer edges (the window's scale changed)");
        // A full room doesn't lose a picture to a waveform drawn again at a new size.
        for n in 1..10 {
            kept.keep(shows(n), at(200.0), Image::default(), 100, 1000);
        }
        kept.frame_starts();
        kept.keep(shows(0), at(321.0), Image::default(), 100, 1000);
        assert_eq!((kept.pictures.len(), kept.bytes), (10, 1000));
        assert!(kept.get(&shows(1), at(200.0)).is_some(), "still kept");
        assert!(kept.get(&shows(0), at(321.0)).is_some());
    }

    /// A frame draws new pictures while it has time; after that, waveforms are left to the next frame, and the first
    /// one left has every waveform ask again (once).
    #[test]
    fn a_frame_draws_pictures_while_it_has_time() {
        let mut frame = ThisFrame::default();
        assert!(frame.has_time());
        frame.spent += FRAME_TIME / 2;
        assert!(frame.has_time());
        frame.spent += FRAME_TIME / 2;
        assert!(!frame.has_time(), "its time is up");
        assert!(frame.leave(), "the first one left asks again");
        assert!(!frame.leave(), "once");
    }

    /// The window drawing a waveform's path itself, as where there's no picture: Slint hands the path to femtovg, its
    /// drawing library (the patched copy in vendor/femtovg), as lines where the path is fitted, as wide as the Waveform
    /// says, with square ends, on a canvas set up at the window's scale rounded up (as Slint sets it up), here on the
    /// same card. How much of the tint shows in each pixel is the alpha read back.
    fn as_the_window_draws(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        scale: f32,
        tint: Colour,
    ) -> image::RgbaImage {
        let width = pixels(scene.width as f32).expect("a size");
        let height = pixels(scene.height as f32).expect("a size");
        let renderer = femtovg::renderer::WGPURenderer::new(device.clone(), queue.clone());
        let mut canvas = femtovg::Canvas::new(renderer).expect("femtovg on the card");
        canvas.set_size(width, height, scale.ceil());
        canvas.clear_rect(0, 0, width, height, femtovg::Color::rgbaf(0.0, 0.0, 0.0, 0.0));
        let mut path = femtovg::Path::new();
        let mut line = 0.0;
        for shape in &scene.shapes {
            if let Shape::Line { from, to, width, .. } = *shape {
                path.move_to(from.0, from.1);
                path.line_to(to.0, to.1);
                line = width;
            }
        }
        let mut paint = femtovg::Paint::color(femtovg::Color::rgba(tint.r, tint.g, tint.b, tint.a));
        paint.set_line_width(line);
        paint.set_line_cap(femtovg::LineCap::Butt);
        canvas.stroke_path(&path, &paint);
        let picture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("as the window draws"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        if let Some(commands) = canvas.flush_to_output(&picture) {
            queue.submit(Some(commands));
        }
        crate::canvas::testing::read_back(device, queue, &picture)
    }

    /// The card draws a waveform the way the window draws its path, so the picture and the path look the same: how
    /// much of the tint shows in each pixel (the picture's alpha) is what the window's drawing library puts down, for
    /// a tile and the sound's card at 80%, 100%, 125%, 150% and 200%, with loud lines, quiet ones and silence; and the
    /// colour is the tint everywhere, so the window shows the tint, see-through where the lines don't cover. Needs a
    /// graphics card (a software one will do), so it's run by hand. With WAVES_GPU_LOOK set to a folder, writes both
    /// and their difference there.
    #[test]
    #[ignore]
    fn the_card_draws_a_waveform_as_the_window_draws_its_path() {
        use crate::canvas::testing::{card, read_back};
        let Some((device, queue)) = card() else {
            eprintln!("No graphics card here, not even a software one: nothing to compare");
            return;
        };
        let out = std::env::var("WAVES_GPU_LOOK").ok();
        let mut canvas = Canvas::new(&device, &queue, Kind::OneColour);
        // A hit that rings and fades to silence, as 160 peaks: loud lines, quiet ones and a silent tail.
        let peaks: Vec<f32> = (0..160)
            .map(|i| {
                let t = i as f32;
                if i >= 120 { 0.0 } else { (t * 0.37).sin() * (-t / 25.0).exp() * (t / 4.0).min(1.0) }
            })
            .collect();
        let shape = mo_app::sounds::wave_path(&peaks);
        let lines = waves::lines(&shape).expect("its own path");
        let tint = Colour::rgb(0x4FB3FF, 1.0);
        // A tile and the sound's card at 100% (each line as wide as the Waveform makes it, in points), at each scale.
        let views = [("tile", (231.5f32, 40.25f32), 1.0f32), ("card", (552.0, 88.0), 2.07)];
        for scale in [0.8f32, 1.0, 1.25, 1.5, 2.0] {
            for (name, (width, height), line) in views {
                let name = format!("{name}-{}", (scale * 100.0).round());
                let (width, height, line) = (width * scale, height * scale, line * scale);
                let scene = waves::scene(&lines, 160, (f64::from(width), f64::from(height)), f64::from(line), tint);
                let size = (pixels(width).expect("a size"), pixels(height).expect("a size"));
                let picture = canvas.draw_new(size, 1.0, scene.background, &scene.pack(1.0), fringe(scale));
                let card_picture = read_back(&device, &queue, &picture);
                let window = as_the_window_draws(&device, &queue, &scene, scale, tint);
                assert_eq!(card_picture.dimensions(), window.dimensions(), "{name}");
                let mut diff = image::RgbaImage::new(window.width(), window.height());
                let (mut off, mut most, mut covered) = (0usize, 0u8, 0usize);
                let (mut card_ink, mut window_ink) = (0u64, 0u64);
                for (x, y, c) in card_picture.enumerate_pixels() {
                    assert_eq!([c[0], c[1], c[2]], [0x4F, 0xB3, 0xFF], "{name}: the colour is the tint at {x},{y}");
                    let w = window.get_pixel(x, y)[3];
                    let d = c[3].abs_diff(w);
                    most = most.max(d);
                    off += usize::from(d > 2);
                    covered += usize::from(w > 0);
                    card_ink += u64::from(c[3]);
                    window_ink += u64::from(w);
                    diff.put_pixel(
                        x,
                        y,
                        image::Rgba([d.saturating_mul(8), d.saturating_mul(8), d.saturating_mul(8), 255]),
                    );
                }
                let all = (window.width() * window.height()) as usize;
                let ink = card_ink as f64 / window_ink.max(1) as f64;
                eprintln!(
                    "{name}: {covered} of {all} pixels covered, {off} differ by more than 2 (of 255), at most {most}; \
                     the card puts down {:.2}% of the window's tint",
                    ink * 100.0
                );
                if let Some(out) = &out {
                    // Both as the window shows them, over black (femtovg's colours are premultiplied already).
                    let card_shown = image::RgbaImage::from_fn(card_picture.width(), card_picture.height(), |x, y| {
                        let q = card_picture.get_pixel(x, y);
                        let over = |v: u8| ((u16::from(v) * u16::from(q[3]) + 127) / 255) as u8;
                        image::Rgba([over(q[0]), over(q[1]), over(q[2]), 255])
                    });
                    let window_shown = image::RgbaImage::from_fn(window.width(), window.height(), |x, y| {
                        let q = window.get_pixel(x, y);
                        image::Rgba([q[0], q[1], q[2], 255])
                    });
                    card_shown.save(format!("{out}/waves-{name}-card.png")).expect("saved");
                    window_shown.save(format!("{out}/waves-{name}-window.png")).expect("saved");
                    diff.save(format!("{out}/waves-{name}-difference.png")).expect("saved");
                }
                assert!(covered * 20 >= all, "{name}: the lines cover {covered} of {all} pixels");
                assert!(most <= 3, "{name}: a pixel differs by {most}");
                assert!(
                    (ink - 1.0).abs() <= 0.005,
                    "{name}: the card puts down {:.2}% of the window's tint",
                    ink * 100.0
                );
            }
        }
    }
}
