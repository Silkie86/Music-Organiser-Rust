//! Drawing the panel on the graphics card.
//!
//! The app draws its window through the graphics card with wgpu, and its own views (the Galaxy) with it too. The
//! plugin does the same in the window the DAW lends it: Slint draws the panel into the window's next picture on the
//! card ([`Gpu`]), so a big window scrolls and resizes without the processor filling every pixel. When there's no
//! card to draw with (or only a software one), or anything about it fails, the window draws on the processor as
//! before (`window.rs`).
//!
//! The card is looked for once for every copy of the plugin in the DAW, on a background thread as the first copy is
//! made ([`hold`]), and let go with the last copy.

use crate::threads::lock;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use slint::platform::femtovg_renderer::FemtoVGWGPURenderer;
use slint::platform::{Renderer, WindowAdapter};
use slint::wgpu_30::{WGPUSettings, wgpu};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::{Rc, Weak as RcWeak};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// The graphics card as the plugin uses it: shared by every plugin window in the DAW.
struct Card {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// How the look for the card stands.
enum Look {
    NotStarted,
    /// A background thread is looking now.
    Going,
    Found(Arc<Card>),
    /// There's none to draw with, and why.
    None(String),
}

struct Shared {
    look: Look,
    /// How many copies of the plugin are loaded: the card is let go with the last.
    held: usize,
}

static CARD: Mutex<Shared> = Mutex::new(Shared { look: Look::NotStarted, held: 0 });
/// Told when a look ends.
static LOOKED: Condvar = Condvar::new();

/// The longest a window waits for a look still going before it draws on the processor for now: it's the DAW's own
/// thread that waits. The window moves to the card once it's found ([`found`]). (On Solomon's PC the first look
/// took 3.7 s, 11 Nov 2026.)
const LONGEST_WAIT: Duration = Duration::from_millis(100);

/// One copy of the plugin's hold on the card: while any copy has one the card found stays, so a window opened again
/// doesn't look for it again.
pub struct Held(());

/// Starts looking for the card on a background thread, if no look has been made: finding one and making its device
/// takes a while on Windows (loading the drivers), and a DAW makes the plugin a moment before it opens its window.
pub fn hold() -> Held {
    let mut shared = lock(&CARD);
    shared.held += 1;
    if matches!(shared.look, Look::NotStarted) && std::env::var_os("MO_DRAW_CPU").is_none() {
        shared.look = Look::Going;
        // At normal priority, unlike other background work: a window may be waiting for it.
        let started = std::thread::Builder::new().name("mo-plugin-graphics".into()).spawn(|| {
            // A fault in a driver while looking counts as no card.
            let found = mo_catch::catch_crash(find).unwrap_or_else(|_| Err("looking for it crashed".into()));
            let mut shared = lock(&CARD);
            shared.look = match found {
                // Every copy went while it looked: nothing is kept.
                _ if shared.held == 0 => Look::NotStarted,
                Ok(card) => Look::Found(card),
                Err(why) => Look::None(why),
            };
            LOOKED.notify_all();
        });
        if started.is_err() {
            shared.look = Look::None("no thread to look for it on".into());
        }
    }
    Held(())
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut shared = lock(&CARD);
        shared.held = shared.held.saturating_sub(1);
        if shared.held == 0 && !matches!(shared.look, Look::Going) {
            // Windows still open keep their own hold on it; the next copy looks again.
            shared.look = Look::NotStarted;
        }
    }
}

/// Whether a card has been found and is ready to draw with.
pub fn found() -> bool {
    matches!(lock(&CARD).look, Look::Found(_))
}

/// The card: found already, or waited for while a look is going (never longer than [`LONGEST_WAIT`]). An error is
/// why there's none to draw with, for the log.
fn card() -> Result<Arc<Card>, String> {
    let began = Instant::now();
    let mut shared = lock(&CARD);
    loop {
        match &shared.look {
            Look::Found(card) => return Ok(Arc::clone(card)),
            Look::None(why) => return Err(why.clone()),
            Look::NotStarted => return Err("no look for a graphics card was started".into()),
            Look::Going => {
                let left = LONGEST_WAIT.saturating_sub(began.elapsed());
                if left.is_zero() {
                    return Err("still looking for the graphics card".into());
                }
                shared = LOOKED.wait_timeout(shared, left).map_or_else(|e| e.into_inner().0, |(guard, _)| guard);
            }
        }
    }
}

