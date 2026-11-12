//! The plugin's window: the app's Slint panel, drawn inside the window the DAW lends the plugin.
//!
//! Slint normally opens and runs its own windows. In a DAW the DAW owns the window, so:
//! - [`baseview`] makes the plugin's own small window inside the DAW's one, and hands it the mouse and keys;
//! - Slint draws the panel into that window on the graphics card (`gpu.rs`), or, when there's no card to draw with,
//!   on the CPU with its software renderer into a picture in memory, which [`softbuffer`] puts in the window;
//! - a short timer on the window's thread runs Slint's timers and animations and takes in what background threads
//!   found (the sound list, a search), newest first.
//!
//! On Windows and Mac the window's thread is the DAW's main thread, so nothing slow ever runs on it: reading the
//! library, searching it and reading a sound all run on background threads at low priority.

mod midi_tab;

use crate::audition::Played;
use crate::galaxy::{self, Sky};
use crate::gpu::Gpu;
use crate::library::{self, Asked, Library, ORDERS, Read};
use crate::threads::{background, lock};
use crate::waves::{self, Kept};
use crate::{
    ExtensionLine, FilterChoice, FilterView, Instance, Panel, SkyName, SoundLine, SweptLine, TrailDot, YearBar,
};
use baseview::dpi::{PhysicalPosition, Size};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, ScrollDelta, TimerHandle, WindowContext, WindowEvent,
    WindowHandler, WindowSettings, WindowSize,
};
use keyboard_types::{Key, KeyState, NamedKey};
use mo_app::backup::thousands;
use mo_app::filters::{Filter, Shown as Choice};
use mo_app::galaxy_grid::Grid;
use mo_app::galaxy_view::{Hold, Moved, View};
use mo_app::sounds::{Order, SoundCounts, TEMPO_HIGH, TEMPO_LOW, TEMPO_STEP, TYPE_CHIPS};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{PointerEventButton, WindowAdapter};
use slint::{ComponentHandle, LogicalPosition, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The panel's size, in logical pixels (before the screen's scaling).
pub const WIDTH: f64 = 760.0;
pub const HEIGHT: f64 = 560.0;
/// The smallest the window can be dragged: the filter column still fits its lines, and the list a few words.
pub const LEAST_WIDTH: f64 = 620.0;
pub const LEAST_HEIGHT: f64 = 560.0;
/// The biggest: more than any screen today, so a DAW that asks for a silly size doesn't get a picture that big.
pub const MOST_WIDTH: f64 = 7680.0;
pub const MOST_HEIGHT: f64 = 4320.0;

/// How often the window's timer runs: Slint's timers and animations, and what background threads found.
const TICK: Duration = Duration::from_millis(16);

/// The longest a glide of the Galaxy goes on by in one step (the timer was held up: it doesn't jump).
const LONGEST_STEP: Duration = Duration::from_millis(100);

/// Opens the panel inside `parent` (the window the DAW lends), at `size`: the size the plugin told the DAW, so the two
/// match. The DAW resizes it afterwards ([`baseview::Window::resize`]); the panel follows in its `resized`. The window goes when the returned one is dropped.
pub fn open(
    parent: &impl raw_window_handle::HasWindowHandle,
    size: Size,
    instance: Arc<Instance>,
) -> Result<baseview::Window, String> {
    crate::threads::note_crashes();
    let settings =
        WindowSettings::new().with_title("Music Organiser").with_size(size).with_parent(parent).with_resizable(true);
    let window = baseview::Window::create(settings, move |context| {
        let shown = Shown::new(context.clone(), Arc::clone(&instance))
            .map_err(|why| HandlerError::from(std::io::Error::other(why)))?;
        Ok(Window { context, instance, shown: RefCell::new(Some(shown)) })
    })
    .map_err(|e| format!("Couldn't open the plugin's window: {e}"))?;
    window.show().map_err(|e| format!("Couldn't show the plugin's window: {e}"))?;
    Ok(window)
}

/// A pixel as `softbuffer` takes it: 0x00RRGGBB.
#[repr(transparent)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Pixel(u32);

impl TargetPixel for Pixel {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let keep = 255 - u32::from(color.alpha);
        let mix = |shift: u32, over: u8| {
            let under = (self.0 >> shift) & 0xFF;
            (u32::from(over) + (under * keep + 127) / 255).min(255) << shift
        };
        self.0 = mix(16, color.red) | mix(8, color.green) | mix(0, color.blue);
    }

    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Pixel(u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue))
    }
}

/// Slint's link to the system, for a thread with plugin windows on it. Slint keeps one per thread; each window hands
/// it the window it should use next ([`NEXT`]), just before making the panel.
struct Platform {
    started: Instant,
}

thread_local! {
    static NEXT: RefCell<Option<Rc<dyn WindowAdapter>>> = const { RefCell::new(None) };
}

impl slint::platform::Platform for Platform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        NEXT.with(|next| next.borrow_mut().take())
            .ok_or_else(|| slint::PlatformError::Other("no plugin window to draw in".into()))
    }

    fn duration_since_start(&self) -> Duration {
        self.started.elapsed()
    }
}

/// What background threads hand the panel: the newest of each wins.
#[derive(Default)]
struct Slots {
    library: Mutex<Option<Read>>,
    /// What the list shows for what was asked, with the filter column's counts and the asking's number.
    found: Mutex<Option<(u64, Vec<usize>, SoundCounts)>>,
    /// Counts askings: only the newest one's answer is shown.
    searches: AtomicU64,
    played: Mutex<Option<Played>>,
    /// A line dragged out: which, and its file, once it's been checked to be there (or written, for a MIDI part),
    /// or why it can't go.
    drag: Mutex<Option<(Carried, Result<std::path::PathBuf, String>)>>,
    /// A drag out that has ended: its file, when it started and how it ended.
    dragged: Mutex<Option<Dragged>>,
    /// The waveforms read for the lines on show, with their asking's round ([`Kept::to_read`]).
    waves: Mutex<Option<(u64, WavesRead)>>,
    /// The Galaxy's map as the app last kept it, or why there's none.
    map: Mutex<Option<Result<galaxy::Map, String>>>,
    /// The grid for finding the star under the mouse, with its asking's number ([`Sky::grid_ask`]).
    grid: Mutex<Option<(u64, Grid)>>,
    sky: Mutex<Option<SkyDrawn>>,
    /// The neighbourhoods the names over the map are for, with their asking's number ([`Sky::names_ask`]).
    names: Mutex<Option<(u64, Vec<mo_app::galaxy_names::Region>)>>,
    /// The stars made ready for the graphics card, and how they're lit ([`Sky::pack_ask`]).
    packed: Mutex<Option<galaxy::Packed>>,
    /// The mini-map's picture, with its asking's number ([`Sky::mini_ask`]).
    mini: Mutex<Option<(u64, SkyPixels)>>,
}

/// The line being dragged out: a row of the Sounds list, or a part of the MIDI tab's (its place in the catalogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carried {
    Sound(usize),
    Part(usize),
}

/// A drag out that has ended: its file, when it started and how it ended.
type Dragged = (std::path::PathBuf, Instant, Result<crate::drag::Outcome, String>);

/// The Galaxy's trail as the panel shows it: its line, its dots, and the Swept card's sounds with the one it's on.
type Trail = (String, Vec<(f32, f32, f32)>, Vec<usize>, usize);

type SkyPixels = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
/// The Galaxy's picture drawn: its number, the view it was drawn for and its pixels; or just its number when it
/// couldn't be drawn.
type SkyDrawn = Result<(u64, View, SkyPixels), u64>;

