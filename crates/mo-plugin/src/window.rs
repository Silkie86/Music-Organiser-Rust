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

mod curve_bench;
mod curves_tab;
mod midi_tab;
mod presets_tab;

use crate::audition::{Own, Played};
use crate::galaxy::{self, Sky};
use crate::gpu::Gpu;
use crate::knobs::Knob;
use crate::library::{self, Asked, Library, Like, ORDERS, Read};
use crate::takes::Label;
use crate::threads::{background, lock};
use crate::waves::{self, Kept};
use crate::{
    ChordBox, ExtensionLine, FilterChoice, FilterView, InputLine, Instance, LabelLine, Panel, SkyName, SoundLine,
    SweptLine, TakeLine, TrailDot, YearBar,
};
use baseview::dpi::{PhysicalPosition, Size};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, ScrollDelta, TimerHandle, WindowContext, WindowEvent,
    WindowHandler, WindowSettings, WindowSize,
};
use keyboard_types::{Key, KeyState, Modifiers, NamedKey};
use mo_app::backup::thousands;
use mo_app::filters::{Filter, Shown as Choice};
use mo_app::galaxy_grid::Grid;
use mo_app::galaxy_view::{Hold, Moved, View};
use mo_app::sounds::{LikeKnown, Order, SoundCounts, TEMPO_HIGH, TEMPO_LOW, TEMPO_STEP, TYPE_CHIPS};
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

/// The size the panel opens at the first time, in logical pixels (before the screen's scaling): room for every
/// button with the list beside the filters, and a list long enough to look through (760 x 560 cut some off, and 960 x
/// 640 was still small on his 3440 x 1440 screen: Solomon 23 Nov 2026). Still fits a 1920 x 1080 screen.
pub const WIDTH: f64 = 1280.0;
pub const HEIGHT: f64 = 800.0;
/// The smallest the window can be dragged: the filter column, and beside it the Sounds tab's widest row of buttons
/// (the playing row, with Match tempo and In Bitwig's Sampler showing), with the folder tree's tab between them.
pub const LEAST_WIDTH: f64 = 764.0;
pub const LEAST_HEIGHT: f64 = 560.0;
/// The biggest: more than any screen today, so a DAW that asks for a silly size doesn't get a picture that big.
pub const MOST_WIDTH: f64 = 7680.0;
pub const MOST_HEIGHT: f64 = 4320.0;
/// Below this many points wide the folder tree folds to its tab, and opens over the list for a while when clicked.
const TREE_NARROW: f32 = 1150.0;

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
    let settings = WindowSettings::new().with_title("DAWmate").with_size(size).with_parent(parent).with_resizable(true);
    let window = baseview::Window::create(settings, move |context| {
        // Made inside Windows' own window code, which a panic can't pass through: one here only fails the opening
        // (the 14 Nov review's PL-2).
        let made =
            mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| Shown::new(context.clone(), Arc::clone(&instance))))
                .unwrap_or_else(|_| Err("The panel stopped with an error as it opened".into()));
        let shown = made.map_err(|why| HandlerError::from(std::io::Error::other(why)))?;
        // The window's one timer, made after the panel so a panel that couldn't be made leaves none behind. It stays
        // through a move to the card: baseview can't stop a timer, so one made with each panel would go on ticking
        // beside the next, and the panel's work would run twice a tick (the 14 Nov review's PW-2).
        let timer = match context.create_timer(TICK) {
            Ok(timer) => timer,
            Err(e) => {
                let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| drop(shown)));
                return Err(HandlerError::from(std::io::Error::other(format!(
                    "Couldn't start the window's timer: {e}"
                ))));
            }
        };
        Ok(Window { context, instance, shown: RefCell::new(Some(shown)), broken: Cell::new(false), _timer: timer })
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

/// A list worked out: the asking's number, the library it's of, the sounds it shows and the filter column's counts.
type Found = (u64, Arc<Library>, Vec<usize>, SoundCounts);

/// What background threads hand the panel: the newest of each wins.
#[derive(Default)]
struct Slots {
    library: Mutex<Option<Read>>,
    /// The library read again, when the window opened on the one kept from before and the app had changed it since.
    library_again: Mutex<Option<Arc<Library>>>,
    /// What the app says about folders (its sample folders, drives away, folders hidden), read with the library.
    folders: Mutex<Option<crate::folders::FolderSettings>>,
    /// What the list shows for what was asked, with the filter column's counts, the asking's number and the library
    /// it was worked out of.
    found: Mutex<Option<Found>>,
    /// Counts askings: only the newest one's answer is shown.
    searches: AtomicU64,
    /// Works the list out, one asking at a time ([`crate::threads::OneAtATime`]).
    lister: crate::threads::OneAtATime,
    /// This panel has shown the list with the sounds like your favourites in (or said they couldn't be found), once
    /// they were known: the library is shared by every panel, so each looks for itself ([`Window::tick`]).
    like_shown: AtomicBool,
    /// The app's Sounds page shows silent sounds (its Hide silent sounds is off), as read with the library.
    shows_silent: AtomicBool,
    /// The app's Sounds page hides duplicates: so does the list, until it's turned off here.
    hides_copies: AtomicBool,
    /// The sound types the track's name names ("Kick" on a Kick track), worked out with the library: the list's
    /// first filter.
    track_types: Mutex<Vec<String>>,
    played: Mutex<Option<Played>>,
    /// How putting a sound in Bitwig's Sampler went, in words.
    sampler: Mutex<Option<String>>,
    /// The sound card inputs Record can open, by name, once the system has said.
    inputs: Mutex<Option<Vec<String>>>,
    /// A line dragged out: which, and its file, once it's been checked to be there (or written, for a MIDI part),
    /// or why it can't go.
    drag: Mutex<Option<(u64, DragReady)>>,
    /// How many drags have been asked for, or called off by letting go before their file was ready: only the answer
    /// to the newest asking starts a drag (the 19 Nov review's PN-3).
    drag_asked: AtomicU64,
    /// A sound dragged out whole with Trim silence on, as its trimmed copy couldn't be made: what the panel says
    /// (SN-12).
    untrimmed: Mutex<Option<String>>,
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
    packed: Mutex<Option<(u64, galaxy::Packed)>>,
    /// The mini-map's picture, with its asking's number ([`Sky::mini_ask`]).
    mini: Mutex<Option<(u64, SkyPixels)>>,
}

/// A dragged line, and its file once it's ready (or why it can't go).
type DragReady = (Carried, Result<std::path::PathBuf, String>);

impl Slots {
    /// A line was dragged out: the number of this asking. An answer to an older one is dropped.
    fn ask_drag(&self) -> u64 {
        self.drag_asked.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// The file for drag asking `asked` is ready (or why it can't go).
    fn drag_ready(&self, asked: u64, carried: Carried, found: Result<std::path::PathBuf, String>) {
        crate::threads::newer(&self.drag, asked, (carried, found));
    }

    /// The mouse was let go: a drag whose file isn't ready yet is called off.
    fn drag_let_go(&self) {
        self.drag_asked.fetch_add(1, Ordering::AcqRel);
    }
}

/// The line being dragged out: a row of the Sounds list, a part of the MIDI tab's (its place in the catalogue), or a
/// preset of the Presets tab's (its place in the list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carried {
    Sound(usize),
    Part(usize),
    Preset(usize),
    /// A curve of the Curves tab's (its place in the list).
    Curve(usize),
    /// The last take Record saved.
    Recording,
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
    /// A library read again, to show once its list is worked out (until then the lines are the old one's).
    next: RefCell<Option<Arc<Library>>>,
    shown: RefCell<Arc<Vec<usize>>>,
    /// The waveforms of the lines on show, as pictures.
    waves: RefCell<Kept<slint::Image>>,
    notify: ModelNotify,
}

impl Lines {
    /// The library a new list is worked out of: the one read again when there is one, else the one on show.
    fn to_view(&self) -> Option<Arc<Library>> {
        self.next.borrow().clone().or_else(|| self.library.borrow().clone())
    }

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

    /// The tempo of the sound at place `at` in the whole list, as the library knows it.
    fn tempo_at(&self, at: usize) -> Own {
        let library = self.library.borrow();
        library.as_ref().and_then(|l| l.list.sound(at)).map_or(Own::NotALoop, library::own_tempo)
    }

    /// [`Lines::tempo_at`] for the sound on `row`.
    fn tempo(&self, row: usize) -> Own {
        match self.shown.borrow().get(row) {
            Some(&at) => self.tempo_at(at),
            None => Own::NotALoop,
        }
    }

    /// The id of the sound on `row`.
    /// Whether the library knows the sound on `row` as a loop (Trim silence drags a loop as it is).
    fn is_loop(&self, row: usize) -> bool {
        let library = self.library.borrow();
        let Some(&at) = self.shown.borrow().get(row) else { return false };
        library.as_ref().and_then(|l| l.list.sound(at)).is_some_and(|s| s.analysis.as_ref().is_some_and(|a| a.is_loop))
    }

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
    /// The panel's code panicked: the window stays empty, and nothing more is passed on to it.
    broken: Cell<bool>,
    /// The window's timer, for whichever panel is open: Slint's timers and animations, and what background threads
    /// found.
    _timer: TimerHandle,
}