/// Looks for a card that draws and makes its device. One graphics interface at a time: on Windows DirectX 12 first
/// (it's Windows' own, so nothing else that hooks into graphics is loaded into the DAW with it), then Vulkan. Not
/// OpenGL: making an OpenGL one on Windows opens a hidden window of its own.
fn find() -> Result<Arc<Card>, String> {
    let tries: &[wgpu::Backends] =
        if cfg!(windows) { &[wgpu::Backends::DX12, wgpu::Backends::VULKAN] } else { &[wgpu::Backends::PRIMARY] };
    let mut why = "no graphics interface answered".to_owned();
    for &backends in tries {
        match find_on(backends) {
            Ok(card) => return Ok(Arc::new(card)),
            Err(e) => why = e,
        }
        // `WGPU_BACKEND` names the interfaces itself: every try would be the same one.
        if std::env::var_os("WGPU_BACKEND").is_some() {
            break;
        }
    }
    Err(why)
}

fn find_on(backends: wgpu::Backends) -> Result<Card, String> {
    let began = Instant::now();
    let mut asked = wgpu::InstanceDescriptor::new_without_display_handle();
    asked.backends = backends;
    // As the app asks: the drivers' own checks stay off (they slow every frame); wgpu checks what's asked itself.
    asked.flags = wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
    let instance = wgpu::Instance::new(asked.with_env());
    let options =
        wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() };
    let adapter = wait(instance.request_adapter(&options)).map_err(|e| format!("no graphics card ({e})"))?;
    let info = adapter.get_info();
    if info.device_type == wgpu::DeviceType::Cpu && std::env::var_os("SLINT_WGPU_CPU").is_none() {
        // A software card is slower than Slint's own drawing on the processor (as the app has it).
        return Err("only a software graphics card".into());
    }
    // What Slint itself asks for (its default settings): no extra features, and pictures as big as the screen.
    let settings = WGPUSettings::default();
    let wanted = wgpu::DeviceDescriptor {
        label: None,
        required_features: settings.device_required_features,
        required_limits: settings.device_required_limits.clone().using_resolution(adapter.limits()),
        experimental_features: settings.device_experimental_features,
        memory_hints: settings.device_memory_hints.clone(),
        trace: wgpu::Trace::default(),
    };
    let (device, queue) =
        wait(adapter.request_device(&wanted)).map_err(|e| format!("the card wouldn't give a device ({e})"))?;
    // A fault the card reports later (a driver reset, say) is written down, not left to end the DAW.
    device.on_uncaptured_error(Arc::new(|e: wgpu::Error| crate::log::line("Graphics", &format!("The card said: {e}"))));
    crate::log::took("Graphics", &format!("Found the graphics card: {} ({:?})", info.name, info.backend), began);
    Ok(Card { instance, adapter, device, queue })
}

