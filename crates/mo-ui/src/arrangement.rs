//! Fills the arrangement on a song's Overview. Picking a song asks a background thread to read its newest save
//! (newest ask wins, rule 12); the mouse then moves the view here on the window's thread, and each change asks a
//! second background thread to work out what to draw. The graphics card draws it (`canvas`), or where there's no card
//! the processor does, on that thread (`mo_app::shapes`); the picture and its words are put on the page, unless
//! another song has been picked meanwhile.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mo_app::arrangement::{self as arr, Area, Frame, Hover, Palette, Peaks, Preview, Style, View};
use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::themes::Look;
use rayon::prelude::*;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::canvas::Canvas;
use crate::gpu;
use crate::{AppWindow, ArrKind, ArrLabel, ArrangementData};

/// What the page says when there's no save to read.
const NOT_HERE: &str = "This save isn't on this computer, so its arrangement can't be drawn.";
const READING: &str = "Reading the arrangement…";
const NOT_YET: &str = "Arrangements can be drawn for FL Studio, Bitwig and Studio One projects.";

/// A picture to work out: for which song, the view as it is, its colours, pixels to a point, and whether the graphics
/// card draws it (else the processor does, on the drawing thread).
type DrawJob = (u64, View, Palette, f64, bool);

#[derive(Default)]
struct State {
    /// Counts the songs shown, so a read or a picture that arrives after another song was picked is dropped.
    asked: u64,
    preview: Option<Preview>,
    /// The audio clips' waveforms, once their files have been read.
    peaks: Arc<HashMap<String, Arc<Peaks>>>,
    view: Option<View>,
    /// The size the page gives the picture, in points.
    size: (f64, f64),
    palette: Option<Palette>,
    /// Read and draw here instead of on the background threads: for the screenshots, which have no event loop.
    here: bool,
}

/// The words on the picture: one list for each strip, kept and changed line by line, so moving the view doesn't make
/// the window build every word again.
#[derive(Default)]
struct Words {
    header: Rc<VecModel<ArrLabel>>,
    timeline: Rc<VecModel<ArrLabel>>,
    ruler: Rc<VecModel<ArrLabel>>,
}

/// The painter on the card it was made on (which card, as `gpu::generation` counts).
struct OnCard {
    generation: u64,
    canvas: Canvas,
}

/// The song shown last, so the reading thread can stop reading an older song's audio files.
static SHOWN: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static WORDS: Words = Words::default();
    static READER: RefCell<Option<Newest<(u64, PathBuf)>>> = const { RefCell::new(None) };
    static DRAWER: RefCell<Option<Newest<DrawJob>>> = const { RefCell::new(None) };
    static CARD: RefCell<Option<OnCard>> = const { RefCell::new(None) };
    /// The card (as `gpu::generation` counts) that refused to draw the arrangement: on that one the processor draws it
    /// from then on, and a new card is tried again.
    static CARD_FAILED: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Wires the preview's mouse and view chips, and starts its reading and drawing threads.