impl Window {
    /// Runs `work`, catching a panic in it: the DAW calls the window through Windows' own window code, which a panic
    /// can't pass through without closing the whole DAW (the 14 Nov review's PL-2). After one, the panel is let go
    /// (once nothing is in the middle of it) and the window stays empty: the DAW and the song carry on.
    fn guarded<T>(&self, fallback: T, work: impl FnOnce() -> T) -> T {
        if self.broken.get() {
            self.let_go();
            return fallback;
        }
        match mo_catch::catch_crash(std::panic::AssertUnwindSafe(work)) {
            Ok(done) => done,
            Err(_) => {
                self.broken.set(true);
                crate::log::line(
                    "Window",
                    "The panel stopped with an error (see the crash note): close and open it again",
                );
                self.let_go();
                fallback
            }
        }
    }

    /// Lets a panel that panicked go, once nothing holds it (a drag out holds it while its own timer runs).
    fn let_go(&self) {
        if let Ok(mut shown) = self.shown.try_borrow_mut()
            && let Some(old) = shown.take()
        {
            let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| drop(old)));
        }
    }

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

impl Drop for Window {
    /// The window closes inside Windows' own window code, which a panic can't pass through: the panel goes inside a
    /// catch (the 19 Nov review's PN-5).
    fn drop(&mut self) {
        let shown = self.shown.get_mut().take();
        let _ = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| drop(shown)));
    }
}

impl WindowHandler for Window {
    fn draw(&self) -> Result<(), HandlerError> {
        self.guarded(Ok(()), || self.shown.borrow().as_ref().map_or(Ok(()), WindowHandler::draw))
    }

    fn resized(&self, new_size: WindowSize) -> Result<(), HandlerError> {
        self.guarded(Ok(()), || self.shown.borrow().as_ref().map_or(Ok(()), |shown| shown.resized(new_size)))
    }

    fn on_timer(&self, timer: &TimerHandle) {
        self.guarded((), || {
            self.move_to_the_card();
            if let Some(shown) = self.shown.borrow().as_ref() {
                shown.on_timer(timer);
            }
        });
    }