/// The answer of a wgpu future. On a PC wgpu answers at once (the work is done by the time it's asked), so this only
/// ever waits a moment (as the app's own `gpu.rs` has it).
fn wait<T>(f: impl Future<Output = T>) -> T {
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// A plugin window drawn on the graphics card: Slint's window, its renderer, and the window's pictures on the card.
pub struct Gpu {
    window: slint::Window,
    renderer: FemtoVGWGPURenderer,
    surface: wgpu::Surface<'static>,
    config: RefCell<wgpu::SurfaceConfiguration>,
    card: Arc<Card>,
    size: Cell<slint::PhysicalSize>,
    /// Slint asked for the panel to be drawn again.
    redraw: Cell<bool>,
    /// No picture could be had to draw in (the window is hidden or covered): not tried again before this, so a hidden
    /// window never holds the DAW's thread up asking the card over and over.
    not_before: Cell<Option<Instant>>,
}

/// How long drawing waits before trying again after the card had no picture of the window to give.
const TRY_AGAIN: Duration = Duration::from_millis(250);

impl Gpu {
    /// Sets the window with this handle up to be drawn on the card, `width` by `height` pixels. An error is why it
    /// can't be, for the log: the window draws on the processor then.
    ///
    /// The window must outlive what this gives: `window.rs` keeps it in the window's own handler, which baseview
    /// drops before it closes the window.
    pub fn new(handle: &(impl HasWindowHandle + HasDisplayHandle), width: u32, height: u32) -> Result<Rc<Gpu>, String> {
        let card = card()?;
        let surface = surface(&card.instance, handle)?;
        let caps = surface.get_capabilities(&card.adapter);
        // A plain (not sRGB) format when there is one: Slint's colours are sRGB already, as its own windows pick.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or("the card can't draw in this window")?;
        let mut config = surface
            .get_default_config(&card.adapter, width.max(1), height.max(1))
            .ok_or("the card can't draw in this window")?;
        config.format = format;
        // In step with the screen: nothing tears, and the card isn't asked for more frames than are shown.
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&card.device, &config);
        let renderer = FemtoVGWGPURenderer::new(card.instance.clone(), card.device.clone(), card.queue.clone())
            .map_err(|e| format!("Slint couldn't draw on the card ({e})"))?;
        Ok(Rc::new_cyclic(|me: &RcWeak<Gpu>| Gpu {
            window: slint::Window::new(me.clone() as RcWeak<dyn WindowAdapter>),
            renderer,
            surface,
            config: RefCell::new(config),
            card,
            size: Cell::new(slint::PhysicalSize::new(width.max(1), height.max(1))),
            redraw: Cell::new(true),
            not_before: Cell::new(None),
        }))
    }

    /// The window is now `width` by `height` pixels.
    pub fn resize(&self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        {
            let mut config = self.config.borrow_mut();
            if (config.width, config.height) != (width, height) {
                config.width = width;
                config.height = height;
                self.surface.configure(&self.card.device, &config);
            }
        }
        let size = slint::PhysicalSize::new(width, height);
        self.size.set(size);
        // A new size is drawn at once: the window is plainly on show.
        self.not_before.set(None);
        let scale = self.window.scale_factor();
        self.window.dispatch_event(slint::platform::WindowEvent::Resized { size: size.to_logical(scale) });
        self.redraw.set(true);
    }

    /// The card's device and its queue, for what's drawn on the card beside the panel itself (the Galaxy's stars).
    pub fn device(&self) -> (&wgpu::Device, &wgpu::Queue) {
        (&self.card.device, &self.card.queue)
    }

    /// Draws the panel into the window if Slint asked for it since the last time. An error is for the log: the
    /// frame is skipped.
    pub fn draw_if_needed(&self) -> Result<(), String> {
        if !self.redraw.get() || self.not_before.get().is_some_and(|at| Instant::now() < at) {
            return Ok(());
        }
        let Some(frame) = self.frame() else {
            // No picture to draw in just now (the window is hidden, or changing size): asked again in a while.
            self.not_before.set(Some(Instant::now() + TRY_AGAIN));
            return Ok(());
        };
        self.redraw.set(false);
        self.not_before.set(None);
        let config = self.config.borrow();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let drawn = self.renderer.render_to_texture_view(&view, config.width, config.height, config.format);
        drop(view);
        self.card.queue.present(frame);
        drawn.map_err(|e| e.to_string())
    }

    /// The window's next picture on the card, set up again once if the window changed under it.
    fn frame(&self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Validation => None,
            stale @ (wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Suboptimal(_)
            | wgpu::CurrentSurfaceTexture::Lost) => {
                // Let go of first: the card won't set the window up again while a picture of it is held.
                drop(stale);
                self.surface.configure(&self.card.device, &self.config.borrow());
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) => Some(frame),
                    _ => None,
                }
            }
        }
    }
}

impl WindowAdapter for Gpu {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn size(&self) -> slint::PhysicalSize {
        self.size.get()
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn request_redraw(&self) {
        self.redraw.set(true);
    }
}

/// The window's pictures on the card.
#[allow(unsafe_code)]
fn surface(
    instance: &wgpu::Instance,
    handle: &(impl HasWindowHandle + HasDisplayHandle),
) -> Result<wgpu::Surface<'static>, String> {
    let window = handle.window_handle().map_err(|e| format!("no window to draw in ({e})"))?.as_raw();
    let display = handle.display_handle().map_err(|e| format!("no screen to draw on ({e})"))?.as_raw();
    let target = wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(display), raw_window_handle: window };
    // SAFETY: the handles are the plugin window's own, which is open now, and the surface is dropped before the
    // window closes: it lives in `Gpu`, which the window's handler owns, and baseview drops the handler first.
    unsafe { instance.create_surface_unsafe(target) }.map_err(|e| format!("the card can't draw in this window ({e})"))
}
