//! The sounds' waveforms on the graphics card. Each `Waveform` on a page (studio.slint) asks `Waves.picture` for a
//! picture of its lines, at its size in pixels: drawn once on the card and kept, the window shows it like any picture,
//! instead of working out every line of every waveform again for each frame. Where there's no card (the screenshots,
//! an old driver, `MO_DRAW_CPU`), while the card's painter is being set up, or once the card refused, there's no
//! picture and the `Waveform` draws its path itself, as before.

use std::cell::RefCell;
use std::collections::HashMap;

use mo_app::log;
use mo_app::shapes::{Colour, MOST_PIXELS};
use mo_app::waves;
use slint::{ComponentHandle, Image, SharedString};

use crate::canvas::{Canvas, Kind};
use crate::{AppWindow, Waves, gpu};

/// How many pictures are kept: the tiles of a big screen several times over (each a few tens of kilobytes on the
/// card), so scrolling back doesn't draw them again.
const KEEP: usize = 256;

/// What a picture shows: a waveform's lines (its path and how many points across), its tint, its width, height and
/// lines' width in pixels (to the bit, so a picture is only used again at exactly its size), and which card it's on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Shows {
    shape: SharedString,
    points: i32,
    tint: u32,
    size: [u32; 3],
    card: u64,
}

/// The pictures drawn, by what they show, with when each was last asked for; the least lately asked for goes first.
#[derive(Default)]
struct Kept {
    pictures: HashMap<Shows, (Image, u64)>,
    asks: u64,
}

impl Kept {
    fn get(&mut self, shows: &Shows) -> Option<Image> {
        self.asks += 1;
        let asks = self.asks;
        self.pictures.get_mut(shows).map(|(image, asked)| {
            *asked = asks;
            image.clone()
        })
    }

    fn keep(&mut self, shows: Shows, image: Image) {
        if self.pictures.len() >= KEEP
            && let Some(oldest) = self.pictures.iter().min_by_key(|(_, (_, asked))| *asked).map(|(k, _)| k.clone())
        {
            self.pictures.remove(&oldest);
        }
        // Counted as asked for now, after everything kept before it.
        self.asks += 1;
        self.pictures.insert(shows, (image, self.asks));
    }
}

thread_local! {
    /// The painter on the graphics card.
    static CARD: RefCell<gpu::Painter<Canvas>> = const { RefCell::new(gpu::Painter::new()) };
    static KEPT: RefCell<Kept> = RefCell::default();
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
    KEPT.with(|k| k.borrow_mut().pictures.clear());
}

/// The card went and the window's still here: every `Waveform` asks again, drawing its path until there's a card again.
pub fn graphics_gone(app: &AppWindow) {
    let weak = app.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            ask_again(&app);
        }
    });
}