pub fn connect(app: &AppWindow) {
    let data = app.global::<ArrangementData>();
    let weak = app.as_weak();
    data.on_pick_view(move |i| {
        if let Some(app) = weak.upgrade() {
            pick_view(&app, usize::try_from(i).unwrap_or(0));
        }
    });
    let weak = app.as_weak();
    data.on_resized(move |w, h| {
        change(&weak, |s| s.size = (f64::from(w), f64::from(h)));
    });
    let weak = app.as_weak();
    data.on_wheel(move |x, across, down, ctrl, shift| {
        with_view(&weak, |v| {
            v.wheel(f64::from(x), f64::from(across), f64::from(down), ctrl, shift);
            true
        });
    });
    let weak = app.as_weak();
    data.on_grab(move |x, y| {
        with_view(&weak, |v| {
            v.press(f64::from(x), f64::from(y));
            false
        });
    });
    // Only a drag changes the picture; the mouse just moving changes what's under it, which the window shows itself.
    let weak = app.as_weak();
    data.on_moved(move |x, y| {
        with_view(&weak, |v| {
            v.moved(f64::from(x), f64::from(y));
            v.dragging()
        });
    });
    let weak = app.as_weak();
    data.on_let_go(move || {
        with_view(&weak, |v| {
            v.release();
            false
        });
    });
    let weak = app.as_weak();
    data.on_double_clicked(move || {
        with_view(&weak, |v| {
            v.double_click();
            true
        });
    });
    let weak = app.as_weak();
    data.on_left(move || {
        with_view(&weak, |v| {
            v.leave();
            false
        });
    });

    let weak = app.as_weak();
    let reader = Newest::start("Arrangement", move |(asked, path): (u64, PathBuf)| {
        let read = arr::read(&path, Some(&mo_app::paths::data_dir().join("sounds.db")));
        let found = read.as_ref().ok().map(|p| p.found.clone()).filter(|f| !f.is_empty());
        let _ = weak.upgrade_in_event_loop(move |app| arrived(&app, asked, read));
        // Then the waveforms: the arrangement is up meanwhile.
        if let Some(found) = found {
            let peaks = background::on_pool(|| read_peaks(&found, asked));
            let _ = weak.upgrade_in_event_loop(move |app| waves_arrived(&app, asked, peaks));
        }
    });
    match reader {
        Ok(r) => READER.with(|slot| *slot.borrow_mut() = Some(r)),
        Err(e) => log::error("Arrangement", &format!("Couldn't start the arrangement's reader: {e}")),
    }
    WORDS.with(|w| {
        data.set_header_labels(ModelRc::from(w.header.clone()));
        data.set_timeline_labels(ModelRc::from(w.timeline.clone()));
        data.set_ruler_labels(ModelRc::from(w.ruler.clone()));
    });
    let weak = app.as_weak();
    let drawer = Newest::start("Arrangement drawing", move |(asked, view, palette, scale, on_card): DrawJob| {
        // Worked out here, and for the processor drawn here too, so the window's thread only puts it up.
        let picture = Picture::new(view.frame(&palette, scale), scale, on_card);
        let _ = weak.upgrade_in_event_loop(move |app| drawn(&app, asked, picture));
    });
    match drawer {
        Ok(d) => DRAWER.with(|slot| *slot.borrow_mut() = Some(d)),
        Err(e) => log::error("Arrangement", &format!("Couldn't start drawing the arrangement: {e}")),
    }
}

/// Uses a look's colours, and draws again in them.
pub fn set_look(app: &AppWindow, look: &Look) {
    STATE.with(|s| s.borrow_mut().palette = Some(Palette::from_look(look)));
    redraw(app);
}

/// Reads and draws on the window's thread from now on: for the screenshots, which have no event loop to post to.
pub fn draw_here() {
    STATE.with(|s| s.borrow_mut().here = true);
}

/// Shows the arrangement of the save at `path` (the picked song's newest), or empties the preview when no song is
/// picked or its save has gone.
pub fn show_song(app: &AppWindow, path: Option<&str>, missing: bool) {
    let data = app.global::<ArrangementData>();
    let (asked, here) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.asked += 1;
        s.preview = None;
        s.peaks = Arc::default();
        s.view = None;
        (s.asked, s.here)
    });
    SHOWN.store(asked, Ordering::Relaxed);
    clear(&data);
    let Some(path) = path.filter(|p| !p.is_empty()) else { return };
    if missing {
        data.set_message(NOT_HERE.into());
        return;
    }
    data.set_message(READING.into());
    let path = PathBuf::from(path);
    if here {
        // The screenshots: their made-up songs, without the real Sounds library.
        arrived(app, asked, arr::read(&path, None));
        return;
    }
    READER.with(|slot| {
        if let Some(reader) = slot.borrow().as_ref() {
            reader.ask((asked, path));
        }
    });
}

/// Shows an arrangement already read: for the screenshots.
pub fn show_preview(app: &AppWindow, preview: Preview) {
    let asked = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.asked += 1;
        s.asked
    });
    arrived(app, asked, Ok(preview));
}

fn clear(data: &ArrangementData) {
    data.set_views(ModelRc::default());
    data.set_view(0);
    data.set_summary(SharedString::new());
    data.set_notes(ModelRc::default());
    data.set_picture(Image::default());
    WORDS.with(|w| {
        for model in [&w.header, &w.timeline, &w.ruler] {
            model.clear();
        }
    });
    data.set_hover_text(SharedString::new());
    data.set_dragging(false);
    data.set_message(SharedString::new());
}