/// Each sound's waveform as a picture's pixels (made off the window's thread), or none.
type WavesRead = Vec<(i64, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>;

/// A picture of the stars drawn on the processor, as pixels the panel can show.
fn sky_pixels(drawn: &mo_app::galaxy_picture::Drawn) -> Option<SkyPixels> {
    let (width, height) = (u32::try_from(drawn.width).ok()?, u32::try_from(drawn.height).ok()?);
    let mut pixels = SkyPixels::new(width, height);
    for (to, from) in pixels.make_mut_slice().iter_mut().zip(&drawn.pixels) {
        *to = slint::Rgba8Pixel { r: from.r, g: from.g, b: from.b, a: from.a };
    }
    Some(pixels)
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    lock(slot).take()
}

/// The list's lines, made only for the rows shown (the list can hold half a million sounds).
struct Lines {
    library: RefCell<Option<Arc<Library>>>,
    shown: RefCell<Arc<Vec<usize>>>,
    /// The waveforms of the lines on show, as pictures.
    waves: RefCell<Kept<slint::Image>>,
    notify: ModelNotify,
}

impl Lines {
    fn show(&self, library: Option<Arc<Library>>, shown: Arc<Vec<usize>>) {
        *self.library.borrow_mut() = library;
        *self.shown.borrow_mut() = shown;
        self.notify.reset();
    }

    fn path(&self, row: usize) -> Option<String> {
        let library = self.library.borrow();
        let at = *self.shown.borrow().get(row)?;
        library.as_ref()?.list.sound(at).map(|s| s.path.clone())
    }

    /// The Swept card's line for the sound at place `at` in the whole list.
    fn swept(&self, at: usize, current: bool) -> Option<SweptLine> {
        let library = self.library.borrow();
        let sound = library.as_ref()?.list.sound(at)?;
        Some(SweptLine {
            name: library::name(&sound.path).into(),
            kind: sound.sound_type.as_str().into(),
            colour: mo_app::sounds::type_colour(&sound.sound_type),
            current,
        })
    }

    /// The file of the sound at place `at` in the whole list (a star of the Galaxy).
    fn path_at(&self, at: usize) -> Option<String> {
        self.library.borrow().as_ref()?.list.sound(at).map(|s| s.path.clone())
    }

    /// The id of the sound on `row`.
    fn sound(&self, row: usize) -> Option<i64> {
        let library = self.library.borrow();
        let at = *self.shown.borrow().get(row)?;
        library.as_ref()?.list.sound(at).map(|s| s.id)
    }
}

impl Model for Lines {
    type Data = SoundLine;

    fn row_count(&self) -> usize {
        self.shown.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<SoundLine> {
        let library = self.library.borrow();
        let at = *self.shown.borrow().get(row)?;
        let sound = library.as_ref()?.list.sound(at)?;
        Some(SoundLine {
            name: library::name(&sound.path).into(),
            folder: library::folder(&sound.path).into(),
            facts: library::facts(sound).into(),
            favourite: sound.favourite,
            colour: mo_app::sounds::type_colour(&sound.sound_type),
            // Asked for here, so only the lines on show are ever read.
            wave: self.waves.borrow_mut().picture(row, sound.id).unwrap_or_default(),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

thread_local! {
    /// The clipboard, kept open on the window's thread: on Linux what's copied only stays while it is.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Puts `text` on the clipboard.
fn copy_text(text: String) -> Result<(), arboard::Error> {
    CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(arboard::Clipboard::new()?);
        }
        match c.as_mut() {
            Some(clip) => clip.set_text(text),
            None => Ok(()),
        }
    })
}

/// How the panel gets into the window.
enum Draws {
    /// On the graphics card (`gpu.rs`).
    Card(Rc<Gpu>),
    /// On the processor: Slint draws a picture in memory, and `softbuffer` puts it in the window.
    Processor {
        window: Rc<MinimalSoftwareWindow>,
        surface: RefCell<softbuffer::Surface<baseview::PlatformHandle, baseview::PlatformHandle>>,
        /// The last picture Slint drew, put in the window whenever it's asked to show.
        picture: RefCell<Vec<Pixel>>,
    },
}

/// Set once drawing on the card has gone wrong in this DAW: windows opened afterwards draw on the processor.
static CARD_FAILED: AtomicBool = AtomicBool::new(false);

impl Draws {
    /// On the card when there's one to draw with; if not, on the processor (and the log says why).
    fn new(handle: baseview::PlatformHandle, size: WindowSize) -> Result<Draws, String> {
        let (width, height) = (size.physical.width.max(1), size.physical.height.max(1));
        if std::env::var_os("MO_DRAW_CPU").is_some() {
            crate::log::line("Graphics", "MO_DRAW_CPU is set: the panel draws on the processor");
        } else if CARD_FAILED.load(Ordering::Acquire) {
            crate::log::line("Graphics", "Drawing on the card went wrong earlier: the panel draws on the processor");
        } else {
            // A fault in the card's own code is caught here, so it never reaches the DAW.
            match mo_catch::catch_crash(AssertUnwindSafe(|| Gpu::new(&handle, width, height))) {
                Ok(Ok(gpu)) => {
                    crate::log::line("Graphics", "The panel draws on the graphics card");
                    return Ok(Draws::Card(gpu));
                }
                Ok(Err(why)) => {
                    // With a card found and still no drawing on it, it isn't tried again in this DAW.
                    if crate::gpu::found() {
                        CARD_FAILED.store(true, Ordering::Release);
                    }
                    crate::log::line("Graphics", &format!("The panel draws on the processor: {why}"));
                }
                Err(_) => {
                    CARD_FAILED.store(true, Ordering::Release);
                    crate::log::line("Graphics", "Setting up the card crashed: the panel draws on the processor");
                }
            }
        }
        let soft = softbuffer::Context::new(handle.clone()).map_err(|e| format!("Couldn't draw in the window: {e}"))?;
        let surface =
            softbuffer::Surface::new(&soft, handle).map_err(|e| format!("Couldn't draw in the window: {e}"))?;
        Ok(Draws::Processor {
            window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
            surface: RefCell::new(surface),
            picture: RefCell::new(Vec::new()),
        })
    }

    /// What Slint takes the window as.
    fn adapter(&self) -> Rc<dyn WindowAdapter> {
        match self {
            Draws::Card(gpu) => Rc::clone(gpu) as Rc<dyn WindowAdapter>,
            Draws::Processor { window, .. } => Rc::clone(window) as Rc<dyn WindowAdapter>,
        }
    }

    fn window(&self) -> &slint::Window {
        match self {
            Draws::Card(gpu) => gpu.window(),
            Draws::Processor { window, .. } => window.window(),
        }
    }

    /// The window is now `width` by `height` pixels: the panel is drawn again at that size.
    fn resize(&self, width: u32, height: u32) {
        match self {
            Draws::Card(gpu) => gpu.resize(width, height),
            Draws::Processor { window, picture, .. } => {
                *picture.borrow_mut() = vec![Pixel::default(); width as usize * height as usize];
                window.set_size(slint::PhysicalSize::new(width, height));
                window.request_redraw();
            }
        }
    }

    /// Whether a picture has been made at the window's size yet.
    fn sized(&self) -> bool {
        match self {
            Draws::Card(_) => true,
            Draws::Processor { picture, .. } => !picture.borrow().is_empty(),
        }
    }

    /// Slint draws the panel again if anything on it changed. On the card it goes straight into the window; on the
    /// processor into the picture, and true says the window should be asked to show it.
    fn draw_if_needed(&self, width: u32) -> bool {
        match self {
            Draws::Card(gpu) => {
                if CARD_FAILED.load(Ordering::Acquire) {
                    return false;
                }
                match mo_catch::catch_crash(AssertUnwindSafe(|| gpu.draw_if_needed())) {
                    Ok(Ok(())) => {}
                    Ok(Err(why)) => crate::log::line("Graphics", &format!("A frame wasn't drawn: {why}")),
                    Err(_) => {
                        // Nothing more is drawn in this window; one opened next draws on the processor.
                        CARD_FAILED.store(true, Ordering::Release);
                        crate::log::line("Graphics", "Drawing on the card crashed: close and open the panel again");
                    }
                }
                false
            }
            Draws::Processor { window, picture, .. } => window.draw_if_needed(|renderer| {
                let mut picture = picture.borrow_mut();
                renderer.render(picture.as_mut_slice(), width as usize);
            }),
        }
    }

    /// The system asks for the window to be shown again (it was uncovered): `width` by `height` pixels.
    fn show(&self, width: u32, height: u32) -> Result<(), HandlerError> {
        match self {
            Draws::Card(gpu) => {
                gpu.request_redraw();
                Ok(())
            }
            Draws::Processor { surface, picture, .. } => {
                let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else { return Ok(()) };
                let mut surface = surface.borrow_mut();
                surface.resize(w, h).map_err(|e| HandlerError::from(std::io::Error::other(e.to_string())))?;
                let mut buffer =
                    surface.buffer_mut().map_err(|e| HandlerError::from(std::io::Error::other(e.to_string())))?;
                let picture = picture.borrow();
                let pixels: &[u32] = bytemuck::cast_slice(picture.as_slice());
                let len = buffer.len().min(pixels.len());
                buffer[..len].copy_from_slice(&pixels[..len]);
                buffer.present().map_err(|e| HandlerError::from(std::io::Error::other(e.to_string())))?;
                Ok(())
            }
        }
    }

    /// Drawing on the processor only because the card was still being looked for when the window opened, and it's
    /// found now.
    fn card_came(&self) -> bool {
        matches!(self, Draws::Processor { .. })
            && std::env::var_os("MO_DRAW_CPU").is_none()
            && !CARD_FAILED.load(Ordering::Acquire)
            && crate::gpu::found()
    }

    /// Whether drawing waits for the screen (the card shows a frame in step with it): then the panel is drawn on the
    /// timer only, not after every move of the mouse, or the moves would queue up behind the screen.
    fn waits_for_the_screen(&self) -> bool {
        matches!(self, Draws::Card(_))
    }
}

/// What the window hands its events to: the open panel, which is made again on the graphics card when the card
/// wasn't ready as the window opened (finding it the first time in a DAW takes a few seconds; the window doesn't
/// wait for it).
struct Window {
    context: WindowContext,
    instance: Arc<Instance>,
    shown: RefCell<Option<Shown>>,
}

impl Window {
    /// Makes the panel again, drawn on the card, once the card is found. What was picked in the panel's first
    /// moments starts again; the sound library is read once all the same (`library::read`).
    fn move_to_the_card(&self) {
        if !self.shown.borrow().as_ref().is_some_and(|shown| shown.draws.card_came()) {
            return;
        }
        // Not while the panel is in the middle of something (a drag out holds this thread, and the timer still runs).
        let Ok(mut shown) = self.shown.try_borrow_mut() else { return };
        crate::log::line("Graphics", "The graphics card is ready: the panel moves to it");
        // The old panel and its hold on the window go first.
        drop(shown.take());
        match Shown::new(self.context.clone(), Arc::clone(&self.instance)) {
            Ok(new) => *shown = Some(new),
            Err(why) => crate::log::line("Window", &format!("The panel couldn't be made again: {why}")),
        }
    }
}

impl WindowHandler for Window {
    fn draw(&self) -> Result<(), HandlerError> {
        self.shown.borrow().as_ref().map_or(Ok(()), WindowHandler::draw)
    }

    fn resized(&self, new_size: WindowSize) -> Result<(), HandlerError> {
        self.shown.borrow().as_ref().map_or(Ok(()), |shown| shown.resized(new_size))
    }

    fn on_timer(&self, timer: &TimerHandle) {
        self.move_to_the_card();
        if let Some(shown) = self.shown.borrow().as_ref() {
            shown.on_timer(timer);
        }
    }

    fn on_event(&self, event: Event) -> EventStatus {
        self.shown.borrow().as_ref().map_or(EventStatus::Ignored, |shown| shown.on_event(event))
    }
}

/// The open panel, on the window's thread.
struct Shown {
    context: WindowContext,
    /// Before `panel`, which draws through it: dropped after it. And before the window closes (baseview drops this
    /// handler first), as `gpu.rs` needs.
    draws: Draws,
    panel: Panel,
    size: Cell<(u32, u32)>,
    scale: Cell<f64>,
    pointer: Cell<LogicalPosition>,
    lines: Rc<Lines>,
    slots: Arc<Slots>,
    instance: Arc<Instance>,
    /// What the search box, the filter column and the order ask of the list.
    asked: Rc<RefCell<Asked>>,
    column: Rc<Column>,
    /// The MIDI tab.
    midi: midi_tab::MidiTab,
    /// What's being dragged out now.
    carried: Cell<Option<Carried>>,
    /// The sound the "Playing" line names, as the player numbers it: the line goes when that sound ends.
    playing: Cell<Option<u64>>,
    /// The line last clicked to play: its row, its sound and its file.
    played: Rc<RefCell<Option<(usize, i64, std::path::PathBuf)>>>,
    /// How wide the panel draws a line's waveform, in logical pixels: the pictures are made for it.
    wave_width: Cell<f32>,
    /// A line's file is being carried out now ([`Shown::drag`]).
    dragging: Cell<bool>,
    /// The panel is being told of the right button going down or coming up, now: a line under the mouse takes
    /// that as a click too (the app's lines open a menu with it), and [`line_click`] has to tell the two apart.
    right_now: Rc<Cell<bool>>,
    /// What was last clicked to be heard, and whether it has started playing: a click on it again stops it.
    heard: Rc<Cell<Option<(Heard, bool)>>>,
    /// The Galaxy: its map, its view and which picture is on show.
    sky: Rc<RefCell<Sky>>,
    /// The names last put over the Galaxy's map.
    sky_names: RefCell<Vec<SkyName>>,
    /// The trail last put on the Galaxy: its line, its dots, and the Swept card's sounds with the one it's on.
    sky_trail: RefCell<Trail>,
    /// The Galaxy's painter on the graphics card (the app's own), once there are stars to give it.
    painter: RefCell<Option<mo_sky::Sky>>,
    /// Painting the stars on the card went wrong: the processor's picture from then on.
    painter_failed: Cell<bool>,
    /// The Galaxy's map has been asked for.
    sky_asked: Cell<bool>,
    /// When the timer last ran: a glide of the Galaxy goes on by the time since.
    ticked: Cell<Instant>,
    _timer: TimerHandle,
}

impl Shown {
    fn new(context: WindowContext, instance: Arc<Instance>) -> Result<Shown, String> {
        // Already there for an earlier plugin window on this thread: that one is used.
        let _ = slint::platform::set_platform(Box::new(Platform { started: Instant::now() }));
        let draws = Draws::new(context.platform_handle(), context.size())?;
        NEXT.with(|next| *next.borrow_mut() = Some(draws.adapter()));
        let panel = Panel::new();
        // Taken back whether or not the panel was made, so no later window gets this one.
        NEXT.with(|next| next.borrow_mut().take());
        let panel = panel.map_err(|e| format!("Couldn't make the panel: {e}"))?;

        let timer = context.create_timer(TICK).map_err(|e| format!("Couldn't start the window's timer: {e}"))?;

        let lines = Rc::new(Lines {
            library: RefCell::new(None),
            shown: RefCell::new(Arc::default()),
            waves: RefCell::new(Kept::default()),
            notify: ModelNotify::default(),
        });
        let slots = Arc::new(Slots::default());
        let shown = Shown {
            context,
            draws,
            panel,
            size: Cell::new((0, 0)),
            scale: Cell::new(1.0),
            pointer: Cell::new(LogicalPosition::default()),
            lines,
            slots,
            instance,
            asked: Rc::new(RefCell::new(Asked { seed: seed(), ..Asked::default() })),
            column: Rc::new(Column::default()),
            midi: midi_tab::MidiTab::new(),
            carried: Cell::new(None),
            playing: Cell::new(None),
            played: Rc::new(RefCell::new(None)),
            wave_width: Cell::new(waves::WIDTH as f32),
            dragging: Cell::new(false),
            right_now: Rc::default(),
            heard: Rc::default(),
            sky: Rc::default(),
            sky_names: RefCell::default(),
            sky_trail: RefCell::default(),
            painter: RefCell::new(None),
            painter_failed: Cell::new(false),
            sky_asked: Cell::new(false),
            ticked: Cell::new(Instant::now()),
            _timer: timer,
        };
        shown.fill_in();
        shown.resize_to(shown.context.size());
        Ok(shown)
    }

    /// The panel's words and lists, and what its buttons do; then the sound library is read in the background.
    fn fill_in(&self) {
        let panel = &self.panel;
        panel.set_host(self.instance.host.as_str().into());
        panel.set_track(lock(&self.instance.track).as_str().into());
        let extensions: Vec<ExtensionLine> = self
            .instance
            .offered
            .iter()
            .map(|o| ExtensionLine { name: o.name.into(), id: o.id.as_str().into(), offered: o.offered })
            .collect();
        panel.set_extensions(ModelRc::new(slint::VecModel::from(extensions)));
        panel.set_extensions_summary(crate::check::summary(&self.instance.offered).into());
        panel.set_sounds(ModelRc::from(Rc::clone(&self.lines) as Rc<dyn Model<Data = SoundLine>>));
        panel.set_status("Reading the sound library".into());

        let lines = Rc::clone(&self.lines);
        let player = Arc::clone(&self.instance.player);
        let slots = Arc::clone(&self.slots);
        let last_played = Rc::clone(&self.played);
        let stop = self.stopper();
        let (right_now, heard, stop_it) = (Rc::clone(&self.right_now), Rc::clone(&self.heard), Rc::clone(&stop));
        panel.on_play(move |row, from| {
            let Some(row) = usize::try_from(row).ok() else { return };
            let Some(sound) = lines.sound(row) else { return };
            let playing = heard.get().is_some_and(|(what, _)| what == Heard::Line(sound));
            let from = match line_click(playing, right_now.get(), from) {
                Click::Play(from) => from,
                Click::Stop => return stop_it(),
                Click::Nothing => return,
            };
            let Some(path) = lines.path(row) else { return };
            *last_played.borrow_mut() = Some((row, sound, path.clone().into()));
            heard.set(Some((Heard::Line(sound), false)));
            let slots = Arc::clone(&slots);
            player.play(path.into(), from, move |played| *lock(&slots.played) = Some(played));
        });
        let stop_it = Rc::clone(&stop);
        panel.on_stop(move || stop_it());
        // Trim silence, for this plugin while it's loaded: it starts off, at -60 dB, and shows what the player has
        // when the window opens again.
        panel.set_trim_choices(ModelRc::new(slint::VecModel::from(
            mo_audio::trim::choices().into_iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
        if let Some(db) = self.instance.player.trim() {
            panel.set_trim_level_db(db);
            panel.set_trim_silence(true);
        }
        let player = Arc::clone(&self.instance.player);
        let weak = panel.as_weak();
        panel.on_pick_trim(move |picked| {
            let Some(panel) = weak.upgrade() else { return };
            // "" turns it off; a level ("-50 dB") turns it on at that level.
            let picked = mo_audio::trim::parse_pick(&picked);
            if let Some(db) = picked {
                panel.set_trim_level_db(db);
            }
            panel.set_trim_silence(picked.is_some());
            player.set_trim(picked);
        });
        panel.set_can_drag(crate::drag::CAN);
        let lines = Rc::clone(&self.lines);
        let slots = Arc::clone(&self.slots);
        panel.on_drag_out(move |row| {
            let Some((row, path)) = usize::try_from(row).ok().and_then(|r| Some((r, lines.path(r)?))) else { return };
            let slots = Arc::clone(&slots);
            // Whether the file is there is asked off the window's thread: its drive may be asleep or away.
            background("mo-plugin-drag", move || {
                let path = std::path::PathBuf::from(path);
                let found = if path.is_file() {
                    Ok(path)
                } else {
                    Err(format!("Can't drag {}: the file isn't there", library::name(&path.to_string_lossy())))
                };
                *lock(&slots.drag) = Some((Carried::Sound(row), found));
            });
        });
        panel.set_orders(ModelRc::new(slint::VecModel::from(
            ORDERS.iter().map(|o| SharedString::from(o.name())).collect::<Vec<_>>(),
        )));
        panel.set_tempo_low_end(TEMPO_LOW);
        panel.set_tempo_high_end(TEMPO_HIGH);
        panel.set_tempo_step(TEMPO_STEP);
        // Each change to what's asked: the panel shows the change at once, the list follows from a background thread.
        let changed = {
            let lines = Rc::clone(&self.lines);
            let slots = Arc::clone(&self.slots);
            let asked = Rc::clone(&self.asked);
            let column = Rc::clone(&self.column);
            let weak = panel.as_weak();
            Rc::new(move |change: &dyn Fn(&mut Asked, Option<&Library>)| {
                let library = lines.library.borrow().clone();
                change(&mut asked.borrow_mut(), library.as_deref());
                let asked = asked.borrow().clone();
                if let Some(panel) = weak.upgrade() {
                    column.show(&panel, library.as_deref(), &asked);
                }
                if let Some(library) = library {
                    view(library, asked, Arc::clone(&slots));
                }
            })
        };
        let change = Rc::clone(&changed);
        panel.on_search(move |typed| change(&|a, _| a.filters.search = typed.trim().to_owned()));
        let change = Rc::clone(&changed);
        panel.on_pick(move |filter, choice| change(&|a, _| a.filters.pick(&filter, &choice)));
        let change = Rc::clone(&changed);
        let shown = Rc::clone(&self.column);
        panel.on_clear(move |filter| {
            change(&|a, library| match filter.as_str() {
                // The More types menu's Any keeps the types ticked in the column itself.
                "more-types" => mo_app::sounds::keep_first_types(
                    &mut a.filters.sound_types,
                    library.map_or(&[][..], |l| &l.types[..]),
                    shown.type_lines.get().max(TYPE_CHIPS),
                ),
                filter => a.filters.clear_chip(filter),
            });
        });
        let change = Rc::clone(&changed);
        panel.on_toggle(move |line| change(&|a, _| a.filters.toggle(&line)));
        let change = Rc::clone(&changed);
        panel.on_tempo_picked(move |low, high| change(&|a, _| a.filters.set_tempo(low, high)));
        // The year strip: places on it dragged over become the years picked.
        let change = Rc::clone(&changed);
        panel.on_years_picked(move |a, b| {
            change(&|asked, library| {
                let years = library.map_or(&[][..], |l| &l.years[..]);
                asked.filters.years = mo_app::year_strip::year_span(years, a.min(b), a.max(b));
            });
        });
        let change = Rc::clone(&changed);
        panel.on_years_cleared(move || change(&|asked, _| asked.filters.years = None));
        let change = changed;
        panel.on_pick_order(move |i| {
            let Some(&order) = usize::try_from(i).ok().and_then(|i| ORDERS.get(i)) else { return };
            change(&|a, _| {
                // Shuffled picked again shuffles again.
                if order == Order::Shuffled {
                    a.seed = seed();
                }
                a.order = order;
            });
        });
        self.column.show(panel, None, &self.asked.borrow());

        let instance = Arc::clone(&self.instance);
        let weak = panel.as_weak();
        panel.on_copy(move || {
            let said = match copy_text(crate::check::as_text(&instance.host, &instance.offered)) {
                Ok(()) => "Copied".to_owned(),
                Err(e) => format!("Couldn't copy: {e}"),
            };
            if let Some(panel) = weak.upgrade() {
                panel.set_copied(said.into());
            }
        });

        self.fill_in_sky();
        self.midi.fill_in(panel, &self.slots);
        self.fill_in_midi_play();
        let slots = Arc::clone(&self.slots);
        background("mo-plugin-library", move || *lock(&slots.library) = Some(library::read()));
    }

    /// A click on a MIDI part plays it as a loop with the app's SoundFont, at the DAW's tempo, through the track; a
    /// click on the one playing stops it (the same rule as a sound's line, which has no waveform here).
    fn fill_in_midi_play(&self) {
        let player = Arc::clone(&self.instance.player);
        let slots = Arc::clone(&self.slots);
        let (right_now, heard, stop_it) = (Rc::clone(&self.right_now), Rc::clone(&self.heard), self.stopper());
        let midi = self.midi.part_finder();
        self.panel.on_midi_play(move |row| {
            let Some((parts, at)) = usize::try_from(row).ok().and_then(&midi) else { return };
            let playing = heard.get().is_some_and(|(what, _)| what == Heard::Part(at));
            match line_click(playing, right_now.get(), -1.0) {
                Click::Play(_) => {}
                Click::Stop => return stop_it(),
                Click::Nothing => return,
            }
            let Some(name) = parts.entry(at).map(mo_app::midi::title) else { return };
            heard.set(Some((Heard::Part(at), false)));
            let tempo = player.tempo();
            let make: crate::audition::Made =
                Box::new(move || parts.sound(at, crate::midi::soundfont().as_deref(), tempo));
            let slots = Arc::clone(&slots);
            player.play_made(name, make, move |played| *lock(&slots.played) = Some(played));
        });
    }

    /// What the mouse does on the Galaxy, as on the app's Sounds page: the wheel flies in and out, the right or
    /// middle button moves the map, a click plays a star and the left button held down plays the stars it passes.
    fn fill_in_sky(&self) {
        let panel = &self.panel;
        {
            let mut sky = self.sky.borrow_mut();
            sky.palette = panel.get_type_colours().iter().map(|c| [c.red(), c.green(), c.blue()]).collect();
            sky.palette.push(galaxy::GREY);
        }
        let sky = Rc::clone(&self.sky);
        panel.on_sky_size(move |width, height| sky.borrow_mut().resize(width, height));
        let sky = Rc::clone(&self.sky);
        panel.on_sky_home(move || {
            let mut sky = sky.borrow_mut();
            sky.view.fly_home();
            sky.moved();
        });
        let sky = Rc::clone(&self.sky);
        panel.on_sky_wheel(move |notches| {
            let mut sky = sky.borrow_mut();
            sky.view.fly_at_mouse(notches);
            sky.moved();
        });
        // The press going on: whether it was taken away (not let go), and the star a sweep last played.
        let cancelled = Rc::new(Cell::new(false));
        let swept: Rc<Cell<Option<usize>>> = Rc::default();
        let play = {
            let lines = Rc::clone(&self.lines);
            let player = Arc::clone(&self.instance.player);
            let slots = Arc::clone(&self.slots);
            let (last_played, heard) = (Rc::clone(&self.played), Rc::clone(&self.heard));
            Rc::new(move |at: usize| {
                let Some(path) = lines.path_at(at) else { return };
                heard.set(Some((Heard::Star(at), false)));
                // No line of the list was clicked: a waveform heard next isn't any line's.
                *last_played.borrow_mut() = None;
                let slots = Arc::clone(&slots);
                player.play(path.into(), 0.0, move |played| *lock(&slots.played) = Some(played));
            })
        };
        let (sky, weak) = (Rc::clone(&self.sky), panel.as_weak());
        let (was_cancelled, last) = (Rc::clone(&cancelled), Rc::clone(&swept));
        panel.on_sky_down(move |x, y, pan| {
            let mut sky = sky.borrow_mut();
            // A second button pressed while one is held changes nothing: the first press carries on.
            if sky.view.pressed() {
                return;
            }
            sky.view.press(x, y, if pan { Hold::Pan } else { Hold::Sweep });
            // While a button is held the ring round the star under the mouse stays away.
            sky.hover = None;
            was_cancelled.set(false);
            last.set(None);
            if let Some(panel) = weak.upgrade() {
                panel.set_sky_grabbing(pan);
                panel.set_sky_hover(SharedString::new());
            }
        });
        let (sky, weak, lines) = (Rc::clone(&self.sky), panel.as_weak(), Rc::clone(&self.lines));
        let (last, play_star) = (Rc::clone(&swept), Rc::clone(&play));
        panel.on_sky_move(move |x, y| {
            let Some(panel) = weak.upgrade() else { return };
            let mut sky = sky.borrow_mut();
            match sky.view.moved(x, y) {
                Moved::Still => {}
                Moved::Panned => sky.moved(),
                Moved::Swept(from_x, from_y) => {
                    // The newest star passed plays, unless it's the one playing already.
                    if let Some(&at) = sky.passed((from_x, from_y), (x, y)).last()
                        && last.replace(Some(at)) != Some(at)
                    {
                        play_star(at);
                        sky.picked = Some(at);
                        sky.on_trail(at, true);
                    }
                }
                Moved::Hover => {
                    // Someone's looking: the stars keep twinkling.
                    sky.looked_at(Instant::now());
                    sky.hover = sky.star_at(x, y);
                    let name = sky.hover.and_then(|at| lines.path_at(at));
                    panel.set_sky_hover(name.as_deref().map(library::name).unwrap_or_default().into());
                    panel.set_sky_hover_x(x);
                    panel.set_sky_hover_y(y);
                }
            }
        });
        let was_cancelled = Rc::clone(&cancelled);
        panel.on_sky_cancelled(move || was_cancelled.set(true));
        let (sky, weak) = (Rc::clone(&self.sky), panel.as_weak());
        let player = Arc::clone(&self.instance.player);
        let (heard, stop) = (Rc::clone(&self.heard), self.stopper());
        let arrow_play = Rc::clone(&play);
        panel.on_sky_up(move |side| {
            let mut sky = sky.borrow_mut();
            let cancelled = cancelled.get();
            // Only the button that started the press ends it (a press taken away ends whichever it was).
            match sky.view.holding() {
                Some(held) if cancelled || held == if side { Hold::Pan } else { Hold::Sweep } => {}
                _ => return,
            }
            let (was_swept, panned) = (sky.view.sweeping() && swept.take().is_some(), sky.view.panning());
            let click = sky.view.release(cancelled);
            if let Some(panel) = weak.upgrade() {
                panel.set_sky_grabbing(false);
            }
            // The drag drew quick pictures: the sharp one now.
            if panned {
                sky.moved();
            }
            // Let go after sweeping: the sound stops with the mouse, as in the app.
            if was_swept {
                player.stop();
                heard.set(None);
            }
            if let Some(at) = click.and_then(|(x, y)| sky.star_at(x, y)) {
                // A click on the star that's playing stops it (it stays ringed).
                if heard.get().is_some_and(|(what, _)| what == Heard::Star(at)) {
                    stop();
                } else {
                    play(at);
                }
                sky.picked = Some(at);
                sky.on_trail(at, false);
            }
        });
        // Light by: what makes a star bigger and brighter, as the app's chip on the map.
        let lights = Choices { chips: Rc::default(), more: Rc::default() };
        let light_names = galaxy::LIGHTS.map(mo_app::galaxy_look::LightBy::name);
        panel.set_sky_light(lights.view(&mo_app::filters::one_of(&light_names, self.sky.borrow().light_by().name())));
        let (sky, weak) = (Rc::clone(&self.sky), panel.as_weak());
        panel.on_sky_light_by(move |name| {
            let Some(by) = galaxy::LIGHTS.into_iter().find(|by| by.name() == name.as_str()) else { return };
            sky.borrow_mut().set_light_by(by);
            if let Some(panel) = weak.upgrade() {
                panel.set_sky_light(lights.view(&mo_app::filters::one_of(&light_names, by.name())));
            }
        });
        let sky = Rc::clone(&self.sky);
        panel.on_sky_mini_go(move |x, y| sky.borrow_mut().mini_go(x, y));
        // An arrow key goes to the nearest star that way and plays it, as a click on it would.
        // The Swept card: a click on a line, Back and Next go to that sound on the map and play it.
        let (sky, play_star, right) = (Rc::clone(&self.sky), Rc::clone(&arrow_play), Rc::clone(&self.right_now));
        panel.on_sky_swept_go(move |line| {
            // Only the left button: a line takes the right one as a click too.
            if right.get() {
                return;
            }
            let found = usize::try_from(line).ok().and_then(|line| sky.borrow_mut().go_on_trail(line));
            if let Some(at) = found {
                play_star(at);
            }
        });
        let (sky, play_star) = (Rc::clone(&self.sky), Rc::clone(&arrow_play));
        panel.on_sky_swept_step(move |by| {
            let found = sky.borrow_mut().step_on_trail(by);
            if let Some(at) = found {
                play_star(at);
            }
        });
        let (sky, weak) = (Rc::clone(&self.sky), panel.as_weak());
        panel.on_sky_swept_clear(move || {
            sky.borrow_mut().clear_trail();
            if let Some(panel) = weak.upgrade() {
                panel.set_sky_swept_keys(false);
            }
        });
        let (sky, play_star, weak) = (Rc::clone(&self.sky), arrow_play, panel.as_weak());
        panel.on_sky_arrow(move |way| {
            let Some(panel) = weak.upgrade() else { return };
            // With the Swept card clicked into, Up and Down go through its sounds (as Back and Next).
            if panel.get_sky_swept_keys() && way.abs() == 2 && !sky.borrow().trail().0.is_empty() {
                let found = sky.borrow_mut().step_on_trail(way.signum());
                if let Some(at) = found {
                    play_star(at);
                }
                return;
            }
            panel.set_sky_swept_keys(false);
            let way = match way {
                -1 => (-1.0, 0.0),
                1 => (1.0, 0.0),
                -2 => (0.0, -1.0),
                2 => (0.0, 1.0),
                _ => return,
            };
            let found = sky.borrow_mut().arrow(way);
            if let Some(at) = found {
                sky.borrow_mut().on_trail(at, false);
                play_star(at);
            }
        });
        let sky = Rc::clone(&self.sky);
        panel.on_sky_name(move |region| {
            if let Ok(region) = usize::try_from(region) {
                sky.borrow_mut().fly_into_name(region);
            }
        });
        let (sky, weak) = (Rc::clone(&self.sky), panel.as_weak());
        panel.on_sky_gone(move || {
            sky.borrow_mut().hover = None;
            if let Some(panel) = weak.upgrade() {
                panel.set_sky_hover(SharedString::new());
            }
        });
    }

    /// The Galaxy while it's on show: its map is read the first time, its glide goes on, and its grid and pictures
    /// are made on background threads and taken in.
    fn sky(&self) {
        let since = self.ticked.replace(Instant::now()).elapsed().min(LONGEST_STEP);
        if !self.panel.get_galaxy() {
            // With no map yet (the app hadn't laid one out), it's looked for again the next time the Galaxy is opened.
            if !self.sky.borrow().has_map() {
                self.sky_asked.set(false);
            }
            return;
        }
        if !self.sky_asked.get()
            && let Some(library) = self.lines.library.borrow().clone()
        {
            self.sky_asked.set(true);
            self.panel.set_sky_note("Reading the galaxy".into());
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-galaxy", move || *lock(&slots.map) = Some(galaxy::read(&library)));
        }
        let mut sky = self.sky.borrow_mut();
        match take(&self.slots.map) {
            Some(Ok(map)) => {
                sky.set_map(map);
                self.panel.set_sky_note(SharedString::new());
            }
            Some(Err(why)) => {
                crate::log::line("Galaxy", &why);
                self.panel.set_sky_note(why.into());
            }
            None => {}
        }
        if let Some((set, grid)) = take(&self.slots.grid) {
            sky.set_grid(set, grid);
        }
        sky.set_scale(self.scale.get() as f32);
        // A glide draws as it goes, and once more (sharp) when it arrives.
        if sky.view.flying() {
            sky.view.glide(since);
            sky.moved();
        }
        if let Some((set, stars, shown)) = sky.grid_ask() {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-galaxy-grid", move || {
                let started = Instant::now();
                let grid = Grid::new(&stars, &shown);
                crate::log::took_if_slow("Galaxy", &format!("Made the grid of {} stars", grid.len()), started);
                *lock(&slots.grid) = Some((set, grid));
            });
        }
        let packed = self.sky_stars(&mut sky);
        let on_card = match &self.draws {
            Draws::Card(gpu) if !self.painter_failed.get() => self.sky_on_card(&mut sky, gpu, packed),
            _ => false,
        };
        if !on_card {
            self.sky_on_processor(&mut sky);
        }
        self.sky_names(&mut sky);
        self.sky_mini(&mut sky);
        self.sky_trail(&sky);
        if let Some((x, y, width, height)) = sky.place() {
            self.panel.set_sky_x(x);
            self.panel.set_sky_y(y);
            self.panel.set_sky_w(width);
            self.panel.set_sky_h(height);
        }
    }

    /// What's over the Galaxy's picture: the names of its neighbourhoods (found on a background thread when the stars
    /// on show change) and the rings round the star under the mouse and the star last played.
    fn sky_names(&self, sky: &mut Sky) {
        if let Some(naming) = sky.names_ask() {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-galaxy-names", move || {
                let started = Instant::now();
                let found = naming.find();
                crate::log::took_if_slow("Galaxy", &format!("Named {} neighbourhoods", found.len()), started);
                *lock(&slots.names) = Some((naming.set, found));
            });
        }
        if let Some((set, regions)) = take(&self.slots.names) {
            sky.set_regions(set, regions);
        }
        let names: Vec<SkyName> = sky
            .names()
            .into_iter()
            .map(|l| SkyName {
                region: i32::try_from(l.region).unwrap_or(-1),
                name: l.name.to_uppercase().into(),
                x: l.x,
                y: l.y,
                colour: l.colour,
                fade: l.fade,
            })
            .collect();
        // Only when they've changed: the panel isn't told to draw the same names again every tick.
        if *self.sky_names.borrow() != names {
            self.panel.set_sky_names(ModelRc::new(VecModel::from(names.clone())));
            *self.sky_names.borrow_mut() = names;
        }
        let hover = sky.hover.and_then(|at| sky.where_is(at));
        self.panel.set_sky_ring(hover.is_some());
        if let Some((x, y)) = hover {
            self.panel.set_sky_ring_x(x);
            self.panel.set_sky_ring_y(y);
        }
        let picked = sky.picked.and_then(|at| sky.where_is(at));
        self.panel.set_sky_picked(picked.is_some());
        if let Some((x, y)) = picked {
            self.panel.set_sky_picked_x(x);
            self.panel.set_sky_picked_y(y);
        }
    }

    /// The trail: the line through the stars heard lately, their dots, and the Swept card. The panel is only told
    /// when they've changed.
    fn sky_trail(&self, sky: &Sky) {
        let (path, dots) = sky.trail_on_view();
        let (trail, on) = sky.trail();
        let now: Trail = (path, dots, trail.to_vec(), on);
        if *self.sky_trail.borrow() == now {
            return;
        }
        self.panel.set_sky_trail(now.0.as_str().into());
        let dots: Vec<TrailDot> = now.1.iter().map(|&(x, y, fade)| TrailDot { x, y, fade }).collect();
        self.panel.set_sky_trail_dots(ModelRc::new(VecModel::from(dots)));
        let lines: Vec<SweptLine> =
            trail.iter().enumerate().filter_map(|(i, &at)| self.lines.swept(at, i == on)).collect();
        self.panel.set_sky_swept(ModelRc::new(VecModel::from(lines)));
        *self.sky_trail.borrow_mut() = now;
    }

    /// The mini-map: its picture of the whole map is drawn on a background thread when the stars on show change, and
    /// its box follows the view.
    fn sky_mini(&self, sky: &mut Sky) {
        if let Some(picture) = sky.mini_ask() {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-galaxy-mini", move || {
                let (set, started) = (picture.id, Instant::now());
                // A fault in the drawing is noted: the mini-map keeps the picture it had.
                let drawn = mo_catch::catch_crash(AssertUnwindSafe(|| sky_pixels(&picture.draw())));
                crate::log::took_if_slow("Galaxy", "Drew the mini-map", started);
                if let Some(pixels) = drawn.ok().flatten() {
                    *lock(&slots.mini) = Some((set, pixels));
                }
            });
        }
        if let Some((set, pixels)) = take(&self.slots.mini)
            && sky.mini_took(set)
        {
            self.panel.set_sky_mini_picture(slint::Image::from_rgba8_premultiplied(pixels));
        }
        let mini = sky.mini();
        self.panel.set_sky_mini(mini.is_some());
        if let Some(mini) = mini {
            self.panel.set_sky_mini_title(mini.title.into());
            self.panel.set_sky_mini_x(mini.x);
            self.panel.set_sky_mini_y(mini.y);
            self.panel.set_sky_mini_w(mini.width);
            self.panel.set_sky_mini_h(mini.height);
        }
    }

    /// The Galaxy's stars made ready, lit by what Light by says, on a background thread when the map, the filters
    /// or Light by change: the ones to show now, when new ones have come.
    fn sky_stars(&self, sky: &mut Sky) -> Option<galaxy::Packed> {
        if let Some(library) = self.lines.library.borrow().as_ref()
            && let Some(pack) = sky.pack_ask(library)
        {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-galaxy-stars", move || {
                let started = Instant::now();
                let packed = pack.pack();
                crate::log::took_if_slow("Galaxy", "Made the stars ready and lit", started);
                *lock(&slots.packed) = Some(packed);
            });
        }
        take(&self.slots.packed).filter(|packed| sky.packed(packed))
    }

    /// The Galaxy's stars painted by the graphics card, with the app's own painter: every star goes to the card once
    /// (when the map or the filters change), and after that each picture only sends the view, so flying and moving
    /// the map keep the screen's pace with half a million stars. `false` when the card can't (the processor's
    /// picture is drawn instead).
    fn sky_on_card(&self, sky: &mut Sky, gpu: &Gpu, packed: Option<galaxy::Packed>) -> bool {
        let (device, queue) = gpu.device();
        let well = self.panel.get_sky_well();
        let background = [well.red(), well.green(), well.blue()].map(|c| f32::from(c) / 255.0);
        let mut painter = self.painter.borrow_mut();
        if painter.is_none() && packed.is_none() {
            // No stars on the card yet: nothing is shown until they are (they're a moment away).
            return true;
        }
        // A fault in the card's own code is caught here, so it never reaches the DAW.
        let painted = mo_catch::catch_crash(AssertUnwindSafe(|| {
            let painter = painter.get_or_insert_with(|| {
                let started = Instant::now();
                let painter = mo_sky::Sky::new(device, queue);
                crate::log::took("Galaxy", "Set up the stars' painter on the graphics card", started);
                painter
            });
            if let Some(packed) = &packed {
                painter.set_stars(&packed.bytes, packed.lit);
            }
            if !painter.has_stars() {
                return Ok(None);
            }
            let Some((view, scale, look, shine)) = sky.on_card(Instant::now()) else { return Ok(None) };
            let texture = painter.draw(&view, scale, &look, shine, background);
            slint::Image::try_from(texture).map(Some).map_err(|e| format!("Slint wouldn't show the picture: {e}"))
        }));
        match painted {
            Ok(Ok(Some(picture))) => self.panel.set_sky_picture(picture),
            Ok(Ok(None)) => {}
            Ok(Err(why)) => {
                crate::log::line("Galaxy", &format!("{why}: the stars are drawn on the processor"));
                self.give_up_the_painter(sky, &mut painter);
            }
            Err(_) => {
                crate::log::line("Galaxy", "Painting the stars on the card crashed: they're drawn on the processor");
                self.give_up_the_painter(sky, &mut painter);
            }
        }
        !self.painter_failed.get()
    }

    fn give_up_the_painter(&self, sky: &mut Sky, painter: &mut Option<mo_sky::Sky>) {
        *painter = None;
        self.painter_failed.set(true);
        sky.moved();
    }

    /// The Galaxy's picture drawn on the processor, on a background thread (no graphics card to draw with).
    fn sky_on_processor(&self, sky: &mut Sky) {
        match take(&self.slots.sky) {
            Some(Ok((id, view, pixels))) => {
                if sky.took(id, view) {
                    self.panel.set_sky_picture(slint::Image::from_rgba8_premultiplied(pixels));
                }
            }
            Some(Err(id)) => sky.gave_up(id),
            None => {}
        }
        if let Some(picture) = sky.picture() {
            let slots = Arc::clone(&self.slots);
            background("mo-plugin-galaxy-picture", move || {
                let (id, started) = (picture.id, Instant::now());
                // A fault in the drawing is noted, and the next picture can still be asked for.
                let drawn = mo_catch::catch_crash(AssertUnwindSafe(|| {
                    let drawn = picture.draw();
                    Some((drawn.id, drawn.view, sky_pixels(&drawn)?))
                }));
                crate::log::took_if_slow("Galaxy", "Drew the stars", started);
                *lock(&slots.sky) = Some(drawn.ok().flatten().ok_or(id));
            });
        }
    }

    /// Takes in what background threads found, frees sounds done playing, and runs Slint's timers.
    fn tick(&self) {
        if let Some(read) = take(&self.slots.library) {
            match read {
                Read::Ready(library) => {
                    // The list comes with its first view, so it never shows in an order nobody asked for.
                    self.lines.show(Some(Arc::clone(&library)), Arc::default());
                    self.panel.set_status("Getting the list ready".into());
                    self.column.show(&self.panel, Some(&library), &self.asked.borrow());
                    view(library, self.asked.borrow().clone(), Arc::clone(&self.slots));
                }
                Read::Not(why) => self.panel.set_status(why.into()),
            }
        }
        if let Some((number, found, counts)) = take(&self.slots.found)
            && number == self.slots.searches.load(Ordering::Acquire)
        {
            let library = self.lines.library.borrow().clone();
            let total = library.as_ref().map_or(0, |l| l.list.len());
            let narrowed = self.asked.borrow().any();
            self.panel.set_status(count_line(total, narrowed.then_some(found.len())).into());
            self.panel.set_picked(-1);
            *self.column.counts.borrow_mut() = Some(counts);
            self.column.show(&self.panel, library.as_deref(), &self.asked.borrow());
            // The Galaxy shows the same sounds as the list.
            let found = Arc::new(found);
            self.sky.borrow_mut().set_shown(Arc::clone(&found));
            self.lines.show(library, found);
        }
        if let Some(played) = take(&self.slots.played) {
            let line = match played {
                Played::Playing(name, n) => {
                    self.playing.set(Some(n));
                    // What was clicked is what's heard now.
                    self.heard.set(self.heard.get().map(|(what, _)| (what, true)));
                    format!("Playing {name}")
                }
                Played::Not(why) => {
                    self.playing.set(None);
                    self.heard.set(None);
                    why
                }
            };
            self.panel.set_playing(line.into());
        }
        if let Some(n) = self.playing.get()
            && !self.instance.player.still_playing(n)
        {
            self.playing.set(None);
            // It ended: a click on it plays it again. (Not while a newer pick is still being read.)
            if self.heard.get().is_some_and(|(_, started)| started) {
                self.heard.set(None);
            }
            self.panel.set_playing(SharedString::new());
        }
        // The line on the waveform of the line clicked, while that sound plays and the line still shows it.
        let line = self.playing.get().and_then(|n| {
            let at = self.instance.player.place(n)?;
            let played = self.played.borrow();
            let (row, sound, _) = played.as_ref()?;
            (self.lines.sound(*row) == Some(*sound)).then_some((i32::try_from(*row).ok()?, at))
        });
        match line {
            Some((row, at)) => {
                self.panel.set_playing_at(at);
                self.panel.set_playing_row(row);
            }
            None => self.panel.set_playing_row(-1),
        }
        self.instance.player.tidy();
        // The window is taller or shorter: the column shows as many types as it now has room for.
        if usize::try_from(self.panel.get_type_lines()).is_ok_and(|lines| lines != self.column.type_lines.get()) {
            let library = self.lines.library.borrow().clone();
            self.column.show(&self.panel, library.as_deref(), &self.asked.borrow());
        }
        // The window is wider or narrower: the waveforms are too, so their pictures are made again.
        let wave_width = self.panel.get_wave_width();
        if (wave_width - self.wave_width.get()).abs() > 0.5 {
            self.wave_width.set(wave_width);
            self.lines.waves.borrow_mut().forget();
            self.lines.notify.reset();
        }
        self.midi.tick(&self.panel);
        self.drag();
        self.waves();
        self.sky();
        self.render();
    }

    /// Carries a dragged line's file out, once it's been found: the system's drag holds this thread until it's let
    /// go (`drag.rs`; the timer still runs meanwhile, and finds `dragging` set), and its end comes back in a slot
    /// ([`Shown::dragged`]).
    fn drag(&self) {
        self.dragged();
        if self.dragging.get() {
            return;
        }
        let Some((carried, found)) = take(&self.slots.drag) else { return };
        self.carried.set(Some(carried));
        match found {
            Ok(path) => {
                self.dragging.set(true);
                match carried {
                    Carried::Sound(row) => self.panel.set_carried(i32::try_from(row).unwrap_or(-1)),
                    Carried::Part(at) => self.panel.set_midi_carried(self.midi.row_of(at)),
                }
                let (slots, started) = (Arc::clone(&self.slots), Instant::now());
                crate::drag::file(path.clone(), move |ended| *lock(&slots.dragged) = Some((path, started, ended)));
            }
            Err(why) => {
                crate::log::line("Drag", &why);
                self.say_drag_failed(why);
                self.carried.set(None);
            }
        }
    }

    /// A drag out has ended: the line is let go, and the panel says so if it couldn't be carried.
    fn dragged(&self) {
        let Some((path, started, ended)) = take(&self.slots.dragged) else { return };
        let how = match &ended {
            Ok(crate::drag::Outcome::Landed) => "landed".to_owned(),
            Ok(crate::drag::Outcome::Cancelled) => "let go where nothing took it, or called off".to_owned(),
            Err(why) => format!("failed: {why}"),
        };
        crate::log::took("Drag", &format!("{} {how}", path.display()), started);
        self.dragging.set(false);
        self.panel.set_carried(-1);
        self.panel.set_midi_carried(-1);
        // The system's drag kept the button's coming up for itself: the panel is told, so the line isn't left
        // pressed.
        let position = self.pointer.get();
        let event = slint::platform::WindowEvent::PointerReleased { position, button: PointerEventButton::Left };
        self.draws.window().dispatch_event(event);
        self.draws.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
        if let Err(why) = ended {
            self.say_drag_failed(why);
        }
        self.carried.set(None);
    }

    /// Said under the list the line was dragged from.
    fn say_drag_failed(&self, why: String) {
        if let Some(Carried::Part(_)) = self.carried.get() {
            self.panel.set_midi_said(why.into());
            return;
        }
        self.playing.set(None);
        self.panel.set_playing(why.into());
    }

    /// A waveform picture's size in the screen's own pixels: as wide as the panel now draws them.
    fn wave_size(&self) -> (u32, u32) {
        let scale = self.scale.get().clamp(0.5, 8.0);
        let width = f64::from(self.wave_width.get()).clamp(waves::WIDTH, MOST_WIDTH);
        ((width * scale).round() as u32, (waves::HEIGHT * scale).round() as u32)
    }

    /// Takes in the waveforms a background thread read, and has the ones the lines on show still lack read next.
    fn waves(&self) {
        if let Some((round, found)) = take(&self.slots.waves) {
            let found = found.into_iter().map(|(id, pixels)| (id, pixels.map(slint::Image::from_rgba8))).collect();
            let rows = self.lines.waves.borrow_mut().take_in(round, found);
            for (row, sound) in rows {
                // The list may have changed since the line asked.
                if self.lines.sound(row) == Some(sound) {
                    self.lines.notify.row_changed(row);
                }
            }
        }
        // A sound just played that the library has no waveform for gets the one made from its audio.
        if let Some((path, peaks)) = take(&self.instance.player.heard)
            && let Some((row, sound, played)) = self.played.borrow().clone()
            && played == path
            && self.lines.waves.borrow().has_none(sound)
        {
            let (width, height) = self.wave_size();
            if let Some(pixels) = waves::picture(&peaks, width, height) {
                self.lines.waves.borrow_mut().give(sound, slint::Image::from_rgba8(pixels));
                if self.lines.sound(row) == Some(sound) {
                    self.lines.notify.row_changed(row);
                }
            }
        }
        let Some((round, ids)) = self.lines.waves.borrow_mut().to_read() else { return };
        let (width, height) = self.wave_size();
        let slots = Arc::clone(&self.slots);
        background("mo-plugin-waves", move || {
            let found = library::waves(&ids);
            let pictures = found.iter().map(|(id, peaks)| (*id, waves::picture(peaks, width, height))).collect();
            *lock(&slots.waves) = Some((round, pictures));
        });
    }

    /// Slint draws the panel again if anything on it changed, and the window is asked to show it.
    fn render(&self) {
        slint::platform::update_timers_and_animations();
        let (width, _) = self.size.get();
        if self.draws.draw_if_needed(width) {
            self.context.request_redraw();
        }
    }

    fn resize_to(&self, size: WindowSize) {
        let (width, height) = (size.physical.width.max(1), size.physical.height.max(1));
        self.size.set((width, height));
        if (size.scale_factor - self.scale.get()).abs() > f64::EPSILON || !self.draws.sized() {
            self.scale.set(size.scale_factor);
            // Pictures made for the old scaling are the wrong size: the lines ask again.
            self.lines.waves.borrow_mut().forget();
            self.lines.notify.reset();
            self.draws.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: size.scale_factor as f32,
            });
        }
        self.draws.resize(width, height);
        self.render();
    }

    fn send(&self, event: slint::platform::WindowEvent) {
        self.draws.window().dispatch_event(event);
        if !self.draws.waits_for_the_screen() {
            self.render();
        }
    }

    /// Stops what's playing, from wherever in the panel it's asked.
    fn stopper(&self) -> Rc<dyn Fn()> {
        let (player, heard) = (Arc::clone(&self.instance.player), Rc::clone(&self.heard));
        let weak = self.panel.as_weak();
        Rc::new(move || {
            player.stop();
            heard.set(None);
            if let Some(panel) = weak.upgrade() {
                panel.set_playing(SharedString::new());
            }
        })
    }

    /// Stops what's playing (Esc).
    fn stop(&self) {
        self.instance.player.stop();
        self.heard.set(None);
        self.playing.set(None);
        self.panel.set_playing(SharedString::new());
    }

    fn at(&self, position: PhysicalPosition<f64>) -> LogicalPosition {
        let scale = self.scale.get().max(0.1);
        let at = LogicalPosition::new((position.x / scale) as f32, (position.y / scale) as f32);
        self.pointer.set(at);
        at
    }
}

