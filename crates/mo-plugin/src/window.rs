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
use crate::library::{self, Library, Read};
use crate::{ExtensionLine, Instance, Panel, SoundLine};
use baseview::dpi::{LogicalSize, PhysicalPosition};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, ScrollDelta, TimerHandle, WindowContext, WindowEvent,
    WindowHandler, WindowSettings, WindowSize,
};
use keyboard_types::{Key, KeyState, NamedKey};
use mo_app::backup::thousands;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{PointerEventButton, WindowAdapter};
use slint::{ComponentHandle, LogicalPosition, Model, ModelNotify, ModelRc, ModelTracker, SharedString};
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The panel's size, in logical pixels (before the screen's scaling).
pub const WIDTH: f64 = 560.0;
pub const HEIGHT: f64 = 520.0;

/// How often the window's timer runs: Slint's timers and animations, and what background threads found.
const TICK: Duration = Duration::from_millis(30);

/// Opens the panel inside `parent` (the window the DAW lends). The window goes when the returned one is dropped.
pub fn open(
    parent: &impl raw_window_handle::HasWindowHandle,
    instance: Arc<Instance>,
) -> Result<baseview::Window, String> {
    let settings = WindowSettings::new()
        .with_title("Music Organiser")
        .with_size(LogicalSize::new(WIDTH, HEIGHT))
        .with_parent(parent)
        .with_resizable(false);
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
    /// A search's matches, and the words searched.
    found: Mutex<Option<(String, Vec<u32>)>>,
    played: Mutex<Option<Played>>,
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    lock(slot).take()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The list's lines, made only for the rows shown (the list can hold half a million sounds).
struct Lines {
    library: RefCell<Option<Arc<Library>>>,
    shown: RefCell<Vec<u32>>,
    notify: ModelNotify,
}

impl Lines {
    fn show(&self, library: Option<Arc<Library>>, shown: Vec<u32>) {
        *self.library.borrow_mut() = library;
        *self.shown.borrow_mut() = shown;
        self.notify.reset();
    }

    fn path(&self, row: usize) -> Option<String> {
        let library = self.library.borrow();
        let at = *self.shown.borrow().get(row)?;
        library.as_ref()?.entries.get(at as usize).map(|e| e.path.clone())
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
        let entry = library.as_ref()?.entries.get(at as usize)?;
        Some(SoundLine { name: entry.name().into(), folder: entry.folder().into() })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
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
    /// The words last searched, so a search that comes back after newer words were typed is dropped.
    words: Rc<RefCell<String>>,
    _timer: TimerHandle,
}

impl Shown {
    fn new(context: WindowContext, instance: Arc<Instance>) -> Result<Shown, String> {
        // Already there for an earlier plugin window on this thread: that one is used.
        let _ = slint::platform::set_platform(Box::new(Platform { started: Instant::now() }));
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        NEXT.with(|next| *next.borrow_mut() = Some(Rc::clone(&window)));
        let panel = Panel::new().map_err(|e| format!("Couldn't make the panel: {e}"))?;
        NEXT.with(|next| next.borrow_mut().take());

        let handle = context.platform_handle();
        let soft = softbuffer::Context::new(handle.clone()).map_err(|e| format!("Couldn't draw in the window: {e}"))?;
        let surface =
            softbuffer::Surface::new(&soft, handle).map_err(|e| format!("Couldn't draw in the window: {e}"))?;
        let timer = context.create_timer(TICK).map_err(|e| format!("Couldn't start the window's timer: {e}"))?;

        let lines = Rc::new(Lines {
            library: RefCell::new(None),
            shown: RefCell::new(Vec::new()),
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
            words: Rc::new(RefCell::new(String::new())),
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
        panel.on_play(move |row| {
            let Some(path) = usize::try_from(row).ok().and_then(|r| lines.path(r)) else { return };
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
        let lines = Rc::clone(&self.lines);
        let slots = Arc::clone(&self.slots);
        let words = Rc::clone(&self.words);
        panel.on_search(move |typed| {
            *words.borrow_mut() = typed.to_string();
            let Some(library) = lines.library.borrow().clone() else { return };
            search(library, typed.to_string(), Arc::clone(&slots));
        });

        let slots = Arc::clone(&self.slots);
        background("mo-plugin-library", move || *lock(&slots.library) = Some(library::read()));
    }

    /// Takes in what background threads found, frees sounds done playing, and runs Slint's timers.
    fn tick(&self) {
        if let Some(read) = take(&self.slots.library) {
            match read {
                Read::Ready(library) => {
                    let count = library.entries.len();
                    let all = (0..count).filter_map(|i| u32::try_from(i).ok()).collect();
                    self.lines.show(Some(Arc::clone(&library)), all);
                    self.panel.set_status(count_line(count, None).into());
                    let words = self.words.borrow().clone();
                    if !words.trim().is_empty() {
                        search(library, words, Arc::clone(&self.slots));
                    }
                }
                Read::Not(why) => self.panel.set_status(why.into()),
            }
        }
        if let Some((words, found)) = take(&self.slots.found)
            && words == *self.words.borrow()
        {
            let library = self.lines.library.borrow().clone();
            let total = library.as_ref().map_or(0, |l| l.entries.len());
            self.panel.set_status(count_line(total, (!words.trim().is_empty()).then_some(found.len())).into());
            self.panel.set_picked(-1);
            self.lines.show(library, found);
        }
        if let Some(played) = take(&self.slots.played) {
            self.panel.set_playing(
                match played {
                    Played::Playing(name) => format!("Playing {name}"),
                    Played::Not(why) => why,
                }
                .into(),
            );
        }
        self.instance.player.tidy();
        self.render();
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

/// Searches `library` for `words` on a background thread; the matches land in `slots.found`.
fn search(library: Arc<Library>, words: String, slots: Arc<Slots>) {
    background("mo-plugin-search", move || {
        let found = library.search(&words);
        *lock(&slots.found) = Some((words, found));
    });
}

/// Runs `work` on a thread of its own at low priority (never the window's thread).
fn background(name: &str, work: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
        work();
    });
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
