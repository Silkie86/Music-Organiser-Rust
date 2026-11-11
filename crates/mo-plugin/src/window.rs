//! The plugin's window: the app's Slint panel, drawn inside the window the DAW lends the plugin.
//!
//! Slint normally opens and runs its own windows. In a DAW the DAW owns the window, so:
//! - [`baseview`] makes the plugin's own small window inside the DAW's one, and hands it the mouse and keys;
//! - Slint draws the panel on the CPU with its software renderer (no graphics card steps to share with the DAW) into
//!   a picture in memory, which [`softbuffer`] puts in that window;
//! - a short timer on the window's thread runs Slint's timers and animations and takes in what background threads
//!   found (the sound list, a search), newest first.
//!
//! On Windows and Mac the window's thread is the DAW's main thread, so nothing slow ever runs on it: reading the
//! library, searching it and reading a sound all run on background threads at low priority.

use crate::audition::Played;
use crate::library::{self, Asked, Library, ORDERS, Read};
use crate::threads::{background, lock};
use crate::waves::{self, Kept};
use crate::{ExtensionLine, FilterChoice, FilterView, Instance, Panel, SoundLine};
use baseview::dpi::{PhysicalPosition, Size};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, ScrollDelta, TimerHandle, WindowContext, WindowEvent,
    WindowHandler, WindowSettings, WindowSize,
};
use keyboard_types::{Key, KeyState, NamedKey};
use mo_app::backup::thousands;
use mo_app::filters::{Filter, Shown as Choice};
use mo_app::sounds::{Order, SoundCounts, TEMPO_HIGH, TEMPO_LOW, TEMPO_STEP};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{PointerEventButton, WindowAdapter};
use slint::{ComponentHandle, LogicalPosition, Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
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
const TICK: Duration = Duration::from_millis(30);

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
        Shown::new(context, instance).map_err(|why| HandlerError::from(std::io::Error::other(why)))
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
    static NEXT: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

impl slint::platform::Platform for Platform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        NEXT.with(|next| next.borrow_mut().take())
            .map(|w| w as Rc<dyn WindowAdapter>)
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
    /// A line dragged out: its row and its file, once it's been checked to be there, or why it can't go.
    drag: Mutex<Option<(usize, Result<std::path::PathBuf, String>)>>,
    /// The waveforms read for the lines on show, with their asking's round ([`Kept::to_read`]).
    waves: Mutex<Option<(u64, WavesRead)>>,
}

/// Each sound's waveform as a picture's pixels (made off the window's thread), or none.
type WavesRead = Vec<(i64, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>;

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    lock(slot).take()
}

/// The list's lines, made only for the rows shown (the list can hold half a million sounds).
struct Lines {
    library: RefCell<Option<Arc<Library>>>,
    shown: RefCell<Vec<usize>>,
    /// The waveforms of the lines on show, as pictures.
    waves: RefCell<Kept<slint::Image>>,
    notify: ModelNotify,
}

impl Lines {
    fn show(&self, library: Option<Arc<Library>>, shown: Vec<usize>) {
        *self.library.borrow_mut() = library;
        *self.shown.borrow_mut() = shown;
        self.notify.reset();
    }