impl WindowHandler for Shown {
    fn draw(&self) -> Result<(), HandlerError> {
        let (width, height) = self.size.get();
        self.draws.show(width, height)
    }

    fn resized(&self, new_size: WindowSize) -> Result<(), HandlerError> {
        self.resize_to(new_size);
        Ok(())
    }

    fn on_timer(&self, _timer: &TimerHandle) {
        self.tick();
    }

    fn on_event(&self, event: Event) -> EventStatus {
        use slint::platform::WindowEvent as Slint;
        match event {
            Event::Mouse(mouse) => match mouse {
                MouseEvent::CursorMoved { position, .. } => {
                    let position = self.at(position);
                    self.send(Slint::PointerMoved { position });
                }
                MouseEvent::ButtonPressed { button, .. } => {
                    // The keys come to the panel once it's clicked (for the search box).
                    if !self.context.has_focus() {
                        let _ = self.context.focus();
                    }
                    self.right_now.set(button == MouseButton::Right);
                    self.send(Slint::PointerPressed { position: self.pointer.get(), button: slint_button(button) });
                    self.right_now.set(false);
                }
                MouseEvent::ButtonReleased { button, .. } => {
                    self.right_now.set(button == MouseButton::Right);
                    self.send(Slint::PointerReleased { position: self.pointer.get(), button: slint_button(button) });
                    self.right_now.set(false);
                }
                MouseEvent::WheelScrolled { delta, .. } => {
                    let (delta_x, delta_y) = match delta {
                        // As Slint's own windows turn a wheel's lines into points.
                        ScrollDelta::Lines { x, y } => (x * 60.0, y * 60.0),
                        ScrollDelta::Pixels { x, y } => {
                            let scale = self.scale.get().max(0.1) as f32;
                            (x / scale, y / scale)
                        }
                    };
                    self.send(Slint::PointerScrolled { position: self.pointer.get(), delta_x, delta_y });
                }
                MouseEvent::CursorLeft => self.send(Slint::PointerExited),
                _ => return EventStatus::Ignored,
            },
            Event::Keyboard(key) => {
                // Esc stops the sound.
                if key.state == KeyState::Down && !key.repeat && key.key == Key::Named(NamedKey::Escape) {
                    self.stop();
                }
                let Some(text) = key_text(&key.key) else { return EventStatus::Ignored };
                self.send(match (key.state, key.repeat) {
                    (KeyState::Down, false) => Slint::KeyPressed { text },
                    (KeyState::Down, true) => Slint::KeyPressRepeated { text },
                    (KeyState::Up, _) => Slint::KeyReleased { text },
                });
            }
            Event::Window(WindowEvent::Focused) => self.send(Slint::WindowActiveChanged(true)),
            Event::Window(WindowEvent::Unfocused) => self.send(Slint::WindowActiveChanged(false)),
            _ => return EventStatus::Ignored,
        }
        EventStatus::Captured
    }
}

