//! The sounds' waveforms on the graphics card. Each `Waveform` on a page (studio.slint) asks `Waves.picture` for a
//! picture of its lines, at its size in pixels and at the part of a pixel it sits at: drawn once on the card and kept,
//! the window shows it like any picture, instead of working out every line of every waveform again for each frame. The
//! lines are drawn the way the window's drawing library draws the path, just where it would draw them, so the picture
//! looks as the path does. Where there's no card (the screenshots, an old driver, `MO_DRAW_CPU`), while the card's
//! painter is being set up, once the card refused, or once a frame has drawn all the new pictures it has time for,
//! there's no picture and the `Waveform` draws its path itself, as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use mo_app::log;
use mo_app::shapes::{Colour, MOST_PIXELS};
use mo_app::waves;
use slint::{ComponentHandle, Image, SharedString};

use crate::canvas::{Canvas, Kind};
use crate::{AppWindow, WavePicture, Waves, gpu};

/// How much room on the card the pictures kept may take: the tiles of two 4K screens (a tile's picture is a few tens of
/// kilobytes), so scrolling back finds them drawn already. A picture asked for in the frame being drawn is on show and
/// stays, even past this.
const KEEP_BYTES: u64 = 64 << 20;

/// How long one frame may spend drawing new pictures. When it needs more (the window being resized, a panel opened, a
/// jump down a long list: every waveform on show at a new size, or new), the rest draw their paths in that frame, which
/// look the same, and ask again for the next one, so the window doesn't wait on the card.
const FRAME_TIME: Duration = Duration::from_millis(4);

/// How finely the part of a pixel a Waveform sits at is told apart: in 64ths of a pixel, a hair next to the soft edges
/// of its lines (a pixel or half a pixel wide).
const PHASES: u8 = 64;

/// What a picture shows: a sound's waveform (its path and how many points across), its tint, and which card it's on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Shows {
    shape: SharedString,
    points: i32,
    tint: u32,
    card: u64,
}

/// A picture's width and height and its lines' width, in pixels, and the window's scale, to the bit: a picture is only
/// shown again at exactly its size.
type Size = [u32; 4];

/// How far past a whole pixel a Waveform sits in the window, across and down, in [`PHASES`]ths of a pixel. The window
/// puts a picture on a whole pixel (Slint's renderer rounds where it draws one) but draws a path just where it is, so
/// the card draws the lines that far over in the picture, and the Waveform puts the picture that far back: the lines
/// land just where the path's would. It shows: a quiet tail's lines are shorter than a pixel, and how much of them
/// shows depends on where they fall in one.
type Phase = [u8; 2];

/// A picture kept, at `size` and `phase`, taking `bytes` on the card: last asked for (at its size and phase) at ask
/// number `asked`, and last given to a Waveform to show (as itself, or in another's place) at ask number `shown`.
struct Picture {
    size: Size,
    phase: Phase,
    image: Image,
    bytes: u64,
    asked: u64,
    shown: u64,
}

/// A sound's waveform's pictures, one for each place it's on show (its tile and the sound's card, say), at its size and
/// phase there; and where it was asked for lately and shown a picture at another phase: the size and phase, with the
/// frame that was in (see [`Found::Near`]).
#[derive(Default)]
struct Pictures {
    kept: Vec<Picture>,
    near: Vec<(Size, Phase, u64)>,
}

/// What's kept for a waveform at a size and phase.
enum Found {
    /// Its picture.
    Itself(Image),
    /// Its picture at the same size but another phase (`phase`), to show, a hair over, until it's drawn at its own:
    /// once it's `still` (asked for at this phase in the last frame waveforms asked in, too), now if there's time; not
    /// while it moves, so scrolling doesn't draw a picture of every waveform on show in every frame.
    Near { image: Image, phase: Phase, still: bool },
    /// Nothing at its size.
    Nothing,
}