    fn on_event(&self, event: Event) -> EventStatus {
        self.guarded(EventStatus::Ignored, || {
            self.shown.borrow().as_ref().map_or(EventStatus::Ignored, |shown| shown.on_event(event))
        })
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
    /// The screen's pixels to a point (shared with what draws pictures for the panel).
    scale: Rc<Cell<f64>>,
    pointer: Cell<LogicalPosition>,
    lines: Rc<Lines>,
    slots: Arc<Slots>,
    instance: Arc<Instance>,
    /// What the search box, the filter column and the order ask of the list.
    asked: Rc<RefCell<Asked>>,
    column: Rc<Column>,
    /// The MIDI tab.
    midi: midi_tab::MidiTab,
    /// The Presets tab.
    presets: presets_tab::PresetsTab,
    curves: curves_tab::CurvesTab,
    /// What's being dragged out now.
    carried: Cell<Option<Carried>>,
    /// The last take Record saved, shown as a line to hear and drag out.
    recorded: Rc<RefCell<Option<std::path::PathBuf>>>,
    /// The Record from menu: its lines, and how loud each input is.
    record_menu: Rc<RecordMenu>,
    /// The take's shape last handed over, and the room it was last drawn for.
    record_shape: RefCell<Vec<f32>>,
    record_shape_room: Cell<(f32, f32)>,
    /// The number of the newest chord Chord check read that the panel shows ([`crate::chordcheck::Checker::newest`]).
    chords_seen: Cell<u64>,
    /// The number of Chord check's list of takes the panel shows (a pick or a label sets it to none, so it's shown
    /// again), and the take picked, by its name.
    takes_seen: Rc<Cell<u64>>,
    take_picked: Rc<RefCell<Option<String>>>,
    /// The takes as the list shows them, by name, so a click picks the take it was on even if one came in since.
    takes_shown: Rc<RefCell<Vec<String>>>,
    /// The take whose chords are shown and their rows, changed in place while it's the same take (so a list
    /// scrolled down stays where it is).
    take_rows: RefCell<Option<(String, Rc<VecModel<LabelLine>>)>>,
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
    /// Whether Slint was last told Shift, Ctrl and Alt are held ([`Self::keys_held`]).
    keys_told: Cell<[bool; 3]>,
    /// What was last clicked to be heard, and whether it has started playing: a click on it again stops it.
    heard: Rc<Cell<Option<(Heard, bool)>>>,
    /// The MIDI part last asked to play, and the DAW's tempo it was made at: made again when the DAW's tempo moves.
    part: Rc<RefCell<Option<PartPlayed>>>,
    /// The DAW's tempo as last seen, and since when: a part is made again once a new tempo has stayed a moment.
    tempo_seen: Cell<Option<(f64, Instant)>>,
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

        let lines = Rc::new(Lines {
            library: RefCell::new(None),
            next: RefCell::new(None),
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
            scale: Rc::new(Cell::new(1.0)),
            pointer: Cell::new(LogicalPosition::default()),
            lines,
            slots,
            curves: curves_tab::CurvesTab::new(Arc::clone(&instance.player)),
            asked: Rc::new(RefCell::new(Asked { seed: instance.seed, ..Asked::default() })),
            instance,
            column: Rc::new(Column::default()),
            midi: midi_tab::MidiTab::new(),
            presets: presets_tab::PresetsTab::new(),
            carried: Cell::new(None),
            recorded: Rc::default(),
            record_menu: Rc::default(),
            record_shape: RefCell::default(),
            record_shape_room: Cell::new((0.0, 0.0)),
            chords_seen: Cell::new(u64::MAX),
            takes_seen: Rc::new(Cell::new(u64::MAX)),
            take_picked: Rc::default(),
            takes_shown: Rc::default(),
            take_rows: RefCell::default(),
            playing: Cell::new(None),
            played: Rc::new(RefCell::new(None)),
            wave_width: Cell::new(waves::WIDTH as f32),
            dragging: Cell::new(false),
            right_now: Rc::default(),
            keys_told: Cell::new([false; 3]),
            heard: Rc::default(),
            part: Rc::default(),
            tempo_seen: Cell::new(None),
            sky: Rc::default(),
            sky_names: RefCell::default(),
            sky_trail: RefCell::default(),
            painter: RefCell::new(None),
            painter_failed: Cell::new(false),
            sky_asked: Cell::new(false),
            ticked: Cell::new(Instant::now()),
        };
        shown.fill_in();
        // What the knobs were turned to while no panel was open isn't acted on now (a sound would play as it opens);
        // the DAW's knobs are moved to what the panel shows instead.
        let (knobs, player) = (&shown.instance.knobs, &shown.instance.player);
        knobs.turned();
        knobs.tell(Knob::Like, f64::from(shown.panel.get_like_favourites()));
        knobs.tell(Knob::Loop, f64::from(u8::from(player.looping())));
        knobs.tell(Knob::Match, f64::from(u8::from(player.matching())));
        knobs.tell(Knob::InTime, f64::from(u8::from(player.in_time())));
        shown.resize_to(shown.context.size());
        Ok(shown)
    }

    /// The panel's words and lists, and what its buttons do; then the sound library is read in the background.
    fn fill_in(&self) {
        let panel = &self.panel;
        // Opened again on the tab it was closed on.
        panel.set_view(self.instance.view.load(Ordering::Relaxed));
        panel.set_host(self.instance.host.as_str().into());
        panel.set_track(lock(&self.instance.track).as_str().into());
        panel.set_track_colour(match *lock(&self.instance.track_colour) {
            Some([r, g, b]) => slint::Color::from_rgb_u8(r, g, b),
            None => slint::Color::from_argb_u8(0, 0, 0, 0),
        });
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
        let instance = Arc::clone(&self.instance);
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
            // The DAW's Sound knob moves to it.
            instance.knobs.tell_line(Knob::Sound, row, lines.row_count());
            *last_played.borrow_mut() = Some((row, sound, path.clone().into()));
            heard.set(Some((Heard::Line(sound), false)));
            let slots = Arc::clone(&slots);
            player.play(path.into(), from, lines.tempo(row), move |played| *lock(&slots.played) = Some(played));
        });
        let stop_it = Rc::clone(&stop);
        panel.on_stop(move || stop_it());
        // Trim silence, for this plugin while it's loaded: it starts off, at -60 dB, and shows what the player has
        // when the window opens again.
        panel.set_trim_choices(ModelRc::new(slint::VecModel::from(
            mo_audio::trim::choices().into_iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
        panel.set_loop_on(self.instance.player.looping());
        let instance = Arc::clone(&self.instance);
        panel.on_loop_changed(move |on| {
            instance.player.set_loop(on);
            instance.knobs.tell(Knob::Loop, f64::from(u8::from(on)));
        });
        panel.set_in_time(self.instance.player.in_time());
        let instance = Arc::clone(&self.instance);
        panel.on_in_time_changed(move |on| {
            instance.player.set_in_time(on);
            instance.knobs.tell(Knob::InTime, f64::from(u8::from(on)));
        });
        panel.set_midi_out(self.instance.player.midi_out());
        let player = Arc::clone(&self.instance.player);
        panel.on_midi_out_changed(move |on| player.set_midi_out(on));
        panel.set_match_tempo(self.instance.player.matching());
        let instance = Arc::clone(&self.instance);
        panel.on_match_tempo_changed(move |on| {
            instance.player.set_matching(on);
            instance.knobs.tell(Knob::Match, f64::from(u8::from(on)));
        });
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
        let player = Arc::clone(&self.instance.player);
        panel.set_in_bitwig(self.instance.host.starts_with("Bitwig"));
        let sampler_slots = Arc::clone(&self.slots);
        let sampler_lines = Rc::clone(&self.lines);
        let weak = panel.as_weak();
        panel.on_try_in_sampler(move || {
            let Some(panel) = weak.upgrade() else { return };
            let Some(path) = usize::try_from(panel.get_picked()).ok().and_then(|row| sampler_lines.path(row)) else {
                return;
            };
            panel.set_playing(format!("Putting {} in Bitwig's Sampler", library::name(&path)).into());
            let slots = Arc::clone(&sampler_slots);
            background("mo-plugin-sampler", move || {
                let turn = mo_dawlink::bitwig::ticket().wait();
                let said = match mo_dawlink::bitwig::Bitwig::default().swap_into_sampler(&path, &turn) {
                    Ok(swapped) => {
                        // "Went in" only when Bitwig showed it did (the 19 Nov review's LK-3).
                        let how = if swapped.unknown { "may have gone in" } else { "went in" };
                        crate::log::line("Sampler", &format!("{} {how}: {}", library::name(&path), swapped.summary()));
                        swapped.summary()
                    }
                    Err(e) => {
                        crate::log::line("Sampler", &format!("{} didn't go in: {e}", library::name(&path)));
                        e.to_string()
                    }
                };
                *lock(&slots.sampler) = Some(said);
            });
        });
        let let_go = Arc::clone(&self.slots);
        panel.on_drag_let_go(move || let_go.drag_let_go());
        panel.on_drag_out(move |row| {
            let Some((row, path)) = usize::try_from(row).ok().and_then(|r| Some((r, lines.path(r)?))) else { return };
            let slots = Arc::clone(&slots);
            let trim = player.trim();
            let is_loop = lines.is_loop(row);
            // Whether the file is there is asked off the window's thread: its drive may be asleep or away.
            let asked = slots.ask_drag();
            background("mo-plugin-drag", move || {
                let path = std::path::PathBuf::from(path);
                let found = if path.is_file() {
                    // Trim silence on: the trimmed copy goes instead, as the app drags it (loops go as they are), or
                    // the whole file when one can't be made: the panel says so, and the log says why (SN-12).
                    let (path, why) = mo_app::trim_copy::to_drag(&path, trim, is_loop);
                    if let Some(why) = why {
                        crate::log::line("Drag", &why);
                        *lock(&slots.untrimmed) = Some(format!(
                            "{} went untrimmed (the log says why)",
                            library::name(&path.to_string_lossy())
                        ));
                    }
                    Ok(path)
                } else {
                    Err(format!("Can't drag {}: the file isn't there", library::name(&path.to_string_lossy())))
                };
                slots.drag_ready(asked, Carried::Sound(row), found);
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
                let library = lines.to_view();
                let before = asked.borrow().clone();
                change(&mut asked.borrow_mut(), library.as_deref());
                let asked = asked.borrow().clone();
                // A Tempo drag says the same step again on each mouse move: nothing changed, nothing to work out.
                if asked == before {
                    return;
                }
                if let Some(panel) = weak.upgrade() {
                    column.show(&panel, library.as_deref(), &asked);
                }
                if let Some(library) = library {
                    like_wanted(&library, &asked, &slots);
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
        let instance = Arc::clone(&self.instance);
        panel.on_like_favourites_set(move |share| {
            instance.knobs.tell(Knob::Like, f64::from(share));
            change(&|a, _| a.filters.like_favourites = mo_app::sounds::like_favourites_step(share));
        });
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
        self.fill_in_tree(&changed);
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
        self.presets.fill_in(panel, &self.slots, &self.instance);
        self.curves.fill_in(panel, &self.slots);
        self.fill_in_midi_play();
        self.fill_in_record();
        self.fill_in_chords();
        let slots = Arc::clone(&self.slots);
        let track = lock(&self.instance.track).clone();
        // The app's sample folders and hidden folders: on their own thread, as a sleeping drive may keep it waiting.
        let folders = Arc::clone(&self.slots);
        background("mo-plugin-folders", move || *lock(&folders.folders) = Some(crate::folders::read()));
        background("mo-plugin-library", move || {
            // As the app's Sounds page left Hide silent sounds, before the list's first view.
            slots.shows_silent.store(library::shows_silent(), Ordering::Release);
            slots.hides_copies.store(library::hides_copies(), Ordering::Release);
            // The types the track's name names, among the library's (it goes over every sound: not on the panel's
            // thread).
            let first = |read: Read| {
                if let Read::Ready(library) = &read {
                    let types = library.list.type_counts();
                    *lock(&slots.track_types) = crate::track::named(&track, types.iter().map(|(t, _)| t.as_str()));
                }
                *lock(&slots.library) = Some(read);
            };
            // The library kept from before shows at once; when the app has changed it since, it's read again and
            // takes its place once its list is worked out.
            let kept = library::kept();
            if let Some(kept) = &kept {
                first(Read::Ready(Arc::clone(kept)));
            }
            let read = library::read();
            match (kept, read) {
                (None, read) => first(read),
                (Some(kept), Read::Ready(again)) if !Arc::ptr_eq(&kept, &again) => {
                    *lock(&slots.library_again) = Some(again);
                }
                // The same, or it couldn't be read again: the one kept stays.
                (Some(_), _) => {}
            }
        });
    }

    /// A click on a MIDI part plays it as a loop with the app's SoundFont, at the DAW's tempo, through the track; a
    /// click on the one playing stops it (the same rule as a sound's line, which has no waveform here).
    /// The DAW's knobs turned ([`crate::knobs`]): a list's line picked (and played) as a click on it would, the Sounds
    /// like slider moved, a switch shown as it now is.
    fn knobs(&self) {
        let (knobs, panel) = (&self.instance.knobs, &self.panel);
        let lines = [
            (Knob::Sound, panel.get_sounds().row_count()),
            (Knob::Part, panel.get_parts().row_count()),
            (Knob::Preset, panel.get_presets().row_count()),
        ];
        for (knob, count) in lines {
            knobs.set_lines(knob, count);
        }
        let count = |knob| lines.iter().find(|(k, _)| *k == knob).map_or(0, |(_, n)| *n);
        let player = &self.instance.player;
        for knob in knobs.turned() {
            let value = knobs.get(knob);
            let line = crate::knobs::line_at(value, count(knob)).and_then(|at| i32::try_from(at).ok());
            // A line is brought into sight in its list (48 points a line).
            let top = |at: i32| at as f32 * 48.0;
            match (knob, line) {
                (Knob::Sound, Some(at)) if at != panel.get_picked() => {
                    panel.set_picked(at);
                    panel.set_sounds_show_top(top(at));
                    panel.set_sounds_show_ask(panel.get_sounds_show_ask().wrapping_add(1));
                    panel.invoke_play(at, 0.0);
                }
                (Knob::Part, Some(at)) if at != panel.get_midi_picked() => {
                    panel.set_midi_picked(at);
                    panel.set_parts_show_top(top(at));
                    panel.set_parts_show_ask(panel.get_parts_show_ask().wrapping_add(1));
                    panel.invoke_midi_play(at);
                }
                (Knob::Preset, Some(at)) if at != panel.get_preset_picked() => {
                    panel.set_preset_picked(at);
                    panel.set_presets_show_top(top(at));
                    panel.set_presets_show_ask(panel.get_presets_show_ask().wrapping_add(1));
                    panel.invoke_preset_click(at);
                }
                (Knob::Like, _) => {
                    panel.set_like_favourites(value as f32);
                    panel.invoke_like_favourites_set(value as f32);
                }
                (Knob::Loop, _) => panel.set_loop_on(player.looping()),
                (Knob::Match, _) => panel.set_match_tempo(player.matching()),
                (Knob::InTime, _) => panel.set_in_time(player.in_time()),
                _ => {}
            }
        }
    }

    /// Chord check: the Thru switch, and the newest chord read (while the panel was shut too).
    fn fill_in_chords(&self) {
        let chords = &self.instance.chords;
        self.panel.set_chord_thru(chords.thru.load(Ordering::Acquire));
        let chords_ = Arc::clone(chords);
        self.panel.on_chord_thru_changed(move |on| chords_.thru.store(on, Ordering::Release));
        self.chords_seen.set(u64::MAX);
        let (seen, picked, shown) =
            (Rc::clone(&self.takes_seen), Rc::clone(&self.take_picked), Rc::clone(&self.takes_shown));
        self.panel.on_take_pick(move |row| {
            let id = usize::try_from(row).ok().and_then(|row| shown.borrow().get(row).cloned());
            *picked.borrow_mut() = id;
            seen.set(u64::MAX);
        });
        let (picked, weak) = (Rc::clone(&self.take_picked), self.panel.as_weak());
        let chords_ = Arc::clone(chords);
        self.panel.on_label(move |row, how, choice| {
            let label = match how.as_str() {
                "right" => Label::Right,
                "choice" => Label::Choice(usize::try_from(choice).unwrap_or(0)),
                "not a chord" => Label::NotAChord,
                _ => Label::Skip,
            };
            label_take(&chords_, picked.borrow().as_deref(), row, label, &weak);
        });
        let (picked, weak) = (Rc::clone(&self.take_picked), self.panel.as_weak());
        let chords_ = Arc::clone(chords);
        self.panel.on_label_typed(move |row, text| {
            let Some(id) = picked.borrow().clone() else { return false };
            let lowest = usize::try_from(row).ok().and_then(|row| chords_.lowest(&id, row));
            match crate::takes::typed(&text, lowest) {
                Some(chord) => {
                    label_take(&chords_, Some(&id), row, Label::Typed(chord), &weak);
                    true
                }
                None => {
                    if let Some(panel) = weak.upgrade() {
                        panel.set_take_said(
                            format!("Can't read \"{}\" as a chord: try Cmaj9, G7#9 or D/C", text.trim()).into(),
                        );
                    }
                    false
                }
            }
        });
        self.chords_said();
    }

    /// Chord check's takes, when they changed: the list, and the chords of the one picked.
    fn takes_said(&self) {
        let chords = &self.instance.chords;
        let Some((number, takes)) = chords.takes(self.takes_seen.get()) else { return };
        self.takes_seen.set(number);
        let panel = &self.panel;
        let lines: Vec<TakeLine> = takes
            .iter()
            .map(|t| TakeLine { title: t.title.as_str().into(), details: t.details.as_str().into() })
            .collect();
        panel.set_takes(ModelRc::new(VecModel::from(lines)));
        *self.takes_shown.borrow_mut() = takes.iter().map(|t| t.id.clone()).collect();
        let picked = self.take_picked.borrow().clone();
        let at = picked.as_ref().and_then(|id| takes.iter().position(|t| &t.id == id));
        panel.set_take_picked(at.map_or(-1, |i| i32::try_from(i).unwrap_or(-1)));
        let Some((id, (grid, labels))) =
            picked.filter(|_| at.is_some()).and_then(|id| Some((id.clone(), chords.chords_of(&id)?)))
        else {
            *self.take_rows.borrow_mut() = None;
            panel.set_take_chords(ModelRc::default());
            return;
        };
        let rows: Vec<LabelLine> = labels
            .iter()
            .map(|c| LabelLine {
                at: crate::takes::place(grid, c.start).into(),
                read: c.read.map(|r| r.name()).unwrap_or_default().into(),
                sure: c.sure.shade(),
                choices: ModelRc::new(VecModel::from(
                    c.choices.iter().map(|s| SharedString::from(s.name())).collect::<Vec<_>>(),
                )),
                said: c.said_words().into(),
            })
            .collect();
        let mut shown = self.take_rows.borrow_mut();
        if let Some((was, model)) = shown.as_ref()
            && *was == id
            && model.row_count() == rows.len()
        {
            // The same take: only the rows that changed (what was said), so the list stays where it was.
            for (i, row) in rows.into_iter().enumerate() {
                if model.row_data(i).is_none_or(|old| old.said != row.said) {
                    model.set_row_data(i, row);
                }
            }
            return;
        }
        let model = Rc::new(VecModel::from(rows));
        panel.set_take_chords(ModelRc::from(Rc::clone(&model)));
        *shown = Some((id, model));
    }

    /// The newest chord Chord check read, when there's a new one.
    fn chords_said(&self) {
        self.takes_said();
        let Some((number, heard)) = self.instance.chords.newest(self.chords_seen.get()) else { return };
        self.chords_seen.set(number);
        let panel = &self.panel;
        panel.set_chord_name(heard.name.clone().unwrap_or_default().into());
        let sure = if heard.name.is_some() { heard.sure.words() } else { "Play a chord into the plugin" };
        panel.set_chord_sure(sure.into());
        panel.set_chord_line(heard.line().into());
        let strip: Vec<ChordBox> = heard
            .strip
            .iter()
            .map(|c| ChordBox {
                name: c.name.as_str().into(),
                at: c.at.as_str().into(),
                sure: c.sure.shade(),
                now: c.now,
            })
            .collect();
        panel.set_chord_strip(ModelRc::new(VecModel::from(strip)));
    }

    /// How loud the inputs are: every input while the Record from menu is open, and what's picked while the Sounds
    /// tab shows (the overall input level beside Record). Inputs are opened to be measured only then.
    fn record_levels(&self) {
        let menu = &self.record_menu;
        let open = menu.alive.get().is_some_and(|at| at.elapsed() < MENU_GONE);
        let picked = self.panel.get_record_from().to_string();
        let asked = if open {
            menu.inputs.borrow().clone()
        } else if !picked.is_empty() && self.panel.get_view() == 0 {
            vec![picked.clone()]
        } else {
            Vec::new()
        };
        menu.meters.ask(asked);
        menu.heard(self.instance.recorder.daw_level.take(), menu.meters.take());
        self.panel.set_record_level(crate::record::meter_share(menu.level(&picked)));
        if open {
            menu.show(&picked);
        }
    }

    /// The folder tree beside the filter column ([`crate::folders`]): a tick narrows the list to the folder's sounds,
    /// the arrow, its search and folding it only change how it's shown.
    fn fill_in_tree<C: Fn(&dyn Fn(&mut Asked, Option<&Library>)) + 'static>(&self, changed: &Rc<C>) {
        let panel = &self.panel;
        // The tree shown again as it now is, without asking for the list again.
        let shown = {
            let lines = Rc::clone(&self.lines);
            let asked = Rc::clone(&self.asked);
            let column = Rc::clone(&self.column);
            let weak = panel.as_weak();
            Rc::new(move || {
                if let Some(panel) = weak.upgrade() {
                    let library = lines.library.borrow().clone();
                    column.show(&panel, library.as_deref(), &asked.borrow());
                }
            })
        };
        let change = Rc::clone(changed);
        panel.on_tree_toggled(move |path| change(&|a, library| crate::folders::ticked(&path, a, library)));
        let change = Rc::clone(changed);
        panel.on_tree_taken_off(move |path, hidden| change(&|a, _| crate::folders::taken_off(&path, hidden, a)));
        let change = Rc::clone(changed);
        panel.on_tree_cleared(move || change(&|a, _| a.filters.folders.clear()));
        // A colour of the picks' type mix picks that type, as its line in the filter column would.
        let change = Rc::clone(changed);
        panel.on_tree_mix_picked(move |name| change(&|a, _| a.filters.pick("type", &name)));
        let (column, again) = (Rc::clone(&self.column), Rc::clone(&shown));
        panel.on_tree_opened(move |path| {
            let key = mo_app::sound_folders::key(&path);
            {
                let mut tree = column.tree.borrow_mut();
                if !tree.open.remove(&key) {
                    tree.open.insert(key);
                }
            }
            again();
        });
        let (column, again) = (Rc::clone(&self.column), Rc::clone(&shown));
        panel.on_tree_search_edited(move |words| {
            column.tree.borrow_mut().search = words.to_string();
            again();
        });
        let (column, again) = (Rc::clone(&self.column), Rc::clone(&shown));
        panel.on_tree_fold(move || {
            {
                let mut tree = column.tree.borrow_mut();
                tree.folded = !tree.folded;
            }
            again();
        });
        let (column, asked, weak) = (Rc::clone(&self.column), Rc::clone(&self.asked), panel.as_weak());
        let lines = Rc::clone(&self.lines);
        panel.on_tree_menu_asked(move |path| {
            column.tree.borrow_mut().menu_for = path.to_string();
            if let Some(panel) = weak.upgrade() {
                let library = lines.library.borrow().clone();
                let lines = crate::folders::menu(&asked.borrow(), library.as_deref(), &path);
                panel.set_tree_menu(ModelRc::new(VecModel::from(lines)));
            }
        });
        let (column, lines, change) = (Rc::clone(&self.column), Rc::clone(&self.lines), Rc::clone(changed));
        panel.on_tree_menu_chosen(move |id| {
            let path = column.tree.borrow().menu_for.clone();
            if path.is_empty() {
                return;
            }
            if id == "open-all" {
                let library = lines.library.borrow().clone();
                crate::folders::opened_inside(&path, library.as_deref(), &column.tree);
                shown();
            } else {
                change(&|a, library| crate::folders::chosen(&id, &path, a, library));
            }
        });
    }

    /// Record: what it records from, its button, and the last take as a line to hear and drag into the project.
    fn fill_in_record(&self) {
        let panel = &self.panel;
        let recorder = &self.instance.recorder;
        panel.set_recording(recorder.recording());
        // The sound cards are asked for off the window's thread: the system can take a while to answer.
        let slots = Arc::clone(&self.slots);
        background("mo-plugin-inputs", move || *lock(&slots.inputs) = Some(crate::record::inputs()));
        panel.set_record_menu(ModelRc::from(Rc::clone(&self.record_menu.lines) as Rc<dyn Model<Data = InputLine>>));
        let (menu, weak) = (Rc::clone(&self.record_menu), panel.as_weak());
        panel.on_record_menu_opened(move || {
            // It opens on the sound cards, showing where what's picked is.
            *menu.card.borrow_mut() = None;
            menu.alive.set(Some(Instant::now()));
            if let Some(panel) = weak.upgrade() {
                menu.show(&panel.get_record_from());
            }
        });
        panel.set_record_open(self.instance.record_open.load(Ordering::Relaxed));
        let instance = Arc::clone(&self.instance);
        panel.on_record_open_changed(move |open| instance.record_open.store(open, Ordering::Relaxed));
        let menu = Rc::clone(&self.record_menu);
        panel.on_record_menu_alive(move || menu.alive.set(Some(Instant::now())));
        let (menu, weak) = (Rc::clone(&self.record_menu), panel.as_weak());
        panel.on_record_menu_clicked(move |row| {
            let (Some(panel), Some(line)) =
                (weak.upgrade(), usize::try_from(row).ok().and_then(|r| menu.lines.row_data(r)))
            else {
                return;
            };
            if line.back {
                *menu.card.borrow_mut() = None;
            } else if line.card {
                *menu.card.borrow_mut() = Some(line.key.to_string());
            } else {
                panel.set_record_from(line.key);
                *menu.card.borrow_mut() = None;
            }
            menu.show(&panel.get_record_from());
        });
        let (recorder, player, weak) = (Arc::clone(recorder), Arc::clone(&self.instance.player), panel.as_weak());
        panel.on_record(move || {
            let Some(panel) = weak.upgrade() else { return };
            let from = panel.get_record_from();
            let card = (!from.is_empty()).then(|| from.to_string());
            // L+R (0) records the pair; L (1) or R (2) one side of it, as a mono take.
            let only = usize::try_from(panel.get_record_side() - 1).ok();
            match recorder.start(card, player.trim(), only) {
                Ok(()) => {
                    panel.set_recording(true);
                    panel.set_record_said("Starting".into());
                }
                Err(why) => panel.set_record_said(why.into()),
            }
        });
        let recorder = Arc::clone(&self.instance.recorder);
        panel.on_record_stop(move || recorder.stop());
        let (recorded, player, slots) =
            (Rc::clone(&self.recorded), Arc::clone(&self.instance.player), Arc::clone(&self.slots));
        let (heard, last_played) = (Rc::clone(&self.heard), Rc::clone(&self.played));
        panel.on_recorded_play(move || {
            let Some(path) = recorded.borrow().clone() else { return };
            heard.set(None);
            *last_played.borrow_mut() = None;
            let slots = Arc::clone(&slots);
            player.play(path, 0.0, Own::NotALoop, move |played| *lock(&slots.played) = Some(played));
        });
        let (recorded, slots) = (Rc::clone(&self.recorded), Arc::clone(&self.slots));
        panel.on_recorded_drag_out(move || {
            let Some(path) = recorded.borrow().clone() else { return };
            let slots = Arc::clone(&slots);
            // Saved trimmed already: it goes as it is, once it's checked to be there still.
            let asked = slots.ask_drag();
            background("mo-plugin-drag", move || {
                let found = if path.is_file() {
                    Ok(path)
                } else {
                    Err(format!("Can't drag {}: the file isn't there any more", path.display()))
                };
                slots.drag_ready(asked, Carried::Recording, found);
            });
        });
    }

    /// What Record has to tell (how the take is going, or what became of it), and what Listen hears.
    fn record_said(&self) {
        if let Some(inputs) = take(&self.slots.inputs) {
            *self.record_menu.inputs.borrow_mut() = inputs;
        }
        self.record_levels();
        // The take's shape, drawn again as it grows or the drawing's room changes.
        if let Some(shape) = lock(&self.instance.recorder.shape).take() {
            self.panel.set_record_has_shape(!shape.is_empty());
            *self.record_shape.borrow_mut() = shape;
            self.record_shape_room.set((0.0, 0.0));
        }
        let room = (self.panel.get_record_wave_width(), self.panel.get_record_wave_height());
        if room != self.record_shape_room.get() {
            self.record_shape_room.set(room);
            let path = shape_path(&self.record_shape.borrow(), crate::record::SHAPE_SHOWN, room.0, room.1);
            self.panel.set_record_shape(path.into());
        }
        if let Some(hears) = lock(&self.instance.listener.hears).take() {
            self.panel.set_hears(hears.words().into());
        }
        let Some(said) = lock(&self.instance.recorder.said).take() else { return };
        self.panel.set_record_said(said.words().into());
        match said {
            crate::record::Said::Going { .. } => self.panel.set_recording(true),
            crate::record::Said::Saved(path) => {
                self.panel.set_recording(false);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.panel.set_recorded_name(name.into());
                *self.recorded.borrow_mut() = Some(path);
            }
            crate::record::Said::Failed(_) => self.panel.set_recording(false),
        }
    }

    fn fill_in_midi_play(&self) {
        let player = Arc::clone(&self.instance.player);
        let slots = Arc::clone(&self.slots);
        let (right_now, heard, stop_it) = (Rc::clone(&self.right_now), Rc::clone(&self.heard), self.stopper());
        let midi = self.midi.part_finder();
        let last_played = Rc::clone(&self.played);
        let roll = self.midi.roll_drawer(&self.panel);
        let scale = Rc::clone(&self.scale);
        let (instance, weak) = (Arc::clone(&self.instance), self.panel.as_weak());
        let part = Rc::clone(&self.part);
        self.panel.on_midi_play(move |row| {
            let Some((parts, at)) = usize::try_from(row).ok().and_then(&midi) else { return };
            if let (Some(panel), Ok(row)) = (weak.upgrade(), usize::try_from(row)) {
                instance.knobs.tell_line(Knob::Part, row, panel.get_parts().row_count());
            }
            // Its piano roll, whether it plays or stops.
            roll(Arc::clone(&parts), at, scale.get());
            let playing = heard.get().is_some_and(|(what, _)| what == Heard::Part(at));
            match line_click(playing, right_now.get(), -1.0) {
                Click::Play(_) => {}
                Click::Stop => return stop_it(),
                Click::Nothing => return,
            }
            let Some(name) = parts.entry(at).map(mo_app::midi::title) else { return };
            heard.set(Some((Heard::Part(at), false)));
            // No sound's line shows where this plays.
            *last_played.borrow_mut() = None;
            let tempo = player.tempo();
            let make = make_part(Arc::clone(&parts), at, tempo, player.midi_out());
            *part.borrow_mut() = Some(PartPlayed { parts, at, name: name.clone(), made_at: tempo });
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
                player.play(path.into(), 0.0, lines.tempo_at(at), move |played| *lock(&slots.played) = Some(played));
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
            background("mo-plugin-galaxy", move || *lock(&slots.map) = Some(galaxy::read_kept(&library)));
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
                crate::threads::newer(&slots.grid, set, grid);
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
                crate::threads::newer(&slots.names, naming.set, found);
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
                    crate::threads::newer(&slots.mini, set, pixels);
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
                let asked = pack.asked;
                let packed = pack.pack();
                crate::log::took_if_slow("Galaxy", "Made the stars ready and lit", started);
                crate::threads::newer(&slots.packed, asked, packed);
            });
        }
        take(&self.slots.packed).map(|(_, packed)| packed).filter(|packed| sky.packed(packed))
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
        self.instance.view.store(self.panel.get_view(), Ordering::Relaxed);
        let size = self.panel.window().size().to_logical(self.panel.window().scale_factor());
        self.panel.set_tree_narrow(size.width < TREE_NARROW);
        self.follow_tempo();
        self.knobs();
        if let Some(read) = take(&self.slots.library) {
            match read {
                Read::Ready(library) => {
                    // The list comes with its first view, so it never shows in an order nobody asked for.
                    self.lines.show(Some(Arc::clone(&library)), Arc::default());
                    self.panel.set_status("Getting the list ready".into());
                    self.asked.borrow_mut().filters.show_silent = self.slots.shows_silent.load(Ordering::Acquire);
                    self.asked.borrow_mut().filters.hide_copies = self.slots.hides_copies.load(Ordering::Acquire);
                    // On a track named for a kind of sound, the list opens on it ("Kick": kicks).
                    for t in std::mem::take(&mut *lock(&self.slots.track_types)) {
                        self.asked.borrow_mut().filters.toggle_type(&t);
                    }
                    self.column.show(&self.panel, Some(&library), &self.asked.borrow());
                    like_wanted(&library, &self.asked.borrow(), &self.slots);
                    view(library, self.asked.borrow().clone(), Arc::clone(&self.slots));
                }
                Read::Not(why) => self.panel.set_status(why.into()),
            }
        }
        // The library read again (the app changed it since the one kept): its list is worked out, and it takes the
        // old one's place when that's in. The sounds like your favourites are looked for in it again.
        if let Some(again) = take(&self.slots.library_again) {
            *self.lines.next.borrow_mut() = Some(Arc::clone(&again));
            self.slots.like_shown.store(false, Ordering::Release);
            like_wanted(&again, &self.asked.borrow(), &self.slots);
            view(again, self.asked.borrow().clone(), Arc::clone(&self.slots));
        }
        // The folders the app hides are hidden here too, below the app's sample folders: the list again once it's
        // there (before it is, the first view takes them).
        if let Some(settings) = take(&self.slots.folders) {
            {
                let mut asked = self.asked.borrow_mut();
                asked.filters.hidden_folders = settings.hidden;
                asked.filters.folder_tops = settings.tops;
            }
            self.column.tree.borrow_mut().away = settings.away.into_iter().collect();
            let library = self.lines.to_view();
            let asked = self.asked.borrow().clone();
            self.column.show(&self.panel, library.as_deref(), &asked);
            if let Some(library) = library {
                view(library, asked, Arc::clone(&self.slots));
            }
        }
        // The sounds like your favourites became known (whichever panel asked for them): the list again, once.
        if let Some(library) = self.lines.to_view()
            && !self.slots.like_shown.load(Ordering::Acquire)
            && matches!(library.like(), Like::Ready(_) | Like::Not(_))
        {
            self.slots.like_shown.store(true, Ordering::Release);
            let asked = self.asked.borrow().clone();
            self.column.show(&self.panel, Some(&library), &asked);
            view(library, asked, Arc::clone(&self.slots));
        }
        if let Some((number, library, found, counts)) = take(&self.slots.found)
            && number == self.slots.searches.load(Ordering::Acquire)
        {
            // A library read again comes on show with its first list: the Galaxy's stars are read again for it.
            let before = self.lines.library.borrow().clone();
            if before.is_some_and(|b| !Arc::ptr_eq(&b, &library)) {
                self.sky.borrow_mut().forget_map();
                self.sky_asked.set(false);
            }
            if self.lines.next.borrow().as_ref().is_some_and(|n| Arc::ptr_eq(n, &library)) {
                *self.lines.next.borrow_mut() = None;
            }
            let library = Some(library);
            let total = library.as_ref().map_or(0, |l| l.list.len());
            // Silent sounds hidden narrow it too (they're not a filter the Clear chip takes off).
            let hiding_silent = {
                let asked = self.asked.borrow();
                !asked.filters.show_silent && library.as_ref().is_some_and(|l| l.list.silent() > 0)
            };
            // So do folders hidden (kept through Clear, as the app keeps them).
            let narrowed =
                self.asked.borrow().any() || hiding_silent || !self.asked.borrow().filters.hidden_folders.is_empty();
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
        if let Some(said) = take(&self.slots.sampler) {
            self.panel.set_playing(said.into());
        }
        self.record_said();
        self.chords_said();
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
        // The play head over the MIDI tab's piano roll, while the part it shows plays.
        let part = match (self.heard.get(), self.playing.get()) {
            (Some((Heard::Part(at), _)), Some(n)) => self.instance.player.place(n).map(|place| (at, place)),
            _ => None,
        };
        self.midi.show_head(&self.panel, part);
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
        self.presets.tick(&self.panel);
        self.curves.tick(&self.panel);
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
        let Some((asked, (carried, found))) = take(&self.slots.drag) else { return };
        // An older drag's file, or one let go before it was ready, never starts a drag under a later press.
        if asked != self.slots.drag_asked.load(Ordering::Acquire) {
            return;
        }
        self.carried.set(Some(carried));
        match found {
            Ok(path) => {
                self.dragging.set(true);
                match carried {
                    Carried::Sound(row) => {
                        self.panel.set_carried(i32::try_from(row).unwrap_or(-1));
                        if let Some(said) = take(&self.slots.untrimmed) {
                            self.playing.set(None);
                            self.panel.set_playing(said.into());
                        }
                    }
                    Carried::Part(at) => self.panel.set_midi_carried(self.midi.row_of(at)),
                    Carried::Preset(at) => self.panel.set_preset_carried(self.presets.row_of(at)),
                    Carried::Curve(at) => {
                        self.panel.set_curve_carried(self.curves.row_of(at));
                        self.panel.set_curve_edit_lifted(self.panel.get_curve_editing());
                    }
                    Carried::Recording => self.panel.set_recorded_carried(true),
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
        self.panel.set_preset_carried(-1);
        self.panel.set_curve_carried(-1);
        self.panel.set_curve_edit_lifted(false);
        self.panel.set_recorded_carried(false);
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
        match self.carried.get() {
            Some(Carried::Part(_)) => return self.panel.set_midi_said(why.into()),
            Some(Carried::Preset(_)) => return self.panel.set_preset_said(why.into()),
            Some(Carried::Curve(_)) => return self.panel.set_curve_said(why.into()),
            Some(Carried::Recording) => return self.panel.set_record_said(why.into()),
            _ => {}
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
            // An older round's answer never takes the newest's place (the 19 Nov review's PN-6): the newest is
            // the one whose sounds wait in `reading`.
            crate::threads::newer(&slots.waves, round, pictures);
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

    /// The DAW's tempo moved while a MIDI part plays: the part is made again at the new tempo and goes on from the
    /// same place in it, once the new tempo has stayed a moment (not at every step of a tempo being dragged). A part
    /// played again later is made at the tempo then (Solomon, 23 Nov 2026: a part kept its old tempo).
    fn follow_tempo(&self) {
        let player = &self.instance.player;
        let Some(now) = player.tempo() else { return };
        match self.tempo_seen.get() {
            Some((seen, since)) if seen == now => {
                if since.elapsed() < TEMPO_SETTLES {
                    return;
                }
            }
            _ => {
                self.tempo_seen.set(Some((now, Instant::now())));
                return;
            }
        }
        let Some((Heard::Part(at), _)) = self.heard.get() else { return };
        let mut part = self.part.borrow_mut();
        let Some(p) = part.as_mut().filter(|p| p.at == at) else { return };
        if !tempo_moved(p.made_at, now) {
            return;
        }
        p.made_at = Some(now);
        let make = make_part(Arc::clone(&p.parts), at, Some(now), player.midi_out());
        let slots = Arc::clone(&self.slots);
        player.swap_made(p.name.clone(), make, move |played| *lock(&slots.played) = Some(played));
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

    /// The mouse says which of Shift, Ctrl and Alt are held: Slint is told of any it doesn't know of. A key
    /// pressed while the DAW had the keys never came here as a key, and Slint forgets them all when its window
    /// loses the keys; the curve editor's drags go by them from the very press.
    fn keys_held(&self, modifiers: Modifiers) {
        use slint::platform::{Key as S, WindowEvent as Slint};
        let ctrl = if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::CONTROL };
        let now = [modifiers.contains(Modifiers::SHIFT), modifiers.contains(ctrl), modifiers.contains(Modifiers::ALT)];
        let told = self.keys_told.replace(now);
        for (at, key) in [S::Shift, S::Control, S::Alt].into_iter().enumerate() {
            if now[at] != told[at] {
                let text: SharedString = key.into();
                self.send(if now[at] { Slint::KeyPressed { text } } else { Slint::KeyReleased { text } });
            }
        }
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
                MouseEvent::CursorMoved { position, modifiers } => {
                    self.keys_held(modifiers);
                    let position = self.at(position);
                    self.send(Slint::PointerMoved { position });
                }
                MouseEvent::ButtonPressed { button, modifiers } => {
                    // The keys come to the panel once it's clicked (for the search box).
                    if !self.context.has_focus() {
                        let _ = self.context.focus();
                    }
                    self.keys_held(modifiers);
                    self.right_now.set(button == MouseButton::Right);
                    self.send(Slint::PointerPressed { position: self.pointer.get(), button: slint_button(button) });
                    self.right_now.set(false);
                }
                MouseEvent::ButtonReleased { button, modifiers } => {
                    self.keys_held(modifiers);
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
                // Up and Down move through the list on show (Solomon, 25 Nov), held down too. Where there's no list
                // to move through (the Galaxy turns with them) they go on to the panel as before.
                let by = match key.key {
                    Key::Named(NamedKey::ArrowUp) => -1,
                    Key::Named(NamedKey::ArrowDown) => 1,
                    _ => 0,
                };
                if by != 0 && key.state == KeyState::Down && self.panel.invoke_step(by) {
                    return EventStatus::Captured;
                }
                // Left stops the sound and Right plays it again, in the Sounds list.
                let side = match key.key {
                    Key::Named(NamedKey::ArrowLeft) => -1,
                    Key::Named(NamedKey::ArrowRight) => 1,
                    _ => 0,
                };
                if side != 0 && key.state == KeyState::Down && !key.repeat && self.panel.invoke_side(side) {
                    return EventStatus::Captured;
                }
                let Some(text) = key_text(&key.key) else { return EventStatus::Ignored };
                // What Slint knows of Shift, Ctrl and Alt from here on.
                if let Some(at) = held_key(&key.key) {
                    let mut told = self.keys_told.get();
                    told[at] = key.state == KeyState::Down;
                    self.keys_told.set(told);
                }
                self.send(match (key.state, key.repeat) {
                    (KeyState::Down, false) => Slint::KeyPressed { text },
                    (KeyState::Down, true) => Slint::KeyPressRepeated { text },
                    (KeyState::Up, _) => Slint::KeyReleased { text },
                });
            }
            Event::Window(WindowEvent::Focused) => self.send(Slint::WindowActiveChanged(true)),
            Event::Window(WindowEvent::Unfocused) => {
                // Slint lets go of Shift, Ctrl and Alt itself when its window loses the keys.
                self.keys_told.set([false; 3]);
                self.send(Slint::WindowActiveChanged(false));
            }
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

/// Starts working out the sounds like your favourites on a background thread, when `asked` wants them and they haven't
/// been yet: every panel shows the list again once they're known ([`Slots::like_shown`]).
fn like_wanted(library: &Arc<Library>, asked: &Asked, slots: &Slots) {
    if !library.start_like(asked) {
        return;
    }
    slots.like_shown.store(false, Ordering::Release);
    let stopped = library::Stopped(Arc::clone(library));
    background("mo-plugin-like", move || stopped.0.read_like());
}

/// Works out what `asked` shows of `library` on a background thread; it lands in `slots.found` with this asking's
/// number, and only the newest asking's answer is shown.
fn view(library: Arc<Library>, asked: Asked, slots: Arc<Slots>) {
    let number = slots.searches.fetch_add(1, Ordering::AcqRel) + 1;
    // Worked out lately: it shows at once (a window opened again asks for what it showed).
    if let Some((shown, counts)) = Views::get(&library, &asked) {
        *lock(&slots.found) = Some((number, library, shown, counts));
        return;
    }
    let theirs = Arc::clone(&slots);
    slots.lister.ask(move || {
        let slots = theirs;
        // Newer words were typed while this one waited: theirs is on its way.
        if slots.searches.load(Ordering::Acquire) != number {
            return;
        }
        let started = Instant::now();
        let (shown, counts) = library.view(&asked);
        crate::log::took_if_slow("List", &format!("Worked out a list of {} sounds", shown.len()), started);
        Views::keep(&library, &asked, &shown, &counts);
        let mut found = lock(&slots.found);
        if found.as_ref().is_none_or(|(n, ..)| *n < number) {
            *found = Some((number, library, shown, counts));
        }
    });
}

/// The last few lists worked out, kept while the plugin is loaded, so a window opened again shows its list at once
/// instead of "Getting the list ready" (Solomon, 25 Nov). Only a few: each is a number a sound (about 4 MB for his
/// library). A list of a library read again since isn't used.
struct Views;

/// How many lists [`Views`] keeps.
const VIEWS_KEPT: usize = 3;

type KeptView = (std::sync::Weak<Library>, Asked, Vec<usize>, SoundCounts);

static VIEWS: Mutex<Vec<KeptView>> = Mutex::new(Vec::new());

impl Views {
    fn get(library: &Arc<Library>, asked: &Asked) -> Option<(Vec<usize>, SoundCounts)> {
        let mut views = lock(&VIEWS);
        views.retain(|(l, ..)| l.strong_count() > 0);
        let at = views.iter().position(|(l, a, ..)| std::ptr::eq(l.as_ptr(), Arc::as_ptr(library)) && a == asked)?;
        // The newest used goes last: the oldest goes first when there's no room.
        let view = views.remove(at);
        let found = (view.2.clone(), view.3.clone());
        views.push(view);
        Some(found)
    }

    fn keep(library: &Arc<Library>, asked: &Asked, shown: &[usize], counts: &SoundCounts) {
        let mut views = lock(&VIEWS);
        views.retain(|(l, a, ..)| {
            l.strong_count() > 0 && !(std::ptr::eq(l.as_ptr(), Arc::as_ptr(library)) && a == asked)
        });
        if views.len() >= VIEWS_KEPT {
            views.remove(0);
        }
        views.push((Arc::downgrade(library), asked.clone(), shown.to_vec(), counts.clone()));
    }
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
    /// The folder tree's own state: branches open, folded, its search, the line whose menu is open.
    tree: RefCell<crate::folders::TreeState>,
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
        panel.set_hide_silent(!asked.filters.show_silent);
        panel.set_hide_copies(asked.filters.hide_copies);
        panel.set_hide_copies_words(library::hide_copies_words(library.and_then(|l| l.list.copies())).into());
        panel.set_hide_silent_words(mo_app::sounds::hide_silent_text(library.map(|l| l.list.silent())).into());
        let step = asked.filters.like_favourites;
        panel.set_like_favourites(mo_app::sounds::like_favourites_share(step));
        let (known, by_measurements) = match library.map(Library::like) {
            Some(Like::Ready(like)) => (LikeKnown::Yes, like.by_measurements(step)),
            Some(Like::Not(_)) => (LikeKnown::Missing, 0),
            _ => (LikeKnown::Finding, 0),
        };
        panel.set_like_favourites_words(mo_app::sounds::like_favourites_words(step, known).into());
        panel.set_like_favourites_note(mo_app::sounds::like_favourites_note(step, by_measurements).into());
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
        crate::folders::show(panel, library, self.counts.borrow().as_ref(), asked, &self.tree);
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

/// How long the DAW's tempo stays at a new value before a playing MIDI part is made again at it.
const TEMPO_SETTLES: Duration = Duration::from_millis(250);

/// A MIDI part asked to play: where it is, its name, and the DAW's tempo it was made at (none when the DAW hadn't
/// said one: it was made at its file's).
struct PartPlayed {
    parts: Arc<crate::midi::Parts>,
    at: usize,
    name: String,
    made_at: Option<f64>,
}

/// Whether a part made at `made_at` is to be made again at the DAW's tempo `now`: it moved by more than the drift
/// the player puts right on its own (0.2%), or the part wasn't made at the DAW's tempo at all.
fn tempo_moved(made_at: Option<f64>, now: f64) -> bool {
    made_at.is_none_or(|m| (now / m - 1.0).abs() > 0.002)
}

/// What makes the MIDI part at `at` to play at `tempo`: its notes sent out (Send MIDI on) or its sound.
fn make_part(parts: Arc<crate::midi::Parts>, at: usize, tempo: Option<f64>, midi_out: bool) -> crate::audition::Made {
    if midi_out {
        Box::new(move || parts.notes_out(at, tempo))
    } else {
        Box::new(move || {
            let (audio, beats) = parts.sound(at, crate::midi::soundfont().as_deref(), tempo)?;
            Ok((crate::audition::MadeLoop::Sound(audio), beats))
        })
    }
}

/// The word for a shuffled list, first among each list's orders, as on the Sounds tab (Solomon, 23 Nov 2026: "a
/// shuffle on all lists").
const SHUFFLED: &str = "Shuffled";

/// Puts `list` in a random order that `seed` decides (the same seed, the same order).
fn shuffle(list: &mut [usize], seed: u64) {
    let mut state = seed;
    // splitmix64: a small, sure maker of numbers from the seed.
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..list.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        list.swap(i, j);
    }
}

/// How long after the Record from menu last said it's open it counts as closed (it says so every 150 ms).
const MENU_GONE: Duration = Duration::from_millis(400);

/// How much of a meter's level is kept from one tick to the next as it falls back (about 25 dB a second).
const LEVEL_FALLS: f32 = 0.93;

/// The Record from menu: the sound cards, then the inputs on the one opened, each with how loud it is.
#[derive(Default)]
struct RecordMenu {
    /// Every input, by its whole name, as the system lists them.
    inputs: RefCell<Vec<String>>,
    /// The card whose inputs show; none for the list of cards.
    card: RefCell<Option<String>>,
    lines: Rc<VecModel<InputLine>>,
    /// When the menu last said it's open.
    alive: Cell<Option<Instant>>,
    /// How loud each input measured is, as shown (falling back slowly from its peaks), by name.
    levels: RefCell<Vec<(String, f32)>>,
    /// The same for the DAW's input to the plugin.
    daw: Cell<f32>,
    meters: Arc<crate::record::Meters>,
}

impl RecordMenu {
    /// New peaks heard: the DAW's input's, and the inputs measured (none for one that couldn't be opened).
    fn heard(&self, daw: f32, measured: Vec<(String, Option<f32>)>) {
        self.daw.set(daw.max(self.daw.get() * LEVEL_FALLS));
        let mut levels = self.levels.borrow_mut();
        for (_, level) in levels.iter_mut() {
            *level *= LEVEL_FALLS;
        }
        for (name, peak) in measured {
            let peak = peak.unwrap_or(0.0);
            match levels.iter_mut().find(|(n, _)| *n == name) {
                Some((_, level)) => *level = level.max(peak),
                None => levels.push((name, peak)),
            }
        }
    }

    /// How loud the input called `name` is ("" for the DAW's input).
    fn level(&self, name: &str) -> f32 {
        if name.is_empty() {
            return self.daw.get();
        }
        self.levels.borrow().iter().find(|(n, _)| n == name).map_or(0.0, |(_, l)| *l)
    }

    /// The menu's lines as they are now, `picked` being what's picked: changed in place where only the levels
    /// moved, so the menu doesn't flicker.
    fn show(&self, picked: &str) {
        let lines = record_menu_lines(&self.inputs.borrow(), self.card.borrow().as_deref(), picked, |n| self.level(n));
        let same = self.lines.row_count() == lines.len()
            && lines.iter().enumerate().all(|(i, l)| {
                self.lines.row_data(i).is_some_and(|o| o.key == l.key && o.text == l.text && o.picked == l.picked)
            });
        if same {
            for (i, line) in lines.into_iter().enumerate() {
                if self.lines.row_data(i).is_some_and(|o| (o.level - line.level).abs() > 0.002) {
                    self.lines.set_row_data(i, line);
                }
            }
        } else {
            self.lines.set_vec(lines);
        }
    }
}

/// The Record from menu's lines for `inputs` (whole names), with the card `card` open (none: the list of cards),
/// `picked` picked, and each input as loud as `level` says. A card with one input is that input's own line.
fn record_menu_lines(
    inputs: &[String],
    card: Option<&str>,
    picked: &str,
    level: impl Fn(&str) -> f32,
) -> Vec<InputLine> {
    use crate::record::{card_and_input, meter_share};
    let line = |text: &str, key: &str, card: bool, back: bool, picked: bool, level: f32| InputLine {
        text: text.into(),
        key: key.into(),
        level: meter_share(level),
        card,
        back,
        picked,
    };
    match card {
        Some(c) => std::iter::once(line("All sound cards", "", false, true, false, 0.0))
            .chain(on_card(inputs, c).map(|n| line(card_and_input(n).1, n, false, false, n == picked, level(n))))
            .collect(),
        None => {
            let mut cards: Vec<&str> = Vec::new();
            for n in inputs {
                let c = card_and_input(n).0;
                if !cards.contains(&c) {
                    cards.push(c);
                }
            }
            let mut lines = vec![line("The DAW's input to the plugin", "", false, false, picked.is_empty(), level(""))];
            for c in cards {
                let names: Vec<&String> = on_card(inputs, c).collect();
                match names.as_slice() {
                    [only] => lines.push(line(only, only, false, false, *only == picked, level(only))),
                    _ => {
                        let loudest = names.iter().map(|n| level(n)).fold(0.0, f32::max);
                        lines.push(line(c, c, true, false, names.iter().any(|n| *n == picked), loudest));
                    }
                }
            }
            lines
        }
    }
}

/// A take's shape (the loudest of each stretch, 0 to 1) as Path commands for a drawing `width` by `height` (in its
/// own units, the same as its size: a Path keeps its drawing's proportions): one shape, up from the middle and back
/// down below it, `room` stretches across it. Nothing for no shape yet.
fn shape_path(shape: &[f32], room: usize, width: f32, height: f32) -> String {
    use std::fmt::Write;
    if shape.is_empty() || width <= 0.0 || height <= 0.0 {
        return String::new();
    }
    let (step, middle) = (width / room.max(1) as f32, height / 2.0);
    let size = |p: f32| if p.is_finite() { p.clamp(0.0, 1.0) * middle } else { 0.0 };
    let mut out = format!("M 0 {middle:.1}");
    for (i, p) in shape.iter().enumerate() {
        let (x, y) = (i as f32 * step, middle - size(*p));
        let _ = write!(out, " L {x:.1} {y:.1} L {:.1} {y:.1}", x + step);
    }
    let _ = write!(out, " L {:.1} {middle:.1}", shape.len() as f32 * step);
    for (i, p) in shape.iter().enumerate().rev() {
        let (x, y) = (i as f32 * step, middle + size(*p));
        let _ = write!(out, " L {:.1} {y:.1} L {x:.1} {y:.1}", x + step);
    }
    out.push_str(" Z");
    out
}

/// The inputs in `inputs` on the card `card`.
fn on_card<'a>(inputs: &'a [String], card: &'a str) -> impl Iterator<Item = &'a String> + 'a {
    inputs.iter().filter(move |n| crate::record::card_and_input(n).0 == card)
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

/// Which of Shift (0), Ctrl (1) and Alt (2) a key is, as Slint has them. On a Mac, Cmd is Slint's Ctrl (as in
/// Slint's own windows): the curve editor's hints name Cmd there.
fn held_key(key: &Key) -> Option<usize> {
    match key {
        Key::Named(NamedKey::Shift) => Some(0),
        Key::Named(NamedKey::Control) if !cfg!(target_os = "macos") => Some(1),
        Key::Named(NamedKey::Meta) if cfg!(target_os = "macos") => Some(1),
        Key::Named(NamedKey::Alt) => Some(2),
        _ => None,
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
        // On a Mac, Cmd is Slint's Ctrl and Ctrl its Meta, as in Slint's own windows.
        NamedKey::Control if cfg!(target_os = "macos") => S::Meta,
        NamedKey::Meta if cfg!(target_os = "macos") => S::Control,
        NamedKey::Control => S::Control,
        NamedKey::Alt => S::Alt,
        _ => return None,
    };
    Some(key.into())
}

/// Labels chord `row` of the take `id` (the one picked) and saves the take off the window's thread, in the data
/// folder's Chord check folder: a take is saved once something in it is labelled, and again at each label after
/// (one save at a time, the newest last: [`crate::chordcheck::Checker::save`]).
fn label_take(
    chords: &Arc<crate::chordcheck::Checker>,
    id: Option<&str>,
    row: i32,
    label: Label,
    weak: &slint::Weak<Panel>,
) {
    let (Some(id), Ok(row)) = (id, usize::try_from(row)) else { return };
    if !chords.label(id, row, label) {
        return;
    }
    if let Some(panel) = weak.upgrade() {
        panel.set_take_said(SharedString::new());
    }
    let (chords, id) = (Arc::clone(chords), id.to_owned());
    background("mo-plugin-take", move || {
        chords.save(&id, &mo_app::paths::data_dir().join(mo_app::chord_check::FOLDER));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_takes_shape_draws_up_and_down_from_the_middle() {
        assert_eq!(shape_path(&[], 4, 40.0, 10.0), "");
        assert_eq!(
            shape_path(&[0.5, 2.0], 4, 40.0, 10.0),
            "M 0 5.0 L 0.0 2.5 L 10.0 2.5 L 10.0 0.0 L 20.0 0.0 L 20.0 5.0 L 20.0 10.0 L 10.0 10.0 L 10.0 7.5 L 0.0 7.5 Z"
        );
    }

    #[test]
    fn a_list_worked_out_is_kept_for_its_library_and_asking_only() {
        let library = Arc::new(Library::new(vec![mo_sounds::Sound::new(1, r"D:\Drums\kick.wav")]));
        let asked = Asked { seed: 7, ..Asked::default() };
        let (shown, counts) = library.view(&asked);
        Views::keep(&library, &asked, &shown, &counts);
        assert_eq!(Views::get(&library, &asked), Some((shown.clone(), counts.clone())));
        // Another asking, or the same library read again: worked out anew.
        assert!(Views::get(&library, &Asked { seed: 8, ..Asked::default() }).is_none());
        let again = Arc::new(Library::new(vec![mo_sounds::Sound::new(1, r"D:\Drums\kick.wav")]));
        assert!(Views::get(&again, &asked).is_none());
        // Only a few are kept: the oldest goes.
        for seed in 10..10 + VIEWS_KEPT as u64 {
            Views::keep(&library, &Asked { seed, ..Asked::default() }, &shown, &counts);
        }
        assert!(Views::get(&library, &asked).is_none());
        assert_eq!(lock(&VIEWS).len(), VIEWS_KEPT);
    }

    #[test]
    fn only_the_newest_drag_asked_for_starts_and_letting_go_calls_it_off() {
        let slots = Slots::default();
        let (first, second) = (slots.ask_drag(), slots.ask_drag());
        // The newer one's file comes first; the older one's, slow, comes after and never takes its place.
        slots.drag_ready(second, Carried::Preset(2), Ok("two.bwpreset".into()));
        slots.drag_ready(first, Carried::Preset(1), Ok("one.bwpreset".into()));
        let (asked, (carried, _)) = take(&slots.drag).unwrap();
        assert_eq!((asked, carried), (second, Carried::Preset(2)));
        assert_eq!(asked, slots.drag_asked.load(Ordering::Acquire), "the newest: it starts");
        // Let go before the file is ready: when it comes, it's no longer the newest asking.
        let third = slots.ask_drag();
        slots.drag_let_go();
        slots.drag_ready(third, Carried::Sound(0), Ok("kick.wav".into()));
        let (asked, _) = take(&slots.drag).unwrap();
        assert_ne!(asked, slots.drag_asked.load(Ordering::Acquire), "called off");
    }

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
    fn a_part_is_made_again_once_the_tempo_moves_past_the_drift_the_player_mends() {
        assert!(!tempo_moved(Some(120.0), 120.0));
        assert!(!tempo_moved(Some(120.0), 120.2));
        assert!(tempo_moved(Some(120.0), 125.0));
        assert!(tempo_moved(Some(120.0), 90.0));
        // Made at its file's tempo before the DAW said one: made again at the DAW's.
        assert!(tempo_moved(None, 120.0));
    }

    #[test]
    fn a_shuffle_keeps_every_line_and_the_same_seed_gives_the_same_order() {
        let mut a: Vec<usize> = (0..50).collect();
        let mut b = a.clone();
        shuffle(&mut a, 7);
        shuffle(&mut b, 7);
        assert_eq!(a, b);
        assert_ne!(a, (0..50).collect::<Vec<_>>());
        a.sort_unstable();
        assert_eq!(a, (0..50).collect::<Vec<_>>());
        let mut c: Vec<usize> = (0..50).collect();
        shuffle(&mut c, 8);
        assert_ne!(b, c);
    }

    #[test]
    fn the_record_menu_lists_cards_then_the_inputs_on_the_one_opened() {
        let inputs: Vec<String> =
            ["Microphone (Blue Snowball)", "Input 1/2 (Bitwig Connect)", "Loopback 5/6 (Bitwig Connect)"]
                .map(String::from)
                .to_vec();
        let loud = |n: &str| if n.starts_with("Loopback") { 1.0 } else { 0.0 };
        let top = record_menu_lines(&inputs, None, "Loopback 5/6 (Bitwig Connect)", loud);
        let texts: Vec<&str> = top.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["The DAW's input to the plugin", "Microphone (Blue Snowball)", "Bitwig Connect"]);
        // The one-input card is its input; the other opens, showing it holds what's picked and its loudest input.
        assert!(!top[1].card && top[1].key == "Microphone (Blue Snowball)");
        assert!(top[2].card && top[2].picked && top[2].level == 1.0);
        let card = record_menu_lines(&inputs, Some("Bitwig Connect"), "", loud);
        let texts: Vec<&str> = card.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["All sound cards", "Input 1/2", "Loopback 5/6"]);
        assert!(card[0].back && card[2].key == "Loopback 5/6 (Bitwig Connect)");
    }

    #[test]
    fn keys_slint_knows() {
        assert_eq!(key_text(&Key::Character("a".into())), Some("a".into()));
        assert_eq!(key_text(&Key::Named(NamedKey::Backspace)), Some(slint::platform::Key::Backspace.into()));
        assert_eq!(key_text(&Key::Named(NamedKey::F1)), None);
    }
}