/// The list's top line: "1,234 sounds", or "12 of 1,234 sounds" when searched.
fn count_line(total: usize, found: Option<usize>) -> String {
    let sounds = if total == 1 { "sound" } else { "sounds" };
    match found {
        Some(found) => format!("{} of {} {sounds}", thousands(found), thousands(total)),
        None => format!("{} {sounds}", thousands(total)),
    }
}

/// Works out what `asked` shows of `library` on a background thread; it lands in `slots.found` with this asking's
/// number, and only the newest asking's answer is shown.
fn view(library: Arc<Library>, asked: Asked, slots: Arc<Slots>) {
    let number = slots.searches.fetch_add(1, Ordering::AcqRel) + 1;
    background("mo-plugin-view", move || {
        // Newer words were typed while this one waited for its thread: theirs is on its way.
        if slots.searches.load(Ordering::Acquire) != number {
            return;
        }
        let started = Instant::now();
        let (shown, counts) = library.view(&asked);
        crate::log::took_if_slow("List", &format!("Worked out a list of {} sounds", shown.len()), started);
        *lock(&slots.found) = Some((number, shown, counts));
    });
}

/// The filter column's lines, kept from one showing to the next, and the counts of the list last worked out (the
/// column shows them until the next list's are in).
#[derive(Default)]
struct Column {
    types: Choices,
    length: Choices,
    key: Choices,
    tag: Choices,
    counts: RefCell<Option<SoundCounts>>,
    /// How many types the column last showed as lines of their own (0 before the first showing).
    type_lines: Cell<usize>,
    /// How many years the year strip was last given (the library's years don't change once it's read).
    years: Cell<usize>,
}