/// The pictures drawn, for each sound's waveform, with when each was last asked for and shown. A picture drawn takes the
/// place of the waveform's pictures as high as it (in the same place: left at another width or phase, as the window is
/// resized or the page scrolls) that haven't been shown in this frame; pictures at other heights (the sound's tile, and
/// its card) are kept apart. Past the room they may take, the least lately shown go first, but never one shown in this
/// frame or the one before (Slint may ask for a picture between two frames, for the second). All go when the window's
/// scale changes (moved to another screen, or the app's size changed): they'd never be shown again at the new one.
#[derive(Default)]
struct Kept {
    sounds: HashMap<Shows, Pictures>,
    /// How many bytes the pictures take on the card, in all.
    bytes: u64,
    /// The asks, counted, so it's known which picture was asked for, or shown, least lately.
    asks: u64,
    /// The frames in which waveforms asked, counted, so it's known when a waveform has stopped moving.
    frames: u64,
    /// The count of asks when this frame started: a picture asked for since then is on show.
    frame_started: u64,
    /// The count of asks when the frame before started.
    frame_before_started: u64,
    /// The window's scale the pictures are drawn at (its bits).
    scale: u32,
}

impl Kept {
    /// What's kept for `shows` at `size` and `phase`.
    fn get(&mut self, shows: &Shows, size: Size, phase: Phase) -> Found {
        self.asks += 1;
        let (asks, frame) = (self.asks, self.frames);
        let Some(pictures) = self.sounds.get_mut(shows) else { return Found::Nothing };
        if let Some(kept) = pictures.kept.iter_mut().find(|p| p.size == size && p.phase == phase) {
            kept.asked = asks;
            kept.shown = asks;
            return Found::Itself(kept.image.clone());
        }
        let Some(near) = pictures.kept.iter().filter(|p| p.size == size).max_by_key(|p| p.asked) else {
            return Found::Nothing;
        };
        let (image, near_phase) = (near.image.clone(), near.phase);
        // Asked for at this phase in the last frame waveforms asked in as well (asks before that go), it's still.
        pictures.near.retain(|(_, _, at)| at + 1 >= frame);
        let still = match pictures.near.iter_mut().find(|(s, p, _)| *s == size && *p == phase) {
            Some(asked) => {
                let still = asked.2 < frame;
                asked.2 = frame;
                still
            }
            None => {
                pictures.near.push((size, phase, frame));
                false
            }
        };
        Found::Near { image, phase: near_phase, still }
    }

    /// The picture of `shows` at `size` and `phase` (as [`Kept::get`] found it near) is shown in another phase's
    /// place: on show in this frame, so it isn't let go to make room, and not taken over by a picture drawn for
    /// another place as high.
    fn shown_in_place(&mut self, shows: &Shows, size: Size, phase: Phase) {
        self.asks += 1;
        let asks = self.asks;
        if let Some(near) =
            self.sounds.get_mut(shows).and_then(|p| p.kept.iter_mut().find(|p| p.size == size && p.phase == phase))
        {
            near.shown = asks;
        }
    }

    /// Keeps the picture of `shows` at `size` and `phase`, `bytes` on the card, making room for it within `most` bytes.
    fn keep(&mut self, shows: Shows, size: Size, phase: Phase, image: Image, bytes: u64, most: u64) {
        let frame_started = self.frame_started;
        if let Some(pictures) = self.sounds.get_mut(&shows) {
            let mut freed = 0;
            pictures.kept.retain(|p| {
                let replaced = p.size == size && p.phase == phase || p.size[1] == size[1] && p.shown <= frame_started;
                if replaced {
                    freed += p.bytes;
                }
                !replaced
            });
            self.bytes = self.bytes.saturating_sub(freed);
        }
        while self.bytes + bytes > most && self.forget_least_lately_shown() {}
        // Counted as asked for and shown now, after everything kept before it.
        self.asks += 1;
        self.bytes += bytes;
        let picture = Picture { size, phase, image, bytes, asked: self.asks, shown: self.asks };
        self.sounds.entry(shows).or_default().kept.push(picture);
    }

    /// Lets go of the picture shown least lately, unless that was in this frame or the one before: whether one went.
    fn forget_least_lately_shown(&mut self) -> bool {
        let oldest = self
            .sounds
            .iter()
            .flat_map(|(shows, pictures)| pictures.kept.iter().enumerate().map(move |(at, p)| (shows, at, p.shown)))
            .filter(|(_, _, shown)| *shown <= self.frame_before_started)
            .min_by_key(|(_, _, shown)| *shown)
            .map(|(shows, at, _)| (shows.clone(), at));
        let Some((shows, at)) = oldest else { return false };
        if let Some(pictures) = self.sounds.get_mut(&shows) {
            let gone = pictures.kept.swap_remove(at);
            self.bytes = self.bytes.saturating_sub(gone.bytes);
            if pictures.kept.is_empty() {
                self.sounds.remove(&shows);
            }
        }
        true
    }

