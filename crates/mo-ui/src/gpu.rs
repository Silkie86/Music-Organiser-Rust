//! The graphics card, for the views the app draws itself (the galaxy, the arrangement, then the waveforms). Slint
//! draws the window with wgpu, and once the window is up it hands over the same device; each view draws into a
//! picture on the card that Slint shows like any other image. Where there's no card to draw with (an old driver,
//! the screenshots, the tests, or `MO_DRAW_CPU=1` to compare), the views draw on the processor instead, as before.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::task::{Context, Poll, Waker};

use mo_app::log;
use slint::wgpu_30::{WGPUConfiguration, WGPUSettings, wgpu};
use slint::{ComponentHandle, GraphicsAPI, RenderingState};

use crate::AppWindow;

/// The graphics card Slint draws with, and its queue.
#[derive(Clone)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

thread_local! {
    static GPU: RefCell<Option<Gpu>> = const { RefCell::new(None) };
    /// Counts the devices handed over, so a view knows when what it made on the card belongs to an old one.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// When Slint began drawing the frame it's drawing now.
    static DRAWING_SINCE: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

/// Asks Slint to draw the window with wgpu, before any window is made, on the fastest card there is. When that can't
/// be done, or `MO_DRAW_CPU` is set, the window is drawn the usual way and the views draw on the processor.
pub fn choose() {
    if std::env::var_os("MO_DRAW_CPU").is_some() {
        log::info("Graphics", "MO_DRAW_CPU is set: the views draw on the processor");
        return;
    }
    // Slint only finds out whether there's a card once the window is being made, too late to draw the usual way
    // instead, so the card is looked for first.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let options =
        wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() };
    match wait(instance.request_adapter(&options)) {
        Ok(adapter) if adapter.get_info().device_type != wgpu::DeviceType::Cpu => {}
        Ok(_) => {
            log::warning("Graphics", "Only a software graphics card here, so the views draw on the processor");
            return;
        }
        Err(e) => {
            log::warning(
                "Graphics",
                &format!("No graphics card to draw with, so the views draw on the processor: {e}"),
            );
            return;
        }
    }
    let mut settings = WGPUSettings::default();
    settings.power_preference = wgpu::PowerPreference::HighPerformance;
    if let Err(e) = slint::BackendSelector::new().require_wgpu_30(WGPUConfiguration::Automatic(settings)).select() {
        log::warning(
            "Graphics",
            &format!("Couldn't draw with the graphics card, so the views draw on the processor: {e}"),
        );
    }
}

/// Takes the card from Slint once the window is up, and lets it go when the window does. Each time a card is handed
/// over, the views draw again on it. It also times each frame Slint draws, for the `[Frames]` lines.
pub fn connect(app: &AppWindow) {
    let weak = app.as_weak();
    let set = app.window().set_rendering_notifier(move |state, api| match state {
        RenderingState::RenderingSetup => {
            let GraphicsAPI::WGPU30 { device, queue, .. } = api else { return };
            let info = device.adapter_info();
            log::info("Graphics", &format!("Drawing with {} ({:?}, {:?})", info.name, info.backend, info.device_type));
            GPU.with(|g| *g.borrow_mut() = Some(Gpu { device: device.clone(), queue: queue.clone() }));
            GENERATION.with(|n| n.set(n.get() + 1));
            // Not from inside Slint's drawing: the views draw at their next turn.
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    crate::sounds::graphics_ready(&app);
                    crate::arrangement::graphics_ready(&app);
                }
            });
        }
        // Frame timing (the `[Frames]` lines): how long Slint takes to lay out and draw the window, marked for the
        // freeze watchdog too. Only measures; it changes nothing that's drawn.
        RenderingState::BeforeRendering => {
            DRAWING_SINCE.with(|d| d.set(Some(std::time::Instant::now())));
            crate::watchdog::drawing(true);
        }
        RenderingState::AfterRendering => {
            crate::watchdog::drawing(false);
            if let Some(since) = DRAWING_SINCE.with(Cell::take) {
                mo_app::frame_timing::drawn(since.elapsed(), crate::page_name());
            }
        }
        RenderingState::RenderingTeardown => {
            GPU.with(|g| *g.borrow_mut() = None);
            GENERATION.with(|n| n.set(n.get() + 1));
            // What the views made on the card goes with it, pictures on show included.
            if let Some(app) = weak.upgrade() {
                crate::sounds::graphics_gone(&app);
                crate::arrangement::graphics_gone(&app);
            }
        }
        _ => {}
    });
    if let Err(e) = set {
        log::info("Graphics", &format!("No graphics card to draw the views with: {e:?}"));
    }
}

/// The card, if there is one now, and which one it is (see [`generation`]).
pub fn current() -> Option<(Gpu, u64)> {
    GPU.with(|g| g.borrow().clone()).map(|g| (g, generation()))
}

/// Which card was handed over last: what a view made on the card is only good while this stays the same.
pub fn generation() -> u64 {
    GENERATION.with(Cell::get)
}

/// Runs `work` on the card, and says what went wrong if the card refused it (a mistake in a shader, or no memory left
/// on the card), instead of the app stopping.
pub fn checked<T>(device: &wgpu::Device, work: impl FnOnce() -> T) -> Result<T, String> {
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let valid = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out = work();
    let invalid = wait(valid.pop());
    let full = wait(memory.pop());
    match invalid.or(full) {
        Some(e) => Err(e.to_string()),
        None => Ok(out),
    }
}

/// The answer of a wgpu future. On a PC wgpu answers at once (the work is done by the time it's asked), so this only
/// ever waits a moment.
pub(crate) fn wait<T>(f: impl Future<Output = T>) -> T {
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            return v;
        }
        std::thread::yield_now();
    }
}