/// A save's arrangement has been read: shown if its song is still the one picked.
fn arrived(app: &AppWindow, asked: u64, read: Result<Preview, String>) {
    if STATE.with(|s| s.borrow().asked != asked) {
        return;
    }
    let data = app.global::<ArrangementData>();
    let preview = match read {
        Ok(p) => p,
        Err(e) => {
            log::warning("Arrangement", &e);
            data.set_message(e.into());
            return;
        }
    };
    if preview.views.is_empty() {
        data.set_message(NOT_YET.into());
        return;
    }
    data.set_views(strings(preview.views.iter().map(|a| a.view.to_owned())));
    STATE.with(|s| s.borrow_mut().preview = Some(preview));
    pick_view(app, 0);
}

/// Reads each audio file the arrangement plays (up to its first minute) and boils it down to a waveform, a file per
/// core on the low-priority pool. Stops early when another song has been picked; a file that can't be read is
/// drawn as a plain band.
fn read_peaks(found: &HashMap<String, PathBuf>, asked: u64) -> Arc<HashMap<String, Arc<Peaks>>> {
    let peaks = found
        .par_iter()
        .filter_map(|(saved, file)| {
            if SHOWN.load(Ordering::Relaxed) != asked {
                return None;
            }
            let audio = mo_audio::read(file, mo_audio::MAX_SECONDS).ok()?;
            Some((saved.clone(), Arc::new(Peaks::of(audio.sample_rate, &audio.channels, audio.file_seconds))))
        })
        .collect();
    Arc::new(peaks)
}

/// The waveforms came: drawn, if their song is still the one picked.
fn waves_arrived(app: &AppWindow, asked: u64, peaks: Arc<HashMap<String, Arc<Peaks>>>) {
    let current = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.asked != asked {
            return false;
        }
        s.peaks = peaks.clone();
        if let Some(v) = s.view.as_mut() {
            v.set_peaks(peaks);
        }
        true
    });
    if current {
        redraw(app);
    }
}

/// Shows the project's view at `index` (the FL playlist, or as it opens in Bitwig), fitted to the width.
fn pick_view(app: &AppWindow, index: usize) {
    let data = app.global::<ArrangementData>();
    let shown = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let p = s.preview.as_ref()?;
        let a = p.views.get(index)?.clone();
        let mut view = View::new(a.clone(), p.found.clone());
        view.set_peaks(s.peaks.clone());
        view.resize(s.size.0, s.size.1);
        s.view = Some(view);
        Some(a)
    });
    let Some(a) = shown else { return };
    data.set_view(i32::try_from(index).unwrap_or(0));
    data.set_summary(arr::summary(&a).into());
    if a.lanes.is_empty() {
        // Nothing to draw: the reason, when there is one, takes the picture's place.
        let why = a.notes.join(" ");
        data.set_notes(ModelRc::default());
        data.set_message(if why.is_empty() {
            "Nothing has been placed in this arrangement yet.".into()
        } else {
            why.into()
        });
        return;
    }
    data.set_notes(strings(a.notes.iter().cloned()));
    data.set_message(SharedString::new());
    redraw(app);
}

/// Changes what the view knows and draws it again.
fn change(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut State)) {
    let Some(app) = weak.upgrade() else { return };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        f(&mut s);
        let size = s.size;
        if let Some(v) = s.view.as_mut() {
            v.resize(size.0, size.1);
        }
    });
    redraw(&app);
}

/// Tells the view what the mouse did, shows what's under it now, and draws it again when `f` says the picture changed.
fn with_view(weak: &slint::Weak<AppWindow>, f: impl FnOnce(&mut View) -> bool) {
    let Some(app) = weak.upgrade() else { return };
    let done = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let v = s.view.as_mut()?;
        let changed = f(v);
        Some((changed, v.dragging(), v.hover()))
    });
    let Some((changed, dragging, hover)) = done else { return };
    let data = app.global::<ArrangementData>();
    data.set_dragging(dragging);
    show_hover(&data, hover);
    if changed {
        redraw(&app);
    }
}

fn show_hover(data: &ArrangementData, hover: Option<Hover>) {
    match hover {
        Some(h) => {
            data.set_hover_x(h.x as f32);
            data.set_hover_y(h.y as f32);
            data.set_hover_text(h.text.into());
        }
        None => data.set_hover_text(SharedString::new()),
    }
}