impl Column {
    /// The panel's filter column and order as `asked` has them, over `library`'s types and tags once it's read.
    fn show(&self, panel: &Panel, library: Option<&Library>, asked: &Asked) {
        let type_lines = usize::try_from(panel.get_type_lines()).unwrap_or(0).max(TYPE_CHIPS);
        self.type_lines.set(type_lines);
        let chips = asked.chips(library, self.counts.borrow().as_ref(), type_lines);
        panel.set_type_filter(self.types.view(&chips.types));
        panel.set_length_filter(self.length.view(&chips.length));
        panel.set_key_filter(self.key.view(&chips.key));
        panel.set_tag_filter(self.tag.view(&chips.tag));
        panel.set_mix_well(asked.filters.mix_well);
        panel.set_favourites(asked.filters.favourites);
        panel.set_tempo_low(chips.tempo.low);
        panel.set_tempo_high(chips.tempo.high);
        panel.set_tempo_words(chips.tempo.words.as_str().into());
        panel.set_tempo_bars(ModelRc::new(VecModel::from(chips.tempo.bars)));
        // The year strip: each year's bar is its share of the fullest year.
        let years = library.map_or(&[][..], |l| &l.years[..]);
        let most = years.iter().map(|y| y.1).max().unwrap_or(1).max(1) as f32;
        if self.years.replace(years.len()) != years.len() {
            let bars: Vec<YearBar> = years
                .iter()
                .map(|&(year, n)| YearBar { year: year.to_string().into(), share: n as f32 / most })
                .collect();
            panel.set_years(ModelRc::new(VecModel::from(bars)));
        }
        let (from, to) = mo_app::year_strip::picked_places(years, asked.filters.years).unwrap_or((-1, -1));
        panel.set_years_from(from);
        panel.set_years_to(to);
        panel.set_order(ORDERS.iter().position(|&o| o == asked.order).and_then(|i| i32::try_from(i).ok()).unwrap_or(0));
    }
}