    /// A frame starts: what's asked for from now on is on show in it. Only frames in which waveforms asked are counted,
    /// so a frame drawn for something else (a sound playing) between a waveform's asks doesn't look like it moving.
    fn frame_starts(&mut self) {
        if self.asks > self.frame_started {
            self.frames += 1;
        }
        self.frame_before_started = self.frame_started;
        self.frame_started = self.asks;
    }

    /// The window's scale is `scale`: if it changed, the pictures drawn at the old one go.
    fn scale_is(&mut self, scale: f32) {
        if self.scale != scale.to_bits() {
            self.clear();
            self.scale = scale.to_bits();
        }
    }

    fn clear(&mut self) {
        self.sounds.clear();
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
pub(crate) fn fringe(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 { 1.0 / scale.ceil() } else { 1.0 }
}

/// Answers the `Waveform`s' asks for pictures.
pub fn connect(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<Waves>().on_picture(move |shape, points, tint, width, height, line, over_x, over_y, _asked| {
        let phase = [phase(over_x), phase(over_y)];
        picture(&weak, shape, points, tint, (width, height), line, phase).unwrap_or_default()
    });
}

/// How far past a whole pixel a Waveform sits (from 0 to 1, as it says it), to the nearest [`PHASES`]th of a pixel: a
/// whole pixel further is the next whole pixel, and anything that isn't a part of a pixel is a whole pixel. Nearly a
/// whole pixel over is taken as the next one with nothing over, which holds because Slint's renderer puts a picture on
/// the nearest whole pixel (i-slint-renderer-femtovg's `pixel_align_origin` rounds), so that one is where it goes.
fn phase(over: f32) -> u8 {
    let over = if over.is_finite() { over.clamp(0.0, 1.0) } else { 0.0 };
    u8::try_from((over * f32::from(PHASES)).round() as u32 % u32::from(PHASES)).unwrap_or(0)
}

/// How far past a whole pixel `phase` is, in pixels.
fn part_of_a_pixel(phase: u8) -> f64 {
    f64::from(phase) / f64::from(PHASES)
}

/// What a Waveform shows of a picture drawn at `phase`: the picture, and how far back it goes, in pixels.
fn shown(picture: Image, phase: Phase) -> WavePicture {
    WavePicture { picture, x: part_of_a_pixel(phase[0]) as f32, y: part_of_a_pixel(phase[1]) as f32 }
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

/// A waveform is left to the next frame: the first one left in a frame has every waveform ask again then.
fn leave_to_next_frame(app: &slint::Weak<AppWindow>) {
    if THIS_FRAME.with(|f| f.borrow_mut().leave()) {
        ask_again_later(app.clone());
    }
}

/// The picture of a waveform's lines (`shape`, as `mo_app::sounds::wave_path` makes it, `points` across) in `tint`, for
/// a Waveform `width` by `height` pixels with lines `line` pixels wide, `phase` past a whole pixel: kept from before, or
/// drawn now; or, while the Waveform moves, its picture at the phase it was at. `None` where the window draws the path
/// itself instead.
fn picture(
    app: &slint::Weak<AppWindow>,
    shape: SharedString,
    points: i32,
    tint: slint::Color,
    (width, height): (f32, f32),
    line: f32,
    phase: Phase,
) -> Option<WavePicture> {
    let (gpu, generation) = CARD.with(|c| c.borrow().card())?;
    // A Waveform less than a pixel wide or high is left to the window.
    if pixels(width).is_none() || pixels(height).is_none() {
        return None;
    }
    let app_window = app.upgrade()?;
    // The soft edges as the window would draw them, at its scale (pictures at another scale go).
    let scale = app_window.window().scale_factor();
    let fringe = fringe(scale);
    KEPT.with(|k| k.borrow_mut().scale_is(scale));
    let shows = Shows { shape, points, tint: tint.as_argb_encoded(), card: generation };
    let size = [width.to_bits(), height.to_bits(), line.to_bits(), scale.to_bits()];
    // A picture at another phase, shown in this one's place.
    let in_place = |(image, at): (Image, Phase)| {
        KEPT.with(|k| k.borrow_mut().shown_in_place(&shows, size, at));
        Some(shown(image, at))
    };
    let near = match KEPT.with(|k| k.borrow_mut().get(&shows, size, phase)) {
        Found::Itself(image) => return Some(shown(image, phase)),
        // Moving (or only just stopped): its picture at the phase it was at, a hair over, and every waveform asks again
        // in the next frame, which draws its own picture if it has stopped.
        Found::Near { image, phase: at, still: false } => {
            leave_to_next_frame(app);
            return in_place((image, at));
        }
        Found::Near { image, phase: at, still: true } => Some((image, at)),
        Found::Nothing => None,
    };
    // The painter is set up away from the window's thread: as the card is handed over, or now, if the window draws a
    // waveform before that (its first frame comes before the card is handed over to the views). The window draws the
    // path until it's ready.
    if !CARD.with(|c| c.borrow().made_on(generation)) {
        gpu::prepare(&app_window, "Waveforms", &CARD, painter, ask_again);
        if CARD.with(|c| c.borrow().card()).is_none() {
            return near.and_then(in_place);
        }
    }
    // Enough drawn in this frame: the window shows this one's picture at another phase, or draws its path, and every
    // waveform asks again for the next.
    if !THIS_FRAME.with(|f| f.borrow().has_time()) {
        leave_to_next_frame(app);
        return near.and_then(in_place);
    }
    let started = Instant::now();
    let lines = waves::lines(&shows.shape)?;
    let points = u32::try_from(points).ok().filter(|p| *p > 0)?;
    let colour = Colour { r: tint.red(), g: tint.green(), b: tint.blue(), a: tint.alpha() };
    let waveform = (f64::from(width), f64::from(height));
    let at = (part_of_a_pixel(phase[0]), part_of_a_pixel(phase[1]));
    let scene = waves::scene(&lines, points, waveform, at, f64::from(line), colour);
    let pixel_size = scene.pixel_size(1.0);
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
            KEPT.with(|k| k.borrow_mut().keep(shows, size, phase, image.clone(), bytes, KEEP_BYTES));
            Some(shown(image, phase))
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

    /// On a whole pixel.
    const WHOLE: Phase = [0, 0];

    /// Whether a waveform finds its own picture, on a whole pixel.
    fn found(kept: &mut Kept, shows: &Shows, size: Size) -> bool {
        matches!(kept.get(shows, size, WHOLE), Found::Itself(_))
    }

    impl Kept {
        fn len(&self) -> usize {
            self.sounds.values().map(|p| p.kept.len()).sum()
        }
    }

    /// Once the pictures fill the room they may take, the least lately shown go first, as many as the new one needs the
    /// room of.
    #[test]
    fn the_pictures_kept_are_the_ones_shown_lately() {
        let mut kept = Kept::default();
        for n in 0..10 {
            kept.keep(shows(n), at(200.0), WHOLE, Image::default(), 100, 1000);
        }
        kept.frame_starts();
        // The first is asked for again, so the second is the least lately shown.
        assert!(found(&mut kept, &shows(0), at(200.0)));
        kept.frame_starts();
        kept.keep(shows(99), at(200.0), WHOLE, Image::default(), 100, 1000);
        assert_eq!((kept.len(), kept.bytes), (10, 1000));
        assert!(found(&mut kept, &shows(0), at(200.0)), "shown lately");
        assert!(!found(&mut kept, &shows(1), at(200.0)), "the least lately shown went");
        assert!(found(&mut kept, &shows(99), at(200.0)));
        // A big one takes the room of several.
        kept.frame_starts();
        kept.keep(shows(100), at(200.0), WHOLE, Image::default(), 350, 1000);
        assert_eq!((kept.len(), kept.bytes), (7, 950));
        // On another card, it's another picture.
        assert!(!found(&mut kept, &Shows { card: 2, ..shows(0) }, at(200.0)));
    }

    /// Pictures shown in the frame being drawn (or the one before) are on show, so none of them goes to make room, even
    /// with more on show than the room holds (a screen bigger than two 4K ones): a page of tiles all asking again, in
    /// order, finds every one, instead of each pushing out the next.
    #[test]
    fn a_picture_on_show_stays() {
        let mut kept = Kept::default();
        kept.frame_starts();
        for n in 0..20 {
            kept.keep(shows(n), at(200.0), WHOLE, Image::default(), 100, 1000);
        }
        assert_eq!(kept.len(), 20, "all on show");
        kept.frame_starts();
        for n in 0..20 {
            assert!(found(&mut kept, &shows(n), at(200.0)), "{n} found again");
        }
        // Once they haven't been on show for a frame, the room counts again.
        kept.frame_starts();
        kept.frame_starts();
        kept.keep(shows(99), at(200.0), WHOLE, Image::default(), 100, 1000);
        assert_eq!((kept.len(), kept.bytes), (10, 1000));
        assert!(found(&mut kept, &shows(99), at(200.0)));
    }

    /// A waveform drawn at a new size (the window resized) takes the place of its picture at the old size, instead of
    /// filling the room with sizes nothing shows any more.
    #[test]
    fn a_new_size_takes_the_old_ones_place() {
        let mut kept = Kept::default();
        for step in 0..100 {
            let width = 200.0 + step as f32 * 0.25;
            kept.frame_starts();
            assert!(matches!(kept.get(&shows(0), at(width), WHOLE), Found::Nothing), "not drawn at {width} yet");
            kept.keep(shows(0), at(width), WHOLE, Image::default(), 100, 1000);
        }
        assert_eq!((kept.len(), kept.bytes), (1, 100), "one picture, at the size it's at now");
        let last = at(200.0 + 99.0 * 0.25);
        assert!(found(&mut kept, &shows(0), last));
        assert!(!found(&mut kept, &shows(0), at(200.0)), "a hair off is another picture");
        let softer = [last[0], last[1], last[2], 0.5f32.to_bits()];
        assert!(!found(&mut kept, &shows(0), softer), "so are softer edges (the window's scale changed)");
        // A full room doesn't lose a picture to a waveform drawn again at a new size.
        for n in 1..10 {
            kept.keep(shows(n), at(200.0), WHOLE, Image::default(), 100, 1000);
        }
        kept.frame_starts();
        kept.keep(shows(0), at(321.0), WHOLE, Image::default(), 100, 1000);
        assert_eq!((kept.len(), kept.bytes), (10, 1000));
        assert!(found(&mut kept, &shows(1), at(200.0)), "still kept");
        assert!(found(&mut kept, &shows(0), at(321.0)));
    }

    /// A sound's tile and its card (as high as each other only by chance) keep a picture each, so a page asking for
    /// both finds both, instead of drawing one in the other's place each time.
    #[test]
    fn a_sounds_tile_and_its_card_keep_a_picture_each() {
        let mut kept = Kept::default();
        let (tile, card) = (at(200.0), [552f32.to_bits(), 88f32.to_bits(), 2f32.to_bits(), 1f32.to_bits()]);
        kept.keep(shows(0), tile, WHOLE, Image::default(), 100, 1000);
        kept.keep(shows(0), card, WHOLE, Image::default(), 100, 1000);
        for _ in 0..3 {
            kept.frame_starts();
            assert!(found(&mut kept, &shows(0), tile));
            assert!(found(&mut kept, &shows(0), card));
        }
        // The tile at a new width (the card opened beside the tiles, say) takes only the tile's place.
        kept.frame_starts();
        kept.keep(shows(0), at(150.0), WHOLE, Image::default(), 100, 1000);
        assert_eq!((kept.len(), kept.bytes), (2, 200));
        assert!(found(&mut kept, &shows(0), card));
        assert!(!found(&mut kept, &shows(0), tile));
    }

    /// While a waveform moves (the page scrolling), a part of a pixel further each frame, it's shown its picture at the
    /// part of a pixel it was at, a hair over, instead of a new picture being drawn for each frame; once it's at the
    /// same part of a pixel in two frames running, it has stopped, its own picture is drawn, and that one takes the
    /// other's place.
    #[test]
    fn a_moving_waveform_is_shown_its_picture_a_hair_over_until_it_stops() {
        let mut kept = Kept::default();
        let (before, moving, stopped) = ([0, 32], [0, 40], [0, 48]);
        kept.keep(shows(0), at(200.0), before, Image::default(), 100, 1000);
        for phase in [moving, stopped] {
            kept.frame_starts();
            let found = kept.get(&shows(0), at(200.0), phase);
            assert!(matches!(found, Found::Near { phase: p, still: false, .. } if p == before), "moving, at {phase:?}");
        }
        // Asked for again in the same frame, it hasn't been still for a frame yet.
        assert!(matches!(kept.get(&shows(0), at(200.0), stopped), Found::Near { still: false, .. }));
        kept.frame_starts();
        let found = kept.get(&shows(0), at(200.0), stopped);
        assert!(matches!(found, Found::Near { phase: p, still: true, .. } if p == before), "still");
        kept.keep(shows(0), at(200.0), stopped, Image::default(), 100, 1000);
        assert_eq!((kept.len(), kept.bytes), (1, 100), "its own picture took the other's place");
        assert!(matches!(kept.get(&shows(0), at(200.0), stopped), Found::Itself(_)));
        // At another size, there's no picture to show a hair over: it's drawn, or the window draws its path.
        assert!(matches!(kept.get(&shows(0), at(201.0), stopped), Found::Nothing));
    }

    /// The same waveform in two places as high as each other (two sounds with the same peaks, side by side), at two
    /// parts of a pixel, keeps a picture for each, instead of each being drawn in the other's place in turn.
    #[test]
    fn the_same_waveform_in_two_places_keeps_a_picture_for_each() {
        let mut kept = Kept::default();
        let (one, two) = ([0, 0], [16, 0]);
        kept.frame_starts();
        kept.keep(shows(0), at(200.0), one, Image::default(), 100, 1000);
        assert!(matches!(kept.get(&shows(0), at(200.0), two), Found::Near { still: false, .. }));
        kept.shown_in_place(&shows(0), at(200.0), one);
        kept.frame_starts();
        assert!(matches!(kept.get(&shows(0), at(200.0), one), Found::Itself(_)));
        assert!(matches!(kept.get(&shows(0), at(200.0), two), Found::Near { still: true, .. }));
        kept.keep(shows(0), at(200.0), two, Image::default(), 100, 1000);
        assert_eq!(kept.len(), 2, "both on show");
        for _ in 0..3 {
            kept.frame_starts();
            assert!(matches!(kept.get(&shows(0), at(200.0), one), Found::Itself(_)));
            assert!(matches!(kept.get(&shows(0), at(200.0), two), Found::Itself(_)));
        }
    }

    /// A Waveform says how far past a whole pixel it sits, which is taken to the nearest 64th of a pixel; a picture
    /// shown goes back as far as it was drawn over.
    #[test]
    fn a_picture_goes_back_as_far_as_it_was_drawn_over() {
        assert_eq!([phase(0.0), phase(0.25), phase(0.5), phase(63.0 / 64.0), phase(0.51)], [0, 16, 32, 63, 33]);
        assert_eq!(phase(0.999), 0, "nearer the next whole pixel");
        assert_eq!([phase(-1.0), phase(2.0), phase(f32::NAN), phase(f32::INFINITY)], [0, 0, 0, 0], "not a part");
        let shown = shown(Image::default(), [16, 48]);
        assert_eq!((shown.x, shown.y), (0.25, 0.75));
    }

    /// A picture shown in another waveform's place (while that one moves) is on show in this frame, so it isn't let go
    /// to make room, however long ago it was asked for itself; the waveform's own picture still takes its place once
    /// it's drawn.
    #[test]
    fn a_picture_shown_in_anothers_place_stays_while_on_show() {
        let mut kept = Kept::default();
        kept.keep(shows(0), at(200.0), WHOLE, Image::default(), 100, 1000);
        for n in 1..10 {
            kept.frame_starts();
            kept.keep(shows(n), at(200.0), WHOLE, Image::default(), 100, 1000);
        }
        // The first was asked for least lately, but it's on show in another's place in this frame.
        kept.frame_starts();
        assert!(matches!(kept.get(&shows(0), at(200.0), [0, 8]), Found::Near { .. }));
        kept.shown_in_place(&shows(0), at(200.0), WHOLE);
        kept.keep(shows(99), at(200.0), WHOLE, Image::default(), 100, 1000);
        assert_eq!((kept.len(), kept.bytes), (10, 1000));
        assert!(!found(&mut kept, &shows(1), at(200.0)), "the least lately shown went instead");
        assert!(matches!(kept.get(&shows(0), at(200.0), [0, 8]), Found::Near { .. }), "still kept");
        // Drawn at its own phase, its own picture takes that one's place.
        kept.frame_starts();
        kept.keep(shows(0), at(200.0), [0, 8], Image::default(), 100, 1000);
        assert_eq!(kept.sounds[&shows(0)].kept.len(), 1);
    }

    /// Moved to a screen at another scale, or the app's size changed, every waveform's pictures would never be shown
    /// again at the new scale (even where its soft edges are as wide, as from 125% to 150%), so they all go.
    #[test]
    fn pictures_at_another_scale_go() {
        let mut kept = Kept::default();
        kept.scale_is(1.25);
        let card = [552f32.to_bits(), 88f32.to_bits(), 2f32.to_bits(), 1.25f32.to_bits()];
        kept.keep(shows(0), at(200.0), WHOLE, Image::default(), 100, 1000);
        kept.keep(shows(0), card, WHOLE, Image::default(), 100, 1000);
        kept.keep(shows(1), at(200.0), WHOLE, Image::default(), 100, 1000);
        kept.scale_is(1.25);
        assert_eq!((kept.len(), kept.bytes), (3, 300), "the same scale");
        kept.scale_is(1.5);
        assert_eq!((kept.len(), kept.bytes), (0, 0));
    }

    /// A frame drawn for something else (a sound playing, the mouse moving), where no waveform asks, comes between a
    /// waveform's asks without making it look as if it moved: at the same phase in the next frame it asks in, it's
    /// still.
    #[test]
    fn a_frame_drawn_for_something_else_doesnt_look_like_moving() {
        let mut kept = Kept::default();
        kept.keep(shows(0), at(200.0), WHOLE, Image::default(), 100, 1000);
        kept.frame_starts();
        assert!(matches!(kept.get(&shows(0), at(200.0), [0, 8]), Found::Near { still: false, .. }));
        kept.frame_starts();
        kept.frame_starts();
        kept.frame_starts();
        assert!(matches!(kept.get(&shows(0), at(200.0), [0, 8]), Found::Near { still: true, .. }));
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
        let (width, height) = scene.pixel_size(1.0);
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
    /// a tile and the sound's card at 80%, 100%, 125%, 150% and 200%, on a whole pixel and parts of a pixel over, with
    /// loud lines, quiet ones and silence; and the colour is the tint everywhere, so the window shows the tint,
    /// see-through where the lines don't cover. Needs a graphics card (a software one will do), so it's run by hand.
    /// With WAVES_GPU_LOOK set to a folder, writes both and their difference there.
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
        // A tile and the sound's card at 100% (each line as wide as the Waveform makes it, in points), at each scale,
        // on a whole pixel and at parts of a pixel over.
        let views = [("tile", (231.5f32, 40.25f32), 1.0f32), ("card", (552.0, 88.0), 2.07)];
        let phases = [WHOLE, [PHASES / 2, PHASES / 2], [19, 45]];
        for scale in [0.8f32, 1.0, 1.25, 1.5, 2.0] {
            for ((name, (width, height), line), phase) in views.into_iter().flat_map(|v| phases.map(|p| (v, p))) {
                let name = format!("{name}-{}-at-{}-{}", (scale * 100.0).round(), phase[0], phase[1]);
                let (width, height, line) = (width * scale, height * scale, line * scale);
                let at = (part_of_a_pixel(phase[0]), part_of_a_pixel(phase[1]));
                let waveform = (f64::from(width), f64::from(height));
                let scene = waves::scene(&lines, 160, waveform, at, f64::from(line), tint);
                let size = scene.pixel_size(1.0);
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