/// Every `Waveform` asks for its picture again.
fn ask_again(app: &AppWindow) {
    let waves = app.global::<Waves>();
    waves.set_asked(waves.get_asked().wrapping_add(1));
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
    let size = (pixels(width)?, pixels(height)?);
    let shows = Shows {
        shape,
        points,
        tint: tint.as_argb_encoded(),
        size: [width.to_bits(), height.to_bits(), line.to_bits()],
        card: generation,
    };
    if let Some(image) = KEPT.with(|k| k.borrow_mut().get(&shows)) {
        return Some(image);
    }
    // The painter is set up away from the window's thread: as the card is handed over, or now, if the window draws a
    // waveform before that (its first frame comes before the card is handed over to the views). The window draws the
    // path until it's ready.
    if !CARD.with(|c| c.borrow().made_on(generation)) {
        gpu::prepare(&app.upgrade()?, "Waveforms", &CARD, painter, ask_again);
        CARD.with(|c| c.borrow().card())?;
    }
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
        let texture = gpu::checked(&gpu.device, || canvas.draw_new(size, 1.0, scene.background, &scene.pack(1.0)))?;
        Image::try_from(texture).map_err(|e| format!("Slint wouldn't show the picture: {e}"))
    });
    match drawn {
        Ok(image) => {
            KEPT.with(|k| k.borrow_mut().keep(shows, image.clone()));
            Some(image)
        }
        Err(e) => {
            log::error(
                "Waveforms",
                &format!("The graphics card couldn't draw a waveform, so the window draws them: {e}"),
            );
            CARD.with(|c| c.borrow_mut().refused(generation));
            KEPT.with(|k| k.borrow_mut().pictures.clear());
            // Every waveform asks again and draws its path; not now, while the window is working out what to show.
            let app = app.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app.upgrade() {
                    ask_again(&app);
                }
            });
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

    /// The least lately asked for goes first once [`KEEP`] are kept.
    #[test]
    fn the_pictures_kept_are_the_ones_asked_for_lately() {
        let shows = |points: usize| Shows {
            shape: "M0.5 10.0V90.0".into(),
            points: points as i32,
            tint: 0xFF4FB3FF,
            size: [200f32.to_bits(), 40f32.to_bits(), 1f32.to_bits()],
            card: 1,
        };
        let mut kept = Kept::default();
        for n in 0..KEEP {
            kept.keep(shows(n), Image::default());
        }
        // The first is asked for again, so the second is the least lately asked for.
        assert!(kept.get(&shows(0)).is_some());
        kept.keep(shows(9999), Image::default());
        assert_eq!(kept.pictures.len(), KEEP);
        assert!(kept.get(&shows(0)).is_some(), "asked for lately");
        assert!(kept.get(&shows(1)).is_none(), "the least lately asked for went");
        assert!(kept.get(&shows(9999)).is_some());
        // Anything else different is another picture: on another card, or a hair wider.
        assert!(kept.get(&Shows { card: 2, ..shows(0) }).is_none());
        assert!(
            kept.get(&Shows { size: [200.01f32.to_bits(), 40f32.to_bits(), 1f32.to_bits()], ..shows(0) }).is_none()
        );
    }

    /// The card draws a waveform's lines where the processor would: how much of the tint shows in each pixel (the
    /// picture's alpha) is about how much of the pixel the processor covers, and the colour is the tint everywhere,
    /// so the window shows it as the tint, see-through where the lines don't cover. The processor works out the soft
    /// edges of lines wider than a pixel in quarters of a pixel, so those differ a little in nearly every pixel they
    /// touch; what's checked is that no pixel is far out (a line in the wrong place) and that both put down the same
    /// amount of the tint. Needs a graphics card (a software one will do), so it's run by hand. With WAVES_GPU_LOOK
    /// set to a folder, writes both and their difference there.
    #[test]
    #[ignore]
    fn the_card_draws_a_waveform_where_the_processor_would() {
        use crate::canvas::testing::{card, read_back};
        let Some((device, queue)) = card() else {
            eprintln!("No graphics card here, not even a software one: nothing to compare");
            return;
        };
        let out = std::env::var("WAVES_GPU_LOOK").ok();
        let mut canvas = Canvas::new(&device, &queue, Kind::OneColour);
        // A sound that swells and fades, as 160 peaks.
        let peaks: Vec<f32> =
            (0..160).map(|i| (i as f32 * 0.37).sin() * (1.0 - i as f32 / 200.0) * (i as f32 / 20.0).min(1.0)).collect();
        let shape = mo_app::sounds::wave_path(&peaks);
        let lines = waves::lines(&shape).expect("its own path");
        let tint = Colour::rgb(0x4FB3FF, 1.0);
        let white = Colour::rgb(0xFFFFFF, 1.0);
        // A tile at 100% and 150%, and the side card at 100% and 200% (each line as wide as the Waveform makes it).
        let views = [
            ("tile", (231.5, 40.25), 1.0),
            ("tile-1.5x", (347.25, 60.375), 1.5),
            ("card", (552.0, 88.0), 2.07),
            ("card-2x", (1104.0, 176.0), 4.14),
        ];
        for (name, (width, height), line) in views {
            let scene = waves::scene(&lines, 160, (width, height), line, tint);
            let size = (pixels(width as f32).expect("a size"), pixels(height as f32).expect("a size"));
            let gpu = read_back(&device, &queue, &canvas.draw_new(size, 1.0, scene.background, &scene.pack(1.0)));
            // The processor: the same lines in white over black, so a pixel's red is how much the lines cover it.
            let mut coverage = waves::scene(&lines, 160, (width, height), line, white);
            coverage.background = 0;
            let cpu = coverage.paint(1.0);
            assert_eq!((gpu.width(), gpu.height()), (cpu.width(), cpu.height()), "{name}");
            let mut diff = image::RgbaImage::new(gpu.width(), gpu.height());
            let (mut off, mut far, mut most, mut covered) = (0usize, 0usize, 0u8, 0usize);
            // How much of the tint each puts down, in all.
            let (mut card_ink, mut processor_ink) = (0u64, 0u64);
            for (x, y, g) in gpu.enumerate_pixels() {
                assert_eq!([g[0], g[1], g[2]], [0x4F, 0xB3, 0xFF], "{name}: the colour is the tint at {x},{y}");
                let c = cpu.pixel(x, y).expect("inside").red();
                let d = g[3].abs_diff(c);
                most = most.max(d);
                off += usize::from(d > 8);
                far += usize::from(d > 64);
                covered += usize::from(c > 0);
                card_ink += u64::from(g[3]);
                processor_ink += u64::from(c);
                diff.put_pixel(x, y, image::Rgba([d.saturating_mul(4), d.saturating_mul(4), d.saturating_mul(4), 255]));
            }
            let pixels = (gpu.width() * gpu.height()) as usize;
            let ink = card_ink as f64 / processor_ink.max(1) as f64;
            eprintln!(
                "{name}: {covered} of {pixels} pixels covered, {off} differ by more than 8, {far} by more than 64, at \
                 most {most}; the card puts down {:.2}% of the processor's tint",
                ink * 100.0
            );
            if let Some(out) = &out {
                let shown = |p: &image::RgbaImage| {
                    // As the window shows it, over black.
                    image::RgbaImage::from_fn(p.width(), p.height(), |x, y| {
                        let q = p.get_pixel(x, y);
                        let a = u16::from(q[3]);
                        let over = |v: u8| ((u16::from(v) * a + 127) / 255) as u8;
                        image::Rgba([over(q[0]), over(q[1]), over(q[2]), 255])
                    })
                };
                shown(&gpu).save(format!("{out}/waves-{name}-card.png")).expect("saved");
                let cpu_png =
                    image::RgbaImage::from_raw(cpu.width(), cpu.height(), cpu.data().to_vec()).expect("same size");
                cpu_png.save(format!("{out}/waves-{name}-processor.png")).expect("saved");
                diff.save(format!("{out}/waves-{name}-difference.png")).expect("saved");
            }
            assert!(covered * 10 >= pixels, "{name}: the lines cover {covered} of {pixels} pixels");
            assert_eq!(far, 0, "{name}: {far} pixels differ by more than 64");
            assert!(
                (ink - 1.0).abs() <= 0.02,
                "{name}: the card puts down {:.2}% of the processor's tint",
                ink * 100.0
            );
        }
    }
}