/// Asks for a picture of the view as it is now: for the graphics card when there's one to draw on.
fn redraw(app: &AppWindow) {
    let scale = f64::from(app.window().scale_factor());
    let job = STATE.with(|s| {
        let s = s.borrow();
        let view = s.view.clone()?;
        (view.width >= 1.0 && view.height >= 1.0).then_some(())?;
        Some((s.asked, view, s.palette?, scale, s.here))
    });
    let Some((asked, view, palette, scale, here)) = job else { return };
    if here {
        drawn(app, asked, Picture::new(view.frame(&palette, scale), scale, false));
        return;
    }
    let on_card = usable_card().is_some();
    DRAWER.with(|slot| {
        if let Some(drawer) = slot.borrow().as_ref() {
            drawer.ask((asked, view, palette, scale, on_card));
        }
    });
}

/// The graphics card to draw the arrangement on, when there's one it can draw on: not the card that refused to
/// (`CARD_FAILED`). The screenshots never ask (they draw here, on the processor).
fn usable_card() -> Option<(gpu::Gpu, u64)> {
    let failed = CARD_FAILED.with(Cell::get);
    gpu::current().filter(|(_, generation)| failed != Some(*generation))
}

/// A graphics card was handed over (the window opened, or the card was reset): the arrangement is drawn again on it.
pub fn graphics_ready(app: &AppWindow) {
    redraw(app);
}

/// The card went (the window closed, or the card was reset): the painter goes, and the picture made on it isn't shown
/// any more. It's drawn again on the next card, or on the processor.
pub fn graphics_gone(app: &AppWindow) {
    CARD.with(|c| *c.borrow_mut() = None);
    let weak = app.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.global::<ArrangementData>().set_picture(Image::default());
            redraw(&app);
        }
    });
}

/// Draws the shapes on the card, now, and gives the picture for Slint to show; `None` when there's no card to draw on
/// any more (it went since the shapes were asked for).
fn draw_on_card(shapes: &Shapes) -> Result<Option<Image>, String> {
    let Some((gpu, generation)) = usable_card() else { return Ok(None) };
    let texture = CARD.with(|c| -> Result<_, String> {
        let mut c = c.borrow_mut();
        if c.as_ref().is_none_or(|on| on.generation != generation) {
            // The first time, the card builds the shader, which can take a moment.
            let _doing = crate::watchdog::doing("Arrangement: set up the GPU painter");
            let canvas = gpu::checked(&gpu.device, || Canvas::new(&gpu.device, &gpu.queue))?;
            *c = Some(OnCard { generation, canvas });
        }
        let Some(on) = c.as_mut() else { unreachable!("made just above") };
        let _doing = crate::watchdog::doing("Arrangement: draw on the GPU");
        gpu::checked(&gpu.device, || on.canvas.draw(shapes.size, shapes.scale, shapes.background, &shapes.bytes))
    });
    let texture = texture.inspect_err(|_| {
        // The processor draws it from now on, on this card.
        CARD_FAILED.with(|f| f.set(Some(generation)));
        CARD.with(|c| *c.borrow_mut() = None);
    })?;
    Image::try_from(texture).map(Some).map_err(|e| {
        CARD_FAILED.with(|f| f.set(Some(generation)));
        format!("Slint wouldn't show the picture: {e}")
    })
}

/// The shapes of a view for the graphics card, packed on the drawing thread.
struct Shapes {
    bytes: Vec<u8>,
    /// The picture's size in pixels, and pixels to a point.
    size: (u32, u32),
    scale: f32,
    background: u32,
}

/// A worked out view, ready for the window: made on the drawing thread. Its picture is drawn there already (on the
/// processor), or its shapes are ready for the card.
struct Picture {
    pixels: Option<SharedPixelBuffer<Rgba8Pixel>>,
    shapes: Option<Shapes>,
    header: Vec<ArrLabel>,
    timeline: Vec<ArrLabel>,
    ruler: Vec<ArrLabel>,
    header_width: f32,
    ruler_height: f32,
}