    fn path(&self, row: usize) -> Option<String> {
        let library = self.library.borrow();
        let at = *self.shown.borrow().get(row)?;
        library.as_ref()?.list.sound(at).map(|s| s.path.clone())
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

/// The open panel, on the window's thread.
struct Shown {
    context: WindowContext,
    window: Rc<MinimalSoftwareWindow>,
    panel: Panel,
    surface: RefCell<softbuffer::Surface<baseview::PlatformHandle, baseview::PlatformHandle>>,
    /// The last picture Slint drew, put in the window whenever it's asked to show.
    picture: RefCell<Vec<Pixel>>,
    size: Cell<(u32, u32)>,
    scale: Cell<f64>,
    pointer: Cell<LogicalPosition>,
    lines: Rc<Lines>,
    slots: Arc<Slots>,
    instance: Arc<Instance>,
    /// What the search box, the filter column and the order ask of the list.
    asked: Rc<RefCell<Asked>>,
    column: Rc<Column>,
    /// The sound the "Playing" line names, as the player numbers it: the line goes when that sound ends.
    playing: Cell<Option<u64>>,
    /// The line last clicked to play: its row, its sound and its file.
    played: Rc<RefCell<Option<(usize, i64, std::path::PathBuf)>>>,
    /// A line's file is being carried out now ([`Shown::drag`]).
    dragging: Cell<bool>,
    _timer: TimerHandle,
}

impl Shown {
    fn new(context: WindowContext, instance: Arc<Instance>) -> Result<Shown, String> {
        // Already there for an earlier plugin window on this thread: that one is used.
        let _ = slint::platform::set_platform(Box::new(Platform { started: Instant::now() }));
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        NEXT.with(|next| *next.borrow_mut() = Some(Rc::clone(&window)));
        let panel = Panel::new();
        // Taken back whether or not the panel was made, so no later window gets this one.
        NEXT.with(|next| next.borrow_mut().take());
        let panel = panel.map_err(|e| format!("Couldn't make the panel: {e}"))?;

        let handle = context.platform_handle();
        let soft = softbuffer::Context::new(handle.clone()).map_err(|e| format!("Couldn't draw in the window: {e}"))?;
        let surface =
            softbuffer::Surface::new(&soft, handle).map_err(|e| format!("Couldn't draw in the window: {e}"))?;
        let timer = context.create_timer(TICK).map_err(|e| format!("Couldn't start the window's timer: {e}"))?;

        let lines = Rc::new(Lines {
            library: RefCell::new(None),
            shown: RefCell::new(Vec::new()),
            waves: RefCell::new(Kept::default()),
            notify: ModelNotify::default(),
        });
        let slots = Arc::new(Slots::default());
        let shown = Shown {
            context,
            window,
            panel,
            surface: RefCell::new(surface),
            picture: RefCell::new(Vec::new()),
            size: Cell::new((0, 0)),
            scale: Cell::new(1.0),
            pointer: Cell::new(LogicalPosition::default()),
            lines,
            slots,
            instance,
            asked: Rc::new(RefCell::new(Asked { seed: seed(), ..Asked::default() })),
            column: Rc::new(Column::default()),
            playing: Cell::new(None),
            played: Rc::new(RefCell::new(None)),
            dragging: Cell::new(false),
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
        panel.on_play(move |row| {
            let Some(row) = usize::try_from(row).ok() else { return };
            let Some(path) = lines.path(row) else { return };
            *last_played.borrow_mut() = lines.sound(row).map(|sound| (row, sound, path.clone().into()));
            let slots = Arc::clone(&slots);
            player.play(path.into(), move |played| *lock(&slots.played) = Some(played));
        });
        let player = Arc::clone(&self.instance.player);
        let weak = panel.as_weak();
        panel.on_stop(move || {
            player.stop();
            if let Some(panel) = weak.upgrade() {
                panel.set_playing(SharedString::new());
            }
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
                *lock(&slots.drag) = Some((row, found));
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
        panel.on_clear(move |filter| {
            change(&|a, library| match filter.as_str() {
                // The More types menu's Any keeps the types ticked in the column itself.
                "more-types" => a.filters.clear_more_types(library.map_or(&[][..], |l| &l.types[..])),
                filter => a.filters.clear_chip(filter),
            });
        });
        let change = Rc::clone(&changed);
        panel.on_toggle(move |line| change(&|a, _| a.filters.toggle(&line)));
        let change = Rc::clone(&changed);
        panel.on_tempo_picked(move |low, high| change(&|a, _| a.filters.set_tempo(low, high)));
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

        let slots = Arc::clone(&self.slots);
        background("mo-plugin-library", move || *lock(&slots.library) = Some(library::read()));
    }

    /// Takes in what background threads found, frees sounds done playing, and runs Slint's timers.
    fn tick(&self) {
        if let Some(read) = take(&self.slots.library) {
            match read {
                Read::Ready(library) => {
                    // The list comes with its first view, so it never shows in an order nobody asked for.
                    self.lines.show(Some(Arc::clone(&library)), Vec::new());
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
            self.lines.show(library, found);
        }
        if let Some(played) = take(&self.slots.played) {
            let line = match played {
                Played::Playing(name, n) => {
                    self.playing.set(Some(n));
                    format!("Playing {name}")
                }
                Played::Not(why) => {
                    self.playing.set(None);
                    why
                }
            };
            self.panel.set_playing(line.into());
        }
        if let Some(n) = self.playing.get()
            && !self.instance.player.still_playing(n)
        {
            self.playing.set(None);
            self.panel.set_playing(SharedString::new());
        }
        self.instance.player.tidy();
        self.drag();
        self.waves();
        self.render();
    }

    /// Carries a dragged line's file out, once it's been found: the system's drag holds this thread until it's let
    /// go (the timer still runs meanwhile, so nothing is kept borrowed across it).
    fn drag(&self) {
        if self.dragging.get() {
            return;
        }
        let Some((row, found)) = take(&self.slots.drag) else { return };
        let said = match found {
            Ok(path) => {
                self.dragging.set(true);
                self.panel.set_carried(i32::try_from(row).unwrap_or(-1));
                self.render();
                let done = crate::drag::file(&path);
                self.dragging.set(false);
                self.panel.set_carried(-1);
                // The system's drag kept the button's coming up for itself: the panel is told, so the line isn't
                // left pressed.
                let position = self.pointer.get();
                let event =
                    slint::platform::WindowEvent::PointerReleased { position, button: PointerEventButton::Left };
                self.window.window().dispatch_event(event);
                self.window.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
                done.err()
            }
            Err(why) => Some(why),
        };
        if let Some(why) = said {
            self.playing.set(None);
            self.panel.set_playing(why.into());
        }
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
            let scale = self.scale.get().clamp(0.5, 8.0);
            let (width, height) = ((waves::WIDTH * scale).round() as u32, (waves::HEIGHT * scale).round() as u32);
            if let Some(pixels) = waves::picture(&peaks, width, height) {
                self.lines.waves.borrow_mut().give(sound, slint::Image::from_rgba8(pixels));
                if self.lines.sound(row) == Some(sound) {
                    self.lines.notify.row_changed(row);
                }
            }
        }
        let Some((round, ids)) = self.lines.waves.borrow_mut().to_read() else { return };
        let scale = self.scale.get().clamp(0.5, 8.0);
        let (width, height) = ((waves::WIDTH * scale).round() as u32, (waves::HEIGHT * scale).round() as u32);
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
        let drew = self.window.draw_if_needed(|renderer| {
            let mut picture = self.picture.borrow_mut();
            renderer.render(picture.as_mut_slice(), width as usize);
        });
        if drew {
            self.context.request_redraw();
        }
    }

    fn resize_to(&self, size: WindowSize) {
        let (width, height) = (size.physical.width.max(1), size.physical.height.max(1));
        self.size.set((width, height));
        if (size.scale_factor - self.scale.get()).abs() > f64::EPSILON || self.picture.borrow().is_empty() {
            self.scale.set(size.scale_factor);
            // Pictures made for the old scaling are the wrong size: the lines ask again.
            self.lines.waves.borrow_mut().forget();
            self.lines.notify.reset();
            self.window.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: size.scale_factor as f32,
            });
        }
        *self.picture.borrow_mut() = vec![Pixel::default(); width as usize * height as usize];
        self.window.set_size(slint::PhysicalSize::new(width, height));
        self.window.request_redraw();
        self.render();
    }

    fn send(&self, event: slint::platform::WindowEvent) {
        self.window.window().dispatch_event(event);
        self.render();
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
        let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else { return Ok(()) };
        let mut surface = self.surface.borrow_mut();
        surface.resize(w, h).map_err(|e| HandlerError::from(std::io::Error::other(e.to_string())))?;
        let mut buffer = surface.buffer_mut().map_err(|e| HandlerError::from(std::io::Error::other(e.to_string())))?;
        let picture = self.picture.borrow();
        let pixels: &[u32] = bytemuck::cast_slice(picture.as_slice());
        let len = buffer.len().min(pixels.len());
        buffer[..len].copy_from_slice(&pixels[..len]);
        buffer.present().map_err(|e| HandlerError::from(std::io::Error::other(e.to_string())))?;
        Ok(())
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
                    self.send(Slint::PointerPressed { position: self.pointer.get(), button: slint_button(button) });
                }
                MouseEvent::ButtonReleased { button, .. } => {
                    self.send(Slint::PointerReleased { position: self.pointer.get(), button: slint_button(button) });
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
        let (shown, counts) = library.view(&asked);
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
}

impl Column {
    /// The panel's filter column and order as `asked` has them, over `library`'s types and tags once it's read.
    fn show(&self, panel: &Panel, library: Option<&Library>, asked: &Asked) {
        let chips = asked.chips(library, self.counts.borrow().as_ref());
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
    fn keys_slint_knows() {
        assert_eq!(key_text(&Key::Character("a".into())), Some("a".into()));
        assert_eq!(key_text(&Key::Named(NamedKey::Backspace)), Some(slint::platform::Key::Backspace.into()));
        assert_eq!(key_text(&Key::Named(NamedKey::F1)), None);
    }
}