/// One filter's lines and menu lines. Only the lines that changed are changed, so an open menu keeps its place when a
/// pick changes what its lines say (as the app's own pages keep theirs, mo-ui's `FilterModels`).
#[derive(Default)]
struct Choices {
    chips: Rc<VecModel<FilterChoice>>,
    more: Rc<VecModel<FilterChoice>>,
}

impl Choices {
    fn view(&self, filter: &Filter) -> FilterView {
        same_lines(&self.chips, &filter.chips);
        same_lines(&self.more, &filter.more);
        FilterView {
            chips: ModelRc::from(Rc::clone(&self.chips)),
            more: ModelRc::from(Rc::clone(&self.more)),
            more_picked: filter.more_picked.as_str().into(),
        }
    }
}

/// Makes `model` say `list`, changing only the lines that differ (all of them when the number of lines changed).
fn same_lines(model: &VecModel<FilterChoice>, list: &[Choice]) {
    let line = |c: &Choice| FilterChoice { text: c.text.as_str().into(), count: c.count.as_str().into(), on: c.on };
    if model.row_count() != list.len() {
        model.set_vec(list.iter().map(line).collect::<Vec<_>>());
        return;
    }
    for (at, c) in list.iter().enumerate() {
        if model.row_data(at).is_none_or(|r| r != line(c)) {
            model.set_row_data(at, line(c));
        }
    }
}