impl Picture {
    fn new(frame: Frame, scale: f64, on_card: bool) -> Picture {
        let Frame { scene, labels, header_width, ruler_height, .. } = frame;
        let (pixels, shapes) = if on_card {
            let shapes = Shapes {
                bytes: scene.pack(),
                size: scene.pixel_size(scale),
                scale: scale as f32,
                background: scene.background,
            };
            (None, Some(shapes))
        } else {
            (Some(paint(&scene, scale)), None)
        };
        let (mut header, mut timeline, mut ruler) = (Vec::new(), Vec::new(), Vec::new());
        for l in labels {
            let (kind, muted) = kind(l.style);
            let line = ArrLabel {
                x: l.x as f32,
                y: l.y as f32,
                width: l.width as f32,
                height: l.height as f32,
                text: l.text.into(),
                kind,
                muted,
            };
            match l.area {
                Area::Header => header.push(line),
                Area::Timeline => timeline.push(line),
                Area::Ruler => ruler.push(line),
            }
        }
        Picture {
            pixels,
            shapes,
            header,
            timeline,
            ruler,
            header_width: header_width as f32,
            ruler_height: ruler_height as f32,
        }
    }
}

/// The scene drawn on the processor, in the window's kind of buffer.
fn paint(scene: &mo_app::shapes::Scene, scale: f64) -> SharedPixelBuffer<Rgba8Pixel> {
    let pixmap = scene.paint(scale);
    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(pixmap.data(), pixmap.width(), pixmap.height())
}

/// A picture has been worked out: drawn on the card if it's for the card, and put on the page with its words, unless
/// another song has been picked since.
fn drawn(app: &AppWindow, asked: u64, picture: Picture) {
    if STATE.with(|s| s.borrow().asked != asked) {
        return;
    }
    let data = app.global::<ArrangementData>();
    if let Some(shapes) = &picture.shapes {
        match draw_on_card(shapes) {
            Ok(Some(image)) => data.set_picture(image),
            // The card went since this was asked for: drawn again, on the processor.
            Ok(None) => {
                redraw(app);
                return;
            }
            Err(e) => {
                log::error(
                    "Arrangement",
                    &format!("The graphics card couldn't draw the arrangement, so the processor does: {e}"),
                );
                redraw(app);
                return;
            }
        }
    }
    if let Some(pixels) = picture.pixels {
        let _doing = crate::watchdog::doing("Arrangement: show the CPU picture");
        data.set_picture(Image::from_rgba8_premultiplied(pixels));
    }
    let _doing = crate::watchdog::doing("Arrangement: place the words");
    data.set_header_width(picture.header_width);
    data.set_ruler_height(picture.ruler_height);
    WORDS.with(|w| {
        refill(&w.header, picture.header);
        refill(&w.timeline, picture.timeline);
        refill(&w.ruler, picture.ruler);
    });
    // What's under the mouse now, which may have moved on since this picture was asked for.
    show_hover(&data, STATE.with(|s| s.borrow().view.as_ref().and_then(View::hover)));
}

/// Puts `rows` in `model` in place of what's there, changing only the lines that differ, so the window keeps the words
/// it has instead of making them all again.
fn refill(model: &VecModel<ArrLabel>, rows: Vec<ArrLabel>) {
    let old = model.row_count();
    let keep = old.min(rows.len());
    let mut rows = rows.into_iter();
    for (i, row) in rows.by_ref().take(keep).enumerate() {
        if model.row_data(i).as_ref() != Some(&row) {
            model.set_row_data(i, row);
        }
    }
    model.extend(rows);
    for i in (keep..old).rev() {
        model.remove(i);
    }
}

/// A label's look, and whether it's greyed out.
fn kind(style: Style) -> (ArrKind, bool) {
    match style {
        Style::Lane { group: true, muted, .. } => (ArrKind::Group, muted),
        Style::Lane { small: true, muted, .. } => (ArrKind::Automation, muted),
        Style::Lane { muted, .. } => (ArrKind::Lane, muted),
        Style::Detail => (ArrKind::Detail, false),
        Style::Head => (ArrKind::Head, false),
        Style::Bar => (ArrKind::Bar, false),
        Style::Marker => (ArrKind::Marker, false),
        Style::Clip => (ArrKind::Clip, false),
        Style::Small => (ArrKind::Small, false),
    }
}

fn strings(items: impl Iterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.map(SharedString::from).collect::<Vec<_>>()))
}

/// A made-up arrangement for the screenshots: names and notes invented here (rule 4).
pub fn sample_preview() -> Preview {
    use mo_formats::arrangement::{
        Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, PreviewPoint,
    };
    let note = |start: f64, key: u8, length: f64| PreviewNote { start, length, key, velocity: 48000, muted: false };
    let pattern = |name: &str, start: f64, length: f64, notes: Vec<PreviewNote>| PreviewClip {
        name: name.into(),
        start,
        length,
        notes,
        ..PreviewClip::default()
    };
    let beat = |bar: f64| bar * 4.0;
    let mut drums = PreviewLane::new("Drums", LaneKind::Group);
    drums.colour = Some(0xE0645A);
    let mut kick = PreviewLane::new("Kick", LaneKind::Instrument);
    kick.depth = 1;
    kick.detail = "FPC".into();
    kick.colour = Some(0xE0645A);
    let kicks: Vec<_> = (0..16).map(|i| note(i as f64, 36, 0.5)).collect();
    kick.clips = (0..6).map(|i| pattern("Kick", beat(8.0 + i as f64 * 4.0), 16.0, kicks.clone())).collect();
    let mut hats = PreviewLane::new("Hats", LaneKind::Instrument);
    hats.depth = 1;
    hats.detail = "FPC".into();
    hats.colour = Some(0xE89A5A);
    let hat: Vec<_> = (0..32).map(|i| note(i as f64 * 0.5, 42 + (i % 2) as u8 * 4, 0.25)).collect();
    hats.clips = (0..5).map(|i| pattern("Hats", beat(12.0 + i as f64 * 4.0), 16.0, hat.clone())).collect();
    let mut bass = PreviewLane::new("Bass", LaneKind::Instrument);
    bass.detail = "Serum".into();
    bass.colour = Some(0x5EA8FF);
    let line: Vec<_> =
        [36u8, 36, 43, 41, 36, 39, 43, 34].iter().enumerate().map(|(i, k)| note(i as f64 * 2.0, *k, 1.5)).collect();
    bass.clips = (0..4).map(|i| pattern("Bass line", beat(16.0 + i as f64 * 4.0), 16.0, line.clone())).collect();
    let mut keys = PreviewLane::new("Keys", LaneKind::Instrument);
    keys.detail = "Keyscape".into();
    keys.colour = Some(0x9B7BFF);
    let chords: Vec<_> = [[60u8, 64, 67], [57, 60, 64], [62, 65, 69], [59, 62, 67]]
        .iter()
        .enumerate()
        .flat_map(|(i, c)| c.iter().map(move |k| note(i as f64 * 4.0, *k, 3.5)))
        .collect();
    keys.clips = vec![
        pattern(
            "Intro chords",
            0.0,
            beat(8.0),
            [chords.clone(), chords.iter().map(|n| PreviewNote { start: n.start + 16.0, ..*n }).collect()].concat(),
        ),
        pattern("Chords", beat(16.0), beat(8.0), chords.clone()),
        PreviewClip { muted: true, ..pattern("Chords (old)", beat(24.0), beat(4.0), chords.clone()) },
    ];
    let mut vox = PreviewLane::new("Vocal", LaneKind::Audio);
    vox.colour = Some(0x4ACB8B);
    let take = |name: &str, path: &str, start: f64, length: f64| PreviewClip {
        name: name.into(),
        start,
        length,
        audio: vec![AudioPart {
            path: path.into(),
            start: 0.0,
            length,
            from: 0.0,
            seconds_per_beat: Some(0.5),
            whole_file_beats: None,
            reversed: false,
        }],
        ..PreviewClip::default()
    };
    // The first take's file is on this computer (its waveform is `sample_peaks`); the second's isn't.
    vox.clips = vec![
        take("Vocal take 3", SAMPLE_TAKE, beat(16.0), beat(8.0)),
        take("Vocal take 1", r"C:\Made up\Vocal take 1.wav", beat(26.0), beat(6.0)),
    ];
    let mut cutoff = PreviewLane::new("Filter cutoff", LaneKind::Automation);
    cutoff.colour = Some(0xF2B33D);
    cutoff.clips = vec![PreviewClip {
        name: "Cutoff".into(),
        start: 0.0,
        length: beat(16.0),
        points: vec![
            PreviewPoint { beat: 0.0, value: 0.1 },
            PreviewPoint { beat: beat(6.0), value: 0.35 },
            PreviewPoint { beat: beat(8.0), value: 0.2 },
            PreviewPoint { beat: beat(15.5), value: 0.95 },
            PreviewPoint { beat: beat(16.0), value: 0.6 },
        ],
        ..PreviewClip::default()
    }];
    let playlist = Arrangement {
        view: "FL Studio playlist",
        tempo: 128.0,
        beats_per_bar: 4,
        lanes: vec![drums, kick, hats, bass, keys, vox, cutoff],
        markers: vec![
            PreviewMarker { beat: 0.0, name: "Intro".into() },
            PreviewMarker { beat: beat(16.0), name: "Drop".into() },
            PreviewMarker { beat: beat(24.0), name: "Break".into() },
        ],
        notes: Vec::new(),
    };
    let mut bitwig = playlist.clone();
    bitwig.view = "As it opens in Bitwig";
    let found = HashMap::from([(SAMPLE_TAKE.to_owned(), PathBuf::from(SAMPLE_TAKE))]);
    Preview { views: vec![Arc::new(playlist), Arc::new(bitwig)], found: Arc::new(found) }
}