/// A new shuffled order: the clock's nanoseconds.
fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

/// What's being heard: a list line's sound (by its id) or a star (by its sound's place in the list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heard {
    Line(i64),
    Star(usize),
    /// A MIDI part, by its place in the MIDI tab's catalogue.
    Part(usize),
}

/// What a click on a list line does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Click {
    /// Plays its sound from this spot (0 the start, 1 the end).
    Play(f32),
    Stop,
    Nothing,
}

/// What a click on a list line does, as on the app's Sounds page (Solomon, 11 Nov 2026): a click on the line that's
/// `playing` stops it, unless it's on its waveform, which plays from that spot (`from` is 0 to 1 there, below 0
/// anywhere else on the line); a `right` click stops it only over its waveform, and does nothing anywhere else.
fn line_click(playing: bool, right: bool, from: f32) -> Click {
    let on_waveform = from >= 0.0;
    match (right, playing, on_waveform) {
        (true, true, true) | (false, true, false) => Click::Stop,
        (true, ..) => Click::Nothing,
        (false, _, true) => Click::Play(from.min(1.0)),
        (false, false, false) => Click::Play(0.0),
    }
}

fn slint_button(button: MouseButton) -> PointerEventButton {
    match button {
        MouseButton::Left => PointerEventButton::Left,
        MouseButton::Right => PointerEventButton::Right,
        MouseButton::Middle => PointerEventButton::Middle,
        MouseButton::Back => PointerEventButton::Back,
        MouseButton::Forward => PointerEventButton::Forward,
        _ => PointerEventButton::Other,
    }
}

/// What Slint is told a key typed: the letters it gives, or Slint's own sign for a key that types none.
fn key_text(key: &Key) -> Option<SharedString> {
    use slint::platform::Key as S;
    let named = match key {
        Key::Character(text) => return Some(text.as_str().into()),
        Key::Named(named) => named,
    };
    let key = match named {
        NamedKey::Backspace => S::Backspace,
        NamedKey::Delete => S::Delete,
        NamedKey::Enter => S::Return,
        NamedKey::Tab => S::Tab,
        NamedKey::Escape => S::Escape,
        NamedKey::ArrowLeft => S::LeftArrow,
        NamedKey::ArrowRight => S::RightArrow,
        NamedKey::ArrowUp => S::UpArrow,
        NamedKey::ArrowDown => S::DownArrow,
        NamedKey::Home => S::Home,
        NamedKey::End => S::End,
        NamedKey::PageUp => S::PageUp,
        NamedKey::PageDown => S::PageDown,
        NamedKey::Shift => S::Shift,
        NamedKey::Control => S::Control,
        NamedKey::Alt => S::Alt,
        _ => return None,
    };
    Some(key.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_blend_as_premultiplied_colour() {
        let mut p = Pixel::from_rgb(0, 0, 255);
        assert_eq!(p.0, 0x0000FF);
        p.blend(PremultipliedRgbaColor { red: 255, green: 0, blue: 0, alpha: 255 });
        assert_eq!(p.0, 0xFF0000);
        let mut p = Pixel::from_rgb(0, 0, 200);
        p.blend(PremultipliedRgbaColor { red: 100, green: 0, blue: 0, alpha: 128 });
        assert_eq!(p.0, 100 << 16 | ((200 * 127 + 127) / 255));
    }

    #[test]
    fn count_lines() {
        assert_eq!(count_line(1, None), "1 sound");
        assert_eq!(count_line(1234567, None), "1,234,567 sounds");
        assert_eq!(count_line(1234, Some(12)), "12 of 1,234 sounds");
    }

    #[test]
    fn a_click_on_the_playing_line_stops_it_and_a_right_click_only_over_its_waveform() {
        // A line that isn't playing: a click plays it, from the start or from the spot on its waveform.
        assert_eq!(line_click(false, false, -1.0), Click::Play(0.0));
        assert_eq!(line_click(false, false, 0.4), Click::Play(0.4));
        // The one playing: a click stops it, a click on its waveform plays from there.
        assert_eq!(line_click(true, false, -1.0), Click::Stop);
        assert_eq!(line_click(true, false, 0.0), Click::Play(0.0));
        // A right click stops only over the playing line's waveform, and never starts anything.
        assert_eq!(line_click(true, true, 0.7), Click::Stop);
        assert_eq!(line_click(true, true, -1.0), Click::Nothing);
        assert_eq!(line_click(false, true, 0.7), Click::Nothing);
        assert_eq!(line_click(false, true, -1.0), Click::Nothing);
    }

    #[test]
    fn keys_slint_knows() {
        assert_eq!(key_text(&Key::Character("a".into())), Some("a".into()));
        assert_eq!(key_text(&Key::Named(NamedKey::Backspace)), Some(slint::platform::Key::Backspace.into()));
        assert_eq!(key_text(&Key::Named(NamedKey::F1)), None);
    }
}