/// The made-up song's vocal take that's on this computer.
const SAMPLE_TAKE: &str = r"C:\Made up\Vocal take 3.wav";

/// A made-up waveform for the made-up song's vocal take (rule 4: no real sound): sixteen seconds of phrases that swell
/// and fade, as `Peaks::of` would boil a file down.
pub fn sample_peaks() -> Arc<HashMap<String, Arc<Peaks>>> {
    let step = 0.005;
    let values = (0..3200)
        .map(|i| {
            let t = f64::from(i) * step;
            // A phrase every two seconds, louder in the middle of each, with a little wobble.
            let phrase = (t % 2.0) / 2.0;
            let swell = (phrase * std::f64::consts::PI).sin().powf(0.7);
            let wobble = 0.75 + 0.25 * (t * 37.0).sin() * (t * 11.0).cos();
            (swell * wobble * if phrase > 0.9 { 0.1 } else { 0.95 }) as f32
        })
        .collect();
    let peaks = Peaks { file_seconds: 16.0, step, values };
    Arc::new(HashMap::from([(SAMPLE_TAKE.to_owned(), Arc::new(peaks))]))
}

/// Shows waveforms already worked out: for the screenshots.
pub fn show_peaks(app: &AppWindow, peaks: Arc<HashMap<String, Arc<Peaks>>>) {
    let asked = STATE.with(|s| s.borrow().asked);
    waves_arrived(app, asked, peaks);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::testing::{card, read_back};
    use mo_app::themes::Look;
    use slint::wgpu_30::wgpu;

    #[test]
    fn the_words_change_line_by_line() {
        let label = |text: &str| ArrLabel { text: text.into(), ..ArrLabel::default() };
        let model = VecModel::from(vec![label("a"), label("b"), label("c")]);
        refill(&model, vec![label("a"), label("x")]);
        assert_eq!(model.iter().map(|l| l.text.to_string()).collect::<Vec<_>>(), ["a", "x"]);
        refill(&model, vec![label("a"), label("x"), label("y"), label("z")]);
        assert_eq!(model.iter().map(|l| l.text.to_string()).collect::<Vec<_>>(), ["a", "x", "y", "z"]);
        refill(&model, Vec::new());
        assert_eq!(model.row_count(), 0);
    }

    /// Draws the made-up song on whatever graphics card there is (a software one will do) and on the processor, in a
    /// few views (fitted, zoomed in on the vocal's waveform, every lane squeezed in, on Paper, at 1, 1.5 and 2 pixels
    /// to a point), and checks the two pictures agree: the same shapes in the same places, differing only a little at
    /// soft edges. With ARRANGEMENT_GPU_LOOK set to a folder, writes both pictures and their difference there, to judge
    /// by eye. With ARRANGEMENT_GPU_TIME set, times the card's frames too (a software card's time is the processor's).
    #[test]
    #[ignore]
    fn the_card_draws_what_the_processor_draws() {
        let Some((device, queue)) = card() else {
            eprintln!("No graphics card here, not even a software one: nothing to compare");
            return;
        };
        let out = std::env::var("ARRANGEMENT_GPU_LOOK").ok();
        let timed = std::env::var("ARRANGEMENT_GPU_TIME").is_ok();
        let mut canvas = Canvas::new(&device, &queue);
        let preview = sample_preview();
        let peaks = sample_peaks();
        let graphite = Palette::from_look(&Look::from_settings(None, None));
        let paper = Palette::from_look(&Look::from_settings(Some("Paper"), None));
        type Setup = fn(&mut View);
        let views: [(&str, Palette, f64, Setup); 6] = [
            ("fitted", graphite, 1.0, |_| {}),
            ("fitted-2x", graphite, 2.0, |_| {}),
            ("zoomed-1.5x", graphite, 1.5, |v| v.wheel(560.0, 0.0, 6.0, true, false)),
            ("squeezed", graphite, 1.0, |v| v.double_click()),
            ("paper", paper, 1.0, |_| {}),
            ("paper-zoomed-2x", paper, 2.0, |v| v.wheel(560.0, 0.0, 6.0, true, false)),
        ];
        for (name, palette, scale, setup) in views {
            let mut view = View::new(preview.views[0].clone(), preview.found.clone());
            view.set_peaks(peaks.clone());
            view.resize(900.0, 300.0);
            setup(&mut view);
            let frame = view.frame(&palette, scale);
            let size = frame.scene.pixel_size(scale);
            let cpu = frame.scene.paint(scale);
            let bytes = frame.scene.pack();
            let picture = canvas.draw(size, scale as f32, frame.scene.background, &bytes);
            let gpu = read_back(&device, &queue, &picture);
            assert_eq!((gpu.width(), gpu.height()), (cpu.width(), cpu.height()), "{name}");
            // How far apart each pixel is: the most any of its colours differs.
            let mut diff = image::RgbaImage::new(gpu.width(), gpu.height());
            let (mut off, mut far, mut most) = (0usize, 0usize, 0u8);
            for (x, y, g) in gpu.enumerate_pixels() {
                let c = cpu.pixel(x, y).expect("inside");
                let d = [g[0].abs_diff(c.red()), g[1].abs_diff(c.green()), g[2].abs_diff(c.blue())];
                let d = d.into_iter().max().unwrap_or(0);
                most = most.max(d);
                off += usize::from(d > 8);
                far += usize::from(d > 64);
                diff.put_pixel(x, y, image::Rgba([d.saturating_mul(4), d.saturating_mul(4), d.saturating_mul(4), 255]));
            }
            let pixels = (gpu.width() * gpu.height()) as usize;
            eprintln!(
                "{name}: {} shapes, {off} of {pixels} pixels differ by more than 8, {far} by more than 64, at most {most}",
                frame.scene.shapes.len()
            );
            if let Some(out) = &out {
                gpu.save(format!("{out}/arrangement-{name}-card.png")).expect("saved");
                let cpu_png =
                    image::RgbaImage::from_raw(cpu.width(), cpu.height(), cpu.data().to_vec()).expect("same size");
                cpu_png.save(format!("{out}/arrangement-{name}-processor.png")).expect("saved");
                diff.save(format!("{out}/arrangement-{name}-difference.png")).expect("saved");
            }
            if timed {
                let started = std::time::Instant::now();
                for _ in 0..20 {
                    let _ = canvas.draw(size, scale as f32, frame.scene.background, &bytes);
                    device.poll(wgpu::PollType::wait_indefinitely()).expect("the card finishes");
                }
                let card = started.elapsed().as_secs_f64() * 1000.0 / 20.0;
                let started = std::time::Instant::now();
                for _ in 0..20 {
                    let _ = frame.scene.paint(scale);
                }
                let processor = started.elapsed().as_secs_f64() * 1000.0 / 20.0;
                eprintln!("TIMING {name}: {card:.2} ms on the card, {processor:.2} ms on the processor");
            }
            // Soft edges are worked out a little differently by each, so a few pixels along them differ; anything
            // more is a shape in the wrong place or the wrong colour.
            assert!(far * 1000 <= pixels, "{name}: {far} pixels differ by more than 64");
            assert!(off * 50 <= pixels, "{name}: {off} pixels differ by more than 8");
        }
    }
}
